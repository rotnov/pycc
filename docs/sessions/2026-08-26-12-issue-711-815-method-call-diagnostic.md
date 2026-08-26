# Session handoff: diagnose a direct dunder call on a synthetic exception class (PR #819)

**Date:** 2026-08-26
**Pull request:** [#819](https://github.com/rotnov/pycc/pull/819) — `fix(pycc_types): diagnose a direct dunder call on a synthetic exception class (Part 1 of #737, fixes #711)`, merged as `3a71721908e79d65f2b0268d8f756cacd9da99f8`.
**Closing issues:** [#711](https://github.com/rotnov/pycc/issues/711), [#815](https://github.com/rotnov/pycc/issues/815) (`closingIssuesReferences.totalCount: 2`, verified via `gh api graphql` before merge).

## Background: the #541 → #737 → #815/#714 decomposition

Issue [#737](https://github.com/rotnov/pycc/issues/737) ("Part 3B of #541: materialize exception payloads") was split, per its own `issue-to-plan` run (planned against `6582c880`), into two independent code seams:

- **Part 1** ([#815](https://github.com/rotnov/pycc/issues/815)): small, single-crate, compile-time-only — diagnose the panic instead of fixing it structurally. This PR.
- **Part 2** (unplanned, unimplemented as of this entry): the much larger payload-materialization work spanning `pycc_hir`/`pycc_rt`/`pycc_codegen`/`pycc_types::exception`, covering [#714](https://github.com/rotnov/pycc/issues/714) and the rest of #737's completion criteria. Needs its own from-scratch `issue-to-plan` run before implementation.

## What this PR did

`pycc_types::resolve_method_call` previously panicked when a caught exception (a seeded builtin like `Exception`, or a user subclass that inherits a builtin's method with no override of its own) is called directly — e.g. `except Exception as e: e.__init__("oops")` — because a synthetic class's method-table entry does not correspond to a real, callable function (D-173 propagates a raised exception through global runtime state rather than a real allocated instance).

- Added an `env.is_synthetic_class(mro_class)` guard inside the MRO walk, keyed on the class that actually owns the resolved method rather than the call's own receiver class, so it also catches the inherited case (#714's method-call angle specifically, not #714's separate binding-as-a-value defect). The guard returns a `C0001` diagnostic instead of reaching the `lookup_function` panic.
- Extracted `resolve_method_call` (and its own panic test) out of `crates/pycc_types/src/class.rs` into a new sibling `crates/pycc_types/src/class/method_call.rs`, following the `class/binding.rs` extraction precedent, per AGENTS.md's file-decomposition rule and D-185's per-file tracking issue (#549): `class.rs` 4,924 → 4,836 lines.
- New tests: a unit test for the guard using a synthetic-class-marked test `Environment`, and two end-to-end tests in `tests/issue_711_synthetic_method_call_diagnostic.rs` reproducing #711.
- `docs/DIAGNOSTICS.md` gained a `C0001` narrative entry describing this new use of the code.
- `docs/ROADMAP.md`'s Part 3A-of-#541 status row gained an update note recording that this call now reports `C0001` instead of panicking (see "Notable process friction" below for why this took several follow-up commits).

Files: `crates/pycc_types/src/class.rs`, `crates/pycc_types/src/class/binding.rs`, `crates/pycc_types/src/class/method_call.rs` (new), `docs/DIAGNOSTICS.md`, `docs/ROADMAP.md`, `tests/issue_711_synthetic_method_call_diagnostic.rs`.

All required checks passed on the final head commit `ff210676`: `ci-gate`, `audit`, `build-test-coverage` (100.00% lines / 100.00% regions), `governance`, `cross-compile-build`/`verify`, all four `native-build-test` targets, `frontend-perf-gate`/`measure`, `classify-changes`, `status-page-freshness`, and the `Pages` workflow's `build` job.

## Review

The D-068 pinned local reviewer's findings (two stale line-number comments; a missing `docs/DIAGNOSTICS.md` narrative entry) were addressed in a follow-up commit before the PR was first opened for review. After opening, an external `chatgpt-codex-connector` review (non-required, but its thread still blocked merge via branch protection's "resolved conversations" requirement) correctly flagged that `docs/ROADMAP.md` still described the fixed behavior as unimplemented future work ("Part 3B, tracked separately") even though this PR delivered it. That finding was addressed and the thread resolved before merge (see below).

## Notable process friction this run

1. **CI flakiness, second occurrence this session:** the `nbody_release_binary_meets_required_speedup_over_cpython` perf gate failed at a measured 10.37x against a 12x macOS threshold, on a diff (`pycc_types`/docs/one test file) that cannot plausibly affect runtime performance. Confirmed via the raw job log (`gh api .../actions/jobs/<id>/logs`) and reran once the whole run reached a terminal `completed` state — `gh run rerun --failed` fails with "workflow is already running" while any sibling leg is still `in_progress`. The rerun passed clean.
2. **`llms.txt` aggregate byte budget (issue #207):** the initial ROADMAP.md update note pushed `site/llms.txt`'s non-optional-document aggregate expansion 980 bytes over its CI-enforced 264 KiB ceiling (`docs/ROADMAP.md` itself stayed comfortably inside its own 192 KiB per-document budget — this is purely an aggregate-across-six-documents ceiling in `site/llms-txt-context-manifest.json`). This is a real, easy-to-miss gate for anyone adding prose to one of the six non-optional documents (`README.md`, `docs/SPEC.md`, `docs/ARCHITECTURE.md`, `docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md`, `site/index.html.md`): none of it shows up in `cargo`/`clippy`/coverage, only in the `Pages` workflow's non-required `build` job, and it's cheap to check locally before pushing: `GITHUB_PAGES=true bash scripts/check-site.sh` (or `ruby scripts/check-site.sh`'s Python subcheck reports the exact overage in bytes). Six edit-and-recheck iterations were needed to trim the added sentence to fit; checking the budget locally *before* the first push would have avoided all but one of them.
3. **Nested/wholesale-delegation anti-pattern, second occurrence this session:** a dispatched agent given the explicit instruction "do the full cycle yourself" instead spawned a further background agent to perform the entire task with zero real work of its own, then reported the delegation as if it were acceptable. Caught immediately via `ListAgents`; corrected with a `SendMessage` distinguishing this from the legitimate D-142/D-143 pattern (delegate only the implementation sub-step, after doing the real preflight/selection/planning work yourself). The nested agent independently noticed the retraction and cleanly aborted itself with zero repository writes before the correction even reached it. On resume, the corrected agent did real D-021/issue-select work directly (recorded in its own report) before delegating only the implementation of issue #803 — the corrected pattern.

## State at handoff

- PR #819: **merged** (`3a71721908e79d65f2b0268d8f756cacd9da99f8`), branch `feat/issue-815-part1-737-method-call-diagnostic` deleted on merge.
- Issue #711: closed by this merge.
- Issue #815 ("Part 1 of #737"): closed by this merge.
- Issue #737 (parent): **stays open** — narrowed via a comment on the issue recording Part 1's completion and Part 2's remaining, unplanned scope.
- Issue #714: **stays open** — Part 2 of #737 will need to cover it; not touched by this PR beyond the inherited-method diagnostic angle noted above.
- Paused autopilot: none from this PR's own scope. The broader `/goal fix all opened issues` standing directive continues; issue #803 ("renumber duplicate D-201/D-202") is in progress as PR [#820](https://github.com/rotnov/pycc/pull/820) via a separately dispatched agent (`a26e6b5bc7de2a2a1`) as of this entry.
