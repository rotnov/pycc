#!/bin/sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)

if ! command -v codex >/dev/null 2>&1; then
  echo "error: install the Codex CLI before bootstrapping agent tools" >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "error: install Python 3 before bootstrapping agent tools" >&2
  exit 1
fi

marketplaces=$(codex plugin marketplace list --json)
marketplace_state=$(python3 -c '
import json
import os
import sys

expected = os.path.realpath(sys.argv[1])
entries = [
    entry
    for entry in json.load(sys.stdin).get("marketplaces", [])
    if entry.get("name") == "ievo-skills"
]
if not entries:
    print("missing")
elif len(entries) == 1 and os.path.realpath(entries[0].get("root", "")) == expected:
    print("current")
else:
    print("stale")
' "$repo_root" <<EOF
$marketplaces
EOF
)

if [ "$marketplace_state" = "stale" ]; then
  echo "Replacing ievo-skills marketplace registration from another checkout."
  codex plugin marketplace remove ievo-skills --json >/dev/null
fi
if [ "$marketplace_state" != "current" ]; then
  codex plugin marketplace add "$repo_root" --json >/dev/null
fi

codex plugin add ievo@ievo-skills

echo "Codex marketplace and pinned iEvo are installed."
echo "Repository skills are discovered project-locally from .agents/skills/."
