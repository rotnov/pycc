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

## 2026-07-29 — Whole-process wall-clock timing has no signal once the workload is a few milliseconds

**What happened:** PR-8 Task 5's first pass at `tests/nbody_bench.rs`
(D-090's same-machine paired nbody benchmark, `pyperformance`'s own
`DEFAULT_ITERATIONS = 20000`) measured a ~10-11x pycc-vs-CPython speedup
ratio, reported as a genuine, investigated shortfall against the design
spec's ≥20x gate (the task's own untracked working notes -- not a repo
file, see `docs/DECISIONS.md`'s D-091 for the tracked, full write-up). A
second-reviewer pass re-derived the real cause from that report's own
numbers: CPython's nbody total (68.2ms) minus its own bare-interpreter
baseline (20.3ms) gives ~47.9ms of actual compute; pycc's nbody total
(6.1ms) minus its own trivial-binary baseline (3.0ms) gives ~3.1ms --
already a ~15.5x compute-only ratio, nowhere near the measured 11.2x. The
gap was fixed per-process overhead (~3ms, essentially this machine's own
OS-level process-spawn/codesign-verification floor, not anything pycc-
specific) consuming ~45-50% of pycc's own ~6ms total versus only ~29% of
CPython's ~68ms total -- a 6ms workload cannot support whole-process
wall-clock timing as a clean compute proxy, no matter how carefully the
timing loop itself is written.

**Root cause:** `pyperformance`'s upstream `DEFAULT_ITERATIONS = 20000` was
copied verbatim into the fixture without recognizing that constant is only
meaningful *inside a harness that loops and amortizes startup* (as
`pyperformance` itself does) -- this benchmark instead spawns one fresh
process per measured run, so the iteration count needed to be chosen for
*this* harness's own overhead profile, not inherited from a different
measurement method's constant.

**What fixed it:** raised the fixture's iteration count (525000, chosen by
directly timing several candidates, not by linear extrapolation -- real
measurement showed compute cost does not scale as cleanly as expected) so
both sides' fixed overhead is a single-digit percentage of their own total.
This dropped the noise band from a ~1.3x-wide swing across runs (10.23x-
11.32x at 20000 iterations) to a tight, reproducible ~0.2x band (18.04x-
18.24x at 525000) -- full details in D-091.

**Lesson:** this is the second time in this one PR a benchmark used a proxy
measurement with near-zero signal for what it was meant to measure -- see
the very next entry below (linked-binary size as an "optimizer ran" proxy,
Task 3). Both share the same shape: an artifact whose value is dominated by
something *other* than the thing being measured (fixed process overhead
here; static-runtime size and segment-alignment padding there). Before
trusting a wall-clock measurement of a program that completes in low
single-digit milliseconds, compute (don't assume) what fraction of that
total is fixed per-process overhead by timing a trivial baseline program
the same way -- if that fraction is not comfortably single-digit, the
measurement is measuring the harness, not the workload, regardless of how
many repetitions or median-taking are applied on top.

## 2026-07-28 — Linked-binary size is not a reliable "did O3 actually run" proxy at the CLI level

**What happened:** while writing PR-8 Task 3's end-to-end test for the
`pycc.toml` release-profile default (`tests/pycc_toml_release_default.rs`),
the first draft compared the *final linked binary's* file size between a
plain build and one driven by a neighboring `pycc.toml`'s
`[build] opt = "release"`, mirroring `pycc_codegen`'s own
`release_mode_actually_runs_llvm_optimization_passes` unit test (which
correctly compares raw *object-file* bytes). A negative control (two plain
builds of identical source, expected equal length) initially "passed," but
so did the positive assertion even under a deliberately broken stub that
ignored `pycc.toml` entirely — the proxy had no real signal in either
direction.

**Root cause:** two compounding effects, found by direct empirical
bisection (equalizing string lengths, then explicit `--release` vs. plain
debug in the same directory): (1) every scenario directory's name and
`-o` output filename differed in *string length* across test scenarios,
and some embedded-path mechanism in the linked Mach-O output (plausibly
OSO/STAB debug-map entries or similar) shifts final file size by
approximately that same character-count delta — a confound unrelated to
optimization entirely; (2) once path lengths were held equal, `--release`
and plain debug builds of the same tiny compute loop produced
byte-identical linked output, because the statically-linked `pycc_rt`
runtime (~1.6MB) dominates total size and Mach-O segments pad to fixed
alignment boundaries that absorb a few-hundred-byte `.text` delta from
unrolling a short loop.

**What fixed it:** dropped the binary-size assertion from the CLI-level
test entirely. The end-to-end test now asserts only functional success
(exit 0, correct stdout) through the real relative-path/`current_dir`
route, which is the part not already covered by unit tests. The
optimization-actually-ran claim stays proven where the effect is real and
measurable: `pycc_codegen`'s own unit test comparing raw object-file
bytes for the identical MIR.

**Lesson:** a linked executable's file size is not a trustworthy proxy for
"did the optimizer run" once a large static runtime and OS-level segment
alignment are in the picture — prove optimization effects at the
smallest artifact where they're real (the object file, not the final
binary), and never compare test-scenario file sizes across paths/names of
different lengths without first confirming a negative control that
actually can fail (a control that "passes" under a deliberately broken
implementation is not a control).

