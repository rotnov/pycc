#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Unpacks the practice skills bundled inside harden into siblings.

A catalogue install (`npx skills add …@harden`) delivers ONE skill directory,
so the practices ride inside it, under `skills/`. A harness only discovers
top-level `<skills-root>/<name>/SKILL.md`, so unpacking is exactly what it
sounds like: copy each bundled skill up, beside harden. Idempotent — an
existing sibling is refreshed file-by-file (project-added files under it are
preserved), and a project's own skill by the same name is never touched
without `--force` printing what it would replace.

Runs from anywhere; the skills root is wherever this script's harden lives.

Two dual-root layouts, chosen by `--layout` (default `auto`):
  link — full copy at the skills root; when that root is `.agents/skills`, a
         relative Claude Code symlink is laid for each skill.
  stub — the full copy is canonical under `.claude/skills/<name>/`, and
         `.agents/skills/<name>/SKILL.md` is a thin wrapper pointing at it.
         Some hosts require exactly this shape and reject symlinked skills.
  auto — stub when the host's existing `.agents/skills` entries are already
         thin wrappers (their SKILL.md names a canonical workflow), else link.
"""
import argparse
import re
import shutil
import sys
from pathlib import Path

HARDEN = Path(__file__).resolve().parent.parent
BUNDLED = HARDEN / "skills"
ROOT = HARDEN.parent                       # the skills root harden sits in
IGNORE = {"__pycache__", ".DS_Store"}

# Inside the bundle a skill reaches harden's references via ../../references/;
# once unpacked beside harden the same files live at ../harden/references/.
PACKED_REFS = "../../references/"
SIBLING_REFS = "../harden/references/"

WRAPPER = """\
---
name: {name}
description: {description}
---

# {name} (wrapper)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/{name}/SKILL.md` from that repository completely and follow
it as the canonical workflow. If the file is missing, stop and report the
missing project instruction instead of substituting a cached copy.
"""


def description_of(skill_md: Path) -> str:
    text = skill_md.read_text(encoding="utf-8")
    m = re.search(r"^description:\s*(.+)$", text, flags=re.MULTILINE)
    return m.group(1).strip() if m else ""


def repo_root() -> Path | None:
    if ROOT.match("*/.agents/skills") or ROOT.match("*/.claude/skills"):
        return ROOT.parent.parent
    return None


def detect_layout() -> str:
    """`stub` when the host already keeps thin wrappers in .agents/skills."""
    repo = repo_root()
    if repo is None:
        return "link"
    for skill_md in sorted((repo / ".agents" / "skills").glob("*/SKILL.md")):
        if skill_md.parent.name in {p.name for p in BUNDLED.iterdir()}:
            continue  # our own earlier unpack proves nothing about the host
        if "canonical workflow" in skill_md.read_text(encoding="utf-8",
                                                      errors="replace"):
            return "stub"
    return "link"


def overlay(src: Path, dst: Path) -> None:
    """Copy the bundle's files over dst, keeping files the bundle does not own.

    Never deletes: a repeated unpack must not eat scripts or references a
    project has grown under a practice skill.
    """
    for f in sorted(src.rglob("*")):
        if f.name in IGNORE or f.suffix == ".pyc" \
                or any(part in IGNORE for part in f.relative_to(src).parts):
            continue
        target = dst / f.relative_to(src)
        if f.is_dir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(f, target)
        if target.suffix == ".md":
            text = target.read_text(encoding="utf-8")
            if PACKED_REFS in text:
                target.write_text(text.replace(PACKED_REFS, SIBLING_REFS),
                                  encoding="utf-8")


def place(name: str, dst: Path, *, force: bool) -> str | None:
    """Lay the full copy at dst; return a 'kept' message, or None on success."""
    src = BUNDLED / name
    if dst.is_symlink():
        if not force:
            return (
                f"kept — {dst} is a project-owned symlink "
                "(--force to replace)"
            )
        dst.unlink()
    if dst.exists() and (dst / "SKILL.md").is_file():
        theirs = (dst / "SKILL.md").read_text(encoding="utf-8", errors="replace")
        ours = (src / "SKILL.md").read_text(encoding="utf-8")
        ours_unpacked = ours.replace(PACKED_REFS, SIBLING_REFS)
        if theirs not in (ours, ours_unpacked) and not force:
            return (f"kept — {dst} already exists and differs; "
                    "the project's own skill wins (--force to replace)")
        if force and theirs not in (ours, ours_unpacked):
            shutil.rmtree(dst)
    overlay(src, dst)
    return None


def unpack(name: str, *, force: bool, layout: str) -> str:
    repo = repo_root()
    if layout == "stub" and repo is not None:
        canonical = repo / ".claude" / "skills" / name
        kept = place(name, canonical, force=force)
        if kept:
            return kept
        wrapper = repo / ".agents" / "skills" / name / "SKILL.md"
        wrapper.parent.mkdir(parents=True, exist_ok=True)
        wrapper.write_text(WRAPPER.format(
            name=name, description=description_of(canonical / "SKILL.md")),
            encoding="utf-8")
        return f"unpacked -> {canonical}  (+ .agents wrapper)"

    dst = ROOT / name
    kept = place(name, dst, force=force)
    if kept:
        return kept
    if repo is not None and ROOT.match("*/.agents/skills"):
        link = repo / ".claude" / "skills" / name
        if link.is_symlink():
            link.unlink()
        elif link.exists():
            return f"unpacked -> {dst}  (.claude/skills/{name} exists, not linked)"
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(Path("../..") / ".agents" / "skills" / name,
                        target_is_directory=True)
        return f"unpacked -> {dst}  (+ claude link)"
    return f"unpacked -> {dst}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*",
                    help="bundled skills to unpack; default: all of them")
    ap.add_argument("--force", action="store_true",
                    help="replace an existing differing sibling")
    ap.add_argument("--layout", choices=["auto", "link", "stub"], default="auto",
                    help="dual-root shape; auto follows the host's existing one")
    a = ap.parse_args()
    bundled = sorted(p.name for p in BUNDLED.iterdir()
                     if (p / "SKILL.md").is_file()) if BUNDLED.is_dir() else []
    if not bundled:
        print("nothing bundled under skills/")
        return 1
    unknown = [n for n in a.names if n not in bundled]
    if unknown:
        print(f"not bundled: {', '.join(unknown)} (have: {', '.join(bundled)})")
        return 2
    layout = detect_layout() if a.layout == "auto" else a.layout
    for name in a.names or bundled:
        print(f"  {name}: {unpack(name, force=a.force, layout=layout)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
