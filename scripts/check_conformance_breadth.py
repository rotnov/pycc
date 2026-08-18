#!/usr/bin/env python3
"""Check that every green conformance row declares the breadth its evidence proves.

`docs/PYTHON_STANDARDS.md`'s matrix has three statuses -- planned, in progress,
and passing -- and no way to say "passing, but only this slice of the PEP". A
green row therefore reads as whole-PEP acceptance no matter how narrow the
fixture behind it is, and `docs/ROADMAP.md`'s milestone gates count those rows
directly. Issue #248 records the consequence: several rows claim substantially
more than their one fixture exercises, while pycc still rejects other core
constructs from those same PEPs.

This checker is Part 1 of the fix (issue #593). It does not change any row's
status -- that is Part 2 (#594). What it does is make the gap *recorded and
machine-checked*: `tests/fixtures/conformance-breadth-manifest.json` carries, for
every green row, the semantic categories that row's evidence actually proves and
the categories that are known-unsupported or intentionally deviating, and this
script fails when the manifest and the matrix disagree. Each entry also carries
the row's `matrix_line`, checked against the parsed line number, so an entry
cannot silently drift onto a different row as the matrix grows.

The matrix is parsed with exactly the rules `tests/conformance_matrix_guard.rs`
uses -- five-cell rows whose status cell is one of the documented markers, and
backtick spans ending in `.py` as the cited fixtures -- so the two checks cannot
disagree about which rows are green or which fixtures a row cites.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
from typing import Any


MANIFEST_VERSION = 1
STATUS_MARKERS = ("☐", "⚙", "✅")
GREEN = "✅"


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


def green_rows(markdown: str) -> list[MatrixRow]:
    return [row for row in parse_matrix(markdown) if row.status == GREEN]


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

    matrix = green_rows(markdown)
    if not matrix:
        raise BreadthError(
            "no ✅ rows parsed out of the conformance matrix — "
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
                "which matches no ✅ row in the matrix"
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
                    f"{where}: PEP {pep} is ✅ but proves no semantic category — "
                    "a green row with no proven breadth is exactly the overclaim "
                    "this manifest exists to surface"
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
        _check_category_list(
            entry.get("not_proven"), "not_proven", where, ("category", "reason"),
            failures,
        )

        for fixture in fixtures:
            if not is_registered(harness, fixture):
                failures.append(
                    f"{where}: `{fixture}` is not registered in tests/conformance.rs, "
                    "so nothing runs it"
                )

    for key, row in matrix_by_key.items():
        if key not in seen:
            failures.append(
                f"line {row.line_number}: PEP {row.pep} is ✅ but has no breadth "
                "manifest entry declaring what its evidence proves"
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

    rows = green_rows(args.matrix.read_text(encoding="utf-8"))
    print(f"conformance breadth: {len(rows)} green rows, all declared")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
