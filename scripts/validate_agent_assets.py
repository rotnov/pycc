#!/usr/bin/env python3
"""Validate repository-scoped agent skills and plugin configuration."""

from __future__ import annotations

import codecs
import hashlib
import ipaddress
import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath
from typing import Iterable, NamedTuple
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
    "using print()'s result as a nested expression is not supported yet",
    "intentional temporary",
)
ALPHA_EVAL_RUNNERS = {
    "pycc": {
        "build-and-run-self-created-fixture",
        "classify-planned-backend-boundary-without-write",
        "observe-current-check-fix-rejection",
    },
    "pycc-feedback": {
        "refuse-accepted-pr5-boundary-publication",
        "refuse-private-automatic-publication",
        "require-exact-payload-preview",
    },
    "issue-to-plan": {
        "refuse-publication-without-payload-preview",
        "refuse-publication-without-approval",
        "refuse-publication-after-payload-edited-post-approval",
    },
    "issue-implement": {
        "partial-resolution-never-closes",
        "refuse-write-on-unnamed-issue",
        "refuse-issue-supplied-shell-execution",
        "inconclusive-never-closes-on-suspicion",
        "delegated-autopilot-closure-authorized",
    },
    "issue-select": {
        "refuse-closure-without-autopilot",
        "priority-always-outranks-size",
        "refuse-issue-supplied-shell-execution",
    },
}
PROJECT_ALPHA_SKILLS = {"pycc", "pycc-feedback"}
# Required PR CI has no model credentials. Promotion stays fail-closed until
# reviewed, stable authenticated runs exist for both supported client surfaces.
AUTHENTICATED_MODEL_EVAL_EVIDENCE: dict[str, dict[str, str]] = {}
REQUIRED_MODEL_EVAL_CLIENTS = {"codex", "claude"}
PINNED_CLAUDE_PLUGINS = {"ievo@ievo-skills"}
INSTRUCTION_FILES = {"AGENTS.md", "CLAUDE.md"}
CLAUDE_INSTRUCTION_IMPORT = "@AGENTS.md\n"
LIVE_MONITORING_HEADING = (
    "## Monitor only live repository events "
    "([D-078](docs/DECISIONS.md#d-078-external-repository-monitoring-is-"
    "checkpointed-and-event-driven))"
)
REQUIRED_LIVE_MONITORING_BULLETS = (
    "Establish an explicit monitoring checkpoint from the refreshed remote "
    "default-branch commit. For every open pull request, record its number, "
    "state, draft status, and head commit; for every task-active pull request, "
    "also record mergeability, unresolved review threads, and required checks. "
    "Report the default-branch commit when monitoring starts or resumes.",
    "After the checkpoint, monitor only a newly observed default-branch commit; "
    "a newly opened or reopened pull request; a state, draft-status, or head "
    "change relative to the recorded baseline for an inventoried open pull "
    "request; or a mergeability, review-thread, or required-check change on a "
    "pull request already active in the current task. Ignore `updated_at` changes "
    "caused only by comments, reactions, labels, or other activity outside those "
    "fields.",
    "A pull request or issue cited only by documentation, an ADR, a retrospective, "
    "or a session log is historical evidence, not a live target. Do not poll a "
    "closed or merged pull request or a closed issue. Evaluate a task-active pull "
    "request's post-checkpoint close or merge once, then remove it from the live "
    "set; inspect an issue only when the active task explicitly names it.",
    "Before waiting on CI, query the pull request's current state, draft status, "
    "mergeability, head commit, and unresolved review threads. Stop waiting and "
    "re-evaluate when it closes, merges, becomes conflicting, or its head changes; "
    "never keep polling checks for a superseded head.",
    "When a new default-branch merge appears, inspect the introduced commit range "
    "and the exact post-merge workflows for that merge commit. Advance the "
    "checkpoint only after recording the new authoritative state, so the next "
    "cycle cannot rediscover the same event as new work.",
)
REQUIRED_LIVE_MONITORING_INSTRUCTIONS = (
    LIVE_MONITORING_HEADING,
    *REQUIRED_LIVE_MONITORING_BULLETS,
)
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
    r"^(?:(?P<user>[^/@:\s]+)@)?"
    r"(?P<host>\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9.-]+):"
    r"(?P<path>[^?#]+)$"
)
DOTTED_HOST_REFERENCE = re.compile(
    r"(?:[A-Za-z0-9-]+\.)+[A-Za-z0-9-]+"
)
SCP_GIT_REFERENCE_IN_TEXT = re.compile(
    r"(?<![A-Za-z0-9_.-])"
    r"(?:(?P<user>[^/@:\s]+)@)?"
    r"(?P<host>"
    r"\[[0-9A-Fa-f:.]+\]|"
    r"(?:[A-Za-z0-9-]+\.)+[A-Za-z0-9-]+"
    r"):"
    r"(?P<path>[A-Za-z0-9._~%+/-]+)"
)
HOST_PATH_REFERENCE_IN_TEXT = re.compile(
    r"(?<![A-Za-z0-9_.-])"
    r"(?P<host>"
    r"\[[0-9A-Fa-f:.]+\]|"
    r"(?:[A-Za-z0-9-]+\.)+[A-Za-z0-9-]+"
    r")"
    r"(?P<port>:\d+)?/"
    r"(?P<path>[A-Za-z0-9._~%+/-]+)"
)
URL_AUTHORITY_REFERENCE = re.compile(
    r"(?P<scheme>[A-Za-z][A-Za-z0-9+.-]*)://"
    r"(?P<authority>[^/\s?#]+)(?P<path>/[^\s?#]*)?"
)
DEFAULT_URL_PORTS = {
    "git": 9418,
    "http": 80,
    "https": 443,
    "ssh": 22,
}
INTERPRETER_REFERENCE = re.compile(
    r"(?<![A-Za-z0-9_.-])"
    r"(?P<interpreter>"
    r"python(?:\d+(?:\.\d+)*)?|ruby|node|perl|bash|sh|zsh|fish|"
    r"pwsh|powershell"
    r")(?:\.exe)?(?![A-Za-z0-9_.-])",
    re.IGNORECASE,
)
WORKFLOW_MAPPING_ENTRY = re.compile(
    r"^(?P<indent>[ ]*)(?P<sequence>-\s+)?"
    r"(?P<key>"
    r'"(?:\\.|[^"])*"|'
    r"'(?:''|[^'])*'|"
    r"[A-Za-z0-9_-]+"
    r")[ ]*:(?P<value>.*)$"
)
WORKFLOW_SEQUENCE_ENTRY = re.compile(
    r"^(?P<indent>[ ]*)-\s*(?:#.*)?$"
)
INTERPRETER_VALUE_OPTIONS = {
    "bash": {"--init-file", "--rcfile", "-O", "-o"},
    "fish": {
        "--debug",
        "--debug-output",
        "--features",
        "--init-command",
        "-C",
        "-D",
        "-d",
        "-f",
    },
    "node": {
        "--conditions",
        "--diagnostic-dir",
        "--experimental-loader",
        "--icu-data-dir",
        "--import",
        "--input-type",
        "--inspect-port",
        "--loader",
        "--openssl-config",
        "--require",
        "--title",
        "-C",
        "-r",
    },
    "perl": {"-F", "-I", "-M", "-m"},
    "python": {"--check-hash-based-pycs", "-W", "-X"},
    "ruby": {
        "--disable",
        "--dump",
        "--enable",
        "--encoding",
        "--external-encoding",
        "--internal-encoding",
        "-C",
        "-E",
        "-F",
        "-I",
        "-K",
        "-r",
    },
    "sh": {"-o"},
    "zsh": {"-o"},
}
INTERPRETER_REQUIRED_FILE_OPTIONS = {
    "bash": {"--init-file", "--rcfile"},
    "node": {
        "--experimental-loader",
        "--import",
        "--loader",
        "--require",
        "-r",
    },
    "ruby": {"-r"},
}
POWERSHELL_VALUE_OPTIONS = {
    "-configurationfile",
    "-configurationname",
    "-custompipename",
    "-executionpolicy",
    "-inputformat",
    "-outputformat",
    "-settingsfile",
    "-version",
    "-windowstyle",
    "-workingdirectory",
}


class RequiredAssetEncodingError(ValueError):
    """Raised when a required agent asset uses an unsupported text encoding."""


class WorkflowMappingEntry(NamedTuple):
    """A simple-key workflow mapping entry with its effective indentation."""

    line_index: int
    indent: int
    sequence: bool
    key: str
    value: str


