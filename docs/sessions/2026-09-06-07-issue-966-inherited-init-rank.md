# 2026-09-06 — #966: an implicit `object`-style constructor now ranks last

## Previous checkpoint's outcome

Iteration 20 (`docs/sessions/2026-09-06-05-issue-960-sibling-base-class-attr.md`)
delivered #960 as PR #967, squash-merged to `main` as
`ad5b25346983831afa00b5d3f5f4c22fbdf43ee9`, closing #960. CI was green on the
first try. The plan went through three deep-reviewer rounds, and the iteration
filed #965 and #966 as the follow-ups its own scope deliberately excluded.

Post-merge runs on `ad5b2534`, re-resolved immediately before committing this
entry:

- CI `34020950245` — success.
- Main history audit `34020950235` — success.
- Status page freshness `34020950256` — success.
- `gh run list --commit ad5b2534…` reports exactly those three workflows for
  that commit; no Pages run was triggered by it.

On the preceding `f29d245b` (PR #961, #914): CI `34017171535` success,
Status page freshness `34017171507` success, Main history audit `34017171527`
success.

While this iteration was in flight, `main` moved again: PR #968 merged as
`89501bbb140c4e7986ccd75f290bf1b974a2f055` (stdlib `import X as Y`, Part 1 of
#883), taking decision number **D-231** and session file
`2026-09-06-06-issue-962-stdlib-import-alias.md`. This iteration therefore took
**D-232** and `-07-`, and merged `origin/main` into its branch (never rebased,
never stashed). The merge had two textual conflicts, both trivial: the `mod`
declaration list in `crates/pycc_types/src/tests.rs` and the generated decision
table in `docs/decisions/README.md`. Both were resolved by keeping both sides.

## Overall status

`autopilot/iter-2026-09-06-21` implements #966 and is pushed with a PR open.
Nothing is merged by this session.

Selection note: the advisor CONFIRMED #966 as the right pick for this
iteration — it is the smallest remaining soundness defect (a runtime abort on
uninitialized slots) among the open v0.4 candidates. #913 needs two HIR seams
plus a conformance-manifest edit; #908 spans three crates including codegen
pointer equality; #958 is decision-bearing on monomorphization; #965 relaxes a
T0044 contract. The advisor also *refuted* the alias route this session first
considered (aliasing the derived class's constructor to the winning base's)
and surfaced that the fix necessarily flips D-189 rule 4 — both of which
reshaped the plan before any code was written.

## What this change is

D-225's `ensure_init` synthesizes an implicit zero-argument `__init__` into a
class's *own* method table when nothing in its MRO provides one, mirroring
CPython's inherited `object.__init__`. Every constructor walk in the compiler
took the first `__init__` it found along the MRO, so that stub out-ranked a
real constructor on a later base. For `class C(A, B)` with `A` init-less and
`B` declaring a real constructor, `C()` ran `A`'s empty stub, left `B`'s slots
uninitialized, and aborted in `pycc_rt` (`invalid encoded int word 0x0`, exit
101 via `pycc run` per D-072) on the first read; the checker separately
rejected `C(5)` with `T0021`. CPython ranks `object.__init__` last.

The fix is a *ranking* fix, not a walk fix. `HirClassDef.implicit_object_init`
records the provenance, set only at `ensure_init`'s own call site from
`ensure_init`'s own report — never from the constructor's shape, and never on
the `@dataclass` path, whose generated constructor is real. Five seams consult
it:

1. `pycc_mir` `Instantiate`'s `ctor` walk (`crates/pycc_mir/src/expr.rs`).
2. `pycc_mir`'s `super()` `MethodCall` arm, gated on `method == "__init__"`.
3. `pycc_types::class::binding::resolve_instantiation`.
4. `pycc_types::class::resolve_super_method_call`, same gate.
5. `pycc_types::exception::reject_own_constructor` (the D-189 rule-4 walk).

The first four layer as `find-unflagged` / `.or_else(find-including-flagged)` /
unchanged panic, so an all-implicit MRO still instantiates and the two pinned
`should_panic` tests (`crates/pycc_mir/src/tests/class_mro.rs`,
`crates/pycc_types/src/class/binding.rs`) keep their exact behaviour. The
fifth deliberately has **no** fallback arm: a fallback branch there would be
unreachable under the D-014 region gate, and `None` already routes to the same
`C0001` as a wrong-ancestor answer.

Two intended behaviour flips follow, both pinned in
`tests/issue_912_no_init_class.rs`:

- `class Base: pass` / `class MyError(Base, Exception): pass` /
  `raise MyError("boom")` is now **raisable** (traceback `MyError: boom`,
  exit 101) where it was `C0001`. This matches CPython.
- Binding that same class as a value (`m = MyError()`) is now `C0001`
  ("cannot instantiate exception class `MyError` as a value"), the same
  rejection every other raisable class already got.

D-232 records the decision and supersedes D-189 rule 4 as well as D-225's own
"stays `C0001`" consequence text.

## Deviations from the plan

- **`cargo fmt` was run, not just checked.** The plan's gate is
  `cargo fmt --all -- --check`; it failed twice (once on the new closure shapes
  in `expr.rs`/`binding.rs`, once after the merge on the reformatted dataclass
  test). Both times the fix was `cargo fmt --all` followed by a clean
  `--check`. No hand-formatting.
- **One extra test beyond the plan.** The deep reviewer's single (note-level)
  finding was that the dataclass-provenance regression test used a one-field
  dataclass, leaving the *zero*-field case — the one shape-identical to the
  implicit constructor — unpinned. The test in
  `crates/pycc_hir/src/class/init.rs` now loops over both field counts.
- **`docs/ROADMAP.md` got prose only.** Per the iteration brief, no new
  `**[#966](...) —` paragraph was added: that would trigger the status-page
  four-pin rotation. The `#432` paragraph gained a "Refined by #966" sentence
  block instead, and its now-false "#541 Part 2's exception rule is unchanged"
  claim was corrected. `ruby scripts/check_status_page_freshness.rb origin/main`
  reports "no roadmap milestone, evidence-checklist, or feature-landing-paragraph
  signal" — exit 0.
- **A merge, not a rebase.** See the `89501bbb` note above.

## Known follow-ups

- **#969 (filed this iteration, v0.4)** — MRO slot aliasing corrupts instance
  attributes when two or more bases carry slots. Found while implementing
  #966, **independent of it**, reproduced unchanged at `ad5b2534`. `mro_attrs`
  (`crates/pycc_mir/src/class.rs`) assigns the derived class's flat layout
  most-base-first, but each base's own `__init__` addresses slots against that
  base's own layout, so two slot-bearing bases overlap. Two observable shapes:
  a silently wrong value (pycc prints `3`/`3` where CPython prints `7`/`3`) and
  a runtime abort. Pinned as a known limitation by
  `tests/issue_966_inherited_init_rank.rs::the_multiple_slot_bearing_base_layout_is_a_known_aliasing_limitation`,
  which asserts **both** reads.
- **Deferred inside #966's own scope**: the generic `c.__init__()` method-call
  surface (`crates/pycc_types/src/class/method_call.rs`,
  `crates/pycc_mir/src/expr.rs`) is deliberately left unranked, which is why
  D-232's rule says "constructor resolution" and not "everywhere". Recorded in
  `docs/TYPE_SYSTEM.md` and in D-232's consequences.
- **#913** — two HIR seams plus a conformance-manifest edit.
- **#958** — decision-bearing on monomorphization.
- **#908** — three crates including codegen pointer equality.
- **#965** — the two class-attribute gates that ask "does it exist" where they
  should ask "does it win".
- **#944**, **#954**, **#952** — still open, untouched.
- **#885** — parent issue for the instance-layout family; #969 belongs to it.
- **#336** — excluded from v0.4 scope by the earlier selection round.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #960 closed by PR #967 (`ad5b2534`).
- This iteration: #966 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

Read the doc comment on `HirClassDef.implicit_object_init` in
`crates/pycc_hir/src/class.rs` first. It states the invariant the whole change
rests on: the flag is **provenance, not shape**, only `ensure_init`'s call site
sets it, and it names all five consumers. An explicitly written
`def __init__(self) -> None: pass` lowers to the identical empty body and must
keep ranking first — that is a test in
`tests/issue_966_inherited_init_rank.rs`, not an accident.

The four-versus-one asymmetry is the other thing to know before touching this:
four seams fall back to a flagged entry, `reject_own_constructor` does not, and
the reason is the D-014 region gate, not taste. Adding a fallback there adds an
unreachable region.

`tests/issue_966_inherited_init_rank.rs` is the map — nine end-to-end tests,
including the two guards that must not move (an explicit empty constructor, a
dataclass base) and the #969 limitation pin. In-crate unit tests for the
branches themselves live in `crates/pycc_mir/src/tests/class_init_rank.rs` and
`crates/pycc_types/src/tests/init_rank.rs`, because integration tests under
`tests/` do not score a crate's own regions (`docs/TESTING.md`).
