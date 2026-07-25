#!/usr/bin/env ruby
# frozen_string_literal: true

require "pathname"
require "psych"
require "digest"

class PolicyError < StandardError; end

SAFE_PERMISSION_VALUES = %w[read none].freeze
WORKFLOW_DIRECTORY = Pathname(".github/workflows")
TRUST_ANCHOR_FILENAME = "workflow-policy.yml"
CI_WORKFLOW_FILENAME = "ci.yml"
LANGUAGE_TRACK_JOB = "language-track-policy"
LANGUAGE_TRACK_GATE_JOB = "ci-gate"
LANGUAGE_TRACK_STEP = "Check Python language-track consistency"
LANGUAGE_TRACK_GATE_STEP = "Fail unless every required job succeeded"
LANGUAGE_TRACK_CHECKOUT = "actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803"
LANGUAGE_TRACK_COMMANDS = [
  "/usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/python3 -I -B scripts/check_language_tracks.py",
  "/usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/python3 -I -B -m unittest discover -s scripts -p 'test_check_language_tracks.py'"
].freeze
LANGUAGE_TRACK_ENVIRONMENT = {
  "BASH_ENV" => "",
  "ENV" => "",
  "DYLD_INSERT_LIBRARIES" => "",
  "DYLD_LIBRARY_PATH" => "",
  "LD_PRELOAD" => "",
  "LD_LIBRARY_PATH" => "",
  "PYTHONHOME" => "",
  "PYTHONPATH" => ""
}.freeze
LANGUAGE_TRACK_ALLOWED_WORKFLOW_ENV = %w[
  CARGO_LLVM_COV_VERSION
  LLVM_VERSION
].freeze
LANGUAGE_TRACK_GATE_DEPENDENCIES = %w[
  build-test-coverage
  language-track-policy
  native-build-test
  cross-compile-build
  cross-compile-verify
].freeze
LANGUAGE_TRACK_GATE_CONDITION =
  LANGUAGE_TRACK_GATE_DEPENDENCIES.map do |dependency|
    "needs.#{dependency}.result != 'success'"
  end.join(" || ").freeze
