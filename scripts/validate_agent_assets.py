#!/usr/bin/env python3
"""Validate repository-scoped agent skills and plugin configuration."""

from __future__ import annotations

import codecs
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable
from urllib.parse import unquote, urlparse, urlunparse


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
        "ref": "i-have-an-issue-v0.1.1",
        "reviewedCommit": "1bc6bcee3766a7e62b936343a48ebb56a3767470",
        "sourceType": "github",
        "skillPath": "skills/i-have-an-issue/SKILL.md",
        "computedHash": (
            "99e492ccae20ad3acf02e28dd76c7d74de28c7cf2141bfc7a2942c46c4bf687c"
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
ALPHA_EVAL_RUNNERS = {
    "pycc": {
        "build-and-run-self-created-fixture",
        "capture-parser-failure-without-write",
        "observe-current-check-fix-rejection",
    },
    "pycc-feedback": {
        "prepare-sanitized-draft-without-write",
        "refuse-private-automatic-publication",
        "require-exact-payload-preview",
    },
}
PROJECT_ALPHA_SKILLS = {"pycc", "pycc-feedback"}
# Required PR CI has no model credentials. Promotion stays fail-closed until
# reviewed, stable authenticated runs exist for both supported client surfaces.
AUTHENTICATED_MODEL_EVAL_EVIDENCE: dict[str, dict[str, str]] = {}
REQUIRED_MODEL_EVAL_CLIENTS = {"codex", "claude"}
PINNED_CLAUDE_PLUGINS = {"ievo@ievo-skills"}
INSTRUCTION_FILES = {"AGENTS.md", "CLAUDE.md"}
CLAUDE_SETTINGS_PATH = Path(".claude/settings.json")
CLAUDE_MARKETPLACE_DECLARATION_FIELDS = {
    "enabledPlugins",
    "extraKnownMarketplaces",
}
LOCAL_ACTION_MANIFESTS = {"action.yml", "action.yaml"}
SCRIPT_SUFFIXES = {
    ".bash",
    ".bat",
    ".cmd",
    ".cjs",
    ".fish",
    ".js",
    ".mjs",
    ".pl",
    ".ps1",
    ".py",
    ".rb",
    ".sh",
    ".ts",
    ".zsh",
}
SOURCE_SUFFIXES_WITH_INLINE_TESTS = {".c", ".cc", ".cpp", ".h", ".hpp", ".rs"}
SCP_GIT_REFERENCE = re.compile(
    r"^(?:[^/@:\s]+@)?(?P<host>[A-Za-z0-9.-]+):(?P<path>[^?#]+)$"
)


class RequiredAssetEncodingError(ValueError):
    """Raised when a required agent asset uses an unsupported text encoding."""


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
    validate_alpha_promotion_gate(entries, failures)
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


def validate_alpha_promotion_gate(
    locked_skills: dict[str, object],
    failures: list[str],
) -> None:
    for name in sorted(PROJECT_ALPHA_SKILLS & set(locked_skills)):
        evidence = AUTHENTICATED_MODEL_EVAL_EVIDENCE.get(name)
        if (
            not isinstance(evidence, dict)
            or set(evidence) != REQUIRED_MODEL_EVAL_CLIENTS
            or not all(
                isinstance(url, str) and url.startswith("https://")
                for url in evidence.values()
            )
        ):
            failures.append(
                f"skills-lock.json: {name} cannot be promoted without "
                "authenticated Codex and Claude model-eval evidence"
            )


def validate_claude_ievo_marketplace(
    settings: dict,
    codex_ievo_ref: str | None,
    failures: list[str],
    settings_path: str = ".claude/settings.json",
) -> None:
    enabled_plugins = settings.get("enabledPlugins")
    if (
        not isinstance(enabled_plugins, dict)
        or enabled_plugins.get("ievo@ievo-skills") is not True
    ):
        failures.append(
            f"{settings_path}: enabledPlugins must enable ievo@ievo-skills"
        )

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
    validate_optional_plugin_boundary(settings, failures)
    validate_skill_parity(SKILLS_ROOT, CODEX_SKILLS_ROOT, failures)


def optional_claude_plugins(settings: dict) -> dict[str, str]:
    enabled_plugins = settings.get("enabledPlugins")
    if not isinstance(enabled_plugins, dict):
        enabled_plugins = {}

    optional: dict[str, str] = {}
    for identity in enabled_plugins:
        if not isinstance(identity, str):
            continue
        name, separator, marketplace = identity.rpartition("@")
        if (
            separator
            and name
            and marketplace
            and identity not in PINNED_CLAUDE_PLUGINS
        ):
            optional[identity] = marketplace
    for identity in PINNED_CLAUDE_PLUGINS:
        if enabled_plugins.get(identity) is True:
            continue
        name, separator, marketplace = identity.rpartition("@")
        if separator and name and marketplace:
            optional[identity] = marketplace
    return optional


def declared_claude_marketplaces(settings: dict) -> set[str]:
    marketplaces = settings.get("extraKnownMarketplaces")
    if not isinstance(marketplaces, dict):
        return set()
    return {
        name
        for name in marketplaces
        if isinstance(name, str) and name
    }


def strip_git_suffix(reference: str) -> str:
    normalized = reference.strip().rstrip("/")
    if normalized.lower().endswith(".git"):
        normalized = normalized[:-4]
    return normalized.rstrip("/")


def repository_path_reference(
    path: str,
    *,
    minimum_parts: int = 2,
) -> str | None:
    normalized = strip_git_suffix(unquote(path).strip("/"))
    if len([part for part in normalized.split("/") if part]) < minimum_parts:
        return None
    return normalized


def optional_marketplace_source_references(
    settings: dict,
    pinned_marketplaces: set[str],
    failures: list[str],
) -> dict[str, tuple[str, str]]:
    marketplaces = settings.get("extraKnownMarketplaces")
    if not isinstance(marketplaces, dict):
        return {}

    references: dict[str, tuple[str, str]] = {}
    for alias, declaration in marketplaces.items():
        if (
            not isinstance(alias, str)
            or alias in pinned_marketplaces
            or not isinstance(declaration, dict)
        ):
            continue
        source = declaration.get("source")
        if not isinstance(source, dict):
            continue
        for key in ("repo", "url"):
            value = source.get(key)
            if not isinstance(value, str) or not value:
                continue
            raw = value.strip()
            if key == "repo":
                repository = repository_path_reference(raw)
                if repository is None:
                    failures.append(
                        f".claude/settings.json: optional marketplace {alias} "
                        "source.repo must identify at least an owner and repository"
                    )
                    continue
                references[raw] = (alias, repository)
                references[repository] = (alias, repository)
                continue

            scp_match = (
                SCP_GIT_REFERENCE.fullmatch(raw)
                if "://" not in raw
                else None
            )
            if scp_match is not None:
                repository = repository_path_reference(
                    scp_match.group("path"),
                    minimum_parts=1,
                )
                if repository is None:
                    failures.append(
                        f".claude/settings.json: optional marketplace {alias} "
                        "source.url must include a repository path"
                    )
                    continue
                host_reference = f"{scp_match.group('host')}/{repository}"
                references[raw] = (alias, host_reference)
                references[strip_git_suffix(raw)] = (alias, host_reference)
                references[host_reference] = (alias, host_reference)
                if len(repository.split("/")) >= 2:
                    references[repository] = (alias, repository)
                continue

            try:
                parsed = urlparse(raw)
                host = parsed.hostname
                port = parsed.port
            except ValueError:
                parsed = None
                host = None
                port = None
            if parsed is None or not parsed.scheme or not parsed.netloc or not host:
                failures.append(
                    f".claude/settings.json: optional marketplace {alias} "
                    "source.url must be a valid absolute or scp-style Git URL"
                )
                continue

            repository = repository_path_reference(
                parsed.path,
                minimum_parts=1,
            )
            if repository is None:
                failures.append(
                    f".claude/settings.json: optional marketplace {alias} "
                    "source.url must include a repository path"
                )
                continue

            public_host = host
            if ":" in public_host and not public_host.startswith("["):
                public_host = f"[{public_host}]"
            if port is not None:
                public_host = f"{public_host}:{port}"
            host_reference = f"{public_host}/{repository}"
            canonical_url = urlunparse(
                (parsed.scheme, public_host, f"/{repository}", "", "", "")
            )
            references[raw] = (alias, canonical_url)
            references[strip_git_suffix(raw)] = (alias, canonical_url)
            references[canonical_url] = (alias, canonical_url)
            references[host_reference] = (alias, host_reference)
            if len(repository.split("/")) >= 2:
                references[repository] = (alias, repository)
    return {
        token: metadata
        for token, metadata in references.items()
        if token
    }


def tracked_repository_files(root: Path) -> list[tuple[Path, str]]:
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "--stage", "-z"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(detail or "git ls-files failed")

    files: list[tuple[Path, str]] = []
    for record in result.stdout.split(b"\0"):
        if not record:
            continue
        metadata, separator, encoded_path = record.partition(b"\t")
        fields = metadata.split()
        if not separator or len(fields) != 3:
            raise RuntimeError("git ls-files returned an invalid staged record")
        mode = fields[0]
        if mode not in {b"100644", b"100755", b"120000"}:
            continue
        relative = Path(encoded_path.decode("utf-8", errors="surrogateescape"))
        files.append((root / relative, mode.decode("ascii")))
    return files


def is_test_asset(relative: Path) -> bool:
    name = relative.name
    return (
        "tests" in relative.parts
        or name.startswith("test_")
        or name.startswith("test-")
        or name.endswith("_test.py")
        or name.endswith("-test.sh")
    )


def is_required_agent_asset(relative: Path, executable: bool) -> bool:
    parts = relative.parts
    return (
        relative.name in INSTRUCTION_FILES
        or relative.name in LOCAL_ACTION_MANIFESTS
        or (parts and parts[0] in {".agents", ".claude", "agents"})
        or parts[:2] == (".ievo", "evolution")
        or parts[:2] == (".github", "workflows")
        or parts[:2] == (".github", "actions")
        or (parts and parts[0] == "scripts")
        or is_test_asset(relative)
        or relative.suffix.lower() in SCRIPT_SUFFIXES
        or relative.suffix.lower() in SOURCE_SUFFIXES_WITH_INLINE_TESTS
        or executable
    )


def required_agent_files(
    root: Path,
    repository_files: Iterable[tuple[Path, str]] | None = None,
) -> list[Path]:
    if repository_files is None:
        repository_files = tracked_repository_files(root)
    paths: set[Path] = set()
    for path, mode in repository_files:
        relative = path.relative_to(root)
        if not is_required_agent_asset(relative, mode == "100755"):
            continue
        if mode == "120000":
            raise RuntimeError(
                f"{relative.as_posix()}: required agent assets must not be symlinks"
            )
        paths.add(path)
    return sorted(paths)


def has_token(text: str, token: str) -> bool:
    boundary = r"[A-Za-z0-9_-]"
    return (
        re.search(
            rf"(?<!{boundary}){re.escape(token)}(?!{boundary})",
            text,
        )
        is not None
    )


def mask_token(text: str, token: str) -> str:
    boundary = r"[A-Za-z0-9_-]"
    return re.sub(
        rf"(?<!{boundary}){re.escape(token)}(?!{boundary})",
        lambda match: " " * len(match.group(0)),
        text,
    )


def required_asset_body(relative: Path, text: str) -> str:
    if relative == CLAUDE_SETTINGS_PATH:
        try:
            settings = json.loads(text)
        except json.JSONDecodeError:
            return text
        if not isinstance(settings, dict):
            return text
        behavioral_settings = {
            key: value
            for key, value in settings.items()
            if key not in CLAUDE_MARKETPLACE_DECLARATION_FIELDS
        }
        return json.dumps(
            behavioral_settings,
            ensure_ascii=False,
            sort_keys=True,
        )
    if relative.parts[:2] != (".ievo", "evolution"):
        return text
    normalized = text.replace("\r\n", "\n").replace("\r", "\n")
    if not normalized.startswith("---\n"):
        return normalized
    end = normalized.find("\n---\n", 4)
    if end == -1:
        return normalized
    frontmatter = normalized[4:end]
    body = normalized[end + len("\n---\n") :]

    source_repository: str | None = None
    in_source = False
    source_child_indent: int | None = None
    for line in frontmatter.splitlines():
        if line == "source:":
            in_source = True
            continue
        if not in_source or not line.strip() or line.lstrip().startswith("#"):
            continue
        indentation = len(line) - len(line.lstrip(" "))
        if indentation == 0:
            break
        if source_child_indent is None:
            source_child_indent = indentation
        if indentation != source_child_indent:
            continue
        match = re.fullmatch(r"repo:\s+([^\s#]+)\s*", line.strip())
        if match is not None:
            source_repository = match.group(1).strip("'\"")
            break
    if source_repository is None:
        return body

    provenance_heading = re.compile(
        r"(?m)^(## \d{4}-\d{2}-\d{2} — Vendored from )"
        rf"({re.escape(source_repository)})([ \t]*)$"
    )
    return provenance_heading.sub(
        lambda match: (
            match.group(1)
            + (" " * len(match.group(2)))
            + match.group(3)
        ),
        body,
        count=1,
    )


def decode_required_asset(data: bytes) -> str:
    if data.startswith((codecs.BOM_UTF32_LE, codecs.BOM_UTF32_BE)):
        raise RequiredAssetEncodingError("UTF-32 is not supported")
    if data.startswith(codecs.BOM_UTF8):
        text = data.decode("utf-8-sig")
    elif data.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)):
        text = data.decode("utf-16")
    else:
        text = data.decode("utf-8")
    if "\0" in text:
        raise RequiredAssetEncodingError("NUL bytes are not allowed")
    return text