class AgentScriptReference(NamedTuple):
    """A repository script reference and the cwd inherited by that script."""

    reference: str
    resolution_directory: str
    execution_directory: str


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
    if len(plugins) != 1:
        failures.append(
            f"{settings_path}: inline marketplace must contain only the pinned "
            "ievo plugin"
        )
        return
    ievo_plugin = plugins[0]
    if not isinstance(ievo_plugin, dict) or ievo_plugin.get("name") != "ievo":
        failures.append(
            f"{settings_path}: inline marketplace plugin must be the pinned ievo plugin"
        )
        return

    plugin_source = ievo_plugin.get("source")
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


def normalized_public_url_host(
    scheme: str,
    host: str,
    port: int | None,
) -> str:
    normalized_scheme = scheme.lower()
    public_host = host.lower()
    if ":" in public_host and not public_host.startswith("["):
        public_host = f"[{public_host}]"
    if port is not None and DEFAULT_URL_PORTS.get(normalized_scheme) != port:
        public_host = f"{public_host}:{port}"
    return public_host


def normalize_url_identity_components(text: str) -> str:
    def normalize(match: re.Match[str]) -> str:
        try:
            parsed = urlparse(match.group(0))
            host = parsed.hostname
            port = parsed.port
        except ValueError:
            return match.group(0)
        if not host:
            return match.group(0)
        scheme = parsed.scheme.lower()
        public_host = normalized_public_url_host(scheme, host, port)
        path = unquote(match.group("path") or "")
        return f"{scheme}://{public_host}{path}"

    return URL_AUTHORITY_REFERENCE.sub(normalize, text)


def normalize_scp_identity_components(text: str) -> str:
    def normalize(match: re.Match[str]) -> str:
        user = match.group("user")
        prefix = f"{user}@" if user else ""
        return (
            f"{prefix}{match.group('host').lower()}:"
            f"{unquote(match.group('path'))}"
        )

    return SCP_GIT_REFERENCE_IN_TEXT.sub(normalize, text)


def normalize_host_path_identity_components(text: str) -> str:
    return HOST_PATH_REFERENCE_IN_TEXT.sub(
        lambda match: (
            f"{match.group('host').lower()}"
            f"{match.group('port') or ''}/"
            f"{unquote(match.group('path'))}"
        ),
        text,
    )


def is_github_repository_host(host: str) -> bool:
    return host.lower() == "github.com"


def is_qualified_scp_host(host: str) -> bool:
    if DOTTED_HOST_REFERENCE.fullmatch(host) is not None:
        return True
    if not (host.startswith("[") and host.endswith("]")):
        return False
    try:
        ipaddress.IPv6Address(host[1:-1])
    except ipaddress.AddressValueError:
        return False
    return True


def optional_marketplace_source_references(
    settings: dict,
    pinned_marketplaces: set[str],
    failures: list[str],
) -> dict[str, set[tuple[str, str, bool]]]:
    marketplaces = settings.get("extraKnownMarketplaces")
    if not isinstance(marketplaces, dict):
        return {}

    references: dict[str, set[tuple[str, str, bool]]] = {}

    def add_reference(
        token: str,
        alias: str,
        display: str,
        case_sensitive: bool,
    ) -> None:
        normalized_token = token if case_sensitive else token.lower()
        if normalized_token:
            references.setdefault(normalized_token, set()).add(
                (alias, display, case_sensitive)
            )

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
                source_kind = source.get("source")
                case_sensitive = not (
                    isinstance(source_kind, str)
                    and source_kind.lower() == "github"
                )
                add_reference(
                    raw,
                    alias,
                    repository,
                    case_sensitive,
                )
                add_reference(
                    repository,
                    alias,
                    repository,
                    case_sensitive,
                )
                continue

            scp_match = (
                SCP_GIT_REFERENCE.fullmatch(raw)
                if "://" not in raw
                else None
            )
            if scp_match is not None:
                if not is_qualified_scp_host(scp_match.group("host")):
                    failures.append(
                        f".claude/settings.json: optional marketplace {alias} "
                        "source.url must use a fully qualified dotted or "
                        "bracketed IPv6 SCP host"
                    )
                    continue
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
                host = scp_match.group("host").lower()
                case_sensitive = not is_github_repository_host(host)
                if not case_sensitive:
                    repository = repository.lower()
                user = scp_match.group("user")
                prefix = f"{user}@" if user else ""
                host_reference = f"{host}/{repository}"
                canonical_scp = f"{prefix}{host}:{repository}"
                source_tokens = {
                    raw,
                    strip_git_suffix(raw),
                    canonical_scp,
                    f"{canonical_scp}.git",
                    host_reference,
                }
                for token in source_tokens:
                    add_reference(
                        token,
                        alias,
                        host_reference,
                        case_sensitive,
                    )
                if len(repository.split("/")) >= 2:
                    add_reference(
                        repository,
                        alias,
                        repository,
                        case_sensitive,
                    )
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

            scheme = parsed.scheme.lower()
            public_host = normalized_public_url_host(scheme, host, port)
            case_sensitive = not is_github_repository_host(host)
            if not case_sensitive:
                repository = repository.lower()
            host_reference = f"{public_host}/{repository}"
            canonical_url = urlunparse(
                (scheme, public_host, f"/{repository}", "", "", "")
            )
            source_tokens = {
                raw,
                strip_git_suffix(raw),
                canonical_url,
                host_reference,
            }
            for token in source_tokens:
                normalized_token = token if case_sensitive else token.lower()
                display = (
                    host_reference
                    if token == host_reference
                    else canonical_url
                )
                add_reference(
                    normalized_token,
                    alias,
                    display,
                    case_sensitive,
                )
            default_port = DEFAULT_URL_PORTS.get(scheme)
            if default_port is not None and port in {None, default_port}:
                default_port_token = (
                    f"{public_host}:{default_port}/{repository}"
                )
                add_reference(
                    default_port_token,
                    alias,
                    host_reference,
                    case_sensitive,
                )
            if len(repository.split("/")) >= 2:
                add_reference(
                    repository,
                    alias,
                    repository,
                    case_sensitive,
                )
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


def interpreter_arguments(text: str, start: int) -> list[str]:
    arguments: list[str] = []
    index = start
    while index < len(text):
        while index < len(text):
            if text[index] in " \t":
                index += 1
                continue
            if text.startswith("\\\r\n", index):
                index += 3
                continue
            if text.startswith("\\\n", index):
                index += 2
                continue
            break
        if index >= len(text) or text[index] in "\r\n;&|`)":
            break
        if text[index] == "#":
            break
        if (
            text[index] in "\"'"
            and index + 1 < len(text)
            and text[index + 1] in ",}]"
        ):
            break
        if (
            text.startswith("0<", index)
            and index + 2 < len(text)
            and text[index + 2] not in "<>&("
        ):
            arguments.append("<")
            index += 2
            continue
        if (
            text[index] == "<"
            and index + 1 < len(text)
            and text[index + 1] not in "<>&("
        ):
            arguments.append("<")
            index += 1
            continue

        escaped_quote = (
            text[index] == "\\"
            and index + 1 < len(text)
            and text[index + 1] in "\"'"
        )
        if escaped_quote or text[index] in "\"'":
            quote = text[index + 1] if escaped_quote else text[index]
            index += 2 if escaped_quote else 1
            value: list[str] = []
            while index < len(text):
                if escaped_quote and text.startswith(f"\\{quote}", index):
                    index += 2
                    break
                if not escaped_quote and text[index] == quote:
                    index += 1
                    break
                if text.startswith("\\\r\n", index):
                    index += 3
                    continue
                if text.startswith("\\\n", index):
                    index += 2
                    continue
                if text[index] == "\\" and index + 1 < len(text):
                    if text[index + 1] in {quote, "\\"}:
                        value.append(text[index + 1])
                        index += 2
                        continue
                    value.append("\\")
                    index += 1
                    continue
                value.append(text[index])
                index += 1
            arguments.append("".join(value))
            continue

        value = []
        command_ended = False
        while index < len(text):
            if text.startswith("\\\r\n", index):
                index += 3
                continue
            if text.startswith("\\\n", index):
                index += 2
                continue
            if text[index] in " \t\r\n;&|`)":
                command_ended = text[index] in "\r\n;&|`)"
                break
            if text[index] in "\"'":
                command_ended = True
                break
            if text[index] in ",}]":
                command_ended = True
                break
            if text[index] == "\\" and index + 1 < len(text):
                if text[index + 1] in " \t`)'\"\\":
                    value.append(text[index + 1])
                    index += 2
                    continue
                value.append("\\")
                index += 1
                continue
            value.append(text[index])
            index += 1
        if value:
            arguments.append("".join(value))
        if command_ended:
            break
    return arguments


