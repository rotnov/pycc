# 2026-08-31 — #628: LLVM target registry initialized exactly once

## Overall status

One autopilot cycle: selected issue #628 (milestone v0.4, P3) from the open-issue
inventory, implemented it, and delivered it as pull request
[#859](https://github.com/rotnov/pycc/pull/859), based on `main` at
`6cfccdb6` ("Implement traceback frames and their rendering (#707) (#858)").

Every local gate was green on the delivered head: `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
(TOTAL 100.00% lines / 100.00% regions), `cargo doc --workspace --no-deps`, and
the `scripts/` Python and Ruby suites apart from one pre-existing failure
recorded below.

## What was delivered

`compile_to_object` re-ran LLVM's entire target registration on every call. The
change puts that initialization behind a `OnceLock` and, in the same commit,
extracts the target-machine construction seam out of
`crates/pycc_codegen/src/lib.rs` (~8,800 lines) into a new
`crates/pycc_codegen/src/target_machine.rs`, discharging AGENTS.md's
decomposition obligation for the part of that file this work touched. The
broader decomposition of `lib.rs` stays tracked by #545 and was deliberately not
attempted here.

The investigation corrected the issue's own stated mechanism, and the new
module's documentation records the corrected version rather than the original
claim. inkwell 0.9.0 *does* write-lock `initialize_all`; the actual gap is that
`Target::create_target_machine` takes no lock while LLVM's `TargetRegistry.h`
registration entry points perform unconditional stores. Because those stores are
same-valued, this is a formal, sanitizer-visible race rather than a known-bad
codegen path — the guard is what the LLVM header asks for, and the code comment
says so without overclaiming an observed failure.

The regression test binds on `assert_eq!(target_init_count(), 1)` rather than on
all four concurrent compiles returning `Ok`, because the latter assertion passes
identically with and without the guard. Negative control: with the `OnceLock`
bypassed, the test fails `left: 4, right: 1`.

## Known follow-ups

- **`scripts/test_check_pages_performance_budget.rb` fails two resource-budget
  assertions locally.** Verified identical at the base commit `6cfccdb6` in a
  separate detached worktree, and unrelated to this change, which touches only
  `crates/pycc_codegen/` and `docs/TESTING.md`. Not filed as an issue: the D-192
  non-milestone open-issue ceiling (20) is in force with roughly 71 non-milestone
  issues open, so this cycle filed no new non-milestone issue.
- **`cargo fmt --all -- --check` is red repository-wide** — 436 divergences
  across 51 files at the base commit, with identical per-file counts afterward
  and none in the new file. Already tracked by #24; `fmt` is not a CI gate.
- **Local-environment note.** The `check_readme_*` Ruby checkers abort with
  `invalid byte sequence in US-ASCII` unless `LANG` and `LC_ALL` are set to a
  UTF-8 locale. This is an environment condition, not a repository defect.

## Selection notes for a fresh session

The active milestone is **v0.4** — verified against `docs/ROADMAP.md`'s own
evidence rule (its "Accept" criteria carry no "met" update note), and it is the
only open GitHub milestone. v0.4 contains no P1 issues. Its P2 set was screened
and set aside as follows, which a later cycle can reuse rather than re-derive:

- **#24** (rustfmt CI gate) — deliberately deprioritized, not excluded: it edits
  `.github/workflows/ci.yml` (D-080's two-PR digest cycle) *and* is a tree-wide
  mechanical reformat.
- **#414** (perf-gate flaking) — fleet-level runner noise, not locally
  reproducible, and partly not closable by any repository change.
- **#585** (PEP 487) — its remaining criteria are conditioned on a precondition
  D-213 deliberately deferred. Gated, not satisfied; left open.
- **#636** (D-182 tuple-literal ingress retain) — blocked by its own body on
  D-124's container refcounting. Confirmed still blocked: D-124 is `status:
  accepted`, leak-only, with no superseding decision record.

Among v0.4's P3 issues, #408 is a hard exclusion (branch-protection settings
require the repository owner), #641 is the same flaky-perf-gate class as #414,
#712 touches `ci.yml` under D-080, #729 is open-ended D-185 decomposition needing
a registration-guard contract redesign, and #706 and #639 remain reasonable
next candidates.

The staleness screen this cycle was a bounded one over the v0.4 in-scope set
rather than a premise re-verification of all 101 open issues; no issue was found
provably stale, so none was closed. Milestone triage made zero assignments: the
non-milestone pool is dominated by apparatus, CI, website, and D-185 tracker
work, none of which clearly fits v0.4's "projects & incremental" scope.
