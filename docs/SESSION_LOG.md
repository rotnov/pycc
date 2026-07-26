# Agent Session Log

A running handoff log for autonomous agent sessions working toward the
version 0.1 delivery goal (see `docs/DELIVERY_PLAN.md`
for the PR-1 through PR-7 breakdown this tracks against). Distinct from
`docs/AGENT_RETROSPECTIVE.md`: this file is "what state is the work in and
what's next," not "what went wrong." Newest entry first. Entries are
snapshots, not a byte-for-byte transcript — write enough for a fresh
session (human or agent) to resume without re-deriving context from git
history alone, not a full narrative.

---

## 2026-07-26 — PR #51 performance repair integrated with current main

**Snapshot evidence:** the containing merge integrates local performance-repair
parent `a7f048d` with refreshed `main@128285fbfbcfaa29b1a6c8fa81da4d84bae8d67f`.
[PR #51](https://github.com/rotnov/pycc/pull/51) remained open and non-draft at
remote head `c1e855590a23307bcd8472979ff37f8bbfd0f8d9` before this local integration
was pushed. That remote head ran required CI as run `30206099702` from
active-D-062 `main@45545bb057f5cd9e8712610c6137f53ef56d3aae`.

**Gate result:** trusted audit, agent checks, 100% coverage, Linux/macOS,
cross-compile, and the 5+5 measurement job passed. The isolated comparator
correctly blocked the changed-source candidate at `+10.7215%`: predecessor
aggregate median `7964.08 ns`, candidate `8817.95 ns`. This was not retried or
waived. The benchmark does not execute the changed root CLI sources, but it
exposed an existing redundant type-checker walk that could be removed without
changing the gate.

**Repair:** `pycc_types::check` now constructs already-concrete
function signatures directly and reserves constraint collection for modules
that contain real `Ty::Infer` signatures; a failed concrete validation falls
back to the historical solver-first order so diagnostic selection is stable.
The workspace coverage gate passes at 100% lines and regions, including
explicit fast-path, diagnostic-parity, solver-path, and collector edge cases;
workspace clippy, Rust documentation, roadmap evidence, and agent-asset checks
also pass. An initial local Criterion comparison improved from about `7.15 µs`
to `5.85 µs` (`−18.0%`); a later run after the diagnostic-order fallback
measured `6.99 µs` (about `−2.3%` from the same original observation). This
single-host evidence is noisy and is not selected as the gate result; the next
fixed 5+5 CI comparison remains authoritative.

**Where to resume:** validate and review the containing default-branch
integration, repeat exact-revision pre-commit installation, then push the new
head. Treat its CI run as new candidate evidence, not a rerun of the failed
head, and merge only if every required check is green with no unresolved
actionable review thread.

## 2026-07-26 — PR #51 pre-commit hook awaiting final CI and merge

**Snapshot evidence:** the checked-out `codex/pre-commit-hook` branch was at
commit `171eceb` with a clean tree before integrating refreshed
`origin/main@841048ec37e20d85a5a0406778f9ec8b66224b04`. The integration was in
progress with its documentation conflicts resolved but not yet committed at
this snapshot. [PR #51](https://github.com/rotnov/pycc/pull/51) is open and no
longer a draft.

**Overall status:** the pull request publishes `pycc-check` from the main
repository as a serial, read-only `language: rust` pre-commit hook; extends
`pycc check` to aggregate diagnostics across native input paths and supported
source encodings; and replaces required asynchronous GitHub review comments
with the immutable pinned local-review loop. D-067 and D-068 record the two
project-wide choices after confirming PR #132's reconciled D-057…D-061 and
D-070…D-073 allocations.

**Validation already observed:** the Rust workspace tests, clippy, generated
API documentation, agent-policy and marketplace checks, roadmap checks, and
100% line/region coverage passed before the latest default-branch integration.
An isolated `pre-commit try-repo` install selected exact revision `10a0502`
and passed `pycc check`; the final merged revision still needs the same check,
the pinned full-range local review, required pull-request CI, and normal
protected-branch merge.

**Where to resume:** finish and review the `841048e` merge, rerun affected
checks, push `codex/pre-commit-hook`, wait for every required PR #51 check and
conversation to clear, then merge normally and verify the post-merge
`main-history-audit`. Do not request `@codex review`; D-068 makes that external
service optional rather than a required gate.

## 2026-07-26 — PR #138 merged; D-062 blocks PR #132

**Delivered state:** [PR #137](https://github.com/rotnov/pycc/pull/137)
merged as `45545bb057f5cd9e8712610c6137f53ef56d3aae`. Post-merge CI run
`30205599108` passed the hard 100% line/region gate, all Tier-1 legs, the
cross-target proof, both frontend-performance jobs, and aggregate `ci-gate`;
the exact merge also passed agent-assets, agent-policy, Pages, and
main-history-audit. [PR #138](https://github.com/rotnov/pycc/pull/138) then
merged the PR-state-first monitoring rule as
`fb5d483daa9f9fd18914a0ceeee1b8448edd1421`. Post-merge run `30206232849`
and its agent and main-history workflows all completed successfully by the
2026-07-26T14:40:20Z inspection checkpoint, including 100% coverage, both
performance jobs, every Tier-1 leg, and aggregate `ci-gate`.

**D-062 evidence:** run `30205599108` retained exact predecessor artifact
`8632975406` and candidate artifact `8632990263`. Their five per-run medians
aggregate to `6924.73 ns -> 7077.93 ns` (`+2.2123%`). The trusted classifier
proved the executable inputs identical, so D-056's retained rule correctly
treated that delta as non-blocking environment telemetry; evaluating the same
evidence as changed inputs would fail the unchanged hard `>2%` gate. This
verifies D-062's delivered identical-input path, but does not close
[#109](https://github.com/rotnov/pycc/issues/109): repeated changed-source PR
and post-merge evidence is still required without result selection.

**Current work at the same checkpoint:** [PR #132](https://github.com/rotnov/pycc/pull/132)
was open at `1ae1b3c90749836aeaa340ad0d8a067dc605d464` while current `main` was
`fb5d483daa9f9fd18914a0ceeee1b8448edd1421`; GitHub reported it conflicting
and `DIRTY`. Its first performance attempt failed before collecting timing
because rustup hit a DNS lookup error. The permitted no-result rerun completed
all fixed samples, but D-062 then blocked the changed-input comparison at
`6686.28, 6777.54, 7088.44, 7228.73, 7185.40 ns` versus
`8498.72, 7405.25, 7527.44, 8023.83, 7353.21 ns`: aggregate medians
`7088.44 ns -> 7527.44 ns`, or `+6.1931%`. Exact artifacts `8633194749`
and `8633213046` retain that evidence. Coverage, audit, cross-target, and every
native platform check passed, but `frontend-perf-gate` and `ci-gate` failed;
do not rerun or select another timing result for this head. Nine current Codex
threads and one outdated thread also remain unresolved. Refresh the branch from
current `main`, verify and fix every confirmed thread with regression coverage,
investigate the retained performance evidence without result selection, resolve
only fixed or proven-obsolete threads, then obtain green required checks and a
Codex review for the final exact head before considering merge.

## 2026-07-26 — PR #138 opened for PR-state-first CI monitoring

**Snapshot evidence:** ready-for-review
[PR #138](https://github.com/rotnov/pycc/pull/138) was opened from
`codex/check-pr-state-before-ci` at head
`61163e35f67af30a5b3dc24b988abc9f3c1eb9a3`, based on
`main@45545bb057f5cd9e8712610c6137f53ef56d3aae`. A live state query reported
the PR `OPEN`, non-draft, and `MERGEABLE`; `mergeStateStatus=BLOCKED`
reflected required checks still in progress rather than a merge conflict.
The containing change is not yet merged.

**Delivered scope if merged:** `.ievo/evolution/project.md` will require
agents to inspect pull-request lifecycle and mergeability before waiting for
CI, and `docs/AGENT_RETROSPECTIVE.md` records the PR #132 incident that
motivated the rule. No compiler behavior, supported platform, roadmap
acceptance evidence, or delivery sequencing changes.

**Required next steps:** push the session-log snapshot as the final PR head,
rerun focused local validation, confirm the PR is still open and mergeable,
request Codex review through the retry guard, and monitor required CI plus
all review surfaces. Merge only after required checks are green and no
actionable review thread remains; then verify the post-merge `main` run and
history audit.
## 2026-07-26 — D-062 activation PR #137 green; refresh onto current main in progress

**Snapshot evidence:** draft [PR #137](https://github.com/rotnov/pycc/pull/137)
at head `b6f5a29d4c56d65d88d82120595bbc04343c6f25` was based on
`e433b849ef1083c0af7aa6da6c022a6e0661dc9f`. Its first complete CI run
`30204610811` and every required check passed, Codex reported no major
issues for that exact head, and the PR had no review threads. While the
run completed, PR #128 advanced `main` to
`841048ec37e20d85a5a0406778f9ec8b66224b04`; the activation worktree is
therefore integrating that exact default-branch commit before review and
merge. This containing snapshot includes the resolved documentation from
that integration but does not itself claim that PR #137 has merged.

**Performance-gate state:** PR #137 activates the staged D-062 workflow
byte-for-byte and retires D-056's live workflow digest while preserving
its identical-input telemetry rule. The first unselected 5+5 PR artifacts
are `frontend-perf-previous` ID `8632698165` and
`frontend-perf-current` ID `8632713088`, retained for 90 days. Their five
per-run medians aggregate to `6869.56 ns -> 6967.66 ns` (`+1.4281%`). The
exact base/head diff contains no `src/` or `crates/` change, so the trusted
classifier reports identical executable inputs; the comparator also falls
within 2% if evaluated as changed. This validates byte-exact execution and
fixed evidence handling, but does not exercise the blocking changed-input
path.

**Required next steps:** finish the non-force merge of current `main`,
rerun focused local policy/site checks, push the new head, run the review
retry checker before requesting Codex review for that SHA, and merge only
after the repeated required checks are green with no unresolved threads.
Then verify the post-merge `main` CI and history audit. Keep issue #109
open until repeated changed-source PR and post-merge runs validate D-062's
blocking aggregate without result selection. PR #132 is a likely future
changed-source observation only after it rebases; its unrelated draft
D-062 through D-065 ADR range currently collides with `main`, and the
renumbering finding is recorded on that PR.

## 2026-07-26 — PR-5 integration and PR loop pending; PR-6/PR-7 not started

**Snapshot evidence:** read-only inspection of
`feat/v0-1-pr5-codegen-depth` at commit `c70ac56`; its worktree was clean.
The branch is not merged and has no open pull request as of this snapshot.

**Overall status:** PR-1 through PR-4 are merged to `main`. The default-
branch snapshot is `619d232` (the merge of PR #130) and includes the later
infrastructure, governance, performance-gate, and agent-tooling changes.
PR-5 remains in progress on branch `feat/v0-1-pr5-codegen-depth`,
following that branch's complete 11-task version of
`docs/superpowers/plans/2026-07-25-pr5-codegen-depth.md`; the version in
the containing `main` snapshot has only Tasks 1–2.

**PR-5 task status:** all 11 planned tasks have implementation commits.
The observed head follows Task 11's end-to-end fixture/documentation sweep
and its review fixes with a commit that adds the top-level-return
terminator guard and clears the recorded deferred minors. Current-`main`
integration, ADR renumbering, full current-base validation, PR creation,
and the PR review loop have not yet completed.

**Known follow-up required before PR-5 merges:** integrate the latest
`main` without overwriting any newer local work. Published PR #132 now
carries D-057 through D-065, while current `main` owns a conflicting
D-062 and this journal uses D-066. PR #132 must reconcile that colliding
tail before merge. Re-check the live ADR tail immediately before editing;
later IDs are candidates, not reservations. Full detail is in
`docs/AGENT_RETROSPECTIVE.md`.

**After PR-5 merges:** PR-6 (conformance and acceptance benchmarking —
`pycc_testkit`, `fib`/`mandelbrot-ascii` vs. pinned CPython on all 5
Tier-1 targets, the `pycc check` <50ms/1k-LOC benchmark, and exact
diagnostic-output acceptance) and PR-7 (buffer to close whatever's left
against the v0.1 acceptance checklist) have not been started. The paired
frontend regression gate is already active and required through
`ci-gate` under D-056 in the containing commit; D-051/D-053 remain the
retained paired-provenance controls, not the current workflow/comparator
authorization. The gate is not deferred PR-6 work. PR-6 is the first point
the full pipeline runs end-to-end on all five Tier-1 platforms — treat it
as the highest-uncertainty remaining slice, not a formality.

**PR-5 recovery boundary:** the local-only state above is historical, not
the current recovery path. A later read-only check found the snapshot commit
in the ancestry of published branch
`origin/feat/v0-1-pr5-codegen-depth`, with observed remote head
`453e7dd9b23effe0390770d8ad7c264c33150bdd` and open
[PR #132](https://github.com/rotnov/pycc/pull/132) based on
`main@6ec86a8e89c7775f9f41a9aa9b12a1a2660952de`. A clean clone can recover
the work without any machine-local path:

```sh
git fetch --prune origin main feat/v0-1-pr5-codegen-depth
git rev-parse origin/main origin/feat/v0-1-pr5-codegen-depth
git merge-base --is-ancestor \
  c70ac5696ff908770350a587ed87210cd6edd80b \
  origin/feat/v0-1-pr5-codegen-depth
git log --oneline --decorate \
  origin/main..origin/feat/v0-1-pr5-codegen-depth
```

The exact historical snapshot remains
`c70ac5696ff908770350a587ed87210cd6edd80b`. If the published head differs
from the observed head above, treat the remote and PR as newer state: inspect
them before acting and never reset, force-push, or overwrite an existing
owner's local worktree to recreate this older snapshot.

**Where to look to resume:**
- Read [PR #132](https://github.com/rotnov/pycc/pull/132) and compare its
  current remote head with the observed head above. If an existing PR-5
  worktree is present, run `git status --short --branch` there before any
  mutation; never use this snapshot to overwrite newer local work.
- `docs/DELIVERY_PLAN.md` — PR breakdown and autonomy policy.
- `docs/ROADMAP.md` — current delivery status and the v0.1 acceptance
  checklist (source of truth for what's actually done vs. claimed).
- `git show origin/feat/v0-1-pr5-codegen-depth:docs/superpowers/plans/2026-07-25-pr5-codegen-depth.md`
  — the branch's complete active plan, task-by-task, if PR-5 is not merged
  yet; do not mistake the shorter `main` copy for the whole plan.
- `git log --oneline origin/main..origin/feat/v0-1-pr5-codegen-depth`
  for the published commit-by-commit state.