def workflow_mapping_entries(lines: list[str]) -> list[WorkflowMappingEntry]:
    entries: list[WorkflowMappingEntry] = []
    for line_index, line in enumerate(lines):
        match = WORKFLOW_MAPPING_ENTRY.match(line)
        if match is None:
            sequence_match = WORKFLOW_SEQUENCE_ENTRY.match(line)
            if sequence_match is not None:
                entries.append(
                    WorkflowMappingEntry(
                        line_index=line_index,
                        indent=len(sequence_match.group("indent")) + 2,
                        sequence=True,
                        key="",
                        value="",
                    )
                )
            continue
        sequence = match.group("sequence") or ""
        entries.append(
            WorkflowMappingEntry(
                line_index=line_index,
                indent=len(match.group("indent")) + len(sequence),
                sequence=bool(sequence),
                key=yaml_inline_scalar(match.group("key")),
                value=match.group("value"),
            )
        )
    return entries


def workflow_entry_end(
    entries: list[WorkflowMappingEntry],
    entry_index: int,
) -> int:
    parent_indent = entries[entry_index].indent
    for candidate_index in range(entry_index + 1, len(entries)):
        if entries[candidate_index].indent <= parent_indent:
            return candidate_index
    return len(entries)


def workflow_direct_children(
    entries: list[WorkflowMappingEntry],
    entry_index: int,
) -> list[int]:
    end = workflow_entry_end(entries, entry_index)
    nested = [
        candidate_index
        for candidate_index in range(entry_index + 1, end)
        if entries[candidate_index].indent > entries[entry_index].indent
    ]
    if not nested:
        return []
    child_indent = min(entries[index].indent for index in nested)
    return [
        index
        for index in nested
        if entries[index].indent == child_indent
    ]


def workflow_child_entry(
    entries: list[WorkflowMappingEntry],
    entry_index: int,
    key: str,
) -> int | None:
    return next(
        (
            child
            for child in workflow_direct_children(entries, entry_index)
            if entries[child].key == key
        ),
        None,
    )


def workflow_descendant_entry(
    entries: list[WorkflowMappingEntry],
    entry_index: int,
    keys: Iterable[str],
) -> int | None:
    current = entry_index
    for key in keys:
        child = workflow_child_entry(entries, current, key)
        if child is None:
            return None
        current = child
    return current


def yaml_inline_scalar(value: str) -> str:
    quote: str | None = None
    index = 0
    while index < len(value):
        character = value[index]
        if quote == "'" and character == "'" and value[index : index + 2] == "''":
            index += 2
            continue
        if character in {"'", '"'}:
            if quote is None:
                quote = character
            elif quote == character:
                quote = None
            index += 1
            continue
        if (
            character == "#"
            and quote is None
            and (index == 0 or value[index - 1].isspace())
        ):
            value = value[:index]
            break
        index += 1

    scalar = value.strip()
    if len(scalar) >= 2 and scalar[0] == scalar[-1] == "'":
        return scalar[1:-1].replace("''", "'")
    if len(scalar) >= 2 and scalar[0] == scalar[-1] == '"':
        try:
            decoded = json.loads(scalar)
        except json.JSONDecodeError:
            return scalar[1:-1]
        return decoded if isinstance(decoded, str) else scalar
    return scalar


def reject_unsupported_workflow_structure(
    lines: list[str],
    entries: list[WorkflowMappingEntry],
    jobs_entry: int | None,
    additional_structural_entries: Iterable[int] = (),
) -> None:
    for line_index, line in enumerate(lines, start=1):
        if (
            re.match(r"^[ ]*-\s*[\[{]", line) is not None
            or re.match(r"^[\[{]", line) is not None
            or re.match(r"^[ ]*---[ ]+[\[{]", line) is not None
        ):
            raise RuntimeError(
                f"workflow line {line_index}: flow-style step mappings are "
                "not supported by agent asset discovery"
            )
        if re.match(r"^[ ]*(?:-\s+)?<<[ ]*:", line) is not None:
            raise RuntimeError(
                f"workflow line {line_index}: YAML merge keys are not "
                "supported by agent asset discovery"
            )

    structural_keys = {
        "defaults",
        "jobs",
        "run",
        "runs",
        "steps",
        "using",
        "working-directory",
    }
    job_entries = (
        set(workflow_direct_children(entries, jobs_entry))
        if jobs_entry is not None
        else set()
    )
    structural_entries = set(additional_structural_entries)
    for entry_index, entry in enumerate(entries):
        value = entry.value.lstrip()
        if not value or value[0] not in {"{", "[", "*"}:
            continue
        if (
            entry.key in structural_keys
            or entry_index in job_entries
            or entry_index in structural_entries
        ):
            raise RuntimeError(
                f"workflow line {entry.line_index + 1}: flow-style or aliased "
                "workflow structure is not supported by agent asset discovery"
            )


def workflow_entry_commands(
    lines: list[str],
    entry: WorkflowMappingEntry,
) -> list[str]:
    scalar = yaml_inline_scalar(entry.value)
    block_match = re.fullmatch(
        r"(?P<style>[>|])"
        r"(?:(?:[+-][1-9]?)|(?:[1-9][+-]?))?",
        scalar,
    )
    if block_match is None:
        command = scalar
        line_index = entry.line_index + 1
        while line_index < len(lines):
            line = lines[line_index]
            if not line.strip():
                if command:
                    command += "\n"
                line_index += 1
                continue
            indentation = len(line) - len(line.lstrip(" "))
            if indentation <= entry.indent:
                break
            content = line.strip()
            separator = "\n" if command.endswith("\\") else " "
            command = f"{command}{separator}{content}".strip()
            line_index += 1
        return [command] if command else []

    body: list[str] = []
    line_index = entry.line_index + 1
    while line_index < len(lines):
        line = lines[line_index]
        if not line.strip():
            body.append("")
            line_index += 1
            continue
        indentation = len(line) - len(line.lstrip(" "))
        if indentation <= entry.indent:
            break
        body.append(line)
        line_index += 1

    content_indents = [
        len(line) - len(line.lstrip(" "))
        for line in body
        if line.strip()
    ]
    if not content_indents:
        return []
    content_indent = min(content_indents)
    content = [
        line[content_indent:].strip() if line.strip() else ""
        for line in body
    ]
    if block_match.group("style") == "|":
        return ["\n".join(content)]

    commands: list[str] = []
    paragraph: list[str] = []
    for line in content:
        if line:
            paragraph.append(line)
            continue
        if paragraph:
            commands.append(" ".join(paragraph))
            paragraph = []
    if paragraph:
        commands.append(" ".join(paragraph))
    return commands


def static_working_directory(entry: WorkflowMappingEntry) -> str | None:
    value = yaml_inline_scalar(entry.value)
    if not value or "${{" in value or value.startswith("$"):
        return None
    if value in {".", "./"}:
        return ""
    return local_script_reference(value)


def step_working_directory_references(
    lines: list[str],
    entries: list[WorkflowMappingEntry],
    steps_entry: int,
    inherited_default: int | None,
    document_kind: str,
) -> set[AgentScriptReference]:
    references: set[AgentScriptReference] = set()
    step_entries = workflow_direct_children(entries, steps_entry)
    item_starts = [
        index
        for index in step_entries
        if entries[index].sequence
    ]
    steps_end = workflow_entry_end(entries, steps_entry)
    for position, item_start in enumerate(item_starts):
        item_end = (
            item_starts[position + 1]
            if position + 1 < len(item_starts)
            else steps_end
        )
        item_indent = entries[item_start].indent
        item_entries = [
            index
            for index in range(item_start, item_end)
            if entries[index].indent == item_indent
        ]
        step_default = next(
            (
                index
                for index in item_entries
                if entries[index].key == "working-directory"
            ),
            None,
        )
        effective_default = (
            step_default
            if step_default is not None
            else inherited_default
        )
        run_references: set[str] = set()
        for run_entry in (
            index
            for index in item_entries
            if entries[index].key == "run"
        ):
            for command in workflow_entry_commands(
                lines,
                entries[run_entry],
            ):
                run_references.update(
                    referenced_interpreter_scripts(command)
                )
        if not run_references:
            continue
        working_directory = ""
        if effective_default is not None:
            resolved_directory = static_working_directory(
                entries[effective_default]
            )
            if resolved_directory is None:
                raise RuntimeError(
                    f"{document_kind} line "
                    f"{entries[effective_default].line_index + 1}: "
                    "dynamic or non-repository working-directory cannot "
                    "be resolved for a relative interpreter script"
                )
            working_directory = resolved_directory
        references.update(
            AgentScriptReference(
                reference,
                working_directory,
                working_directory,
            )
            for reference in run_references
        )
    return references


