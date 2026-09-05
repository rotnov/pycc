# 2026-09-05 — #948: a self-referential protocol member return resolves to the protocol type

## Previous checkpoint's outcome

Iteration 13 delivered [#905](https://github.com/rotnov/pycc/issues/905)
(validate `if TYPE_CHECKING:` bodies for context violations): PR
[#950](https://github.com/rotnov/pycc/pull/950) merged by squash as
`a199029c6346ed6f5b2136d33d56216d11e32e1e` and #905 is CLOSED. CI on that
pull request was green on the first try.

Post-merge `main` runs for `a199029c` were all `completed`/`success`: CI
[33992498995](https://github.com/rotnov/pycc/actions/runs/33992498995), Main
history audit
[33992498896](https://github.com/rotnov/pycc/actions/runs/33992498896),
Status page freshness
[33992498929](https://github.com/rotnov/pycc/actions/runs/33992498929), and
Pages [33992498809](https://github.com/rotnov/pycc/actions/runs/33992498809)
(each re-queried immediately before this snapshot was committed).

## Overall status

Implemented [#948](https://github.com/rotnov/pycc/issues/948) on
`autopilot/iter-2026-09-05-14`, cut from `a199029c`, which was still
`origin/main` at push time (fetched immediately before the push; no merge was
needed and no other pull request was open at any point in this iteration).
One pull request carrying `Fixes #948`; the orchestrating session watches CI
and merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 948, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #948
([issuecomment-5555033272](https://github.com/rotnov/pycc/issues/948#issuecomment-5555033272),
published against `a199029c` after two adversarial review rounds); this
snapshot records where the implementation followed it and where it deviated.

## What the change is

A protocol member whose annotation names its own protocol
(`class P(Protocol): def clone(self) -> P: ...`) lowered to
`Ty::Instance("P")` rather than `Ty::Protocol("P")`, so every concrete class
implementing `clone` was rejected with a spurious `T0046` return-type
mismatch rendered at the module's first line.

**The issue's stated cause is wrong**, and the plan comment records the
correction. #947's `C0001` gate is not "after the protocol class is
registered and does not reach a self-referential member": `class.rs` pushes
the class's own `ClassAnnotationInfo` (with `is_protocol: true`) into
`class_name_defs` *before* dispatching to `lower_protocol_class`, and the
gate in `lower_return_annotation` is reached — it simply never saw a
`Ty::Protocol` to fire on. The real cause is **arm ordering** in
`pycc_hir::func::annotation_to_ty`: the PEP 649/749 self-reference arm and
the PEP 673 `Self` arm both ran above the general `Expr::Name` arm whose
`class_defs` lookup resolves a protocol name, so the protocol branch was
unreachable for the enclosing class itself.

Both arms now go through one new private helper,
`func::enclosing_class_ty(class_name, class_defs)`, which yields
`Ty::Protocol` when the enclosing class is a protocol and `Ty::Instance`
otherwise. #934/#947's existing `C0001` protocol-return gate then fires
unchanged — same code, same message, the annotation's own span. **No new
diagnostic string, no new diagnostic code, no new ADR.** That is the issue's
own second stated outcome; its first ("structural conformance accepts
`-> C`") is out of reach at this scale and was rejected on evidence:
`check_protocol_conformance` matches member signatures with plain
`is_assignable`, so even a cross-class `other: D` against a member
`other: Q` — no self-reference involved — is already `T0046` today.
Accepting `-> C` means building recursive structural signature matching from
scratch, D-166/#894-scale work.

`-> Self` was folded in deliberately rather than split off: it is the same
defect at the same seam. `from typing import Self` is itself `C0002` in this
version, but a bare unimported `Self` reaches the PEP 673 arm and produced
the identical spurious `T0046`; with no conforming class in the module it
compiled silently at exit 0. Splitting would have meant two pull requests
touching the same three lines for the same bug.

The parameter and attribute positions are not gated — `Ty::Protocol` is
supported there (D-166) — so they simply carry the protocol type now. A
protocol declaring `def same(self, other: P) -> bool` plus a concrete class
spelling the parameter the same way previously failed with the
self-contradictory `T0046: ... parameter 1 has type `P`, expected `P`` (one
side `Ty::Protocol("P")`, the other `Ty::Instance("P")`, both rendered as a
bare `P`); it now conforms, compiles and runs.

Two adjacent limitations were measured, are **unchanged by this work**, and
are recorded in `docs/TYPE_SYSTEM.md` and the `#380` `docs/ROADMAP.md`
paragraph rather than fixed: a conforming self-referential *parameter*
member still cannot be **called** through a protocol-typed receiver with a
concrete argument (`p.same(C())` is `T0021`, since `check_call_args` uses
plain assignability rather than the environment-aware conformance path), and
a self-referential *attribute* member is **unsatisfiable outright** — every
path by which a concrete class establishes an instance-attribute slot
restricts it to a scalar, so `self.nxt = C()` is `C0001` before conformance
is consulted, exactly as the existing container-typed-protocol-attribute
carve-out already records.

Tests: five rewritten/added unit tests in
`crates/pycc_hir/src/class/protocol_return_tests.rs` pinning the lowered
`Ty` for the return (both spellings, now `C0001`), parameter, and attribute
positions plus the non-protocol `Ty::Instance` branch of the new helper; a
byte-exact fixture pair
`tests/diagnostics/c0001_protocol_self_return.{py,expected.txt}` registered
in `tests/diagnostics_test.rs`; and five end-to-end CLI tests in
`tests/issue_948_protocol_self_reference.rs` covering `check` and `build`
for both spellings (exit 1, never 101, no leftover binary, and an explicit
assertion that `T0046` is gone) plus the positive `pycc run` case. Docs:
`docs/TYPE_SYSTEM.md` (the Protocol row, the "Protocols and structural
typing" section, and the two "Current state" paragraphs that claimed `Self`
and a self-referential class name always resolve to `Ty::Instance`),
`docs/DIAGNOSTICS.md`'s `C0001` prose, and the `#380` paragraph in
`docs/ROADMAP.md` — prose-only, no new feature paragraph, so the status-page
four-pin rotation is not triggered (`check_status_page_freshness.rb
origin/main` reports no signal).

## Gates

All run from the worktree, all exit 0: `cargo fmt --all -- --check`;
`cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace` (4706 passed, 0 failed, 58 ignored, across 86 test
binaries); the CI coverage sequence
(`cargo build --target x86_64-apple-darwin -p pycc_rt`,
`cargo build --workspace`, `cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100`: TOTAL regions 53420/0 missed = 100.00%, functions 2455/0 missed =
100.00%, lines 35121/0 missed = 100.00%, and `crates/pycc_hir/src/func.rs`
itself 663 regions / 27 functions / 541 lines, all 100.00%);
`python3 -m unittest discover -s scripts -p 'test_*.py'`;
`check_roadmap_evidence.rb`; `check_status_page_freshness.rb origin/main`
(no signal); `check-site.sh`; `check_conformance_breadth.py` (39
evidence-backed rows); `check_readme_milestone_projection.rb`;
`generate_decisions_index.py --check`; `check_ci_permissions.rb` (10 files);
and `cargo doc --workspace --no-deps`. Nothing touched appears in
`tests/fixtures/policy-successor-manifest.json`.

The pinned D-068 reviewer (`ievo:deep-reviewer`) ran twice on the plan draft
and once on the committed range. On the plan it produced five accepted edits
across two rounds: a third `docs/TYPE_SYSTEM.md` edit site whose categorical
`Self`-resolves-to-`Ty::Instance` claim the fix falsifies; `DIAGNOSTICS.md`
promoted from contingent to required; the protocol-*attribute* call site in
`class/protocol.rs` added to the test plan; the shared test helper's
hardcoded `rfind("-> P:")`/one-character span flagged as unusable for the
four-character `Self` spelling and for the ungated attribute case; and the
D-130 sequence-number/re-fetch obligations spelled out. On the committed
range it produced two documentation findings, both confirmed against the
binary before being fixed in `c15b6984`: the attribute "spell it as the
protocol too" workaround does not exist (scalar-only slots), and the e2e
test's doc comment implied a callable self-referential parameter member when
`p.same(C())` is still `T0021`. It also noted the `-> Self` spelling had no
`build`-path test; that test was added. No correctness finding at any round.

## Deviations from the plan

- None material. The plan named `docs/DIAGNOSTICS.md` and the `#380`
  `docs/ROADMAP.md` paragraph as prose-only edits, and both stayed that way;
  the post-review corrections in `c15b6984` extended those same paragraphs
  rather than opening new ones.
- `cargo test --workspace` was run without `--include-ignored`: the local
  oracle is CPython 3.14.6 and the ignored conformance tests require the
  pinned 3.14.7. CI runs them.

## Known follow-ups

- **Non-structural protocol member matching** — `check_protocol_conformance`
  compares member parameter and return types with plain `is_assignable`, so
  a concrete class conforms only by spelling a protocol-typed member
  parameter as the protocol itself. Demonstrated with a cross-class case
  that has no self-reference in it at all (`other: D` against `other: Q`
  where `D` conforms to `Q` is `T0046`). Not yet filed as its own issue;
  file it before the next protocol-soundness iteration.
- **Calling a protocol member that takes a protocol-typed parameter** —
  `p.same(C())` is `T0021`, because `check_call_args`
  (`crates/pycc_types/src/class.rs`) uses plain assignability rather than
  the environment-aware `is_assignable_env` conformance path. Also unfiled.
- [#889](https://github.com/rotnov/pycc/issues/889) — string forward
  references (`-> "P"`) are unimplemented; explicitly out of scope here.
- [#894](https://github.com/rotnov/pycc/issues/894),
  [#944](https://github.com/rotnov/pycc/issues/944),
  [#949](https://github.com/rotnov/pycc/issues/949),
  [#932](https://github.com/rotnov/pycc/issues/932),
  [#798](https://github.com/rotnov/pycc/issues/798) — untouched here, for
  `issue-select` to weigh.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #905 closed by PR #950 (`a199029c`).
- This iteration: #948 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

`func::enclosing_class_ty` in `crates/pycc_hir/src/func.rs` is the whole of
the production change; its two callers are the `Self` arm and the
self-reference-by-name arm of `annotation_to_ty`, a few lines below it, and
every downstream behavior change flows from those. The three call sites that
observe it inside a protocol body are in
`crates/pycc_hir/src/class/protocol.rs` — method parameters, method returns
(the gated one), and attribute `AnnAssign` — and each is pinned by a unit
test in `crates/pycc_hir/src/class/protocol_return_tests.rs`, whose module
doc comment carries the reasoning for which positions are gated and why.

The two follow-ups above both live in `pycc_types`, not `pycc_hir`:
`check_protocol_conformance` and `check_call_args` in
`crates/pycc_types/src/class.rs`, plus `resolve_method_call`'s
protocol-dispatch branch in `crates/pycc_types/src/class/method_call.rs`.
Both come down to the same thing — those paths use plain `is_assignable`
where they would need the environment-aware `is_assignable_env`, or true
structural matching. That is the seam a future PEP 544 soundness iteration
starts from.
