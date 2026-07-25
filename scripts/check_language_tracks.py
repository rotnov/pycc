#!/usr/bin/env python3
"""Enforce registered regressions in the documented Python language tracks."""

from __future__ import annotations

import ast
import hashlib
import re
import sys
from collections.abc import Mapping
from html.parser import HTMLParser
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
POLICY_DOCUMENTS = (
    "README.md",
    "docs/SPEC.md",
    "docs/ROADMAP.md",
    "docs/PYTHON_STANDARDS.md",
    "docs/DECISIONS.md",
    "docs/TESTING.md",
    "docs/CLI_SPEC.md",
    "docs/STDLIB_PLAN.md",
    "docs/TYPE_SYSTEM.md",
    "docs/DELIVERY_PLAN.md",
)
SITE_PROJECTIONS = (
    "site/architecture/index.html",
    "site/python-aot-compilers/index.html",
)
DOCUMENTS = (*POLICY_DOCUMENTS, *SITE_PROJECTIONS)
CI_WORKFLOW = ".github/workflows/ci.yml"
SELF_TEST_PATH = "scripts/test_check_language_tracks.py"
CI_JOB_NAME = "language-track-policy"
CI_STEP_NAME = "Check Python language-track consistency"
REQUIRED_SELF_TESTS = (
    "test_repository_contract_is_consistent",
    "test_permanent_python_314_ceiling_is_rejected",
    "test_each_normative_document_must_keep_v1_0_scope",
    "test_supported_language_runs_must_remain_cumulative",
    "test_preview_and_upgrade_gate_are_scoped_after_v1_0",
    "test_additive_major_version_ceiling_is_rejected",
    "test_compatibility_language_is_not_a_false_positive",
    "test_visible_additive_contradictions_are_rejected",
    "test_hidden_or_nonnormative_text_cannot_satisfy_claim",
    "test_policy_digest_requires_explicit_registration",
    "test_later_track_keeps_independent_python_314_compatibility",
    "test_upgrade_contract_keeps_superseding_adr",
    "test_active_d012_row_is_required",
    "test_guard_and_self_tests_are_required_in_ci",
    "test_commented_ci_commands_do_not_count_as_execution",
    "test_language_track_ci_job_and_step_must_be_unconditional",
    "test_semantic_yaml_quoting_does_not_break_ci_guard",
    "test_ci_trigger_and_default_shell_cannot_bypass_guard",
    "test_language_track_ci_commands_run_directly_and_in_order",
    "test_required_self_test_inventory_is_enforced",
    "test_missing_document_fails_closed",
)
EXPECTED_SELF_TEST_SUITE_SHA256 = (
    "785b7d201e33ada25688e21a3371933334c2b41fca7348de30c78de7e329599b"
)

REQUIRED_CLAIMS: dict[str, tuple[str, ...]] = {
    "README.md": (
        "Python 3.14 is the v1.0 target, not a permanent language ceiling.",
        "The v1.0 track uses CPython 3.14.",
        "Each supported language level compiles and runs its cumulative fixture set",
    ),
    "docs/SPEC.md": (
        "pycc never adds its own dialect or syntax.",
        "The v1.0 language level is exactly CPython 3.14",
        "admitting a later standard Python language level requires its versioned "
        "roadmap gate and a superseding ADR",
    ),
    "docs/ROADMAP.md": (
        "D-012 fixes v1.0 to the Python 3.14 language level",
        "the cumulative Python 3.0–3.15 fixture set",
        "the independent Python 3.14 configuration keeps the complete Python "
        "3.0–3.14 suite green",
        "a new ADR supersedes D-012",
    ),
    "docs/PYTHON_STANDARDS.md": (
        "D-012 fixes v1.0 to exactly Python 3.14.",
        "Post-v1.0 preview",
        "the post-v1.0 roadmap gate",
        "The v1.0 Python 3.14 run therefore covers `py30/` through `py314/` "
        "against CPython 3.14.6.",
        "the Python 3.15 run covers `py30/` through `py315/` against its pinned "
        "current 3.15 patch",
        "the independent 3.14 compatibility run remains required",
        "a new ADR that supersedes D-012",
    ),
    "docs/DECISIONS.md": (
        "Language level: exactly CPython 3.14 in v1.0",
        "a later standard Python level requires a superseding ADR plus a "
        "machine-checked support/cumulative-fixture/oracle mapping",
        "pycc-specific syntax remains forbidden",
    ),
    "docs/TESTING.md": (
        "each supported language level compiles and runs its cumulative fixture set",
        "The v1.0 Python 3.14 run covers `py30/` through `py314/` against "
        "CPython 3.14.6.",
        "the Python 3.15 run covers `py30/` through `py315/` against a pinned "
        "current Python 3.15 patch",
        "the separate Python 3.14 compatibility run remains required",
    ),
    "docs/CLI_SPEC.md": (
        'python = "3.14" # v1.0 language level; later v1.x needs its gate + ADR',
    ),
    "docs/STDLIB_PLAN.md": (
        "The v1.0 surface remains CPython 3.14.",
        "The v1.x upgrade defined in ROADMAP.md adds these feature-frozen 3.15 deltas",
    ),
    "docs/TYPE_SYSTEM.md": (
        "The preview does not expand the v1.0 Python 3.14 contract.",
        "The v1.x language upgrade in ROADMAP.md adds four type-system obligations",
    ),
    "docs/DELIVERY_PLAN.md": ("Matches the v1.0 language line",),
    "site/architecture/index.html": (
        "The v1.0 language contract is Python 3.14.",
        "Python 3.15 adoption starts only in v1.x behind the roadmap gate and "
        "a superseding ADR.",
    ),
    "site/python-aot-compilers/index.html": (
        "Typed, standard Python 3.14 is the v1.0 design target; Python 3.15 is "
        "a gated v1.x adoption",
    ),
}