def validate_optional_plugin_boundary(
    settings: dict,
    failures: list[str],
    root: Path = ROOT,
    repository_files: Iterable[tuple[Path, str]] | None = None,
) -> None:
    optional = optional_claude_plugins(settings)

    names: dict[str, list[str]] = {}
    for identity, marketplace in optional.items():
        name = identity.rpartition("@")[0]
        names.setdefault(name, []).append(identity)
    marketplaces = set(optional.values())
    pinned_marketplaces = {
        identity.rpartition("@")[2]
        for identity in PINNED_CLAUDE_PLUGINS
        if "@" in identity
    }
    marketplaces.update(
        declared_claude_marketplaces(settings).difference(pinned_marketplaces)
    )
    marketplace_sources = optional_marketplace_source_references(
        settings,
        pinned_marketplaces,
        failures,
    )
    enabled_pinned = PINNED_CLAUDE_PLUGINS.difference(optional)

    try:
        paths = required_agent_files(root, repository_files)
    except (OSError, RuntimeError, ValueError) as error:
        failures.append(f"agent asset discovery failed: {error}")
        return

    for path in paths:
        relative = path.relative_to(root).as_posix()
        try:
            text = decode_required_asset(path.read_bytes())
        except OSError as error:
            failures.append(
                f"{relative}: unable to read tracked required agent asset: {error}"
            )
            continue
        except (UnicodeDecodeError, RequiredAssetEncodingError) as error:
            failures.append(
                f"{relative}: required agent asset must be UTF-8 or BOM-tagged "
                f"UTF-16: {error}"
            )
            continue
        text = required_asset_body(Path(relative), text)
        pinned_violation: str | None = None
        boundary = r"[A-Za-z0-9_-]"
        for marketplace in pinned_marketplaces:
            pattern = re.compile(
                rf"(?<!{boundary})([A-Za-z0-9_-]+)@"
                rf"{re.escape(marketplace)}(?!{boundary})"
            )
            for match in pattern.finditer(text):
                identity = f"{match.group(1)}@{marketplace}"
                if identity not in enabled_pinned:
                    pinned_violation = identity
                    break
            if pinned_violation is not None:
                break
        if pinned_violation is not None:
            failures.append(
                f"{relative}: required agent asset references unvalidated Claude "
                f"plugin {pinned_violation}; pin it and provide Codex parity first"
            )
            continue

        for identity in sorted(optional, key=lambda value: (-len(value), value)):
            if has_token(text, identity):
                failures.append(
                    f"{relative}: required agent asset references optional Claude "
                    f"plugin {identity}; pin it and provide Codex parity first"
                )
                break
        else:
            fallback_text = text
            for identity in enabled_pinned:
                fallback_text = mask_token(fallback_text, identity)
            for name in sorted(names, key=lambda value: (-len(value), value)):
                if has_token(fallback_text, name):
                    identities = ", ".join(sorted(names[name]))
                    failures.append(
                        f"{relative}: required agent asset references optional "
                        f"Claude plugin name {name} ({identities}); pin it and "
                        "provide Codex parity first"
                    )
                    break
            else:
                for marketplace in sorted(
                    marketplaces,
                    key=lambda value: (-len(value), value),
                ):
                    if has_token(fallback_text, marketplace):
                        failures.append(
                            f"{relative}: required agent asset references optional "
                            f"Claude marketplace {marketplace}; pin the dependency "
                            "and provide Codex parity first"
                        )
                        break
                else:
                    for token in sorted(
                        marketplace_sources,
                        key=lambda value: (-len(value), value),
                    ):
                        if has_token(fallback_text, token):
                            alias, display = marketplace_sources[token]
                            failures.append(
                                f"{relative}: required agent asset references "
                                f"optional Claude marketplace source {display} "
                                f"({alias}); pin the dependency and provide Codex "
                                "parity first"
                            )
                            break


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
            continue
        if not all(
            isinstance(case, dict)
            and isinstance(case.get("id"), int)
            and isinstance(case.get("prompt"), str)
            and bool(case["prompt"].strip())
            and isinstance(case.get("expected_output"), str)
            and bool(case["expected_output"].strip())
            for case in cases
        ):
            failures.append(f"{evals_relative}: contains a malformed eval")
            continue
        identifiers = [case["id"] for case in cases]
        if len(identifiers) != len(set(identifiers)):
            failures.append(f"{evals_relative}: eval ids must be unique")
        runners = {
            case["runner"]
            for case in cases
            if isinstance(case.get("runner"), str)
        }
        if runners != ALPHA_EVAL_RUNNERS[name]:
            failures.append(
                f"{evals_relative}: must bind the complete executable runner set"
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
