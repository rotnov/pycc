# 2026-09-05 — #934: reject a protocol class in return-annotation position with `C0001`

## Previous checkpoint's outcome

Iteration 11 delivered [#941](https://github.com/rotnov/pycc/issues/941)
(reject subclassing an enum class): PR
[#945](https://github.com/rotnov/pycc/pull/945) merged by squash as
`f37ca75fdf37b48e6f45e6869b84f23a8c43d52a` at 2026-09-05T17:57:22Z and #941
is CLOSED. One CI round-trip was lost to a Windows path-separator assertion
in the new CLI test (the renderer prints forward slashes on every platform;
fixed in `151e0511` by normalizing the expected path the same way).

Post-merge `main` runs for `f37ca75f`: Main history audit
[33982547944](https://github.com/rotnov/pycc/actions/runs/33982547944)
`success`, Status page freshness
[33982547935](https://github.com/rotnov/pycc/actions/runs/33982547935)
`success`, Pages
[33982547940](https://github.com/rotnov/pycc/actions/runs/33982547940)
`success`, and CI
[33982547932](https://github.com/rotnov/pycc/actions/runs/33982547932)
**`failure`** — the `native-build-test (ubuntu-latest,
x86_64-unknown-linux-gnu)` leg failed in its `cargo test --workspace` step
(the arm64 leg and every other job succeeded), which propagated to
`ci-gate`. The next `main` commit, `4752915a` (the concurrent actor's PR
[#946](https://github.com/rotnov/pycc/pull/946) for
[#931](https://github.com/rotnov/pycc/issues/931), merged at
2026-09-05T18:47:10Z), ran fully green: CI
[33985135875](https://github.com/rotnov/pycc/actions/runs/33985135875),
Main history audit
[33985135956](https://github.com/rotnov/pycc/actions/runs/33985135956),
Status page freshness
[33985135876](https://github.com/rotnov/pycc/actions/runs/33985135876), and
Pages [33985135877](https://github.com/rotnov/pycc/actions/runs/33985135877)
all `success`, so the `f37ca75f` failure did not reproduce on the following
push and no `[ci-bypass]` or governance action was taken; it stays recorded
here as an unexplained single-leg failure worth a look if it recurs.

## Overall status

Implemented [#934](https://github.com/rotnov/pycc/issues/934) on
`autopilot/iter-2026-09-05-12`, cut from `f37ca75f` and merged with
`origin/main` at `4752915a` (a clean auto-merge; #946 touched
`crates/pycc_hir/src/func.rs`, `tests/diagnostics_test.rs`, and the same
three documents on adjacent lines, with no conflicting hunk). One pull
request carrying `Fixes #934`; the orchestrating session watches CI and
merges. The issue was reconfirmed at `f37ca75f` before the first edit
(`pycc check` exit 0, `pycc run` exit 101 with the `pycc_mir` internal error
at `crates/pycc_mir/src/expr.rs:875`), and the issue and open-PR list were
re-checked before the first edit, at the first commit, and before the push.
The plan is the `issue-to-plan` comment on #934 (published against
`f37ca75f`); this snapshot records where the implementation followed and
where it deviated.

## What the change is

`crates/pycc_hir/src/func.rs::lower_return_annotation` rejects a top-level
`Ty::Protocol` return type with `C0001` on the annotation's own span, after
`annotation_to_ty` has lowered it — the same seam and ordering as the
D-228 decision-9 container gate that #925 removed. That function is the
single choke point for a module-level function, a method
(`class::lower_method`), and a protocol member declaration (both branches
of `class::protocol::lower_protocol_class`), so one check closes every
panicking shape the plan measured: the issue's `p: P = make(); p.foo()`
(`pycc_mir` "method `foo` not declared on class `P`"), the result passed to
a protocol-typed parameter (`pycc_mir` "has no recorded type"), and an
unused or never-called `-> P` function (`pycc_codegen` "P has no LLVM
representation yet"). Because the check runs after lowering, `-> list[P]`
still reports `T0034` and `-> P | None` still `T0049`; a PEP 695 type
parameter named like a protocol still resolves to `Ty::Param` and is
unaffected. No decision record is added or edited: `C0001` is by definition
a versioned capability boundary, D-166 items 6-7 never promised protocol
*returns*, and the direct precedent is #925 (`4eca5e24`) editing this same
function without an ADR; the return-type-narrowing alternative, if ever
pursued, is the change that would need one.

- `crates/pycc_hir/src/func/return_annotation_tests.rs` (new; wired by
  `#[cfg(test)] mod return_annotation_tests;` at the bottom of `func.rs`):
  the module-level function, a sub-protocol `-> Q` (message names `Q`), a
  private helper, a concrete method, the `T0034`/`T0049` ordering, and the
  negatives (parameter, local, module-level variable, PEP 695 shadowing).
- `crates/pycc_hir/src/class/protocol_return_tests.rs` (new; wired from
  `class.rs`): both protocol-member branches with the exact message and
  span, and a `lower_ok` pin that the self-referential
  `class P(Protocol): def clone(self) -> P: ...` resolves to
  `Instance("P")` and does not fire the gate.
- `crates/pycc_types/src/tests.rs`: the four tests whose `-> P` fixture
  would now panic inside `check_source`'s `expect` are deleted;
  `crates/pycc_types/src/tests/protocol_return.rs` (new) re-covers the
  regions two of them owned — `monomorphize.rs`'s non-`Instance` argument
  and empty-substitution branches through a lowered `-> C` module whose
  `make` return type is rewritten to `Ty::Protocol("P")` by hand, and
  `lib.rs`'s `Assign`/`Return` mismatch arms with a `Ty::Protocol` side
  from source (`T0021` "type mismatch: `P` is not assignable to `int`").
  `has_protocol_param` drops its unused return-type parameter.
- Stale comments corrected: `pycc_codegen/src/call_result.rs` (the
  `Ty::Protocol` arm is now a hand-built-MIR backstop like `Infer`/`Param`),
  the `should_panic` protocol test in `pycc_codegen/src/tests.rs`, and
  `pycc_mir/src/class.rs::class_def_of`.
- `tests/diagnostics/c0001_protocol_return_annotation.{py,expected.txt}`:
  the issue's program verbatim, exactly one diagnostic, registered in
  `tests/diagnostics_test.rs`.
- `tests/issue_934_protocol_return.rs`: `check` and `build` on the issue's
  program (exit 1, `error[C0001]`, `<file>:13:15`, the `def` source line,
  no `panicked`/`internal error`/`pycc_rt:`, no artifact), the uncalled,
  protocol-parameter-argument, and method shapes, and two programs that
  must still run: protocol-typed parameter/local/module variable, and an
  unannotated D-146 helper returning its protocol-typed parameter. Paths
  are normalized to forward slashes for the Windows leg.
- `docs/TYPE_SYSTEM.md` (Protocol row and the PEP 544 section),
  `docs/DIAGNOSTICS.md` (C0001 prose), `docs/ROADMAP.md` (the #380
  paragraph, prose-only — no new landing paragraph, so the status page's
  four pins are untouched), and `pycc explain C0001`
  (`crates/pycc_diag/src/explain.rs`).

Gates run locally on the merged tree (`234bf475`): `cargo fmt --all --
--check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (4611 passed, 0 failed, on the pre-merge tree;
`cargo test -p pycc_hir -p pycc_types` plus the `diagnostics_test`,
`issue_934_protocol_return`, and `issue_941_enum_subclass` binaries
re-run after the merge, 0 failed), the CI coverage sequence
(`cargo build --target x86_64-apple-darwin -p pycc_rt`,
`cargo build --workspace`, `cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100`: TOTAL regions 52934/0 missed = 100.00%, lines 34734/0 missed =
100.00%), `python3 -m unittest discover -s scripts -p 'test_*.py'` (996
tests, OK), `check_roadmap_evidence.rb`, `check_status_page_freshness.rb
origin/main` (no signal), `check-site.sh`, `check_conformance_breadth.py`,
`check_readme_milestone_projection.rb`, `generate_decisions_index.py
--check`, and `cargo doc --workspace --no-deps`.

## Deviations from the plan

- The `monomorphize.rs` re-cover does not hand-build a `HirModule` item by
  item: it lowers a `-> C` program with `pycc_hir::lower_checked` and
  rewrites `make`'s `return_ty` to `Ty::Protocol("P")`, which is the exact
  HIR the front end used to produce and is far less brittle than
  constructing class defs by hand. Both false branches are executed (the
  post-change coverage total proves it).
- The HIR gate tests went into `func/return_annotation_tests.rs` (the
  plan's first option) rather than #946's `src/tests/<topic>.rs` layout;
  #946 had not merged when the file was created, and both satisfy the
  decomposability rule.
- The `pycc_codegen/src/tests.rs` comment near the `Infer` `should_panic`
  test needed no change (it never claimed protocol returns were accepted);
  a short comment was added to the protocol `should_panic` test instead.
- `cargo test --workspace` was run without `--include-ignored`: the local
  oracle is CPython 3.14.6 and the conformance tests require 3.14.7.
- Neither follow-up issue named in the plan's "out of scope" section
  (the spurious `T0046` for a self-referential protocol member; the
  `T0022` "private helper" wording on a public protocol-parameter
  function) was filed from this branch; they are listed below for the
  orchestrating session to file.

## Known follow-ups

- [#944](https://github.com/rotnov/pycc/issues/944) — the enum-call `C0001`
  (#921) renders at `1:1`; carry the call expression's span.
- [#931](https://github.com/rotnov/pycc/issues/931) — closed by #946
  (`4752915a`) while this iteration ran; nothing left to do.
- [#932](https://github.com/rotnov/pycc/issues/932),
  [#905](https://github.com/rotnov/pycc/issues/905),
  [#889](https://github.com/rotnov/pycc/issues/889),
  [#882](https://github.com/rotnov/pycc/issues/882) — open `v0.4` work
  for `issue-select` to weigh.
- To file (found while planning #934, not caused or fixed by it): a
  self-referential protocol member `class P(Protocol): def clone(self)
  -> P: ...` lowers to `Instance("P")` and every conforming class is then
  rejected with a spurious `T0046`; and `def f(p: P) -> int: return p`
  reports `T0022` worded "private helper return type" for a public
  function.
- The unexplained `cargo test --workspace` failure on the ubuntu x86_64
  leg of CI run 33982547932 (`f37ca75f`), green again on `4752915a`.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #941 closed by PR #945 (`f37ca75f`).
- This iteration: #934 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

`lower_return_annotation` in `crates/pycc_hir/src/func.rs` is the whole
code change; `func/return_annotation_tests.rs`,
`class/protocol_return_tests.rs`, the `c0001_protocol_return_annotation`
fixture, and `tests/issue_934_protocol_return.rs` lock the message and the
span. If a later slice makes protocol returns work (it would need an ADR:
either narrow the return type from the body or give a protocol-typed call
result a concrete binding), the `if let Ty::Protocol(..)` block is what to
remove, `pycc_types/src/tests/protocol_return.rs`'s hand-rewritten HIR
becomes reachable from source again, and `docs/TYPE_SYSTEM.md`'s Protocol
row and PEP 544 section, `docs/DIAGNOSTICS.md`'s C0001 prose, the #380
paragraph in `docs/ROADMAP.md`, and `pycc explain C0001` are the prose that
must change with it.
