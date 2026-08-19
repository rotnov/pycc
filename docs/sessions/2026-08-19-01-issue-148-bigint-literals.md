# 2026-08-19-01 — issue #148, out-of-range `int` literal materialization

## Overall status

Autopilot run under a bare `/next-milestone` directive (open-ended: loop over
milestones, not one). Baseline for this checkpoint: `origin/main` =
`6abf16ba38165c5cd7b18c5c0a7f6e9ad535723f`, zero open pull requests at the
start of the run, `tests/fixtures/policy-successor-manifest.json` steady-state
(49 targets, 0 mid-transition), branch protection matching the documented
baseline.

`next-milestone` steps 1–5 completed: **v0.3 is the active milestone**,
independently verified unmet (31 of the 37 required `◐`-or-better conformance
rows, plus unverified `pycc explain` and diagnostics-registry sub-criteria).
v0.1 and v0.2 were both ruled out on cited evidence.

## In flight

**[PR #617](https://github.com/rotnov/pycc/pull/617)** — `Fixes #148`, `Fixes #616`.
Opened from `claude/next-milestone-c1bab4` at commit `7bd75265`, on top of
`6abf16ba`. CI result is **not yet known at the time of writing**; this entry
records the pull request as opened, not as green or merged.

The change makes an `int` literal outside the D-061 tagged smallint range
materialize as a heap bigint instead of aborting the compiler with a Rust
`panic!` from `pycc_codegen::tag_smallint_const`.

### What the run added beyond the issue's own text

- **A second reachable panic site.** Enum member discriminants fold through the
  same helper, and the HIR guard validates a member value only against `i64`
  range, not the 63-bit tagged range — so any member in `[2^62, 2^63)` hit the
  identical panic. Recorded on the issue as comment `5343227521`. A fix confined
  to the literal path would have left it live.
- **An accepted consequence, measured rather than assumed.** Materializable
  bigint literals can now reach `int`-boundary positions that abort in
  `pycc_rt_int_untag_checked`, so `x = [<oversized literal>]` moves from a
  compile-time panic to a runtime abort. A bigint arriving by arithmetic
  (`b: int = 4611686018427387903 + 1`) *already* aborts identically at all 14
  such positions today, so the change removes an inconsistency rather than
  inventing a failure mode. The cost is real for the 12 value positions among
  those 14: the gap moves from compile time to run time, and `pycc check` alone
  stops catching it. Rejected alternative and reasoning recorded in **D-178**.

## Known follow-ups

- **File the deferred issue** for a compile-time diagnostic rejecting an
  out-of-range literal in an `int`-boundary position. D-178 records the
  reasoning; no issue number exists yet, and the ROADMAP text deliberately
  points at D-178 rather than at a fabricated number.
- **Conformance oracle could not be run locally.** This machine has CPython
  3.14.6; the harness pins 3.14.7 exactly, so all 47 `#[ignore]`d conformance
  tests — including the fixture added here — fail the version assertion on this
  machine. The new fixture was verified byte-for-byte against local 3.14.6 in
  both `--debug` and `--release`. CI holds the pinned oracle and is the real
  verdict.
- **Staleness screen is incomplete.** `issue-select` step 3 covered only the P1
  compiler candidates read this pass. The July-2026 cohort (#14, #20, #44, #45,
  #53) remains unscreened.
- **Documentation drift observed, not yet fixed:** `docs/ROADMAP.md`'s v0.3
  prose says 29 evidence-backed rows where the tree shows 31; its "Current
  delivery status" is stale at `Last reviewed on 2026-08-07`; issue #545 cites
  17,665 lines for `crates/pycc_codegen/src/lib.rs`, which is now 19,090.

## Housekeeping done this run

- **#123** (P1, string repetition) closed as stale with a three-row evidence
  table and the resolving commits cited.
- **#146, #147, #148, #20** assigned to the v0.3 milestone.

## Paused autopilot

- **Directive scope:** open-ended (`/next-milestone` with no argument) — loop
  over milestones, not a single one.
- **Active milestone:** v0.3, verified unmet.
- **Last iteration outcome:** #148 selected, planned, implemented, and opened as
  PR #617. Terminal state not yet reached — CI unverified, not merged.
- **Exact next autopilot step:** monitor PR #617 per `issue-implement` step 7
  (checkpoint its state, head, mergeability, and required checks; react only to
  real events). On green and clean, merge per step 8 and confirm #148 and #616
  closed. Then deliver the brief report, run `next-milestone` step 2's evidence
  check against v0.3 unconditionally, and — if still unmet — re-enter
  `issue-select` step 1 with a fresh baseline.
- **In-run denylist:** empty. No issue hit a per-issue stop condition this run.

## Where to resume

Read this file, then `docs/ROADMAP.md`'s v0.3 section and the live state of
PR #617 and issues #148/#616. The `next-milestone` and `issue-select` skills
under `.claude/skills/` define the loop being executed.
