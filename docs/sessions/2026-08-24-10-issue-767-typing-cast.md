# 2026-08-24-10 -- Issue #767: `typing.cast(T, value)` as a special-cased builtin call

## Status: delivered by the pull request that carries this file

Branch `feat/typing-cast-767`, originally based on `origin/main` at `5be4a055`
and rebased onto `origin/main` at `68ee4eda` (the merge of PR #772, issue
#763's PEP 604/`Optional[T]` work) after that commit landed mid-task. The pull
request opened from that branch carries `Fixes #767`. This snapshot is written
inside that pull request, so it records the branch rather than a PR number.

## What shipped

`typing.cast(T, value)` is a compile-time-only construct: in CPython it is a
runtime no-op whose sole effect is to declare `value`'s static type as `T`.
Before this change `from typing import cast` failed at import resolution with
`C0002`, and a registry entry alone would not have been enough -- `args[0]` is
a *type* expression, not a value binding, so the generic argument-inference
loop rejects it as an undefined name. Five seams:

- `crates/pycc_std/src/lib.rs` -- registers `typing.cast` under a new
  `StdSymbolKind::CastMarker`. A dedicated kind rather than reusing
  `AnnotationMarker` because `cast` is genuinely callable, so both existing
  marker messages would state something false about it; a dedicated
  `cast_marker_is_not_a_value` diagnostic points at `from typing import cast`.
  This applies #762/PR #766's own review finding proactively.
- `crates/pycc_types/src/class.rs` -- `check_cast`, plus the shared
  `cast_target_name` / `cast_target_ty` helpers both passes use.
- `crates/pycc_types/src/expr.rs` -- validation-pass interception ahead of the
  generic argument loop, like `isinstance`/`issubclass`.
- `crates/pycc_types/src/constraints.rs` -- the solver mirror, for calls inside
  return-type-inferred private helpers.
- `crates/pycc_mir/src/expr.rs` -- lowers the whole call to `args[1]`, so no
  `MirExpr::Call` for `cast` ever reaches codegen.

A user-defined `def cast(...)` takes priority in all three passes.

## Decisions made autonomously (D-127)

- **No new diagnostic code.** A subscripted or otherwise non-bare-name target
  reuses `C0001`, the versioned capability code, matching `isinstance`'s own
  use of it for an implemented builtin called with an argument shape this
  version does not support. An unknown target name reuses `T0001` via the
  shared `validate_class_name`; wrong arity reuses `T0021`. This avoids D-150's
  `EXPLANATIONS`/`docs/DIAGNOSTICS.md` coupling for a case that is exactly what
  the existing codes already mean. `docs/DIAGNOSTICS.md`'s `C0001` prose now
  names both instances rather than leaving them undocumented.
- **The solver reports malformed-shape diagnostics itself**, an intentional
  divergence from `isinstance` (which returns `Ty::Bool` unconditionally from
  the solver and validates only in the check pass). Without it a malformed
  `cast` inside a private helper surfaced as the generic `T0021` "cannot infer
  return type of private helper", masking the accurate `C0001`.
- **The solver produces a term only for the four builtin scalar targets.** It
  has no class table, and returning an unverified `Ty::Instance(name)` made
  `cast(Nope, x)` report `T0022` ("conflicting inferred types") ahead of
  `check_cast`'s accurate `T0001`. An unrecognized name yields `Ok(None)` and
  leaves the decision to `check_cast`.
- **`cast` is representation-preserving, not unchecked (new ADR
  [D-198](../decisions/D-198-cast-erasure-limits-cast-to-representation.md)).**
  The pinned local reviewer found that eliding `cast(str, 5)` to the integer
  `5` leaves the checker validating the program against `str` while the code
  still carries an `i64` -- a codegen `debug_assert_eq!` panic in a debug
  build, misinterpreted bits in a release one. `check_cast` now requires the
  target to preserve the value's runtime representation, attribute layout,
  *and* method-dispatch behavior: the value's own type, or an up-cast to one
  of its class's MRO ancestors that crosses no method-override boundary (the
  nominal relationship is deliberately unverified for that subset, matching
  CPython's and mypy's unchecked `cast`). A genuine down-cast is rejected,
  not merely unverified -- see the third-pass entry below, which also covers
  the method-override restriction. Four follow-on calls made across three
  further reviewer passes: the first attempt reused `is_assignable_env` as
  the gate, which is a *subtyping* test -- it admits `bool` -> `int` (a real
  representation change, `i8` vs `i64` per `TYPE_SYSTEM.md`) and restricted
  `Instance` -> `Instance` to protocol conformance, so it accepted the
  unsound case and rejected the useful one; it was replaced with an explicit
  representation predicate that (in its first version) accepted every
  `Instance` -> `Instance` pair unconditionally, which a second review pass
  then found unsound for down-casts specifically (see below) and narrowed to
  MRO ancestry; a third review pass then found the MRO-ancestry-narrowed
  version still unsound for a distinct, dispatch-related reason (see below),
  and the predicate was renamed `cast_compatibility` and split to also
  reject an up-cast crossing a method-override boundary. And the rejection
  reports `C0001`, not `T0021`: the program is not ill-typed Python, the
  limit is pycc's own erasure, and `C0001`'s prose already covers "an
  implemented special-cased builtin called with an argument shape this
  version does not support".
