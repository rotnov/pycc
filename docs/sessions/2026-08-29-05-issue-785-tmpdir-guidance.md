# 2026-08-29 — Issue #785: operational TMPDIR guidance + #779 closing verification (Part 5 of #779)

## Status

Delivered. This entry lands in the pull request that delivers Part 5 of
[#779](https://github.com/rotnov/pycc/issues/779) — the operational
`TMPDIR` guidance and the executed closing verification — and closes
both [#785](https://github.com/rotnov/pycc/issues/785) and #779 (the
last open sub-issue). Docs-only change; no Rust code touched. Based on
`origin/main` at `ccd30e9c4b42492e5aa285c9dd9edcc34c6dae54` (the PR #835
merge that closed #784), per the issue-to-plan comment on #785.

## What was delivered

- `docs/TESTING.md` "Scratch directories": new "Operational TMPDIR
  guidance (issue #785, Part 5 of #779)" subsection — the
  isolated-auto-cleaned-`TMPDIR` rule for high-volume test runs (with
  `trap`-based interrupt cleanup), the two cases the D-209 sweep can
  never serve (entry-budget starvation in foreign-crowded temp
  directories; pre-Part-1 legacy `pycc_*` names, with one-time manual
  cleanup guidance), the named anti-pattern (periodic global `/tmp`
  deletion), the reusable three-consecutive-runs verification protocol,
  and the dated evidence sentence for the run below.
- `AGENTS.md` "Testing and hard coverage gate": one pointer bullet
  (isolated auto-cleaned `TMPDIR` for high-volume loops; details in
  `docs/TESTING.md`).
- `docs/DELIVERY_PLAN.md`: the #779 paragraph now records all five
  sub-issues delivered and #779 closed.
- No new decision entry (design already recorded in D-201/D-209); no
  ROADMAP/README/SPEC/site edits (ROADMAP and README are llms.txt
  aggregate-budget members; aggregate unchanged at 270323/270336 bytes,
  13 bytes headroom).

## Measured verification (closes #779)

Executed on 2026-08-29 on macOS arm64 (Darwin 25.5.0) at the tree
delivering this entry (docs-only diff over `ccd30e9c`):

- Default per-user temp dir baseline: 14,677 `pycc_*` entries,
  5,135,936 KiB (~4.9 GiB). By name pattern: 10,641 `pycc_obj_{pid}.o`
  files and 1,559 `pycc_run_{pid}` dirs (pre-Part-3 production-leak
  formats), plus assorted pre-Part-2 legacy test dirs; only 19 entries
  parse as the full sweepable `pycc_{category}_{pid}_{nanos}_{seq}`
  format. Entry mtimes: 4,888 from 2026-08-26, 7,484 from 2026-08-27,
  1 from 2026-08-28, 2,304 from 2026-08-29 — accumulated by test runs
  of pre-Part-2/3 revisions (other sessions on this machine), not by
  this task. This differs from the plan's planning-time measurement
  (0), and it makes both documented sweep-unreachable cases live on
  this machine: the names are legacy (never parseable by the sweep)
  and the 14,677-entry crowd exceeds the sweep's 10,000-entry budget
  (starvation). The full baseline listing is preserved in the
  implementing session's protocol log; this entry records the count,
  size, and pattern breakdown in its place — a deliberate deviation
  from the plan's "keep the listing itself", which was written
  expecting ~0 entries, not 14,677 lines.
- Three consecutive `cargo test --workspace` runs under one fresh
  isolated `TMPDIR`: all exited 0 with 69/69 green `test result: ok`
  blocks each; the isolated root held zero `pycc_*` entries — and zero
  entries of any kind — after every run.
- Wall times per run: 203 s, 113 s, 298 s (warm tree; runs 1 and 3
  shared the machine with concurrent sessions).
- After the runs: the default temp dir's `pycc_*` set, count, and size
  were exactly baseline again (14,677 entries, 5,135,936 KiB;
  legacy-format subset identical before/after at 14,658). The after
  listing momentarily caught 5 extra full-format roots belonging to a
  concurrent pycc test process (different pids, default `TMPDIR`); all
  5 were gone seconds later — transient live roots correctly
  RAII-dropped by their owner, not leaks.

Regression-test mapping for #779's five "Regression tests required"
bullets: normal-`Drop` disappearance, panic-unwind disappearance, and
parallel-creation uniqueness are `crates/pycc_scratch/src/lib.rs` unit
tests; stale-root cleanup sparing live/unrelated roots is
`crates/pycc_scratch/src/sweep.rs`'s keep/delete-class tests plus the
`tests/slice0.rs` sweep e2e; "repeated representative runs return to
baseline" has no single automated repeated-run test — it is discharged
by the single-run `tests/slice0.rs` leak e2e plus the three executed
consecutive runs above.

Gates at the final tree, all exit 0: `cargo build --workspace`,
`cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100`, `cargo test --workspace` (the three protocol runs),
`cargo clippy --workspace --all-targets -- -D warnings`, the `scripts/`
unittest suite, `check_scratch_dir_usage.py`,
`validate_agent_policies.py`, `validate_agent_assets.py`,
`check_ci_permissions.rb`, `check_roadmap_evidence.rb` (+ its test),
`generate_decisions_index.py --check`, the llms.txt aggregate-budget
arithmetic, and `cargo doc --workspace --no-deps`. The Windows
cross-check was skipped as docs-only (no Rust code touched).

## Known follow-ups

- #779 and #785 close with the delivering PR; the verification report
  above is the source for the closing comment on #779.
- Machine-local (not tracked as an issue — operational state, not
  repository state): this machine's default temp dir still holds the
  ~4.9 GiB of legacy `pycc_*` entries measured above. The one-time
  manual cleanup documented in `docs/TESTING.md`'s Part 5 subsection
  applies, but its precondition (no pycc, `cargo test`, or CI process
  running) could not be confirmed during this task — concurrent agent
  sessions were demonstrably active — so the cleanup was deliberately
  not performed here. Run it from an idle machine.

## Where to resume

- The guidance itself: `docs/TESTING.md` "Scratch directories", Part 5
  subsection. The plan: the issue-to-plan comment on #785
  (issuecomment-5463046922). The sweep design: D-201/D-209.
