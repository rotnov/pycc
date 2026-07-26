# Agent Retrospective Log

A running log of process mistakes made by an AI agent working autonomously
on this repository — not code bugs (those belong in issues, tests, and
fixes), but mistakes in *how the work was done*: wasted time, wrong
assumptions, thrashing against a moving target, or a convention violated
before it was caught. The purpose is retrospective learning across
sessions, not blame — this file has no bearing on code correctness and is
never a merge gate.

## How to use this file

- **When to add an entry:** when a mistake cost meaningful time or
  produced a wrong intermediate result, and the lesson would help a future
  session avoid repeating it. Do not log routine debugging, ordinary
  compiler errors, or a first-try success — only genuine process mistakes.
- **What to write:** date, one-line title, what happened, the root cause,
  what fixed it, and the lesson in a form a future session can actually
  act on ("stop after N failed identical attempts and switch approach",
  not "be more careful"). Keep entries factual and specific — cite the
  actual commit, PR, or file where relevant instead of paraphrasing.
- **When NOT to add an entry:** a mistake immediately self-corrected within
  the same turn with no lasting effect; a disagreement about a genuinely
  ambiguous design call (that belongs in `docs/DECISIONS.md` as a decision
  with alternatives, not here as a mistake); anything containing
  credentials, secrets, or personal information.
- Newest entries first.

---

## 2026-07-26 — Re-verifying before picking an ADR ID isn't enough against a live concurrent actor; park the tail ahead instead

**What happened:** PR #132 (PR-5, "Codegen depth") hit the *same* ADR-ID
collision with `main`'s independent concurrent actor four separate times
within one session, despite following the exact lesson recorded below
("re-check the current highest ID immediately before picking a new one").
Each time, this branch renumbered its own colliding tail to whatever was
free *at that moment* (D-048–053 → D-056–061 → D-057–064), and each time
`main` advanced again before the next push landed, reusing the next ID
this branch had just claimed (`D-056` for MIR-mirror, then `D-056` again
for source-aware telemetry, then `D-062` for fixed-replicate
stabilization). Re-verifying immediately before writing an entry does not
help when the other actor's own next commit — landing minutes to hours
later, with no coordination signal — claims the exact ID just re-verified
as free.

**Root cause:** "re-check before picking" only defends against *stale*
information; it does nothing against a genuinely *live* concurrent writer
with no reservation protocol. Adjacent-to-the-current-tip numbering
guarantees a race whenever both sides advance the tip during the same
open-PR window, no matter how recently either side last checked.

**What fixed it:** on the third and fourth collisions, stopped picking
"the next free ID after the current tip" and instead parked this branch's
entire remaining tail (four entries: str-leak correction, the
renumbering-record itself, the `print()`-nested-expression boundary, and
the `RelocMode::PIC` fix) at D-070–073 — a block chosen to sit well ahead
of `main`'s observed advancement rate, not merely past its tip at that
instant. `main`'s own next two advances (D-062's refinement, then new
D-066) landed with zero further collision against that parked range.

**Lesson:** against a live, uncoordinated concurrent writer to the same
ID sequence, "re-verify immediately before picking" bounds staleness but
not races — prefer parking a colliding tail several IDs beyond the other
actor's *observed rate of advancement* (not just its current tip) once a
collision has already happened twice, rather than continuing to claim
the adjacent-next ID each time. This trades a temporary gap in the
sequence (harmless — IDs are not required to be contiguous) for
eliminating the renumber-repush-collide cycle for the rest of the PR's
open lifetime.

## 2026-07-26 — A handoff correction was drafted against moving PR state

**What happened:** the session snapshot committed in `1671223` still
described PR #137's refresh onto `main` as in progress even though that merge
commit itself completed the refresh. An independent review caught the stale
handoff. While its first uncommitted correction was being reviewed, PR #137
merged as `45545bb` and its post-merge checks completed, so the proposed
replacement immediately became stale too. The original snapshot reached
`main` through PR #137; the stale corrective draft did not.

**Root cause:** exact GitHub state was gathered while drafting the snapshot
and then treated as stable through the review interval. D-066 required a
commit-grounded snapshot, but the operational rule did not explicitly require
one final fetch and PR/check re-resolution immediately before committing it.

**What fixed it:** stopped when a fresh fetch showed that `origin/main` had
advanced, inspected the merge commit and its exact post-merge CI and history
audit, re-read the current PR state and unresolved threads, and replaced the
stale current-state handoff with a newer snapshot. The commit-boundary refresh
is now an explicit rule in `AGENTS.md`.

**Lesson:** treat external PR and CI status in a handoff as volatile until the
commit is created. Immediately before committing, fetch and re-resolve every
referenced head, merge state, review thread, and check; if anything moved,
rewrite the newest snapshot instead of preserving completed work as a future
step.

