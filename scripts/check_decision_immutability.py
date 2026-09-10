#!/usr/bin/env python3
"""Reject edits to the original lines of an accepted decision (issue #1000,
Part 2 of #77; D-240).

`docs/decisions/` is the project's ADR log and `AGENTS.md` has always said
"do not silently rewrite an accepted decision; add a new decision that
supersedes it". Nothing enforced it: PR #74 appended a parenthetical to
D-032's accepted Consequences text, the edit reached `main` with every
required check green, and Part 1 of #77 (#999, PR #1001) had to restore the
original bytes by hand. This script is the guard that makes the rule a merge
invariant. It runs in `.github/workflows/ci.yml`'s `governance` job, so a
violation fails `ci-gate`.

What it compares
----------------

Only the event's own `base..head` range, two-dot (`git diff --name-status
--no-renames <base> <head> -- docs/decisions/`). History is never
re-judged: an accepted file that was edited in place before this guard
existed stays as it is, and a pull request is only answerable for the lines
*it* removes or changes. Under `--no-renames` a rename arrives as `D` + `A`,
so the `D` half is judged like any deletion.

Only paths matching `docs/decisions/D-<n>-<slug>.md` (anchored; no `/` in the
basename) are decision files. `README.md` and `TEMPLATE.md` are not
decisions, and neither is a file inside a directory that happens to be named
like one.

The rule (`check_file`)
-----------------------

A file is *frozen* when its frontmatter at the base revision says
`status: accepted` or `status: superseded`. A `proposed` file, a file that
is new at the head (`A`), and a base file whose frontmatter does not parse
(the decisions-index step rejects that on its own) are unconstrained. For a
frozen file, in order:

(a) deletion (or rename, see above) is a violation;
(b) the head frontmatter must parse (`FRONTMATTER_RE` already rejects a BOM,
    CRLF, an inserted frontmatter line, or trailing whitespace after the
    status) and its status must be `accepted` or `superseded`; `superseded`
    can never go back to `accepted`;
(c) a D-151 *index-only stub* -- the base's 0-based `splitlines()` index 8
    is exactly `Index-only: no long-form entry recorded yet.` **and** no
    base line starts with `- Status:` -- may replace its five stub body
    lines (indices 6-10: `# D-NNN`, blank, the marker, blank, the bare
    title) with the long-form entry, **but only when the head actually
    carries one**: some head line must be a well-formed body status line
    (`BODY_STATUS_LINE_RE`, i.e. `- Status: accepted` or
    `- Status: superseded`, optionally followed by whitespace and an
    annotation). Without such a line the exemption is off, the stub body
    stays frozen, and the strict walk in (d) reports the removed stub line
    with an `index-only stub replaced without a long-form entry` hint --
    so deleting the stub body outright, or replacing it with prose that has
    no `- Status:` line, is a violation rather than a free rewrite. The
    marker test is positional, not membership: the exemption unfreezes
    indices 6-10 by number, so a file carrying the marker anywhere else is
    not the shape the exemption models and falls through to the strict walk
    in (d). Both base conditions are required: every long-form entry has a
    `- Status:` line and that line can never be removed under this rule, so
    the marker cannot be smuggled into a long-form entry in one pull request
    and exploited in the next. The frontmatter and every base line after
    the stub (D-005's appended supersession paragraph) stay frozen;
(d) otherwise every base line must survive verbatim and in order: an exact
    greedy subsequence walk over `base.splitlines()` / `head.splitlines()`
    (no `keepends`, so a trailing-newline-only change is not a violation)
    requires each base line to reappear in the head at or after the previous
    match. Exactly two base lines may be *replaced* instead of matched, and
    each only by a line of its documented shape: the frontmatter `status:`
    line (index 3, fixed there by `FRONTMATTER_RE`) by `status: accepted` or
    `status: superseded` (which (b) already guarantees for the parsed head),
    and the *first* body line starting with `- Status:` by a line matching
    `BODY_STATUS_LINE_RE` -- `- Status: accepted` or `- Status: superseded`,
    optionally followed by whitespace and a supersession or narrowing
    annotation such as `- Status: superseded by D-241` or `- Status:
    accepted (one clause is narrowly superseded by D-241)`. These are the
    status transition and the narrowing-annotation shape (D-024, D-066,
    D-130, D-185). A `- Status:` line with any other value (`proposed`,
    `rejected`, blank) is not a replacement: the walk fails on the base
    status line and names the offending head line. Everything else in the
    head is an insertion, which is how an amendment is recorded.

`difflib.SequenceMatcher` opcodes are deliberately not the verdict: with
`autojunk=False` it misreports pure insertions as `delete` on repetitive
ADR-like line sequences (303 of 20,000 fuzzed cases, e.g. base `['  z',
'  z', '- x', '  z', '- y']` plus three inserted lines). Greedy
earliest-match is exact for the subsequence test; `difflib.unified_diff`
is used only to print context after a violation.

Revision plumbing
-----------------

`--base REV --head REV` names the range explicitly. Locally use the merge
base, not `origin/main`: `python3 -B scripts/check_decision_immutability.py
--base "$(git merge-base origin/main HEAD)" --head HEAD`. A two-dot diff
against a base that has since merged an inserted amendment to a frozen file
would report `main`'s own insertions as removed lines. Without flags the
script reads `GITHUB_EVENT_NAME` and the JSON at `GITHUB_EVENT_PATH`: on
`pull_request` the base is `pull_request.base.sha` and the head is
`GITHUB_SHA` (the checked-out `refs/pull/N/merge` commit, whose first
parent is `base.sha`, so two-dot is exact there); on `push` the base is
`before` and the head `GITHUB_SHA`, and an all-zero `before` (branch
creation) is a skip. Any other event name, no event at all, or an event
JSON without the field the event name promises (`pull_request.base.sha`,
`before`) is a usage error (exit 2). `pull_request.base.sha` can lag
`main`'s tip when `main` moves between the event and the run; lines `main`
inserted in between then show as insertions, never as deletions, so there
is no false failure, and branch protection's up-to-date requirement means
the final pre-merge run has `base.sha` equal to `main`'s tip.

The governance checkout is depth 1, so neither `base.sha` nor a pushed
`before` is normally present: each revision is checked with
`git cat-file -e <rev>^{commit}` and shallow-fetched
(`git fetch --no-tags --depth=1 origin <rev>`) when missing, the same shape
as `scripts/check_site_pin_merge_currency.rb`'s `ensure_revision_available`.

Blobs are read with `git show <rev>:<path>` and decoded as UTF-8 with
`errors="surrogateescape"`, so a stray non-UTF-8 byte becomes a line
mismatch rather than a traceback. A frozen path whose diff status is
anything other than `M` or `D` (`T` -- a file replaced by a symlink, whose
`git show` then returns the link target -- `C`, `R`, `U`, `X`) is a
violation: the guard fails closed on shapes it does not model. `--no-renames`
means git never emits `R`/`C` here; should one arrive anyway, its
three-token record (`status`, source, destination) is consumed whole -- so
the entries after it are not misread -- and the frozen file is judged under
its *source* path, which exists at the base, so the verdict is that
violation rather than a plumbing error on the destination path.

Exit codes: 0 passed (or skipped), 1 at least one violation, 2 usage or
plumbing error.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

from generate_decisions_index import parse_frontmatter

FROZEN_STATUSES = frozenset({"accepted", "superseded"})
STUB_MARKER = "Index-only: no long-form entry recorded yet."
STUB_BODY_INDICES = frozenset(range(6, 11))
STUB_MARKER_INDEX = 8
FRONTMATTER_STATUS_INDEX = 3
BODY_STATUS_PREFIX = "- Status:"
# The only values a replaced status line may carry. The body form may be
# followed by whitespace and an annotation (`superseded by D-241`,
# `(one clause is narrowly superseded by D-241)`); the frontmatter form is
# exactly the two bare lines (`FRONTMATTER_RE` allows no trailing text).
BODY_STATUS_LINE_RE = re.compile(r"- Status: (?:accepted|superseded)(?:\s.*)?")
FRONTMATTER_STATUS_LINES = frozenset(f"status: {s}" for s in FROZEN_STATUSES)
DECISION_PATH_RE = re.compile(r"docs/decisions/D-\d+-[^/]+\.md")
ZERO_SHA = "0" * 40
EXIT_PASS = 0
EXIT_VIOLATION = 1
EXIT_USAGE = 2


class PlumbingError(Exception):
    """A revision or event could not be resolved; the check cannot run."""


def is_decision_path(path):
    return DECISION_PATH_RE.fullmatch(path) is not None


def frozen_status(text):
    """Return the frontmatter status when it freezes the file, else None."""
    try:
        _id, _title, status = parse_frontmatter(text)
    except ValueError:
        return None
    return status if status in FROZEN_STATUSES else None


def is_index_only_stub(base_lines):
    """True when the base has the D-151 stub shape: the marker at index 8 and
    no `- Status:` line anywhere. Positional on purpose: the exemption
    unfreezes `STUB_BODY_INDICES` by number, so a marker elsewhere is not
    the modelled shape and the file stays under the strict walk."""
    return (
        len(base_lines) > STUB_MARKER_INDEX
        and base_lines[STUB_MARKER_INDEX] == STUB_MARKER
        and not any(line.startswith(BODY_STATUS_PREFIX) for line in base_lines)
    )


def first_body_status_index(lines):
    for index, line in enumerate(lines):
        if line.startswith(BODY_STATUS_PREFIX):
            return index
    return None


def is_permitted_body_status_line(line):
    return BODY_STATUS_LINE_RE.fullmatch(line) is not None


def has_long_form_entry(head_lines):
    """True when the head carries a well-formed body status line -- the
    minimum shape a long-form entry has, and what a stub replacement must
    produce for the stub exemption to apply."""
    return any(is_permitted_body_status_line(line) for line in head_lines)


def check_file(path, base_text, head_text):
    """Return the violations for one decision file, or [] when it passes.

    `head_text` is None when the file is absent at the head revision.
    """
    base_status = frozen_status(base_text)
    if base_status is None:
        return []
    if head_text is None:
        return [f"{path}: {base_status} decision deleted or renamed"]
    try:
        _id, _title, head_status = parse_frontmatter(head_text)
    except ValueError as exc:
        return [f"{path}: head frontmatter is missing or malformed ({exc})"]
    if head_status not in FROZEN_STATUSES:
        return [
            f"{path}: status {base_status} -> {head_status} is not a "
            "permitted transition (a frozen decision never becomes "
            f"{head_status})"
        ]
    if base_status == "superseded" and head_status == "accepted":
        return [f"{path}: status superseded -> accepted is not a permitted transition"]

    base_lines = base_text.splitlines()
    head_lines = head_text.splitlines()
    required = range(len(base_lines))
    stub_base = is_index_only_stub(base_lines)
    if stub_base and has_long_form_entry(head_lines):
        required = [i for i in required if i not in STUB_BODY_INDICES]
    status_index = first_body_status_index(base_lines)

    def matches(index, base_line, head_line):
        if head_line == base_line:
            return True
        if index == FRONTMATTER_STATUS_INDEX:
            return head_line in FRONTMATTER_STATUS_LINES
        if index == status_index:
            return is_permitted_body_status_line(head_line)
        return False

    cursor = 0
    for index in required:
        base_line = base_lines[index]
        position = cursor
        while position < len(head_lines) and not matches(
            index, base_line, head_lines[position]
        ):
            position += 1
        if position == len(head_lines):
            message = f"{path}: base line {index + 1} removed or changed: {base_line}"
            if index == status_index:
                offered = next(
                    (
                        line
                        for line in head_lines[cursor:]
                        if line.startswith(BODY_STATUS_PREFIX)
                    ),
                    None,
                )
                if offered is not None:
                    message += (
                        f" (head offers {offered!r}, but a replaced body status "
                        "line must start with '- Status: accepted' or "
                        "'- Status: superseded')"
                    )
            elif stub_base and index in STUB_BODY_INDICES:
                message += (
                    " (index-only stub replaced without a long-form entry: no "
                    "'- Status: accepted' or '- Status: superseded' line at head)"
                )
            return [message]
        cursor = position + 1
    return []


def context_diff(path, base_text, head_text):
    """Unified diff printed after a violation; a deleted file gets none."""
    return "".join(
        difflib.unified_diff(
            base_text.splitlines(keepends=True),
            head_text.splitlines(keepends=True),
            fromfile=f"base/{path}",
            tofile=f"head/{path}",
            n=1,
        )
    )


def run_git(root, *args):
    return subprocess.run(
        ["git", *args], cwd=str(root), capture_output=True, check=False
    )


def revision_present(root, revision):
    return run_git(root, "cat-file", "-e", f"{revision}^{{commit}}").returncode == 0


def ensure_revision_available(root, revision):
    if revision_present(root, revision):
        return
    fetched = run_git(root, "fetch", "--no-tags", "--depth=1", "origin", revision)
    if fetched.returncode == 0 or revision_present(root, revision):
        return
    raise PlumbingError(
        f"could not resolve revision {revision!r} locally or via "
        f"'git fetch --no-tags --depth=1 origin {revision}': "
        f"{fetched.stderr.decode('utf-8', 'replace').strip()}"
    )


def changed_decision_files(root, base, head):
    """Yield (status, path) for every decision file the range touches."""
    completed = run_git(
        root,
        "diff",
        "--name-status",
        "--no-renames",
        "-z",
        base,
        head,
        "--",
        "docs/decisions/",
    )
    if completed.returncode != 0:
        raise PlumbingError(
            f"git diff {base}..{head} failed: "
            f"{completed.stderr.decode('utf-8', 'replace').strip()}"
        )
    tokens = completed.stdout.decode("utf-8", "surrogateescape").split("\0")
    entries = []
    position = 0
    while position < len(tokens) and tokens[position]:
        status = tokens[position]
        # `--no-renames` means R/C never appear; if one does, consume its
        # three-token record whole so later entries stay aligned, and judge
        # the source path (present at the base) so `check_range`'s
        # unexpected-status branch reports the violation.
        path = tokens[position + 1]
        position += 3 if status[0] in "RC" else 2
        if is_decision_path(path):
            entries.append((status[0], path))
    return entries


def read_blob(root, revision, path):
    completed = run_git(root, "show", f"{revision}:{path}")
    if completed.returncode != 0:
        raise PlumbingError(
            f"git show {revision}:{path} failed: "
            f"{completed.stderr.decode('utf-8', 'replace').strip()}"
        )
    return completed.stdout.decode("utf-8", "surrogateescape")


def check_range(root, base, head):
    """Return (violations, compared_count, context) for base..head."""
    violations = []
    contexts = []
    compared = 0
    for status, path in changed_decision_files(root, base, head):
        if status == "A":
            continue
        compared += 1
        base_text = read_blob(root, base, path)
        if status == "D":
            found = check_file(path, base_text, None)
            head_text = None
        elif status == "M":
            head_text = read_blob(root, head, path)
            found = check_file(path, base_text, head_text)
        else:
            head_text = None
            found = (
                [
                    f"{path}: diff status {status!r} (type change, copy, rename, "
                    "or unmerged) is not permitted for a frozen decision"
                ]
                if frozen_status(base_text) is not None
                else []
            )
        if found:
            violations.extend(found)
            if head_text is not None:
                contexts.append(context_diff(path, base_text, head_text))
    return violations, compared, "".join(contexts)


def resolve_revisions(args, environ):
    """Return (base, head), or None when the event is a documented skip."""
    if (args.base is None) != (args.head is None):
        raise PlumbingError("--base and --head must be given together")
    if args.base is not None:
        return args.base, args.head
    event_name = environ.get("GITHUB_EVENT_NAME")
    if not event_name:
        raise PlumbingError(
            "pass --base REV --head REV, or run under a GitHub Actions "
            "pull_request or push event"
        )
    event_path = environ.get("GITHUB_EVENT_PATH")
    head = environ.get("GITHUB_SHA")
    if not event_path or not head:
        raise PlumbingError(
            f"GITHUB_EVENT_NAME={event_name!r} needs GITHUB_EVENT_PATH and GITHUB_SHA"
        )
    with open(event_path, encoding="utf-8") as handle:
        event = json.load(handle)
    if event_name == "pull_request":
        try:
            base = event["pull_request"]["base"]["sha"]
        except (KeyError, TypeError):
            raise PlumbingError(
                f"pull_request event JSON at {event_path} has no "
                "pull_request.base.sha"
            ) from None
        if not isinstance(base, str) or not base:
            raise PlumbingError(
                f"pull_request event JSON at {event_path} has a non-string "
                f"or empty pull_request.base.sha: {base!r}"
            )
        return base, head
    if event_name == "push":
        base = event.get("before")
        if base == ZERO_SHA:
            return None
        if not isinstance(base, str) or not base:
            raise PlumbingError(
                f"push event JSON at {event_path} has a non-string or empty "
                f"before: {base!r}"
            )
        return base, head
    raise PlumbingError(
        f"unsupported GITHUB_EVENT_NAME {event_name!r}: only pull_request "
        "and push are modelled"
    )


def build_parser():
    parser = argparse.ArgumentParser(
        description="Reject edits to the original lines of an accepted decision."
    )
    parser.add_argument("--base", help="base revision (with --head)")
    parser.add_argument("--head", help="head revision (with --base)")
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository root (default: this script's repository)",
    )
    return parser


def main(argv=None, environ=None):
    environ = os.environ if environ is None else environ
    args = build_parser().parse_args(argv)
    try:
        resolved = resolve_revisions(args, environ)
        if resolved is None:
            print("push event with no base revision (branch creation); skipping.")
            return EXIT_PASS
        base, head = resolved
        ensure_revision_available(args.root, base)
        ensure_revision_available(args.root, head)
        violations, compared, context = check_range(args.root, base, head)
    except PlumbingError as exc:
        print(f"check_decision_immutability: {exc}", file=sys.stderr)
        return EXIT_USAGE
    if violations:
        for violation in violations:
            print(violation, file=sys.stderr)
        if context:
            print(context, file=sys.stderr, end="")
        print(
            f"Accepted-decision immutability check failed: {len(violations)} "
            f"violation(s) in {compared} decision file(s) compared. An "
            "accepted or superseded decision's original lines are immutable; "
            "record an amendment as inserted dated lines or a superseding "
            "decision (D-240, docs/decisions/TEMPLATE.md).",
            file=sys.stderr,
        )
        return EXIT_VIOLATION
    print(
        f"Accepted-decision immutability check passed ({compared} decision "
        "files compared)."
    )
    return EXIT_PASS


if __name__ == "__main__":
    sys.exit(main())
