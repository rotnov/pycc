# Session handoff: #614 LLVM-install timeout hardening — activation half (closes #614)

## Status: PR-2 opened, carries `Fixes #614`. Second half of the coexist-then-activate sequence.

This session is PR-2 of the two-pull-request sequence described in
`docs/sessions/2026-08-30-04-issue-614-llvm-install-timeout-coexist.md`
(PR-1, merged to `main` as commit `1f948e78`). PR-1 taught
`scripts/check_roadmap_evidence.rb` to accept a new, timeout-hardened shape
for three "Install LLVM 22..." steps in `.github/workflows/ci.yml`, purely
additively — the live `ci.yml` was left unchanged by PR-1 and still passed
the checker against the old, unhardened shape. This PR activates that new
shape in the live workflow.

## What changed

`.github/workflows/ci.yml` — all three `apt.llvm.org`-based "Install LLVM
22..." steps (re-verified directly against the current file rather than
trusting PR-1's count, which turned out to match): the `governance` job's
"Install LLVM 22 for offline alpha skill contract evals" step, the
`native-build-test` job's "Install LLVM 22 (Linux, via apt.llvm.org)" step
(Linux-only, matrix), and the `frontend-perf-measure` job's step of the same
name. Each step now carries:

- `timeout-minutes: 5` (bounds the whole step, not just the download).
- `wget --timeout=30 --tries=3 https://apt.llvm.org/llvm.sh` in place of the
  bare `wget https://apt.llvm.org/llvm.sh` (bounds the download itself).
- The explanatory comment citing issue #614, matching
  `ISSUE614_LLVM_INSTALL_RUN_SCRIPT` in `scripts/check_roadmap_evidence.rb`
  byte-for-byte.

The `governance` step's exact shape (keys `if`/`name`/`run`/`timeout-minutes`,
run text equal to `ISSUE614_LLVM_INSTALL_RUN_SCRIPT`) is pinned by
`D171_GOVERNANCE_AGENT_STEPS_ISSUE614_RUN` /
`D171_GOVERNANCE_AGENT_STEP_EXTRA_KEYS`. The `frontend-perf-measure` step's
exact shape (keys `name`/`timeout-minutes`/`run` only — no `if` key — the
whole job compared by equality) is pinned by
`ISSUE614_LLVM_TIMEOUT_FRONTEND_PERF_MEASURE_STEPS`/`_JOB`. The
`native-build-test` step is not pinned by any exact-shape check in the
checker (confirmed by direct reading, matching PR-1's own note); it was
hardened identically for consistency, as PR-1 anticipated.

`tests/fixtures/policy-successor-manifest.json`: updated the `.github/
workflows/ci.yml` `sha256` entry to the new file's digest
(`6c5385dd021cb7f63d9e1c8e0c9a0bbc09456d5df841f35c952b92b5ed942756`) for
hygiene only. Confirmed (again, independently of PR-1's claim) that this
field is unenforced: `workflow-policy.yml`'s `parseManifestPaths` only reads
`path`/`source_path` from each entry, and `check_roadmap_evidence.rb`'s only
reachable whole-file digest check for the live, D171-routed `ci.yml`
compares against a different frozen constant that matches a historical test
fixture, not live `ci.yml`.

## Documentation impact: none

This is a CI-hardening fix with no user-visible behavior change (no new
CLI flag, diagnostic, or language semantic). `docs/ROADMAP.md` describes
compiler/CLI capability, not internal CI robustness, so no roadmap edit is
needed. No specification under `docs/SPEC.md` covers CI workflow internals.
Verified rather than assumed by grepping `docs/ROADMAP.md`, `docs/
DELIVERY_PLAN.md`, and `docs/decisions/*.md` for issue #614 and finding no
hits describing this behavior.

## Local gates

- `RUBYOPT`/locale note: this environment's default locale trips
  `check_roadmap_evidence.rb`'s markdown blockquote scan with `invalid byte
  sequence in US-ASCII`; running with `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8`
  (equivalent to PR-1's `RUBYOPT="-E UTF-8"`) fixes it. Recorded here in case
  a future session hits the same thing without PR-1's prior note.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb .`:
  pass ("Roadmap evidence policy passed.") against the newly hardened
  `ci.yml` — confirms the live workflow now validates as the shape PR-1
  taught the checker to accept.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/test_check_roadmap_evidence.rb`:
  pass, 244 runs / 1242 assertions / 0 failures / 0 errors.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_ci_permissions.rb`:
  pass, 10 workflow files, unaffected by this diff.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_readme_coverage_badge.rb`:
  pass, unaffected (checked because it also reads `ci.yml`, for the coverage
  step and badge binding, neither touched here).
- `python3 -B scripts/test_classify_ci_changes.py`: pass, 23/23 (checked
  because it also reads `ci.yml`'s job/step names for change classification).
- No Rust source file changed by this diff (`git diff --stat` against the
  branch base shows only `.github/workflows/ci.yml` and `tests/fixtures/
  policy-successor-manifest.json`), so the D-014 100%-coverage gate and
  `cargo clippy` were not re-run — verified rather than assumed by checking
  the diff's file list directly.

## Local pinned reviewer (`ievo:deep-reviewer`)

Dispatched and awaited synchronously in this session (unlike PR-1's session,
whose runtime lacked `Agent`/`Task` dispatch). Verdict: 4 findings, all
`warning`/`note`, no `P0`/`P1`:

1. `warning`: the 5-minute step timeout bounds the whole install (download +
   `llvm.sh` + `apt-get install libpolly-22-dev`), not just the download —
   a slow-but-not-hung mirror day could turn into a recurring flake on two
   required-check jobs. No action taken: the value is the one PR-1's
   checker constants already froze on `main`, and changing it now would
   need its own coexist-then-activate two-PR cycle per this issue's own
   established pattern. Left as a forward-looking observation, not a
   blocker.
2. `warning`: two other, non-required, `workflow_dispatch`-only workflows
   (`.github/workflows/frontend-perf-shadow.yml`,
   `.github/workflows/hook-install-check.yml`) still carry the unhardened
   `apt.llvm.org` `wget` pattern. Out of scope for issue #614, which the
   issue title scopes to `ci.yml`/`ubuntu-latest` runners specifically, and
   `frontend-perf-shadow.yml`'s own header already documents it as
   temporary/scheduled for deletion. Flagged as a background task instead of
   expanding this PR's scope (see Follow-ups below).
3. `note`: flagged the (at-review-time) absence of this very handoff file —
   addressed by adding it in this commit.
4. `note`: the reviewer couldn't independently recompute the sha256 hex
   value (no shell access in that dispatch) but confirmed the field is
   functionally unenforced either way. `sha256sum .github/workflows/ci.yml`
   was run directly by this session to produce the value used.

No fixes required before merge; the diff was already correct on first pass.

## Confirmed: this PR closes #614

`gh api graphql` confirmation of `closingIssuesReferences` is run after
opening the PR — see the PR itself / the coordinating session's report for
the exact query result. The PR body carries `Fixes #614`.

## Follow-ups

- Flagged (not filed, per this task's instruction to flag rather than
  expand scope) via `spawn_task`: `frontend-perf-shadow.yml` and
  `hook-install-check.yml` still carry the unhardened `apt.llvm.org` install
  pattern found by the deep-reviewer. Both are `workflow_dispatch`-only and
  not required checks, so low urgency; `frontend-perf-shadow.yml` is already
  documented as temporary. Whoever picks up the flagged task should file a
  milestone-scoped issue only if the pattern turns out to matter in
  practice (per AGENTS.md's filing bar for process observations), not
  automatically.
