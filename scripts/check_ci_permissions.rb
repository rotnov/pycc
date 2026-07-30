#!/usr/bin/env ruby
# frozen_string_literal: true

require "pathname"
require "psych"
require "digest"
require "json"
require "open3"

class PolicyError < StandardError; end

SAFE_PERMISSION_VALUES = %w[read none].freeze
WORKFLOW_DIRECTORY = Pathname(".github/workflows")
TRUST_ANCHOR_FILENAME = "workflow-policy.yml"
TRUST_ANCHOR_SHA256_ALLOWLIST = %w[
  4dc12b9c053dbc94011ba86c32c7a103afe223582cc94e93ff79255dc6e5b2e6
  8636af7fe96f773f5f32d0e6e8d6d86433ceba6b509173e41cd8af138b413e43
].freeze
SEARCH_LEDGER_TRUST_ANCHOR_SHA256 =
  "8636af7fe96f773f5f32d0e6e8d6d86433ceba6b509173e41cd8af138b413e43"
STAGED_SEARCH_ACTIVATION_SHA256 = {
  "scripts/check_search_visibility_audit.py" =>
    "8b4c6680f6bcdadb55196ece98b1d480597ec3ee5c57b144839423b36708271d",
  "docs/SEARCH_QUERY_REGISTRY.json" =>
    "aad5421200b1719c5e826b4c9ad916ca1a9a3644a64ce0c43c9534a41f106c1c",
  "docs/SEARCH_VISIBILITY.md" =>
    "d09c3386b9b9c088cf6a0a69479d5f0bfe470edeca0fbdb47bec5af9c8a72d44",
  "docs/SEARCH_VISIBILITY_CHECKPOINTS.json" =>
    "c55b4a4f1a11025bdde26825bfe762fc243d62997edc2f72ab5725f80ded943b"
}.freeze
SEARCH_ROADMAP_PATH = "docs/ROADMAP.md"
SEARCH_ROADMAP_CHECKPOINTS = [
  "<!-- search-history-checkpoint: github_repository_search 108 " \
    "e1e44e137edce9300e75648e898b41dd3b8e25f13e06ba5264b8ee61b0fad433 -->",
  "<!-- search-history-checkpoint: github_repository_search 130 " \
    "3ebf1ad5457aef04840be6ce397bb4e03415ffdac04edcab3e8cde3a5a76bef5 -->"
].freeze
SEARCH_SUCCESSOR_EXECUTABLES = %w[
  scripts/check_ci_permissions.rb
  scripts/check_roadmap_evidence.rb
  scripts/check_search_visibility_audit.py
  scripts/test_check_ci_permissions.rb
  scripts/test_check_roadmap_evidence.rb
  scripts/test_check_search_visibility_audit.py
].freeze
SEARCH_ACTIVATION_PATHS =
  (STAGED_SEARCH_ACTIVATION_SHA256.keys + SEARCH_SUCCESSOR_EXECUTABLES +
    [SEARCH_ROADMAP_PATH]).uniq.freeze
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

def pull_request_head_data(event_path, repository_root)
  event = JSON.parse(Pathname(event_path).read)
  pull_request = event["pull_request"]
  unless pull_request.is_a?(Hash)
    raise PolicyError, "pull_request_target event is missing pull_request data"
  end
  number = pull_request["number"] || event["number"]
  head = pull_request["head"]
  head_sha = head["sha"] if head.is_a?(Hash)
  unless number.is_a?(Integer) && number.positive? &&
         head_sha.is_a?(String) && head_sha.match?(/\A[0-9a-f]{40}\z/)
    raise PolicyError, "pull_request_target event has an invalid PR number or head SHA"
  end

  _stdout, stderr, status = Open3.capture3(
    "git", "fetch", "--no-tags", "--depth=1", "origin",
    "refs/pull/#{number}/head",
    chdir: repository_root.to_s
  )
  unless status.success?
    raise PolicyError, "could not fetch candidate PR head as data: #{stderr.strip}"
  end
  fetched, stderr, status = Open3.capture3(
    "git", "rev-parse", "FETCH_HEAD", chdir: repository_root.to_s
  )
  unless status.success? && fetched.strip == head_sha
    raise PolicyError, "fetched candidate PR head does not match the event SHA: #{stderr.strip}"
  end

  SEARCH_ACTIVATION_PATHS.to_h do |relative|
    content, error, result = Open3.capture3(
      "git", "cat-file", "blob", "#{head_sha}:#{relative}",
      chdir: repository_root.to_s
    )
    unless result.success?
      raise PolicyError, "candidate PR head is missing #{relative}: #{error.strip}"
    end
    [relative, content.b]
  end
rescue Errno::ENOENT, JSON::ParserError => e
  raise PolicyError, "could not read pull_request_target event data: #{e.message}"
end

def validate_search_activation_transition(
  paths,
  event_name: ENV["GITHUB_EVENT_NAME"],
  event_path: ENV["GITHUB_EVENT_PATH"],
  repository_root: Pathname(__dir__).parent,
  data_loader: nil
)
  anchor = paths.find { |path| path.basename.to_s == TRUST_ANCHOR_FILENAME }
  return unless anchor
  return unless Digest::SHA256.file(anchor).hexdigest == SEARCH_LEDGER_TRUST_ANCHOR_SHA256
  return unless event_name == "pull_request_target"
  raise PolicyError, "pull_request_target event path is missing" unless event_path || data_loader

  candidate = if data_loader
                data_loader.call(SEARCH_ACTIVATION_PATHS)
              else
                pull_request_head_data(event_path, repository_root)
              end
  STAGED_SEARCH_ACTIVATION_SHA256.each do |relative, expected_digest|
    content = candidate[relative]
    unless content.is_a?(String) && Digest::SHA256.hexdigest(content) == expected_digest
      raise PolicyError,
            "search trust-anchor activation must preserve staged #{relative} byte-for-byte"
    end
    base_path = repository_root / relative
    unless base_path.file? &&
           Digest::SHA256.file(base_path).hexdigest == expected_digest
      raise PolicyError,
            "search trust-anchor activation disagrees with trusted base #{relative}"
    end
  end
  SEARCH_SUCCESSOR_EXECUTABLES.each do |relative|
    content = candidate[relative]
    base_path = repository_root / relative
    unless content.is_a?(String) && base_path.file? &&
           content.b == base_path.binread
      raise PolicyError,
            "search trust-anchor activation must preserve trusted successor " \
            "executable #{relative} byte-for-byte"
    end
  end
  roadmap = candidate[SEARCH_ROADMAP_PATH]
  unless roadmap.is_a?(String)
    raise PolicyError,
          "search trust-anchor activation is missing staged #{SEARCH_ROADMAP_PATH}"
  end
  checkpoints = roadmap.lines(chomp: true).select do |line|
    line.include?("search-history-checkpoint:")
  end
  unless checkpoints == SEARCH_ROADMAP_CHECKPOINTS
    raise PolicyError,
          "search trust-anchor activation must preserve the staged roadmap checkpoint projection"
  end
end

def main(arguments)
  paths = expand_paths(arguments)
  validate_policy_set(paths)
  validate_search_activation_transition(paths)
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
