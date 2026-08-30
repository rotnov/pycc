# Session handoff: #714 reject exception-value-binding as a value

## Status: merged

[PR #843](https://github.com/rotnov/pycc/pull/843) merged as `15dd4633`,
closing [#714](https://github.com/rotnov/pycc/issues/714). This session
took over an in-flight fix from a stalled dispatched agent (the core fix,
commit `04df9940`, was authored and independently verified in the prior
compacted window), then in this window: received two independent
`ievo:deep-reviewer` passes over that commit, reconciled their findings,
implemented the warranted fixes, and merged.

Note on this file's own lateness: AGENTS.md's D-066/D-130 convention is to
commit this handoff inside the same pull request it documents, so it lands
with that merge. This entry was instead written and committed directly to
`main` after #843 had already merged -- a process slip discovered only
while drafting it, not a deliberate deviation. Recorded here rather than
silently corrected, per this project's own retrospective discipline.

## What changed (commit `04df9940`, prior window)

Binding a user-defined exception subclass as an ordinary value
(`e = MyError("boom")`, not raising it) used to compile cleanly and then
abort at runtime with a `NameError` naming the synthetic
`Exception.__init__` placeholder. `class::resolve_instantiation`'s existing
`is_synthetic_class` guard (D-188) now also rejects that construction at
compile time as `C0001` ("cannot instantiate exception class `...` as a
value"). `raise MyError("boom")` stays accepted: `check_raise_operand`
validates the raise operand's constructor arguments directly against the
placeholder's registered signature instead of routing through
`resolve_instantiation`, since MIR's `lower_exception_value` constructs the
raised object directly and never calls through `Exception.__init__` at
runtime.

## What changed (commit `15d90960`, this window -- deep-review findings)

Two independent `ievo:deep-reviewer` dispatches (`ae63b9e623398a974` and
`a883a03d3c7ba979f`) reviewed commit `04df9940`. Findings reconciled and
addressed:

- **Genuine correctness regression (confirmed).** `check_raise_operand`'s
  bypass of `resolve_instantiation` had silently dropped
  `resolve_instantiation`'s pre-existing `is_abstract` (PEP 3119, #380)
  instantiability guard for the `raise` path -- verified by diffing the
  pre-#714 code, which routed through `infer_expr_in` ->
  `resolve_instantiation` and so inherited that guard for free. Restored
  the guard directly in `check_raise_operand`, with a new regression test
  (`raising_an_abstract_exception_subclass_stays_rejected` in
  `crates/pycc_types/src/exception/user_class_tests.rs`) proving
  `class MyError(Exception, ABC): @abstractmethod ...` followed by
  `raise MyError("boom")` stays rejected as `C0001`.
- **Rejected a naive mirror-fix.** `resolve_instantiation` also has a
  sibling `is_protocol` guard; a first attempt at this fix mirrored it into
  `check_raise_operand` too. That branch is structurally unreachable:
  `is_protocol` (PEP 544) requires `Protocol` as a class's *sole* base
  (`pycc_hir::class`'s own lowering enforces `bases.len() == 1`), which
  precludes also reaching `Exception` in the MRO -- so `def.is_protocol`
  can never be true for a `def` this branch reaches. Confirmed empirically:
  adding the branch made the local D-014 coverage gate fail (99.60%
  lines / 97.78% regions on `exception.rs`, 2 uncovered lines / 9
  uncovered regions). Removed the branch rather than padding coverage with
  a synthetic-only test for a shape no real compiler input can produce,
  citing `redeclaration.rs`'s established `.expect()`-for-unreachable-shapes
  convention as precedent. Documented the reasoning inline at the removal
  site.
- **Doc-drift fixes.** Two comments (in `crates/pycc_types/src/class/binding.rs`
  and `tests/issue_714_exception_value_binding.rs`) inaccurately claimed
  codegen never materializes a callable definition for the synthetic
  `Exception.__init__` item. Investigated directly against
  `crates/pycc_hir/src/lib.rs`'s item-append order and codegen's
  function-emission pass: codegen *does* emit a real body for every
  `MirItem::Function`, including this one. The actual defect is
  call-ordering -- `pycc_hir::lib.rs` always appends the synthetic item
  **last** in `hir.items` regardless of source position, and codegen binds
  each item's function-pointer slot at its module-order position, so the
  synthetic item's slot is bound after every earlier statement runs; any
  earlier call through the slot observes a null pointer. Rewrote both
  comments to state this correctly.
- `docs/TYPE_SYSTEM.md` and `docs/DIAGNOSTICS.md` updated to describe the
  new rejection and its corrected mechanism (AGENTS.md's "documentation
  work is part of every implementation task").
- Fixed a stale "own private helper" comment in `crates/pycc_types/src/enum_lower.rs`
  (both `class::binding` and `exception` reach `class`'s `pub(crate)`
  `check_call_args`, not just `class` itself).

## Local gates (this session, commit `15d90960`)

- `cargo build`, `cargo test -p pycc_types`, full workspace test suite,
  release-mode integration test: all green.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo doc --workspace --no-deps`: clean.
- D-014 coverage gate: `cargo llvm-cov --workspace --fail-under-lines 100
  --fail-under-regions 100` -- TOTAL 48980 lines / 2113 regions / 31622
  functions, all 100.00%, after removing the dead `is_protocol` branch (see
  above for the intermediate failing run).
- `scripts/test_*.rb` (run with `RUBYOPT="-E UTF-8"` to avoid unrelated Ruby
  encoding failures): all green except the pre-existing, already-confirmed
  unrelated `scripts/test_check_pages_performance_budget.rb` failure.
- CI on PR #843: green (`ci-watch.sh` reported `READY`), `mergeStateStatus:
  CLEAN`, zero unresolved review threads, `closingIssuesReferences`
  confirmed as exactly `{714}` before merge.

## Follow-ups / known non-issues

- No new issues were filed or narrowed as a side effect of this task.
- The standing `/goal fix all opened issues` autopilot directive remains
  open-ended; the next iteration re-enters `issue-select` (or
  `next-milestone` if a milestone boundary applies) from a fresh baseline.
- Process note for future sessions: commit the `docs/sessions/` handoff
  file inside the delivering pull request itself, before merging -- not
  as a follow-up commit to `main` afterward, which is what happened here.
