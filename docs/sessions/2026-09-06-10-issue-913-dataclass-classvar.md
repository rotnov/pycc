# 2026-09-06 — #913: `ClassVar` inside a `@dataclass` body

## Previous checkpoint's outcome

Iteration 22 (`docs/sessions/2026-09-06-09-issue-969-mi-slot-layout-gate.md`)
delivered #969 as PR #973, squash-merged to `main` as
`00b0f5a023668a996cafdb09c504cb7330390ccb`, closing #969 and landing
**D-234**. CI needed one `--failed` rerun for the known nbody perf flake
(#641) on `macos-15-intel`; everything else was green on the first try.

While that iteration was in flight the concurrent actor merged the website
PR #972 (`5f942512`), which turned iteration 22's branch STALE; it merged
`origin/main` once, with no conflicts, before pushing. The concurrent actor's
PR **#971** (`#944`, **D-233**, session `-08-`) was open and DIRTY at that
point and is still open now — hence this iteration takes **D-235** and the
`-10-` slot, and `-08-` stays reserved for #971.

Post-merge runs on `00b0f5a0`, re-resolved immediately before committing this
entry:

- CI `34032584932` — success.
- Main history audit `34032584993` — success.
- Status page freshness `34032584923` — success.
- Pages `34032584927` — success.

On the preceding `50a0dc2a` (PR #970, #966): CI `34026612613` success, Main
history audit `34026612550` success, Status page freshness `34026612616`
success, Pages `34026612629` success.

## Overall status

`autopilot/iter-2026-09-06-23` implements #913 and is pushed with a PR open.
Nothing is merged by this session.

Selection note: the advisor CONFIRMED #913 as the smallest sound v0.4 pick,
over #798 (8 signatures across 5 files), #932 (undecided variants, ~19 call
sites), #954 (3+ separate defects), #908 (3 crates including `pycc_codegen`),
and #927/#952/#606/#903/#893/#768. #944 was excluded because it is already in
flight as the concurrent actor's PR #971. The advisor also *enlarged* the
scope before any code was written: it added check **A** (a `ClassVar` named
after a synthesized dunder) and check **B** (a dataclass field colliding with
a `ClassVar` anywhere in the MRO), and accepted rejection **C** (the
value-less `ClassVar`, which #911's existing class-attribute path already
rejects) as sufficient rather than requiring a bespoke diagnostic.

## What this change is

PEP 557 says a `ClassVar`-annotated name in a `@dataclass` body is *not* a
field: it stays an ordinary class attribute and is excluded from the
synthesized `__init__`, `__eq__` and `__repr__`. #911 taught pycc class-level
attributes but explicitly rejected `ClassVar` inside a `@dataclass` body with
`C0001`, because nothing modelled that exclusion.

The positive change is one routing decision. In
`crates/pycc_hir/src/class/body.rs`, the dataclass branch no longer rejects a
`ClassVar` annotation — it calls the same `lower_class_attr` the non-dataclass
branch uses and `continue`s, so the name never reaches `merged_fields`.
Because `crates/pycc_hir/src/class.rs` sets `attrs = merged_fields.clone()`,
that single list is *both* the synthesis source for the three dunders and the
D-154 flat slot layout, so excluding the name from it excludes it from all
four places at once. No `pycc_types`, `pycc_mir`, `pycc_codegen` or `pycc_rt`
change was needed; the read side is #911's existing constant fold, which
already folds a class attribute to its literal at every read site.

Four shapes are rejected with `C0001` (**D-235**):

1. A `ClassVar` named `__init__`, `__eq__` or `__repr__` in a `@dataclass`
   body (widened to six names in the review round below).
   CPython keeps the class attribute and skips synthesizing that method;
   pycc's existing collision check (`reject_class_attr_collisions`) runs
   *before* synthesis, so without this guard the `ClassVar` and the
   synthesized method would both exist. Checked in `body.rs`, at the point the
   `ClassVar` is routed.
2. A dataclass field that shares its name with a `ClassVar` **in the same
   body** (either declaration order).
3. The same collision against a `ClassVar` declared in a **base** class.
4. The same collision across **sibling bases** — one base contributes the
   field, another the `ClassVar`.

Shapes 2-4 are one unified post-merge check in `class.rs`, run over
`merged_fields` immediately before `attrs` is assigned, with the
`class_attr_owner` helper walking the class's own `class_attrs` and then the
MRO. They are one check because they are one CPython behaviour: verified
against the pinned oracle, CPython either *drops* the field (making the
`ClassVar` win) or gives the field the class attribute's value as a
**default** — and dataclass field defaults are not supported in this version,
so neither outcome is representable. Silently picking one would violate D-198.

Shape 4 is a deliberate narrowing: `class D(B, A)` where `A` holds the
`ClassVar` and `B` the field is a program CPython runs and pycc could compile
correctly, and it escapes #969's MI slot-layout gate precisely because a
`ClassVar`-only class declares no instance attributes at all. D-235 records
that, following D-234's precedent of preferring a loud conservative rejection
over an under-specified acceptance.

Diagnostic precedence is four deep and pinned in both test suites: `T0052`
(dataclass field type conflict across the MRO) outranks the new collision
check, which outranks the dunder guard, which outranks #911's scalar-only
`C0001`.

## Deviations from the plan

- **The brief named a golden fixture pair `911_classvar_dataclass`.** No such
  file exists — that string is a `ScratchDir::new(tag)` label inside the Rust
  test, not a fixture. Only the Rust test
  (`class_var_in_a_dataclass_body_is_rejected` in
  `tests/issue_911_class_attrs.rs`) and its in-crate mirror were removed.
- **The brief's premise that `P.LIMIT` resolves through
  `lookup_class_attr_through_mro` was wrong.** Review round 1 found that
  `crates/pycc_types/src/expr.rs` handles a class-*name*-qualified read
  separately and never walks the MRO, so `Derived.LIMIT` for a `ClassVar`
  inherited from a base is `T0044` while `instance.LIMIT` works. That
  asymmetry predates this change; it is pinned by a test here and filed as
  follow-up **#974** rather than fixed in scope.
- **The dunder guard (shape 1) was not in the plan's first draft.** Review
  round 1 found that `reject_reserved_class_attr_name` blocks only
  `__slots__`, and that the collision check runs before dataclass synthesis,
  so a `ClassVar` named `__repr__` would have evaded both.
- **Shapes 2-4 were originally two checks in `body.rs`.** Review round 1's
  cross-base/diamond finding forced the redesign into one post-merge check in
  `class.rs`, which is the only place the merged field list and the full MRO
  are both available.
- **The plan's round-3 claim that no decision entry was needed was wrong.**
  D-234 is the direct precedent — a conservative narrowing that also merely
  *follows* D-224 and still got its own entry. **D-235** was written.
- **The deep review returned three note-level findings and no blocker.** One
  was a stale test doc comment (fixed here); one became **#975**; the third
  asked for an in-crate case where a dataclass-body `ClassVar` annotation
  targets something other than a bare name. That third one was considered and
  not acted on: the shape already falls through to the existing "a dataclass
  field annotation must target a bare name" rejection two blocks below, and
  the D-014 region gate reports 100% without it, so there is no uncovered
  region to close.

## Known follow-ups

- **#974** (filed by this iteration) — a class-name-qualified read of an
  *inherited* class attribute is `T0044`. Milestone v0.4.
- **#975** (filed by this iteration, from the deep review of this change) —
  the same synthesized-dunder hazard the new guard closes for `@dataclass`
  bodies is still open on the **non-dataclass** path: `__init__:
  ClassVar[int] = 8` in a plain class body coexists with the `__init__`
  `ensure_init` synthesizes. Pre-existing since #911 + #912/D-225.
  Milestone v0.4.
- `docs/ROADMAP.md` and `docs/TYPE_SYSTEM.md` still list `__post_init__` and
  `InitVar` as the remaining unsupported PEP 557 surface; the conformance
  manifest's PEP 557 `not_proven` entry was narrowed to exactly those two.
- Open v0.4 candidates untouched by this iteration: #908, #952, #954, #927,
  #932, #798, #768, #958, #965, #944 (in flight as PR #971), and the #885
  parent. #336 stays excluded.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #969 closed by PR #973 (`00b0f5a0`).
- This iteration: #913 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

`tests/issue_913_dataclass_classvar.rs` is the map — twelve end-to-end tests
through the public CLI, split into the accepted surface (both read forms, a
derived class overriding a base's `ClassVar`) and the rejected shapes, with
the accepted ones asserting *stdout* so the `__repr__` and `__eq__` exclusions
are actually observed rather than merely compiling. The in-crate unit tests
live in `crates/pycc_hir/src/tests.rs`, because integration tests under
`tests/` do not score a crate's own regions (`docs/TESTING.md`); the
sibling-base test there uses `D(B, A)` specifically to exercise
`class_attr_owner`'s "keep walking the MRO" arm.

`tests/fixtures/pep_0557_dataclasses.py` now carries a `Bounded(Point)` class
with two `ClassVar`s, verified byte-for-byte identical between CPython 3.14.6
and the pycc-built binary. That fixture is the conformance evidence behind the
manifest's new PEP 557 `proven` row; changing the dataclass synthesis order or
`__repr__` formatting will show up there first.

If the collision check ever needs relaxing, the three things that must move
together are D-235, `docs/TYPE_SYSTEM.md`'s
"`ClassVar` in a `@dataclass` body" subsection, and the `class_attr_owner`
call site in `crates/pycc_hir/src/class.rs` — the check is placed immediately
before `attrs = merged_fields.clone()` for the same reason #969's gate sits at
the end of `lower_class`: `merged_fields` is only complete at that point.
Relaxing shape 2-4 at all requires dataclass field *defaults* first, which
nothing in v0.4 supports.

## Review round (PR #976, codex-bot threads)

Three unresolved `chatgpt-codex-connector` P1 threads were addressed on the
same branch before merge; all three are now fixed and resolved.

1. **The dunder guard was too narrow.** The bot claimed `__ne__` and
   `__str__` escape it. Verified against CPython 3.13.9 versus
   `./target/debug/pycc run`, and a third name turned up that the bot did not
   name: `__format__`. All three make pycc succeed silently where CPython
   raises `TypeError: 'int' object is not callable`, because `pycc_mir`
   rewrites `!=` through the synthesized `__eq__` and both `print(instance)`
   and f-string interpolation through the synthesized `__repr__`, never
   consulting the dunder CPython tries first. The guard is now
   `DATACLASS_IMPLICIT_DUNDERS` (`crates/pycc_hir/src/class/body.rs`), a
   six-name set generated by a stated rule rather than a list: *the three
   methods pycc synthesizes, plus every dunder CPython consults for an
   operation pycc rewrites through one of them.* Two boundary names were
   checked and deliberately left out: `__hash__` (`P.__hash__` reads `8`
   under both, because `dataclasses` leaves an explicitly bound one alone)
   and `__lt__` (already `T0021` "cannot compare" before MIR). `__slots__`
   keeps its own earlier rejection in `attrs.rs`.
   The same gap does **not** extend to the non-dataclass path tracked by
   [#975](https://github.com/rotnov/pycc/issues/975): both rewrites are
   `is_dataclass`-gated, a non-dataclass `!=` is `T0021`, and printing a
   non-dataclass instance is a pre-existing codegen limitation unrelated to
   `ClassVar`. No comment was added to #975, whose own three names are
   unaffected by this change.
2. **`class.rs` decomposition** (AGENTS.md "Keep source files
   decomposable"): the merged-field/`ClassVar` collision walk and
   `class_attr_owner` moved into `crates/pycc_hir/src/class/attrs.rs`, which
   already owns every other class-attribute collision rule, as
   `pub(super) fn reject_dataclass_field_class_var_collisions`. `lower_class`
   keeps a single call at the same position (immediately before
   `attrs = merged_fields.clone()`), so the note above about what must move
   together now points at `attrs.rs` rather than `class.rs`.
3. **`tests.rs` decomposition**: the #913 unit-test group moved out of the
   7k-line `crates/pycc_hir/src/tests.rs` into
   `crates/pycc_hir/src/tests/dataclass_class_vars.rs`, declared next to the
   existing `mod subscript_annotations;` and following the same
   `use super::*` convention. The dunder test now loops over all six names
   individually (a single `contains` hit is not evidence the whole set is
   honoured), and a new accepted-case test pins `__hash__`.
