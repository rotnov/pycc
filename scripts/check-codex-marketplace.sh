#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_codex_home=$(mktemp -d "${TMPDIR:-/tmp}/pycc-codex-home.XXXXXX")

cleanup() {
  rm -rf -- "$test_codex_home"
}
trap cleanup EXIT HUP INT TERM

if ! command -v codex >/dev/null 2>&1; then
  echo "error: Codex CLI is required for marketplace validation" >&2
  exit 1
fi

initial=$(env CODEX_HOME="$test_codex_home" codex plugin marketplace list --json)
python3 -c '
import json, sys
data = json.load(sys.stdin)
assert not data.get("marketplaces"), "isolated Codex home is not fresh"
' <<EOF
$initial
EOF

env CODEX_HOME="$test_codex_home" codex plugin marketplace add "$repo_root" --json

marketplaces=$(env CODEX_HOME="$test_codex_home" codex plugin marketplace list --json)
python3 -c '
import json, sys
data = json.load(sys.stdin)
names = {entry["name"] for entry in data.get("marketplaces", [])}
assert "ievo-skills" in names, "ievo-skills marketplace was not registered"
' <<EOF
$marketplaces
EOF

env CODEX_HOME="$test_codex_home" codex plugin add ievo@ievo-skills --json

plugins=$(env CODEX_HOME="$test_codex_home" codex plugin list --json)
python3 -c '
import json, sys
data = json.load(sys.stdin)
entries = data.get("installed", [])
matches = [entry for entry in entries if entry.get("pluginId") == "ievo@ievo-skills"]
assert len(matches) == 1, "ievo plugin was not installed exactly once"
assert matches[0].get("installed") is True, "ievo plugin is not installed"
assert matches[0].get("enabled") is True, "ievo plugin is not enabled"
' <<EOF
$plugins
EOF

echo "Codex marketplace: valid in an isolated fresh environment"