FORBIDDEN_CLAIMS = (
    (
        re.compile(
            r"Standard Python 3\.14 in, native binary out\..*"
            r"No dialect, no new syntax — ever\.",
            re.IGNORECASE,
        ),
        "permanent Python 3.14 ceiling",
    ),
    (
        re.compile(
            r"\bthe v1 track uses CPython 3\.14\b"
            r"(?![^.]{0,60}\b(?:compatibility|oracle)\b)",
            re.IGNORECASE,
        ),
        "major-version-wide Python 3.14 track",
    ),
    (
        re.compile(r"\bv1 accepts exactly Python 3\.14\b", re.IGNORECASE),
        "major-version-wide Python 3.14 acceptance gate",
    ),
    (
        re.compile(
            r"\bD-012 (?:still )?fixes v1 to (?:the )?Python 3\.14\b",
            re.IGNORECASE,
        ),
        "major-version-wide D-012 scope",
    ),
    (
        re.compile(r"\bexpand the v1 grammar or acceptance gate\b", re.IGNORECASE),
        "major-version-wide grammar gate",
    ),
    (
        re.compile(r"\bonly 3\.14 in v1\b", re.IGNORECASE),
        "major-version-wide configuration ceiling",
    ),
    (
        re.compile(r"\bthe v1 surface remains CPython 3\.14\b", re.IGNORECASE),
        "major-version-wide standard-library ceiling",
    ),
    (
        re.compile(r"\bv1 Python 3\.14 contract\b", re.IGNORECASE),
        "major-version-wide type-system ceiling",
    ),
    (
        re.compile(r"\bmatches the v1 language line\b", re.IGNORECASE),
        "major-version-wide delivery target",
    ),
    (
        re.compile(r"\bpost-v1(?: preview| roadmap gate)\b", re.IGNORECASE),
        "major-version-wide post-v1 gate",
    ),
    (
        re.compile(
            r"\bv1 (?:track )?(?:accepts|supports|uses|targets|requires) "
            r"(?:only |exactly )?(?:CPython |Python )?3\.14\b"
            r"(?![^.]{0,60}\b(?:compatibility|oracle)\b)",
            re.IGNORECASE,
        ),
        "additive major-version-wide Python 3.14 ceiling",
    ),
    (
        re.compile(
            r"\b(?:all |every |the |(?:the )?entire |(?:the )?whole )?"
            r"v1(?:\.x)?(?!\.\d|\d)"
            r"(?: releases?| family| track| line)?\b"
            r"(?:(?!\b(?:CPython |Python )?3\.\d+\b"
            r"|\b(?:compatibility|oracle|not)\b).){0,80}"
            r"\b(?:only|permanently|pinned|limited|restricted|fixed|ceiling)\b"
            r"(?:(?!\b(?:CPython |Python )?3\.\d+\b).){0,50}"
            r"\b(?:CPython |Python )?3\.14\b",
            re.IGNORECASE,
        ),
        "rephrased major-version-wide Python 3.14 ceiling",
    ),
    (
        re.compile(
            r"\b(?:CPython |Python )3\.14\b"
            r"(?:(?!\b(?:not|compatibility|oracle)\b).){0,80}"
            r"\b(?:the )?language ceiling\b.{0,60}"
            r"\b(?:throughout |across |for )?(?:the )?v1(?:\.x)?(?!\.\d|\d)",
            re.IGNORECASE,
        ),
        "reversed major-version-wide Python 3.14 ceiling",
    ),
    (
        re.compile(
            r"\bPython 3\.15 support\b(?:(?!\bnot\b).){0,60}"
            r"\b(?:deferred|postponed|blocked)\b.{0,40}\buntil v2(?:\.0)?\b",
            re.IGNORECASE,
        ),
        "Python 3.15 deferred beyond v1.x",
    ),
    (
        re.compile(
            r"\bv1\.0 (?:language level|track|target)\b.{0,50}"
            r"\b(?:is|uses|targets|accepts|supports)\b"
            r"(?:(?!\bnot\b).){0,30}"
            r"\b(?:CPython |Python )?3\.15\b",
            re.IGNORECASE,
        ),
        "v1.0 reassigned to Python 3.15",
    ),
    (
        re.compile(
            r"\b(?:Python )?3\.15 (?:configuration|run)\b"
            r"(?![^.]{0,70}\b(?:does not|doesn't|never)\b[^.]{0,20}\bonly\b)"
            r".{0,70}"
            r"\b(?:only|solely)\b.{0,40}"
            r"\b(?:the )?(?:py315/?|3\.15) fixture",
            re.IGNORECASE,
        ),
        "non-cumulative Python 3.15 run",
    ),
    (
        re.compile(
            r"\bindependent (?:Python )?3\.14 "
            r"(?:compatibility )?(?:configuration|run)\b"
            r"(?![^.]{0,60}\bnot\b[^.]{0,15}\b(?:optional|removed|discontinued)\b)"
            r".{0,60}\b(?:optional|removed|discontinued|not required)\b",
            re.IGNORECASE,
        ),
        "optional independent Python 3.14 run",
    ),
    (
        re.compile(
            r"\b(?:Python )?3\.15 gate\b.{0,50}"
            r"\b(?:(?:does not|doesn't) (?:need|require)|need not|requires? no)\b"
            r".{0,40}"
            r"\b(?:superseding )?ADR\b",
            re.IGNORECASE,
        ),
        "Python 3.15 gate without a superseding ADR",
    ),
    (
        re.compile(
            r"\bv1(?:\.x)?(?!\.\d|\d)\b.{0,40}"
            r"\b(?:will not|won't|never)\b.{0,30}"
            r"\bsupport(?:s|ed|ing)?\b.{0,30}\b(?:Python )?3\.15\b",
            re.IGNORECASE,
        ),
        "v1-wide rejection of Python 3.15",
    ),
    (
        re.compile(
            r"\b(?:Python )?3\.14 run\b.{0,60}\b(?:only|solely)\b.{0,30}"
            r"\bpy314/?\b",
            re.IGNORECASE,
        ),
        "non-cumulative Python 3.14 run",
    ),
    (
        re.compile(
            r"\b(?:Python )?3\.14 run\b.{0,100}"
            r"\bagainst (?:pinned )?CPython 3\.14\.(?!6\b)\d+\b",
            re.IGNORECASE,
        ),
        "conflicting Python 3.14 oracle",
    ),
    (
        re.compile(
            r"\bpycc-only\b.{0,30}\bsyntax\b.{0,40}"
            r"\b(?:allowed|permitted|supported)\b",
            re.IGNORECASE,
        ),
        "pycc-specific syntax permission",
    ),
    (
        re.compile(
            r"\b(?:Python )?3\.15 preview\b.{0,40}"
            r"\b(?:is |becomes )?(?:part of|included in)\b.{0,30}"
            r"\b(?:the )?v1\.0 (?:language )?(?:surface|gate|track)\b",
            re.IGNORECASE,
        ),
        "Python 3.15 preview inside v1.0",
    ),
    (
        re.compile(
            r"\bD-012\b(?:(?!\bnot\b).){0,40}\b(?:is |was )?"
            r"(?:superseded|rejected|withdrawn)\b",
            re.IGNORECASE,
        ),
        "inactive D-012 claim",
    ),
)

