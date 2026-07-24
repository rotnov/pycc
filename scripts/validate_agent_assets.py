#!/usr/bin/env python3
"""Validate repository-scoped agent skills and plugin configuration."""

from __future__ import annotations

import hashlib
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
CANONICAL_SKILL_PATH = re.compile(r"`(\.claude/skills/([a-z][a-z0-9-]*)/SKILL\.md)`")
MARKDOWN_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
SLASH_SKILL = re.compile(r"`/([a-z][a-z0-9-]+)`")
ABSOLUTE_OUTPUT = re.compile(
    r"(?i)(?:save|saved|write|written|output|destination).{0,160}`(/[^`]+)`"
)
EXPECTED_SKILL_LOCK_ENTRIES = {
    "i-have-an-issue": {
        "source": "rotnov/skills",
        "ref": "i-have-an-issue-v0.1.0",
        "reviewedCommit": "6cdefb4bfc3d73c43265e56530b85cab0703b3fa",
        "sourceType": "github",
        "skillPath": "skills/i-have-an-issue/SKILL.md",
        "computedHash": (
            "2a9cbea3a31c59aa42b4ea1c827bcc69982ef925be0400924625cbe773023b22"
        ),
    }
}
FEEDBACK_CONSENT_GUARDS = (
    "explicit approval",
    "exact payload",
    "rotnov/pycc",
    "search open and closed issues",
    "make no external change",
    "sanitize every outbound query",
    "user-authored code",
)


def load_json(
    relative_path: str,
    failures: list[str],
    root: Path = ROOT,
) -> dict:
    path = root / relative_path
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        failures.append(f"{relative_path}: invalid JSON: {error}")
        return {}
    if not isinstance(value, dict):
        failures.append(f"{relative_path}: top-level value must be an object")
        return {}
    return value


def compute_skill_folder_hash(skill_root: Path) -> str:
    """Match skills CLI 1.5.20's path-plus-content SHA-256."""
    files = [
        path
        for path in skill_root.rglob("*")
        if path.is_file()
        and "__pycache__" not in path.parts
        and path.suffix != ".pyc"
    ]
    files.sort(
        key=lambda path: path.relative_to(skill_root).as_posix().casefold()
    )
    digest = hashlib.sha256()
    for path in files:
        digest.update(path.relative_to(skill_root).as_posix().encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def validate_skill_lock(
    failures: list[str],
    root: Path = ROOT,
    skills_root: Path = SKILLS_ROOT,
) -> None:
    lock = load_json("skills-lock.json", failures, root)
    if lock.get("version") != 1:
        failures.append("skills-lock.json: version must be 1")
    entries = lock.get("skills")
    if not isinstance(entries, dict) or not entries:
        failures.append("skills-lock.json: skills must be a non-empty object")
        return

    expected_names = set(EXPECTED_SKILL_LOCK_ENTRIES)
    actual_names = set(entries)
    if actual_names != expected_names:
        failures.append(
            "skills-lock.json: locked skill set must be exactly "
            + ", ".join(sorted(expected_names))
        )

    policy_path = root / "docs" / "AGENT_TOOLING.md"
    try:
        policy = policy_path.read_text(encoding="utf-8")
    except OSError as error:
        failures.append(f"docs/AGENT_TOOLING.md: could not read policy: {error}")
        policy = ""

    for name, expected_entry in EXPECTED_SKILL_LOCK_ENTRIES.items():
        entry = entries.get(name)
        label = f"skills-lock.json: skills.{name}"
        if not isinstance(entry, dict):
            failures.append(f"{label} must be an object")
            continue
        for field, expected_value in expected_entry.items():
            if entry.get(field) != expected_value:
                failures.append(
                    f"{label}.{field} must be {expected_value!r}"
                )
        reviewed_commit = entry.get("reviewedCommit")
        if (
            not isinstance(reviewed_commit, str)
            or IMMUTABLE_SHA.fullmatch(reviewed_commit) is None
        ):
            failures.append(
                f"{label}.reviewedCommit must be a full immutable commit SHA"
            )

        skill_root = skills_root / name
        if not (skill_root / "SKILL.md").is_file():
            failures.append(f"{label} has no canonical .claude skill")
            continue
        expected_hash = expected_entry["computedHash"]
        locked_hash = entry.get("computedHash")
        if (
            not isinstance(locked_hash, str)
            or re.fullmatch(r"[0-9a-f]{64}", locked_hash) is None
        ):
            failures.append(f"{label}.computedHash must be a SHA-256 digest")
            continue
        actual = compute_skill_folder_hash(skill_root)
        if actual != expected_hash:
            failures.append(
                f"{label}.computedHash does not match the reviewed vendored skill: "
                f"expected {expected_hash}, got {actual}"
            )
        for field in ("ref", "reviewedCommit", "computedHash"):
            value = expected_entry[field]
            if value not in policy:
                failures.append(
                    f"docs/AGENT_TOOLING.md: missing {field} for {name}"
                )


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


def display_path(path: Path, root: Path) -> Path:
    try:
        return path.relative_to(root)
    except ValueError:
        return path


def validate_alpha_skill_contracts(
    skills_root: Path,
    failures: list[str],
    root: Path = ROOT,
) -> None:
    for name in ("pycc", "pycc-feedback"):
        path = skills_root / name / "SKILL.md"
        relative = display_path(path, root)
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            failures.append(f"{relative}: could not read alpha skill: {error}")
            continue
        if "alpha" not in text.lower():
            failures.append(f"{relative}: must remain visibly alpha")

        evals_path = skills_root / name / "evals" / "evals.json"
        evals_relative = display_path(evals_path, root)
        try:
            evals = json.loads(evals_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            failures.append(f"{evals_relative}: invalid evals: {error}")
            continue
        cases = evals.get("evals") if isinstance(evals, dict) else None
        skill_name = evals.get("skill_name") if isinstance(evals, dict) else None
        if skill_name != name or not isinstance(cases, list) or len(cases) < 2:
            failures.append(
                f"{evals_relative}: must define at least two evals for {name}"
            )

    feedback_path = skills_root / "pycc-feedback" / "SKILL.md"
    try:
        feedback = feedback_path.read_text(encoding="utf-8")
    except OSError:
        return
    normalized_feedback = " ".join(feedback.split())
    for required in FEEDBACK_CONSENT_GUARDS:
        if required not in normalized_feedback:
            failures.append(
                f"{display_path(feedback_path, root)}: missing consent guard "
                f"{required!r}"
            )


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

    validate_alpha_skill_contracts(SKILLS_ROOT, failures)


def main() -> int:
    failures: list[str] = []
    validate_marketplaces(failures)
    validate_skill_lock(failures)
    validate_skill_documents(failures)
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    print("agent assets: valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
