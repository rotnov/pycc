#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Installs a practice into a project as a bundle, not as a paragraph.

The tempting version of this tool appends a paragraph to the governance file and
calls the feature installed. That is the move which grew one real AGENTS.md to
158 KB, and rules are the artefact type with the worst measured record. So a
feature here is files plus, at most, a routing line: where it lives, what the
format is, what regenerates it. The mechanics stay in the feature's own README —
copying them into the governance file creates a second copy that drifts.

The injected line is fenced by markers so a second run replaces rather than
duplicates, and so --uninstall can find it again.

Usage:
  uv run install-feature.py --list
  uv run install-feature.py adr sessions
  uv run install-feature.py adr --check
  uv run install-feature.py adr --uninstall
"""
import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path
from typing import Any

FEATURES = Path(__file__).resolve().parent.parent / "assets" / "features"
LOCK = Path(".harden") / "installed.json"
GOVERNANCE = ("AGENTS.md", "CLAUDE.md")


def resolve(names: list[str]) -> list[str]:
    """Expand `requires`, dependencies first, each name once.

    A feature whose rule routes to another feature's directory is broken without
    it — the line points at a path that does not exist, which is worse than no
    line at all because it reads as an instruction.
    """
    order: list[str] = []

    def visit(name: str, seen: tuple[str, ...]) -> None:
        if name in order:
            return
        if name in seen:
            msg = f"circular requires: {' -> '.join([*seen, name])}"
            raise SystemExit(msg)
        for dep in load(name).get("requires") or []:
            visit(dep, (*seen, name))
        order.append(name)

    for n in names:
        visit(n, ())
    return order


def load(name: str) -> dict[str, Any]:
    """Metadata from feature.json, rule text from rule.md beside it.

    The rule lived inside the JSON as an escaped string, which made the one part
    a human edits the one part hardest to read. Prose belongs in a prose file.
    """
    manifest = FEATURES / name / "feature.json"
    if not manifest.is_file():
        msg = f"unknown feature: {name}"
        raise SystemExit(msg)
    data: dict[str, Any] = json.loads(manifest.read_text(encoding="utf-8"))
    rule = FEATURES / name / "rule.md"
    data["rule"] = rule.read_text(encoding="utf-8").rstrip() if rule.is_file() else ""
    return data


def digest(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def read_lock(project: Path) -> dict[str, Any]:
    p = project / LOCK
    if not p.is_file():
        return {"features": {}}
    data: dict[str, Any] = json.loads(p.read_text(encoding="utf-8"))
    data.setdefault("features", {})
    return data


def write_lock(project: Path, lock: dict[str, Any]) -> None:
    """What was installed, so a later check knows what to expect.

    Without it `--check` can only compare bytes against the bundle: it cannot
    tell a feature that was never installed from one whose files were deleted,
    and both look identical from the outside.
    """
    p = project / LOCK
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(lock, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def governance_file(project: Path) -> Path:
    """The file every harness reads. Both present means AGENTS.md and only it —
    writing to both makes two copies of one rule, and they drift."""
    for name in GOVERNANCE:
        if (project / name).is_file():
            return project / name
    return project / GOVERNANCE[0]


def markers(name: str) -> tuple[str, str]:
    return f"<!-- harden:{name} -->", f"<!-- /harden:{name} -->"


def inject(path: Path, name: str, rule: str) -> str:
    start, end = markers(name)
    block = f"{start}\n{rule}\n{end}"
    text = path.read_text(encoding="utf-8") if path.is_file() else "# AGENTS.md\n"

    if start in text and end in text:
        head, rest = text.split(start, 1)
        _, tail = rest.split(end, 1)
        path.write_text(head + block + tail, encoding="utf-8")
        return "updated"

    sep = "" if text.endswith("\n\n") else ("\n" if text.endswith("\n") else "\n\n")
    path.write_text(text + sep + block + "\n", encoding="utf-8")
    return "added"


def remove(path: Path, name: str) -> bool:
    start, end = markers(name)
    if not path.is_file():
        return False
    text = path.read_text(encoding="utf-8")
    if start not in text or end not in text:
        return False
    head, rest = text.split(start, 1)
    _, tail = rest.split(end, 1)
    path.write_text((head.rstrip("\n") + "\n" + tail.lstrip("\n")), encoding="utf-8")
    return True


def install(feature: dict[str, Any], project: Path, lock: dict[str, Any]) -> None:
    src = FEATURES / feature["name"]
    for source, target in feature["files"].items():
        dst = project / target
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(src / source, dst)
        if dst.suffix == ".py":
            dst.chmod(0o755)
        print(f"    {target}")

    # `init` files are written ONCE and never overwritten: they are seeds the
    # project grows into (a decisions README, a template). The contract was
    # documented before it was implemented, and the gap nearly cost a field
    # project its own decisions README on install — caught one command short.
    for source, target in (feature.get("init") or {}).items():
        dst = project / target
        if dst.exists():
            print(f"    {target} (kept — the project's own)")
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(src / source, dst)
        print(f"    {target} (seeded)")

    gov = governance_file(project)
    print(f"    {gov.name}: rule {inject(gov, feature['name'], feature['rule'])}")

    if gate := feature.get("gate"):
        print("\n  This feature has a gate. It is not wired in automatically —")
        print("  a pre-commit config is the project's own, and overwriting one")
        print("  is worse than leaving a gate uninstalled. Add this line yourself:")
        print(f"\n    {gate}\n")

    lock["features"][feature["name"]] = {
        "rule": digest(feature["rule"]),
        "files": sorted(feature["files"].values()),
        "requires": feature.get("requires") or [],
    }

    for cmd in feature.get("after") or []:
        print(f"  Run once to initialise:  {cmd}")


def check(feature: dict[str, Any], project: Path, lock: dict[str, Any]) -> int:
    name = feature["name"]
    if name not in lock["features"]:
        print(f"  {name}: not installed here (absent from {LOCK})")
        return 1
    src, missing = FEATURES / name, 0
    if lock["features"][name].get("rule") != digest(feature["rule"]):
        print("    rule text changed in the bundle since it was installed")
        missing += 1
    for source, target in feature["files"].items():
        dst = project / target
        if not dst.is_file():
            print(f"    missing: {target}")
            missing += 1
        elif dst.read_bytes() != (src / source).read_bytes():
            print(f"    differs: {target}")
            missing += 1
    gov = governance_file(project)
    start, end = markers(feature["name"])
    text = gov.read_text(encoding="utf-8") if gov.is_file() else ""
    if start not in text or end not in text:
        print(f"    missing: rule block in {gov.name}")
        missing += 1
    elif text.split(start, 1)[1].split(end, 1)[0] != f"\n{feature['rule']}\n":
        # A block that exists but was edited or truncated is not "installed":
        # the routing line agents actually read is no longer the one proven.
        print(f"    differs: rule block in {gov.name} was edited since install")
        missing += 1
    print(f"  {name}: {'installed' if not missing else f'{missing} problem(s)'}")
    return missing


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("features", nargs="*")
    ap.add_argument("--project", type=Path, default=Path.cwd())
    ap.add_argument("--list", action="store_true", dest="show")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--uninstall", action="store_true")
    a = ap.parse_args()

    available = sorted(p.name for p in FEATURES.iterdir() if (p / "feature.json").is_file())

    if a.show or not a.features:
        print("Features this skill can install:\n")
        for name in available:
            f = load(name)
            print(f"  [ ] {name:<10} {f['summary']}")
            print(f"      {f['why']}\n")
        print("Install with:  uv run install-feature.py <name> [<name>…]")
        print("Each is files plus one routing line in the governance file — never a")
        print("paragraph of mechanics, which is how that file grows without bound.")
        return 0

    unknown = [n for n in a.features if n not in available]
    if unknown:
        print(f"unknown: {', '.join(unknown)} (available: {', '.join(available)})")
        return 2

    project = a.project.resolve()
    lock = read_lock(project)
    problems = 0
    requested = list(a.features)
    features = requested if a.uninstall else resolve(requested)
    if pulled := [n for n in features if n not in requested]:
        print(f"  pulling in required: {', '.join(pulled)}\n")
    for name in features:
        feature = load(name)
        if a.check:
            problems += check(feature, project, lock)
        elif a.uninstall:
            gov = governance_file(project)
            print(f"  {name}: rule block "
                  f"{'removed from ' + gov.name if remove(gov, name) else 'not found'}")
            # Reverse dependencies: install resolves them forwards, so removal has
            # to look backwards or it silently strips the ground from under a rule
            # that is still installed and still pointing at it.
            dependents = sorted(
                other for other, meta in lock["features"].items()
                if other != name and name in (meta.get("requires") or []))
            if dependents:
                print(f"    WARNING: still required by {', '.join(dependents)} — "
                      f"their rules route into what {name} sets up")
            lock["features"].pop(name, None)
            print("    Files are left in place — they may hold real content by now.")
        else:
            print(f"  {name}:")
            install(feature, project, lock)

    if not a.check:
        write_lock(project, lock)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
