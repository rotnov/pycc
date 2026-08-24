# 2026-08-24-10 -- Issue #767: `typing.cast(T, value)` as a special-cased builtin call

## Status: delivered by the pull request that carries this file

Branch `feat/typing-cast-767`, based on `origin/main` at `5be4a055`. The pull
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

## Test evidence

- `crates/pycc_types/src/tests.rs` -- 17 new unit tests: every builtin scalar
  target, a user-defined class target, wrong arity, subscripted target, unknown
  target, errors inside the value operand, user-defined `cast` priority in both
  passes, the qualified-marker-as-value / qualified-marker-called paths in both
  passes, and `cast_without_its_import_is_currently_accepted` (the #768 pin).
- `crates/pycc_mir/src/tests/builtin.rs` -- 2 new tests: the call is elided to
  its value argument; a user-defined `cast` still lowers to a real call.
- `tests/issue_767_typing_cast.rs` (new) -- 3 CLI end-to-end tests, including a
  `build`+`run` whose stdout is asserted byte-identical to the same program with
  every `cast(T, v)` replaced by `v` alone, and a runtime check that a
  user-defined `def cast(...)` is really called. All three pass under coverage
  instrumentation.
- Full workspace suite green; `cargo llvm-cov --workspace --fail-under-lines 100
  --fail-under-regions 100` run locally.
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
