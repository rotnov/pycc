# Session handoff: issue #782 Batch B — root-crate scratch dirs

## Status

Batch B of issue #782 (Part 2 of #779's scratch-directory migration),
implemented against `origin/main` at
`aed29cd02b47e3edb33c6e8776a1e68c0f65fc7c` (the PR #808 merge;
re-verified at implementation start) on branch
`feat/issue-782-batch-b-root-crate`, from the reviewed plan published at
issue #782's 2026-08-25T12:35 plan comment (§4 item 3 "Batch B"). This
entry lands with this PR's merge (D-192). #782 stays open — Batches C/D
(the `tests/*.rs` integration files) remain.

## What this PR delivers

- `src/project_config.rs`: all 26 raw `std::env::temp_dir().join(...)`
  call sites in its `#[cfg(test)] mod tests` migrated onto
  `pycc_scratch::ScratchDir` (RAII cleanup that survives panic unwind,
  collision-safe `pid`/`nanos`/`seq` naming). Includes the pair closed
  PR #793 had left raw: `write_new_in` requires a capture-free `fn`
  pointer, so `chmod_containing_dir_readonly_then_fail_write` cannot
  close over the test's `ScratchDir` nor recompute its
  timestamp-embedding name — resolved with a test-module
  `static CLEANUP_FAIL_DIR: OnceLock<PathBuf>` the owning test sets
  before injecting the writer.
- `src/main.rs`: its 5 test-only raw call sites migrated. Four are
  straight `ScratchDir` bindings (the two `init_tests`, the
  `release_flag_tests` helper — now returning `ScratchDir`, not
  `PathBuf` — and the release-isolation fixture dir). The fifth
  recomputed `try_build`'s production `pycc_obj_{pid}.o` path to read
  back the exact object `try_build` wrote; that formula is now the
  named production helper `try_build_obj_path()` (used by `try_build`
  and the test), so the file's raw-pattern count is exactly its two
  production sites. The production behavior is byte-identical; both
  production sites (`try_build_obj_path`, `run`'s output path) are
  deliberately left raw — #783's scope, and #783 now has one place to
  change the object path.
- `scripts/check_scratch_dir_usage.py`: ALLOWLIST ratcheted —
  `src/project_config.rs` entry removed (26 → 0, entry deleted per the
  plan's stale-entry rule), `src/main.rs` lowered 7 → 2 with the
  explanatory comment rewritten to name the two remaining production
  sites. Checker logic untouched.
- Most call-site hunks were reused from closed PR #793 (head
  `82fe31a0`, retired base): its `src/main.rs`/`src/project_config.rs`
  hunks applied cleanly onto `main` (zero drift in those files since
  its base). Dropped from #793: the Cargo.toml/Cargo.lock dev-dep hunks
  (already on `main` via #807/D-203) and its ALLOWLIST wording (kept
  counts at 3/2 for the sites this PR migrates via the two designs
  above).

## Documentation review (verified, not skipped)

- `docs/ROADMAP.md`: **no update required, none made.** Batch B is
  test-infrastructure migration inside an open issue — no behavior,
  platform-support, milestone-acceptance, or sequencing change, which
  is the roadmap's own bar for same-PR updates. (Independently, the
  D-200 llms.txt budget has ~5 bytes of headroom, so any ROADMAP
  addition would trip `scripts/check-site.sh`.)
- `docs/TESTING.md` "Scratch directories": still accurate — it
  describes Part 1's snapshot state and defers migration to #782; the
  plan (§5) updates its caveat only in the final batch's PR. Its "two
  production leaks" claim stays true (the helper extraction changes no
  behavior).
- D-201: historical statements only; nothing invalidated.
- No specification added/removed/repurposed — `docs/SPEC.md` untouched.

## Gates (all green at this snapshot, macOS local run)

- `python3 scripts/check_scratch_dir_usage.py`: passed ("no new raw
  temp_dir().join(...) call sites").
- `python3 -m unittest discover -s scripts -p 'test_check_scratch*'`:
  12 tests, OK (includes the exact-count ratchet test, so the 2/removed
  ALLOWLIST values are proven equal to the tree's real counts).
- `cargo test --workspace`: every result line ok, 0 failures (root bin
  suite 59 passed; full workspace including doc-tests exit 0).
- `cargo doc --workspace --no-deps`: green (one pre-existing
  `pycc_types` doc warning, untouched by this diff).
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby
  scripts/check_roadmap_evidence.rb`: "Roadmap evidence policy passed."
- `cargo build -p pycc` / test compile: 0 warnings.

## Empirical leak check

`$TMPDIR` held 122,743 `pycc_*` entries before the run (the 2026-08-26
disk incident's residue, still uncollected on this machine). After
`cargo test -p pycc --bin pycc`, the only new entry attributable to this
process (pid 4506) was `pycc_obj_4506.o` — exactly the #783-owned
production leak, expected. Zero directories from the migrated modules
remained. (15 transient `pycc_*_983_*` ScratchDirs also appeared during
the window — a concurrent `pycc_codegen` test run from a different
worktree, pid 983, cleaned by its own RAII.)

## Pending — NOT delivered by this PR

- Batches C/D: the remaining `tests/*.rs` ALLOWLIST entries (~370
  sites), in follow-up PRs under #782.
- #783 (production sites in `src/main.rs`), #784 (stale-root sweep),
  #785 (docs/closing verification) — untouched.
- `docs/TESTING.md`'s migration-status caveat flips only in #782's
  final batch.

## Where to resume

Pick up Batch C per the #782 plan comment §3/§4: split the remaining
`tests/*.rs` files by ALLOWLIST occurrence count into 2–3 batches,
re-running `python3 scripts/check_scratch_dir_usage.py` first to catch
count drift, and checking open PRs for file overlap before each batch.
