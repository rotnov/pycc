---
id: D-176
title: "Declare per-row conformance breadth in a validated manifest"
status: accepted
---

## D-176: Declare per-row conformance breadth in a validated manifest

- Status: accepted
- Context: `docs/PYTHON_STANDARDS.md`'s matrix carries one status marker per
  PEP row, and `tests/conformance_matrix_guard.rs` proves only that a `✅`
  row cites a fixture that exists and is registered in `tests/conformance.rs`
  ([D-175](./D-175-scope-the-conformance-matrix-fixture-guard-to-green.md)).
  Nothing checks how much of the PEP that fixture actually exercises, so a
  single smoke fixture can flip a whole PEP green. Several rows already
  compensate informally by carrying qualifying scope prose in the Feature
  column ("v0.2 scope per D-088 — no class support exists" on PEP 695,
  "pycc has no bytecode/frame model" on PEP 709, "#387 Part 1" on PEP 673),
  which proves the need is real but leaves the qualification unstructured,
  unvalidated, and invisible to any count of "how many PEPs are green".
  [D-153](./D-153-set-the-v0-3-conformance-target-at-37-rows-of-39.md)'s
  milestone target is stated in exactly that unqualified row count.
- Decision: every `✅` row in `docs/PYTHON_STANDARDS.md` declares its breadth
  in `tests/fixtures/conformance-breadth-manifest.json`: what the row's cited
  fixtures actually prove (`proven`, each item naming a category and the
  fixture that is its evidence) and what the PEP contains that they do not
  (`not_proven`, each item naming a category and the reason, optionally the
  issue tracking it). `scripts/check_conformance_breadth.py` validates the
  manifest against the matrix, parsing the matrix with exactly the rules
  `tests/conformance_matrix_guard.rs` uses so the two checks cannot disagree
  about which rows are green or which fixtures a row cites. It enforces a
  bijection between manifest entries and green rows in both directions, that
  each `proven` item's evidence is one of that row's own fixtures, that every
  cited fixture is registered in `tests/conformance.rs`, and that a matrix
  with no green rows fails rather than passing vacuously.
  `scripts/test_check_conformance_breadth.py` is its mutation self-test.
- Alternatives:
  - *Keep the informal Feature-column prose.* Rejected: it is unvalidated
    free text, so it rots silently, and it cannot be aggregated — the count
    that D-153 gates the milestone on stays unqualified either way.
  - *Widen `tests/conformance_matrix_guard.rs` to check breadth.* Rejected:
    D-175 deliberately scoped that guard to green rows and to fixture
    existence; breadth is a different contract with a different data source,
    and folding it in would make one failure mode indistinguishable from the
    other. It also cannot run in this environment (LLVM 22 is unavailable, so
    `tests/` does not compile locally), whereas a pure-Python checker does.
  - *Split rows in the matrix instead, one per PEP sub-feature.* Rejected as
    the first step: it changes the row count that D-153 and `docs/ROADMAP.md`
    both cite, and it is the corrective action, not the contract. Recording
    breadth first produces the evidence that any later re-scoping needs.
- Consequences: adding or flipping a green matrix row now also requires a
  manifest entry, and the checker fails until one exists. The manifest makes
  overclaimed rows visible and enumerable for the first time; correcting them
  will likely *lower* the green count, which is the intended outcome rather
  than a regression. This entry defines the contract and lands its validator
  only — reconciling the overclaimed rows and the milestone count it feeds,
  and wiring the checker into required CI, are deliberately separate changes.
