#!/bin/sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
test_codex_home=$(mktemp -d "${TMPDIR:-/tmp}/pycc-codex-home.XXXXXX")
stale_checkout=$(mktemp -d "${TMPDIR:-/tmp}/pycc-stale-checkout.XXXXXX")

cleanup() {
  rm -rf -- "$test_codex_home" "$stale_checkout"
}
trap cleanup EXIT HUP INT TERM

if ! command -v codex >/dev/null 2>&1; then
  echo "error: Codex CLI is required for marketplace validation" >&2
  exit 1
fi

cp -R "$repo_root/.agents" "$stale_checkout/.agents"
python3 -c '
from pathlib import Path
import sys

path = Path(sys.argv[1])
path.write_text(
    path.read_text(encoding="utf-8") + "\nSTALE_CHECKOUT_MARKER\n",
    encoding="utf-8",
)
' "$stale_checkout/.agents/plugins/plugins/pycc-agent-skills/skills/tdd/SKILL.md"
env CODEX_HOME="$test_codex_home" \
  codex plugin marketplace add "$stale_checkout" --json >/dev/null
env CODEX_HOME="$test_codex_home" \
  codex plugin add pycc-agent-skills@ievo-skills --json >/dev/null

env CODEX_HOME="$test_codex_home" "$repo_root/scripts/bootstrap-agent-tools.sh"

marketplaces=$(env CODEX_HOME="$test_codex_home" codex plugin marketplace list --json)
python3 -c '
import json
import os
import sys

data = json.load(sys.stdin)
matches = [
    entry
    for entry in data.get("marketplaces", [])
    if entry.get("name") == "ievo-skills"
]
assert len(matches) == 1, "ievo-skills marketplace was not registered exactly once"
assert os.path.realpath(matches[0]["root"]) == os.path.realpath(sys.argv[1]), (
    "stale marketplace registration was not replaced"
)
' "$repo_root" <<EOF
$marketplaces
EOF

plugins=$(env CODEX_HOME="$test_codex_home" codex plugin list --json)
python3 -c '
import json, sys
data = json.load(sys.stdin)
entries = data.get("installed", [])
for plugin_id in ("ievo@ievo-skills", "pycc-agent-skills@ievo-skills"):
    matches = [entry for entry in entries if entry.get("pluginId") == plugin_id]
    assert len(matches) == 1, f"{plugin_id} was not installed exactly once"
    assert matches[0].get("installed") is True, f"{plugin_id} is not installed"
    assert matches[0].get("enabled") is True, f"{plugin_id} is not enabled"
' <<EOF
$plugins
EOF

project_plugin=$(env CODEX_HOME="$test_codex_home" \
  codex plugin add pycc-agent-skills@ievo-skills --json)
prompt_input="$test_codex_home/prompt-input.json"
env CODEX_HOME="$test_codex_home" \
  codex -C "$repo_root" debug prompt-input >"$prompt_input"
python3 -c '
import json
import pathlib
import re
import sys

installed = pathlib.Path(json.load(sys.stdin)["installedPath"])
wrappers = sorted(path.parent.name for path in installed.glob("skills/*/SKILL.md"))
assert "grill-with-docs" in wrappers, "Codex skill wrappers were not installed"
assert len(wrappers) == 13, "Codex did not install every repository skill wrapper"
assert "STALE_CHECKOUT_MARKER" not in (
    installed / "skills" / "tdd" / "SKILL.md"
).read_text(encoding="utf-8"), "stale plugin cache was not refreshed"
prompt = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
model_input = "\n".join(
    content.get("text", "")
    for item in prompt
    for content in item.get("content", [])
    if isinstance(content, dict)
)
discovered = sorted(
    re.findall(r"^- pycc-agent-skills:([^:]+):", model_input, flags=re.MULTILINE)
)
assert discovered == wrappers, "Codex did not discover every project skill wrapper"
' "$prompt_input" <<EOF
$project_plugin
EOF

echo "Codex marketplace and discovered skills: valid after replacing a stale checkout"
