# 2026-09-05 — #919: `from __future__ import ...` as a compile-time no-op (D-229)

## Previous checkpoint's outcome

The iteration before this one delivered
[#923](https://github.com/rotnov/pycc/issues/923) (llms.txt per-resource
budgets, D-227): PR [#928](https://github.com/rotnov/pycc/pull/928) merged by
squash as `c639e682` at 2026-09-04T21:23:32Z, issue #923 is CLOSED, and its
branch was deleted. `autopilot/iter-2026-09-04-08` was then re-based on the
remote default branch at `4eca5e24` (`feat(pycc_codegen): return container
types from functions (#925, Part 2 of #918) (#933)`), which was still
`origin/main` when this snapshot was written.

## Overall status

Implemented [#919](https://github.com/rotnov/pycc/issues/919) on
`autopilot/iter-2026-09-04-08` following the four-round-reviewed plan at
<https://github.com/rotnov/pycc/issues/919#issuecomment-5552207393>. One pull
request, body carrying `Fixes #919`; the orchestrating session watches CI and
merges. The decision record is
[D-229](../decisions/D-229-reserve-from-future-import-as-a-compile-time-directive.md).

## What the change is

`__future__` is a compiler directive, never a module, reserved at both
module-level import sites through one predicate
(`crates/pycc_hir/src/import.rs::is_future_import`):

- `project_import_request` no longer forwards it to the driver, so a sibling
  `__future__.py` is never loaded (it was, before this change).
- `lower_import_stmt` routes it to `lower_future_import` ahead of the
  `pycc_std` registry fallback. The nine no-op features lower to nothing; an
  unknown name (including `*`), `braces`, and a future import after the
  docstring-and-future-imports prologue are `L0001` with CPython 3.14's
  wording; `barry_as_FLUFL` and `x as y` are `C0001`. The prologue is computed
  once per module (`future_prologue_len`) and threaded as a two-variant
  `FuturePosition` through `module::lower_module` → `lower_top_level_item`.
- `module::poisonable_names` mirrors the success condition (position-blind,
  a recorded divergence from D-222's poison rule, harmless because an accepted
  future import binds nothing).

Tests: unit tests in `crates/pycc_hir/src/module/tests.rs` (every feature
name, the precedence ladder, the prologue shapes, the cascade, six new
`IMPORT_SHAPES` rows) and `crates/pycc_hir/src/import/tests.rs` (the driver
request is skipped); four CLI fixtures under `tests/diagnostics/`;
`tests/issue_919_future_import.rs` (reproduction, multi-name after a
docstring, the never-loaded `__future__.py` sibling);
`tests/fixtures/pep_0563_lazy_annotations.py` registered as an `#[ignore]`d
dual-profile conformance test in `tests/conformance/classes.rs` (compared by
hand against the local CPython 3.14.6 — the pinned 3.14.7 oracle is CI-only).

## Deviations from the plan

- `docs/PYTHON_STANDARDS.md` note 13 shifted the matrix by 11 lines, so every
  `matrix_line` in `tests/fixtures/conformance-breadth-manifest.json` is
  bumped by 11 (the #925 precedent). The plan did not anticipate the twin.
- The end-to-end position test drops the `b"doc"` first-statement shape
  because pycc rejects a bytes-literal expression statement itself (`C0001`)
  before the position `L0001`; the shape is pinned on `future_prologue_len`
  instead.

## Known follow-ups

- [#937](https://github.com/rotnov/pycc/issues/937) — flip the PEP 563 row
  to `◐` once the fixture is observed green on `main`, with the manifest entry
  (`core` gaps: later-defined forward references, string annotations,
  `__annotations__` introspection) and the ROADMAP headline re-derivation.
- [#889](https://github.com/rotnov/pycc/issues/889) — string-literal and
  attribute-qualified annotations, still `C0001` with or without the directive.
- [#882](https://github.com/rotnov/pycc/issues/882) — its
  `__future__.annotations` "typing-only registration" candidate is superseded
  by this change (no registry entry); the rest of the widening is untouched.
- A future import inside a function/block body keeps the generic block-import
  `C0001`; one inside `if TYPE_CHECKING:` is silently accepted (body folded
  away). Both recorded in D-229, neither locked in by a test.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4`.
- Last iteration outcome: #928 merged (`c639e682`), #923 closed.
- This iteration: #919 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty. #414, #585, #636, #641, #408, and #706 were excluded by the
  blocker screen, not denylisted.

## Where to resume

`crates/pycc_hir/src/import.rs` (`is_future_import`, `future_prologue_len`,
`FuturePosition`, `lower_future_import`) is the whole story;
`crates/pycc_hir/src/module.rs` only threads the position and mirrors the
success condition in `poisonable_names`. Read D-229 for the routing table and
the recorded divergences before touching either.
