# 2026-09-05 — #925: container return types (Part 2 of #918)

## Overall status

Implemented [#925](https://github.com/rotnov/pycc/issues/925), Part 2 of
[#918](https://github.com/rotnov/pycc/issues/918): a `list[T]` / `dict[K, V]` /
`set[T]` / `tuple[A, B]` annotation is now accepted in **return** position and
runs end to end. Delivered on `feat/issue-925-container-returns`, branched from
`origin/main` at `71ad0bd6` ("feat(pycc_hir): lower parameterized container type
annotations (#918 Part 1) (#930)"), which was still the remote default-branch
head when this snapshot was written. One pull request, body carrying `Fixes
#925` only.

## What the change is

Two seams, matching the two halves of the Part-1 split:

- **Codegen.** `crates/pycc_codegen/src/call_result.rs` is a new module carved
  out of `lib.rs` under AGENTS.md's decomposability rule (`lib.rs` 8843 → 8754
  lines; [#545](https://github.com/rotnov/pycc/issues/545) is narrowed, not
  closed). It holds the `MirExpr::Call` result dispatch and adds a real arm per
  container family — `List`/`Dict`/`Set` as opaque pointers, `Tuple` as D-115's
  by-value struct. The old `other => panic!` catch-all is gone: the match now
  ends in `Ty::Infer | Ty::Param(_) | Ty::Protocol(_)`, so a `Ty` variant added
  later is a compile error here rather than a runtime panic. That is what makes
  the "only these three remain unhandled" claim behind
  [#926](https://github.com/rotnov/pycc/issues/926) mechanical rather than a
  comment. The three are unhandled for two different reasons: `Ty::Infer` and
  `Ty::Param` cannot be produced by type-checked source at all, while
  `Ty::Protocol` is accepted by the front end and is unreached only because MIR
  lowering panics first — filed as
  [#934](https://github.com/rotnov/pycc/issues/934).
- **HIR.** `lower_return_annotation` no longer runs D-228's return-position
  `C0001`; it lowers the annotation exactly like every other position and joins
  the set that advises the parameterized form for a bare `list`/`dict`/`set`/
  `tuple`. Every element-type and arity gate still fires, on the return
  annotation's own span.

No ownership work accompanies this. Returning a container is a genuine pointer
transfer that adds no new free site, so lists, dicts and sets stay leak-only
(D-107, D-124) and no refcount wiring changed. The identity round-trip is pinned
by a test rather than left as prose.

## Measured deviation from the published plan

The plan's §11 predicted nested containers would report `T0034` on the return
span. That holds for `list[list[int]]` only. `tuple[list[int], int]` reports
**`T0039`** — D-116's tuple element gate rejects a non-`int`/`bool`/`float`
element before D-105's list gate ever sees the inner `list[int]`. Both shapes
are now enumerated concretely in `crates/pycc_hir/src/tests.rs`. A consequence
worth stating: because D-116 rejects the shape in the front end, a tuple
carrying a heap container never reaches codegen, so the leak-only ownership
finding is never exercised transitively through a tuple.

## Documentation touched

`docs/ROADMAP.md` (the v0.4 type-system status cell plus the two D-116/D-228
follow-up notes), `docs/TYPE_SYSTEM.md` (the container-annotation-surface
bullet: three deliberately-rejecting positions become two), `docs/PYTHON_STANDARDS.md`
line 326 and its `matrix_line: 326` twin in
`tests/fixtures/conformance-breadth-manifest.json` (row 23's third
`not_proven` entry becomes `proven`, evidenced by the extended
`tests/fixtures/pep_0585_builtin_generics.py`), and
`crates/pycc_diag/src/explain.rs`'s C0001 text (D-150 coupling; the C0001 entry
itself stays, and so does its protocol-attribute clause).
`docs/DIAGNOSTICS.md` needs no edit — verified rather than assumed: its C0001
row is generic and no row mentions return-position containers. No decision
record is added or edited: D-227 is unrelated (llms.txt budgets) and D-228 is an
accepted record that Part 2 delivers rather than revises.

## Known follow-ups

- [#926](https://github.com/rotnov/pycc/issues/926) — the unhandled
  container-typed call result it tracks is gone; what remains is the three
  now-explicit variants.
- [#927](https://github.com/rotnov/pycc/issues/927) — `-> list[int]: return []`
  is still `T0021`. #925 does not infer an empty literal's element type from its
  annotation, and a test pins that boundary.
- `pycc_types`' private-helper solver is still scalar-only, so an *unannotated*
  helper returning a container remains out of reach. The annotation is what
  supplies the type here.
- [#545](https://github.com/rotnov/pycc/issues/545) — `pycc_codegen/src/lib.rs`
  is narrowed by 89 lines and stays far above the ~1,000-line threshold.

## Where to resume

`crates/pycc_codegen/src/call_result.rs` is the whole codegen story;
`crates/pycc_hir/src/func.rs`'s `lower_return_annotation` is the whole front-end
story. End-to-end behaviour lives in `tests/issue_925_container_returns.rs`, and
the four inline `crates/pycc_codegen/src/tests.rs` unit tests are what actually
cover the new arms — llvm-cov takes the maximum over instantiations, not their
union, so an integration test alone would not have sufficed (the #603 precedent
in `docs/TESTING.md`).
