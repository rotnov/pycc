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
CODEX_SKILLS_ROOT = ROOT / ".agents" / "skills"
AUTHENTICATION_POLICIES = {"ON_INSTALL", "ON_USE"}
IMMUTABLE_SHA = re.compile(r"^[0-9a-f]{40}$")
IEVO_REPOSITORY_URL = "https://github.com/ievo-ai/skills.git"
IEVO_PLUGIN_PATH = "./plugins/ievo"
CLAUDE_MARKETPLACE_REPOSITORIES = {
    "ievo-skills": IEVO_REPOSITORY_URL,
    "pycc-official-pinned": (
        "https://github.com/anthropics/claude-plugins-official.git"
    ),
    "pycc-workflows-pinned": "https://github.com/wshobson/agents.git",
}
EXPECTED_CLAUDE_PLUGIN_COORDINATES = {
    "ievo@ievo-skills",
    "feature-dev@pycc-official-pinned",
    "code-review@pycc-official-pinned",
    "pr-review-toolkit@pycc-official-pinned",
    "rust-analyzer-lsp@pycc-official-pinned",
    "claude-security@pycc-official-pinned",
    "systems-programming@pycc-workflows-pinned",
    "security-scanning@pycc-workflows-pinned",
    "dependency-management@pycc-workflows-pinned",
    "tdd-workflows@pycc-workflows-pinned",
    "performance-testing-review@pycc-workflows-pinned",
}
FORBIDDEN_CLAUDE_PLUGINS = {"security-guidance"}
CLAUDE_PLUGIN_ENTRY_CONTRACTS = {
    ("pycc-official-pinned", "rust-analyzer-lsp"): {
        "strict": False,
        "lspServers": {
            "rust-analyzer": {
                "command": "rust-analyzer",
                "extensionToLanguage": {".rs": "rust"},
            }
        },
    }
}
CANONICAL_SKILL_PATH = re.compile(r"`(\.claude/skills/([a-z][a-z0-9-]*)/SKILL\.md)`")
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


