---
id: D-177
title: "Scope matrix acceptance to proven semantics"
status: accepted
---

## D-177: Scope matrix acceptance to proven semantics

- Status: accepted
- Context:
  [D-176](./D-176-declare-per-row-conformance-breadth-in-a-validated.md) made
  every green row in `docs/PYTHON_STANDARDS.md` declare what its fixtures
  prove and what they do not, and populating that manifest produced the
  inventory [#248](https://github.com/rotnov/pycc/issues/248) asked for: of
  the 29 rows then marked `✅`, 27 record at least one category of the PEP
  that no fixture reaches. PEP 498's row is green on a fixture that
  interpolates names and never uses a format specifier; PEP 3105's proves
  `print` is callable but not `sep=`/`end=`; PEP 526's covers simple annotated
  assignment but not a parenthesized target. `✅` therefore did not mean what
  its own Conventions block said it meant — the marker read as whole-PEP
  acceptance while the evidence behind most rows was a slice. That mattered
  beyond honesty in a table:
  [D-153](./D-153-correct-v0-3-s-conformance-target-before-any-v0.md) gates
  v0.3 on a count of exactly these rows, so the milestone criterion inherited
  the overclaim.
- Decision: the matrix carries four statuses instead of three. `◐` (**subset**)
  and `✅` (**accepted**) both mean the row's fixtures pass; they differ only
  in how much of the PEP the row is allowed to claim. Every gap a row records
  in `tests/fixtures/conformance-breadth-manifest.json` is classified `core`
  (a category of the PEP that is simply not implemented or not exercised) or
  `out-of-scope` (a deliberate, permanent non-goal for pycc, not an
  unimplemented category), and `scripts/check_conformance_breadth.py` enforces
  the rule in both directions: **any `core` gap forces `◐`; `✅` requires
  zero.** Which marker a row may carry is thus not a judgment call made row by
  row at review time — it is derived mechanically from the classified gaps.
  The manifest moves to `manifest_version: 2`, which makes `kind` required on
  every `not_proven` item. `tests/conformance_matrix_guard.rs` treats `◐` and
  `✅` identically: both cite fixtures claimed to pass, so D-175's
  existence-and-registration reasoning applies to them unchanged.

  Applying the rule re-marks 27 of the 29 rows `◐`. Only PEPs 414 and 594
  remain `✅`, because their recorded gaps are the only two that are permanent
  non-goals rather than unimplemented work.

  v0.3's Accept criterion is restated in terms of the unit it always actually
  measured: **≥ 37 rows at `◐` or better, encompassing 39 distinct PEP
  numbers.** The whole-PEP `✅` count is reported alongside it and is
  deliberately **not gated before v1.0**. This supersedes the *unit* of
  D-153's target, not its numbers: 37 and 39 are unchanged, and D-153's own
  itemized derivation of them stands.
- Alternatives:
  - *Restate v0.3's target against the `✅` unit instead.* Rejected. It is the
    reading #594 itself predicted, and it does not survive the numbers: the
    honest whole-PEP count is **2**. A target of 37 whole-PEP acceptances is
    unreachable in v0.3 and in several milestones after it, so adopting it
    would replace one fiction with another and stall the milestone on a
    criterion nobody intended to set. The `◐`-or-better count is what the
    figure has measured since it was written; naming it correctly is the
    correction, and the objection that this "preserves the number by
    relabeling" is answered by the fact that the specific move it fears — a
    narrow fixture being promoted to whole-PEP acceptance — is now
    mechanically impossible.
  - *Split each overclaimed row into one row per sub-feature.* Rejected, as it
    was in D-176 for the same reason it is rejected here: a row's identity is
    `(pep, fixtures)` throughout the guard, the checker, and the manifest, so
    splitting rows changes every count in flight at once and needs its own
    reconciliation before it can be evaluated. It remains available later, on
    top of the classified inventory this entry produces.
  - *Derive each row's required categories from the PEP text upstream.*
    Rejected: there is no machine-readable source of a PEP's semantic
    categories, so this amounts to hand-authoring the same list with an
    appearance of derivation, while adding a network dependency to a merge
    gate.
  - *Leave the markers alone and qualify the count in prose.* Rejected: prose
    beside a table is exactly the unvalidated qualification D-176 replaced,
    and it leaves `✅` meaning something different from what the Conventions
    block says.
- Consequences: the matrix's headline green count drops from 29 `✅` to 2
  `✅` plus 27 `◐`, with no change to what the compiler does — the number was
  wrong, not the compiler. v0.3's tracked progress is unchanged at 29 of 37,
  because that criterion counts `◐`-or-better rows. Promoting a row from `◐`
  to `✅` now requires either implementing its core gaps or justifying each as
  a permanent non-goal in the manifest, reviewed like any other change.
  Enforcing this contract in required CI is
  [#595](https://github.com/rotnov/pycc/issues/595); until it lands, the
  checker runs locally and via its self-test rather than as a merge gate.