REQUIRED_CI_COMMANDS = (
    "/usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/python3 -I -B "
    "scripts/check_language_tracks.py",
    "/usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/python3 -I -B -m unittest "
    "discover -s scripts -p 'test_check_language_tracks.py'",
)
REQUIRED_CI_ENVIRONMENT = {
    "BASH_ENV": "",
    "DYLD_INSERT_LIBRARIES": "",
    "DYLD_LIBRARY_PATH": "",
    "ENV": "",
    "LD_LIBRARY_PATH": "",
    "LD_PRELOAD": "",
    "PYTHONHOME": "",
    "PYTHONPATH": "",
}
ALLOWED_WORKFLOW_ENVIRONMENT = {"CARGO_LLVM_COV_VERSION", "LLVM_VERSION"}
D012_ROW = re.compile(
    r"^\|\s*D-012\s*\|\s*"
    r"Language level: exactly CPython 3\.14 in v1\.0 "
    r"\(`python = \"3\.14\"` in pycc\.toml\)\. "
    r"No per-version grammar switches before v1\.x; "
    r"a later standard Python level requires a superseding ADR plus a "
    r"machine-checked support/cumulative-fixture/oracle mapping\. "
    r"pycc-specific syntax remains forbidden\s*\|\s*(proposed|accepted)\s*\|$",
    re.MULTILINE,
)
CLI_CONFIG_SECTION = re.compile(
    r"^## `pycc\.toml`\s*$.*?^```toml\s*$"
    r"(?P<configuration>.*?)^```\s*$",
    re.DOTALL | re.MULTILINE,
)
EXPECTED_POLICY_DIGESTS = {
    "README.md": "1cd60f9cd90d7d9a3c1e65e48e99859ef18b1cca8c6a218b9d9a542d322ee4e3",
    "docs/SPEC.md": "de4b9c86660c80a4f3801674c12ff70c9e7e59e7ead0d18fcb882266f7ec9fc2",
    "docs/ROADMAP.md": "b5b56093b2e7d6a849251a20e661ba028c9d3bd380a6b28363c2b7a763febf5f",
    "docs/PYTHON_STANDARDS.md": (
        "f73bcdae6679d3073da1753a94c408e5288674466add44f26222abd5e8905a56"
    ),
    "docs/DECISIONS.md": (
        "385570656a51e415af503b83e6e14a22b9f862b4c25548533a24d6c8d7b41203"
    ),
    "docs/TESTING.md": "2de7100da50ce8f8ad56d5180abca0aa835359607b4258626acb71d5183844d0",
    "docs/CLI_SPEC.md": (
        "c5820bc66bac803f35772cf65aaeaef89f9a3ddf01fe1f4c34cae4fa7d7911cd"
    ),
    "docs/STDLIB_PLAN.md": (
        "ae2c711df98b7f26c79a54fef6bf5dc59eaaa322e6711270fcfc321fb1945c90"
    ),
    "docs/TYPE_SYSTEM.md": (
        "6091e926e6472b528368b6b4d73def84c71e8f33ebedefe2b2b5459238ce91c8"
    ),
    "docs/DELIVERY_PLAN.md": (
        "d03b0169025722084f2877e7ead8b8b58df26f2f29b83b34bb1566501a5674fb"
    ),
    "site/architecture/index.html": (
        "0effa0545996b50b322b1049eb5b63585aa028bc9046c7973b3b15ef5d792577"
    ),
    "site/python-aot-compilers/index.html": (
        "fa0c8f89bef0c88e63533358659161af76a20cca287fe97b74a7157be075c36e"
    ),
}


