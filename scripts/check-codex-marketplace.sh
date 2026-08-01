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
env CODEX_HOME="$test_codex_home" \
  codex plugin marketplace add "$stale_checkout" --json >/dev/null

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
matches = [entry for entry in entries if entry.get("pluginId") == "ievo@ievo-skills"]
assert len(matches) == 1, "ievo@ievo-skills was not installed exactly once"
assert matches[0].get("installed") is True, "ievo@ievo-skills is not installed"
assert matches[0].get("enabled") is True, "ievo@ievo-skills is not enabled"
assert not any(
    entry.get("pluginId") == "pycc-agent-skills@" "ievo-skills"
    for entry in entries
), "repository skills must not be installed globally"
' <<EOF
$plugins
EOF

prompt_input="$test_codex_home/prompt-input.json"
project_trust="projects.\"$repo_root\".trust_level=\"trusted\""
env CODEX_HOME="$test_codex_home" \
  codex -c "$project_trust" -C "$repo_root" debug prompt-input >"$prompt_input"
# The Python regex anchors use `$`; the single-quoted program must not shell-expand it.
# shellcheck disable=SC2016
python3 -c '
import json
import pathlib
import re
import sys

repo_root = pathlib.Path(sys.argv[2]).resolve()
wrappers = sorted(
    path.parent.name for path in (repo_root / ".agents" / "skills").glob("*/SKILL.md")
)
assert "grill-with-docs" in wrappers, "Codex skill wrappers are missing"
assert "i-have-an-issue" in wrappers, "installed issue-research skill is missing"
assert "pycc" in wrappers, "project-local pycc alpha skill is missing"
assert "pycc-feedback" in wrappers, "project-local feedback alpha skill is missing"
assert len(wrappers) == 19, "Codex did not retain every repository skill wrapper"
prompt = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
model_input = "\n".join(
    content.get("text", "")
    for item in prompt
    for content in item.get("content", [])
    if isinstance(content, dict)
)
skill_roots = {
    alias: pathlib.Path(root).resolve()
    for alias, root in re.findall(
        r"^- `(r[0-9]+)` = `([^`]+)`$",
        model_input,
        re.MULTILINE,
    )
}
expected_root = (repo_root / ".agents" / "skills").resolve()
discovered = []
for name, raw_path in re.findall(
    r"^- ([a-z][a-z0-9-]+):.*\(file: ([^)]+)\)$",
    model_input,
    re.MULTILINE,
):
    alias, separator, suffix = raw_path.partition("/")
    skill_path = (
        skill_roots[alias] / suffix
        if separator and alias in skill_roots
        else pathlib.Path(raw_path)
    ).resolve()
    if (
        skill_path.parent.parent == expected_root
        and skill_path.parent.name == name
        and skill_path.name == "SKILL.md"
    ):
        discovered.append(name)
discovered.sort()
assert discovered == wrappers, "Codex did not discover every project skill wrapper"
assert "pycc-agent-skills:" not in model_input, (
    "repository skills must not be discovered from a global plugin namespace"
)
' "$prompt_input" "$repo_root"

set -- "$test_codex_home"/plugins/cache/ievo-skills/ievo/*/agents/deep-reviewer.md
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  echo "error: expected one pinned Codex deep-reviewer artifact" >&2
  exit 1
fi
reviewer_manifest=$1
set -- "$test_codex_home"/plugins/cache/ievo-skills/ievo/*/skills/deep-review/SKILL.md
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  echo "error: expected one pinned Codex deep-review entrypoint" >&2
  exit 1
fi
review_skill=$1
python3 - "$review_skill" "$reviewer_manifest" <<'PY'
import hashlib
import pathlib
import sys

expected = {
    "SKILL.md": "ec8805e22fff7db49cfe49c2a7cd49f340a618bf58da6acaf4253e875279670d",
    "deep-reviewer.md": "b5e11469ba8144686d07eccc3d0759662b9c1bc4c3a6f3d79961dc82f5e53ab2",
}
for raw_path in sys.argv[1:]:
    path = pathlib.Path(raw_path)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    assert digest == expected[path.name], f"{path.name} digest drifted"
print("Codex pinned deep-review entrypoint and agent: valid")
PY

skill_dir="$repo_root/.claude/skills/i-have-an-issue"
python3 "$skill_dir/scripts/search_github.py" --help >/dev/null

echo "Codex pinned marketplace and project-scoped skills: valid"