def workflow_working_directory_references(
    text: str,
) -> set[AgentScriptReference]:
    lines = text.splitlines()
    entries = workflow_mapping_entries(lines)
    top_level = [
        index
        for index, entry in enumerate(entries)
        if entry.indent == 0 and not entry.sequence
    ]
    jobs_entry = next(
        (
            index
            for index in top_level
            if entries[index].key == "jobs"
        ),
        None,
    )
    reject_unsupported_workflow_structure(lines, entries, jobs_entry)
    if jobs_entry is None:
        return set()

    workflow_default: int | None = None
    workflow_defaults = next(
        (
            index
            for index in top_level
            if entries[index].key == "defaults"
        ),
        None,
    )
    if workflow_defaults is not None:
        workflow_default = workflow_descendant_entry(
            entries,
            workflow_defaults,
            ("run", "working-directory"),
        )

    references: set[AgentScriptReference] = set()
    for job_entry in workflow_direct_children(entries, jobs_entry):
        job_default = workflow_descendant_entry(
            entries,
            job_entry,
            ("defaults", "run", "working-directory"),
        )
        steps_entry = workflow_child_entry(entries, job_entry, "steps")
        if steps_entry is None:
            continue
        effective_default = (
            job_default
            if job_default is not None
            else workflow_default
        )
        references.update(
            step_working_directory_references(
                lines,
                entries,
                steps_entry,
                effective_default,
                "workflow",
            )
        )
    return references


def action_execution_references(
    text: str,
    action_directory: str,
) -> set[AgentScriptReference]:
    lines = text.splitlines()
    entries = workflow_mapping_entries(lines)
    top_level = [
        index
        for index, entry in enumerate(entries)
        if entry.indent == 0 and not entry.sequence
    ]
    runs_entry = next(
        (
            index
            for index in top_level
            if entries[index].key == "runs"
        ),
        None,
    )
    action_execution_entries = (
        {
            index
            for index in workflow_direct_children(entries, runs_entry)
            if entries[index].key in {"main", "pre", "post", "image"}
        }
        if runs_entry is not None
        else set()
    )
    reject_unsupported_workflow_structure(
        lines,
        entries,
        None,
        action_execution_entries,
    )
    if runs_entry is None:
        return set()

    references: set[AgentScriptReference] = set()
    steps_entry = workflow_child_entry(entries, runs_entry, "steps")
    if steps_entry is not None:
        references.update(
            step_working_directory_references(
                lines,
                entries,
                steps_entry,
                None,
                "action",
            )
        )

    for key in ("main", "pre", "post", "image"):
        entry_index = workflow_child_entry(entries, runs_entry, key)
        if entry_index is None:
            continue
        reference = local_script_reference(
            yaml_inline_scalar(entries[entry_index].value)
        )
        if reference is not None:
            references.add(
                AgentScriptReference(
                    reference,
                    action_directory,
                    "",
                )
            )
    return references


def resolve_working_directory_reference(
    reference: str,
    working_directory: str,
) -> str | None:
    parts: list[str] = []
    for part in PurePosixPath(working_directory, reference).parts:
        if part in {"", "."}:
            continue
        if part == "..":
            if not parts:
                raise RuntimeError(
                    "relative interpreter script path escapes the repository"
                )
            parts.pop()
            continue
        parts.append(part)
    if not parts:
        return None
    return PurePosixPath(*parts).as_posix()


def interpreter_family(interpreter: str) -> str:
    normalized = interpreter.lower().removesuffix(".exe")
    if normalized.startswith("python"):
        return "python"
    if normalized in {"powershell", "pwsh"}:
        return "powershell"
    return normalized


def is_inline_interpreter_option(family: str, option: str) -> bool:
    if family == "powershell":
        normalized = option.lower()
        return (
            normalized in {
                "--command",
                "--commandwithargs",
                "--encodedcommand",
                "-c",
                "-command",
                "-commandwithargs",
                "-e",
                "-ec",
                "-encodedcommand",
            }
            or normalized.startswith("--command=")
            or normalized.startswith("--command:")
            or normalized.startswith("--encodedcommand=")
            or normalized.startswith("--encodedcommand:")
            or normalized.startswith("-command=")
            or normalized.startswith("-command:")
            or normalized.startswith("-encodedcommand=")
            or normalized.startswith("-encodedcommand:")
        )
    if family == "python":
        return (
            option in {"-c", "-m"}
            or (option.startswith("-c") and not option.startswith("--"))
            or (option.startswith("-m") and not option.startswith("--"))
        )
    if family == "node":
        return (
            option in {"--eval", "--print", "-e", "-p"}
            or option.startswith("--eval=")
            or option.startswith("--print=")
            or (
                not option.startswith("--")
                and option.startswith(("-e", "-p"))
            )
        )
    if family == "perl":
        return (
            option in {"-E", "-e"}
            or (
                not option.startswith("--")
                and option.startswith(("-E", "-e"))
            )
        )
    if family == "ruby":
        return option == "-e" or (
            not option.startswith("--") and option.startswith("-e")
        )
    if family == "fish":
        return (
            option in {"--command", "-c"}
            or option.startswith("--command=")
            or (
                not option.startswith("--")
                and option.startswith("-c")
            )
        )
    if family in {"bash", "sh", "zsh"}:
        return option == "-c" or (
            not option.startswith("--") and option.startswith("-c")
        )
    return False


def option_value(
    family: str,
    option: str,
) -> tuple[str, str | None] | None:
    options = INTERPRETER_VALUE_OPTIONS.get(family, set())
    if family == "powershell":
        normalized = option.lower()
        for candidate in POWERSHELL_VALUE_OPTIONS:
            if normalized == candidate:
                return candidate, None
            for separator in ("=", ":"):
                prefix = f"{candidate}{separator}"
                if normalized.startswith(prefix):
                    return candidate, option[len(prefix) :]
        return None

    for candidate in sorted(options, key=len, reverse=True):
        if option == candidate:
            return candidate, None
        if candidate.startswith("--"):
            prefix = f"{candidate}="
            if option.startswith(prefix):
                return candidate, option[len(prefix) :]
        elif option.startswith(candidate) and len(option) > len(candidate):
            return candidate, option[len(candidate) :]
    return None


def local_script_reference(argument: str) -> str | None:
    normalized = argument.replace("\\", "/")
    if (
        not normalized
        or normalized == "-"
        or normalized.startswith(("/", "$"))
        or re.match(r"^[A-Za-z]:", normalized) is not None
        or "://" in normalized
    ):
        return None
    reference = normalized.removeprefix("./")
    parts = PurePosixPath(reference).parts
    if not parts or any(part in {"", "."} for part in parts):
        return None
    return PurePosixPath(*parts).as_posix()


def ambiguous_option_references(
    family: str,
    option: str,
    remaining_arguments: Iterable[str],
) -> set[str]:
    candidates: list[str] = []
    if option.startswith("-"):
        for separator in ("=", ":"):
            if separator in option:
                candidates.append(option.split(separator, 1)[1])
                break
    terminated = False
    for argument in remaining_arguments:
        if argument == "--" and not terminated:
            terminated = True
            continue
        if not terminated and argument.startswith("-"):
            value_option = option_value(family, argument)
            if value_option is not None:
                parsed_option, attached_value = value_option
                if (
                    attached_value is not None
                    and parsed_option
                    in INTERPRETER_REQUIRED_FILE_OPTIONS.get(family, set())
                ):
                    candidates.append(attached_value)
            for separator in ("=", ":"):
                if separator in argument:
                    candidates.append(argument.split(separator, 1)[1])
                    break
            continue
        candidates.append(argument)
    return {
        reference
        for candidate in candidates
        if (reference := local_script_reference(candidate)) is not None
    }


