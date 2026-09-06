# 2026-09-06 (13) — #977: string conversion of a non-dataclass, non-exception instance

Delegated implementation under `issue-implement` (D-142 dispatch), milestone v0.4.

## Overall status

The task base is `f8e9d2e3`, the `origin/main` tip when the worktree `issue-977` and branch
`feat/issue-977` were created. Re-resolved with `git fetch --prune origin` immediately before this file
was written, `origin/main` is now **`e77b4b13`** ("fix(hir): reject a `@property` getter named
`__slots__` (#980) (#983)"), one commit ahead of the base. That commit touches `docs/ROADMAP.md` (the #910
paragraph near line 213) and `docs/TYPE_SYSTEM.md` (near line 209), neither of which overlaps the
sentences this branch amends (ROADMAP lines 160, 212 and the new v0.4 row before `## v0.5`; TYPE_SYSTEM
line 262), and it claims session slot `-12-`, which is why this file is `-13-`. The orchestrating
`issue-implement` session rebased the branch onto `e77b4b13` (a clean rebase, no conflicts) and re-ran
the full local gate set from that single-writer baseline before pushing: fmt, clippy with warnings
denied, the workspace tests, `cargo llvm-cov` at 100.00% lines and regions, `cargo doc`, the
decisions-index freshness check, the roadmap-evidence checker, the `scripts/` unittest suite, both
agent validators, and `check_ci_permissions.rb` all exited 0.

The branch is pushed as `feat/issue-977`; the pull request opened from it carries `Fixes #977` and is
the delivery vehicle for this file: [PR #985](https://github.com/rotnov/pycc/pull/985), opened from
head `8f678416` with `closingIssuesReferences` = {#977}.

### Post-merge workflow runs on `e77b4b13`

Resolved with `gh run list --commit e77b4b13` immediately before writing:

| run | workflow | result |
|---|---|---|
| 34055421094 | CI | **failure** |
| 34055421089 | Pages | success |
| 34055421095 | Main history audit | success |
| 34055421099 | Status page freshness | success |

The CI failure is the known [#414](https://github.com/rotnov/pycc/issues/414) flake, not a regression:
the only failing test is `nbody_release_binary_meets_required_speedup_over_cpython`
(`tests/nbody_bench.rs:577`) in `build-test-coverage`, and `ci-gate` failed as the aggregate. No new
issue was filed. `python3 scripts/manage_ci_bypass.py status` reports branch protection matching the
documented baseline; no `[ci-bypass]` incident is open.

## What #977 delivered

The issue's program (`class C` with `__init__`; `print(C(1))`) passed `pycc check` and panicked in
`pycc build`, because `pycc_types` placed no restriction on a `print()` argument or an f-string
interpolation while `pycc_codegen`'s `to_str` renders only what two MIR rewrites (dataclass `__repr__`,
caught-builtin-exception message) keep away from a `Scalar::Instance`. Reproduction before implementing
found the same check-passes/build-panics gap for every non-dataclass instance shape, and three *runtime*
shapes worse than a panic: a user class declared under a flat-seven builtin exception name is rewritten
to `ExceptionMessage` by name and dereferences a null (plain) or misaligned (`@dataclass`) pointer, and a
user `@dataclass ExceptionGroup` printed from an `except*` binding segfaults.

Implementation, per the published plan comment on
[#977](https://github.com/rotnov/pycc/issues/977):

- New `crates/pycc_types/src/string_conversion.rs`: `StringConversionSite { PrintArgument,
  FStringInterpolation }` and `reject_unrenderable(env, ty, site)`, a fail-closed predicate deciding the
  25 builtin exception names by provenance (`Environment::is_synthetic_class`, with the flat seven still
  accepted when the shadow gate withholds seeding) and every other class by `is_dataclass`, rejecting
  `Ty::Protocol` outright. Called from `infer_expr_in`'s `FString` and `print` arms in
  `crates/pycc_types/src/expr.rs`. No `pycc_hir` class-file changes; `pycc_mir` and `pycc_codegen` gained
  comments only (the false "the type checker has already rejected non-dataclass instances" claim in
  `crates/pycc_mir/src/class.rs` is replaced with what the gate actually does).
- New ADR [D-237](../decisions/D-237-reject-string-conversion-of-a-non-dataclass-non-exception.md):
  the repro table with CPython 3.14.6 outputs, the verdict table, the two deliberate divergences from
  MIR's name-first resolution, the six rejected alternatives, and the one newly rejected program class
  that used to render (a user `@dataclass` under a non-flat builtin exception name printed directly).
- Tests: the inline unit module covers every predicate region (both sites, seeded/unseeded/shadowed
  exception names, flat and non-flat, dataclass and plain, an absent class via a hand-built
  `check_function` item, protocol, and the per-site wording); `tests/issue_977_instance_string_conversion.rs`
  drives the public CLI through every reproduced rejection shape plus positive controls (dataclass
  `P(x=1, y=2)`, flat-seven `boom` seeded and unseeded, `OSError` family `gone`, seeded `except*`).
- Docs in the same commits: `docs/TYPE_SYSTEM.md` (dataclass rendering sentence), `docs/DIAGNOSTICS.md`
  (`C0001` now also emitted by `pycc_types` at `1:1`; both message shapes), `docs/ROADMAP.md` (the
  #378 sentence, the 2026-08-01 tuple follow-up clause, a new v0.4 #977 row), the `C0001` prose in
  `crates/pycc_diag/src/explain.rs`, the regenerated decisions index, and a `docs/AGENT_RETROSPECTIVE.md`
  entry about PR #971's eighteen review rounds.

D-127 decision taken during implementation (recorded in D-237's Alternatives): when the concrete pass
rejects a program that *calls* a user class declared under a builtin exception name
(`print(ValueError(1))`), `check_all_keyed` falls through to the constraint solver, whose
`ConstraintEnvironment` has no class table and classifies the call as ``call to builtin `ValueError` ``;
D-220's solver-first merge then reports that pre-existing wording. The solver was left untouched (an
independent seam with ten `ConstraintEnvironment` literals, and the misclassification already exists at
`f8e9d2e3` for any solver-path module that instantiates a user class shadowing a builtin name). The
public-CLI tests for those shapes pin `C0001` plus the class name; the predicate's own verdict on them is
pinned directly in the unit tests.

CPython oracle note: the pinned 3.14.7 oracle was not on PATH in this session (`python3.14` is 3.14.6,
`python3` is 3.13.9); the reference outputs in D-237 were taken with 3.14.6, whose `object.__repr__` form
is unchanged.

## Known follow-ups

None of these is filed as an issue by this session (the brief and D-192's ceiling reserve filing for
the orchestrating session and `issue-select`):

- **`pycc_rt` double-render abort.** Rendering the same caught builtin exception twice
  (`except ValueError as e: print(e); print(e)`, or `print(e); print(f"{e}")`) prints the message once and
  then aborts in `pycc_rt_str_decref` (`crates/pycc_rt/src/lib.rs:934`) — the message string is decref'd
  once per render. Reproduced for the flat seven and the `OSError` family; pre-existing at `f8e9d2e3`;
  the #977 positive tests render each caught exception exactly once so they stay clear of it.
- **Solver misclassifies a builtin-named user class call as a builtin call.** See the D-127 note above:
  `crates/pycc_types/src/constraints.rs` (`is_known_callable_builtin`, around line 853) has no class
  table, so `class ValueError: ...` followed by `ValueError()` in any solver-path module reports
  ``call to builtin `ValueError` `` rather than a message about the user class. Pre-existing; a fix
  needs the class table threaded into every `ConstraintEnvironment` construction.
- **Deferred positive paths.** A user `__repr__` on a plain class and `Enum` member rendering
  (`Color.RED`) are `C0001` under D-237 until their own slices; both need MIR's
  `rewrite_instance_to_repr` widened (and, for `__repr__`, D-236's reserved-name machinery and a
  signature check), so they were kept out of this rejection-only change deliberately. The `userrepr` and
  `enum` tests pin the current rejection so either flip is visible.

## Where to resume

1. `git fetch --prune origin`; this snapshot is anchored at `origin/main` = `e77b4b13`, branch base
   `f8e9d2e3`.
2. Integrate `feat/issue-977` onto the refreshed default branch (the only expected overlap is
   `docs/ROADMAP.md`/`docs/TYPE_SYSTEM.md` at unrelated lines and the decisions index), push, open the
   pull request, watch CI, merge.
3. Then decide whether the two runtime/solver follow-ups above earn a milestone issue under D-192.
