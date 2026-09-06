# 2026-09-06 — #960: a sibling base's class attribute no longer shadows an instance slot

## Previous checkpoint's outcome

Iteration 19 delivered [#914](https://github.com/rotnov/pycc/issues/914) (a
class attribute satisfies a `Protocol` attribute member): PR
[#961](https://github.com/rotnov/pycc/pull/961) merged by squash as
`f29d245b7885fff2f5fcc63432b05fb74de5f91e` and #914 is CLOSED. CI on that pull
request was green on the first try.

Post-merge `main` runs for `f29d245b`, re-queried immediately before this
snapshot was committed, were all `completed`/`success`: CI
[34017171535](https://github.com/rotnov/pycc/actions/runs/34017171535), Main
history audit
[34017171527](https://github.com/rotnov/pycc/actions/runs/34017171527), and
Status page freshness
[34017171507](https://github.com/rotnov/pycc/actions/runs/34017171507). No
Pages run and no Source link check run is listed for that commit. (A CI failure
confined to the nbody benchmark would have been the known
[#641](https://github.com/rotnov/pycc/issues/641); none occurred.) For the
prior commit `f77544d8`: CI
[34013629258](https://github.com/rotnov/pycc/actions/runs/34013629258) and
Source link check
[34015820753](https://github.com/rotnov/pycc/actions/runs/34015820753), both
success.

Selection note: the advisor round **changed** the pick from #913 to #960. #960
is one function's statement order in `pycc_mir` and it is a soundness defect —
silently wrong output, plus a types/MIR disagreement that aborts `pycc_codegen`
outright when the two declared types differ. #913 measured larger: two HIR
seams (rerouting `ClassVar` out of `dataclass_fields`, and a
`reject_class_attr_collisions` ordering fix relative to the dataclass field
merge) plus a conformance fixture that needs a
`tests/fixtures/conformance-breadth-manifest.json` edit, since the PEP 557 row
lists `ClassVar` under `not_proven`. Two corrections recorded for #913's future
pick: `synthesize_dataclass_init`/`_eq`/`_repr` all take `&merged_fields`, so
excluding the `ClassVar` from `dataclass_fields` is sufficient; and Seam B's
uncovered direction is only the class's **own** dataclass fields — inherited
ones are already checked, because a base dataclass's `attrs` is its merged
fields. [#958](https://github.com/rotnov/pycc/issues/958) measured larger still
(it defers a conformance-side vs post-monomorphization decision and touches
`monomorphize.rs`).

## Overall status

Implemented #960 on `autopilot/iter-2026-09-06-20`, cut from `f29d245b`. One
pull request carrying `Fixes #960`; the orchestrating session watches CI and
merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 960, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #960
([issuecomment-5557710899](https://github.com/rotnov/pycc/issues/960#issuecomment-5557710899)),
published against `f29d245b` after three adversarial review rounds; this
snapshot records where the implementation followed it and where it deviated.

## What the change is

`crates/pycc_mir/src/expr.rs`'s instance `AttrGet` arm ran its
`fold_class_attr` MRO walk **before** the `mro_attrs` instance-slot lookup. Two
independent sibling MRO bases — one contributing the slot in its `__init__`,
the other a same-named class attribute — are never compared by
`pycc_hir`'s `reject_class_attr_collisions`, which walks only a class's *own*
`class_attrs` against its own MRO. So the shape is legal, and the fold won.

The fix hoists the non-panicking slot lookup above the fold loop, leaving the
property walk first, so the arm mirrors
`pycc_types::class::resolve_attr_get`'s properties → instance slots → class
attributes order exactly, and leaving the existing `panic!` as the tail. One
new branch, not a restructure.

**Route decision — MIR reorder, not an HIR `C0001`.** The issue's own Scope
preferred extending the collision check to reject the shape. Rejected, and the
reasoning is recorded in the published plan and in the code comments:

- `pycc_types` **already** implements CPython precedence here, and #914 (merged
  one commit earlier) added `protocol_conformance_prefers_a_sibling_base_instance_attribute`
  to pin it. The shape is modelled and tested; only `pycc_mir` was out of sync.
  D-224/#910's "reject what cannot be modelled" reasoning was about a class
  attribute shadowing a *method*, where CPython's answer depends on binding
  order — not this.
- The rejection route would have to delete that just-merged test, whose
  `check_source` helper `expect`s HIR lowering to succeed, and rewrite the
  `.or_else` rationale it pins.
- Rejecting a program CPython accepts and pycc can compile correctly is
  strictly worse than compiling it correctly.

No ADR: this is a bug fix bringing one layer into agreement with an existing,
already-accepted contract in another. It reverses nothing and establishes no
new project-wide constraint.

## Deviations from the plan

- **The mirror shape could not be tested end to end, because of a separate
  pre-existing bug now filed as
  [#966](https://github.com/rotnov/pycc/issues/966).** The plan's acceptance
  list included the mirror order (`class A` contributing the class attribute,
  `class B` the slot, `class C(A, B)`). It aborts in `pycc_rt` with
  `invalid encoded int word 0x0` — because an `__init__` inherited from a
  **non-first** MRO base is never called at all, so the slot is left as a raw
  zero word. Verified as pre-existing by rebuilding with `origin/main`'s own
  `expr.rs` and reproducing with a snippet that has no name collision
  whatsoever (`class A` with only a method, `class B` with the `__init__`,
  `class C(A, B)`, read `c.z`). It is independent of #960 and untouched here;
  the mirror case is listed in #966's acceptance criteria instead.
- **A regression fixture had to read `B.LIMIT`, not `C.LIMIT`.** Reading an
  inherited class attribute through the *derived class name* is rejected with
  `T0044` ("class `C` has no attribute named `LIMIT`") — the class-name fold
  path consults only the named class, never its MRO. Also pre-existing, also
  unrelated to the instance read path this change touches; not filed, since it
  is the documented `C.X` surface rather than a wrong answer.
- **Two conservative `pycc_types` pre-checks are deliberately left standing,
  and are now filed as [#965](https://github.com/rotnov/pycc/issues/965).**
  `check_attr_set` and `infer_expr_in`'s non-bare-name-base gate both reject a
  name purely because a class attribute of that name exists somewhere in the
  MRO, never asking whether it *wins* the corrected precedence. After this fix
  their messages ("no storage to write to"; "would discard the base
  expression") are inaccurate for the cross-sibling shape — but both are
  rejections of programs CPython accepts, never mis-compiles, and lifting them
  relaxes `docs/TYPE_SYSTEM.md`'s documented "every write path is `T0044`"
  contract, which is decision-bearing work of its own. Both are pinned by guard
  tests in the new end-to-end file so #965 has to update them deliberately
  rather than silently.
- **Three review rounds, not two.** Round 1 corrected a miscount in the plan's
  own doc-site list and added two stale comments; round 2 added three more
  stale sites plus the `mro_attrs` cost and no-new-panic arguments; round 3
  fixed a line-range citation and found the read-side gate above. Rounds 2 and
  3 both changed the plan, so the loop was not clean at two.
- **`docs/ROADMAP.md` was not touched at all** — no new feature paragraph, so
  the status-page four-pin rotation is not triggered; proved with
  `ruby scripts/check_status_page_freshness.rb origin/main`, exit 0. No new
  `docs/decisions/` entry, per the route decision above.

## Known follow-ups

- [#965](https://github.com/rotnov/pycc/issues/965) — the two coarse
  `pycc_types` gates; filed by this iteration, the recorded out-of-scope
  boundary for this change.
- [#966](https://github.com/rotnov/pycc/issues/966) — an `__init__` inherited
  from a non-first MRO base is never called; filed by this iteration, a silent
  compile with a runtime abort, and arguably the more serious of the two.
- [#913](https://github.com/rotnov/pycc/issues/913) — the last remaining #911
  follow-up; see the selection note above for the two corrections that shrink
  it.
- [#958](https://github.com/rotnov/pycc/issues/958) — attribute store through a
  Protocol-typed receiver panics in `pycc_mir`.
- [#908](https://github.com/rotnov/pycc/issues/908) — enum equality; needs MIR
  and codegen identity lowering.
- [#944](https://github.com/rotnov/pycc/issues/944),
  [#954](https://github.com/rotnov/pycc/issues/954),
  [#952](https://github.com/rotnov/pycc/issues/952) — untouched, for
  `issue-select` to weigh.
- [#885](https://github.com/rotnov/pycc/issues/885) — the parent, which stays
  open until #913 closes.
- [#336](https://github.com/rotnov/pycc/issues/336) — still excluded; it spans
  four independent seams and belongs to the multi-PR decomposition class.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #914 closed by PR #961 (`f29d245b`).
- This iteration: #960 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

Read the comment block above the hoisted slot lookup in
`crates/pycc_mir/src/expr.rs` first. It states the invariant the whole change
rests on: `pycc_types::class::resolve_attr_get` and this arm must order their
walks identically — properties, then instance slots, then class attributes —
because `reject_class_attr_collisions` does not make the two mutually
exclusive, and a divergence between them is not a wrong value but an internal
abort in `pycc_codegen` when the declared types differ.

Anyone touching class attributes next should know that the *ordering* fix is
now complete on the read path and deliberately incomplete on the two gates
#965 tracks. The pattern to look for in both: a check that asks "does a class
attribute of this name exist in the MRO" where it should ask "does it win".

`tests/issue_960_sibling_base_class_attr.rs` is the map — three tests that fail
under the old order and pin the fix, and three guards for behavior that must
not move by accident.
