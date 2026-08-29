# 2026-08-29-04: issue #784 — bounded stale scratch-root cleanup (Part 4 of #779)

## Overall status

Implementation of issue #784 is complete on branch
`issue-784-stale-scratch-cleanup`, based on `origin/main` at `ffe6fe7f`
(the Part 3 merge). This entry lands in the pull request that delivers the
work (D-192). At snapshot time the only other open pull request was #833
(bigint temporary on the exception-unwinding edge) — no file overlap, but
it claims decision number D-208, which is why this work's entry is D-209.

## What the PR contains

Implemented exactly per the plan published verbatim at
<https://github.com/rotnov/pycc/issues/784#issuecomment-5462143214>:

- `crates/pycc_scratch/src/lib.rs`: every root now carries a
  `LOCK_FILE_NAME` (`.pycc-scratch.lock`) marker held under an exclusive
  OS advisory lock (`std::fs::File::lock`) for the handle's lifetime;
  lock-file creation failure removes the fresh directory and propagates;
  `Drop` closes the handle before `remove_dir_all`; acquisition failure is
  deliberately ignored (advisory-lock-free filesystems degrade to
  age-floor-only protection). New unit tests for the held/released lock
  lifecycle and the creation-failure cleanup (via a private injectable
  lock-file-name seam).
- `crates/pycc_scratch/src/sweep.rs` (new): `SweepConfig`, `SweepReport`,
  `sweep_stale_roots()` and the injectable `sweep_stale_roots_in(root,
  config, now)`. Deletes an entry only if it fully parses as
  `pycc_{category}_{pid}_{nanos}_{seq}` with non-empty category, is a real
  directory, exceeds its class's age floor (1 h locked / 24 h lockless, by
  dir mtime), and holds no live lock; budgets 10,000 entries / 512
  deletions / 250 ms. Hermetic unit tests cover the parse accept/reject
  table, every keep/delete class, each budget, and the error folds;
  deletion-success assertions are `#[cfg(not(windows))]`-gated per the
  known Defender delete-contention class.
- `src/main.rs::create_scratch`: the single-line production wiring
  (`let _ = pycc_scratch::sweep_stale_roots();`) covering both `build` and
  `run`; report discarded, no output/exit-code change.
- `tests/slice0.rs`: `#[cfg(unix)]` end-to-end test
  (`build_sweeps_a_provably_stale_scratch_root_and_spares_everything_else`)
  through a spawned `pycc build` with a redirected TMPDIR.
- Docs, same commit: new `docs/decisions/D-209-...` (lock-file liveness
  design, safety bar, accepted edge cases), dated D-201 update recording
  the supersession of its anticipated pid/nanos heuristic,
  `docs/TESTING.md` Part 4 section, `docs/CLI_SPEC.md` sweep sentence,
  regenerated `docs/decisions/README.md`.

**One deviation from the plan's affected-site inventory** (single
occurrence, not a stop condition): the consumer audit's `read_dir` grep
missed `tests/slice0.rs::init_reports_an_unavailable_cwd_without_panicking`,
which `rmdir`ed its scratch root to simulate a vanished cwd — the new lock
file makes roots non-empty, so that test now creates and `rmdir`s an empty
subdirectory instead. No other `rmdir`/`remove_dir`-on-root consumer
exists (verified by grep across `tests/`, `src/`, `crates/*/src`).

## Verification performed (local, macOS aarch64)

- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100` after the CI-order preparation builds: pass (100% lines and
  regions; see the delivering PR's CI for the authoritative run).
- `cargo test --workspace`: all green (the new sweep unit tests, lock
  lifecycle tests, and the e2e sweep test included).
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0 (only
  the known pre-existing `slice1_codegen_depth` escaped-newline notes).
- Python script unittests, `check_scratch_dir_usage.py` (no change needed
  — the sweep passes `temp_dir()` as an argument, never `.join`s it),
  `validate_agent_policies.py`, `check_ci_permissions.rb`,
  `check_roadmap_evidence.rb`, `generate_decisions_index.py --check`,
  `cargo doc --workspace --no-deps`: all exit 0.
- llms.txt aggregate budget untouched (no manifest document edited):
  270332 ≤ 270336.

## Follow-ups

- Part 5 (#785): `TMPDIR` operational guidance, the before/after
  root-count accounting, and #779's three-consecutive-runs verification
  protocol. #779 stays open until Part 5 lands.
- The windows-latest and ubuntu CI legs are where the plan's derived lock
  semantics become observed; a *lock-behavior* failure there is a design
  signal (stop and re-design), while transient Defender delete-contention
  is the already-gated flake class.

## Resume pointers

- Plan: issue #784's comment thread (published verbatim by issue-to-plan).
- Design: `docs/decisions/D-209-lock-file-liveness-and-a-bounded-stale-scratch.md`;
  D-201 carries the dated supersession update.
- Code: `crates/pycc_scratch/src/{lib.rs,sweep.rs}`,
  `src/main.rs::create_scratch`, `tests/slice0.rs` (the two `#[cfg(unix)]`
  scratch e2e tests).
- Tests-and-docs map: `docs/TESTING.md` "Scratch directories" section.
