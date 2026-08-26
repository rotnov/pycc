# Session handoff: issue #782 Batch C — four largest integration-test files

## Status

Batch C of issue #782 (Part 2 of #779's scratch-directory migration),
implemented against `origin/main` at `6582c880` (the PR #811 merge, which
delivered Batch B; re-verified at implementation start) on branch
`feat/issue-782-batch-c-tests`, from the reviewed plan published at issue
#782's 2026-08-25 plan comment (§4 "Batch C"). This entry lands with this
PR's merge (D-192). #782 stays open — Batch D (the remaining smaller
`tests/*.rs` files) remains.

## What this PR delivers

- All 192 raw `std::env::temp_dir().join(...)` call sites in the four
  largest integration-test files migrated onto `pycc_scratch::ScratchDir`
  (RAII cleanup that survives panic unwind, collision-safe
  `pid`/`nanos`/`seq` naming): `tests/slice0.rs` (62 sites),
  `tests/issue_382_exceptions.rs` (56), `tests/issue_542_except_star.rs`
  (41), `tests/issue_435_isinstance_issubclass.rs` (33). Every site binds
  the handle to a named local spanning the whole test
  (`let dir = ScratchDir::new("label").expect(...)`) — never a chained
  temporary, which would drop and delete the directory immediately — and
  uses `.expect()` uniformly on the `io::Result` (no `?`/`match`, per
  D-014's region-coverage constraint). Labels are the old directory names
  minus the redundant `pycc_` prefix and pid suffix.
- Now-redundant `std::fs::create_dir_all(&dir)` calls on the scratch root
  were dropped (`ScratchDir::new` creates it); `create_dir_all` on nested
  subpaths below the root (`dir.join("src")`, `dir.join("pycc.toml")` in
  slice0's init tests) is kept. The two manual
  `let _ = std::fs::remove_dir_all(&dir);` cleanups in
  `tests/issue_382_exceptions.rs` were removed — `Drop` now owns cleanup.
- `tests/issue_382_exceptions.rs`'s `assert_raw_codegen_error` helper was
  migrated once at the helper (its per-call label interpolates the static
  case name: `ScratchDir::new(&format!("382_raw_codegen_{name}"))`),
  matching `src/main.rs`'s Batch-B `release_flag_tests` idiom, instead of
  touching each caller.
- Eleven slice0 call sites that pass the directory to an
  `AsRef`-generic parameter (`Command::current_dir` x10, `Command::arg`
  x1 in the deleted-cwd test) now pass `&*dir` — `ScratchDir` derefs to
  `Path` but `&ScratchDir` does not itself implement `AsRef<Path>`;
  concrete `&Path` parameters (`write_fixture`, `build_and_run`) coerce
  unchanged. Deliberately-nonexistent child paths (e.g. slice0's
  `does_not_exist.py`, `does_not_exist_dir`, `missing_tmp`) stay
  nonexistent children of a real scratch root, with no `create_dir_all`
  added. The `init_reports_an_unavailable_cwd_without_panicking` test
  still `rmdir`s the (empty) scratch root inside its shell wrapper;
  `Drop`'s `remove_dir_all` on the already-removed path is ignored by
  design.
- `scripts/check_scratch_dir_usage.py`: ALLOWLIST ratcheted — the four
  entries for these files removed outright (192 tracked sites → 0; a
  missing key means allowed=0). None of the four carried an attached
  comment block; the comments describing surviving Batch-D entries
  (`tests/issue_150_zero_step_range.rs`,
  `tests/issue_769_optional_narrowing.rs`,
  `tests/issue_790_typing_type_checking.rs`, `src/main.rs`) are
  untouched. Checker logic untouched. 207 tracked raw sites remain in
  the ALLOWLIST (Batch D scope, plus `src/main.rs`'s two #783-owned
  production sites).
- Per the #782 plan §3.1's recorded no-change decision, none of the four
  files was decomposed into submodules despite `tests/slice0.rs` (1,918
  lines) and `tests/issue_382_exceptions.rs` (1,385) exceeding the
  ~1,000-line guidance: a pure token-level substitution across a file
  creates no cohesion-driven extraction boundary, and no unrelated code
  was rewritten.

## Documentation review (verified, not skipped)

- `docs/ROADMAP.md`: **no update required, none made.** Batch C is
  test-infrastructure migration inside an open issue — no behavior,
  platform-support, milestone-acceptance, or sequencing change. Its only
  scratch-related sentence (the D-203 bench-manifest tolerance note on
  the quality-gates row) is unaffected. Independently, the D-200 llms.txt
  budget has ~5 bytes of headroom, so any ROADMAP growth would trip
  `scripts/check-site.sh`.
- `docs/TESTING.md` "Scratch directories": **no edit**, per the plan §5
  decision that the migration-status caveat flips only in the final
  batch's PR. Verified the section stays literally accurate: "Migrating
  the remaining ALLOWLIST-tracked call sites … is Part 2 (#782)" still
  holds while Batch D is open, as does the two-production-leaks claim.
  One sentence is now borderline rather than wrong — "most of the tree's
  scratch-directory handling still predates this section" describes
  roughly half after this batch (207 raw sites remain of the ~384
  snapshot) — noted here so Batch D's rewrite replaces it rather than
  merely appending.
- D-201/D-203: historical statements only; nothing invalidated.
- No specification added/removed/repurposed — `docs/SPEC.md` untouched.

## Gates (all green at this snapshot, macOS local run)

- `python3 scripts/check_scratch_dir_usage.py`: passed ("no new raw
  temp_dir().join(...) call sites") with the four entries removed,
  proving 0 raw sites remain in those files.
- `python3 -m unittest discover -s scripts -p 'test_check_scratch*'`:
  12 tests, OK (includes the exact-count ratchet test, so every
  remaining ALLOWLIST value is proven equal to the tree's real count).
- `cargo test --workspace`: every result line ok, 0 failures. The four
  migrated binaries: slice0 80 passed, issue_382_exceptions 61,
  issue_542_except_star 41, issue_435_isinstance_issubclass 33.
- Warning check: the four migrated files compile with 0 warnings. The
  workspace run's only warnings (3x "multiple lines skipped by escaped
  newline", `tests/slice1_codegen_depth.rs`) are pre-existing in a file
  this diff does not touch.
- `cargo doc --workspace --no-deps`: green (one pre-existing
  `pycc_types` doc warning, untouched by this diff).
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby
  scripts/check_roadmap_evidence.rb`: "Roadmap evidence policy passed."

## Empirical leak check

`$TMPDIR` held 127,777 `pycc_*` entries before the run (the 2026-08-26
disk incident's residue, still uncollected on this machine). After
`cargo test --test slice0 --test issue_382_exceptions --test
issue_542_except_star --test issue_435_isinstance_issubclass`, a full
before/after listing diff showed exactly 15 new entries and 0 removed:
12 `pycc_obj_<pid>.o` files and 3 `pycc_run_<pid>` executables, each pid
belonging to a `pycc` CLI child process the tests spawned — exactly the
two #783-owned production call sites in `src/main.rs`, pre-existing and
out of this batch's scope. **Zero** entries from the migrated test sites
(`pycc_e2e_*`, `pycc_382_*`, `pycc_542_*`, `pycc_435_*`) leaked; every
migrated `ScratchDir` cleaned up via RAII.

## Pending — NOT delivered by this PR

- Batch D: the remaining `tests/*.rs` ALLOWLIST entries (205 sites
  across 32 files), in a follow-up PR under #782, including the
  `docs/TESTING.md` caveat flip per plan §5.
- #783 (production sites in `src/main.rs` — the `pycc_obj_*`/`pycc_run_*`
  leaks reconfirmed by the leak check above), #784 (stale-root sweep,
  for which this machine's 127k-entry backlog is direct motivation),
  #785 (docs/closing verification) — untouched.

## Where to resume

Pick up Batch D per the #782 plan comment §3/§4: migrate the remaining
ALLOWLIST files, re-running `python3 scripts/check_scratch_dir_usage.py`
first to catch count drift, checking open PRs for file overlap, and
flipping `docs/TESTING.md`'s migration-status caveat in that final
batch's PR.