def normalized(contents: str) -> str:
    """Make prose checks insensitive to Markdown line wrapping."""
    return " ".join(contents.split())


def without_fenced_blocks(contents: str) -> str:
    """Remove non-normative quotes/code and CommonMark fenced blocks."""
    lines = contents.splitlines(keepends=True)
    visible: list[str] = []
    fence_character: str | None = None
    fence_length = 0
    for line in lines:
        candidate = line.rstrip("\r\n")
        quoted = False
        while True:
            container = re.match(
                r"^ {0,3}(?:> ?|(?:[-+*]|\d+[.)]) {1,4})",
                candidate,
            )
            if container is None:
                break
            quoted = quoted or ">" in container.group(0)
            candidate = candidate[container.end() :]
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})", candidate)
        if fence_character is None:
            if quoted:
                visible.append("\n" if line.endswith("\n") else "")
                continue
            if re.match(r"^(?: {4,}|\t)", candidate):
                visible.append("\n" if line.endswith("\n") else "")
                continue
            if marker is None:
                visible.append(line)
                continue
            fence_character = marker.group(1)[0]
            fence_length = len(marker.group(1))
            visible.append("\n" if line.endswith("\n") else "")
            continue

        closing = re.fullmatch(
            rf" *{re.escape(fence_character)}{{{fence_length},}}[ \t]*",
            candidate,
        )
        visible.append("\n" if line.endswith("\n") else "")
        if closing is not None:
            fence_character = None
            fence_length = 0
    return "".join(visible)


def without_html_comments(contents: str) -> str:
    """Remove closed comments and fail closed from an unterminated opener."""
    return re.sub(r"<!--.*?(?:-->|\Z)", " ", contents, flags=re.DOTALL)


