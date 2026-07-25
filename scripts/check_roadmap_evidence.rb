#!/usr/bin/env ruby
# frozen_string_literal: true

require "pathname"
require "psych"
require "digest"

class RoadmapEvidenceError < StandardError; end

LIST_ITEM =
  /\A(?<indent>[ \t]*)(?<marker>[-*+]|\d+[.)])(?:(?<spacing>[ \t]+)(?<body>[^\r\n]*))?(?:\r?\n)?\z/
CHECKED_ITEM_BODY = /\A\[[xX]\][ \t]+(?<claim>.*)$/
ATX_HEADING = /\A {0,3}(?<marks>\#{1,6})[ \t]+(?<title>.*)$/
SETEXT_UNDERLINE = /\A {0,3}(?:=+|-+)[ \t]*(?:\r?\n)?\z/
RAW_HTML_BLOCK_START =
  /\A {0,3}<(?:\/?[A-Za-z][A-Za-z0-9-]*(?=[ \t\/>])|\?|![A-Z]|!\[CDATA\[)/i
EVIDENCE_MARKER = /<!--\s*roadmap-evidence:\s*(?<id>[a-z0-9][a-z0-9-]*)\s*-->/
EVIDENCE_CLAIMS = {
  "ci-tier1-cross-compile" =>
    "The five-target native CI matrix and one cross-host compilation path are live on `main`.",
  "ci-build-test-coverage-100" =>
    "The 100% line and region coverage gate is required and green for the current slice."
}.freeze
EVIDENCE_SECTIONS = {
  "ci-tier1-cross-compile" => [
    "pycc Roadmap",
    "Current delivery status",
    "v0.1 acceptance checklist"
  ],
  "ci-build-test-coverage-100" => [
    "pycc Roadmap",
    "Current delivery status",
    "v0.1 acceptance checklist"
  ]
}.freeze
TIER1_CI_WORKFLOW_SHA256S = [
  "b77ab0c1c3bcc69e69d3cb8f08e081f6eae246e7d5d19c9356455db1ff4291d2"
].freeze
COVERAGE_JOB = "build-test-coverage"
COVERAGE_STEP = "Hard coverage gate — 100% lines + regions (D-014)"
COVERAGE_COMMAND =
  "run_isolated \"$TRUSTED_COV\" llvm-cov --workspace " \
  "--fail-under-lines 100 --fail-under-regions 100"
COVERAGE_SCRIPT = <<~SHELL.strip
  set -euo pipefail
  LLVM_SYS_221_PREFIX_VALUE="$(brew --prefix llvm@22)"
  TRUSTED_CARGO="$(rustup which cargo)"
  TRUSTED_RUSTC="$(rustup which rustc)"
  TRUSTED_RUSTDOC="$(rustup which rustdoc)"
  TRUSTED_COV="/Users/runner/.cargo/bin/cargo-llvm-cov"
  TRUSTED_TOOLCHAIN="$(dirname "$(dirname "$TRUSTED_CARGO")")"
  cd "$RUNNER_TEMP"
  RUSTC="$TRUSTED_RUSTC" RUSTDOC="$TRUSTED_RUSTDOC" "$TRUSTED_CARGO" install cargo-llvm-cov --locked --version "${CARGO_LLVM_COV_VERSION}"
  "$TRUSTED_COV" llvm-cov --version
  sudo chmod o+x /Users/runner /Users/runner/.cargo /Users/runner/.cargo/bin /Users/runner/.rustup /Users/runner/.rustup/toolchains
  sudo chmod -R o+rX "$TRUSTED_TOOLCHAIN"
  sudo chmod o+rx "$TRUSTED_COV"
  ISOLATED_ROOT="$RUNNER_TEMP/pycc-coverage"
  mkdir -p "$ISOLATED_ROOT/home" "$ISOLATED_ROOT/tmp" "$ISOLATED_ROOT/cargo-home" "$ISOLATED_ROOT/target"
  sudo chown -R nobody:nobody "$ISOLATED_ROOT"
  ISOLATED_ENV=(
    "HOME=$ISOLATED_ROOT/home"
    "TMPDIR=$ISOLATED_ROOT/tmp/"
    "CARGO_HOME=$ISOLATED_ROOT/cargo-home"
    "CARGO_TARGET_DIR=$ISOLATED_ROOT/target"
    "CARGO=$TRUSTED_CARGO"
    "RUSTC=$TRUSTED_RUSTC"
    "RUSTDOC=$TRUSTED_RUSTDOC"
    "LLVM_SYS_221_PREFIX=$LLVM_SYS_221_PREFIX_VALUE"
    "PATH=$(dirname "$TRUSTED_CARGO"):/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"
  )
  run_isolated() {
    sudo -u nobody env -i "${ISOLATED_ENV[@]}" "$@"
  }
  ln -s "$ISOLATED_ROOT/target" "$GITHUB_WORKSPACE/target"
  cd "$GITHUB_WORKSPACE"
  run_isolated "$TRUSTED_CARGO" build --target x86_64-apple-darwin -p pycc_rt
  run_isolated "$TRUSTED_CARGO" build --workspace
  #{COVERAGE_COMMAND}
  rm "$GITHUB_WORKSPACE/target"
  printf 'LLVM_SYS_221_PREFIX=%s\\n' "$LLVM_SYS_221_PREFIX_VALUE" >> "$GITHUB_ENV"
SHELL
TRUSTED_COVERAGE_ENV = {
  "CARGO_LLVM_COV_VERSION" => "0.8.7",
  "LLVM_VERSION" => "22.1.1"
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
    "name" => "Add x86_64-apple-darwin Rust target",
    "run" => "rustup target add x86_64-apple-darwin"
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

def opening_fence(line)
  match = /\A {0,3}(?<marker>`{3,}|~{3,})(?<info>[^\r\n]*)(?:\r?\n)?\z/.match(line)
  return unless match

  marker = match[:marker]
  return if marker.start_with?("`") && match[:info].include?("`")

  [marker[0], marker.length]
end

def closing_fence?(line, fence)
  character, minimum_length = fence
  pattern =
    /\A {0,3}#{Regexp.escape(character)}{#{minimum_length},}[ \t]*(?:\r?\n)?\z/
  pattern.match?(line)
end

def without_html_comments(line, in_comment)
  visible = String.new
  cursor = 0
  loop do
    if in_comment
      closing = line.index("-->", cursor)
      return [visible, true] unless closing

      cursor = closing + 3
      in_comment = false
    else
      opening = line.index("<!--", cursor)
      unless opening
        visible << line[cursor..]
        return [visible, false]
      end

      visible << line[cursor...opening]
      cursor = opening + 4
      in_comment = true
    end
  end
end

def blockquote_content(line)
  content = line
  depth = 0
  loop do
    prefix = /\A {0,3}>[ \t]?/.match(content)
    break unless prefix

    content = content[prefix.end(0)..] || ""
    depth += 1
  end
  [content, depth]
end

def indentation_width(whitespace, initial_column = 0)
  whitespace.each_char.reduce(initial_column) do |column, character|
    character == "\t" ? column + (4 - (column % 4)) : column + 1
  end
end

def strip_indentation(line, columns)
  column = 0
  index = 0
  while column < columns
    character = line[index]
    return line unless character == " " || character == "\t"

    if character == "\t"
      next_column = column + (4 - (column % 4))
      if next_column > columns
        remainder = next_column - columns
        return (" " * remainder) + (line[(index + 1)..] || "")
      end
      column = next_column
    else
      column += 1
    end
    index += 1
  end
  line[index..] || ""
end

def list_container_content(line, containers)
  indent = indentation_width(line[/\A[ \t]*/])
  content_indent = containers.reverse.find { |candidate| indent >= candidate } || 0
  [strip_indentation(line, content_indent), content_indent]
end

def rendered_list_item(line, containers)
  item = LIST_ITEM.match(line)
  return unless item

  indent = indentation_width(item[:indent])
  parent_index = containers.rindex { |content_indent| indent >= content_indent }
  return if indent > 3 && parent_index.nil?

  containers.replace(parent_index ? containers.first(parent_index + 1) : [])
  marker_end = indent + item[:marker].length
  spacing = item[:spacing] || " "
  content_indent = indentation_width(spacing, marker_end)
  padding = content_indent - marker_end
  containers << (padding <= 4 ? content_indent : marker_end + 1)

  body = padding <= 4 ? (item[:body] || "") : nil
  {
    body: body,
    checked_item: body && CHECKED_ITEM_BODY.match(body),
    content_indent: containers.last
  }
end

def validate_roadmap(text)
  evidence_ids = []
  heading_path = []
  fence = nil
  in_html_comment = false
  list_containers = Hash.new { |containers, depth| containers[depth] = [] }
  text.each_line.with_index(1) do |line, line_number|
    if fence
      candidate, quote_depth = blockquote_content(line)
      candidate =
        if quote_depth == fence[2]
          strip_indentation(candidate, fence[3])
        else
          line
        end
      fence = nil if closing_fence?(candidate, fence)
      next
    end

    visible_line, in_html_comment = without_html_comments(line, in_html_comment)
    fence_candidate, quote_depth = blockquote_content(visible_line)
    containers = list_containers[quote_depth]
    normalized_container_content, content_indent =
      list_container_content(fence_candidate, containers)
    list_item = rendered_list_item(fence_candidate, containers)
    block_content = list_item ? list_item[:body] : normalized_container_content
    content_indent = list_item[:content_indent] if list_item

    if block_content && RAW_HTML_BLOCK_START.match?(block_content)
      raise RoadmapEvidenceError,
            "line #{line_number}: raw HTML blocks are not supported in the roadmap"
    end

    opening = block_content && opening_fence(block_content)
    if opening
      fence = [*opening, quote_depth, content_indent]
      next
    end

    if block_content && SETEXT_UNDERLINE.match?(block_content)
      raise RoadmapEvidenceError,
            "line #{line_number}: Setext headings are not supported; use ATX headings"
    end

    heading = block_content && ATX_HEADING.match(block_content)
    if heading
      list_containers.clear if quote_depth.zero? && content_indent.zero?
      level = heading[:marks].length
      title = heading[:title].sub(/[ \t]+#+[ \t]*$/, "").strip
      heading_path = heading_path.first(level - 1)
      heading_path[level - 1] = title
      next
    end

    item = list_item && list_item[:checked_item]
    item ||= CHECKED_ITEM_BODY.match(block_content) if !list_item && content_indent.positive?
    unless list_item || LIST_ITEM.match?(fence_candidate)
      if fence_candidate.strip.empty?
        next
      end

      indent = indentation_width(fence_candidate[/\A[ \t]*/])
      containers.pop while containers.any? && indent < containers.last
    end
    next unless item

    marker_ids = []
    line.scan(EVIDENCE_MARKER) { marker_ids << Regexp.last_match[:id] }
    if marker_ids.empty?
      raise RoadmapEvidenceError,
            "line #{line_number}: checked roadmap item is missing an evidence marker"
    end
    unless marker_ids.length == 1
      raise RoadmapEvidenceError,
            "line #{line_number}: checked roadmap item must contain exactly one evidence marker"
    end

    evidence_id = marker_ids.first
    expected_claim = EVIDENCE_CLAIMS[evidence_id]
    unless expected_claim
      raise RoadmapEvidenceError,
            "line #{line_number}: unknown roadmap evidence #{evidence_id.inspect}"
    end

    expected_section = EVIDENCE_SECTIONS.fetch(evidence_id)
    unless heading_path == expected_section
      raise RoadmapEvidenceError,
            "line #{line_number}: evidence #{evidence_id.inspect} must appear under " \
            "the expected roadmap section #{expected_section.join(' > ').inspect}"
    end

    actual_claim = item[:claim].strip
    if actual_claim == expected_claim
      evidence_ids << evidence_id
      next
    end

    raise RoadmapEvidenceError,
          "line #{line_number}: evidence #{evidence_id.inspect} does not prove this roadmap claim"
  end
  evidence_ids
end

def validate_evidence(root, evidence_ids)
  workflow = root / ".github/workflows/ci.yml"
  workflow_text = workflow.read
  if evidence_ids.include?("ci-tier1-cross-compile")
    digest = Digest::SHA256.hexdigest(workflow_text)
    unless TIER1_CI_WORKFLOW_SHA256S.include?(digest)
      raise RoadmapEvidenceError,
            "#{workflow}: does not match the reviewed Tier-1 CI workflow"
    end
  end

  return unless evidence_ids.include?("ci-build-test-coverage-100")
  return if coverage_gate_present?(workflow_text, workflow.to_s)

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
