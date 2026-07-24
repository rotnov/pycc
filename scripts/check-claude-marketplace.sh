#!/bin/sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
test_claude_config=$(mktemp -d "${TMPDIR:-/tmp}/pycc-claude-config.XXXXXX")
marketplace_root="$test_claude_config/inline-marketplace"

cleanup() {
  rm -rf -- "$test_claude_config"
}
trap cleanup EXIT HUP INT TERM

if ! command -v claude >/dev/null 2>&1; then
  echo "error: Claude Code CLI is required for marketplace validation" >&2
  exit 1
fi

doctor_output=$(
  cd "$repo_root"
  env CLAUDE_CONFIG_DIR="$test_claude_config" claude doctor 2>&1
)
printf '%s\n' "$doctor_output"
if printf '%s\n' "$doctor_output" | grep -q "Invalid settings"; then
  echo "error: Claude Code rejected the project settings" >&2
  exit 1
fi

mkdir -p "$marketplace_root/.claude-plugin"
python3 - "$repo_root/.claude/settings.json" \
  "$marketplace_root/.claude-plugin/marketplace.json" <<'PY'
import json
import sys
from pathlib import Path

settings_path, marketplace_path = map(Path, sys.argv[1:])
settings = json.loads(settings_path.read_text(encoding="utf-8"))
source = settings["extraKnownMarketplaces"]["ievo-skills"]["source"]
marketplace = {
    "name": source["name"],
    "owner": {"name": "pycc maintainers"},
    "description": "Pinned repository agent tooling.",
    "plugins": source["plugins"],
}
marketplace_path.write_text(
    json.dumps(marketplace, indent=2) + "\n",
    encoding="utf-8",
)
PY

env CLAUDE_CONFIG_DIR="$test_claude_config" \
  claude plugin validate --strict "$marketplace_root"

skill_debug="$test_claude_config/skill-discovery.log"
known_skill_output="$test_claude_config/known-skill.out"
unknown_skill_output="$test_claude_config/unknown-skill.out"

# An unauthenticated print session still resolves slash commands before it
# reaches the model. This checks project-skill discovery without credentials or
# a paid API call.
set +e
env -u ANTHROPIC_API_KEY \
  -u ANTHROPIC_AUTH_TOKEN \
  -u CLAUDE_CODE_OAUTH_TOKEN \
  CLAUDE_CONFIG_DIR="$test_claude_config" \
  claude -p --no-session-persistence --debug-file "$skill_debug" \
  "/i-have-an-issue" >"$known_skill_output" 2>&1
known_skill_rc=$?
set -e

if [ "$known_skill_rc" -eq 0 ] ||
  grep -q "Unknown command: /i-have-an-issue" "$known_skill_output"; then
  echo "error: Claude Code did not resolve /i-have-an-issue as a project skill" >&2
  cat "$known_skill_output" >&2
  exit 1
fi

skill_count=$(
  find "$repo_root/.claude/skills" -mindepth 2 -maxdepth 2 \
    -name SKILL.md -type f | wc -l | tr -d ' '
)
if ! grep -Fq \
  "project: $skill_count, additional: 0, legacy commands: 0)" \
  "$skill_debug"; then
  echo "error: Claude Code did not load every project skill" >&2
  exit 1
fi

env -u ANTHROPIC_API_KEY \
  -u ANTHROPIC_AUTH_TOKEN \
  -u CLAUDE_CODE_OAUTH_TOKEN \
  CLAUDE_CONFIG_DIR="$test_claude_config" \
  claude -p --no-session-persistence \
  "/pycc-definitely-not-a-skill" >"$unknown_skill_output" 2>&1
if ! grep -q "Unknown command: /pycc-definitely-not-a-skill" \
  "$unknown_skill_output"; then
  echo "error: Claude Code unknown-skill failure path changed" >&2
  exit 1
fi

skill_dir="$repo_root/.claude/skills/i-have-an-issue"
python3 "$skill_dir/scripts/search_github.py" --help >/dev/null

echo "Claude marketplace and project-scoped skills: valid"