def referenced_interpreter_scripts(text: str) -> set[str]:
    references: set[str] = set()
    for match in INTERPRETER_REFERENCE.finditer(text):
        family = interpreter_family(match.group("interpreter"))
        arguments = interpreter_arguments(text, match.end())
        index = 0
        while index < len(arguments):
            argument = arguments[index]
            if argument == "<":
                index += 1
                if index < len(arguments):
                    reference = local_script_reference(arguments[index])
                    if reference is not None:
                        references.add(reference)
                break
            if argument == "--":
                index += 1
                if index < len(arguments) and arguments[index] == "<":
                    index += 1
                if index < len(arguments):
                    reference = local_script_reference(arguments[index])
                    if reference is not None:
                        references.add(reference)
                break
            if is_inline_interpreter_option(family, argument):
                break
            if family == "powershell":
                normalized = argument.lower()
                if normalized in {"-f", "-file"}:
                    index += 1
                    if index < len(arguments):
                        reference = local_script_reference(arguments[index])
                        if reference is not None:
                            references.add(reference)
                    break
                if normalized.startswith(("-file=", "-file:")):
                    reference = local_script_reference(argument[6:])
                    if reference is not None:
                        references.add(reference)
                    break
            value_option = option_value(family, argument)
            if value_option is not None:
                option, attached_value = value_option
                value = attached_value
                if value is None:
                    index += 1
                    if index < len(arguments):
                        value = arguments[index]
                if (
                    value is not None
                    and option
                    in INTERPRETER_REQUIRED_FILE_OPTIONS.get(family, set())
                ):
                    reference = local_script_reference(value)
                    if reference is not None:
                        references.add(reference)
                index += 1
                continue
            if argument.startswith("-"):
                references.update(
                    ambiguous_option_references(
                        family,
                        argument,
                        arguments[index + 1 :],
                    )
                )
                break
            reference = local_script_reference(argument)
            if reference is not None:
                references.add(reference)
            break
    return references


def required_agent_files(
    root: Path,
    repository_files: Iterable[tuple[Path, str]] | None = None,
) -> list[Path]:
    if repository_files is None:
        repository_files = tracked_repository_files(root)
    entries = list(repository_files)
    tracked = {
        path.relative_to(root).as_posix(): (path, mode)
        for path, mode in entries
    }
    paths: set[Path] = set()
    for path, mode in entries:
        relative = path.relative_to(root)
        if not is_required_agent_asset(relative, mode == "100755"):
            continue
        if mode == "120000":
            raise RuntimeError(
                f"{relative.as_posix()}: required agent assets must not be symlinks"
            )
        paths.add(path)

    pending = [(path, "") for path in paths]
    visited: set[tuple[Path, str]] = set()
    while pending:
        source, working_directory = pending.pop()
        context = (source, working_directory)
        if context in visited:
            continue
        visited.add(context)
        try:
            text = decode_required_asset(source.read_bytes())
        except (OSError, UnicodeDecodeError, RequiredAssetEncodingError):
            continue
        relative = source.relative_to(root)
        text = required_asset_body(relative, text)
        if relative.parts[:2] == (".github", "workflows"):
            references = workflow_working_directory_references(text)
        elif relative.name in LOCAL_ACTION_MANIFESTS:
            action_directory = relative.parent.as_posix()
            if action_directory == ".":
                action_directory = ""
            references = action_execution_references(
                text,
                action_directory,
            )
        else:
            references = {
                AgentScriptReference(
                    reference,
                    working_directory,
                    working_directory,
                )
                for reference in referenced_interpreter_scripts(text)
            }
        for script_reference in references:
            try:
                resolved_reference = resolve_working_directory_reference(
                    script_reference.reference,
                    script_reference.resolution_directory,
                )
            except RuntimeError as error:
                raise RuntimeError(
                    f"{relative.as_posix()}: {error}"
                ) from error
            if resolved_reference is None:
                continue
            tracked_entry = tracked.get(resolved_reference)
            if tracked_entry is None:
                continue
            target, mode = tracked_entry
            if mode == "120000":
                raise RuntimeError(
                    f"{resolved_reference}: required agent assets must not be symlinks"
                )
            paths.add(target)
            pending.append(
                (target, script_reference.execution_directory)
            )
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
                    source_text = normalize_host_path_identity_components(
                        normalize_scp_identity_components(
                            normalize_url_identity_components(fallback_text)
                        )
                    )
                    for token in sorted(
                        marketplace_sources,
                        key=lambda value: (-len(value), value),
                    ):
                        source_match: tuple[str, str] | None = None
                        for alias, display, case_sensitive in sorted(
                            marketplace_sources[token]
                        ):
                            candidate_text = (
                                source_text
                                if case_sensitive
                                else source_text.lower()
                            )
                            if has_token(candidate_text, token):
                                source_match = (alias, display)
                                break
                        if source_match is not None:
                            alias, display = source_match
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

def markdown_indent(line: str) -> tuple[int, int]:
    columns = 0
    for index, character in enumerate(line):
        if character == " ":
            columns += 1
        elif character == "\t":
            columns += 4 - (columns % 4)
        else:
            return columns, index
    return columns, len(line)


def markdown_columns(text: str) -> int:
    columns = 0
    for character in text:
        if character == "\t":
            columns += 4 - (columns % 4)
        else:
            columns += 1
    return columns


def markdown_blank(text: str) -> bool:
    return re.fullmatch(r"[ \t]*", text) is not None


def markdown_fence(
    line: str,
    *,
    opening: bool,
    min_indent: int = 0,
    max_indent: int = 3,
) -> tuple[str, str, int] | None:
    columns, content_start = markdown_indent(line)
    if columns < min_indent or columns > max_indent:
        return None
    match = re.match(r"(`{3,}|~{3,})(.*)$", line[content_start:])
    if match is None:
        return None
    marker, suffix = match.groups()
    if opening and marker[0] == "`" and "`" in suffix:
        return None
    return marker, suffix, columns


def markdown_list_fence(
    line: str,
    *,
    at_block_start: bool,
) -> tuple[str, str, int] | None:
    columns, content_start = markdown_indent(line)
    if columns >= 4:
        return None
    match = re.match(
        r"(?P<list_marker>[-+*]|[0-9]{1,9}[.)])"
        r"(?P<padding>[ \t]+)(?P<marker>`{3,}|~{3,})(?P<suffix>.*)$",
        line[content_start:],
    )
    if match is None:
        return None
    list_marker = match.group("list_marker")
    if not at_block_start and list_marker[0].isdigit() and list_marker[:-1] != "1":
        return None
    marker = match.group("marker")
    suffix = match.group("suffix")
    if marker[0] == "`" and "`" in suffix:
        return None
    marker_start = content_start + match.start("marker")
    list_marker_end = content_start + match.end("list_marker")
    marker_column = markdown_columns(line[:marker_start])
    padding_columns = marker_column - markdown_columns(line[:list_marker_end])
    if padding_columns > 4:
        return None
    return marker, suffix, marker_column


def markdown_html_comment_start(text: str) -> int | None:
    index = 0
    while index < len(text):
        if text[index] == "\\" and index + 1 < len(text):
            index += 2
            continue
        if text[index] == "`":
            run_end = index + 1
            while run_end < len(text) and text[run_end] == "`":
                run_end += 1
            run_length = run_end - index
            candidate = run_end
            while candidate < len(text):
                candidate = text.find("`", candidate)
                if candidate == -1:
                    break
                candidate_end = candidate + 1
                while candidate_end < len(text) and text[candidate_end] == "`":
                    candidate_end += 1
                if candidate_end - candidate == run_length:
                    index = candidate_end
                    break
                candidate = candidate_end
            else:
                return None
            if candidate == -1:
                index = run_end
            continue
        if text.startswith("<!--", index):
            return index
        index += 1
    return None


def markdown_list_comment_kind(
    prefix: str,
    *,
    at_block_start: bool,
) -> str | None:
    remainder = prefix
    consumed_container = False
    while True:
        columns, content_start = markdown_indent(remainder)
        if columns >= 4:
            return "literal" if consumed_container else None
        content = remainder[content_start:]
        if content.startswith(">"):
            consumed_container = True
            remainder = content[1:]
            if remainder.startswith((" ", "\t")):
                remainder = remainder[1:]
            at_block_start = True
            continue
        match = re.match(
            r"(?P<marker>[-+*]|[0-9]{1,9}[.)])(?P<padding>[ \t]+)",
            content,
        )
        if match is None:
            break
        marker = match.group("marker")
        if not at_block_start and marker[0].isdigit() and marker[:-1] != "1":
            return None
        marker_end = match.end("marker")
        body_start = match.end("padding")
        padding_columns = markdown_columns(content[:body_start]) - markdown_columns(
            content[:marker_end]
        )
        consumed_container = True
        if padding_columns > 4:
            return "literal"
        remainder = content[body_start:]
        at_block_start = True
    if consumed_container and markdown_blank(remainder):
        return "block"
    return None


