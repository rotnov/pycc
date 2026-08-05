#!/bin/sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
test_codex_home=$(mktemp -d "${TMPDIR:-/tmp}/pycc-codex-home.XXXXXX")

cleanup() {
  rm -rf -- "$test_codex_home"
}
trap cleanup EXIT HUP INT TERM

if ! command -v codex >/dev/null 2>&1; then
  echo "error: Codex CLI is required for skill validation" >&2
  exit 1
fi

env CODEX_HOME="$test_codex_home" "$repo_root/scripts/bootstrap-agent-tools.sh"

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

skill_dir="$repo_root/.claude/skills/i-have-an-issue"
python3 "$skill_dir/scripts/search_github.py" --help >/dev/null

echo "Codex project-scoped skills: valid"
