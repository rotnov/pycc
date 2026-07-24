#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
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

echo "Claude marketplace: project settings and inline manifest are valid"