COMMONMARK_HTML_BLOCK_TAGS = (
    "address|article|aside|base|basefont|blockquote|body|caption|center|col|"
    "colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|"
    "footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|"
    "link|main|menu|menuitem|nav|noframes|ol|optgroup|option|p|param|search|"
    "section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul"
)


def markdown_html_block_start(
    content: str,
    *,
    at_block_start: bool,
) -> tuple[str | None, bool] | None:
    raw_tag = re.match(r"<(script|pre|style|textarea)(?:[ \t]|>|$)", content, re.I)
    if raw_tag is not None:
        return f"</{raw_tag.group(1).lower()}>", False
    if content.startswith("<?"):
        return "?>", False
    if content.startswith("<![CDATA["):
        return "]]>", False
    if re.match(r"<![A-Z]", content):
        return ">", False
    if re.match(rf"</?(?:{COMMONMARK_HTML_BLOCK_TAGS})(?:[ \t]|/?>|$)", content, re.I):
        return None, True
    if at_block_start:
        attribute_name = r"[A-Za-z_:][A-Za-z0-9_.:-]*"
        attribute_value = r'''(?:[^ "'=<>`\x00-\x20]+|'[^']*'|"[^"]*")'''
        attribute = rf"[ \t]+{attribute_name}(?:[ \t]*=[ \t]*{attribute_value})?"
        open_tag = rf"<[A-Za-z][A-Za-z0-9-]*(?:{attribute})*[ \t]*/?>"
        close_tag = r"</[A-Za-z][A-Za-z0-9-]*[ \t]*>"
        if re.fullmatch(rf"(?:{open_tag}|{close_tag})[ \t]*", content):
            return None, True
    return None


def markdown_interrupts_paragraph(line: str) -> bool:
    columns, content_start = markdown_indent(line)
    if columns > 3:
        return False
    content = line[content_start:]
    compact = re.sub(r"[ \t]", "", content)
    return bool(
        re.match(r"#{1,6}(?:[ \t]+|$)", content)
        or re.match(r">(?:[ \t]+|$)", content)
        or markdown_fence(line, opening=True) is not None
        or re.match(r"(?:[-+*]|1[.)])[ \t]+", content)
        or (
            len(compact) >= 3
            and compact[0] in "*-_"
            and set(compact) == {compact[0]}
        )
        or markdown_html_block_start(content, at_block_start=False) is not None
    )


def markdown_line_ends_paragraph(
    content: str,
    *,
    at_block_start: bool,
) -> bool:
    compact = re.sub(r"[ \t]", "", content)
    if re.match(r"#{1,6}(?:[ \t]+|$)", content):
        return True
    if (
        len(compact) >= 3
        and compact[0] in "*-_"
        and set(compact) == {compact[0]}
    ):
        return True
    if not at_block_start and re.fullmatch(r"[=-]+[ \t]*", content):
        return True
    return markdown_list_item_starts_block(
        content,
        at_block_start=at_block_start,
    )


def markdown_list_item_starts_block(
    content: str,
    *,
    at_block_start: bool,
) -> bool:
    compact = re.sub(r"[ \t]", "", content)
    if (
        len(compact) >= 3
        and compact[0] in "*-_"
        and set(compact) == {compact[0]}
    ):
        return False
    list_item = re.match(
        r"(?P<marker>[-+*]|[0-9]{1,9}[.)])"
        r"(?:(?P<padding>[ \t]+)(?P<body>.*)|$)",
        content,
    )
    if list_item is None:
        return False
    if at_block_start:
        return True
    marker = list_item.group("marker")
    body = list_item.group("body") or ""
    return not markdown_blank(body) and (
        not marker[0].isdigit() or marker[:-1] == "1"
    )


def markdown_list_item_has_lazy_paragraph(content: str) -> bool:
    list_item = re.match(
        r"(?P<marker>[-+*]|[0-9]{1,9}[.)])(?P<padding>[ \t]+)(?P<body>.*)",
        content,
    )
    if list_item is None:
        return False
    marker_end = list_item.end("marker")
    body_start = list_item.start("body")
    padding_columns = markdown_columns(content[:body_start]) - markdown_columns(
        content[:marker_end]
    )
    body = list_item.group("body")
    return padding_columns <= 4 and markdown_container_body_has_lazy_paragraph(body)


def markdown_list_item_structure(
    line: str,
) -> tuple[tuple[int, ...], int | None]:
    columns, content_start = markdown_indent(line)
    if columns > 3:
        return (), None
    content = line[content_start:]
    compact = re.sub(r"[ \t]", "", content)
    if (
        len(compact) >= 3
        and compact[0] in "*-_"
        and set(compact) == {compact[0]}
    ):
        return (), None
    list_item = re.match(
        r"(?P<marker>[-+*]|[0-9]{1,9}[.)])"
        r"(?:(?P<padding>[ \t]+)(?P<body>.*)|$)",
        content,
    )
    if list_item is None:
        return (), None
    marker_end = list_item.end("marker")
    padding = list_item.group("padding")
    if padding is None:
        item_indent = columns + markdown_columns(content[:marker_end]) + 1
        body = ""
    else:
        body_start = list_item.start("body")
        padding_columns = markdown_columns(content[:body_start]) - markdown_columns(
            content[:marker_end]
        )
        if padding_columns > 4:
            item_indent = columns + markdown_columns(content[:marker_end]) + 1
            body = content[marker_end + 1 :]
        else:
            item_indent = columns + markdown_columns(content[:body_start])
            body = list_item.group("body") or ""
    nested, nested_empty_indent = markdown_list_item_structure(body)
    indents = (item_indent,) + tuple(item_indent + indent for indent in nested)
    empty_indent = (
        item_indent + nested_empty_indent
        if nested_empty_indent is not None
        else item_indent if markdown_blank(body) else None
    )
    return indents, empty_indent


def markdown_container_body_has_lazy_paragraph(body: str) -> bool:
    if markdown_blank(body):
        return False
    columns, content_start = markdown_indent(body)
    if columns > 3:
        return False
    content = body[content_start:]
    if content.startswith(">"):
        nested = content[1:]
        if nested.startswith((" ", "\t")):
            nested = nested[1:]
        return markdown_container_body_has_lazy_paragraph(nested)
    nested_list = re.match(
        r"(?P<marker>[-+*]|[0-9]{1,9}[.)])(?P<padding>[ \t]+)(?P<body>.*)",
        content,
    )
    if nested_list is not None:
        marker_end = nested_list.end("marker")
        body_start = nested_list.start("body")
        padding_columns = markdown_columns(content[:body_start]) - markdown_columns(
            content[:marker_end]
        )
        return padding_columns <= 4 and markdown_container_body_has_lazy_paragraph(
            nested_list.group("body")
        )
    compact = re.sub(r"[ \t]", "", content)
    return not (
        content.startswith("<!--")
        or re.match(r"#{1,6}(?:[ \t]+|$)", content)
        or markdown_fence(body, opening=True) is not None
        or (
            len(compact) >= 3
            and compact[0] in "*-_"
            and set(compact) == {compact[0]}
        )
        or markdown_html_block_start(content, at_block_start=True) is not None
    )


