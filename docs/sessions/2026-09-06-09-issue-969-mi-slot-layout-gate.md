# 2026-09-06 — #969: multiple inheritance with conflicting slot layouts is now rejected

## Previous checkpoint's outcome

Iteration 21 (`docs/sessions/2026-09-06-07-issue-966-inherited-init-rank.md`)
delivered #966 as PR #970, squash-merged to `main` as
`50a0dc2a24f1cde8ff6ace35f9ad30ee7786b911`, closing #966 and landing
**D-232**. CI was green on the first try.

While that iteration was in flight the concurrent actor merged PR #968 (#962,
stdlib `import X as Y`, **D-231**, session `-06-`) at 08:59Z, so iteration 21's
branch merged `origin/main` before pushing, with two trivial conflicts (the
`mod` list in `crates/pycc_types/src/tests.rs` and the generated decision table
in `docs/decisions/README.md`), both resolved by keeping both sides.

Post-merge runs on `50a0dc2a`, re-resolved immediately before committing this
entry:

- CI `34026612613` — success.
- Main history audit `34026612550` — success.
- Status page freshness `34026612616` — success.
- Pages `34026612629` — success.

On the preceding `ad5b2534` (PR #967, #960): CI `34020950245` success, Main
history audit `34020950235` success, Status page freshness `34020950256`
success.

The known nbody-only CI failure mode is #641; it did not appear here.

## Overall status

`autopilot/iter-2026-09-06-22` implements #969 and is pushed with a PR open.
Nothing is merged by this session.

Selection note: the advisor CONFIRMED #969 as this iteration's pick over #913,
#908, #958, #965, #944, #952 and #954 — it is the last remaining *soundness*
defect in the class model that produces a **silently wrong value** rather than
a loud failure, and #966 had just re-pinned it as a live limitation. The
advisor also amended the route in three ways that reshaped the plan before any
code was written: the predicate must be the naive exact-prefix test (a
"liveness" refinement is unsound, because `super()` re-enters a shadowed base
member); the layout walk should move into `pycc_hir` with `pycc_mir`
delegating, so the gate and the lowering cannot drift; and the resulting
narrowing of `tests/issue_915_super_class_attr.rs` must be owned deliberately
rather than worked around.

**Contention with the concurrent actor.** PR **#971** (`#944`, enum-call
`C0001` span) opened while this iteration was implementing, and it claims
decision number **D-233** and the `docs/sessions/2026-09-06-08-` slot. This
iteration therefore took **D-234** and `-09-`. #971 also edits
`crates/pycc_hir/src/class.rs`, `crates/pycc_hir/src/lib.rs`,
`docs/ROADMAP.md` and `docs/TYPE_SYSTEM.md`; whichever PR lands second will
need a `git merge origin/main` over those four files.

## What this change is

Under D-154 an instance is a flat array of attribute slots and every method is
lowered exactly **once**, against its own class's layout. A derived class's
layout is computed most-base-first over its MRO, one slot per unique attribute
name. Those two facts are only compatible while every class in the MRO sees
its own attributes at the same slot indices it was lowered against — that is,
while every ancestor's layout is a **prefix** of the derived layout.

It was not. For `class A: self.a` / `class B: self.b` / `class C(A, B)` the
MRO is `[C, A, B]`, so the walk gives `b` slot 0 and `a` slot 1, while `A`'s
own methods still read slot 0. The issue's own program printed `3` twice where
CPython prints `7` then `3`. The failure is not uniformly loud: the slot the
running constructor never wrote aborts in `pycc_rt`
(`invalid encoded int word 0x0`, the exit-101 boundary), while the slot it
aliased over returns a silently wrong value.

The fix is a rejection, not a redesign (**D-234**). `pycc_hir`'s new
`validate_mro_slot_layout` runs at the end of `lower_class`, on the finished
`HirClassDef`, and emits `C0001` when a class has more than one base and some
class in its MRO has a flat layout whose attribute-**name sequence** is not a
prefix of the derived class's. The layout walk itself moved out of `pycc_mir`
into `pycc_hir::flat_attr_layout`, taking an already-resolved
`&[&HirClassDef]`; `pycc_mir::class::mro_attrs` keeps its signature, its
MRO-def collection loop and its ghost-class panic path, and delegates the walk,
so the gate and the lowering cannot drift on what counts as a slot.

Three things make the naive predicate the right one:

1. **No cheaper fix exists.** Whenever two classes in one MRO have layouts
   that agree on slots `0..k` and then name different attributes at slot `k`,
   no single flat ordering can put both names at index `k`. Reordering
   `mro_attrs` is impossible, not merely expensive.
