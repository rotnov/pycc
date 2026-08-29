# Session: issue #783 — Part 3 of #779: `src/main.rs` production temp-file leaks

Date: 2026-08-29
Branch: `issue-783-main-rs-scratch`
Base: `origin/main` at `9164a806` ("Merge pull request #831")

## What the delivering pull request contains

Fixes the last two raw scratch sites in the tree — `src/main.rs`'s
production temp-file leaks (`try_build`'s `pycc_obj_<pid>.o`, `run`'s
`pycc_run_<pid>` executable) — per the plan published on
[#783](https://github.com/rotnov/pycc/issues/783) (issue comment,
2026-08-29). Part 3 of the parent
[#779](https://github.com/rotnov/pycc/issues/779); #779 stays open.

- `Cargo.toml`: `pycc_scratch` moved from `[dev-dependencies]` to
  `[dependencies]` (the move passes ci.yml's D-091/D-203 bench-manifest
  tail check — verified empirically during planning; no `Cargo.lock`
  change, as predicted, since the lock does not record dependency kind).
- `src/main.rs`: new `create_scratch(category)` helper mapping a failed
  `pycc_scratch::ScratchDir::new` to CLI_SPEC.md's exit-2
  invocation/environment class; `try_build` takes an injected
  `obj_path: &Path` instead of computing a process-keyed temp path
  (`try_build_obj_path` deleted); `main()`'s `Command::Build` arm and
  `run()` each own one `ScratchDir` that holds every temp artifact and is
  dropped — removing the files — on all exit paths, only after the linker
  (and, for `run`, the awaited child process) is done with them. Two new
  unit tests cover `create_scratch`'s success and failure regions; the
  release-isolation test now injects and reads back its own object path.
- Two deliberate user-visible behavior changes, both called out in the PR
  body: the bad-temp-dir scenario's exit class changes 1 → 2 (environment
  class, was "codegen failed"), and it now fails fast before any frontend
  work (matching `init`'s unreadable-cwd handling).
- `tests/slice0.rs`: the bad-TMPDIR build test renamed/re-asserted for
  exit 2 and the new message; a `pycc run` sibling added; and a
  `#[cfg(unix)]`-gated leak-regression e2e proving one successful
  `pycc build` + `pycc run` return a controlled temp directory to empty
  (the unix gate's Windows-flake rationale is recorded in the test).
- `scripts/check_scratch_dir_usage.py`: `ALLOWLIST` is now **empty** —
  its terminal state and #779's completeness signal for Parts 2/3;
  docstring/comments rewritten to the completed state. Its 12-test suite
  needed no changes
  (`test_the_real_repository_tree_passes_its_own_checked_in_allowlist`
  now validates every tracked `.rs` file against the empty dict).
- Docs in the same change: `docs/TESTING.md` scratch section rewritten to
  the completed state; `docs/CLI_SPEC.md` exit-2 examples gain the
  unusable-temp-directory case with its fail-fast note; `docs/ROADMAP.md`
  D-203 sentence annotated (the tolerated dev-dependency line no longer
  exists in the tree — the filter is vestigial but still armed, left per
  D-203's own two-PR retire lifecycle and D-192's filing bar, no issue
  filed); D-201 gained a dated Part 3 completion update.

## Verification performed this session (local, this worktree)

- Coverage gate, CI-equivalent sequence (`cargo build --target
  x86_64-apple-darwin -p pycc_rt`, `cargo build --workspace`,
  `cargo build --release -p pycc_rt`, then `cargo llvm-cov --workspace
  --fail-under-lines 100 --fail-under-regions 100`): pass, 100% lines and
  regions.
- `cargo test --workspace`: all green, including the three new/updated
  slice0 e2e tests and the new `create_scratch` unit tests.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` and
  `python3 -B scripts/check_scratch_dir_usage.py`: green with the empty
  allowlist; `python3 -B scripts/validate_agent_policies.py`,
  `ruby scripts/check_ci_permissions.rb`,
  `ruby scripts/check_roadmap_evidence.rb`: green.
- `cargo doc --workspace --no-deps`: succeeds.
- Manual leak verification (#779 requirement 5): with a controlled
  `TMPDIR`, one `pycc build` + one `pycc run` succeed and leave the
  directory empty; a missing `TMPDIR` yields exit 2 with the scratch
  diagnostic and no frontend output; `cargo test --bin pycc` leaves no
  `pycc_obj_<pid>.o` in the real temp directory.

## In flight / follow-ups

- #779 stays open: Part 4 (#784, bounded stale-root cleanup for
  killed/crashed processes) and Part 5 (#785, `TMPDIR` operational
  guidance + the parent's closing whole-suite verification) remain. Both
  were parallel-blocked on nothing after this merge.
- After merge: comment on #779 that Part 3 is complete, requirement 5 is
  covered, and the allowlist is empty (the parent's per-part narration
  convention).

## Where a fresh session should look to resume

The plan on #783 (issue comment) is the authoritative record of scope,
corrections, and rejected alternatives. For Part 4, start from
`crates/pycc_scratch/src/lib.rs`'s naming-format stability commitment
(`pycc_{category}_{pid}_{nanos}_{seq}`) — the stale-root sweep parses
those fields back out of directory names.