def visible_markdown_lines(text: str) -> list[str]:
    visible_lines: list[str] = []
    in_html_comment = False
    html_comment_is_block = False
    in_html_block = False
    html_block_terminator: str | None = None
    html_block_until_blank = False
    fence_character: str | None = None
    fence_length = 0
    fence_close_min_indent = 0
    fence_close_indent = 3
    fence_is_list_nested = False
    at_block_start = True
    lazy_container_active = False
    lazy_container_saw_blank = False
    lazy_list_indents: tuple[int, ...] = ()
    empty_list_indent: int | None = None

    for raw_line in text.split("\n"):
        if fence_character is not None:
            columns, _ = markdown_indent(raw_line)
            container_ended = (
                fence_is_list_nested
                and not markdown_blank(raw_line)
                and columns < fence_close_min_indent
            )
            if container_ended:
                lazy_list_indents = tuple(
                    indent for indent in lazy_list_indents if indent <= columns
                )
                fence_character = None
                fence_length = 0
                fence_close_min_indent = 0
                fence_close_indent = 3
                fence_is_list_nested = False
                at_block_start = True
                lazy_container_active = False
                lazy_container_saw_blank = False
            else:
                fence_closed = False
                fence_match = markdown_fence(
                    raw_line,
                    opening=False,
                    min_indent=fence_close_min_indent,
                    max_indent=fence_close_indent,
                )
                if fence_match is not None:
                    marker, suffix, _ = fence_match
                    if (
                        marker[0] == fence_character
                        and len(marker) >= fence_length
                        and markdown_blank(suffix)
                    ):
                        fence_character = None
                        fence_length = 0
                        fence_close_min_indent = 0
                        fence_close_indent = 3
                        fence_is_list_nested = False
                        fence_closed = True
                at_block_start = fence_closed or markdown_blank(raw_line)
                continue

        if in_html_block:
            html_block_ended = False
            if html_block_until_blank:
                if markdown_blank(raw_line):
                    in_html_block = False
                    html_block_until_blank = False
                    html_block_ended = True
            elif (
                html_block_terminator is not None
                and html_block_terminator in raw_line.lower()
            ):
                in_html_block = False
                html_block_terminator = None
                html_block_ended = True
            at_block_start = html_block_ended or markdown_blank(raw_line)
            continue

        if in_html_comment:
            inline_comment_ended = not html_comment_is_block and (
                markdown_blank(raw_line) or markdown_interrupts_paragraph(raw_line)
            )
            if inline_comment_ended:
                in_html_comment = False
                at_block_start = True
                if markdown_blank(raw_line):
                    visible_lines.append("")
                    continue
            else:
                if "-->" in raw_line:
                    comment_was_block = html_comment_is_block
                    in_html_comment = False
                    html_comment_is_block = False
                    at_block_start = comment_was_block
                else:
                    at_block_start = markdown_blank(raw_line)
                continue

        if markdown_blank(raw_line):
            visible_lines.append("")
            at_block_start = True
            if empty_list_indent is not None:
                lazy_list_indents = tuple(
                    indent
                    for indent in lazy_list_indents
                    if indent < empty_list_indent
                )
                empty_list_indent = None
            if lazy_container_active:
                lazy_container_saw_blank = True
            continue

        columns, content_start = markdown_indent(raw_line)
        if empty_list_indent is not None:
            if columns < empty_list_indent:
                lazy_list_indents = tuple(
                    indent for indent in lazy_list_indents if indent <= columns
                )
            empty_list_indent = None
        if (
            lazy_list_indents
            and (not lazy_container_active or lazy_container_saw_blank)
            and columns < lazy_list_indents[0]
        ):
            lazy_list_indents = ()

        continuation_fence_match: tuple[str, str, int] | None = None
        continuation_fence_indent = 0
        for indent in reversed(lazy_list_indents):
            candidate = markdown_fence(
                raw_line,
                opening=True,
                min_indent=indent,
                max_indent=indent + 3,
            )
            if candidate is not None:
                continuation_fence_match = candidate
                continuation_fence_indent = indent
                break
        if continuation_fence_match is not None:
            marker, _, _ = continuation_fence_match
            fence_character = marker[0]
            fence_length = len(marker)
            fence_close_min_indent = continuation_fence_indent
            fence_close_indent = continuation_fence_indent + 3
            fence_is_list_nested = True
            lazy_list_indents = tuple(
                indent
                for indent in lazy_list_indents
                if indent <= continuation_fence_indent
            )
            at_block_start = False
            lazy_container_active = False
            lazy_container_saw_blank = False
            continue
        if columns >= 4:
            continue

        fence_match = markdown_fence(raw_line, opening=True)
        if fence_match is not None:
            marker, _, _ = fence_match
            fence_character = marker[0]
            fence_length = len(marker)
            fence_close_min_indent = 0
            fence_close_indent = 3
            fence_is_list_nested = False
            at_block_start = False
            lazy_container_active = False
            lazy_container_saw_blank = False
            lazy_list_indents = ()
            continue
        list_fence_match = markdown_list_fence(
            raw_line,
            at_block_start=at_block_start,
        )
        if list_fence_match is not None:
            marker, _, marker_column = list_fence_match
            fence_character = marker[0]
            fence_length = len(marker)
            fence_close_min_indent = marker_column
            fence_close_indent = marker_column + 3
            fence_is_list_nested = True
            lazy_list_indents = (
                *(
                    indent
                    for indent in lazy_list_indents
                    if indent <= columns
                ),
                marker_column,
            )
            at_block_start = False
            lazy_container_active = False
            lazy_container_saw_blank = False
            continue
        content = raw_line[content_start:]
        if content.startswith(">"):
            at_block_start = True
            nested = content[1:]
            if nested.startswith((" ", "\t")):
                nested = nested[1:]
            lazy_container_active = markdown_container_body_has_lazy_paragraph(nested)
            lazy_container_saw_blank = False
            lazy_list_indents = tuple(
                indent for indent in lazy_list_indents if indent <= columns
            )
            continue
        if content.startswith("<!--"):
            if "-->" not in raw_line[content_start + 4 :]:
                in_html_comment = True
                html_comment_is_block = True
            at_block_start = True
            lazy_container_active = False
            lazy_container_saw_blank = False
            lazy_list_indents = tuple(
                indent for indent in lazy_list_indents if indent <= columns
            )
            continue
        html_block = markdown_html_block_start(
            content,
            at_block_start=at_block_start or lazy_container_active,
        )
        if html_block is not None:
            terminator, until_blank = html_block
            if until_blank or (
                terminator is not None and terminator not in content.lower()
            ):
                in_html_block = True
                html_block_terminator = terminator
                html_block_until_blank = until_blank
            at_block_start = not in_html_block
            lazy_container_active = False
            lazy_container_saw_blank = False
            lazy_list_indents = tuple(
                indent for indent in lazy_list_indents if indent <= columns
            )
            continue

        visible_parts: list[str] = []
        remainder = raw_line
        line_started_at_block = at_block_start
        while remainder:
            if in_html_comment:
                comment_end = remainder.find("-->")
                if comment_end == -1:
                    remainder = ""
                    continue
                remainder = remainder[comment_end + 3 :]
                in_html_comment = False
                continue

            comment_start = markdown_html_comment_start(remainder)
            if comment_start is None:
                visible_parts.append(remainder)
                remainder = ""
            else:
                visible_parts.append(remainder[:comment_start])
                comment_kind = markdown_list_comment_kind(
                    remainder[:comment_start],
                    at_block_start=line_started_at_block,
                )
                if comment_kind == "literal":
                    visible_parts.append(remainder[comment_start:])
                    remainder = ""
                    continue
                html_comment_is_block = comment_kind == "block"
                remainder = remainder[comment_start + 4 :]
                in_html_comment = True

        line = "".join(visible_parts)
        columns, content_start = markdown_indent(line)
        if columns >= 4 or line[content_start:].startswith(">"):
            continue
        visible_lines.append(line)
        line_started_list = markdown_list_item_starts_block(
            content,
            at_block_start=at_block_start,
        )
        line_ended_paragraph = markdown_line_ends_paragraph(
            content,
            at_block_start=at_block_start,
        )
        if line_started_list:
            lazy_container_active = markdown_list_item_has_lazy_paragraph(content)
            lazy_container_saw_blank = False
            new_list_indents, empty_list_indent = markdown_list_item_structure(
                raw_line
            )
            lazy_list_indents = (
                *(
                    indent
                    for indent in lazy_list_indents
                    if indent <= columns
                ),
                *new_list_indents,
            )
        elif lazy_container_active and (
            lazy_container_saw_blank or line_ended_paragraph
        ):
            lazy_container_active = False
            lazy_container_saw_blank = False
            lazy_list_indents = tuple(
                indent for indent in lazy_list_indents if indent <= columns
            )
        at_block_start = line_ended_paragraph

    return visible_lines


def markdown_section_paragraph_open(line: str, *, was_open: bool) -> bool:
    if markdown_blank(line):
        return False
    columns, content_start = markdown_indent(line)
    if columns > 3:
        return was_open
    content = line[content_start:]
    compact = re.sub(r"[ \t]", "", content)
    if (
        len(compact) >= 3
        and compact[0] in "*-_"
        and set(compact) == {compact[0]}
    ):
        return False
    if markdown_list_item_starts_block(
        content,
        at_block_start=not was_open,
    ):
        return False
    return True


def markdown_escapable(character: str) -> bool:
    return bool(re.fullmatch(r"[!-/:-@\[-`{-~]", character))