class VisibleHTMLParser(HTMLParser):
    """Collect rendered body text while ignoring common hidden containers."""

    HIDDEN_TAGS = {"script", "style", "template", "noscript"}
    VOID_TAGS = {
        "area",
        "base",
        "br",
        "col",
        "embed",
        "hr",
        "img",
        "input",
        "link",
        "meta",
        "param",
        "source",
        "track",
        "wbr",
    }

    def __init__(self) -> None:
        super().__init__()
        self.in_body = False
        self.hidden_depth = 0
        self.body_stack: list[tuple[str, bool]] = []
        self.visible_text: list[str] = []

    def handle_starttag(
        self,
        tag: str,
        attrs: list[tuple[str, str | None]],
    ) -> None:
        attributes = dict(attrs)
        if tag == "body":
            self.in_body = True
            return
        if not self.in_body:
            return
        inline_style = (attributes.get("style") or "").replace(" ", "").lower()
        hidden = (
            self.hidden_depth > 0
            or tag in self.HIDDEN_TAGS
            or "hidden" in attributes
            or (attributes.get("aria-hidden") or "").lower() == "true"
            or "display:none" in inline_style
            or "visibility:hidden" in inline_style
        )
        if tag not in self.VOID_TAGS:
            self.body_stack.append((tag, hidden))
            if hidden:
                self.hidden_depth += 1

    def handle_endtag(self, tag: str) -> None:
        if tag == "body":
            self.in_body = False
            return
        if not self.in_body or tag in self.VOID_TAGS or not self.body_stack:
            return
        _, hidden = self.body_stack.pop()
        if hidden:
            self.hidden_depth -= 1

    def handle_data(self, data: str) -> None:
        if self.in_body and self.hidden_depth == 0:
            self.visible_text.append(data)


def visible_html(contents: str) -> str:
    """Normalize text rendered in the HTML body, excluding hidden content."""
    parser = VisibleHTMLParser()
    parser.feed(without_html_comments(contents))
    return normalized(" ".join(parser.visible_text))


def visible_markdown(contents: str) -> str:
    """Return normative Markdown without comments, quotes, code, or struck text."""
    without_comments = without_html_comments(contents)
    without_fences = without_fenced_blocks(without_comments)
    without_strikethrough = re.sub(
        r"~~.*?~~",
        " ",
        without_fences,
        flags=re.DOTALL,
    )
    return without_strikethrough


def visible_normalized(contents: str) -> str:
    """Normalize rendered normative prose across Markdown line wrapping."""
    return normalized(visible_markdown(contents))


def registered_policy_blocks(path: str, contents: str) -> tuple[str, ...]:
    """Return all normalized normative content governed by the contract."""
    if path in SITE_PROJECTIONS:
        return (visible_html(contents),)
    blocks = [normalized(visible_markdown(contents))]
    if path == "docs/CLI_SPEC.md":
        section = CLI_CONFIG_SECTION.search(without_html_comments(contents))
        if section is not None:
            blocks.append(normalized(section.group("configuration")))
    return tuple(blocks)


def policy_digest(path: str, contents: str) -> str:
    """Hash the canonical visible policy blocks for explicit change registration."""
    payload = "\n\n".join(registered_policy_blocks(path, contents))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def indentation(line: str) -> int:
    """Return a YAML line's leading-space count."""
    return len(line) - len(line.lstrip())


def yaml_mapping_key(line: str, expected_indent: int) -> tuple[str, str] | None:
    """Return a simple YAML mapping key/value with quotes and spacing normalized."""
    if indentation(line) != expected_indent:
        return None
    text = line.strip()
    match = re.fullmatch(
        r'(?:"([A-Za-z0-9_-]+)"|\'([A-Za-z0-9_-]+)\'|([A-Za-z0-9_-]+))'
        r"\s*:\s*(.*)",
        text,
    )
    if match is None:
        return None
    key = next(group for group in match.groups()[:3] if group is not None)
    return key, match.group(4)


def yaml_sequence_mapping_key(
    line: str,
    expected_indent: int,
) -> tuple[str, str] | None:
    """Return the first mapping entry of a YAML sequence item."""
    if indentation(line) != expected_indent:
        return None
    text = line.strip()
    if not text.startswith("- "):
        return None
    return yaml_mapping_key(
        (" " * expected_indent) + text[2:],
        expected_indent,
    )


def yaml_scalar_text(value: str) -> str:
    """Normalize the simple quoted scalars used by workflow names."""
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        return value[1:-1]
    return value


def unique_mapping_entries(
    lines: list[str],
    expected_indent: int,
) -> dict[str, str] | None:
    """Parse one YAML mapping level and reject unknown syntax or duplicate keys."""
    entries: dict[str, str] = {}
    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if indentation(line) != expected_indent:
            continue
        entry = yaml_mapping_key(line, expected_indent)
        if entry is None:
            return None
        key, value = entry
        if key in entries:
            return None
        entries[key] = value
    return entries


