#!/usr/bin/env python3
"""Read back a `.harden/findings/<task>.jsonl` pile before it is batched.

The pile is the whole interface between a task's review loop and the
`harden batch` pass (`.claude/skills/harden/references/batch.md`): the
workflow appends one JSON line per finding per round, and the batch pass
clusters what it reads. Two silent failure modes were observed on one task
(incident: process-record-written-without-read-back):

* the file never reached the commit -- a machine-local `.git/info/exclude`
  entry hid the tracked `.harden/` directory from `git add -A`, and nothing
  in the loop reads the staged set back, so `git status` showed a clean tree
  while the pile was absent from the diff under review;
* a line recorded `disposition: fixed` while its `note` was a refutation
  reason, so the batch pass would have counted a reviewer error as a fix.

This checker is the read-back. Given one or more pile paths it verifies
that each is tracked by git (`git ls-files --error-unmatch`), that every
line is a JSON object carrying the schema's required keys, that
`disposition` is `fixed` or `refuted`, and that a `refuted` line carries a
non-empty `note` (the refutation reason is the datum the batch pass routes
to the reviewer's own artefact). Exit 0 when every pile passes, 1 with one
message per defect otherwise, 2 for a usage error.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REQUIRED_KEYS = ("round", "file", "category", "summary", "disposition", "note")
DISPOSITIONS = ("fixed", "refuted")

# Piles written before `references/batch.md` fixed the line schema (they carry
# `status`/`finding`/`description` instead of `disposition`/`summary`/`note`).
# A one-time snapshot taken when this checker was introduced: every pile
# outside it is held to the schema, and this set only ever shrinks. The
# tracked-by-git check still applies to a legacy pile.
LEGACY_SCHEMA_PILES = frozenset(
    {
        "issue-197.jsonl",
        "issue-211.jsonl",
        "issue-378.jsonl",
        "issue-380.jsonl",
        "issue-522.jsonl",
        "issue-624.jsonl",
        "issue-629.jsonl",
        "issue-719.jsonl",
    }
)


def is_tracked(path: Path, repo_root: Path) -> bool:
    """Return whether git tracks ``path`` (index or HEAD) inside ``repo_root``."""
    result = subprocess.run(
        ["git", "-C", str(repo_root), "ls-files", "--error-unmatch", "--", str(path)],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.returncode == 0


def validate_lines(text: str, label: str) -> list[str]:
    """Return the schema defects found in ``text``, prefixed with ``label``."""
    problems: list[str] = []
    seen = 0
    for number, raw in enumerate(text.splitlines(), start=1):
        if not raw.strip():
            continue
        seen += 1
        try:
            row = json.loads(raw)
        except json.JSONDecodeError as error:
            problems.append(f"{label}:{number}: not valid JSON ({error.msg})")
            continue
        if not isinstance(row, dict):
            problems.append(f"{label}:{number}: line is not a JSON object")
            continue
        missing = [key for key in REQUIRED_KEYS if key not in row]
        if missing:
            problems.append(f"{label}:{number}: missing key(s) {', '.join(missing)}")
            continue
        disposition = row["disposition"]
        if disposition == "clean" and row["category"] == "clean-round":
            continue  # a round marker recording zero findings, not a finding
        if disposition not in DISPOSITIONS:
            problems.append(
                f"{label}:{number}: disposition {disposition!r} is not one of "
                f"{', '.join(DISPOSITIONS)}"
            )
        elif disposition == "refuted" and not str(row["note"]).strip():
            problems.append(f"{label}:{number}: refuted finding has an empty note")
    if seen == 0:
        problems.append(f"{label}: pile is empty")
    return problems


def validate(paths: list[Path], repo_root: Path) -> list[str]:
    """Validate every pile in ``paths``; return all defects found."""
    problems: list[str] = []
    for path in paths:
        label = str(path)
        if not path.is_file():
            problems.append(f"{label}: no such file")
            continue
        if not is_tracked(path, repo_root):
            problems.append(
                f"{label}: not tracked by git -- stage it with `git add -f` "
                "(a machine-local exclude can hide a tracked directory from `git add -A`)"
            )
        if path.name in LEGACY_SCHEMA_PILES:
            continue
        problems.extend(validate_lines(path.read_text(encoding="utf-8"), label))
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("piles", nargs="+", help="findings .jsonl file(s) to read back")
    parser.add_argument(
        "--repo-root",
        default=".",
        help="git checkout the piles live in (default: current directory)",
    )
    args = parser.parse_args(argv)
    repo_root = Path(args.repo_root)
    if not (repo_root / ".git").exists():
        print(f"error: {repo_root} is not a git checkout", file=sys.stderr)
        return 2
    problems = validate([Path(p) for p in args.piles], repo_root)
    for problem in problems:
        print(problem)
    if problems:
        return 1
    print(f"ok: {len(args.piles)} findings pile(s) tracked and well-formed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