- **The missing import gate is filed as #768, not fixed here.** The pinned
  local reviewer's one finding (severity `warning`) was that bare `cast(...)`
  is intercepted without checking that the module wrote
  `from typing import cast`, so pycc accepts a program CPython rejects with
  `NameError`. Investigation showed the gap is not `cast`-specific:
  `pycc_types` never reads `HirModule::imports` -- those bindings are consumed
  only by `pycc_hir`'s class-name-collision check -- so no bare-name stdlib
  symbol is import-gated, `Final`/`Annotated`/`Enum`/`Protocol`/`ABC` included.
  Gating `cast` alone would open a new architectural seam (import visibility
  threaded through `Environment`'s three constructors and `child_for_function`,
  plus a separate path for the solver's `ConstraintEnvironment`) and would make
  `cast` stricter than every neighbouring symbol. Per AGENTS.md's decomposition
  rule the gap is tracked uniformly in
  [#768](https://github.com/rotnov/pycc/issues/768) (milestone v0.3), pinned by
  `cast_without_its_import_is_currently_accepted` and by a comment at the
  interception site so the fix has to invert it deliberately. The finding is
  over-acceptance of an invalid program, not a miscompile: no correct program
  compiles wrong.
- **Two `check_isinstance` guards are deliberately absent** and the omission is
  documented on `check_cast`: no side-effect guard on the value operand (a
  `cast` *is* its value operand after lowering, so a call there is evaluated
  exactly once, as in CPython), and no `@runtime_checkable` gate on a protocol
  target (`cast` performs no runtime class test and emits no code).
- **Third review pass: the accepted up-cast subset still diverged silently
  from CPython through method dispatch (new blocker, closed here).** After
  the second pass narrowed the class-to-class predicate to MRO
  ancestry-or-identity, a third pinned-reviewer pass (run after that fix was
  committed) found the accepted up-cast subset still unsound for a reason
  orthogonal to representation and layout: `pycc_mir` resolves method calls
  *statically* from the MIR-tracked type of the receiver (no vtable), and an
  `AnnAssign` re-anchors that tracked type to the declared annotation, so
  `b: Base = cast(Base, d)` makes every later `b.m()` dispatch through
  `Base`'s MRO even when `d`'s real class is `Derived`. If `Derived`
  overrides a method `Base` defines or inherits, that static resolution
  silently returns `Base`'s implementation instead of the override CPython's
  dynamic dispatch would call -- a wrong answer with no diagnostic and no
  crash, worse than every other failure mode this issue's `cast` handles.
  Before this pass no other construct in the accepted language subset could
  produce a variable whose MIR-tracked type differs from its actual
  allocated class (plain assignment and call-argument checking both require
  exact `Ty::Instance` equality), so `cast`'s up-cast was the first construct
  able to expose this latent gap in pycc's static-dispatch model.
  `cast_compatibility` (renamed from `cast_shares_representation`, since it
  now answers a broader question than pure representation) additionally
  rejects an up-cast when any class strictly more derived than the target,
  down to and including the value's own class, overrides a method reachable
  from the target's own MRO. `__init__` is excluded from that check on both
  sides: it runs once at construction, before any `cast` of the resulting
  object could apply, so every subclass defining its own `__init__` -- the
  ordinary case for a real class hierarchy, not an exception -- would
  otherwise make nearly every up-cast unsound by this rule for a call that
  can never be re-dispatched through the cast result. Two options considered
  and rejected in favor of this targeted check: rejecting every class-to-class
  `cast` (deletes the already-tested up-cast path to close a hole a cheap
  check closes just as soundly), and documenting the gap as a known
  limitation parallel to #768's import-gate deferral (rejected because #768
  defers over-acceptance of an *invalid* program, while this gap would have
  made a *correct* program compile to a silently wrong answer -- see
  D-198's "Third review pass" paragraph and its Alternatives entries for the
  full reasoning). The rejection reports `C0001` with a message naming the
  specific overridden method, distinct from the representation and layout
  messages (see the next bullet).
- **The fused `C0001` message was split into one string per rejection
  reason** (reviewer `note`, closed here). The representation, layout, and
  method-override rejections previously shared one message containing both
  "runtime representation" and "attribute layout" phrasing regardless of the
  actual cause, so a test asserting either substring passed no matter which
  branch fired -- the assertions looked discriminating but were not.
  `cast_compatibility` now returns a `CastMismatch` enum (`Representation` /
  `Layout` / `OverriddenMethod(method_name)`) and `check_cast` matches on it
  to build a message and help text specific to the actual failure, so
  `cast_changing_representation_is_c0001`,
  `cast_down_to_a_derived_class_is_c0001`, and the new
  `cast_up_across_an_overridden_method_is_c0001` each assert a substring only
  their own branch produces.
- **Added CLI-level (build+run) coverage for the up-cast acceptance path**
  (reviewer `note`, closed here). `tests/issue_767_typing_cast.rs` previously
  exercised only the identity-cast case end-to-end; a new test builds and
  runs a program that up-casts and calls a *non*-overridden inherited
  method through the result, asserting the compiled program's output matches
  calling the method directly on the original value -- exactly the kind of
  test that would have caught the method-dispatch gap empirically before a
  human review pass had to find it by inspection.
- **Fourth review pass: two doc-wording defects and one test gap, all closed
  here.** A fourth pinned-reviewer pass (run after the third pass's fixes and
  the message-split/CLI-coverage `note`s above were committed) confirmed the
  override-detection loop sound by hand-tracing 3-level chains and diamond
  MRO, confirmed the two new white-box tests genuinely exercise their claimed
  branches, and found: (1) `docs/ROADMAP.md`'s #767 paragraph still described
  the pre-third-pass rule, omitting the method-override rejection entirely --
  reworded to state the boundary and its `__init__` exclusion, citing D-198's
  third review pass; (2) `CastMismatch::OverriddenMethod`'s help string was
  phrased backwards -- it told the reader to cast "to a base class that
  overrides none of the value's class's methods", the wrong direction (the
  unsound case is a *subclass between* the value's class and the target
  overriding a *base* method, not the base overriding anything) -- reworded to
  name the actual direction; (3) no test exercised an override living in an
  *intermediate* ancestor rather than the value's own class directly -- every
  existing `OverriddenMethod` test put the override at position 0 of
  `from_def.mro[..to_pos]`. Closed with a new
  `cast_up_across_an_override_in_an_intermediate_ancestor_is_c0001` test (see
  Test evidence) using a 3-level `A` / `B(A)` overriding `describe` / `C(B)`
  not overriding it, asserting `cast(A, c_instance)` is still rejected even
  though `C`, the value's own class, overrides nothing. No logic change; `check_cast`
  and `cast_compatibility` are unchanged by this pass. Judged (D-127) not to
  warrant a fifth reviewer pass before commit: all three fixes are the fourth
  pass's own literal suggestions, two are prose-only and the third is a test
  whose correctness was independently confirmed by running it, so the fourth
  pass's analysis still describes the code after applying them.
