#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if ! command -v codex >/dev/null 2>&1; then
  echo "error: install the Codex CLI before bootstrapping agent tools" >&2
  exit 1
fi

codex plugin marketplace add "$repo_root"
codex plugin add ievo@ievo-skills

echo "Codex marketplace and ievo@ievo-skills are installed from repository pins."