## 2026-07-27 — Nearly designed a `roadmap-evidence` content check that would have permanently broken the `workflow-policy.yml` audit

**What happened:** while registering the three new `roadmap-evidence` IDs
PR-7 needed to close v0.1's last three unchecked acceptance-checklist items
(`conformance-fib-mandelbrot-tier1`, `check-throughput-1k-loc-50ms`,
`cli-spec-diagnostic-match`), an automated review correctly flagged that
`scripts/check_roadmap_evidence.rb`'s new evidence IDs only prove CI
*invokes* the right test/script paths, not that their *content* still
asserts real behavior. The natural next step was
to add `validate_evidence` checks reading `scripts/check_frontend_throughput.rb`,
`tests/conformance.rs`, and `docs/CLI_SPEC.md`/its fixture directly from
`root` — mirroring how the existing `ci.yml` digest check already reads that
file from `root`. This was fully drafted before being caught.

**Root cause:** `.github/workflows/workflow-policy.yml`'s `audit` job (the
`pull_request_target` job that actually runs the checker against PR heads)
does not check out the PR's full tree. It checks out the *base* branch's
full tree, then downloads only `docs/ROADMAP.md` and `.github/workflows/*.yml`
from the PR head via the GitHub API into an isolated `/tmp/pr-policy-input`
directory, as inert data. Any `validate_evidence` check reading a file
outside that exact set — `scripts/*`, `tests/*`, any other `docs/*` file —
would hit `Errno::ENOENT` in that sandbox on *every* PR, not just the one
introducing the check. Because the new evidence IDs weren't cited by any
checked box yet, this defect wouldn't have surfaced in the PR that introduced
it (its own audit would pass, since `evidence_ids` wouldn't include the new
ID) — it would have surfaced only in the next PR that tried to check a box
citing it, as a mysterious, permanent audit failure with no obvious
connection to the real cause.

**What fixed it:** reading `.github/workflows/workflow-policy.yml`'s `audit`
job step-by-step (not just the two `ruby scripts/check_roadmap_evidence.rb`
invocation lines already known from prior sessions) before implementing,
which surfaced the `/tmp/pr-policy-input` provisioning boundary. The fix that
survived is a documented, deliberate scope decision (reply-and-resolve the
review thread, tracked as a follow-up task) rather than new code — the only
sandbox-compatible way to content-verify a file is to embed a `shasum`/diff
step *inside `ci.yml` itself* (the one file the audit's sandbox does
provision), matching the pre-existing `PAIRED_PERF_CHECKER_SHA256` pattern.

**Lesson:** before adding any check to `scripts/check_roadmap_evidence.rb`
(or any script invoked by a `pull_request_target` audit job) that reads a
file from its `root` argument, first read the calling workflow's *complete*
file-provisioning step, not just its invocation line — a
`pull_request_target` audit's sandbox is defined by what it provisions as
data, and that provisioning is almost always narrower than "the whole repo,"
even when the checker's own code makes it look like an ordinary filesystem
read. A check that would break every future PR, not just the one adding it,
is exactly the kind of defect that won't show up in that PR's own CI run.

