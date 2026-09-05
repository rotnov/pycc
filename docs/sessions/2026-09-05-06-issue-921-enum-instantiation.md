# 2026-09-05 — #921: calling an `Enum` class diagnoses instead of panicking

## Context

[#921](https://github.com/rotnov/pycc/issues/921): `Color()` and `Color(1)`
on a PEP 435 enum class aborted the compiler at
`pycc_types::class::binding::resolve_instantiation`'s `__init__` MRO walk
(`internal error: no `__init__` found in class `Color`'s MRO`), because
`lower_enum_class` deliberately lowers an enum with no constructor and
D-225's `ensure_init` never sees it. Implemented from the plan published as
issue comment 5553043236, against `origin/main` `2c92a74b` (#940 merged)
and rebased cleanly onto `cf7cc40f` (PR #935's merge) before review.
The pull request delivering this snapshot carries `Fixes #921`.

## What this pull request changes

- `HirClassDef.is_enum: bool` (`crates/pycc_hir/src/class.rs`): the enum
  provenance marker, set only by `lower_enum_class` (`Enum` and `StrEnum`
  alike) and `false` at every other construction site (155 struct literals,
  compiler-checked). A member-less docstring-only enum (#744) has an empty
  `enum_members` table, so the table is not the marker; its doc comment now
  says so and records why the member-keyed consumers stay as they are.
- `resolve_instantiation` rejects `is_enum` classes with `C0001`
  (``cannot call enum class `Color` -- ...``) after the protocol check and
  before the MRO walk. "Not supported yet" attaches only to the by-value
  lookup form (`Color(1)`); `Color()` is named as the CPython `TypeError` it
  is, since no later slice will accept it.
- The two internal-error panics (`binding.rs`, `crates/pycc_mir/src/expr.rs`)
  and the `#[should_panic]` test comment now state the invariant that holds:
  every non-enum class has an `__init__` (D-225), and an enum class is
  rejected before either walk. The `should_panic` expected fragments are
  byte-identical.
- Tests: five unit tests in `binding.rs`'s `mod tests` (`Color()` in a
  function, `Color(1)`, docstring-only enum, `StrEnum`, `raise Color()`),
  `is_enum` assertions in the HIR enum and `ensure_init` unit tests, and
  `tests/issue_921_enum_call.rs` (six `pycc check`/`pycc build` cases
  asserting exit status 1, `error[C0001]`, and no `panicked`/`internal error`).
- Docs: `docs/TYPE_SYSTEM.md` (`enum.Enum` row and the #912 paragraph),
  `docs/ROADMAP.md` (#432 and #379 paragraphs; the #379 paragraph's stale
  "stays `☐`" now reads `◐`, matching `docs/PYTHON_STANDARDS.md`),
  `docs/DIAGNOSTICS.md` (`C0001` paragraph). No decision entry; D-225 is
  left as the accurate historical record and no `roadmap-evidence` box
  changes.

## Noticed and deferred

- Subclassing an enum class (`class Foo(Color): pass`) is accepted by
  `validate_bases`, and `Foo()` aborts the compiled program at runtime
  (`pycc_rt: invalid encoded int word 0x0`); CPython rejects the class
  definition. Found by this pull request's D-068 deep-review (P1), reproduced,
  and filed as [#941](https://github.com/rotnov/pycc/issues/941) rather than
  folded in here: it is a distinct HIR-lowering seam from #921's
  instantiation guard and pre-dates this diff, so per the one-issue-per-gap
  rule it gets its own pull request.
- By-value member lookup (`Color(1)` compiling) is a separate feature
  (MIR/codegen plus a runtime `ValueError` path).
- Real source spans for `pycc_types` diagnostics: #877. The new diagnostic
  renders at `1:1` like its sibling instantiation rejections.
- Member equality: #908. Migrating the `!enum_members.is_empty()` sites to
  `is_enum`, and what a member-less enum does in `for`/`match`, are untouched.

## Where a fresh session should look

`origin/main` after this merge; `docs/ROADMAP.md`'s v0.4 section and the
open v0.4 milestone list for the next `issue-select` pass.