LANGUAGE_TRACK_GATE_COMMANDS = [
  'echo "one or more required jobs did not succeed:"',
  %q[echo '${{ toJSON(needs) }}'],
  "exit 1"
].freeze
TRUST_ANCHOR_SHA256_ALLOWLIST = %w[
  4dc12b9c053dbc94011ba86c32c7a103afe223582cc94e93ff79255dc6e5b2e6
].freeze
TRUSTED_EVENT_AND_REF_GUARD = /\A(?:\$\{\{\s*)?github\.event_name\s*==\s*(['"])push\1\s*&&\s*github\.ref\s*==\s*(['"])refs\/heads\/main\2\s*(?:\}\})?\z/

def mapping_entries(node, context)
  raise PolicyError, "#{context} must be a mapping" unless node.is_a?(Psych::Nodes::Mapping)

  entries = {}
  node.children.each_slice(2) do |key_node, value_node|
    unless key_node.is_a?(Psych::Nodes::Scalar)
      raise PolicyError, "#{context} contains a non-scalar key"
    end

    key = key_node.value
    raise PolicyError, "#{context} contains duplicate key #{key.inspect}" if entries.key?(key)
    raise PolicyError, "#{context} contains unsupported YAML merge key" if key == "<<"

    entries[key] = value_node
  end
  entries
end

def scalar_value(node, context)
  raise PolicyError, "#{context} must be a scalar" unless node.is_a?(Psych::Nodes::Scalar)

  node.value
end

def permission_entries(node, context)
  entries = mapping_entries(node, context)
  entries.transform_values do |value_node|
    scalar_value(value_node, "#{context} value").strip
  end
end

def trigger_names(node)
  case node
  when Psych::Nodes::Scalar
    [node.value]
  when Psych::Nodes::Sequence
    node.children.map { |child| scalar_value(child, "workflow trigger") }
  when Psych::Nodes::Mapping
    mapping_entries(node, "workflow triggers").keys
  else
    raise PolicyError, "workflow triggers must be a scalar, sequence, or mapping"
  end
end

def scalar_tree_values(node)
  values = []
  stack = [node]
  until stack.empty?
    current = stack.pop
    values << current.value if current.is_a?(Psych::Nodes::Scalar)
    children = current.children if current.respond_to?(:children)
    stack.concat(children) if children
  end
  values
end

def tree_contains_alias?(node)
  stack = [node]
  until stack.empty?
    current = stack.pop
    return true if current.is_a?(Psych::Nodes::Alias)

    children = current.children if current.respond_to?(:children)
    stack.concat(children) if children
  end
  false
end

def secret_reference?(value)
  expressions = value.scan(/\$\{\{(.*?)\}\}/m).flatten
  return true if expressions.any? { |expression| expression.match?(/\bsecrets\b/i) }

  value.strip.match?(/\A!?\s*secrets(?:\s*(?:\.|\[)|\z)/i)
end

def trusted_ref_guard?(node)
  return false unless node

  guard = scalar_value(node, "job if").strip
  TRUSTED_EVENT_AND_REF_GUARD.match?(guard)
end

def privileged_job?(entries, job_node, context)
  permissions = entries["permissions"]
  if permissions
    permission_entries(permissions, "#{context} permissions").each_value do |value|
      return true if value == "write"
      unless SAFE_PERMISSION_VALUES.include?(value)
        raise PolicyError, "#{context} has invalid permission value #{value.inspect}"
      end
    end
  end

  return true if entries.key?("environment") || entries.key?("secrets")

  scalar_tree_values(job_node).any? do |value|
    secret_reference?(value)
  end
end

def parse_workflow(text, source)
  stream = Psych.parse_stream(text, filename: source)
  if stream.children.length != 1 || stream.children.first.root.nil?
    raise PolicyError, "workflow must contain exactly one YAML document"
  end

  mapping_entries(stream.children.first.root, "workflow")
end

def empty_configuration?(node)
  (node.is_a?(Psych::Nodes::Scalar) && node.value.to_s.empty?) ||
    (node.is_a?(Psych::Nodes::Mapping) && node.children.empty?)
end

def validate_language_track_ci_contract(text, source = CI_WORKFLOW_FILENAME)
  root = parse_workflow(text, source)
  raise PolicyError, "#{source}: workflow-level defaults can replace the guard shell" if root.key?("defaults")
  inherited_environment = root["env"]
  if inherited_environment
    inherited_entries = mapping_entries(inherited_environment, "#{source} workflow env")
    unexpected = inherited_entries.keys - LANGUAGE_TRACK_ALLOWED_WORKFLOW_ENV
    unless unexpected.empty?
      raise PolicyError,
            "#{source}: workflow env can redirect the language-track guard: #{unexpected.join(", ")}"
    end
  end

  triggers = root["on"]
  raise PolicyError, "#{source}: missing workflow triggers" unless triggers
  unless triggers.is_a?(Psych::Nodes::Mapping)
    raise PolicyError, "#{source}: language-track guard requires mapped triggers"
  end
  trigger_entries = mapping_entries(triggers, "#{source} triggers")
  pull_request = trigger_entries["pull_request"]
  unless pull_request && empty_configuration?(pull_request)
    raise PolicyError, "#{source}: language-track guard requires an unfiltered pull_request trigger"
  end

  jobs = root["jobs"]
  raise PolicyError, "#{source}: missing jobs mapping" unless jobs
  job_definitions = mapping_entries(jobs, "#{source} jobs")
  job = job_definitions[LANGUAGE_TRACK_JOB]
  raise PolicyError, "#{source}: missing #{LANGUAGE_TRACK_JOB} job" unless job
  job_entries = mapping_entries(job, "#{source} #{LANGUAGE_TRACK_JOB} job")
  allowed_job_keys = %w[name permissions runs-on steps timeout-minutes]
  unexpected_job_keys = job_entries.keys - allowed_job_keys
  unless unexpected_job_keys.empty?
    raise PolicyError,
          "#{source}: language-track job has execution-altering keys: " \
          "#{unexpected_job_keys.join(", ")}"
  end
  runner = job_entries["runs-on"]
  unless runner &&
         scalar_value(runner, "#{source} language-track runner") == "ubuntu-latest"
    raise PolicyError, "#{source}: language-track job must use ubuntu-latest"
  end

  steps = job_entries["steps"]
  unless steps.is_a?(Psych::Nodes::Sequence)
    raise PolicyError, "#{source}: language-track job steps must be a sequence"
  end
  parsed_steps = steps.children.map do |step|
    mapping_entries(step, "#{source} #{LANGUAGE_TRACK_JOB} step")
  end
  checkout = parsed_steps.first
  unless checkout && (checkout.keys - %w[uses with]).empty?
    raise PolicyError, "#{source}: language-track checkout has execution-altering keys"
  end
  checkout_uses = checkout && checkout["uses"]
  unless checkout_uses &&
         scalar_value(checkout_uses, "#{source} checkout uses") == LANGUAGE_TRACK_CHECKOUT
    raise PolicyError, "#{source}: language-track guard requires the pinned candidate checkout first"
  end
  checkout_with = checkout["with"]
  unless checkout_with
    raise PolicyError, "#{source}: language-track checkout must disable persisted credentials"
  end
  checkout_options = permission_entries(checkout_with, "#{source} checkout options")
  unless checkout_options == { "persist-credentials" => "false" }
    raise PolicyError, "#{source}: language-track checkout options changed"
  end

  matching_steps = parsed_steps.each_with_index.each_with_object([]) do |(entries, index), matches|
    name = entries["name"]
    next unless name && scalar_value(name, "#{source} step name") == LANGUAGE_TRACK_STEP

    matches << [index, entries]
  end
  unless matching_steps.length == 1 &&
         matching_steps.first[0] == 1
    raise PolicyError,
          "#{source}: requires exactly one #{LANGUAGE_TRACK_STEP.inspect} step " \
          "immediately after checkout"
  end

  step_entries = matching_steps.first[1]
  allowed_step_keys = %w[env name run timeout-minutes]
  unexpected_step_keys = step_entries.keys - allowed_step_keys
  unless unexpected_step_keys.empty?
    raise PolicyError,
          "#{source}: language-track step has execution-altering keys: " \
          "#{unexpected_step_keys.join(", ")}"
  end
  run = step_entries["run"]
  raise PolicyError, "#{source}: language-track step requires run commands" unless run
  environment = step_entries["env"]
  unless environment
    raise PolicyError, "#{source}: language-track step requires an isolated environment"
  end
  actual_environment = mapping_entries(
    environment,
    "#{source} language-track env"
  ).transform_values do |value_node|
    scalar_value(value_node, "#{source} language-track env value")
  end
  unless actual_environment == LANGUAGE_TRACK_ENVIRONMENT
    raise PolicyError, "#{source}: language-track step environment changed"
  end

  commands = scalar_value(run, "#{source} language-track run").lines.map(&:strip).reject(&:empty?)
  unless commands == LANGUAGE_TRACK_COMMANDS
    raise PolicyError, "#{source}: language-track step commands or order changed"
  end

  gate = job_definitions[LANGUAGE_TRACK_GATE_JOB]
  raise PolicyError, "#{source}: missing #{LANGUAGE_TRACK_GATE_JOB} job" unless gate
  gate_entries = mapping_entries(gate, "#{source} #{LANGUAGE_TRACK_GATE_JOB} job")
  unexpected_gate_keys =
    gate_entries.keys - %w[if needs permissions runs-on steps]
  unless unexpected_gate_keys.empty?
    raise PolicyError,
          "#{source}: #{LANGUAGE_TRACK_GATE_JOB} has execution-altering keys: " \
          "#{unexpected_gate_keys.join(", ")}"
  end
  gate_runner = gate_entries["runs-on"]
  unless gate_runner &&
         scalar_value(
           gate_runner,
           "#{source} #{LANGUAGE_TRACK_GATE_JOB} runner"
         ) == "ubuntu-latest"
    raise PolicyError, "#{source}: #{LANGUAGE_TRACK_GATE_JOB} must use ubuntu-latest"
  end
  gate_permissions = gate_entries["permissions"]
  unless gate_permissions &&
         mapping_entries(
           gate_permissions,
           "#{source} #{LANGUAGE_TRACK_GATE_JOB} permissions"
         ).empty?
    raise PolicyError,
          "#{source}: #{LANGUAGE_TRACK_GATE_JOB} must have empty permissions"
  end
  needs = gate_entries["needs"]
  unless needs.is_a?(Psych::Nodes::Sequence)
    raise PolicyError,
          "#{source}: #{LANGUAGE_TRACK_GATE_JOB} dependencies changed"
  end
  dependency_names = needs.children.map do |dependency|
    scalar_value(dependency, "#{source} #{LANGUAGE_TRACK_GATE_JOB} dependency")
  end
  unless dependency_names == LANGUAGE_TRACK_GATE_DEPENDENCIES
    raise PolicyError, "#{source}: #{LANGUAGE_TRACK_GATE_JOB} dependencies changed"
  end
  gate_if = gate_entries["if"]
  unless gate_if &&
         scalar_value(gate_if, "#{source} #{LANGUAGE_TRACK_GATE_JOB} if").strip == "always()"
    raise PolicyError,
          "#{source}: #{LANGUAGE_TRACK_GATE_JOB} must evaluate every dependency result"
  end
  gate_steps = gate_entries["steps"]
  unless gate_steps.is_a?(Psych::Nodes::Sequence) &&
         gate_steps.children.length == 1
    raise PolicyError,
          "#{source}: #{LANGUAGE_TRACK_GATE_JOB} must contain one fail-closed step"
  end
  gate_step = mapping_entries(
    gate_steps.children.first,
    "#{source} #{LANGUAGE_TRACK_GATE_JOB} step"
  )
  unless gate_step.keys.sort == %w[if name run]
    raise PolicyError,
          "#{source}: #{LANGUAGE_TRACK_GATE_JOB} fail-closed step changed"
  end
  step_name = scalar_value(
    gate_step["name"],
    "#{source} #{LANGUAGE_TRACK_GATE_JOB} step name"
  )
  condition = scalar_value(
    gate_step["if"],
    "#{source} #{LANGUAGE_TRACK_GATE_JOB} step if"
  ).split.join(" ")
  commands = scalar_value(
    gate_step["run"],
    "#{source} #{LANGUAGE_TRACK_GATE_JOB} step run"
  ).lines.map(&:strip).reject(&:empty?)
  if step_name == LANGUAGE_TRACK_GATE_STEP &&
     condition == LANGUAGE_TRACK_GATE_CONDITION &&
     commands == LANGUAGE_TRACK_GATE_COMMANDS
    return
  end

  raise PolicyError,
        "#{source}: #{LANGUAGE_TRACK_GATE_JOB} must use the exact fail-closed " \
        "predicate and command"
end

def validate_workflow(text, source = "(workflow)")
  root = parse_workflow(text, source)
  permissions_node = root["permissions"]
  raise PolicyError, "missing top-level permissions mapping" unless permissions_node

  permission_entries(permissions_node, "top-level permissions").each do |name, value|
    unless SAFE_PERMISSION_VALUES.include?(value)
      raise PolicyError, "privileged workflow-level permission #{name}: #{value}"
    end
  end

  triggers_node = root["on"]
  raise PolicyError, "missing workflow triggers" unless triggers_node

  trigger_names(triggers_node)

  if root.values.any? { |node| tree_contains_alias?(node) }
    raise PolicyError, "workflow contains an unsupported YAML alias"
  end

  root.each do |name, node|
    next if name == "jobs"
    next unless scalar_tree_values(node).any? { |value| secret_reference?(value) }

    raise PolicyError, "workflow references a secret outside a guarded job"
  end

  jobs_node = root["jobs"]
  raise PolicyError, "missing jobs mapping" unless jobs_node

  mapping_entries(jobs_node, "jobs").each do |job_name, job_node|
    entries = mapping_entries(job_node, "job #{job_name.inspect}")
    next unless privileged_job?(entries, job_node, "job #{job_name.inspect}")
    next if trusted_ref_guard?(entries["if"])

    raise PolicyError,
          "job #{job_name.inspect} is privileged without an exact push-and-main guard"
  end
end

def discover_workflows(directory = WORKFLOW_DIRECTORY)
  directory.children.select do |path|
    path.file? && %w[.yml .yaml].include?(path.extname)
  end.sort
end

def expand_paths(arguments)
  paths = arguments.empty? ? discover_workflows : arguments.flat_map do |argument|
    path = Pathname(argument)
    path.directory? ? discover_workflows(path) : [path]
  end
  raise PolicyError, "no workflow files found" if paths.empty?

  paths
end

def validate_policy_set(paths)
  anchors = paths.select { |path| path.basename.to_s == TRUST_ANCHOR_FILENAME }
  unless anchors.length == 1
    raise PolicyError, "workflow set must contain exactly one #{TRUST_ANCHOR_FILENAME}"
  end

  digest = Digest::SHA256.file(anchors.first).hexdigest
  return if TRUST_ANCHOR_SHA256_ALLOWLIST.include?(digest)

  raise PolicyError,
        "#{TRUST_ANCHOR_FILENAME} does not match an approved trust-anchor digest"
end

def validate_required_ci_contract(paths)
  ci_workflows = paths.select { |path| path.basename.to_s == CI_WORKFLOW_FILENAME }
  unless ci_workflows.length == 1
    raise PolicyError, "workflow set must contain exactly one #{CI_WORKFLOW_FILENAME}"
  end

  validate_language_track_ci_contract(ci_workflows.first.read, ci_workflows.first.to_s)
end

def main(arguments)
  paths = expand_paths(arguments)
  validate_policy_set(paths)
  validate_required_ci_contract(paths)
  failures = []
  paths.each do |path|
    begin
      validate_workflow(path.read, path.to_s)
    rescue Errno::ENOENT, Psych::SyntaxError, PolicyError => e
      failures << "#{path}: #{e.message}"
    end
  end

  unless failures.empty?
    warn failures.join("\n")
    return 1
  end

  puts "Workflow permission policy passed for #{paths.length} file(s)."
  0
rescue PolicyError => e
  warn e.message
  1
end

exit(main(ARGV)) if $PROGRAM_NAME == __FILE__