def workflow_has_unfiltered_pull_request(workflow: str) -> bool:
    """Require the workflow to run for every pull request."""
    lines = workflow.splitlines()
    on_start = next(
        (
            index
            for index, line in enumerate(lines)
            if yaml_mapping_key(line, 0) == ("on", "")
        ),
        None,
    )
    if on_start is None:
        return False
    on_end = next(
        (
            index
            for index in range(on_start + 1, len(lines))
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and indentation(lines[index]) == 0
        ),
        len(lines),
    )
    triggers = unique_mapping_entries(lines[on_start + 1 : on_end], 2)
    if triggers is None or triggers.get("pull_request") not in {"", "{}"}:
        return False
    pull_request_start = next(
        (
            index
            for index in range(on_start + 1, on_end)
            if yaml_mapping_key(lines[index], 2)
            and yaml_mapping_key(lines[index], 2)[0] == "pull_request"
        ),
        None,
    )
    if pull_request_start is None:
        return False
    pull_request_end = next(
        (
            index
            for index in range(pull_request_start + 1, on_end)
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and indentation(lines[index]) <= 2
        ),
        on_end,
    )
    return not any(
        line.strip() and not line.lstrip().startswith("#")
        for line in lines[pull_request_start + 1 : pull_request_end]
    )


def workflow_has_root_run_defaults(workflow: str) -> bool:
    """Return whether workflow-level defaults can replace the guard's shell."""
    return any(
        entry is not None and entry[0] == "defaults"
        for line in workflow.splitlines()
        if (entry := yaml_mapping_key(line, 0)) is not None
    )


def workflow_has_unapproved_root_environment(workflow: str) -> bool:
    """Return whether inherited workflow env is outside the fail-closed allowlist."""
    lines = workflow.splitlines()
    env_start = next(
        (
            index
            for index, line in enumerate(lines)
            if yaml_mapping_key(line, 0) == ("env", "")
        ),
        None,
    )
    if env_start is None:
        return False
    env_end = next(
        (
            index
            for index in range(env_start + 1, len(lines))
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and indentation(lines[index]) == 0
        ),
        len(lines),
    )
    entries = unique_mapping_entries(lines[env_start + 1 : env_end], 2)
    return entries is None or not entries.keys() <= ALLOWED_WORKFLOW_ENVIRONMENT


def ci_step_commands(workflow: str) -> list[str]:
    """Return commands from the unconditional language-track workflow step."""
    lines = workflow.splitlines()
    job_start = next(
        (
            index
            for index, line in enumerate(lines)
            if yaml_mapping_key(line, 2) == (CI_JOB_NAME, "")
        ),
        None,
    )
    if job_start is None:
        return []

    job_end = next(
        (
            index
            for index in range(job_start + 1, len(lines))
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and indentation(lines[index]) <= 2
        ),
        len(lines),
    )
    job_contract = unique_mapping_entries(lines[job_start + 1 : job_end], 4)
    if job_contract is None:
        return []
    allowed_job_keys = {
        "container",
        "name",
        "permissions",
        "runs-on",
        "services",
        "steps",
        "strategy",
        "timeout-minutes",
    }
    if (
        not {"runs-on", "steps"} <= job_contract.keys()
        or not job_contract.keys() <= allowed_job_keys
        or not job_contract["runs-on"]
        or job_contract["steps"]
    ):
        return []

    step_start = next(
        (
            index
            for index in range(job_start + 1, job_end)
            if (
                (entry := yaml_sequence_mapping_key(lines[index], 6)) is not None
                and entry[0] == "name"
                and yaml_scalar_text(entry[1]) == CI_STEP_NAME
            )
        ),
        None,
    )
    if step_start is None:
        return []

    step_end = next(
        (
            index
            for index in range(step_start + 1, job_end)
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and indentation(lines[index]) <= 6
        ),
        job_end,
    )
    step_contract = unique_mapping_entries(lines[step_start + 1 : step_end], 8)
    if step_contract is None:
        return []
    allowed_step_keys = {"env", "id", "run", "timeout-minutes"}
    if set(step_contract) - allowed_step_keys or step_contract.get("run") != "|":
        return []
    if step_contract.get("env") != "":
        return []

    env_start = next(
        (
            index
            for index in range(step_start + 1, step_end)
            if yaml_mapping_key(lines[index], 8) == ("env", "")
        ),
        None,
    )
    if env_start is None:
        return []
    env_end = next(
        (
            index
            for index in range(env_start + 1, step_end)
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and indentation(lines[index]) <= 8
        ),
        step_end,
    )
    environment = unique_mapping_entries(lines[env_start + 1 : env_end], 10)
    if environment is None:
        return []
    normalized_environment = {
        key: yaml_scalar_text(value) for key, value in environment.items()
    }
    if normalized_environment != REQUIRED_CI_ENVIRONMENT:
        return []

    run_start: int | None = None
    for index in range(step_start + 1, step_end):
        line = lines[index]
        if yaml_mapping_key(line, 8) == ("run", "|"):
            run_start = index + 1
            break
    if run_start is None:
        return []

    commands: list[str] = []
    for line in lines[run_start:step_end]:
        if line.strip() and indentation(line) < 10:
            break
        command = line.strip()
        if command and not command.startswith("#"):
            commands.append(command)
    return commands


