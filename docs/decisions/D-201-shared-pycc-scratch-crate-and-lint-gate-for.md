---
id: D-201
title: "Shared `pycc_scratch` crate and lint gate for scratch-directory lifecycle"
status: accepted
---

## D-201: Shared `pycc_scratch` crate and lint gate for scratch-directory lifecycle

- Status: accepted (Part 1 of #779, issue #781)
- Context: issue #779 found ~384 ad hoc `std::env::temp_dir().join(...)` call
  sites across 36 tracked test files, plus two unconditional production leaks
  in `src/main.rs` (`try_build`'s `pycc_obj_*` object file, `run`'s
  `pycc_run_*` executable) — together responsible for a real disk-fill
  incident (70,000+ leaked temp directories, 70+ GiB). Two independent
  defects caused this: no shared `Drop`-based cleanup, so a panic partway
  through a test (or an early return in production code) skipped a manual
  `remove_dir_all`; and no collision-safe naming, so two call sites in the
  same process could pick the same directory name and race. The existing
  `crates/pycc_codegen/src/tests_support.rs::TempTestDir`/`tempfile_dir`
  pattern gets the `Drop` half right (normal exit and panic unwind both
  covered by its own tests) but not the naming half: its
  `pycc_codegen_test_{label}_{pid}` scheme uses only the process ID and a
  caller-supplied label, so two call sites in the same test binary sharing a
  label collide on the same directory today. No workspace crate declares
  `tempfile` as a direct dependency (only transitively, via `rand` 0.8.7
  pulled in elsewhere), and no existing repository check scans `.rs` source
  text for a banned call shape and fails CI on a match.
- Decision: add a new workspace member crate, `crates/pycc_scratch`, whose
  only public API is `ScratchDir` — an RAII handle that derefs to `Path` and
  removes its directory tree on `Drop`, including during panic unwinding.
  `ScratchDir::new(category: &str) -> io::Result<Self>` creates a directory
  named `pycc_{category}_{pid}_{nanos}_{seq}` under `std::env::temp_dir()`,
  where `pid` is `std::process::id()`, `nanos` is the **full epoch
  nanosecond count** (a `u128`, not just the sub-second remainder, so two
  process restarts a few seconds apart that reuse the same PID still get
  distinguishable names), and `seq` is a per-process `AtomicU64` counter
  that rules out any same-process collision even when two calls land on the
  same nanosecond tick. All three fields come from `std`; the crate adds no
  external dependency. `ScratchDir::new` does not retry on collision:
  `std::fs::create_dir` already fails atomically if the path exists, and
  with `pid`/`nanos`/`seq` a same-process collision is impossible (the `seq`
  counter alone rules it out) and a cross-process collision is not a
  reachable case in any test — a retry loop's exhaustion arm would be an
  untestable branch under D-014's 100%-region coverage gate. The crate name
  is deliberately not `pycc_testkit`: that name is reserved for a materially
  different, deliberately deferred v1.0-scale conformance harness
  (D-018/D-037/D-085) and reusing it here would misrepresent scope. Part 1
  registers `pycc_scratch` in the workspace `[members]` list; it is not a
  dependency of the `pycc` binary package itself, since `src/main.rs` has no
  consumer of it until Part 3 (#783) rewrites `try_build`/`run` to use it.
  It does become a `[dev-dependencies]` entry of `crates/pycc_codegen`,
  incidentally: this pull request landed via a rebase onto a refreshed
  `main`, and resolving that rebase's conflicts in `crates/pycc_codegen`'s
  own test helper (`tests_support.rs`'s `TempTestDir`/`tempfile_dir`) meant
  retiring it directly onto `ScratchDir` rather than reconciling two
  divergent copies of code already slated for deletion. `tests_support.rs`
  is removed entirely and every call site in `crates/pycc_codegen/src/tests.rs`
  and `bigint_rc.rs` now calls `pycc_scratch::ScratchDir::new(...)`. This
  narrows Part 2 (#782)'s remaining migration scope by one crate but does
  not close it: the ~384-call-site count tracked by the `ALLOWLIST` below
  covers other crates' raw `temp_dir().join(...)` sites, which
  `crates/pycc_codegen` never used (it already had its own wrapper).

  Alongside the crate, `scripts/check_scratch_dir_usage.py` (self-tested by
  `scripts/test_check_scratch_dir_usage.py`, wired into the `governance` CI
  job) rejects new raw `temp_dir().join(...)` call sites. It scans every
  tracked `.rs` file for that literal pattern (whitespace/line-break
  tolerant) and fails if a file has more occurrences than a checked-in
  snapshot allowlist records for it. The allowlist maps each already-violating
  file to its **exact occurrence count** at Part 1's merge commit, not a
  bare filename list: a bare filename list would let an already-listed file
  accumulate brand-new raw calls undetected, which defeats the actual goal
  of stopping the leak from getting worse. A file not in the allowlist is
  held to the new rule from day one (any occurrence is a violation); a
  listed file's count is intended to only stay the same or decrease on any
  later pull request. That intent is a review convention today, not a
  mechanically enforced property: `check_scratch_dir_usage.py` compares the
  current tree's occurrence count against the checked-in `ALLOWLIST` value,
  but has no way to see a prior commit's `ALLOWLIST` entry, so a pull
  request that both adds new raw call sites to an already-listed file and
  raises that file's `ALLOWLIST` count to match would still pass. The D-068
  pinned reviewer pass on every pull request is the intended backstop for
  this gap, the same way it is for the textual-pattern-match limitation
  noted below; a merge-base-comparison check may be added later if this
  proves insufficient in practice. The allowlist reaching empty is the
  completeness signal for
  closing out #779's Parts 2 (#782) and 3 (#783). `crates/pycc_scratch/src/lib.rs`
  itself is exempt unconditionally, as the one legitimate implementation
  site.

  The snapshot was taken once, but this pull request landed via a rebase
  onto a refreshed `main` that had, in the interim, merged issue #150's fix
  (a `tests/` integration test using the raw `temp_dir().join(...)` pattern)
  after the snapshot was generated but before this PR's own merge commit.
  Because the snapshot's definition is "every file containing the pattern at
  the commit where the gate takes effect" -- that commit is the rebased
  merge, not the pre-rebase branch tip -- `tests/issue_150_zero_step_range.rs`
  was added to `ALLOWLIST` with count 1 to fulfill that definition rather
  than breach the allowlist's one-time-snapshot property. Migrating it is
  out of scope here: doing so would require re-adding `pycc_scratch` as a
  root `[dev-dependencies]` entry, which this same PR's `f79bb2b5` tried and
  reverted because it trips D-091's bench-manifest fingerprint gate in
  `frontend-perf-measure` -- the identical blocker tracked against #782
  Batch B (PR #793). It stays tracked under #782's Part 2 migration scope.

  Update (2026-08-26): D-203 has since narrowed the D-091 bench-manifest
  tail check to tolerate exactly the
  `pycc_scratch = { path = "crates/pycc_scratch" }` line, and the D-203
  activation pull request (Part 2 of #800) re-added the root
  `pycc_scratch` dev-dependency under that tolerance. The blocker
  described here is resolved; the migration itself proceeds under #782.

  Update (2026-08-26, later the same day): Part 2 (#782) has migrated every
  `ALLOWLIST`-tracked test call site — including
  `tests/issue_150_zero_step_range.rs` and the other files the paragraphs
  above deferred — onto `pycc_scratch::ScratchDir`, except one:
  `tests/quick_start.rs`'s single site stays raw because the site's
  versioned evidence-hero contract (`docs/WEBSITE.md`, enforced by
  `scripts/check-site.sh` against `site/evidence-heroes.json`) pins that
  file's exact bytes to the reviewed evidence commit, so migrating it
  requires the full re-attestation ceremony (new evidence commit, accepted
  CI run, updated checker allowlist and site projections) rather than an
  ordinary edit. The allowlist holds exactly two entries: that
  evidence-pinned site, and `src/main.rs`'s two production call sites,
  owned by Part 3 (#783). #782 stays open for the pinned site.

  Update (2026-08-28): Part 2 (#782) is complete. The pinned
  `tests/quick_start.rs` site migrated via the evidence-hero
  re-attestation its byte pin required — a new evidence commit, its green
  CI run recorded as the accepted attestation, and the reviewed
  `LANDING_ALLOWLIST`, manifest, and site projections rotated together in
  the same pull request. The allowlist now holds exactly one entry:
  `src/main.rs`'s two production call sites, owned by Part 3 (#783).

  Update (2026-08-29): Part 3 (#783) is complete. `pycc_scratch` moved
  from the root manifest's `[dev-dependencies]` to `[dependencies]` (the
  root dependency this decision anticipated Part 3 adding; D-203's tail
  filter tolerates the removal symmetrically), and `src/main.rs`'s two
  production call sites now place their artifacts inside caller-owned
  `ScratchDir`s — `main()`'s `Command::Build` arm and `run()` each create
  one and inject the temp-object path into `try_build`, and an unusable
  temp directory fails fast as a CLI_SPEC.md exit-2 environment error.
  The `ALLOWLIST` is now **empty**, this decision's completeness signal
  for #779's Parts 2/3. Parts 4 (#784, bounded stale-root cleanup) and 5
  (#785, `TMPDIR` operational guidance) remain open under #779.

  Update (2026-08-29, later the same day): Part 4 (#784) is complete —
  D-209 holds the design. One correction to this entry's consequences
  note: the anticipated pid/nanos liveness heuristic ("recover the owning
  `pid` and use `nanos` to defeat PID reuse") is superseded. `nanos` is no
  longer a trustworthy timestamp (`f761a42a` degrades it to `0` under a
  pre-epoch clock), and the PID-reuse comparison would need a live
  process's start time, which `std` cannot portably provide — while
  `std::fs::File::{lock, try_lock}` (stabilized in 1.89, available under
  the 1.97.1 toolchain pin) gives exact kernel-backed liveness that is
  immune to PID reuse and released even on SIGKILL. Every root now
  carries a locked `.pycc-scratch.lock` marker, and the sweep
  (`crates/pycc_scratch/src/sweep.rs`) parses the name format for
  *ownership validation only*, taking staleness from filesystem mtime and
  liveness from the lock. The naming-format stability commitment stands
  unchanged — the sweep is exactly the anticipated parsing consumer, and
  the crate remains std-only. Part 5 (#785) remains open under #779.

  **Known, accepted scope limitation**: the lint is a textual pattern match,
  not a data-flow analysis. A caller that splits the expression across a
  binding (`let dir = std::env::temp_dir(); ... dir.join(...)`) evades it
  even though it has the same effect as the banned call. This is a
  deliberate, conspicuous evasion, not something ordinary refactoring
  produces by accident, and is out of proportion to the defect being fixed;
  the D-068 pinned reviewer pass on every pull request is the backstop for
  it.
- Alternatives: add the `tempfile` crate (rejected — it solves a broader
  problem than this narrow need, and every code path it pulls in would need
  100% branch coverage from this repository's own tests rather than
  `tempfile`'s own upstream suite; a ~100-line stdlib-only implementation is
  easier to review and has no supply-chain surface). Add `rand` as a direct
  dependency for the uniqueness suffix (rejected for the same reason —
  `nanos + seq` is already collision-safe without it). Reuse
  `tests_support.rs`'s `TempTestDir` in place, exported more broadly,
  instead of a new crate (rejected — it is `pub(crate)`, not a reusable
  public API, its naming scheme is the exact collision defect being fixed,
  and it does nothing for the ~26 other test files that don't already
  depend on `pycc_codegen`). Make `ScratchDir::new` synchronize on a
  process-wide directory registry or lock file instead of pure name
  uniqueness (rejected — `create_dir` failing on an existing path is
  already an atomic OS-level collision check; no additional coordination is
  needed for uniqueness, as opposed to Part 4's separate concern of
  detecting *other* processes' stale roots). Ship the lint as
  warning-only/non-blocking in Part 1 and promote it to a required gate
  only once Parts 2–4 close out (rejected in favor of the snapshot
  allowlist — a blocking gate from Part 1's own merge, tolerant of the
  pre-existing backlog but not of new growth, gives Parts 2/3 immediate,
  mechanical, per-PR proof of progress and closes the actually load-bearing
  gap — new code adding to the leak — from day one).
- Consequences: every future scratch directory in this repository's test
  code or production code goes through `pycc_scratch::ScratchDir`, enforced
  mechanically rather than by convention alone. The `pycc_{category}_{pid}_{nanos}_{seq}`
  field order is a stability commitment: Part 4's (#784's) bounded
  stale-root cleanup needs to recover the owning `pid` (and use `nanos` to
  defeat PID reuse) from a directory name alone, without a live handle, so
  changing this format is a breaking change for that consumer. Part 1
  changes zero `src/main.rs` behavior and, of the ~384 raw call sites
  `ALLOWLIST` tracks, migrates none — it only adds the crate and the lint.
  It does migrate `crates/pycc_codegen`'s separate, already-wrapped
  `tests_support.rs` helper onto `ScratchDir`, as described above, since
  that crate's own tests were never part of the raw-pattern backlog the
  lint tracks. Migrating the ~384 `ALLOWLIST`-tracked sites in other crates
  (Part 2), fixing the two production leaks (Part 3), bounded stale-root
  cleanup (Part 4), and operational `TMPDIR` guidance (Part 5) are tracked
  as separate, dependency-ordered follow-up issues under the parent #779.
  `pycc_testkit` remains reserved and untouched by this decision — see
  D-085's own consequences note for that.
