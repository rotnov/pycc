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
  "/Users/runner/.cargo/bin/cargo-llvm-cov llvm-cov --workspace " \
  "--fail-under-lines 100 --fail-under-regions 100"
COVERAGE_SCRIPT = <<~SHELL.strip
  set -euo pipefail
  LLVM_SYS_221_PREFIX_VALUE="$(brew --prefix llvm@22)"
  export LLVM_SYS_221_PREFIX="$LLVM_SYS_221_PREFIX_VALUE"
  /Users/runner/.cargo/bin/cargo build --workspace
  cd "$RUNNER_TEMP"
  /Users/runner/.cargo/bin/cargo install cargo-llvm-cov --locked --version "${CARGO_LLVM_COV_VERSION}"
  cd "$GITHUB_WORKSPACE"
  /Users/runner/.cargo/bin/cargo-llvm-cov llvm-cov --version
  #{COVERAGE_COMMAND}
  printf 'LLVM_SYS_221_PREFIX=%s\\n' "$LLVM_SYS_221_PREFIX_VALUE" >> "$GITHUB_ENV"
SHELL
TRUSTED_COVERAGE_ENV = {
  "CARGO_LLVM_COV_VERSION" => "0.8.7"
}.freeze
TRUSTED_COVERAGE_STEPS = [
  {
    "uses" => "actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803",
    "with" => { "persist-credentials" => "false" }
  },
  {
    "name" => "Show pinned toolchain",
    "run" => "rustup show"
  },
  {
    "name" => "Install LLVM 22 (D-015)",
    "run" => "brew install llvm@22"
  },
  {
    "name" => "Install llvm-tools-preview",
    "run" => "rustup component add llvm-tools-preview"
  },
  {
    "name" => COVERAGE_STEP,
    "run" => COVERAGE_SCRIPT
  }
].freeze

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

def yaml_value(node, context)
  case node
  when Psych::Nodes::Scalar
    node.value.strip
  when Psych::Nodes::Mapping
    yaml_mapping(node, context).transform_values do |value|
      yaml_value(value, context)
    end
  when Psych::Nodes::Sequence
    node.children.map { |value| yaml_value(value, context) }
  else
    raise RoadmapEvidenceError, "#{context} contains an unsupported YAML value"
  end
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
  workflow_env = root["env"] ? yaml_value(root["env"], "#{source} environment") : {}
  unless workflow_env == TRUSTED_COVERAGE_ENV
    raise RoadmapEvidenceError,
          "#{source}: coverage workflow environment does not match the trusted values"
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
  if job.key?("env")
    raise RoadmapEvidenceError,
          "#{source}: coverage job environment does not match the trusted values"
  end
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
  allowed_job_keys = %w[runs-on steps continue-on-error]
  unless (job.keys - allowed_job_keys).empty?
    raise RoadmapEvidenceError,
          "#{source}: coverage job contains untrusted configuration"
  end
  unless yaml_scalar(job["runs-on"], "#{source} coverage runner") == "macos-14"
    raise RoadmapEvidenceError,
          "#{source}: coverage job must use the trusted runner"
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

  coverage_index = nil
  steps.children.each_with_index do |step_node, index|
    step = yaml_mapping(step_node, "#{source} step")
    next unless step["name"] && step["run"]

    next unless yaml_scalar(step["name"], "#{source} step name") == COVERAGE_STEP
    next unless yaml_scalar(step["run"], "#{source} step run").strip == COVERAGE_SCRIPT

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

    coverage_index = index
    break
  end
  return false unless coverage_index

  actual_prefix = steps.children.first(coverage_index + 1).map do |step_node|
    step = yaml_value(step_node, "#{source} coverage setup step")
    step.delete("continue-on-error") if step["continue-on-error"] == "false"
    step
  end
  unless actual_prefix == TRUSTED_COVERAGE_STEPS
    raise RoadmapEvidenceError,
          "#{source}: coverage setup steps do not match the trusted sequence"
  end

  true
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
