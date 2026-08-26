---
id: D-203
title: "Narrow the D-091 bench-manifest tail check to tolerate the `pycc_scratch` root dev-dependency line"
status: accepted
---

## D-203: Narrow the D-091 bench-manifest tail check to tolerate the `pycc_scratch` root dev-dependency line

- Status: accepted
- Context: D-091 pinned root `Cargo.toml`'s `[dev-dependencies]`-onward tail byte-exactly in
  `frontend-perf-measure`'s "Verify exact benchmark revisions" step, so a faster measuring apparatus (a bumped
  criterion, a rewritten `[[bench]]` target) cannot pass the perf gate as a faster compiler. The D-201
  scratch-directory program (#779/#781/#782) needs `pycc_scratch = { path = "crates/pycc_scratch" }` as a root
  dev-dependency so integration tests under `tests/` can use `ScratchDir`; commit `f79bb2b5` deliberately
  reverted exactly that line because it trips the tail check, and #780's `f9231e2f` repeated the same
  add-trip-revert cycle mid-review, falling back to a new `scripts/check_scratch_dir_usage.py` ALLOWLIST
  escape-hatch entry. Each recurrence adds allowlisted raw-`temp_dir` debt. Issue #800 tracks narrowing the
  gate; its one-PR premise is wrong — the base-owned D-172 `audit` deep-compares the measure job against the
  pre-PR checker's frozen constants and never executes the head's checker, so a single PR editing both ci.yml
  and the checker hard-fails the required check (empirically reproduced during planning). The D-125 bypass is
  ineligible because the failure would be caused by the candidate's own diff.
- Decision: tolerate exactly the one reviewed line by filtering it from both tails symmetrically before the
  byte-exact diff — `grep -vxF 'pycc_scratch = { path = "crates/pycc_scratch" }'` applied to the
  `[dev-dependencies]`-onward extraction of both `previous/Cargo.toml` and `current/Cargo.toml`. Every other
  tail difference, including any spacing or path variant of this line, still hard-aborts. Deliver it through
  the checker's documented coexist-then-retire lifecycle in two PRs: PR-1 adds a new frozen
  `D203_VERIFY_REVISIONS_SCRIPT` heredoc (byte-identical to `D91_VERIFY_REVISIONS_SCRIPT` except the final
  diff lines), derived `D203_SCRATCH_DEVDEP_FRONTEND_PERF_MEASURE_STEPS`/`_JOB` constants, and a new `elsif`
  branch in `validate_source_aware_perf_gate_lifecycle` returning the unchanged D-112/D-114 gate-job set —
  purely additive, `D91_VERIFY_REVISIONS_SCRIPT` and the D-112/D-114 fixtures stay frozen. PR-2 activates the
  narrowed step in ci.yml (byte-identical post-dedent to the new constant) together with the `Cargo.toml`
  dev-dependency line and `Cargo.lock`, and appends a dated update annotation to D-201's blocker paragraph
  (annotating an accepted decision with a dated cross-reference is not a silent rewrite; precedent: D-103's
  supersession note landed post-acceptance via PR #570, commit `163bf49f`).
- Alternatives: (a) regex/pattern-family tolerance (`grep -vE '^pycc_scratch *='`) — rejected: widens the
  reviewed tolerance from one byte sequence to a family, letting an unreviewed variant ride through the
  fingerprint; the gate's value is byte-exactness, so the tolerance must be byte-exact too. (b) Expected-diff
  comparison (diff the tails, compare against a stored expected hunk) — rejected: more logic in a script that
  must stay a byte mirror in the checker, sensitive to diff output formatting, harder to review than a
  one-line symmetric filter. (c) Mutating `D91_VERIFY_REVISIONS_SCRIPT` in place — rejected: breaks the
  frozen D-112/D-114 fixture tests and destroys historical mutation-test evidence. (d) Adding `pycc_scratch`
  to the `[target.'cfg(unix)'.dev-dependencies]`/`cfg(windows)` blocks above the tail — rejected: mechanically
  passes the gate but misdescribes a platform-independent dev-dependency as two platform-specific ones and is
  exactly the evasion-shaped change the gate exists to surface. (e) Gutting the tail diff (keep only the awk
  invariants) — rejected: abandons D-091's core property. (f) One PR plus a D-024/D-125 bypass — rejected:
  the failure would be candidate-caused, which the standing D-125 authorization's Gate 1 excludes, and the
  one-time #558 recovery authorization is spent.
- Consequences: exactly one tolerated line; the next root dev-dependency still hard-aborts until a successor
  decision widens the set. The filter is symmetric and position-independent within the tail, so a future
  removal of the line is also tolerated, and a copy placed inside the `[[bench]]` stanza is an inert unused
  manifest key (a duplicate copy under `[dev-dependencies]` is a Cargo duplicate-key error). D-172's
  "ordinary CI changes use one PR" consequence gains a recorded counterexample class: gate-defining checker +
  ci.yml co-changes still need coexist-then-retire, because the base-owned audit validates the head's ci.yml
  against the pre-PR checker's constants. Unblocks #782 Batches B–D (PR #793's migration and the
  `f9231e2f`-era ALLOWLIST debt) once PR-2 lands.