def markdown_reference_label_fragment(
    content: str,
    *,
    source_length: int = 0,
    has_nonspace: bool = False,
) -> tuple[str, int, bool, str | None]:
    index = 0
    while index < len(content):
        character = content[index]
        if character == "\\" and index + 1 < len(content):
            escaped = content[index + 1]
            if markdown_escapable(escaped):
                source_length += 2
                if source_length > 999:
                    return "invalid", source_length, has_nonspace, None
                has_nonspace = has_nonspace or not escaped.isspace()
                index += 2
                continue
        if character == "[":
            return "invalid", source_length, has_nonspace, None
        if character == "]":
            if index + 1 >= len(content) or content[index + 1] != ":":
                return "invalid", source_length, has_nonspace, None
            if source_length > 999 or not has_nonspace:
                return "invalid", source_length, has_nonspace, None
            return "complete", source_length, has_nonspace, content[index + 2 :]
        source_length += 1
        if source_length > 999:
            return "invalid", source_length, has_nonspace, None
        has_nonspace = has_nonspace or not character.isspace()
        index += 1
    return "incomplete", source_length, has_nonspace, None


def markdown_reference_destination_state(text: str) -> tuple[bool, str | None]:
    remainder = text.lstrip(" \t")
    if not remainder:
        return False, None
    if remainder.startswith("<"):
        end = 1
        escaped = False
        while end < len(remainder):
            character = remainder[end]
            if character in "\r\n":
                return False, None
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == "<":
                return False, None
            elif character == ">":
                break
            end += 1
        if end >= len(remainder):
            return False, None
        remainder = remainder[end + 1 :].lstrip(" \t")
    else:
        index = 0
        depth = 0
        while index < len(remainder):
            character = remainder[index]
            if character in " \t":
                break
            if ord(character) < 0x20 or character == "\x7f":
                return False, None
            if character == "\\" and index + 1 < len(remainder):
                escaped = remainder[index + 1]
                if escaped in " \t":
                    return False, None
                if markdown_escapable(escaped):
                    index += 2
                    continue
            elif character == "(":
                depth += 1
                if depth > 32:
                    return False, None
            elif character == ")":
                if depth == 0:
                    return False, None
                depth -= 1
            index += 1
        if index == 0 or depth != 0:
            return False, None
        remainder = remainder[index:].lstrip(" \t")
    if not remainder:
        return True, "may_title"
    if remainder[0] not in "\"'(":
        return False, None
    closer = ")" if remainder[0] == "(" else remainder[0]
    escaped = False
    for index, character in enumerate(remainder[1:], start=1):
        if escaped:
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == closer:
            return markdown_blank(remainder[index + 1 :]), None
    return True, f"title:{closer}"


def markdown_reference_definition_state(line: str) -> tuple[bool, str | None]:
    columns, content_start = markdown_indent(line)
    if columns > 3:
        return False, None
    content = line[content_start:]
    if not content.startswith("["):
        return False, None
    status, source_length, has_nonspace, remainder = markdown_reference_label_fragment(
        content[1:]
    )
    if status == "invalid":
        return False, None
    if status == "incomplete":
        return True, f"label:{source_length}:{int(has_nonspace)}"
    assert remainder is not None
    if markdown_blank(remainder):
        return True, "destination"
    return markdown_reference_destination_state(remainder)


def markdown_reference_continuation_state(
    line: str,
    state: str,
) -> tuple[bool, str | None]:
    columns, content_start = markdown_indent(line)
    content = line[content_start:]
    if state.startswith("label:"):
        if markdown_blank(line):
            return False, None
        _, source_text, nonspace_text = state.split(":")
        status, source_length, has_nonspace, remainder = (
            markdown_reference_label_fragment(
                content,
                source_length=int(source_text) + 1,
                has_nonspace=bool(int(nonspace_text)),
            )
        )
        if status == "invalid":
            return False, None
        if status == "incomplete":
            return True, f"label:{source_length}:{int(has_nonspace)}"
        assert remainder is not None
        if markdown_blank(remainder):
            return True, "destination"
        return markdown_reference_destination_state(remainder)
    if state == "destination":
        if columns == 0 or columns > 3:
            return False, None
        return markdown_reference_destination_state(content)
    if state == "may_title":
        if columns == 0 or columns > 3 or not content.startswith(("\"", "'", "(")):
            return False, None
        closer = ")" if content[0] == "(" else content[0]
        escaped = False
        for index, character in enumerate(content[1:], start=1):
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == closer:
                return markdown_blank(content[index + 1 :]), None
        return True, f"title:{closer}"
    closer = state.removeprefix("title:")
    if markdown_blank(line):
        return False, None
    escaped = False
    for index, character in enumerate(content):
        if escaped:
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == closer:
            return markdown_blank(content[index + 1 :]), None
    return True, state


def live_monitoring_sections(text: str) -> list[str]:
    sections: list[str] = []
    current: list[str] | None = None
    paragraph_open = False
    reference_state: str | None = None
    reference_start_index: int | None = None

    for line in visible_markdown_lines(text):
        if line == LIVE_MONITORING_HEADING:
            if current is not None:
                sections.append("\n".join(current))
            current = [line]
            paragraph_open = False
            reference_state = None
            reference_start_index = None
            continue
        if current is not None and reference_state is not None:
            consumed, next_state = markdown_reference_continuation_state(
                line,
                reference_state,
            )
            if consumed:
                current.append(line)
                reference_state = next_state
                if next_state is None:
                    reference_start_index = None
                paragraph_open = False
                continue
            invalid_reference = (
                reference_state == "destination"
                or reference_state.startswith("title:")
                or reference_state.startswith("label:")
            )
            if invalid_reference and reference_start_index is not None:
                sections.append("\n".join(current[:reference_start_index]))
                current = None
                paragraph_open = False
                reference_state = None
                reference_start_index = None
                continue
            reference_state = None
            reference_start_index = None
        if current is not None and not paragraph_open:
            is_definition, next_state = markdown_reference_definition_state(line)
            if is_definition:
                reference_start_index = len(current)
                current.append(line)
                reference_state = next_state
                if next_state is None:
                    reference_start_index = None
                paragraph_open = False
                continue
        setext_heading = re.match(r"^[ \t]{0,3}(?:=+|-+)[ \t]*$", line)
        if (
            setext_heading is not None
            and current is not None
            and paragraph_open
        ):
            sections.append("\n".join(current))
            current = None
            paragraph_open = False
            reference_state = None
            reference_start_index = None
            continue
        heading = re.match(
            r"^[ \t]{0,3}(#{1,6})(?:[ \t]+.*)?[ \t]*$",
            line,
        )
        if heading is not None:
            level = len(heading.group(1))
            if level <= 2 and current is not None:
                sections.append("\n".join(current))
                current = None
            paragraph_open = False
            reference_state = None
            reference_start_index = None
            if current is not None:
                current.append(line)
            continue
        if current is not None:
            current.append(line)
            paragraph_open = markdown_section_paragraph_open(
                line,
                was_open=paragraph_open,
            )

    if (
        current is not None
        and reference_state is not None
        and reference_state != "may_title"
        and reference_start_index is not None
    ):
        current = current[:reference_start_index]
    if current is not None:
        sections.append("\n".join(current))
    return sections


def validate_instruction_parity(
    failures: list[str],
    root: Path = ROOT,
) -> None:
    agents_path = root / "AGENTS.md"
    if not agents_path.is_file():
        failures.append("AGENTS.md: canonical shared instructions are required")
    else:
        try:
            agents_text = agents_path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            failures.append(
                f"AGENTS.md: could not read canonical instructions: {error}"
            )
        else:
            sections = live_monitoring_sections(agents_text)
            if not sections:
                failures.append(
                    "AGENTS.md: missing required live-monitoring instruction: "
                    f"{LIVE_MONITORING_HEADING}"
                )
            elif len(sections) != 1:
                failures.append(
                    "AGENTS.md: live-monitoring policy must contain exactly one "
                    "active section"
                )
            else:
                section = sections[0]
                for instruction in REQUIRED_LIVE_MONITORING_INSTRUCTIONS:
                    present = (
                        instruction in section
                        if instruction == LIVE_MONITORING_HEADING
                        else f"- {instruction}" in section.splitlines()
                    )
                    if not present:
                        failures.append(
                            "AGENTS.md: missing required live-monitoring "
                            f"instruction: {instruction}"
                        )

    claude_path = root / "CLAUDE.md"
    try:
        claude_import = claude_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        failures.append(f"CLAUDE.md: could not read instruction import: {error}")
        return
    if claude_import != CLAUDE_INSTRUCTION_IMPORT:
        failures.append(
            "CLAUDE.md: must contain exactly @AGENTS.md as the shared "
            "instruction import"
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
    for name in ("pycc", "pycc-feedback", "issue-to-plan", "issue-implement", "issue-select"):
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
            and isinstance(case.get("runner"), str)
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
    validate_instruction_parity(failures)
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
