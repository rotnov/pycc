# 2026-09-06 — #915: `super().CLASS_CONST` reaches a base class's class attribute

## Previous checkpoint's outcome

Iteration 16 delivered [#949](https://github.com/rotnov/pycc/issues/949) (a
`T0022` on a *declared* return no longer calls the function a "private
helper"): PR [#956](https://github.com/rotnov/pycc/pull/956) merged by squash
as `678486d1b789e53ca67daa08043c0cbb40a84e81` and #949 is CLOSED. CI on that
pull request was green on the first try.

Post-merge `main` runs for `678486d1` were all `completed`/`success`, each
re-queried immediately before this snapshot was committed: CI
[34004127080](https://github.com/rotnov/pycc/actions/runs/34004127080), Main
history audit
[34004127057](https://github.com/rotnov/pycc/actions/runs/34004127057),
Status page freshness
[34004127097](https://github.com/rotnov/pycc/actions/runs/34004127097), and
Pages [34004127054](https://github.com/rotnov/pycc/actions/runs/34004127054).
The earlier CI run
[34001147750](https://github.com/rotnov/pycc/actions/runs/34001147750) on
`923f9886` had also concluded `success`. (A CI failure confined to the nbody
benchmark would have been the known
[#641](https://github.com/rotnov/pycc/issues/641); none occurred.)

Selection note: the advisor round came back clean on #915 as the smallest
sound pick among the [#911](https://github.com/rotnov/pycc/issues/911)
follow-ups. Runners-up and why they were not taken:
[#914](https://github.com/rotnov/pycc/issues/914) is slightly smaller in code
but carries a mypy-divergence decision (whether a `ClassVar`/class constant
satisfies a *mutable* protocol attribute member);
[#916](https://github.com/rotnov/pycc/issues/916) is a documented scope
boundary plus an open `Final`-semantics question; and
[#908](https://github.com/rotnov/pycc/issues/908) needs MIR/codegen identity
comparison.

## Overall status

Implemented #915 on `autopilot/iter-2026-09-06-17`, cut from `678486d1`. One
pull request carrying `Fixes #915`; the orchestrating session watches CI and
merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 915, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #915
([issuecomment-5556411877](https://github.com/rotnov/pycc/issues/915#issuecomment-5556411877),
published against `678486d1` after three adversarial review rounds); this
snapshot records where the implementation followed it and where it deviated.

## What the change is

A class attribute (#911) is a compile-time constant with no instance slot,
folded to its literal at MIR-lowering time. `super().X` did not reach one:
`pycc_types::class::resolve_super_attr_get` resolved a base class `@property`,
rejected an instance attribute with `T0047` (#587), and otherwise returned
`T0044`; `pycc_mir`'s `Super` `AttrGet` branch handled properties and then
panicked. But a class attribute *is* a genuine entry in its class's
`__dict__`, so a CPython `super` object proxies it exactly as it proxies a
property.

The fix walks the post-current MRO slice (`mro[current_pos + 1..]`, #433 /
D-160) in both crates. The MIR side reuses the existing `fold_class_attr`
helper, so `super().X` produces MIR *identical* to `Base.X` and
`pycc_codegen` is unchanged.

Three things the issue does not mention were found empirically and are part
of the change:

- **A false rejection is corrected.** The class-level walk runs before the
  `T0047` instance-attribute rejection, and that ordering is deliberately
  non-positional: an instance attribute is in no class `__dict__` at all, so
  a `super` object never sees one. For `class B: X: int = 1` / `class C:` (a
  sibling base setting `self.X`) / `class D(B, C)`, pycc rejected
  `super().X` with `T0047` where CPython prints `1`. Nothing in `pycc_hir`'s
  `reject_class_attr_collisions` or `T0052`'s diamond check compares two
  independent sibling bases' tables, so the shape is reachable.
- **MRO position decides, not member kind.** The first implementation kept
  the properties loop and the class-attribute loop separate, each scanning
  the whole slice. The pinned reviewer caught that a `@property` on a *later*
  MRO entry then outranked a class attribute on an *earlier* one:
  `class D(B, C)` with `B.X = 1` and a `C.X` property yielded `99` where
  CPython yields `1`. The two walks are now one pass per class, matching a
  `super` object's own one-`__dict__`-at-a-time resolution. Both orderings
  are pinned by end-to-end tests.
- **A receiver-less `super()` is now `C0001` instead of aborting the
  compiler.** `pycc_mir`'s `Super` arms unconditionally compute a `self`
  receiver. Verified on `678486d1`: `super().m()` in a `@classmethod`,
  `super().p` in a `@staticmethod`, and `super().p` in a `@classmethod` all
  pass `pycc check` with exit 0 and then abort with
  ``pycc_mir: internal error: `self` has no recorded type`` at
  `crates/pycc_mir/src/lib.rs:1085`. The one member of that family that was
  *not* an abort was a `@classmethod` reading a base class attribute — a
  clean `T0044` only because the arm this issue adds did not exist. Without a
  guard, #915 would have recruited it into the abort class. Both
  `resolve_super_attr_get` and `resolve_super_method_call` now reject the
  receiver-less form with `C0001` when `env.binding_state("self").is_none()`,
  which converts three pre-existing compiler aborts into clean diagnostics as
  well. The guard is sound because `pycc_hir` requires a regular method's
  first parameter to be literally named `self`
  (`crates/pycc_hir/src/class.rs:1561-1582`), a `@classmethod` binds `cls`,
  and a `@staticmethod` binds no receiver.

**User-visible diagnostic change:** a `@classmethod` reading a base class
attribute through `super()` moves from `T0044` to `C0001`. That is recorded
in the pull-request body's before/after.

## Deviations from the plan

- The plan's §5 listed "one pass per class instead of member-kind-by-member-
  kind" as a **known gap, out of scope**, on the reading that the divergence
  was pre-existing. That was wrong in this pull request's context: before
  #915 the competing shape was rejected outright with `T0044`, so shipping
  the class-attribute arm alone would have turned a rejection into a *wrong
  value*. The pinned reviewer caught it; the loops were merged and the §5
  bullet no longer applies.
- The plan did not mention the D-176 conformance-breadth manifest. The new
  capability is now declared `proven` on the PEP 3135 row, with two scenarios
  added to `tests/fixtures/pep_3135_super.py` (base-value read under an
  override, and the earlier-class-attribute-versus-later-property diamond).
  The byte-for-byte oracle test is `#[ignore]`d locally because the pinned
  oracle is CPython 3.14.7 and this machine has 3.14.6; the fixture's output
  was compared against local CPython by hand and is identical (`6 4 4 8 4 2
  60 100 50 10 3 1`). CI runs the pinned comparison.
- `docs/TYPE_SYSTEM.md`'s accepted-read-forms bullet (line 213) was added to
  the docs work items during review — the plan named only the limitation list
  and the #433 paragraph.

Everything else followed the plan: the `pycc_types` arm and its ordering
comment, the `pycc_mir` fold and its new panic text, the
`#[should_panic]` substring update, the flipped #911 pinning test, the new
end-to-end file, in-crate unit tests in both crates in each crate's own
fixture style, `docs/ROADMAP.md`'s #433 paragraph prose-edited in place (no
new feature paragraph, so the status-page rotation is not triggered — proved
with `ruby scripts/check_status_page_freshness.rb origin/main`), and the
`T0047` long-form explanation in `crates/pycc_diag/src/explain.rs`. No new
`docs/decisions/` entry: this extends #911's already-accepted model rather
than reversing anything.

## Known follow-ups

- [#914](https://github.com/rotnov/pycc/issues/914),
  [#916](https://github.com/rotnov/pycc/issues/916),
  [#913](https://github.com/rotnov/pycc/issues/913) — the remaining #911
  follow-ups, untouched here.
- [#908](https://github.com/rotnov/pycc/issues/908) — enum equality; needs
  MIR/codegen identity lowering.
- [#944](https://github.com/rotnov/pycc/issues/944),
  [#954](https://github.com/rotnov/pycc/issues/954),
  [#952](https://github.com/rotnov/pycc/issues/952) — untouched, for
  `issue-select` to weigh.
- [#877](https://github.com/rotnov/pycc/issues/877) — every `pycc_types`
  diagnostic still renders at `:1:1` (D-043's placeholder span). The new
  `C0001` inherits it.
- **`T0044`'s wording through `super()`** still renders "class `Derived` has
  no attribute named `X`", naming the current class although the search began
  *after* it. The plan's own "class attribute declared only on the current
  class" test is exactly such a case (CPython says
  `AttributeError: 'super' object has no attribute 'X'`). Fixing it properly
  means a `docs/DIAGNOSTICS.md` pass across every `t0044_unknown_member` call
  site. Worth its own issue.
- **`docs/decisions/D-224-restrict-class-level-attributes-to-scalar.md`**
  lines 51-59 name `super().CLASS_CONST` as one of four tracked follow-ups;
  one of the four is now shipped. **`D-160`**'s lowering-shape enumeration
  (lines 70-72) is likewise further incomplete. `AGENTS.md` forbids
  rewriting an accepted decision in place, and neither warrants a superseding
  entry on its own — fold both corrections into whichever decision closes the
  remaining gaps.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #949 closed by PR #956 (`678486d1`).
- This iteration: #915 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

The whole production change is three places: the merged member-kind walk and
the `C0001` receiver guard in `resolve_super_attr_get` /
`resolve_super_method_call` (`crates/pycc_types/src/class.rs`), and the
mirrored walk in the `Super` `AttrGet` branch of
`crates/pycc_mir/src/expr.rs`. The comment on the `pycc_types` loop is the
authoritative statement of *why* the two orderings differ (one positional,
one not); the `pycc_mir` comment points back at it rather than restating it,
so read the `pycc_types` one first.

Anyone extending `super()` to another class-level member kind should add it
*inside* the existing per-class loop, not as a third slice-wide pass — that
is precisely the defect the review caught. And the `panic!` at the end of the
`pycc_mir` branch enumerates the member kinds the type checker guarantees;
its text is pinned by `#[should_panic]` in
`crates/pycc_mir/src/tests/class_super.rs`, so the two move together.

The receiver guard's one approximation is documented in its own comment: a
`@classmethod`/`@staticmethod` that itself declares a non-receiver parameter
named `self` passes it. Closing that would mean threading a method kind
through `pycc_hir`'s `lower_expr`/`lower_body`, which carry only
`class_name: Option<&str>` today — 15 `lower_body` call sites plus every
recursive `lower_expr` site, a larger change than this issue.
