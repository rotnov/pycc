# 2026-09-05 — #953: a conforming concrete argument is accepted for a protocol-typed method parameter

## Previous checkpoint's outcome

Iteration 14 delivered [#948](https://github.com/rotnov/pycc/issues/948) (a
self-referential protocol member return resolves to the protocol type): PR
[#951](https://github.com/rotnov/pycc/pull/951) merged by squash as
`0084258b333e63417475e2069a0d707a943cd9dd` and #948 is CLOSED. CI on that
pull request was green on the first try. Two follow-ups were filed from that
iteration's own measurements: [#952](https://github.com/rotnov/pycc/issues/952)
(non-structural protocol member matching) and
[#953](https://github.com/rotnov/pycc/issues/953), this iteration's subject.

Post-merge `main` runs for `0084258b` were all `completed`/`success`: CI
[33996635017](https://github.com/rotnov/pycc/actions/runs/33996635017), Main
history audit
[33996635025](https://github.com/rotnov/pycc/actions/runs/33996635025),
Status page freshness
[33996635073](https://github.com/rotnov/pycc/actions/runs/33996635073), and
Pages [33996635039](https://github.com/rotnov/pycc/actions/runs/33996635039)
(each re-queried immediately before this snapshot was committed).

Selection note: the advisor round on the candidate set was clean.
[#949](https://github.com/rotnov/pycc/issues/949) (T0022 wording) is a
smaller change but is polish; #953 was chosen on soundness grounds — it is a
spurious rejection of valid programs, which costs users more than a wording
improvement gains them.

## Overall status

Implemented #953 on `autopilot/iter-2026-09-05-15`, cut from `0084258b`.
One pull request carrying `Fixes #953`; the orchestrating session watches CI
and merges.

The issue and the open-PR list were re-checked before the first edit, at the
first commit, and before the push: state `OPEN` throughout, no open pull
request referencing 953, and the only comment on the issue is this session's
own plan comment. The plan is the `issue-to-plan` comment on #953
([issuecomment-5555496188](https://github.com/rotnov/pycc/issues/953#issuecomment-5555496188),
published against `0084258b` after two adversarial review rounds); this
snapshot records where the implementation followed it and where it deviated.

## What the change is

A protocol member taking a protocol-typed parameter could be *declared* and
*conformed to* (that is #948's fix) but not *called* with a conforming
concrete argument: `p.same(C())` and `c.same(C())`, where `same` takes
`other: P` and `C` conforms to `P`, were rejected with a spurious
``T0021: argument 1 of `same` expects `P`, got `C` ``. `check_call_args`
in `crates/pycc_types/src/class.rs` compared each argument against its
parameter with the plain nominal `is_assignable`, so structural conformance
was invisible — even though `crates/pycc_types/src/expr.rs` already used the
environment-aware `class::is_assignable_env` for the same argument passed to
a plain function. That asymmetry was the whole defect.

**Two seams, not one.** The front-end fix alone is not shippable, and this
was verified empirically rather than reasoned about. `monomorphize_protocol_params`
drops every `HirItem::Function` whose signature carries a protocol-typed
parameter and re-emits it only as a `0gen_…` specialization at each call
site it rewrites — and it rewrote `HirExpr::Call` only. With the predicate
changed and nothing else, the repro type-checks and then dies in the back
end (`pycc_mir: internal error: $fn:C.same has no recorded type`). So:

1. `check_call_args` gained a `structural: Option<&Environment>` parameter
   and runs `is_assignable_env` when it is `Some`. The two instance-method
   call sites in `crates/pycc_types/src/class/method_call.rs` pass the
   environment. The arity branch and both `T0021` messages are byte-identical
   to before, so a genuinely non-conforming argument reports exactly what it
   reported before — **no new diagnostic string, no new diagnostic code, no
   new ADR**.
2. `monomorphize_protocol_params` now specializes `HirExpr::MethodCall` via
   a new `specialize_protocol_method_call`, which infers the receiver's
   concrete class, resolves the method through that class's MRO exactly as
   `pycc_mir` does, and mints `0gen_{DefiningClass}.{method}__{P}_{C}`.

The mangled name was the hard part, and both plan review rounds turned on
it. Round 1 rejected the first shape because `0gen_C.take__P_C` breaks
`pycc_mir::lower_item`'s owning-class recovery (`name.split('.').next()`
then a raw `HashMap` index — a panic). Round 2 rejected the obvious repair,
`C.0gen_take__P_C`, because `pycc_codegen` keys direct-value dispatch on
`name.starts_with("0gen_")` (`crates/pycc_codegen/src/lib.rs:5420`), so the
renamed specialization got a null `fn_ptr_global` and aborted with
`pycc_rt_name_error`. The resolution keeps the `0gen_` prefix and fixes the
two `pycc_mir` consumers instead: `resolve_method_owner_class` (tries the
raw prefix, then the `0gen_`-stripped one, then gives the prefix back
unchanged) and `source_frame_name` (strips the same prefix before splitting
on `.`, so a traceback frame reads `take__P_C` rather than the full mangled
name).

**Scope was held to instance methods.** The static-method, class-method,
`super()`-forwarding, constructor and exception-constructor sites keep
`structural: None`. This is not an oversight: each needs its own
monomorphization rewrite for its own call shape, and flipping the predicate
without that only moves the failure from the checker into the back end. All
five were measured against the built binary and filed as
[#954](https://github.com/rotnov/pycc/issues/954), together with an
unrelated pre-existing defect found while probing them — a module containing
both a protocol-typed parameter and *any* `super()` call fails `pycc run`
with a spurious ``error[C0001]: a bare `super()` expression is not
supported``, span pinned to line 1, because monomorphize's generic pass
propagates `infer_expr_in`'s `Err` for `HirExpr::Super`
(`crates/pycc_types/src/expr.rs:1147`). #952 (member-signature conformance
and variance) is a different seam and stayed out.

Tests: 20 in-crate unit tests in
`crates/pycc_types/src/tests/protocol_argument.rs` — both predicates, the
unchanged negative and arity paths, all five still-nominal sites, and every
branch of the new specialization helper including the ones no source program
can reach; 5 in `crates/pycc_mir/src/tests/protocol.rs` for owner-class
resolution and traceback-frame rendering; and 6 end-to-end tests in
`tests/issue_953_protocol_argument.rs` that `check`, then compile and *run*
the issue's own program and four variants, asserting the printed stdout and
that stderr carries no `panicked`, `internal error` or `pycc_rt:` marker.
In-crate coverage is required here rather than optional: integration tests
under `tests/` do not count toward `pycc_types`' own regions (D-014,
`docs/TESTING.md`). Docs: the protocol paragraph in `docs/TYPE_SYSTEM.md`,
whose sentence asserting the `T0021` limitation this change removes was now
false, and the module doc comment of
`tests/issue_948_protocol_self_reference.rs`, which carried the same claim
as a deliberate limitation of that issue's fix. `docs/DIAGNOSTICS.md` needed
no edit: its `T0021` entry is a category description, not an enumeration of
accepted and rejected cases. `docs/ROADMAP.md` carried the same stale
limitation inside the existing #380 protocol paragraph and received a
prose-only edit in place: no new feature-landing paragraph, no new evidence
bullet and no checklist change, so the status-page four-pin rotation is not
triggered (`check_status_page_freshness.rb origin/main` reports no signal).

## Gates

All run from the worktree, all exit 0: `cargo fmt --all -- --check`;
`cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace` (4736 passed, 0 failed, 58 ignored, across 88 test
binaries); the CI coverage sequence
(`cargo build --target x86_64-apple-darwin -p pycc_rt`,
`cargo build --workspace`, `cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100`: TOTAL regions 53586/0 missed = 100.00%, functions 2464/0 missed =
100.00%, lines 35228/0 missed = 100.00%, with
`crates/pycc_types/src/monomorphize.rs` itself at 2998 regions / 76
functions / 1970 lines, all 100.00%);
`python3 -m unittest discover -s scripts -p 'test_*.py'`;
`check_roadmap_evidence.rb`; `check_status_page_freshness.rb origin/main`
(no signal); `check-site.sh`; `check_conformance_breadth.py`;
`check_readme_milestone_projection.rb`;
`generate_decisions_index.py --check`; `check_ci_permissions.rb`; and
`cargo doc --workspace --no-deps`. Nothing touched appears in
`tests/fixtures/policy-successor-manifest.json`.

The pinned D-068 reviewer (`ievo:deep-reviewer`) ran twice on the plan draft
and once on the committed range. On the plan it produced the two mangled-name
blockers described above, both of which changed the design rather than the
prose. On the committed range it produced one finding, no correctness issue:
`docs/TYPE_SYSTEM.md` enumerated four still-nominal call sites while
`resolve_super_method_call` passes `structural: None` as well, making five,
and claimed those sites were "verified" when only the constructor case had a
test. Both halves were fixed in `9d708765` after confirming the `super()`
shape against the built binary (`super().take(C())` reports the same
`T0021`), and unit tests now pin the static-method, class-method and
`super()` shapes.

## Deviations from the plan

- The plan's own advisor round had assumed the front-end predicate change
  might be sufficient. It is not, and the plan was corrected before
  publication; the published plan carries both seams. The implementation
  followed the published plan.
- `docs/DIAGNOSTICS.md` was named as a contingent edit ("only if the T0021
  prose enumerates cases"). It does not, so it was not edited.
- `cargo test --workspace` was run without `--include-ignored`: the local
  oracle is CPython 3.14.6 and the ignored conformance tests require the
  pinned 3.14.7. CI runs them.

## Known follow-ups

- [#954](https://github.com/rotnov/pycc/issues/954) — filed by this
  iteration. Protocol-typed parameters on static methods, class methods,
  `super()`-forwarded methods, constructors and exception constructors still
  report a spurious `T0021`, and each needs a monomorphization rewrite for
  its own call shape alongside the predicate change. Also carries the
  pre-existing spurious `C0001` for a module holding both a protocol-typed
  parameter and a `super()` call, which is independent of the assignability
  question and is the smallest of the five items.
- [#952](https://github.com/rotnov/pycc/issues/952) — non-structural
  protocol member matching: `check_protocol_conformance` compares member
  parameter and return types with plain `is_assignable`, so a concrete class
  conforms only by spelling a protocol-typed member parameter as the
  protocol itself. The seam adjacent to this one, deliberately untouched.
- [#949](https://github.com/rotnov/pycc/issues/949),
  [#944](https://github.com/rotnov/pycc/issues/944),
  [#932](https://github.com/rotnov/pycc/issues/932),
  [#889](https://github.com/rotnov/pycc/issues/889),
  [#894](https://github.com/rotnov/pycc/issues/894),
  [#798](https://github.com/rotnov/pycc/issues/798) — untouched here, for
  `issue-select` to weigh.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #948 closed by PR #951 (`0084258b`).
- This iteration: #953 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

The production change is four files. `check_call_args` in
`crates/pycc_types/src/class.rs` is the predicate seam — its `structural`
parameter documents which callers pass `Some` and which deliberately pass
`None`, and that doc comment is the authoritative list. Do not touch the
"Deliberately not `is_assignable_env`" site further down the same file: that
one is a subtyping test, not an argument check.

`specialize_protocol_method_call` in
`crates/pycc_types/src/monomorphize.rs` is the back-end seam, and the two
`pycc_mir` functions it constrains — `resolve_method_owner_class` and
`source_frame_name` in `crates/pycc_mir/src/lib.rs` — are the reason the
`0gen_` prefix survives on a dotted method name. Anything that changes the
mangled shape must re-check both of them *and* `pycc_codegen`'s
`is_monomorphized` check at `crates/pycc_codegen/src/lib.rs:5420`; the two
plan review rounds were spent discovering that those three consumers
disagree about what a specialization name looks like.

A future iteration extending this to #954's call sites starts by copying
`specialize_protocol_method_call`'s shape for the constructor and
static/class-method call nodes, not by flipping more `None`s to `Some`. The
module doc comment of `crates/pycc_types/src/tests/protocol_argument.rs`
records why.
