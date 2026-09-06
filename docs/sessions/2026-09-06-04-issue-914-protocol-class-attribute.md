# 2026-09-06 — #914: a class attribute satisfies a `Protocol` attribute member

## Previous checkpoint's outcome

Iteration 18 delivered [#916](https://github.com/rotnov/pycc/issues/916)
(`Final[X]` on a class-level attribute): PR
[#959](https://github.com/rotnov/pycc/pull/959) merged by squash as
`f77544d8d8d7edadf057af2752e45fb77d69a333` and #916 is CLOSED. CI on that
pull request was green on the first try.

Post-merge `main` runs for `f77544d8` were all `completed`/`success`,
re-queried immediately before this snapshot was committed: CI
[34013629258](https://github.com/rotnov/pycc/actions/runs/34013629258), Main
history audit
[34013629270](https://github.com/rotnov/pycc/actions/runs/34013629270),
Status page freshness
[34013629262](https://github.com/rotnov/pycc/actions/runs/34013629262), and
Source link check
[34015820753](https://github.com/rotnov/pycc/actions/runs/34015820753). No
Pages run is listed for that commit. (A CI failure confined to the nbody
benchmark would have been the known
[#641](https://github.com/rotnov/pycc/issues/641); none occurred.)

Selection note: the advisor round **confirmed** #914 for this iteration. It
is two small seams — `check_protocol_conformance` in `pycc_types` and
`eval_isinstance_protocol` in `pycc_mir` — and the MRO walk over
`class_attrs` it needs, `lookup_class_attr_through_mro`, already exists from
#911. The alternatives were weighed and deferred:
[#944](https://github.com/rotnov/pycc/issues/944) has no cheap span fix
(`HirExpr::Call` carries no span and `resolve_instantiation` takes none, so
it needs the #943 AST pre-scan);
[#908](https://github.com/rotnov/pycc/issues/908) needs new MIR
enum-identity lowering; and
[#913](https://github.com/rotnov/pycc/issues/913) is **not** a three-liner —
`reject_class_attr_collisions` runs at `crates/pycc_hir/src/class.rs:1161`
*before* the dataclass field merge at ~1199-1256, so a rerouted `ClassVar`
needs a collision check after the merge. #913 is the recommended next pick
with that ordering seam named.

Exclusion recorded: [#336](https://github.com/rotnov/pycc/issues/336) (P3,
v0.4, `status.json` publishing) is neither maintainer-gated nor stale, but it
spans four independent seams (the schema, writers in both `next-milestone`
and `issue-select` with Codex + Claude parity, a privileged Pages publish job
under the D-172 guard, and the four-pin status-page rotation). That is the
multi-PR decomposition class, so it is deprioritized rather than taken.

## Overall status

Implemented #914 on `autopilot/iter-2026-09-06-19`, cut from `f77544d8`. One
pull request carrying `Fixes #914`; the orchestrating session watches CI and
merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 914, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #914
([issuecomment-5557320561](https://github.com/rotnov/pycc/issues/914#issuecomment-5557320561),
published against `f77544d8` after two adversarial review rounds); this
snapshot records where the implementation followed it and where it deviated.

## What the change is

A PEP 544 protocol attribute member (`limit: int`) was satisfied only by an
instance slot or a `@property`. #911 deliberately left
`check_protocol_conformance` untouched, so the #911 kind of class attribute —
a compile-time constant folded at every read — did not count, and the issue's
own program was rejected with `T0046 ... missing attribute`.

Two seams moved together, three lines each:

- `crates/pycc_types/src/class.rs` — the `ProtocolMember::Attribute` arm's
  `lookup_attr_through_mro` call now falls back to the existing
  `lookup_class_attr_through_mro` (#911's full-MRO walk over `class_attrs`).
  The fallback sits *inside* the existing `let ... else` producer, so a
  mismatched class attribute flows through the same `is_assignable` branch
  and yields the unchanged "attribute `x` has type ..., expected ..." wording.
- `crates/pycc_mir/src/class.rs` — `eval_isinstance_protocol`'s attribute arm
  gained a third disjunct over `class_attrs`. Without it `pycc check` accepts
  the class while `isinstance(c, HasLimit)` folds to `False`, which is exactly
  the #380 W2 consistency invariant the comment on `lookup_attr_through_mro`
  already names. Verified as a real divergence on the tree before the fix.

No MIR read-path change was needed: `crates/pycc_mir/src/expr.rs`'s
instance-attribute arm already folds a class-attribute read through
`fold_class_attr` before the slot lookup, and D-166 monomorphization binds the
protocol-typed parameter to the concrete class first. Verified empirically
rather than assumed.

Two resolutions recorded in `docs/TYPE_SYSTEM.md` alongside the change:

- **Write-through is out of scope.** pycc tracks no write through a
  protocol-typed receiver at all — for an instance attribute exactly as much
  as for a class attribute — so admitting class attributes widens nothing, and
  every direct write path to a class attribute stays `T0044`. The tracked
  follow-up is [#958](https://github.com/rotnov/pycc/issues/958).
- **The order of the two walks is load-bearing.** The instance/property walk
  runs first, and that is deliberate rather than cosmetic; see the deviation
  below.

## Deviations from the plan

- **The plan's own premise about collision-freedom was wrong, and the second
  review round caught it.** The draft asserted that the walk order was
  unobservable because `reject_class_attr_collisions` rejects a same-name
  split. It does — but only for the *declaring* class's own `class_attrs`
  against its own MRO. A class that declares none of its own performs a
  zero-iteration check, so two independent sibling bases can contribute an
  instance slot and a same-named class attribute to one derived class and
  never be compared (D-210's `check_incompatible_attribute_redeclarations`
  does not close it either — it compares `attrs`, never `class_attrs`). The
  published plan records the ordering as deliberate, and
  `protocol_conformance_prefers_a_sibling_base_instance_attribute` pins it so
  a later refactor cannot swap the two walks believing the order arbitrary.
- **That same shape exposed a pre-existing bug, now filed as
  [#960](https://github.com/rotnov/pycc/issues/960).** With no protocol
  involved at all, `class A` (instance `self.x = 1`) plus `class B`
  (`x: int = 2`) joined by `class C(A, B)` makes `pycc run` print `2` where
  CPython prints `1`: `fold_class_attr`'s MRO walk runs before the
  `mro_attrs` slot lookup. It pre-dates #914 and is untouched here.
- **An existing #911 test pinned the old limitation and had to be flipped.**
  `a_class_attribute_does_not_satisfy_a_protocol_attribute_member` in
  `tests/issue_911_class_attrs.rs` is now
  `a_class_attribute_satisfies_a_protocol_attribute_member`, asserting the
  program's stdout and pointing at the new file for the full surface. Its
  fixture was also missing `from typing import Protocol`, which is why it
  passed for the wrong reason; the replacement imports it.
- **The `isinstance` end-to-end fixture needed two locals.**
  `isinstance(C(), HasLimit)` is rejected with `C0001` — `isinstance` is a
  compile-time predicate and cannot evaluate a call as its first argument —
  so the fixture binds `c = C()` and `d = D()` first.
- Nothing else. Both seams, the in-crate unit tests in `pycc_types` and
  `pycc_mir`, the new end-to-end file, and the documentation sites followed
  the plan. `docs/ROADMAP.md` was not touched at all (no new feature
  paragraph, so the status-page four-pin rotation is not triggered — proved
  with `ruby scripts/check_status_page_freshness.rb origin/main`, exit 0).
  `docs/DIAGNOSTICS.md`'s `T0046` row does not enumerate attribute sources
  and stays accurate. No new `docs/decisions/` entry: this extends #911
  inside D-166's existing compile-time-only protocol model and reverses
  nothing.

The `or_else` closure's `None` path needed no new negative test — the
pre-existing `protocol_conformance_with_missing_attribute_is_t0046` already
sweeps it, confirmed against the `llvm-cov` region output (100.00% lines,
100.00% regions, 53916 regions with 0 missed).

## Known follow-ups

- [#960](https://github.com/rotnov/pycc/issues/960) — a sibling MRO base's
  class attribute silently shadows another base's instance slot; found and
  filed by this iteration's planning round, untouched.
- [#958](https://github.com/rotnov/pycc/issues/958) — attribute store through
  a Protocol-typed receiver panics in `pycc_mir`; the recorded out-of-scope
  boundary for this change.
- [#913](https://github.com/rotnov/pycc/issues/913) — the last remaining #911
  follow-up, and the recommended next pick; see the ordering seam in the
  selection note above.
- [#908](https://github.com/rotnov/pycc/issues/908) — enum equality; needs MIR
  and codegen identity lowering.
- [#944](https://github.com/rotnov/pycc/issues/944),
  [#954](https://github.com/rotnov/pycc/issues/954),
  [#952](https://github.com/rotnov/pycc/issues/952) — untouched, for
  `issue-select` to weigh.
- [#885](https://github.com/rotnov/pycc/issues/885) — the parent, which stays
  open until #913 closes.
- [#336](https://github.com/rotnov/pycc/issues/336) — excluded this
  iteration; see the exclusion note above.
- [#877](https://github.com/rotnov/pycc/issues/877) — every `pycc_types`
  diagnostic still renders at `:1:1` (D-043's placeholder span), which is why
  the new end-to-end assertions match on the code plus a message substring
  rather than on a rendered span.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #916 closed by PR #959 (`f77544d8`).
- This iteration: #914 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

The production change is six lines across two files, and neither is where the
interesting part lives. Read `lookup_class_attr_through_mro`'s doc comment in
`crates/pycc_types/src/class.rs` first — it explains why the class-attribute
walk is deliberately separate from the instance-attribute walk rather than
merged into it, which is the reason this fix is a fallback and not a widened
lookup.

Anyone touching the attribute arm again should read the comment above the
`or_else`: the order of the two walks is load-bearing for the sibling-base
diamond, and `protocol_conformance_prefers_a_sibling_base_instance_attribute`
fails if it is swapped. The same diamond is the reproduction in #960, whose
fix belongs in `pycc_hir`'s collision check or `pycc_mir`'s fold ordering, not
here.

The standing invariant to keep in mind for any further protocol-member work:
`pycc_types::class::check_protocol_conformance` and
`pycc_mir::class::eval_isinstance_protocol` must agree about what satisfies a
member (#380 W2). They are in different crates with different tables, and
nothing but tests couples them.
