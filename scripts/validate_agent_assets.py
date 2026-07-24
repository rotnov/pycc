#!/usr/bin/env python3
"""Validate repository-scoped agent skills and plugin configuration."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
SKILLS_ROOT = ROOT / ".claude" / "skills"
AUTHENTICATION_POLICIES = {"ON_INSTALL", "ON_USE"}
IMMUTABLE_SHA = re.compile(r"^[0-9a-f]{40}$")
RELEASE_REF = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
MARKDOWN_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
SLASH_SKILL = re.compile(r"`/([a-z][a-z0-9-]+)`")
ABSOLUTE_OUTPUT = re.compile(
    r"(?i)(?:save|saved|write|written|output|destination).{0,160}`(/[^`]+)`"
)


def load_json(relative_path: str, failures: list[str]) -> dict:
    path = ROOT / relative_path
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        failures.append(f"{relative_path}: invalid JSON: {error}")
        return {}
    if not isinstance(value, dict):
        failures.append(f"{relative_path}: top-level value must be an object")
        return {}
    return value


def validate_marketplaces(failures: list[str]) -> None:
    marketplace_path = ".agents/plugins/marketplace.json"
    marketplace = load_json(marketplace_path, failures)
    plugins = marketplace.get("plugins")
    if not isinstance(plugins, list) or not plugins:
        failures.append(f"{marketplace_path}: plugins must be a non-empty array")
        return

    for index, plugin in enumerate(plugins):
        label = f"{marketplace_path}: plugins[{index}]"
        if not isinstance(plugin, dict):
            failures.append(f"{label} must be an object")
            continue
        policy = plugin.get("policy")
        authentication = (
            policy.get("authentication") if isinstance(policy, dict) else None
        )
        if authentication not in AUTHENTICATION_POLICIES:
            failures.append(
                f"{label}.policy.authentication must be ON_INSTALL or ON_USE"
            )
        source = plugin.get("source")
        ref = source.get("ref") if isinstance(source, dict) else None
        if not isinstance(ref, str) or IMMUTABLE_SHA.fullmatch(ref) is None:
            failures.append(f"{label}.source.ref must be a full immutable commit SHA")

    claude_path = ".claude/settings.json"
    settings = load_json(claude_path, failures)
    marketplaces = settings.get("extraKnownMarketplaces")
    if not isinstance(marketplaces, dict):
        failures.append(f"{claude_path}: extraKnownMarketplaces must be an object")
        return

    ievo = marketplaces.get("ievo-skills")
    if not isinstance(ievo, dict):
        failures.append(f"{claude_path}: ievo-skills marketplace is required")
        return
    if ievo.get("autoUpdate") is not False:
        failures.append(f"{claude_path}: ievo-skills.autoUpdate must be false")
    source = ievo.get("source")
    ref = source.get("ref") if isinstance(source, dict) else None
    if not isinstance(ref, str) or RELEASE_REF.fullmatch(ref) is None:
        failures.append(
            f"{claude_path}: ievo-skills source.ref must pin a vMAJOR.MINOR.PATCH release"
        )


def fence_error(path: Path) -> str | None:
    active_character: str | None = None
    active_length = 0
    active_line = 0
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        match = re.match(r"^[ \t]{0,3}(`{3,}|~{3,})(.*)$", line)
        if match is None:
            continue
        marker, suffix = match.groups()
        character = marker[0]
        if active_character is None:
            active_character = character
            active_length = len(marker)
            active_line = number
        elif (
            character == active_character
            and len(marker) >= active_length
            and suffix.strip() == ""
        ):
            active_character = None
            active_length = 0
            active_line = 0
        elif character == active_character and suffix.strip():
            return (
                f"nested {marker} fence at line {number} inside fence opened "
                f"at line {active_line}"
            )
    if active_character is None:
        return None
    return f"unclosed {active_character * active_length} fence opened at line {active_line}"


def link_target(raw_target: str) -> str:
    target = raw_target.strip()
    if target.startswith("<") and ">" in target:
        target = target[1 : target.index(">")]
    else:
        target = target.split(maxsplit=1)[0]
    return unquote(target.split("#", 1)[0])


def validate_skill_documents(failures: list[str]) -> None:
    skill_names = {path.parent.name for path in SKILLS_ROOT.glob("*/SKILL.md")}
    markdown_files = sorted(SKILLS_ROOT.rglob("*.md"))
    if not markdown_files:
        failures.append(".claude/skills: no Markdown skill files found")
        return

    for path in markdown_files:
        relative = path.relative_to(ROOT)
        text = path.read_text(encoding="utf-8")

        error = fence_error(path)
        if error is not None:
            failures.append(f"{relative}: {error}")

        for line_number, line in enumerate(text.splitlines(), 1):
            if "skill" in line.lower() or "session" in line.lower():
                for dependency in SLASH_SKILL.findall(line):
                    if dependency not in skill_names:
                        failures.append(
                            f"{relative}:{line_number}: references missing project "
                            f"skill /{dependency}"
                        )
            match = ABSOLUTE_OUTPUT.search(line)
            if match is not None and match.group(1) != "/tmp":
                failures.append(
                    f"{relative}:{line_number}: repository output path must be relative: "
                    f"{match.group(1)}"
                )

        for raw_target in MARKDOWN_LINK.findall(text):
            target = link_target(raw_target)
            if not target or target.startswith("#"):
                continue
            if target.startswith("/"):
                failures.append(
                    f"{relative}: local link must be relative: {raw_target}"
                )
                continue
            if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or "{" in target:
                continue
            resolved = (path.parent / target).resolve()
            try:
                resolved.relative_to(ROOT)
            except ValueError:
                failures.append(
                    f"{relative}: local link escapes repository: {raw_target}"
                )
                continue
            if not resolved.exists():
                failures.append(f"{relative}: broken relative link: {raw_target}")


def main() -> int:
    failures: list[str] = []
    validate_marketplaces(failures)
    validate_skill_documents(failures)
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    print("agent assets: valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