## 2026-07-26 — CI monitoring started before checking the pull-request state

**What happened:** agents monitoring
[PR #132](https://github.com/rotnov/pycc/pull/132) treated the missing
head-branch CI checks as work still in progress and waited for them. A live
PR-state query at 12:58 UTC instead reported the open PR as
`mergeable=CONFLICTING` and `mergeStateStatus=DIRTY`; only the separate
`Workflow policy` check was present. The useful next action was conflict
resolution, not another CI poll.

**Root cause:** the monitoring loop started from the checks collection and
interpreted an absent or incomplete check set as a timing condition. It did
not first establish whether the PR was open and ready, whether its head was
current, or whether conflicts prevented the normal head workflow from
starting.

**What fixed it:** queried the PR's lifecycle and mergeability fields before
examining its checks, surfaced the conflict immediately, and recorded the
ordering rule in `.ievo/evolution/project.md`.

**Lesson:** before waiting for PR CI, inspect `state`, `isDraft`, head SHA,
`mergeable`, and `mergeStateStatus`. A closed, merged, draft, stale, or
conflicting PR needs state-specific handling; only a PR that can actually
run its required workflows belongs in the CI polling loop. Distinguish a
base-trusted `pull_request_target` policy check from the ordinary head CI
whose absence may be the symptom being diagnosed.

## 2026-07-26 — A parallel agent changed this file's introducing PR branch

**What happened:** while this pull request (adding this very file and
`docs/SESSION_LOG.md`, originally drafted as ADR `D-054`) was still open,
a second, independent agent session pushed a new commit to this PR's
branch. That commit rewrote the PR-5 snapshot from six colliding ADRs
(`D-048` through `D-053`) to five on the assumption that PR-5 had never
used `D-053`. Branch-scoped inspection showed that assumption was false:
the PR-5 branch has a `D-053` table entry as well as references to it in
the detailed `D-052` section.

**Root cause:** two agent sessions, given the same standing goal and the
same repository state, edited the same active PR branch without first
coordinating ownership or verifying their branch-specific claim against
the referenced PR-5 commit. A plausible prose correction was treated as
authoritative before the exact source snapshot was inspected.

**What fixed it:** fetched the new remote head, confirmed it was a direct
descendant of the reviewed head, and fast-forwarded the clean local
worktree. Then compared the remote commit rather than overwriting it,
verified the count with a branch-scoped `git diff`, and restored the six
actual colliding IDs in both files.

**Lesson:** before changing an active PR branch, confirm ownership and
current head; after any unexpected remote advance, preserve it and audit
the exact delta before proceeding. Verify concrete claims against the
named snapshot with branch-scoped commands — never infer a feature
branch's contents from `main` or from prose in the competing change.

## 2026-07-26 — Two three-way ADR ID collisions from a concurrent independent actor

**What happened:** while executing PR-5 ("Codegen depth") on a long-lived
feature branch, this session picked ADR IDs D-047 through D-052 based on
the highest ID visible in `docs/DECISIONS.md` at the moment the branch was
created. A second, independent automated actor (a separate agent preparing
concurrent pull requests for the same repository, unrelated to this
session) continued advancing its own D-047 through D-053 sequence in
parallel, for entirely different decisions (frontend-performance-gate CI
activation work). Those decisions entered `main` through reviewed pull
requests before this branch was ready. The branch's own D-047 happened
to match what later landed on `main` (both
were the same decision, converged independently), but D-048 onward
diverged: the branch's D-048 ("PR-5's MIR stays a typed structural mirror
of HIR") collides with `main`'s D-048 ("Stage and activate the performance
gate with exact-predecessor artifacts") — same ID, unrelated content.

**Root cause:** ADR IDs were picked once, at branch-creation time, and
never re-verified against `main`'s live tip during the ~24 hours the
branch stayed open executing an 11-task plan. `docs/DECISIONS.md`'s own
header ("changing an accepted decision requires a new entry, not an
edit") assumes IDs are claimed close to when they're recorded, not
reserved speculatively for a whole multi-day plan up front.

**What fixed it / will fix it:** the plan's own task briefs already
carried a defensive note ("re-verify the actual next-free ID at execution
time... this branch keeps integrating `main`"), which caught the
divergence before it caused a real conflict — but only because a human
question happened to prompt a fresh `git log`/`grep` check partway
through. Renumbering the branch's D-048 through D-053 (6 IDs: D-048
through D-053 are table entries, with a detailed section for D-052) to
whatever is actually free on `main` at merge time is a mechanical fix,
tracked as a pre-merge cleanup step for that branch.

**Lesson:** when a multi-task plan front-loads a block of ADR IDs (a
whole plan's Task 1 reserving IDs for Task 3 through Task 9's later
decisions), treat every one of those IDs as **provisional** until the
task that actually records it runs — re-check `docs/DECISIONS.md`'s
current highest ID immediately before writing each entry, not just once
at plan-authoring time. This project has independent, active automated
contributors whose pull requests can merge into `main`; any ID claimed
more than a few hours in advance should be assumed stale.

## 2026-07-26 — Three staged-digest reconciliation rounds before deciding to decouple

**What happened:** merging `origin/main` into a PR-4 feature branch
surfaced a CI trust-anchor structural validator
(`scripts/check_roadmap_evidence.rb`'s `TRUSTED_PERF_LIFECYCLE_STEPS`)
that a concurrent, independent actor had added for a `frontend-perf-gate`
job shape incompatible with the branch's own two-job split design. This
session spent three separate rounds — reverting `ci.yml` to a single-job
shape, recomputing SHA-256 digests, discovering the target digest itself
had moved again — trying to reconcile the branch's design against a
target that kept changing underneath it, before stepping back and
deciding to defer the entire feature to a later PR instead (recorded as
`docs/DECISIONS.md` D-047).

**Root cause:** no explicit stopping rule for "reconciling against a
target owned by someone else." Each round felt like "just one more fix"
right up until the third failure.

**What fixed it:** a deliberate decision to decouple — diff-check
confirmed the *entire* delta between the branch's `ci.yml` and `main`'s
own copy was exactly the contested feature, so reverting it to
byte-identical and deferring the feature to its own future PR let the
actual deliverable (frontend-depth compiler work) merge with zero
CI-trust-anchor delta, no staging round needed.

**Lesson:** cap reconciliation attempts against infrastructure or trust
anchors owned by a different, independently-evolving actor at **two**
rounds. If the second attempt still doesn't converge, check whether the
contested piece can be cleanly reverted and deferred to its own focused
follow-up change instead of continuing to chase a moving target inside an
unrelated PR's merge.

## 2026-07-26 — Four consecutive background-agent stalls before switching to manual work

**What happened:** while executing PR-5's subagent-driven-development
plan, a task-review dispatch (Task 8) stalled four times in a row with an
identical infrastructure "no progress for 600s" watchdog failure —
across a full prompt, a retry, a foreground attempt (interrupted), and a
deliberately shortened lean prompt. The same failure mode then recurred
for Task 9's *implementer* dispatch, three times, before this session
switched to implementing that task directly rather than continuing to
retry the same dispatch pattern.

**Root cause:** the failures were transient background-agent
infrastructure issues, not anything about the task content (confirmed:
the diff file involved was verified healthy — normal size, ASCII text,
no pathological lines — and a later, unrelated task dispatched fine).
But four retries of essentially the same approach were spent before
adapting, rather than pivoting after the second identical failure.

**What fixed it:** for Task 8, reading the two source files directly and
performing the review inline instead of dispatching another agent. For
Task 9, implementing the task directly (with the same TDD discipline and
coverage gate) instead of re-dispatching a fifth time, after confirming
via `git status`/`git diff` exactly how far each failed attempt had
gotten so no completed work was silently discarded.

**Lesson:** after **two** consecutive identical infrastructure failures
on the same dispatch (not two different failures — the same watchdog/
timeout signature), stop retrying the same shape of call. Check what
partial progress (commits, uncommitted diffs) the failed attempts left
behind before starting over, and either do the work directly or change
something structural about the dispatch (model, scope, foreground vs.
background) rather than resubmitting the identical prompt a third time.

## 2026-07-25 — `pycc_rt`'s staticlib build-order trap caused one false-negative test run

**What happened:** after editing `crates/pycc_rt/src/lib.rs` directly (in
the Task 9 manual-implementation episode above) and running `cargo test
-p pycc_rt` (which passed), a subsequent `pycc_codegen` end-to-end test
that links and runs a real compiled binary against `pycc_rt`'s staticlib
failed with the *old*, pre-edit panic message — the compiled test binary
had linked against a stale `libpycc_rt.a` from before the edit.

**Root cause:** `pycc_rt`'s own crate-level doc comment already documents
this exact trap (its staticlib output is consumed by linking, not by
Cargo's normal dependency graph, so `cargo test -p pycc_codegen` alone
does not know to rebuild it) — the documentation was read once, early in
the session, but not re-applied at the point it mattered several hours
later.

**What fixed it:** running `cargo build -p pycc_rt` explicitly before
re-running the `pycc_codegen` test, which then passed correctly.

**Lesson:** a documented sharp edge that isn't a link in the immediate
next step's instructions gets forgotten under context load. When a task
brief or dispatch touches `pycc_rt`, restate the build-order requirement
inline in that specific task's instructions rather than relying on
having read it once at the top of a long session.