def validate_language_tracks(
    documents: Mapping[str, str],
    workflow: str,
) -> list[str]:
    failures: list[str] = []
    normalized_documents = {
        path: (
            visible_html(contents)
            if path in SITE_PROJECTIONS
            else visible_normalized(contents)
        )
        for path, contents in documents.items()
    }

    for path in DOCUMENTS:
        if path not in normalized_documents:
            failures.append(f"language-track document is unavailable: {path}")
            continue
        contents = normalized_documents[path]
        for claim in REQUIRED_CLAIMS[path]:
            if path == "docs/CLI_SPEC.md":
                section = CLI_CONFIG_SECTION.search(
                    without_html_comments(documents[path])
                )
                if section is not None:
                    configuration = section.group("configuration")
                    python_assignments = [
                        line.strip()
                        for line in configuration.splitlines()
                        if re.match(r"^\s*python\s*=", line)
                    ]
                    if len(python_assignments) == 1 and normalized(
                        python_assignments[0]
                    ) == normalized(claim):
                        continue
                failures.append(
                    f"{path} is missing required language-track claim: "
                    f"{normalized(claim)}"
                )
                continue
            if normalized(claim) not in contents:
                failures.append(
                    f"{path} is missing required language-track claim: "
                    f"{normalized(claim)}"
                )
        for pattern, label in FORBIDDEN_CLAIMS:
            if pattern.search(contents):
                failures.append(f"{path} contains forbidden {label}")
        expected_digest = EXPECTED_POLICY_DIGESTS.get(path)
        if (
            expected_digest is not None
            and policy_digest(path, documents[path]) != expected_digest
        ):
            failures.append(
                f"{path} has unregistered visible language-track policy changes"
            )

    decisions = documents.get("docs/DECISIONS.md")
    if (
        decisions is not None
        and len(D012_ROW.findall(visible_markdown(decisions))) != 1
    ):
        failures.append(
            "docs/DECISIONS.md must contain exactly one active proposed/accepted "
            "D-012 language-track row"
        )

    if not workflow_has_unfiltered_pull_request(workflow):
        failures.append(
            "CI language-track guard requires an unfiltered pull_request trigger"
        )
    if workflow_has_root_run_defaults(workflow):
        failures.append("CI language-track guard forbids workflow-level run defaults")
    if workflow_has_unapproved_root_environment(workflow):
        failures.append(
            "CI language-track guard forbids inherited execution environment"
        )

    workflow_commands = ci_step_commands(workflow)
    for command in REQUIRED_CI_COMMANDS:
        if command not in workflow_commands:
            failures.append(f"CI must run the language-track guard: {command}")
    if (
        all(command in workflow_commands for command in REQUIRED_CI_COMMANDS)
        and tuple(workflow_commands) != REQUIRED_CI_COMMANDS
    ):
        failures.append(
            "CI language-track step must contain exactly the repository guard "
            "and self-tests, in that order"
        )

    return failures


def self_test_suite_digest(source: str) -> str:
    """Hash the complete executable self-test source across Python versions."""
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