2. **Reachability refinement is unsound.** `super()` re-enters a member the
   derived class shadows: with `class B: self.n; def f` / `class C: self.X` /
   `class D(B, C)` overriding both, `D.f` calls `super().f()` and `B.f` reads
   a mis-addressed slot. Pinned in `tests/issue_969_mi_slot_layout.rs`.
3. **Name-only comparison is sufficient** because D-210's `T0052` already
   rejects two MRO classes declaring the same attribute name with differing
   types. That dependency is written into `validate_mro_slot_layout`'s doc
   comment and into D-234.

Accepted shapes are unchanged and pinned end to end: single inheritance of any
depth, a methods-only mixin or class-attribute-only second base, two bases
declaring the *same* attribute names (`tests/fixtures/pep_3135_super.py`'s
`class Mixed(Slow, Fast)`), a builtin exception base, a diamond with
attribute-free intermediates, and a one-sided diamond where only one branch
adds an attribute.

## Deviations from the plan

- **Three tests flipped, not two.** The dispatch brief said the whole-tree
  sweep had found exactly two. Prototyping the gate and running
  `cargo test --workspace --no-fail-fast` found a third:
  `crates/pycc_types/src/tests.rs`'s in-crate mirror of the #915 shape, which
  flips because `check_source` runs `pycc_hir::lower_checked` first.
- **That third test was narrowed rather than flipped to a rejection.** Turning
  it into an `expect_err` would have dropped `pycc_types`' own coverage of the
  `super()` class-attribute path it exists to pin. Instead its program drops
  `B`'s instance attribute, which leaves `B`'s layout empty — a prefix of
  everything — and exercises the identical resolution path. The rejected shape
  is pinned in `tests/issue_969_mi_slot_layout.rs`.
- **The plan's second "deliberate narrowing" was wrong and was corrected in
  review round 2.** A diamond is rejected only when **both** branches add an
  instance attribute of their own; a one-sided diamond is accepted. A test now
  pins each side.
- **The plan's claim that enum classes carry no slots was wrong.** They carry
  two (`value`, `name`); they are irrelevant to this gate only because #941's
  `validate_bases` rejects using an enum as a base at all. That dependency is
  recorded in D-234 next to the `T0052` one.
- **D-234 and session `-09-`, not D-233 and `-08-`** — see the contention note
  above.

## Known follow-ups

- **The redesign that would lift this restriction** — per-derived-class
  re-lowering of inherited methods, or a runtime name-to-slot indirection.
  This is an architectural change to D-154 spanning `pycc_mir`,
  `pycc_codegen` and `pycc_rt`. D-234 records it as the tracked path and
  foreclosed it for now; no separate issue was filed, because the direction is
  not yet concrete enough to scope and #885 (class-model parent) already holds
  the surface.
- Open v0.4 candidates untouched by this iteration: #913, #958, #908, #965,
  #944 (in flight as PR #971), #954, #952, and the #885 parent. #336 stays
  excluded.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #966 closed by PR #970 (`50a0dc2a`).
- This iteration: #969 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

Read `validate_mro_slot_layout`'s doc comment in
`crates/pycc_hir/src/class/mro.rs` first. It states the two things the gate
rests on that are not visible from the code: the name-only comparison is sound
only because D-210's `T0052` guarantees name-equality implies type-equality
across an MRO, and checking each ancestor against the *derived* layout alone is
equivalent to checking every pair of ancestors, because two sequences that are
each a prefix of one common sequence are prefix-comparable.

The placement is forced, not incidental: the gate runs immediately before
`lower_class`'s `Ok(...)` because `attrs` is only complete after the
`@dataclass` field merge assigns it, and `validate_bases` runs long before any
attribute is known. Moving it earlier silently stops seeing dataclass fields.

`tests/issue_969_mi_slot_layout.rs` is the map — ten end-to-end tests split
into four rejected shapes and six accepted ones, with the accepted ones
asserting *stdout* rather than exit 0, because the defect being replaced was a
wrong value from a program that compiled fine. The in-crate unit tests for the
layout walk and the gate's own branches live in
`crates/pycc_hir/src/class/mro.rs`'s test module, because integration tests
under `tests/` do not score a crate's own regions (`docs/TESTING.md`).

If the gate ever needs relaxing, the two files that must move together are
D-234 and `docs/TYPE_SYSTEM.md`'s "Multiple inheritance and the flat slot
layout" section, plus the two flipped tests in
`tests/issue_915_super_class_attr.rs` and
`tests/issue_966_inherited_init_rank.rs`, whose comments name the narrowing
explicitly.
