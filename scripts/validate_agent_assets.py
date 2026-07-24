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
IEVO_REPOSITORY_URL = "https://github.com/ievo-ai/skills.git"
IEVO_PLUGIN_PATH = "./plugins/ievo"
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


def validate_claude_ievo_marketplace(
    settings: dict,
    codex_ievo_ref: str | None,
    failures: list[str],
    settings_path: str = ".claude/settings.json",
) -> None:
    marketplaces = settings.get("extraKnownMarketplaces")
    if not isinstance(marketplaces, dict):
        failures.append(f"{settings_path}: extraKnownMarketplaces must be an object")
        return

    ievo_marketplace = marketplaces.get("ievo-skills")
    if not isinstance(ievo_marketplace, dict):
        failures.append(f"{settings_path}: ievo-skills marketplace is required")
        return
    if ievo_marketplace.get("autoUpdate") is not False:
        failures.append(f"{settings_path}: ievo-skills.autoUpdate must be false")

    marketplace_source = ievo_marketplace.get("source")
    if not isinstance(marketplace_source, dict):
        failures.append(f"{settings_path}: ievo-skills.source must be an object")
        return
    if marketplace_source.get("source") != "settings":
        failures.append(
            f"{settings_path}: ievo-skills must use an inline settings marketplace"
        )
    if marketplace_source.get("name") != "ievo-skills":
        failures.append(f"{settings_path}: inline marketplace name must be ievo-skills")

    plugins = marketplace_source.get("plugins")
    if not isinstance(plugins, list) or not plugins:
        failures.append(
            f"{settings_path}: inline marketplace plugins must be a non-empty array"
        )
        return
    ievo_plugins = [
        plugin
        for plugin in plugins
        if isinstance(plugin, dict) and plugin.get("name") == "ievo"
    ]
    if len(ievo_plugins) != 1:
        failures.append(
            f"{settings_path}: inline marketplace must contain exactly one ievo plugin"
        )
        return

    plugin_source = ievo_plugins[0].get("source")
    if not isinstance(plugin_source, dict):
        failures.append(f"{settings_path}: ievo plugin source must be an object")
        return
    if plugin_source.get("source") != "git-subdir":
        failures.append(f"{settings_path}: ievo plugin must use a git-subdir source")
    if plugin_source.get("url") != IEVO_REPOSITORY_URL:
        failures.append(
            f"{settings_path}: ievo plugin URL must be {IEVO_REPOSITORY_URL}"
        )
    if plugin_source.get("path") != IEVO_PLUGIN_PATH:
        failures.append(f"{settings_path}: ievo plugin path must be {IEVO_PLUGIN_PATH}")
    if "ref" in plugin_source:
        failures.append(
            f"{settings_path}: ievo plugin must use sha, not ref, for an exact pin"
        )
    sha = plugin_source.get("sha")
    if not isinstance(sha, str) or IMMUTABLE_SHA.fullmatch(sha) is None:
        failures.append(
            f"{settings_path}: ievo plugin sha must be a full immutable commit SHA"
        )
    elif codex_ievo_ref is not None and sha != codex_ievo_ref:
        failures.append(
            f"{settings_path}: ievo plugin sha must match the Codex iEvo commit"
        )


def validate_marketplaces(failures: list[str]) -> None:
    codex_path = ".agents/plugins/marketplace.json"
    marketplace = load_json(codex_path, failures)
    plugins = marketplace.get("plugins")
    if not isinstance(plugins, list) or not plugins:
        failures.append(f"{codex_path}: plugins must be a non-empty array")
        return

    codex_ievo_ref: str | None = None
    for index, plugin in enumerate(plugins):
        label = f"{codex_path}: plugins[{index}]"
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
        if plugin.get("name") == "ievo" and isinstance(ref, str):
            codex_ievo_ref = ref

    if codex_ievo_ref is None:
        failures.append(f"{codex_path}: ievo plugin entry is required")

    claude_path = ".claude/settings.json"
    settings = load_json(claude_path, failures)
    validate_claude_ievo_marketplace(
        settings,
        codex_ievo_ref,
        failures,
        claude_path,
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
