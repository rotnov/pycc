#!/usr/bin/env python3
"""Check that every evidence-backed conformance row declares the breadth it proves.

`docs/PYTHON_STANDARDS.md`'s matrix used to have three statuses -- planned, in
progress, and passing -- with no way to say "passing, but only this slice of the
PEP". A passing row therefore read as whole-PEP acceptance no matter how narrow
the fixture behind it was, and `docs/ROADMAP.md`'s milestone gates counted those
rows directly. Issue #248 records the consequence: several rows claimed
substantially more than their one fixture exercised, while pycc still rejects
other core constructs from those same PEPs.

Part 1 (issue #593) introduced this manifest and checker without changing any
row's status. Part 2 (issue #594, D-177) adds the missing status and the rule
that makes the distinction mechanical:

* `◐` -- **subset**. The row's fixtures pass and its breadth is declared, but
  the PEP has core surface those fixtures do not reach. Evidence-backed; *not*
  whole-PEP acceptance.
* `✅` -- **accepted**. The fixtures pass and every gap the manifest records is
  a deliberate, permanent non-goal rather than an unimplemented category.

Both are evidence-backed, so both need a manifest entry. Each `not_proven` item
carries a `kind`: `core` (part of the PEP's core surface, unimplemented) or
`out-of-scope` (deliberately excluded, with a reason citing an accepted decision
or a language-level fact). The rule this script enforces, in both directions, is
exactly: **any `core` gap forces `◐`; `✅` requires zero of them.** A narrow
smoke fixture therefore cannot be promoted to whole-PEP acceptance, which is the
defect #248 names.

Every entry also carries the row's `matrix_line`, checked against the parsed
line number, so an entry cannot silently drift onto a different row as the
matrix grows.

The matrix is parsed with exactly the rules `tests/conformance_matrix_guard.rs`
uses -- five-cell rows whose status cell is one of the documented markers, and
backtick spans ending in `.py` as the cited fixtures -- so the two checks cannot
disagree about which rows are evidence-backed or which fixtures a row cites.

Issue #623's third completion criterion adds one more binding: `docs/ROADMAP.md`
states the same totals in prose, and that prose used to drift silently. The
checker now parses the bold `**Conformance progress (...)**` headline and
requires every number in it -- the evidence-backed total, the required-row
target, the derived gap, and the whole-PEP count -- to agree with what the
matrix says, plus the checker's own summary line to appear quoted verbatim in
the paragraph. The parse is fail-closed: a missing, duplicated, or unparseable
headline is a failure, so rewording the paragraph cannot quietly disable the
guard.

Issue #732 adds a second figure alongside the row count: the number of
*distinct PEP numbers* the evidence-backed rows encompass, counted per
D-153's "Two ways to count" convention -- a range cell (`634–636`) counts
every number in the range, a two-link cell (`649/749`) counts each linked
number, an unnumbered cell (`-`/`—`) counts zero, and a PEP number repeated
across two rows (PEP 695's two rows) counts once across both. `docs/ROADMAP.md`'s
headline states this figure too, and the checker cross-checks it the same
fail-closed way as the row counts.

Issue #732 also binds the headline's `required`/`pep_required` totals to the
milestone's own `**Accept:** conformance ≥ N ... matrix rows ... encompassing
M distinct PEP numbers` bullet, so lowering the headline's stated targets
without also changing the Accept clause -- or vice versa -- is a failure
rather than a silent pass. That bullet lookup is fail-closed the same way:
missing or ambiguous (more than one bullet matching that exact phrasing) is a
failure, not a guess.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any


MANIFEST_VERSION = 2
SUBSET = "◐"
ACCEPTED = "✅"
STATUS_MARKERS = ("☐", "⚙", SUBSET, ACCEPTED)
#: Statuses whose rows claim their fixtures pass, and so must declare breadth.
EVIDENCE_STATUSES = (SUBSET, ACCEPTED)
CORE = "core"
OUT_OF_SCOPE = "out-of-scope"
GAP_KINDS = (CORE, OUT_OF_SCOPE)


class BreadthError(Exception):
    """A disagreement between the matrix, the manifest, and the harness."""


class MatrixRow:
    """One parsed matrix row: its line number, PEP cell, fixtures, and status."""

    def __init__(
        self, line_number: int, pep: str, feature: str, fixtures: list[str], status: str
    ) -> None:
        self.line_number = line_number
        self.pep = pep
        self.feature = feature
        self.fixtures = fixtures
        self.status = status

    @property
    def key(self) -> tuple[str, tuple[str, ...]]:
        """The row's identity: its PEP cell plus the fixtures it cites.

        The PEP cell alone is not unique -- PEP 695 has two rows, one for the
        `type` statement and generic functions and one for generic classes --
        and two rows carry `-` in the PEP cell because they describe language
        guarantees with no PEP of their own. The cited-fixture list separates
        every such pair, and a rename that breaks the key is a rename that
        should have updated the manifest anyway.
        """
        return (self.pep, tuple(self.fixtures))


def cited_fixtures(test_cell: str) -> list[str]:
    """Every ``` `...py` ``` span in a Test cell, in order.

    A cell may cite more than one fixture (PEP 594 cites two, PEP 585 cites
    four) and may carry prose and links around them, so the backtick spans are
    the signal rather than whitespace splitting.
    """
    found: list[str] = []
    rest = test_cell
    while True:
        opening = rest.find("`")
        if opening < 0:
            break
        after_open = rest[opening + 1 :]
        closing = after_open.find("`")
        if closing < 0:
            break
        span = after_open[:closing]
        if span.endswith(".py"):
            found.append(span)
        rest = after_open[closing + 1 :]
    return found


#: A markdown link to a PEP's canonical page, e.g. `[634](https://peps.python.org/pep-0634/)`.
PEP_LINK = re.compile(r"\[(\d+)\]\(https://peps\.python\.org/pep-\d+/\)")


def pep_numbers(cell: str) -> set[int]:
    """The distinct PEP numbers a matrix row's PEP cell names, per D-153.

    * An unnumbered cell (`-` or `—`) names none -- it describes a language
      guarantee with no PEP of its own. `PEP_LINK` finds no match in such a
      cell, so this falls out of the general case below with no special
      casing needed.
    * A cell citing two PEP links (e.g. `649/749`) names both; `PEP_LINK`
      already finds every link in the cell, so no special casing is needed.
    * A cell whose last link is immediately followed by a bare range suffix
      (e.g. `634–636`, where only `634` is linked) names every integer from
      the smallest linked number through the suffix, inclusive.
    """
    cell = cell.strip()
    numbers = {int(match) for match in PEP_LINK.findall(cell)}
    if not numbers:
        return numbers
    range_suffix = re.search(r"\)[–-](\d+)\s*$", cell)
    if range_suffix:
        numbers |= set(range(min(numbers), int(range_suffix.group(1)) + 1))
    return numbers


def distinct_pep_count(rows: list[MatrixRow]) -> int:
    """The number of distinct PEP numbers the given rows encompass.

    A PEP number that appears on two separate rows -- PEP 695 has one row for
    the `type` statement and generic functions and another for generic
    classes -- is counted once across both, per D-153's convention.
    """
    peps: set[int] = set()
    for row in rows:
        peps |= pep_numbers(row.pep)
    return len(peps)


def parse_matrix(markdown: str) -> list[MatrixRow]:
    """Parses the `| PEP | Feature | Cat | Test | St |` rows.

    Header and separator rows are dropped by requiring a status cell that is one
    of the documented markers, which also skips every unrelated five-column
    table elsewhere in the file.
    """
    rows: list[MatrixRow] = []
    for index, line in enumerate(markdown.splitlines()):
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 5:
            continue
        if cells[4] not in STATUS_MARKERS:
            continue
        rows.append(
            MatrixRow(index + 1, cells[0], cells[1], cited_fixtures(cells[3]), cells[4])
        )
    return rows


def evidence_rows(markdown: str) -> list[MatrixRow]:
    """Rows claiming their fixtures pass -- `◐` and `✅` alike."""
    return [row for row in parse_matrix(markdown) if row.status in EVIDENCE_STATUSES]


def read_harness(root_file: Path) -> str:
    """The conformance harness sources: `root_file` plus its cohort files.

    The harness is `tests/conformance.rs` followed by every direct `*.rs` file
    under the sibling `tests/conformance/` directory in sorted file-name order
    (non-recursive; a missing directory yields the root alone). Deliberately
    identical to `tests/harness_support/conformance_sources.rs`, the rule the
    two Rust guards share: a test moved into a cohort file must stay visible to
    every text-reader of the harness, or it silently leaves that reader's audit.
    """
    parts = [root_file.read_text(encoding="utf-8")]
    module_dir = root_file.with_suffix("")
    if module_dir.is_dir():
        for module in sorted(module_dir.glob("*.rs")):
            if module.is_file():
                parts.append(module.read_text(encoding="utf-8"))
    return "\n".join(parts)


def is_registered(harness: str, fixture: str) -> bool:
    """Whether the conformance harness joins this fixture as a path.

    Deliberately identical to `conformance_matrix_guard.rs`'s own rule: a
    fixture named only in a doc comment or an assertion message is not a
    registration, because only the joined path is what a test actually runs.
    """
    return f"tests/fixtures/{fixture}" in harness


def _require(condition: bool, message: str, failures: list[str]) -> bool:
    if not condition:
        failures.append(message)
    return condition


def _check_category_list(
    entries: Any, label: str, where: str, required_fields: tuple[str, ...],
    failures: list[str],
) -> None:
    if not isinstance(entries, list):
        failures.append(f"{where}: `{label}` must be a list")
        return
    for position, entry in enumerate(entries):
        origin = f"{where}: `{label}`[{position}]"
        if not isinstance(entry, dict):
            failures.append(f"{origin} must be an object")
            continue
        for field in required_fields:
            value = entry.get(field)
            if not isinstance(value, str) or not value.strip():
                failures.append(f"{origin} needs a non-empty `{field}`")


def _check_acceptance(
    where: str, pep: str, row: MatrixRow, core_gaps: list[str], failures: list[str],
) -> None:
    """Enforce D-177's rule: any `core` gap forces `◐`; `✅` requires zero.

    Checked in both directions, because each failure is a different defect. A
    `✅` row with core gaps is the #248 overclaim itself. A `◐` row with none is
    a row being held below the acceptance it has earned -- harmless to a reader,
    but it means the manifest and the matrix disagree about the same fact, and
    letting that drift is how the first direction stops being trustworthy.
    """
    if row.status == ACCEPTED and core_gaps:
        failures.append(
            f"{where}: PEP {pep} is ✅ (whole-PEP acceptance) but records "
            f"{len(core_gaps)} unimplemented core category/categories "
            f"({', '.join(repr(gap) for gap in core_gaps)}) — mark the row ◐, or "
            "reclassify the gap as `out-of-scope` with a reason if it is a "
            "deliberate permanent non-goal"
        )
    elif row.status == SUBSET and not core_gaps:
        failures.append(
            f"{where}: PEP {pep} is ◐ (subset) but records no unimplemented core "
            "category — either name the core gap that keeps it a subset, or "
            "promote the row to ✅"
        )


def validate(markdown: str, manifest: Any, harness: str) -> None:
    """Raises `BreadthError` describing every disagreement found, or returns."""
    failures: list[str] = []

    if not isinstance(manifest, dict):
        raise BreadthError("the breadth manifest must be a JSON object")
    if manifest.get("manifest_version") != MANIFEST_VERSION:
        failures.append(
            f"`manifest_version` must be {MANIFEST_VERSION}, "
            f"found {manifest.get('manifest_version')!r}"
        )
    rows = manifest.get("rows")
    if not isinstance(rows, list):
        raise BreadthError("the breadth manifest needs a `rows` list")

    matrix = evidence_rows(markdown)
    if not matrix:
        raise BreadthError(
            "no ◐ or ✅ rows parsed out of the conformance matrix — "
            "the check would pass vacuously"
        )
    matrix_by_key = {row.key: row for row in matrix}

    seen: dict[tuple[str, tuple[str, ...]], int] = {}
    for position, entry in enumerate(rows):
        where = f"rows[{position}]"
        if not isinstance(entry, dict):
            failures.append(f"{where} must be an object")
            continue
        pep = entry.get("pep")
        fixtures = entry.get("fixtures")
        if not _require(
            isinstance(pep, str) and pep.strip(), f"{where} needs a non-empty `pep`",
            failures,
        ):
            continue
        if not _require(
            isinstance(fixtures, list)
            and fixtures
            and all(isinstance(name, str) and name.strip() for name in fixtures),
            f"{where} needs a non-empty `fixtures` list of strings",
            failures,
        ):
            continue

        feature = entry.get("feature")
        if not isinstance(feature, str) or not feature.strip():
            failures.append(f"{where} needs a non-empty `feature`")

        key = (pep, tuple(fixtures))
        if key in seen:
            failures.append(
                f"{where} repeats the row already declared at rows[{seen[key]}]"
            )
            continue
        seen[key] = position

        if key not in matrix_by_key:
            failures.append(
                f"{where} declares PEP {pep} citing {list(fixtures)}, "
                "which matches no ◐ or ✅ row in the matrix"
            )
            continue

        matrix_line = entry.get("matrix_line")
        row = matrix_by_key[key]
        if matrix_line != row.line_number:
            failures.append(
                f"{where}: `matrix_line` is {matrix_line!r} but PEP {pep} is "
                f"on line {row.line_number} of the matrix"
            )

        proven = entry.get("proven")
        _check_category_list(proven, "proven", where, ("category", "evidence"), failures)
        if isinstance(proven, list):
            if not proven:
                failures.append(
                    f"{where}: PEP {pep} is {row.status} but proves no semantic "
                    "category — an evidence-backed row with no proven breadth is "
                    "exactly the overclaim this manifest exists to surface"
                )
            for item in proven:
                if not isinstance(item, dict):
                    continue
                evidence = item.get("evidence")
                if isinstance(evidence, str) and evidence not in fixtures:
                    failures.append(
                        f"{where}: proven category "
                        f"{item.get('category', '?')!r} cites `{evidence}`, "
                        "which is not one of this row's fixtures"
                    )
        not_proven = entry.get("not_proven")
        _check_category_list(
            not_proven, "not_proven", where, ("category", "kind", "reason"),
            failures,
        )
        if isinstance(not_proven, list):
            core_gaps = []
            for item in not_proven:
                if not isinstance(item, dict):
                    continue
                kind = item.get("kind")
                if kind not in GAP_KINDS:
                    failures.append(
                        f"{where}: not_proven category "
                        f"{item.get('category', '?')!r} has `kind` {kind!r}, "
                        f"which is not one of {list(GAP_KINDS)}"
                    )
                elif kind == CORE:
                    core_gaps.append(item.get("category", "?"))
            _check_acceptance(where, pep, row, core_gaps, failures)

        for fixture in fixtures:
            if not is_registered(harness, fixture):
                failures.append(
                    f"{where}: `{fixture}` is not registered in the conformance harness, "
                    "so nothing runs it"
                )

    for key, row in matrix_by_key.items():
        if key not in seen:
            failures.append(
                f"line {row.line_number}: PEP {row.pep} is {row.status} but has no "
                "breadth manifest entry declaring what its evidence proves"
            )

    if failures:
        raise BreadthError("\n".join(failures))


#: The bold headline `docs/ROADMAP.md` uses to state the conformance totals. It
#: is matched non-greedily and must occur exactly once: zero matches means the
#: paragraph was renamed away, and two mean the file states the totals twice.
ROADMAP_HEADLINE = re.compile(r"\*\*Conformance progress \([^)]*\):(?P<body>.*?)\*\*", re.DOTALL)

#: The numbers inside that headline. Anchoring here rather than on the
#: surrounding paragraph keeps the historical figures it narrates -- earlier
#: totals and earlier gaps -- from being mistaken for the current claim.
ROADMAP_FIGURES = re.compile(
    r"(?P<total>\d+) of the required (?P<required>\d+) matrix rows are at `"
    + SUBSET
    + r"` or better, leaving a (?P<gap>\d+)-row gap; "
    r"(?P<accepted>\d+) of those (?P<restated_total>\d+) are `"
    + ACCEPTED
    + r"`"
)

#: The distinct-PEP-count clause of the same headline (issue #732), matched
#: independently of `ROADMAP_FIGURES` since it can be separated from the row
#: figures by intervening prose (e.g. the "not gated before v1.0" clause).
ROADMAP_PEP_FIGURES = re.compile(
    r"encompass (?P<pep_total>\d+) of the required (?P<pep_required>\d+) "
    r"distinct PEP numbers, leaving a (?P<pep_gap>\d+)-PEP gap"
)

#: The milestone's own `**Accept:**` bullet -- the normative source for both
#: `required` figures the progress headline restates. Binding the headline's
#: `required`/`pep_required` to this clause (rather than only checking their
#: own internal gap arithmetic) closes the gap a reviewer flagged on #732:
#: without it, lowering the headline's stated targets (e.g. 39 -> 38) passes
#: `check_roadmap_counts` cleanly while silently relaxing the tracked
#: milestone target. The inter-group gap is deliberately scoped to a single
#: line (`[^\n]*?`, no `re.DOTALL`) rather than spanning the whole document:
#: an unbounded span risks silently crossing into the checker-summary
#: sentence two lines below, which also contains an "encompassing N distinct
#: PEP numbers" phrase but names the *achieved* figure, not the target --
#: binding to that instead would defeat this guard's fail-closed intent.
ACCEPT_CLAUSE_FIGURES = re.compile(
    r"\*\*Accept:\*\* conformance ≥ (?P<required>\d+) `PYTHON_STANDARDS\.md` "
    r"matrix rows [^\n]*? encompassing (?P<pep_required>\d+) distinct PEP numbers"
)


def summary_body(rows: list[MatrixRow]) -> str:
    """Render the totals sentence shared by the printed summary and the roadmap.

    `docs/ROADMAP.md` quotes this exact string, minus the `conformance breadth: `
    prefix `main` prints in front of it, so there is one source for both.
    """

    accepted = sum(1 for row in rows if row.status == ACCEPTED)
    return (
        f"{len(rows)} evidence-backed rows, all declared "
        f"({accepted} accepted as whole-PEP, {len(rows) - accepted} subset), "
        f"encompassing {distinct_pep_count(rows)} distinct PEP numbers"
    )


def resolve_roadmap_text(path: Path) -> str:
    """Resolve roadmap content from either a single file or a directory tree.

    Mirrors `scripts/check_roadmap_evidence.rb`'s dual-layout support: if
    `path` (the `--roadmap` default, `docs/ROADMAP.md`) exists, read it
    directly -- today's single-file behavior, unchanged. Otherwise, if a
    sibling `roadmap/` directory exists next to `path` and holds at least one
    Markdown file, read and concatenate every `**/*.md` file under it, sorted
    by path. Unlike the Ruby checker's per-file heading-path parse, this is
    safe as plain concatenation: `check_roadmap_counts` only searches the
    result with whole-text regexes (`ROADMAP_HEADLINE`, `ROADMAP_FIGURES`,
    `ROADMAP_PEP_FIGURES`, `ACCEPT_CLAUSE_FIGURES`) that do not track any
    running state across file boundaries the way the Ruby checker's
    heading-path stack does.

    Fails closed when neither the file nor a non-empty directory exists, by
    falling through to `path.read_text()` so the caller still sees the
    ordinary `FileNotFoundError` rather than a bespoke one.
    """

    if path.exists():
        return path.read_text(encoding="utf-8")

    roadmap_dir = path.parent / "roadmap"
    if roadmap_dir.is_dir():
        files = sorted(roadmap_dir.glob("**/*.md"))
        if files:
            return "".join(file.read_text(encoding="utf-8") for file in files)

    return path.read_text(encoding="utf-8")


def check_roadmap_counts(
    roadmap: str, rows: list[MatrixRow], label: str = "docs/ROADMAP.md"
) -> None:
    """Bind `docs/ROADMAP.md`'s stated totals to the ones the matrix supports.

    `label` names the file in diagnostics. `main` passes the path relative to
    the repository root when the file lives inside it, so the ordinary run
    reports `docs/ROADMAP.md` rather than an absolute path, and an out-of-tree
    `--roadmap` is reported as itself.

    Raises `BreadthError` when the headline is missing, ambiguous, unparseable,
    internally inconsistent, or disagrees with `rows`; also when the
    milestone's own `**Accept:**` bullet is missing or ambiguous, or when the
    headline's stated `required`/`pep_required` totals drift from that
    bullet's own figures (#732).
    """

    headlines = ROADMAP_HEADLINE.findall(roadmap)
    if len(headlines) != 1:
        raise BreadthError(
            f"{label}: expected exactly one `**Conformance progress (...)**` "
            f"headline stating the matrix totals, found {len(headlines)} -- this guard "
            f"is fail-closed, so restore the headline rather than rewording it away"
        )

    figures = ROADMAP_FIGURES.search(headlines[0])
    if figures is None:
        raise BreadthError(
            f"{label}: the conformance-progress headline no longer states its "
            "totals in the form this guard parses (`N of the required M matrix rows "
            f"are at `{SUBSET}` or better, leaving a G-row gap; A of those N are "
            f"`{ACCEPTED}`)"
        )

    pep_figures = ROADMAP_PEP_FIGURES.search(headlines[0])
    if pep_figures is None:
        raise BreadthError(
            f"{label}: the conformance-progress headline no longer states its "
            "distinct-PEP totals in the form this guard parses (`encompass N of "
            "the required M distinct PEP numbers, leaving a G-PEP gap`)"
        )

    accept_matches = list(ACCEPT_CLAUSE_FIGURES.finditer(roadmap))
    if len(accept_matches) != 1:
        raise BreadthError(
            f"{label}: expected exactly one `**Accept:**` bullet stating "
            "`conformance ≥ N ... matrix rows ... encompassing M distinct PEP "
            f"numbers` to bind the progress headline's required totals to, "
            f"found {len(accept_matches)} -- this guard is fail-closed, so "
            "restore that bullet's wording rather than rewording it away"
        )
    accept_figures = accept_matches[0]

    total = int(figures["total"])
    required = int(figures["required"])
    gap = int(figures["gap"])
    accepted = int(figures["accepted"])
    restated_total = int(figures["restated_total"])

    pep_total = int(pep_figures["pep_total"])
    pep_required = int(pep_figures["pep_required"])
    pep_gap = int(pep_figures["pep_gap"])

    accept_required = int(accept_figures["required"])
    accept_pep_required = int(accept_figures["pep_required"])

    computed_total = len(rows)
    computed_accepted = sum(1 for row in rows if row.status == ACCEPTED)
    computed_pep_total = distinct_pep_count(rows)

    failures: list[str] = []
    if total != computed_total:
        failures.append(
            f"{label} claims {total} evidence-backed rows, but the matrix has "
            f"{computed_total}"
        )
    if accepted != computed_accepted:
        failures.append(
            f"{label} claims {accepted} whole-PEP (`{ACCEPTED}`) rows, but the "
            f"matrix has {computed_accepted}"
        )
    if restated_total != total:
        failures.append(
            f"{label}'s headline states {total} evidence-backed rows but then "
            f"says `{accepted} of those {restated_total}`"
        )
    expected_gap = max(0, required - total)
    if gap != expected_gap:
        failures.append(
            f"{label} states a {gap}-row gap, but {required} required minus "
            f"{total} evidence-backed is {required - total} "
            f"(a met or exceeded floor states a 0-row gap, not a negative one, "
            f"so the expected gap here is {expected_gap})"
        )
    if pep_total != computed_pep_total:
        failures.append(
            f"{label} claims {pep_total} distinct PEP numbers, but the matrix "
            f"encompasses {computed_pep_total}"
        )
    expected_pep_gap = max(0, pep_required - pep_total)
    if pep_gap != expected_pep_gap:
        failures.append(
            f"{label} states a {pep_gap}-PEP gap, but {pep_required} required "
            f"minus {pep_total} distinct PEP numbers is "
            f"{pep_required - pep_total} (a met or exceeded floor states a "
            f"0-PEP gap, not a negative one, so the expected gap here is "
            f"{expected_pep_gap})"
        )
    if required != accept_required:
        failures.append(
            f"{label}'s progress headline states {required} required matrix "
            f"rows, but the milestone's own `**Accept:**` bullet states "
            f"{accept_required} -- the headline's required figure must track "
            "the normative Accept clause, not drift from it"
        )
    if pep_required != accept_pep_required:
        failures.append(
            f"{label}'s progress headline states {pep_required} required "
            f"distinct PEP numbers, but the milestone's own `**Accept:**` "
            f"bullet states {accept_pep_required} -- the headline's required "
            "figure must track the normative Accept clause, not drift from it"
        )

    expected = summary_body(rows)
    if expected not in roadmap:
        failures.append(
            f"{label} does not quote the checker's current summary verbatim; "
            f"expected to find {expected!r}"
        )

    if failures:
        raise BreadthError("\n".join(failures))


def main(argv: list[str] | None = None) -> int:
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--matrix", type=Path, default=root / "docs/PYTHON_STANDARDS.md")
    parser.add_argument(
        "--manifest",
        type=Path,
        default=root / "tests/fixtures/conformance-breadth-manifest.json",
    )
    parser.add_argument(
        "--harness",
        type=Path,
        default=root / "tests/conformance.rs",
        help=(
            "crate root of the conformance harness; its sibling <stem>/*.rs cohort "
            "files are concatenated after it"
        ),
    )
    parser.add_argument("--roadmap", type=Path, default=root / "docs/ROADMAP.md")
    args = parser.parse_args(argv)

    try:
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        print(f"{args.manifest}: invalid JSON: {error}", file=sys.stderr)
        return 1

    try:
        validate(
            args.matrix.read_text(encoding="utf-8"),
            manifest,
            read_harness(args.harness),
        )
    except BreadthError as error:
        print(str(error), file=sys.stderr)
        return 1

    rows = evidence_rows(args.matrix.read_text(encoding="utf-8"))

    try:
        try:
            label = str(args.roadmap.resolve().relative_to(root))
        except ValueError:
            label = str(args.roadmap)
        check_roadmap_counts(resolve_roadmap_text(args.roadmap), rows, label)
    except BreadthError as error:
        print(str(error), file=sys.stderr)
        return 1

    print(f"conformance breadth: {summary_body(rows)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