- **Rebased onto `origin/main` after PR #772 merged, and resolved a
  decision-number collision by renumbering.** While the fourth review pass's
  fixes were being finalized, `origin/main` advanced past this branch's
  original merge base (`5be4a055`) with the merge of PR #772 (issue #763's
  PEP 604/`Optional[T]` work), which had independently claimed decision
  number D-197 for its own `docs/decisions/D-197-optional-t-representation-and-is-none-part1.md`.
  This branch had also filed its cast-soundness decision as D-197. Per AGENTS.md
  D-021 preflight ("start every new task from the exact latest commit of that
  remote default branch"; "prefer a fast-forward update ... never merge,
  rebase, reset, switch branches, or pull over uncommitted or user-owned
  changes" -- read here as governing how to *integrate* new upstream commits
  while preserving this branch's own work, which a rebase does) and the
  `issue-to-plan` skill's own numbering guidance ("resolve the next free
  entry number at pull-request-open time, not at planning time, because open
  pull requests may claim numbers first"), the branch not yet on `main`
  renumbers: this branch's decision was renamed to
  `docs/decisions/D-198-cast-erasure-limits-cast-to-representation.md` (via
  `git mv` plus an in-file `s/D-197/D-198/g`), and every cast-specific
  cross-reference in the diff (`crates/pycc_diag/src/explain.rs`,
  `crates/pycc_types/src/class.rs`, `crates/pycc_types/src/constraints.rs`,
  `crates/pycc_types/src/tests.rs`, `docs/DIAGNOSTICS.md`, `docs/ROADMAP.md`,
  `docs/STDLIB_PLAN.md`, `tests/issue_767_typing_cast.rs`, this file) was
  swept to match, while issue #763's own D-197 mentions were left untouched.
  `docs/decisions/README.md` carries both rows. All five of this branch's
  commits replayed cleanly across the rebase; the only conflicts were
  `docs/AGENT_RETROSPECTIVE.md` (both sides purely appending new dated
  entries -- resolved by keeping both) and the D-197/D-198 collision itself
  in `docs/decisions/README.md`.
- **A pre-existing, unrelated diagnostic-quality bug was found and filed
  separately, not fixed here.** A `cast` rejection reports a misleading
  `T0021` "local name not bound" instead of its real diagnostic when the
  cast expression is bound to a plain (non-annotated) local variable before
  use, rather than used inline -- e.g. `d = cast(Derived, base); return d.b`
  reports `T0021` where the equivalent `return cast(Derived, base).b`
  correctly reports `C0001`. Confirmed pre-existing (reproduces on the
  already-merged down-cast rejection from the second pass, not only on the
  new method-override rejection), and confirmed cosmetic: the program is
  still correctly rejected, just under a confusing code. Root cause is
  outside `pycc_types/src/class.rs`'s `cast`-specific logic -- most likely
  the definite-assignment tracking added for issue #118 (D-147) evaluating
  the assignment's RHS independently of (or ahead of) the ordinary
  type-check pass. Filed as
  [#771](https://github.com/rotnov/pycc/issues/771) (milestone v0.3) rather
  than investigated further here, since it is a distinct defect in a
  different subsystem, not a soundness issue, and not `cast`-specific in its
  likely cause. The new `cast_up_across_an_overridden_method_is_c0001` test
  works around it by keeping the rejected `cast` expression inline rather
  than assigned to a variable first.
- **The D-197-to-D-198 renumbering commit pushed `docs/ROADMAP.md`'s
  non-optional `llms.txt` expansion over budget; fixed by trimming, not by
  relaxing the gate.** `scripts/check-site.sh` enforces a 256 KiB aggregate
  ceiling (issue #207) on the sum of the non-optional documents
  `site/llms-txt-context-manifest.json` lists, `docs/ROADMAP.md` among them.
  Confirmed at `origin/main`'s tip (`68ee4eda`, via a scratch `git worktree
  add --detach`) the aggregate had only ~3.6 KiB of headroom; this branch's
  own `docs/ROADMAP.md` growth across four review passes plus the D-198
  rename exceeded it by 548 bytes. Condensed the #767 changelog paragraph's
  wording (no fact removed -- same scope, same soundness rule, same test and
  diagnostic-code references) rather than trimming any other document or
  touching the manifest/budget itself; `scripts/check-site.sh` now passes
  with margin to spare.

## Test evidence

- `crates/pycc_types/src/tests.rs` -- 28 `cast`-named unit tests (through the
  public `check_source` entry point): every builtin
  scalar target, a user-defined class target, wrong arity, subscripted target,
  unknown target, errors inside the value operand, user-defined `cast`
  priority in both passes, the qualified-marker-as-value /
  qualified-marker-called paths in both passes,
  `cast_without_its_import_is_currently_accepted` (the #768 pin), and ten for
  D-198's representation/layout/dispatch rule across its four review passes:
  `cast_up_across_an_override_in_an_intermediate_ancestor_is_c0001` (the
  fourth-pass test above, pinning a 3-level chain where the override lives
  strictly between the value's class and the cast target rather than on the
  value's own class), `cast_up_to_a_base_class_checks` and
  `cast_up_to_a_base_class_with_no_overridden_methods_still_checks` (up-cast
  accepted, the second not defeated by an `__init__` override),
  `cast_down_to_a_derived_class_is_c0001` and
  `cast_between_two_unrelated_class_types_is_c0001` (layout rejections),
  `cast_up_across_an_overridden_method_is_c0001` (the third-pass
  method-dispatch rejection, asserting the message names the specific
  overridden method), `cast_changing_representation_is_c0001` /
  `cast_changing_representation_in_an_annassign_is_c0001` /
  `cast_widening_bool_to_int_is_c0001` / `cast_narrowing_int_to_bool_is_c0001`
  (representation rejections). Each rejection test asserts a message substring
  specific to its own `CastMismatch` variant, not a fused string that would be
  true regardless of cause (see the "fused message was split" bullet above).
- `crates/pycc_types/src/class.rs`'s own inline `#[cfg(test)] mod tests`
  (white-box, direct calls into private helpers, alongside this file's
  existing "internal-consistency panics" section) -- 2 new tests closing the
  coverage gate the D-198 rename to `cast_compatibility` opened:
  `cast_compatibility_treats_an_unregistered_from_class_as_a_representation_mismatch`
  and
  `cast_compatibility_treats_an_mro_entry_missing_its_own_class_def_as_a_layout_mismatch`.
  The original `cast_shares_representation(...) -> bool` folded its
  `env.lookup_class(from_name).is_none()` case into the same trailing `false`
  another, already-tested path also returned; splitting the function into
  three named `CastMismatch` outcomes gave each `let-else`'s failure arm its
  own source region, and neither region is reachable through `check_cast`
  from any real `check`-validated program -- every `Ty::Instance` name it can
  see is either a user class or one of the 23 HIR-seeded builtin exception
  classes (`crate::exception::is_user_defined_class`), and `validate_bases`
  only ever admits an already-defined class into a base's MRO. These two
  tests hand-build the same kind of "declared shape and `Environment`
  disagree" state the file's pre-existing internal-consistency tests already
  exercise for `resolve_attr_get`/`resolve_method_call`/`check_attr_set`, and
  assert `cast_compatibility` degrades to the correct `CastMismatch` variant
  instead of panicking -- consistent with the fact that `cast_compatibility`
  is public-diagnostic-facing (unlike those panicking helpers) and must never
  crash the compiler even if the invariant it depends on is ever violated
  elsewhere. `CastMismatch` picked up `#[derive(Debug, PartialEq)]` so the
  two tests could assert with `assert_eq!` instead of `assert!(matches!(...))`
  -- the latter's `_ => false` arm is dead in a passing test and cost another
  region point under the same 100%-regions gate.
- `crates/pycc_mir/src/tests/builtin.rs` -- 2 new tests: the call is elided to
  its value argument; a user-defined `cast` still lowers to a real call.
- `tests/issue_767_typing_cast.rs` -- 6 CLI end-to-end tests, including a
  `build`+`run` whose stdout is asserted byte-identical to the same program with
  every `cast(T, v)` replaced by `v` alone, a runtime check that a
  user-defined `def cast(...)` is really called, the down-cast and
  method-override rejections reproduced through `pycc check` (not only the
  unit-test harness), and a `build`+`run` up-cast test calling a
  non-overridden inherited method through the cast result (the third-pass
  `note` finding's empirical gap, now closed). All six pass under coverage
  instrumentation.
- `cargo test --workspace`: 0 failures, re-run three times on this tree (after
  the third-pass fix; again after the fourth-pass doc/help-string/test fixes
  above).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  re-run locally on this tree after the third-pass fix, and again from a clean
  profile (`cargo llvm-cov clean --workspace`) after the fourth-pass fixes:
  both runs report 41874 regions / 27228 lines, `0` missed of either,
  `100.00%`/`100.00%`, 0 test failures. The identical totals across the two
  runs despite the fourth pass adding one new `#[cfg(test)]` function are
  expected, not stale data: test-module code itself is not counted in the
  instrumented totals, only the library code it exercises, which was already
  at 100%; the clean re-run's log confirms a genuine recompile
  (`Compiling pycc_types ...`) and that the new test's name appears in the
  run's own test output, ruling out a stale profile. The first attempt at the
  third-pass re-run (before the two new internal-consistency tests above)
  surfaced a real gap this bullet's sibling above describes: 2 missed
  lines/regions in `crates/pycc_types/src/class.rs` (the two
  now-separately-tracked `let-else` failure arms in `cast_compatibility`),
  located precisely via `cargo llvm-cov --workspace --no-run --show-missing-lines`
  and `cargo llvm-cov --workspace --no-run --json --output-path ...` (the
  latter needed to disambiguate a region miss from a line miss once the line
  count alone reached 100%). Run it from a clean profile if a previous
  coverage run was interrupted -- a partial profile from a killed run silently
  under-reports (it produced a spurious 98.05%/98.07% here, with the losses
  concentrated in the code exercised through `pycc` subprocesses:
  `src/main.rs`, `pycc_ast`, `pycc_artifact_layout`). The fourth-pass
  remediation hit this same trap once more: a first coverage attempt died with
  `signal: 9 (SIGKILL)` (OOM from a concurrent peer session's own
  `cargo test --workspace` in a separate worktree, confirmed via `vm_stat` and
  `ps aux`) -- a killed run's own reported wrapper exit code is not proof the
  underlying `cargo llvm-cov` invocation succeeded, only the log's `TOTAL` row
  and explicit `EXIT:` marker are, so grep for both rather than trusting a
  background task notification's summary.
- `bash scripts/check-site.sh`, `ruby scripts/check_status_page_freshness.rb`,
  `ruby scripts/check_roadmap_evidence.rb`, `ruby scripts/check_ci_permissions.rb`,
  and `python3 scripts/generate_decisions_index.py ... --check` all pass.
- `python3 scripts/manage_ci_bypass.py status`: branch protection matches the
  documented baseline; no `[ci-bypass]` incident open.

## Documentation updated in the same PR

- `docs/ROADMAP.md` -- new v0.3 feature-landing paragraph for #767.
- `docs/STDLIB_PLAN.md` -- `typing.cast` added to the typing row, with a note
  that it is the one entry that is not purely an import-resolution fix.
- `docs/DIAGNOSTICS.md` -- `C0001` prose extended to cover an implemented
  special-cased builtin called with an unsupported argument shape
  (`isinstance(f(), int)`, `cast(list[int], x)`).
- `site/status/index.html` + `tests/fixtures/pages-performance-manifest.json`
  -- D-156/D-170 status-page freshness: the roadmap feature paragraph obliges a
  status-page edit, and the edit obliges a new pinned source digest. The
  `dateModified`/`<lastmod>` values were already `2026-08-24`, so
  `scripts/check-site.sh`'s expected-date constants needed no change this time.

## Known follow-ups / non-actions

- No conformance-matrix row moves: no PEP fixture covers `typing.cast`.
- `cast(C, p)` inside a function with a protocol-typed parameter reports
  `T0021 name `C` is not defined`: the protocol-monomorphization rewrite path
  re-infers the body without the `cast` interception, so the target name is
  treated as a value. Found while writing a D-198 test for a protocol-typed
  value; the test was dropped rather than pinning a confusing message. Not a
  regression from this PR (the same path predates it), and not reachable in
  the accepted subset any other way. Worth an issue in v0.3 if `cast` use
  spreads.
- [#768](https://github.com/rotnov/pycc/issues/768) (v0.3) -- bare-name stdlib
  symbols accepted without their import; see the decision above.
- `main` at `5be4a055` is not `cargo fmt` clean -- `crates/pycc_codegen/src/tests.rs`,
  `crates/pycc_hir/src/exception.rs`, `crates/pycc_hir/src/exception/tag_tests.rs`,
  `crates/pycc_hir/src/stmt/exception.rs`,
  `crates/pycc_types/src/exception/synthetic_class_tests.rs`,
  `tests/issue_739_oserror_hierarchy.rs`, `tests/issue_740_multi_type_except.rs`,
  and `tests/issue_762_typing_final_annotated.rs` all differ under
  `cargo fmt --all -- --check`. A blanket `cargo fmt --all` during this task
  reformatted them; the reformatting was reverted to keep this PR scoped to
  #767. There is no `cargo fmt` gate in `.github/workflows/`, which is why the
  drift accumulated. Not filed as an issue: it cannot cause an incorrect merge
  decision or hide a compiler defect, so it fails AGENTS.md's process-observation
  filing bar. Recorded here and in `docs/AGENT_RETROSPECTIVE.md` instead.
- The local `x86_64-apple-darwin` `pycc_rt` build was stale (missing the
  `pycc_rt_exception_*` symbols), which failed
  `build_and_run_cross_compiled_to_a_different_tier_1_target` in the local
  coverage run: the test's availability guard sees an archive and does not
  skip, then the cross link fails on the undefined symbols. The archive that
  matters is `target/x86_64-apple-darwin/debug/libpycc_rt.a` -- the *root*
  target directory, which is where the `pycc` binary under test resolves the
  runtime from, not `target/llvm-cov-target/`. Rebuilding the llvm-cov copy
  first had no effect for that reason; `cargo build --target x86_64-apple-darwin -p pycc_rt`
  fixed it. A local environment artifact, not a repository defect.

## Where to resume

Nothing outstanding from this task beyond the PR's own CI and review cycle.
`docs/sessions/` listing sorted by filename remains the resume mechanism; this
is the newest entry for 2026-08-24.