def validate_enabled_claude_plugin_pins(
    settings: dict,
    failures: list[str],
    settings_path: str = ".claude/settings.json",
) -> None:
    marketplaces = settings.get("extraKnownMarketplaces")
    if not isinstance(marketplaces, dict):
        failures.append(f"{settings_path}: extraKnownMarketplaces must be an object")
        return
    enabled_plugins = settings.get("enabledPlugins")
    if not isinstance(enabled_plugins, dict):
        failures.append(f"{settings_path}: enabledPlugins must be an object")
        return
    enabled_coordinates = {
        coordinate for coordinate in enabled_plugins if isinstance(coordinate, str)
    }
    missing_coordinates = sorted(
        EXPECTED_CLAUDE_PLUGIN_COORDINATES - enabled_coordinates
    )
    unexpected_coordinates = sorted(
        enabled_coordinates - EXPECTED_CLAUDE_PLUGIN_COORDINATES
    )
    if missing_coordinates:
        failures.append(
            f"{settings_path}: required enabled plugins are missing: "
            + ", ".join(missing_coordinates)
        )
    if unexpected_coordinates:
        failures.append(
            f"{settings_path}: unexpected enabled plugins: "
            + ", ".join(unexpected_coordinates)
        )
    for coordinate, enabled in enabled_plugins.items():
        if enabled is not True:
            failures.append(
                f"{settings_path}: enabled plugin {coordinate!r} must be true"
            )

    for marketplace_name, marketplace in sorted(marketplaces.items()):
        source = marketplace.get("source") if isinstance(marketplace, dict) else None
        plugins = source.get("plugins") if isinstance(source, dict) else None
        if not isinstance(plugins, list):
            continue
        for plugin in plugins:
            name = plugin.get("name") if isinstance(plugin, dict) else None
            if name in FORBIDDEN_CLAUDE_PLUGINS:
                failures.append(
                    f"{settings_path}: {name}@{marketplace_name} must not be declared"
                )

    for coordinate, enabled in sorted(enabled_plugins.items()):
        if enabled is not True:
            continue
        if not isinstance(coordinate, str) or coordinate.count("@") != 1:
            failures.append(
                f"{settings_path}: enabled plugin coordinate {coordinate!r} "
                "must have the form plugin@marketplace"
            )
            continue
        plugin_name, marketplace_name = coordinate.split("@")
        marketplace = marketplaces.get(marketplace_name)
        label = f"{settings_path}: enabled plugin {coordinate}"
        if not isinstance(marketplace, dict):
            failures.append(f"{label} has no declared marketplace")
            continue
        if marketplace.get("autoUpdate") is not False:
            failures.append(f"{label} marketplace autoUpdate must be false")
        source = marketplace.get("source")
        if (
            not isinstance(source, dict)
            or source.get("source") != "settings"
            or source.get("name") != marketplace_name
        ):
            failures.append(
                f"{label} marketplace must be an inline settings marketplace"
            )
            continue
        plugins = source.get("plugins")
        if not isinstance(plugins, list):
            failures.append(f"{label} marketplace plugins must be an array")
            continue
        matches = [
            plugin
            for plugin in plugins
            if isinstance(plugin, dict) and plugin.get("name") == plugin_name
        ]
        if len(matches) != 1:
            failures.append(f"{label} must have exactly one inline marketplace entry")
            continue
        entry_contract = CLAUDE_PLUGIN_ENTRY_CONTRACTS.get(
            (marketplace_name, plugin_name)
        )
        if entry_contract is not None:
            for key, expected in entry_contract.items():
                if matches[0].get(key) != expected:
                    failures.append(f"{label} {key} must match the reviewed contract")
        plugin_source = matches[0].get("source")
        if not isinstance(plugin_source, dict):
            failures.append(f"{label} source must be an object")
            continue
        if plugin_source.get("source") != "git-subdir":
            failures.append(f"{label} source must use git-subdir")
        expected_url = CLAUDE_MARKETPLACE_REPOSITORIES.get(marketplace_name)
        if expected_url is None:
            failures.append(f"{label} marketplace is not an approved source")
        elif plugin_source.get("url") != expected_url:
            failures.append(f"{label} URL must be {expected_url}")
        expected_path = f"./plugins/{plugin_name}"
        if plugin_source.get("path") != expected_path:
            failures.append(f"{label} path must be {expected_path}")
        if "ref" in plugin_source:
            failures.append(f"{label} must use sha, not ref, for an exact pin")
        sha = plugin_source.get("sha")
        if not isinstance(sha, str) or IMMUTABLE_SHA.fullmatch(sha) is None:
            failures.append(f"{label} sha must be a full immutable commit SHA")


def validate_marketplaces(failures: list[str]) -> None:
    codex_path = ".agents/plugins/marketplace.json"
    marketplace = load_json(codex_path, failures)
    plugins = marketplace.get("plugins")
    if not isinstance(plugins, list) or not plugins:
        failures.append(f"{codex_path}: plugins must be a non-empty array")
        return

    codex_ievo_ref: str | None = None
    ievo_entries = 0
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
        if not isinstance(source, dict):
            failures.append(f"{label}.source must be an object")
            continue

        name = plugin.get("name")
        if name == "ievo":
            ievo_entries += 1
            if source.get("source") != "git-subdir":
                failures.append(f"{label}.source.source must be git-subdir")
            if source.get("url") != IEVO_REPOSITORY_URL:
                failures.append(f"{label}.source.url must be {IEVO_REPOSITORY_URL}")
            if source.get("path") != IEVO_PLUGIN_PATH:
                failures.append(f"{label}.source.path must be {IEVO_PLUGIN_PATH}")
            ref = source.get("ref")
            if not isinstance(ref, str) or IMMUTABLE_SHA.fullmatch(ref) is None:
                failures.append(
                    f"{label}.source.ref must be a full immutable commit SHA"
                )
            else:
                codex_ievo_ref = ref
        else:
            failures.append(f"{label}: unsupported plugin entry {name!r}")

    if ievo_entries != 1 or codex_ievo_ref is None:
        failures.append(f"{codex_path}: exactly one pinned ievo plugin is required")
    claude_path = ".claude/settings.json"
    settings = load_json(claude_path, failures)
    validate_claude_ievo_marketplace(
        settings,
        codex_ievo_ref,
        failures,
        claude_path,
    )
    validate_enabled_claude_plugin_pins(settings, failures, claude_path)
    validate_skill_parity(SKILLS_ROOT, CODEX_SKILLS_ROOT, failures)