def validate_self_test_inventory(source: str) -> list[str]:
    """Require every registered mutation test to remain discoverable."""
    try:
        tree = ast.parse(source, filename=SELF_TEST_PATH)
    except SyntaxError as error:
        return [f"{SELF_TEST_PATH} is not valid Python: {error.msg}"]
    test_classes = [
        node
        for node in tree.body
        if isinstance(node, ast.ClassDef)
        and node.name == "LanguageTrackConsistencyTests"
        and any(
            isinstance(base, ast.Attribute)
            and isinstance(base.value, ast.Name)
            and base.value.id == "unittest"
            and base.attr == "TestCase"
            for base in node.bases
        )
    ]
    if len(test_classes) != 1:
        return [
            f"{SELF_TEST_PATH} must define exactly one discoverable "
            "LanguageTrackConsistencyTests(unittest.TestCase)"
        ]
    discovered = [
        node.name
        for node in test_classes[0].body
        if isinstance(node, ast.FunctionDef) and node.name.startswith("test_")
    ]
    failures = [
        f"{SELF_TEST_PATH} is missing required mutation test: {name}"
        for name in REQUIRED_SELF_TESTS
        if name not in discovered
    ]
    duplicates = sorted(
        name for name in REQUIRED_SELF_TESTS if discovered.count(name) != 1
    )
    if duplicates:
        failures.append(
            f"{SELF_TEST_PATH} must define each required mutation test exactly "
            f"once: {', '.join(duplicates)}"
        )
    unexpected = sorted(set(discovered) - set(REQUIRED_SELF_TESTS))
    if unexpected:
        failures.append(
            f"{SELF_TEST_PATH} has unregistered mutation tests: {', '.join(unexpected)}"
        )
    if self_test_suite_digest(source) != EXPECTED_SELF_TEST_SUITE_SHA256:
        failures.append(
            f"{SELF_TEST_PATH} has unregistered mutation-test behavior"
        )
    skip_controls = [
        node
        for node in test_classes[0].body
        if (
            isinstance(node, ast.Assign)
            and any(
                isinstance(target, ast.Name)
                and target.id in {"__unittest_skip__", "__unittest_skip_why__"}
                for target in node.targets
            )
        )
        or (
            isinstance(node, ast.AnnAssign)
            and isinstance(node.target, ast.Name)
            and node.target.id in {"__unittest_skip__", "__unittest_skip_why__"}
        )
        or (
            isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            and node.name.startswith("test_")
            and node.decorator_list
        )
    ]
    if test_classes[0].decorator_list or skip_controls:
        failures.append(
            f"{SELF_TEST_PATH} must not skip or decorate required mutation tests"
        )
    unsupported_methods = [
        node.name
        for node in test_classes[0].body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and node.name not in REQUIRED_SELF_TESTS
    ]
    unsupported_class_statements = [
        node
        for node in test_classes[0].body
        if not isinstance(node, ast.FunctionDef)
    ]
    load_tests_hooks = [
        node
        for node in tree.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and node.name == "load_tests"
    ]
    protected_names = {
        "LanguageTrackConsistencyTests",
        "__unittest_skip__",
        "__unittest_skip_why__",
        "load_tests",
        *REQUIRED_SELF_TESTS,
    }

    def rebound_names(node: ast.AST) -> set[str]:
        targets: list[ast.AST] = []
        if isinstance(node, ast.Assign):
            targets.extend(node.targets)
        elif isinstance(node, (ast.AnnAssign, ast.AugAssign)):
            targets.append(node.target)
        elif isinstance(node, ast.Delete):
            targets.extend(node.targets)
        names: set[str] = set()
        for target in targets:
            for child in ast.walk(target):
                if isinstance(child, ast.Name):
                    names.add(child.id)
                elif (
                    isinstance(child, ast.Attribute)
                    and isinstance(child.value, ast.Name)
                    and child.value.id == "LanguageTrackConsistencyTests"
                ):
                    names.add(child.attr)
        if (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Name)
            and node.func.id in {"setattr", "delattr"}
            and len(node.args) >= 2
            and isinstance(node.args[0], ast.Name)
            and node.args[0].id == "LanguageTrackConsistencyTests"
            and isinstance(node.args[1], ast.Constant)
            and isinstance(node.args[1].value, str)
        ):
            names.add(node.args[1].value)
        return names

    rebindings = [
        node for node in ast.walk(tree) if rebound_names(node) & protected_names
    ]
    if (
        unsupported_methods
        or unsupported_class_statements
        or load_tests_hooks
        or rebindings
    ):
        failures.append(
            f"{SELF_TEST_PATH} must not override unittest discovery or lifecycle"
        )
    runner = any(
        isinstance(node, ast.If)
        and isinstance(node.test, ast.Compare)
        and isinstance(node.test.left, ast.Name)
        and node.test.left.id == "__name__"
        and any(
            isinstance(child, ast.Expr)
            and isinstance(child.value, ast.Call)
            and isinstance(child.value.func, ast.Attribute)
            and isinstance(child.value.func.value, ast.Name)
            and child.value.func.value.id == "unittest"
            and child.value.func.attr == "main"
            for child in node.body
        )
        for node in tree.body
    )
    if not runner:
        failures.append(f"{SELF_TEST_PATH} must execute unittest.main() as a script")
    return failures


def repository_failures(root: Path = ROOT) -> list[str]:
    documents: dict[str, str] = {}
    for path in DOCUMENTS:
        source = root / path
        if source.is_file():
            documents[path] = source.read_text(encoding="utf-8")
    workflow_path = root / CI_WORKFLOW
    workflow = (
        workflow_path.read_text(encoding="utf-8") if workflow_path.is_file() else ""
    )
    failures = validate_language_tracks(documents, workflow)
    self_test_path = root / SELF_TEST_PATH
    if not self_test_path.is_file():
        failures.append(f"language-track self-test is unavailable: {SELF_TEST_PATH}")
    else:
        failures.extend(
            validate_self_test_inventory(self_test_path.read_text(encoding="utf-8"))
        )
    return failures


def main() -> int:
    failures = repository_failures()
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    print("language-track regression assertions: satisfied")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
