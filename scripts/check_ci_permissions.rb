#!/usr/bin/env ruby
# frozen_string_literal: true

require "pathname"
require "psych"
require "digest"

class PolicyError < StandardError; end

SAFE_PERMISSION_VALUES = %w[read none].freeze
WORKFLOW_DIRECTORY = Pathname(".github/workflows")
TRUST_ANCHOR_FILENAME = "workflow-policy.yml"
TRUST_ANCHOR_SHA256_ALLOWLIST = %w[
  f8d60936438c48362d0a5dc11ee709c9dd5354c3f697038bc36b620c266f0688
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

def main(arguments)
  paths = expand_paths(arguments)
  validate_policy_set(paths)
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
