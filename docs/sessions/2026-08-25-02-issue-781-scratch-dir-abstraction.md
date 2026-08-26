# Session handoff: issue #781 — shared scratch-directory abstraction + repo lint gate

Status: implementation complete on branch `feat/issue-781-scratch-dir`, PR opened against `main`, **not yet merged**. A later session in this same overall task rebased the branch onto a refreshed `main` (new merge-base `af2384fa`, superseding the `7b3c4301` baseline this entry was originally planned against), re-ran every gate below against the rebased tip, and ran the previously-outstanding D-068 pinned reviewer pass — see the two new subsections below for what changed.

## What this session did

Implemented issue [#781](https://github.com/rotnov/pycc/issues/781) (Part 1 of
5 of [#779](https://github.com/rotnov/pycc/issues/779), the disk-fill
incident: 70,000+ leaked temp directories, 70+ GiB) per the full
implementation-ready plan published as
[a comment on #781](https://github.com/rotnov/pycc/issues/781#issuecomment-5406783460),
planned against baseline `origin/main` tip `7b3c4301` after 4 rounds of
adversarial review. That baseline was still current when this session's own
D-021 preflight fetched and re-checked it.

Delivered, faithfully following the plan:

- New workspace member crate `crates/pycc_scratch`
  (`crates/pycc_scratch/src/lib.rs`), with `ScratchDir` as its only public
  API: an RAII handle that derefs to `Path` and removes its directory tree
  on `Drop`, including during panic unwinding. Naming scheme
  `pycc_{category}_{pid}_{nanos}_{seq}` (full epoch nanoseconds + a
  per-process atomic counter) — collision-safe within a process and, in
  practice, across process restarts. No external dependency; `tempfile`/
  `rand` were both considered and rejected (see D-201).
- 4 regression tests in `pycc_scratch`'s own `#[cfg(test)]` module: normal
  `Drop`, panic-unwind `Drop`, 32-thread concurrent creation with the same
  `category` (proven to fail against a PID-only naming scheme, verified
  manually by temporarily reverting the naming scheme and re-running this
  test before restoring it), and a deterministic `Err`-propagation test
  (NUL-byte `category`, which makes `std::fs::create_dir` fail portably).
  `cargo llvm-cov -p pycc_scratch --fail-under-lines 100 --fail-under-regions 100`
  is 100%/100%.
- `scripts/check_scratch_dir_usage.py` (self-tested by
  `scripts/test_check_scratch_dir_usage.py`, 8 mutation-test cases), wired
  into the `governance` CI job (`.github/workflows/ci.yml`). Rejects any
  tracked `.rs` file with more raw `temp_dir().join(...)` occurrences than a
  checked-in snapshot allowlist records for it — a per-file *count* map (not
  a bare filename list), generated mechanically at this implementation
  commit: 384 occurrences across 36 files, matching the plan's corrected
  baseline count exactly. `crates/pycc_scratch/src/lib.rs` itself is exempt
  unconditionally.
- `docs/decisions/D-201-shared-pycc-scratch-crate-and-lint-gate-for.md`
  (new; D-199 was already double-claimed by concurrent open PRs #778/#780 at
  planning time, so this used the next free number instead), plus a short
  "unrelated-scope" note added to D-085 clarifying `pycc_scratch` is not
  `pycc_testkit`. `docs/decisions/README.md` regenerated and passes
  `--check`.
- `docs/TESTING.md` gained a new "Scratch directories (issue #781, Part 1 of
  #779)" subsection, explicit that Part 1 does not migrate any of the ~384
  existing call sites or `src/main.rs`'s two production leaks — that is
  Parts 2 ([#782](https://github.com/rotnov/pycc/issues/782)) and 3
  ([#783](https://github.com/rotnov/pycc/issues/783)).
- `docs/DELIVERY_PLAN.md`'s v0.3 section gained one paragraph noting #779 is
  tracked as 5 dependency-ordered sub-issues, Part 1 first, with all 5
  issue numbers linked. `docs/ROADMAP.md` needed no change — Part 1 ships
  infrastructure with no v0.3 acceptance-criterion status change, matching
  the plan's own reasoning, reconfirmed against the actual current v0.3
  section rather than assumed.

## Gates run locally, all green

- `cargo doc --workspace --no-deps` (D-021 step 5, run before any code
  change) — succeeded.
- `cargo build --workspace` — succeeded.
- `cargo test --workspace` — 0 failures (1281+ tests across every crate;
  the diagnostic-rejection `error[...]` lines in the raw output are expected
  assertions inside diagnostics tests, not real failures).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  — **100.00% lines, 100.00% regions, 0 missed**, across all 49 measured
  files including the new `crates/pycc_scratch/src/lib.rs` (154/154 regions,
  86/86 lines). Full per-file table captured in this session's own tool
  output.
- `python3 -B scripts/check_scratch_dir_usage.py` and
  `python3 -B scripts/test_check_scratch_dir_usage.py` — both pass.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` (the
  `governance` job's own first step) — 947 tests, 0 failures.
- `ruby scripts/check_ci_permissions.rb` — passes (workflow YAML only added
  one `run:` step with no permission/trigger changes).
- `ruby scripts/check_roadmap_evidence.rb` and
  `ruby scripts/test_check_roadmap_evidence.rb` — both pass under a UTF-8
  locale (`LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8`; this worktree's default
  shell locale is `C`/`US-ASCII`, which makes the Ruby script raise
  `invalid byte sequence in US-ASCII` on an unrelated Unicode character
  inside `docs/ROADMAP.md` — a local-environment artifact, not a defect
  introduced by this change, and not present in CI's own locale).
- `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check`
  — up to date.

## Rebase onto refreshed `main` (later session)

`origin/main` moved after this PR opened; a later session in this same
task rebased `feat/issue-781-scratch-dir` onto the new tip (`af2384fa`) and
resolved the resulting conflicts, including a large hand-merge of
`crates/pycc_codegen/src/tests.rs` and `bigint_rc.rs`. That conflict
resolution retired `crates/pycc_codegen/src/tests_support.rs`'s
`TempTestDir`/`tempfile_dir` wrapper directly onto `pycc_scratch::ScratchDir`
rather than reconciling two divergent copies of code already slated for
deletion — see D-201's updated text and `docs/TESTING.md`'s "Scratch
directories" section for the resulting scope change. Two dangling
`tempfile_dir(...)` calls left over from an incomplete first pass at that
conflict resolution (in two walrus-operator test functions added
independently on `main` by #774, which the original migration commit never
saw) were found and fixed. Every gate in the section above was re-run
against the rebased tip and passed again, including a fresh (non-cached)
`cargo clippy --workspace --all-targets -- -D warnings` and
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
(100.00%/100.00%, 0 missed).

## D-068 pinned reviewer pass (later session)

The reviewer ran against the rebased diff (merge-base `af2384fa` through
tip) and returned 7 findings: 1 blocker (D-201 misstated Part 1's own scope
as unchanged when the rebase-driven `tests_support.rs` retirement changed
it), 2 warnings (`docs/TESTING.md` and this file both had gone stale
relative to the rebased diff), 1 warning (the `ALLOWLIST` "never up" ratchet
described as mechanically enforced when it is currently only a review
convention — see `scripts/check_scratch_dir_usage.py`'s docstring), and 3
notes (all confirmed non-issues: `ScratchDir::new`'s unvalidated `category`
splice is deliberate caller-discipline already documented, `Drop`'s
discarded `remove_dir_all` result is correct panic-safety behavior, and a
scope note that the reviewer's own environment lacked shell access to
directly diff the rebase). All four actionable findings (the blocker and
three warnings) were addressed in this same pull request: D-201 and
`docs/TESTING.md` were corrected to describe the `tests_support.rs`
retirement and the `[dev-dependencies]` addition it required, the
`ALLOWLIST` ratchet language in both D-201 and the script's own docstring
was softened to describe it accurately as a review convention rather than
a mechanically enforced property, and this file was updated in place
(itself part of the unmerged diff, not a foreign prior session's file).

## What's still open (tracked, not this PR's scope)

- Part 2 — [#782](https://github.com/rotnov/pycc/issues/782): migrate the
  ~384 existing test call sites onto `ScratchDir`.
- Part 3 — [#783](https://github.com/rotnov/pycc/issues/783): fix
  `src/main.rs`'s two unconditional production leaks (`try_build`'s
  `pycc_obj_*`, `run`'s `pycc_run_*`) using `ScratchDir` (file-inside-a-
  directory, not a 1:1 substitution — flagged in the plan for whoever picks
  this up).
- Part 4 — [#784](https://github.com/rotnov/pycc/issues/784): bounded
  stale-root cleanup for crashed/killed processes.
- Part 5 — [#785](https://github.com/rotnov/pycc/issues/785): operational
  `TMPDIR` guidance + the issue's closing verification section. Depends on
  Parts 1–4 all merging first.
- The parent issue [#779](https://github.com/rotnov/pycc/issues/779) stays
  open until all 5 parts land.

## Where to resume

Once this PR merges, Parts 2/3/4 can each be planned (via `issue-to-plan`)
and implemented independently and in parallel — none of them depend on each
other, only on this PR's `pycc_scratch` crate. `crates/pycc_codegen/src/tests_support.rs`'s
own `TempTestDir`/`tempfile_dir` wrapper is already retired (a byproduct of
this PR's own rebase conflict resolution, described above), so Part 2's
remaining scope is exactly the `ALLOWLIST`-tracked raw call sites in other
crates.
