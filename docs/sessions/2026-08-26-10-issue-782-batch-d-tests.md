# Session handoff: issue #782 Batch D — final batch, all remaining test files

## Status

Batch D of issue #782 (Part 2 of #779's scratch-directory migration),
implemented against `origin/main` at `4906ba58` (the Batch C merge;
re-verified at implementation start) on branch
`feat/issue-782-batch-d-tests`, from the reviewed plan published at issue
#782's 2026-08-25 plan comment (§3/§4 "Batch D"). This entry lands with
this PR's merge (D-192). **This PR completes and closes #782** — after it,
zero raw `std::env::temp_dir().join(...)` call sites remain anywhere in
test code; the only remaining raw sites in the tree are `src/main.rs`'s
two production call sites, owned by #783.

## What this PR delivers

- All 205 raw `std::env::temp_dir().join(...)` call sites in the 32
  remaining `tests/*.rs` files migrated onto `pycc_scratch::ScratchDir`
  (RAII cleanup that survives panic unwind, collision-safe
  `pid`/`nanos`/`seq` naming). Largest files: `tests/issue_378_dataclasses.rs`
  (25 sites), `tests/issue_380_protocols.rs` (22), `tests/issue_379_enum.rs`
  (21), `tests/issue_381_match.rs` (20), `tests/issue_436_classmethod_staticmethod.rs`
  (18), `tests/issue_22_execution_order.rs` (15), `tests/issue_433_super.rs`
  (15), `tests/issue_432_inheritance.rs` (11); the remaining 24 files carried
  1–8 sites each. Every site binds the handle to a named local spanning the
  whole test (`let dir = ScratchDir::new("label").expect(...)`) — never a
  chained temporary — and uses `.expect()` uniformly (no `?`/`match`, per
  D-014's region-coverage constraint). Labels are the old directory names
  minus the redundant `pycc_` prefix and pid suffix.
- Dir-returning shared helpers were migrated once at the helper and now
  return the `ScratchDir` handle itself (`tests/issue_150_zero_step_range.rs`,
  `issue_603`, `issue_702`, `issue_739`, `issue_740`), or a
  `(ScratchDir, PathBuf)` pair where callers need the source path too
  (`issue_146`/`issue_147`'s `write_case`, callers binding `let (_dir, src)`
  so the handle outlives the path's use) — returning only a path out of a
  helper would drop the handle and delete the directory at helper exit.
- `tests/issue_630_pycc_rt_build_dependency.rs`'s bespoke `TempTree`
  RAII struct (a hand-rolled subset of what `ScratchDir` provides) was
  deleted outright; its `fresh_target_root` helper now returns
  `ScratchDir` directly.
- `tests/nbody_bench.rs`'s single site joined a bare *file* name under
  `temp_dir()` with no directory; it now builds the binary inside a
  `ScratchDir` whose handle spans both timed loops (commented in-place,
  since dropping it early would delete the binary mid-benchmark).
  `tests/issue_146_bigint_release.rs`'s `built_program_peak_rss` had the
  same bare-file-path shape and gets the same treatment via `write_case`'s
  returned handle.
- Now-redundant root-level `std::fs::create_dir_all(&dir)` calls were
  dropped (`ScratchDir::new` creates the root); the 40+ manual
  `remove_dir_all` cleanups across these files were removed — `Drop` owns
  cleanup. Deliberately-nonexistent failure-path targets stay nonexistent
  children of a real scratch root. One `AsRef`-generic call site
  (`pycc_toml_release_default.rs`'s `Command::current_dir`) now passes
  `&*dir`; concrete `&Path` parameters coerce unchanged.
- `tests/conformance.rs`'s oracle-harness site migrated at its helper with
  behavior unchanged when the pinned python3.14 oracle is absent (the
  `#[ignore]` attributes and panic path are untouched; `Drop` now cleans
  up where the old code leaked on panic).
- `tests/issue_769_optional_narrowing.rs`'s obsolete blocker comment block
  (D-091/f79bb2b5/f9231e2f, already superseded by D-203's tolerance and
  resolved by this migration) was removed with the site it described.
  `issue_150`/`issue_767`/`issue_790` carried no in-file blocker comments.
- `scripts/check_scratch_dir_usage.py`: ALLOWLIST reduced to its final
  state — all 32 tests entries removed (205 tracked sites → 0) together
  with the three attached comment blocks describing now-migrated files
  (above `issue_150`, `issue_769`, `issue_790`). Exactly one entry
  remains: `"src/main.rs": 2` with its existing #783 comment. The module
  docstring and the ALLOWLIST header comment were updated where they
  stated Part 2 was still pending; checker logic untouched, and the
  allowlist-empty completeness signal for #779 Parts 2/3 stays as written.
- No file decomposition: pure token-level substitution creates no
  cohesion-driven extraction boundary (same recorded reasoning as
  Batch C); `tests/slice1_codegen_depth.rs`'s 3 pre-existing
  escaped-newline warnings sit in string literals this diff does not
  rewrite and were left alone.

## Documentation review (verified, not skipped)

- `docs/TESTING.md` "Scratch directories": the migration-status caveat
  paragraph flipped to the completed state, per the plan §5 decision that
  this happens in the final batch's PR — the section now records that
  every ALLOWLIST-tracked test call site uses `ScratchDir` and only
  `src/main.rs`'s two production sites (#783) remain raw. (This also
  replaced the "most of the tree's scratch-directory handling still
  predates this section" sentence Batch C's log flagged as borderline.)
- `docs/decisions/D-201-shared-pycc-scratch-crate-and-lint-gate-for.md`:
  dated addendum appended (the same annotation pattern as the existing
  D-203 update block and the #807 precedent) recording Part 2's
  completion; no accepted content rewritten.
- `docs/ROADMAP.md`: **no update required, none made.** Batch D is
  test-infrastructure migration closing an open issue — no behavior,
  platform-support, milestone-acceptance, or sequencing change. Its only
  scratch-related sentence (the D-203 bench-manifest tolerance note on
  the quality-gates row) stays accurate. Independently, the D-200
  llms.txt budget has ~5 bytes of headroom, so ROADMAP growth would trip
  `scripts/check-site.sh`.
- Swept `docs/` (excluding `docs/sessions/` history and
  `docs/AGENT_RETROSPECTIVE.md`) for `temp_dir`/`ScratchDir`/`#782`
  statements made false by this PR: `docs/ARCHITECTURE.md`'s crate-table
  row, `docs/DELIVERY_PLAN.md`'s #779 decomposition paragraph, and
  D-203's historical narrative all remain accurate as written.
- No specification added/removed/repurposed — `docs/SPEC.md` untouched.

## Gates (all green at this snapshot, macOS local run)

- `python3 scripts/check_scratch_dir_usage.py`: passed with the 32
  entries removed, proving 0 raw sites remain in those files.
- `python3 -m unittest discover -s scripts -p 'test_check_scratch*'`:
  12 tests, OK (includes the exact-count ratchet test, so the sole
  remaining ALLOWLIST value is proven equal to the tree's real count).
- `cargo test --workspace`: 65 result lines, all ok — 3,837 passed,
  0 failed.
- Warning check: 0 new compiler warnings; the workspace build's only
  warnings are the 3 pre-existing escaped-newline warnings in
  `tests/slice1_codegen_depth.rs` string literals this diff does not
  touch.
- `cargo doc --workspace --no-deps`: green (two pre-existing private-item
  link warnings, `pycc_types` and `pycc_scratch`, both in files this diff
  does not touch).
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby
  scripts/check_roadmap_evidence.rb`: "Roadmap evidence policy passed."

## Empirical leak check

The shared `$TMPDIR` was contaminated mid-check by a concurrent agent
session running the *unmigrated* suite from a different worktree
(`pycc-worktrees/issue-809-optional-float-bool` — old-style
`pycc_702_<tag>_<pid>` names with no nanos/seq appeared from binaries
this session never ran), so the check was redone under an isolated
per-run `TMPDIR` for clean attribution:

- 8 largest migrated binaries (`issue_378`, `issue_380`, `issue_379`,
  `issue_381`, `issue_436`, `issue_22`, `issue_433`, `issue_432`;
  147 tests): the isolated TMPDIR ended with **76 entries, every one a
  `pycc_obj_<pid>.o` file** from `src/main.rs`'s #783-owned `try_build`
  production site (each pid a `pycc` CLI child the tests spawned).
  **Zero scratch directories leaked** — every migrated `ScratchDir`
  cleaned up via RAII.
- Full `cargo test --workspace` under a second isolated TMPDIR: leftovers
  were exclusively `pycc_obj_<pid>.o` and `pycc_run_<pid>` entries — the
  two #783 production sites — and nothing else; zero test-scratch
  directories from any migrated site, in this batch or the previous ones.
- The machine's shared `$TMPDIR` backlog (131k+ `pycc_*` entries at this
  snapshot, up from Batch C's 127k via the concurrent unmigrated-suite
  runs) remains #784's motivation.

## Pending — NOT delivered by this PR

- #783 (the two production sites in `src/main.rs`, reconfirmed by the
  leak check above), #784 (bounded stale-root sweep), #785
  (operational `TMPDIR` guidance + closing verification) — untouched,
  tracked under the parent #779.

## Where to resume

#782 closes with this PR's merge. Next in the #779 sequence: #783
(rewrite `try_build`'s `pycc_obj_*` object path and `run`'s `pycc_run_*`
executable path in `src/main.rs` onto `ScratchDir`), then #784, then
#785. The `src/main.rs` ALLOWLIST entry and its comment in
`scripts/check_scratch_dir_usage.py` are the mechanical tracker: #783's
PR removes it, leaving the ALLOWLIST empty — #779 Parts 2/3's recorded
completeness signal.
