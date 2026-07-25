#!/usr/bin/env ruby
# frozen_string_literal: true

require "pathname"
require "psych"

class RoadmapEvidenceError < StandardError; end

CHECKED_ITEM = /^\s*(?:>\s*)*(?:[-*+]|\d+[.)])\s+\[[xX]\]\s+(?<claim>.*)$/
EVIDENCE_MARKER = /<!--\s*roadmap-evidence:\s*(?<id>[a-z0-9][a-z0-9-]*)\s*-->/
EVIDENCE_CLAIMS = {
  "ci-build-test-coverage-100" =>
    "The 100% line and region coverage gate is required and green for the current slice."
}.freeze
COVERAGE_JOB = "build-test-coverage"
COVERAGE_STEP = "Hard coverage gate — 100% lines + regions (D-014)"
COVERAGE_COMMAND =
  "cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100"

def yaml_mapping(node, context)
  raise RoadmapEvidenceError, "#{context} must be a mapping" unless node.is_a?(Psych::Nodes::Mapping)

  entries = {}
  node.children.each_slice(2) do |key, value|
    unless key.is_a?(Psych::Nodes::Scalar)
      raise RoadmapEvidenceError, "#{context} contains a non-scalar key"
    end

    if entries.key?(key.value)
      raise RoadmapEvidenceError, "#{context} contains duplicate key #{key.value.inspect}"
    end

    entries[key.value] = value
  end
  entries
end

def yaml_scalar(node, context)
  raise RoadmapEvidenceError, "#{context} must be a scalar" unless node.is_a?(Psych::Nodes::Scalar)

  node.value
end

def coverage_gate_present?(workflow_text, source)
  stream = Psych.parse_stream(workflow_text, filename: source)
  if stream.children.length != 1 || stream.children.first.root.nil?
    raise RoadmapEvidenceError, "#{source} must contain exactly one YAML document"
  end

  root = yaml_mapping(stream.children.first.root, source)
  if root.key?("defaults")
    raise RoadmapEvidenceError,
          "#{source}: coverage evidence must not inherit run defaults"
  end

  triggers = yaml_mapping(root["on"], "#{source} triggers")
  pull_request = triggers["pull_request"]
  unfiltered_pull_request =
    (pull_request.is_a?(Psych::Nodes::Scalar) && pull_request.value.empty?) ||
    (pull_request.is_a?(Psych::Nodes::Mapping) && pull_request.children.empty?)
  unless unfiltered_pull_request
    raise RoadmapEvidenceError,
          "#{source}: coverage evidence must run on every pull request"
  end

  jobs = yaml_mapping(root["jobs"], "#{source} jobs")
  job = yaml_mapping(jobs[COVERAGE_JOB], "#{source} job #{COVERAGE_JOB.inspect}")
  if job.key?("defaults")
    raise RoadmapEvidenceError,
          "#{source}: coverage evidence must not inherit run defaults"
  end
  if job.key?("needs")
    raise RoadmapEvidenceError,
          "#{source}: coverage evidence must not depend on other jobs"
  end
  if job.key?("if")
    raise RoadmapEvidenceError, "#{source}: coverage evidence must run unconditionally"
  end
  job_continue_on_error = job["continue-on-error"]
  unsafe_job_continue_on_error =
    job_continue_on_error &&
    yaml_scalar(
      job_continue_on_error,
      "#{source} job #{COVERAGE_JOB.inspect} continue-on-error"
    ).strip != "false"
  if unsafe_job_continue_on_error
    raise RoadmapEvidenceError, "#{source}: coverage job must propagate failures"
  end

  steps = job["steps"]
  unless steps.is_a?(Psych::Nodes::Sequence)
    raise RoadmapEvidenceError, "#{source} job #{COVERAGE_JOB.inspect} steps must be a sequence"
  end

  steps.children.any? do |step_node|
    step = yaml_mapping(step_node, "#{source} step")
    next false unless step["name"] && step["run"]

    next false unless yaml_scalar(step["name"], "#{source} step name") == COVERAGE_STEP
    next false unless yaml_scalar(step["run"], "#{source} step run").strip == COVERAGE_COMMAND

    if step.key?("shell")
      raise RoadmapEvidenceError, "#{source}: coverage step must use the default shell"
    end

    continue_on_error = step["continue-on-error"]
    unsafe_continue_on_error =
      continue_on_error &&
      yaml_scalar(continue_on_error, "#{source} step continue-on-error").strip != "false"
    if step.key?("if") || unsafe_continue_on_error
      raise RoadmapEvidenceError, "#{source}: coverage evidence must run unconditionally"
    end

    true
  end
end

def validate_roadmap(text)
  evidence_ids = []
  text.each_line.with_index(1) do |line, line_number|
    item = CHECKED_ITEM.match(line)
    next unless item

    marker = EVIDENCE_MARKER.match(item[:claim])
    unless marker
      raise RoadmapEvidenceError,
            "line #{line_number}: checked roadmap item is missing an evidence marker"
    end

    expected_claim = EVIDENCE_CLAIMS[marker[:id]]
    unless expected_claim
      raise RoadmapEvidenceError,
            "line #{line_number}: unknown roadmap evidence #{marker[:id].inspect}"
    end

    actual_claim = item[:claim].sub(EVIDENCE_MARKER, "").strip
    if actual_claim == expected_claim
      evidence_ids << marker[:id]
      next
    end

    raise RoadmapEvidenceError,
          "line #{line_number}: evidence #{marker[:id].inspect} does not prove this roadmap claim"
  end
  evidence_ids
end

def validate_evidence(root, evidence_ids)
  return unless evidence_ids.include?("ci-build-test-coverage-100")

  workflow = root / ".github/workflows/ci.yml"
  return if coverage_gate_present?(workflow.read, workflow.to_s)

  raise RoadmapEvidenceError,
        "#{workflow}: evidence does not provide the exact 100% line and region gate"
end

def main(arguments)
  raise RoadmapEvidenceError, "usage: check_roadmap_evidence.rb [repository-root]" if arguments.length > 1

  root = Pathname(arguments.first || ".")
  roadmap = root / "docs/ROADMAP.md"
  evidence_ids = validate_roadmap(roadmap.read)
  validate_evidence(root, evidence_ids)
  puts "Roadmap evidence policy passed."
  0
rescue Errno::ENOENT, Psych::SyntaxError, RoadmapEvidenceError => e
  warn e.message
  1
end

exit(main(ARGV)) if $PROGRAM_NAME == __FILE__
