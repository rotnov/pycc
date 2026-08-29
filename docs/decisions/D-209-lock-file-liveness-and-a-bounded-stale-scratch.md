---
id: D-209
title: "Lock-file liveness and a bounded stale scratch-root sweep"
status: accepted
---

## D-209: Lock-file liveness and a bounded stale scratch-root sweep

- Status: accepted (Part 4 of #779, issue #784)
- Context: D-201 gave every scratch directory `Drop`-based cleanup, but
  `Drop` never runs when the process dies hard — SIGKILL, power loss, a
  `kill -9`'d test runner — and #779's originating incident (70,000+ leaked
  roots, 70+ GiB) shows what an unswept temp directory accumulates. #779's
  requirement 6 asks for defense-in-depth cleanup of roots whose creator is
  provably gone. D-201's consequences note anticipated a pid/nanos design:
  parse the owning `pid` from the root's name and use `nanos` to
  disambiguate PID reuse. Two facts have overtaken that anticipation.
  First, `nanos` is no longer a trustworthy timestamp: since `f761a42a`
  (merged in #832), a pre-epoch system clock degrades it to `0` instead of
  panicking, so nothing may treat the parsed value as a plausible creation
  time. Second, comparing a root's creation time against a live process's
  *start time* — the datum PID-reuse disambiguation actually needs — is not
  portably obtainable from `std` on the five Tier-1 targets without new
  dependencies or per-platform subprocess parsing, while the toolchain pin
  (Rust 1.97.1) makes `std::fs::File::{lock, try_lock}` (stabilized 1.89)
  available: an OS advisory lock is exact, kernel-backed liveness —
  released even on SIGKILL, immune to PID reuse by construction (the lock
  belongs to the open file description, not the PID), and portable through
  `std` (flock on Linux/macOS, LockFileEx on Windows). `pycc_scratch`
  stays dependency-free per D-201.
- Decision: two coupled pieces inside `pycc_scratch`, plus a one-line
  wiring in `src/main.rs::create_scratch` (the single site both producers —
  `build` and `run` — pass through; `check`/`init` create no roots and do
  not sweep).

  **Liveness marker.** `ScratchDir::new` additionally creates
  `LOCK_FILE_NAME` (`".pycc-scratch.lock"`, a `pub const`) inside the fresh
  root, opened read+write, and holds an exclusive advisory lock on it for
  the handle's lifetime. Lock *acquisition* failure is deliberately
  ignored: on a filesystem without advisory-lock support the root degrades
  to age-floor-only protection instead of failing the user's build —
  failing a build to protect a janitor would invert priorities. Lock-file
  *creation* failure removes the just-created directory and propagates the
  error. `Drop` closes the lock handle before `remove_dir_all` (an open
  handle inside the tree can block removal of its parent on Windows).

  **The sweep** (`crates/pycc_scratch/src/sweep.rs`): `sweep_stale_roots()`
  scans `std::env::temp_dir()` with `SweepConfig::default()` and the
  current time; `sweep_stale_roots_in(root, config, now)` is the injectable
  seam tests use. The safety bar: the sweep deletes an entry only if it
  (a) fully parses as `pycc_{category}_{pid}_{nanos}_{seq}` with non-empty
  category, (b) is a real directory (not a symlink), (c) exceeds the age
  floor for its class, and (d) holds no live lock. The guarantee is
  precisely **creator-liveness**: "never delete a root whose *creator* is
  alive" holds by kernel lock for every post-Part-4 root; the 1 h locked
  floor (`min_age_locked`) covers the microsecond create-dir-to-lock
  window and mid-`Drop` races (both harmless anyway — the creator is
  deleting too); the 24 h lockless floor (`min_age_lockless`) covers roots
  from stale pre-Part-4 build artifacts during the transition. Parsed
  `pid`/`nanos`/`seq` are format validation only, never trusted as data;
  age comes from the directory's filesystem mtime — which tracks entry
  churn (create/remove/rename inside the root), not last IO (verified:
  rewriting an existing file's content does not advance the parent's
  mtime) — and liveness from a read-only `try_lock` probe on the marker.
  Budgets bound the caller's added latency: `entry_budget` 10,000
  (measured ~1.9 µs/entry, so ~20 ms full scan), `deletion_budget` 512
  (~60 ms typical; a single measured small-root `remove_dir_all` is
  ~0.12 ms), `time_budget` 250 ms hard cap checked between entries (a
  single pathological huge-tree deletion can overshoot it — accepted;
  typical pycc roots hold one object file or executable). A clean temp
  directory costs ~1–2 ms. The report is informational; the production
  caller discards it and the CLI's output, diagnostics, and exit codes are
  unchanged.

  **Concurrency tolerance**: parallel sweeps (routine when parallel test
  processes spawn pycc) are serialized per root by lock acquisition where
  it matters, and a `remove_dir_all` `NotFound` — the other sweep won the
  race — is counted as already-deleted rather than an error; the worst
  case is duplicated scan work within budgets.

  **Accepted edge cases, recorded rather than glossed:**
  - *Full-format name = pycc ownership.* A user's own directory named in
    the full format and ≥ 24 h old would be deleted. Accepted: D-201 makes
    the name format the pycc-ownership marker inside the OS temp
    directory. Scope precisely: non-UTF-8 names are rejected only when the
    lossy `U+FFFD` replacement lands in a *numeric* field and fails the
    digit parse; a `U+FFFD` confined to the category position parses
    cleanly and is owned (category is only checked non-empty), consistent
    with the ownership rule.
  - *Orphaned `run` child.* `pycc run` waits on the compiled child via
    `.status()` while holding the `ScratchDir`, and `std` opens the lock
    file close-on-exec, so the child never holds the lock. If the pycc
    parent is SIGKILLed mid-run, the orphaned child keeps executing the
    binary from the root while the lock is released, and a sweep more than
    1 h later deletes that root under the live child — the one accepted
    live-user-of-a-dead-root case. Harmless on unix (the
    unlinked-but-open executable inode stays valid, and `run_command`
    sets no `current_dir`, so the child's cwd is never the root) and
    conservative on Windows (`remove_dir_all` fails on the running exe →
    `errors` skip); requirement 6's letter holds because the root's
    *creator* is dead.
  - *Directory at the lock path* (which no pycc process ever creates): a
    platform divergence, safe on both sides. On unix the read-only open
    succeeds and `try_lock` on the directory fd acquires (observed), so
    the root reads as dead and is deleted once past the floor; on Windows
    the open fails non-`NotFound` (no backup semantics) and the root is
    conservatively skipped as an error. A unit test pins the unix outcome.
  - *Advisory-lock-free filesystems* (e.g. some NFS setups): with
    acquisition failure ignored, such roots are protected only by the 1 h
    floor, so a > 1 h-running live build with `TMPDIR` on such a
    filesystem could lose its scratch root to another pycc process
    sweeping the same temp directory. Accepted: the consequence is bounded
    to one build's temp artifacts and the probability is compound-low.
  - *Pre-Part-1 ad hoc names* (e.g. `pycc_codegen_test_{label}_{pid}`) do
    not match the full format and are never deleted — unattributable
    entries are out of the sweep's authority; Part 5 (#785) owns the
    operational cleanup guidance for those.
- Alternatives: PID liveness via `/proc`/`ps`/`tasklist` plus the
  nanos-vs-start-time PID-reuse disambiguation D-201 anticipated (rejected —
  needs per-platform subprocess parsing, start time is exactly the datum
  `std` cannot provide, `hidepid`-style environments make enumeration
  incomplete in the unsafe direction, and post-`f761a42a` `nanos` may be 0
  anyway; the lock is exact, simpler, and strictly safer). New
  dependencies (`tempfile`/`sysinfo`/`libc`/`windows-sys`) for liveness or
  cleanup (rejected — D-201's std-only decision stands, and every
  pulled-in path would need this repo's own 100% coverage). Sweeping from
  inside `ScratchDir::new` (rejected — surprise IO and nondeterminism on a
  library primitive called thousands of times per test run; the sweep is
  wired at the CLI's two producer commands instead). A persistent
  rate-limit stamp file in the temp dir (rejected — breaks the
  empty-tempdir leak-regression assertion, adds shared mutable state and
  error branches; the measured bounded scan doesn't justify it). A manual
  `pycc` sweep subcommand as the only trigger (rejected — manual triggers
  don't deliver requirement 6's defense-in-depth, and it would add public
  CLI surface; one can be layered on the same public API later).
  Rename-to-tombstone two-phase deletion (rejected — the acquired lock
  already serializes claimants; `NotFound`-tolerant deletion absorbs the
  residual race). An env kill-switch such as `PYCC_NO_SCRATCH_SWEEP`
  (rejected — undocumented surface, no demonstrated need).
- Consequences: every post-Part-4 root carries a lock file, so its
  liveness is exactly probeable and stale roots drain automatically at the
  1 h floor; pre-Part-4 format-matching lockless backlogs drain at the
  24 h floor; `build`/`run` pay at most `time_budget` (250 ms) extra only
  when a large stale backlog exists. The D-201 naming commitment now has
  its real parser, making the format doubly frozen. The pid/nanos
  liveness design D-201's consequences note anticipated is superseded by
  this entry (recorded as a dated update there). `pycc_scratch` remains
  std-only. Part 5 (#785, `TMPDIR` operational guidance and #779's
  verification protocol) builds on this sweep and stays open, as does the
  parent #779.
