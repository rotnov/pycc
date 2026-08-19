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
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
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


def is_registered(harness: str, fixture: str) -> bool:
    """Whether `tests/conformance.rs` joins this fixture as a path.

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
                    f"{where}: `{fixture}` is not registered in tests/conformance.rs, "
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


def main(argv: list[str] | None = None) -> int:
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--matrix", type=Path, default=root / "docs/PYTHON_STANDARDS.md")
    parser.add_argument(
        "--manifest",
        type=Path,
        default=root / "tests/fixtures/conformance-breadth-manifest.json",
    )
    parser.add_argument("--harness", type=Path, default=root / "tests/conformance.rs")
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
            args.harness.read_text(encoding="utf-8"),
        )
    except BreadthError as error:
        print(str(error), file=sys.stderr)
        return 1

    rows = evidence_rows(args.matrix.read_text(encoding="utf-8"))
    accepted = sum(1 for row in rows if row.status == ACCEPTED)
    print(
        f"conformance breadth: {len(rows)} evidence-backed rows, all declared "
        f"({accepted} accepted as whole-PEP, {len(rows) - accepted} subset)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