## 2026-07-26 — Re-derived a parallel session's already-planned PR #132 reconciliation from git archaeology instead of reading `SESSION_LOG.md` first

**What happened:** a push to `feat/v0-1-pr5-codegen-depth` was rejected as
non-fast-forward after another session had pushed 5 commits directly to the
same branch (via a `codex/fix-pr132-review-0764` lineage), independently
fixing an overlapping-but-not-identical subset of the same 8 Codex review
findings. Before reading `docs/SESSION_LOG.md`, roughly 30 minutes were spent
manually diffing commits (`git show <sha>:<path>`, function-by-function) to
figure out which findings the other session had already fixed, whether its
`D-074` collided with a local draft entry, and whether the two lineages were
genuinely complementary or in conflict.

**Root cause:** `docs/SESSION_LOG.md` (added by D-066 specifically to answer
"what state is the work in and what's next" across sessions) already
contained a same-day entry recording that exact reconciliation as planned and
partly executed — which commits to keep, which review threads it covered, and
the exact next steps ("push normally... resolve only threads verified against
the resulting remote head... request `@codex review` once for that new
head"). Reading it first would have made the manual diffing largely
redundant: the log already answered "is this a rogue conflicting process or
planned parallel work," which is exactly the question the diffing was trying
to answer from first principles.

**What fixed it:** the manual diffing still reached the correct
conclusion (remote is a superset in every substantive area except two doc
files it never touched), so no rework was needed — but reading the log
partway through confirmed it was reinventing an already-recorded plan.

**Lesson:** when a push conflict or unexpected remote state is discovered on
a branch this project's own automation actively works, check
`docs/SESSION_LOG.md` for a same-branch entry *before* reaching for `git
show`/`git diff` archaeology to reconstruct intent — the log exists
precisely to make that reconstruction unnecessary. Git diffing is still the
right tool to *verify* what the log claims, just not the right first step to
*discover* it.

## 2026-07-26 — Historical governance PRs were mistaken for live monitors

**What happened:** PR #119 and issue/PR-era #125 were included in the live
monitoring set even though their only current role is historical evidence for
the one-shot governance recovery recorded in D-054. This created irrelevant
status noise and required the user to ask why completed history was still being
watched.

**Root cause:** links found in current governance documentation were treated as
operational targets without first checking whether they were open, changing,
or named by an active task. Documentary relevance was conflated with live
state.

**What fixed it:** removed #119/#125 from the monitoring scope and retained only
the active PR #132 plus newly opened PRs and newly merged default-branch
commits.

**Lesson:** build every monitoring set from current remote state first. A PR or
issue referenced by an ADR is historical unless it is still open or the active
task explicitly names it; do not poll documentation citations as live work.

## 2026-07-26 — Retried a hanging Apple Git submodule probe before inspecting it

**What happened:** the exact-revision `pre-commit try-repo` verification for
PR #51 twice stopped after “Initializing environment.” Both attempts were left
waiting for several minutes before the process tree was inspected. The blocked
child was Apple Git 2.50.1 running `git submodule update` in a repository with
no submodules; the same command also hung when invoked directly.

**Root cause:** the second attempt repeated the first with the same Git binary
instead of first reducing the stall to its child process. The visible
pre-commit message was mistaken for a slow Rust environment build even though
Cargo had not started.

**What fixed it:** inspected the process tree, reproduced the empty-submodule
command directly, and then ran the same command with the already installed
bundled Git 2.53.0, which returned immediately. Putting that verified Git first
in the isolated command's `PATH` let `pre-commit try-repo` reach Cargo and pass.

**Lesson:** after one silent repeatable stall, inspect the youngest child and
reduce it outside the orchestrating tool before retrying. Distinguish “no
output” from “build in progress” by confirming that the expected compiler
process actually exists.

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
