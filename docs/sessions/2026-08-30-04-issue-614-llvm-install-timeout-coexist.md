# Session handoff: #614 LLVM-install timeout hardening — coexist half only

## Status: PR-1 opened (checker-only, purely additive). #614 stays open.

This session selected #614 (`P2: 'Install LLVM 22 (Linux, via apt.llvm.org)'
can wedge 'ubuntu-latest' jobs for hours — no timeout-minutes, no wget
retry/timeout`, milestone v0.4) via `issue-select`, then discovered mid-
implementation that the natural single-PR fix cannot pass the required
`audit` GitHub Actions check, and instead delivered only the first half of
a two-pull-request coexist-then-retire sequence. **#614 is not closed by
this pull request.**

## The blocking discovery

The natural fix edits three "Install LLVM 22..." steps in
`.github/workflows/ci.yml` (governance, native-build-test,
frontend-perf-measure jobs), adding `timeout-minutes: 5` and
`wget --timeout=30 --tries=3`. Two of those three steps are pinned by exact
frozen-constant equality checks in `scripts/check_roadmap_evidence.rb`
(`D171_GOVERNANCE_AGENT_STEPS` for governance;
`validate_source_aware_perf_gate_lifecycle`'s job-shape dispatch chain for
frontend-perf-measure), so the fix also requires new checker knowledge of
the changed step shape.

`docs/decisions/D-172-nonblocking-property-based-ci-policy-audit.md`'s own
2026-08-26 update note documents a counterexample to its "ordinary CI
changes use one PR" rule: the required `audit` job
(`.github/workflows/workflow-policy.yml`) runs under `pull_request_target`
with `ref: ${{ github.sha }}` — for that event type this resolves to the
**base branch tip**, not the PR head. It checks out that trusted base
revision, downloads the PR head's `.github/workflows/*` files and
`scripts/check_roadmap_evidence.rb`'s *inputs* as non-executable data, then
runs the **base-checked-out** (i.e. `main`'s, pre-PR) version of
`check_roadmap_evidence.rb` against that head data. The PR head's own
modified checker script is never executed.

`docs/decisions/D-203-narrow-the-d-091-bench-manifest-tail-check-to.md` is
direct, empirical precedent for exactly this failure mode: its Context
section states "a single PR editing both ci.yml and the checker hard-fails
the required check (empirically reproduced during planning)", and its
Decision section records that D-203 itself shipped as two pull requests —
PR-1 purely additive to the checker (new frozen constants, a new `elsif`
branch), PR-2 activating the ci.yml change once PR-1's checker knowledge
was already on `main`.

A prior session's handoff note
(`docs/sessions/2026-08-27-02-issue-246-tier1-target-parity.md`) claimed
"D-203 did so in one PR, 2026-08-26" — **this claim is wrong.** D-203's own
decision document is authoritative and directly contradicts it. That
session's own Follow-ups section had already flagged this as unverified
("Whoever picks up #614 should verify against its concrete diff rather than
relying on either directional claim from this entry") — this session did
that verification and the answer is: the checker-edit mechanism itself
works fine (backward-compatible, both shapes accepted), but the
**combination** of the checker edit with the ci.yml edit in one PR is what
`audit` cannot survive.

This is a genuine D-127 judgment fork (continue with a two-PR split, or
reselect a cleaner issue). Resolved via this session's own advisor tool:
don't reselect — the premise is fully verified and the checker patch is
already correct and tested; follow the D-203 house pattern rather than
inventing a new one.

## What this pull request actually contains (PR-1, the "coexist" half)

`scripts/check_roadmap_evidence.rb` only — no `.github/workflows/ci.yml`
change, no `tests/fixtures/policy-successor-manifest.json` change (that
file's `sha256` field was confirmed this session, by exhaustive grep across
`scripts/`, `tests/`, and `workflow-policy.yml`, to be unenforced/vestigial
for these two files, so it needed no update for a checker-only change):

- `ISSUE614_LLVM_INSTALL_RUN_SCRIPT` (new constant): the hardened run text
  (`wget --timeout=30 --tries=3`, explanatory comment citing #614) shared by
  all three step occurrences.
- `ISSUE614_LLVM_TIMEOUT_FRONTEND_PERF_MEASURE_STEPS`/`_JOB` (new
  constants, derived from `D203_SCRATCH_DEVDEP_FRONTEND_PERF_MEASURE_STEPS`/
  `_JOB` via `Marshal.load(Marshal.dump(...)).tap`, replacing only the
  LLVM-install step) plus a new `elsif` branch in
  `validate_source_aware_perf_gate_lifecycle`, mapping to the same accepted
  gate-job set as D112/D114/D203 — the frontend-perf-measure job is
  unchanged in every other respect, so the existing gate-job pairing still
  applies.
- `D171_GOVERNANCE_AGENT_STEP_EXTRA_KEYS` / `_ISSUE614_RUN` (new
  constants) plus a rewrite of the `agent_steps.each` loop inside
  `validate_d171_ci_routing`: `timeout-minutes` becomes an *optional* extra
  key (present-and-correct, or absent), and the step's `run` text may match
  *either* the pre-#614 or post-#614 text. `D171_GOVERNANCE_AGENT_STEPS`
  itself (the frozen historical constant) is left untouched — required by
  several existing fixtures/round-trip tests — so both shapes validate.
- `native-build-test`'s copy of the LLVM-install step is **not** pinned by
  any exact-shape check today (confirmed by direct code reading — it does
  not appear in `D171_NATIVE_REQUIRED_RUN_STEPS`), so PR-2 can edit it
  freely with no checker change needed.

Five new tests added to `scripts/test_check_roadmap_evidence.rb`:
- `test_d171_accepts_issue614_governance_llvm_step_with_timeout` — accepts
  the new shape.
- `test_d171_rejects_issue614_governance_llvm_step_wrong_timeout_value` —
  rejects a wrong `timeout-minutes` value, proving the key is validated,
  not merely permitted.
- `test_d171_rejects_issue614_governance_llvm_step_unexpected_extra_key` —
  rejects an unrelated extra key, proving the allowlist stays narrow.
- `test_issue614_lifecycle_accepts_llvm_timeout_measure_job` — accepts the
  new frontend-perf-measure job shape, paired with either reviewed gate job.
- `test_issue614_lifecycle_rejects_measure_job_missing_the_timeout_key` —
  proves the equality check is on the whole step shape, not just the key.

`docs/TESTING.md` (~line 1097): corrected a stale attribution that cited
"D-103's staging rules" for why a `ci.yml` step needing new checker
knowledge forces a two-PR cycle. D-103 is retired (D-172, PR #570); the
live constraint is D-172's base-owned `audit` job as narrowed and
empirically confirmed by D-203. Reworded with a citation to D-203 instead
of deleting the underlying (correct) two-PR-cycle claim.

## Local gates (checker/docs-only diff; no Rust source changed)

- `RUBYOPT="-E UTF-8" ruby scripts/test_check_roadmap_evidence.rb`: pass,
  242 runs / 1236 assertions / 0 failures / 0 errors (237 pre-existing +
  5 new).
- `RUBYOPT="-E UTF-8" ruby scripts/check_roadmap_evidence.rb .`: pass
  ("Roadmap evidence policy passed.") against the **current, unmodified**
  `ci.yml` — confirms this change is genuinely additive: the live workflow
  needs no update to keep passing.
- `RUBYOPT="-E UTF-8" ruby scripts/check_ci_permissions.rb .github/workflows`
  and `scripts/test_check_ci_permissions.rb`: pass, unaffected.
- `python3 scripts/test_classify_ci_changes.py` (run without `-I`, since
  isolated mode excludes the script's own directory from `sys.path` and
  breaks its local `classify_ci_changes` import): pass, 23/23.
- `cargo build --workspace`: pass (baseline; no Rust source touched).
- `cargo test --workspace -- --include-ignored` /
  `--no-fail-fast`: every suite passes except the `pycc` crate's
  `conformance` integration binary (55/58 failing), each failing with
  `conformance oracle must be exactly Python 3.14.7, found "Python
  3.14.6\n"`. This is a pre-existing local-environment gap (this machine's
  Python is 3.14.6, one patch behind the pinned oracle) unrelated to this
  diff, which touches no Rust or Python codegen. Not something this
  session could or should fix.
- `cargo doc --workspace --no-deps`: pass; the same pre-existing, unrelated
  `pycc_types/src/env.rs:308` private-intra-doc-link warning noted in prior
  sessions.
- `cargo clippy` and the D-014 coverage gate were not re-run this session:
  no Rust source changed, so neither result differs from the already-clean
  baseline recorded in the merged `#834`/PR #847 session
  (`docs/sessions/2026-08-30-03-...md`).

## Known limitation: pinned local reviewer

This session's runtime does not expose an `Agent`/`Task` dispatch tool for
`ievo:deep-reviewer` (same gap recorded in prior sessions in this
environment). The coordinating session, or whoever merges this PR, must
still run it over the committed range before merge.

## For whoever picks up PR-2 (the actual fix, closes #614)

1. Re-apply the `ci.yml` change (three steps: governance, native-build-test,
   frontend-perf-measure — add `timeout-minutes: 5` and change the `wget`
   line to `wget --timeout=30 --tries=3 https://apt.llvm.org/llvm.sh`, plus
   the explanatory comment citing #614; governance's exact text must equal
   `ISSUE614_LLVM_INSTALL_RUN_SCRIPT`/`D171_GOVERNANCE_AGENT_STEP_EXTRA_KEYS`
   from this PR verbatim, and frontend-perf-measure's must equal
   `ISSUE614_LLVM_TIMEOUT_FRONTEND_PERF_MEASURE_STEPS`).
2. Update `tests/fixtures/policy-successor-manifest.json`'s
   `.github/workflows/ci.yml` `sha256` entry to the new file's digest (for
   hygiene only — confirmed unenforced for this path, but keep it current).
3. This session decided **not** to retire the pre-#614 shapes from
   `check_roadmap_evidence.rb` in PR-2 — D-203's own precedent keeps the
   older shapes coexisting rather than retiring them in the same PR that
   activates the new one (its Decision section describes only a two-step
   coexist-then-activate sequence, not a three-step
   coexist-then-activate-then-retire one). Retiring the pre-#614 governance
   shape and its now-redundant `D171_GOVERNANCE_AGENT_STEPS`/`_EXTRA_KEYS`
   optionality is a separate future cleanup, not a blocker for PR-2.
4. `Fixes #614` belongs on PR-2, not PR-1.
5. Confirm PR-1 has actually merged to `main` before opening PR-2 — PR-2's
   own `audit` run needs PR-1's checker knowledge already present on the
   base branch.

## Follow-ups / known non-issues

- No new issues were filed or narrowed as a side effect of this task.
- The stale "D-203 did so in one PR" claim in
  `docs/sessions/2026-08-27-02-issue-246-tier1-target-parity.md` is left
  unedited per AGENTS.md's D-066 journal rule (historical record, reviewed
  for accuracy but not rewritten) — this entry is the correction.