def frontmatter_scalar(path: Path, key: str) -> str | None:
    """Return a single-line frontmatter value without interpreting YAML."""
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---", 4)
    if end == -1:
        return None
    prefix = f"{key}:"
    for line in text[4:end].splitlines():
        if line.startswith(prefix):
            return line[len(prefix) :].strip()
    return None


def validate_skill_parity(
    canonical_root: Path,
    codex_root: Path,
    failures: list[str],
) -> None:
    canonical = {path.parent.name: path for path in canonical_root.glob("*/SKILL.md")}
    wrappers = {path.parent.name: path for path in codex_root.glob("*/SKILL.md")}

    missing = sorted(canonical.keys() - wrappers.keys())
    extra = sorted(wrappers.keys() - canonical.keys())
    if missing:
        failures.append(
            ".agents/skills: missing canonical skill wrappers: " + ", ".join(missing)
        )
    if extra:
        failures.append(
            ".agents/skills: wrappers without canonical Claude skills: "
            + ", ".join(extra)
        )

    for name in sorted(canonical.keys() & wrappers.keys()):
        canonical_path = canonical[name]
        wrapper_path = wrappers[name]
        relative = (
            wrapper_path.relative_to(ROOT)
            if wrapper_path.is_relative_to(ROOT)
            else wrapper_path
        )
        if frontmatter_scalar(canonical_path, "name") != name:
            failures.append(
                f"{canonical_path}: frontmatter name must match directory {name}"
            )
        if frontmatter_scalar(wrapper_path, "name") != name:
            failures.append(f"{relative}: frontmatter name must be {name}")
        if frontmatter_scalar(wrapper_path, "description") != frontmatter_scalar(
            canonical_path, "description"
        ):
            failures.append(
                f"{relative}: description must match the canonical Claude skill"
            )

        text = wrapper_path.read_text(encoding="utf-8")
        normalized_text = " ".join(text.split())
        references = CANONICAL_SKILL_PATH.findall(text)
        expected = f".claude/skills/{name}/SKILL.md"
        if references != [(expected, name)]:
            failures.append(
                f"{relative}: must reference exactly the canonical {expected}"
            )
        if "completely" not in text or "canonical workflow" not in text:
            failures.append(
                f"{relative}: must require complete canonical workflow loading"
            )
        if frontmatter_scalar(canonical_path, "disable-model-invocation") == "true":
            explicit_name = f"`${name}`"
            description = frontmatter_scalar(wrapper_path, "description") or ""
            if not description.startswith(
                "Explicit invocation only; never select this skill implicitly."
            ):
                failures.append(
                    f"{relative}: explicit-only skill description must prevent "
                    "implicit selection"
                )
            if (
                "canonical workflow is explicit-only" not in normalized_text
                or explicit_name not in normalized_text
                or "selected implicitly" not in normalized_text
                or "stop without writing files" not in normalized_text
            ):
                failures.append(
                    f"{relative}: must preserve the canonical explicit-only "
                    "invocation gate"
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

    grill_path = SKILLS_ROOT / "grill-with-docs" / "SKILL.md"
    grill = grill_path.read_text(encoding="utf-8")
    if "docs/DECISIONS.md" not in grill:
        failures.append(
            ".claude/skills/grill-with-docs/SKILL.md: project-wide decisions "
            "must route to docs/DECISIONS.md"
        )
    if "never replaces it" not in grill:
        failures.append(
            ".claude/skills/grill-with-docs/SKILL.md: ADRs must not replace "
            "the canonical decision log"
        )


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
