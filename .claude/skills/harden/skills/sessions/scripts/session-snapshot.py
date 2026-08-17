#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Drafts a handoff snapshot: the mechanical half, for a model to finish.

A rule saying "write a snapshot at a checkpoint" dies the way every rule keyed to
the agent's memory dies. This is keyed to an event instead — the PreCompact hook
runs it when context is about to be lost, which is exactly when a snapshot has
value. What a script cannot know (what is in flight, what to do next) is left as
headings for the model to fill.

Filename is `YYYY-MM-DD-NN-<slug>.md`; NN is the next unused number for the day,
so two agents writing at once never touch the same file.

Usage:
  uv run .claude/skills/sessions/scripts/session-snapshot.py --slug context-compaction
  uv run .claude/skills/sessions/scripts/session-snapshot.py --slug handoff --dir docs/sessions
  uv run .claude/skills/sessions/scripts/session-snapshot.py --root /path/to/repo --slug handoff
"""
import argparse
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path

RECENT_COMMITS = 8
MAX_DIRTY = 12
SEQ_LIMIT = 100


def git(root: Path, *args: str) -> str:
    r = subprocess.run(["git", *args], capture_output=True, text=True, check=False,
                       cwd=root)
    return r.stdout.strip() if r.returncode == 0 else ""


def open_incidents(root: Path) -> list[str]:
    """An incident with `verdict: pending` is unfinished work by definition."""
    out = []
    for f in sorted((root / ".harden" / "incidents").rglob("*.md")):
        text = f.read_text(encoding="utf-8", errors="replace")
        if "verdict: pending" in text:
            out.append(str(f.relative_to(root)))
    return out


def write_next(dir_: Path, date: str, slug: str, content: str) -> Path:
    # "x" reserves the sequence number atomically — an exists() check first would
    # let two concurrent sessions pick the same slot and one overwrite the other.
    for n in range(1, SEQ_LIMIT):
        p = dir_ / f"{date}-{n:02d}-{slug}.md"
        try:
            with p.open("x", encoding="utf-8") as f:
                f.write(content)
        except FileExistsError:
            continue
        return p
    msg = f"more than {SEQ_LIMIT} snapshots for {date}"
    raise RuntimeError(msg)


def render(date: str, slug: str, root: Path) -> str:
    dirty = [ln for ln in git(root, "status", "--porcelain").splitlines() if ln.strip()]
    commits = git(root, "log", f"-{RECENT_COMMITS}", "--format=- `%h` %s")
    pending = open_incidents(root)

    lines = [
        f"# {date} — {slug.replace('-', ' ')}\n",
        "> Drafted by `scripts/session-snapshot.py`. The sections below the rule are",
        "> for a human or model to complete — a snapshot of only what a script can see",
        "> is a git status with a date on it.\n",
        "## Repository state\n",
        f"- Branch: `{git(root, 'rev-parse', '--abbrev-ref', 'HEAD') or '?'}`",
        (f"- Commit: `{git(root, 'rev-parse', '--short', 'HEAD') or '?'}` "
         f"— {git(root, 'log', '-1', '--format=%s') or '?'}"),
        f"- Uncommitted: {len(dirty)} path(s)"
        + (" — **snapshot taken with a dirty tree**" if dirty else ""),
    ]
    lines += [f"  - `{ln}`" for ln in dirty[:MAX_DIRTY]]
    if len(dirty) > MAX_DIRTY:
        lines.append(f"  - … and {len(dirty) - MAX_DIRTY} more")

    lines += ["\n## Recent commits\n", commits or "- (none)"]

    lines += ["\n## Open incidents\n"]
    lines += ([f"- `{p}` — `verdict: pending`" for p in pending]
              or ["- none carrying `verdict: pending`"])

    lines += [
        "\n---\n",
        "## In flight\n",
        "<!-- What is half-done right now. Distinguish committed from merely written. -->\n",
        "## Next\n",
        "<!-- What a fresh session should pick up first, and where to look. -->\n",
        "## Open questions\n",
        "<!-- Decisions deferred, assumptions not yet checked. -->\n",
    ]
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--slug", default="checkpoint")
    ap.add_argument("--dir", type=Path, default=Path("docs/sessions"))
    ap.add_argument("--root", type=Path, default=Path.cwd())
    a = ap.parse_args()

    root = a.root.resolve()
    if not root.is_dir():
        print(f"not a directory: {root}")
        return 2
    output_dir = a.dir if a.dir.is_absolute() else root / a.dir
    output_dir.mkdir(parents=True, exist_ok=True)
    date = datetime.now(UTC).strftime("%Y-%m-%d")
    slug = "".join(c if c.isalnum() or c == "-" else "-" for c in a.slug.lower()).strip("-")

    path = write_next(output_dir, date, slug or "checkpoint", render(date, slug, root))
    print(f"snapshot drafted: {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
