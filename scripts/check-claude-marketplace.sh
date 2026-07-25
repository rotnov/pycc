#!/bin/sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
test_claude_config=$(mktemp -d "${TMPDIR:-/tmp}/pycc-claude-config.XXXXXX")
marketplace_root="$test_claude_config/inline-marketplace"
test_claude_home="$test_claude_config/home"
test_tmp=${TMPDIR:-/tmp}

cleanup() {
  rm -rf -- "$test_claude_config"
}
trap cleanup EXIT HUP INT TERM

run_with_timeout() {
  command_output=$1
  shift
  python3 - "$command_output" "$@" <<'PY'
import subprocess
import os
import signal
import sys
import time

output_path = sys.argv[1]
command = sys.argv[2:]
with open(output_path, "w", encoding="utf-8") as output:
    popen_arguments = {
        "stdout": output,
        "stderr": subprocess.STDOUT,
        "text": True,
    }
    if os.name == "nt":
        popen_arguments["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_arguments["start_new_session"] = True
    process = subprocess.Popen(command, **popen_arguments)
    try:
        result = process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        print("command timed out after 20s", file=output)
        output.flush()
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                check=False,
                capture_output=True,
                text=True,
                timeout=5,
            )
            if process.poll() is None:
                process.kill()
        else:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 1
            while time.monotonic() < deadline:
                try:
                    os.killpg(process.pid, 0)
                except (PermissionError, ProcessLookupError):
                    break
                time.sleep(0.05)
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except (PermissionError, ProcessLookupError):
                    pass
        process.wait()
        raise SystemExit(124)
raise SystemExit(result)
PY
}

if ! command -v claude >/dev/null 2>&1; then
  echo "error: Claude Code CLI is required for marketplace validation" >&2
  exit 1
fi

mkdir -p "$test_claude_home" "$marketplace_root/.claude-plugin"

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

plugin_output="$test_claude_config/plugin-validate.out"
run_with_timeout "$plugin_output" \
  env -i \
  PATH="$PATH" \
  HOME="$test_claude_home" \
  TMPDIR="$test_tmp" \
  CI=1 \
  NO_COLOR=1 \
  TERM=dumb \
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
  CLAUDE_CONFIG_DIR="$test_claude_config" \
  claude plugin validate --strict "$marketplace_root"
cat "$plugin_output"

skill_debug="$test_claude_config/skill-discovery.log"
unknown_skill_output="$test_claude_config/unknown-skill.out"

# The unknown-command path is resolved locally after Claude has loaded the
# project skill registry, but before any model request. A minimal environment
# prevents inherited OAuth, cloud-provider, or API credentials from escaping
# this no-network discovery smoke.
run_with_timeout "$unknown_skill_output" \
  env -i \
  PATH="$PATH" \
  HOME="$test_claude_home" \
  TMPDIR="$test_tmp" \
  CI=1 \
  NO_COLOR=1 \
  TERM=dumb \
  ANTHROPIC_API_KEY=sk-ant-api03-invalid \
  ANTHROPIC_MAX_RETRIES=0 \
  ANTHROPIC_BASE_URL=http://127.0.0.1:9 \
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
  CLAUDE_CONFIG_DIR="$test_claude_config" \
  claude -p --no-session-persistence --debug-file "$skill_debug" \
  "/pycc-definitely-not-a-skill"
if ! grep -q "Unknown command: /pycc-definitely-not-a-skill" \
  "$unknown_skill_output"; then
  echo "error: Claude Code unknown-skill failure path changed" >&2
  exit 1
fi
if grep -q "Invalid settings" "$unknown_skill_output" ||
  grep -q "Invalid settings" "$skill_debug"; then
  echo "error: Claude Code rejected the project settings" >&2
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
if ! grep -Fq \
  "project=[$repo_root/.claude/skills]" \
  "$skill_debug"; then
  echo "error: Claude Code did not use the repository skill directory" >&2
  exit 1
fi

skill_dir="$repo_root/.claude/skills/i-have-an-issue"
python3 "$skill_dir/scripts/search_github.py" --help >/dev/null
python3 "$repo_root/scripts/check_alpha_skill_evals.py" --client-entrypoint claude
python3 "$repo_root/scripts/check_alpha_skill_evals.py" --behavioral-evidence

echo "Claude marketplace and project-scoped skills: valid"
