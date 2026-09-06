# 2026-09-06 — #916: `Final[X]` on a class-level attribute

## Previous checkpoint's outcome

Iteration 17 delivered [#915](https://github.com/rotnov/pycc/issues/915)
(`super().CLASS_CONST` reaches a base class's class attribute): PR
[#957](https://github.com/rotnov/pycc/pull/957) merged by squash as
`73b15320815cd6ff0db66f9f9ffa44299fc47cf0` and #915 is CLOSED. CI on that
pull request was green on the first try.

Post-merge `main` runs for `73b15320` were all `completed`/`success`, each
re-queried immediately before this snapshot was committed: CI
[34009293382](https://github.com/rotnov/pycc/actions/runs/34009293382), Main
history audit
[34009293391](https://github.com/rotnov/pycc/actions/runs/34009293391),
Status page freshness
[34009293389](https://github.com/rotnov/pycc/actions/runs/34009293389), and
Pages [34009293397](https://github.com/rotnov/pycc/actions/runs/34009293397).
The earlier `678486d1` runs also concluded `success`: CI
[34004127080](https://github.com/rotnov/pycc/actions/runs/34004127080) and
Pages [34004127054](https://github.com/rotnov/pycc/actions/runs/34004127054).
(A CI failure confined to the nbody benchmark would have been the known
[#641](https://github.com/rotnov/pycc/issues/641); none occurred.)

Selection note: the advisor round **changed** the pick from
[#914](https://github.com/rotnov/pycc/issues/914) to #916. #914 measures at
two seams in two crates — `lookup_attr_through_mro` in `pycc_types` *and*
`eval_isinstance_protocol` in `crates/pycc_mir/src/class.rs`, which would
otherwise fold `isinstance(c, HasLimit)` to `False` while the checker says
`C` conforms — whereas #916 is HIR-only. The same advisor round found a
pre-existing panic: an attribute store through a Protocol-typed receiver
bypasses the setter check (`check_attr_set` gates on `Ty::Instance` and
nothing re-checks after D-166 monomorphization), so a setter-less
`@property` member panics at `crates/pycc_mir/src/stmt.rs:582`. That is
filed as [#958](https://github.com/rotnov/pycc/issues/958) (v0.4). The
recorded resolution for #914 when it is picked: accept the class attribute
for conformance (mypy/pyright parity), fix `eval_isinstance_protocol` in the
same pull request, and leave the write-through hole to #958.

## Overall status

Implemented #916 on `autopilot/iter-2026-09-06-18`, cut from `73b15320`. One
pull request carrying `Fixes #916`; the orchestrating session watches CI and
merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 916, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #916
([issuecomment-5556789324](https://github.com/rotnov/pycc/issues/916#issuecomment-5556789324),
published against `73b15320` after two adversarial review rounds); this
snapshot records where the implementation followed it and where it deviated.

## What the change is

`crates/pycc_hir/src/class/attrs.rs`'s `lower_class_attr` opened with a
blanket rejection of every `Final` spelling on a class-body attribute. The
rejection was a scope boundary, not a semantic one: under #911/#910 a class
attribute is a compile-time constant folded at every read, it has no
instance slot, and every write path already reports `T0044`. `Final` adds no
guarantee the model does not already provide.

`strip_final` now mirrors `strip_class_var`'s shape and unwraps `Final[X]`,
`Final[X,]` (the one-element tuple slice `annotation_to_ty` already unwraps
in variable position), and a bare `Final`, whose type is inferred from the
literal right-hand side through #910's existing `infer_class_attr_ty`. Both
PEP 591-invalid nestings are rejected with their own messages.

Two things worth recording:

- **`is_class_var` had to be threaded, not recomputed.** `body.rs` calls
  `strip_class_var` *before* `lower_class_attr`, so by the time the
  annotation arrives a `ClassVar[Final[int]]` is byte-identical to a valid
  `Final[int]`. The flag is now carried alongside the stripped expression in
  a `StrippedAnnotation` struct rather than as a separate parameter —
  `clippy::too_many_arguments` fires at eight.
- **The inverse nesting needed its own matcher.** `strip_class_var` returns
  `Err` for a bare inner `ClassVar`, so it cannot be reused to detect
  `Final[ClassVar]`; `is_class_var_annotation` is a plain predicate over the
  two spellings. The first adversarial review round caught that the draft's
  detection recipe could not produce the promised message for
  `Final[ClassVar]`.

Zero blast radius outside `pycc_hir`, verified empirically rather than
assumed: `X: Final[int] = 1` and `X: int = 1` produce identical lowered
`class_attrs` and identical program output through `C.X`, `d.LIMIT`,
`self.LIMIT`, and `super().LIMIT`, and both are rejected identically on
write. `Environment.finals` is untouched — a class attribute produces no
`HirStmt::AnnAssign`, so `T0045` never applies to it.

## Deviations from the plan

- **Two extra in-crate unit tests, on pre-existing #911 paths.** The D-014
  gate initially failed with three missed regions in `attrs.rs`. The cause
  is not a new uncovered branch: `llvm-cov` scores a multi-instantiation
  function group by its *best single* instantiation, not by the union, and
  `lower_class_attr` grew large enough that the two instantiations
  (`pycc_hir`'s own unit-test build and the dependency build the end-to-end
  tests exercise) each covered a set the other missed. Adding
  `an_annotated_attribute_target_is_rejected` and
  `an_annotated_class_attribute_with_a_mismatched_literal_is_rejected`
  brings the unit-test instantiation to a full sweep. Both pin behavior that
  predates this change; the gate then reports 100.00%/100.00%.
- **One pin added after the review round.** The pinned reviewer observed that
  `Final[Final[int]]` is newly accepted (#911 rejected every `Final`
  spelling). Verified against the compiler: it is accepted at both the class
  and the module level, so the class body now matches the variable-level
  position rather than being stricter than it. That is the deliberate
  resolution; it is pinned in
  `every_final_spelling_lowers_identically_to_the_plain_annotation` and stated
  in `docs/TYPE_SYSTEM.md`.
- Nothing else. The two seams, the bare-`Final` acceptance via
  `infer_class_attr_ty`, both nesting rejections, the flipped #911 pins, the
  new end-to-end file, and the four documentation sites all followed the
  plan. `docs/ROADMAP.md` was not touched at all (no new feature paragraph,
  so the status-page rotation is not triggered — proved with
  `ruby scripts/check_status_page_freshness.rb origin/main`, exit 0). No new
  `docs/decisions/` entry: this widens #911's already-accepted model rather
  than reversing anything.

The fourth documentation site — the comment above `t0045_final_reassignment`
in `tests/diagnostics_test.rs` — was added by the second review round, which
also required the duplicate-name-outranks-invalid-nesting precedence test.

## Known follow-ups

- [#914](https://github.com/rotnov/pycc/issues/914),
  [#913](https://github.com/rotnov/pycc/issues/913) — the remaining #911
  follow-ups, untouched here. #914's recorded resolution is in the selection
  note above.
- [#958](https://github.com/rotnov/pycc/issues/958) — attribute store
  through a Protocol-typed receiver panics in `pycc_mir`; found by this
  iteration's advisor round, filed, untouched.
- [#908](https://github.com/rotnov/pycc/issues/908) — enum equality; needs
  MIR/codegen identity lowering.
- [#944](https://github.com/rotnov/pycc/issues/944),
  [#954](https://github.com/rotnov/pycc/issues/954),
  [#952](https://github.com/rotnov/pycc/issues/952) — untouched, for
  `issue-select` to weigh.
- [#877](https://github.com/rotnov/pycc/issues/877) — every `pycc_types`
  diagnostic still renders at `:1:1` (D-043's placeholder span).
- **`typing.Final[...]` is still unrecognized** in class-attribute position,
  as it is elsewhere in the class body — only the bare `Final` name is
  matched. Consistent with today's handling of `typing.ClassVar[...]`, and
  noted in `docs/TYPE_SYSTEM.md`, but it is a real gap for anyone who
  imports the module rather than the name.
- **`Final` on a parameter** remains out of scope and unchanged.
- **A `ClassVar` buried under two `Final` layers reports the generic
  message.** `Final[Final[ClassVar[int]]]` is still rejected with `C0001`, but
  by `annotation_to_ty`'s "`ClassVar` is only valid on a class-body attribute
  declaration" rather than by #916's nesting-specific text, because the
  nesting checks inspect only the annotation immediately under the first
  `Final`. Found by the pinned reviewer; the program is rejected either way,
  so only the message quality is at stake.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #915 closed by PR #957 (`73b15320`).
- This iteration: #916 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

The production change is two files, and all of the logic is in the first:
`crates/pycc_hir/src/class/attrs.rs`, plus a two-line call-site change in
`crates/pycc_hir/src/class/body.rs` that hands the already-computed
`is_class_var` flag along with the stripped annotation. Read `strip_class_var`
first — it is
the shape `strip_final` deliberately mirrors, and its doc comment explains
why `ClassVar` is *not* unwrapped by the shared `annotation_to_ty` the way
`Final` and `Annotated` are. `StrippedAnnotation`'s own comment states the
one fact that is not recoverable downstream (a stripped `ClassVar[Final[T]]`
is indistinguishable from `Final[T]`), which is the reason the struct exists
rather than a second parameter.

Anyone widening class attributes further should note the ordering inside
`lower_class_attr`: the reserved-name and duplicate-name checks run *before*
the `Final` nesting checks, so a duplicate name outranks an invalid nesting.
That precedence is pinned by
`a_duplicate_name_outranks_an_invalid_final_nesting`.

The coverage lesson generalizes past this issue: when a `pycc_hir` function
is exercised from both in-crate unit tests and end-to-end tests, the D-014
gate is satisfied only if *one* instantiation sweeps every region. Adding an
end-to-end test for a branch that in-crate tests miss does not close the
gap.
