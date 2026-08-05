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

echo "Codex repository skills are discovered project-locally from .agents/skills/."
