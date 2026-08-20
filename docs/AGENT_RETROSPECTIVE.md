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

## 2026-08-20 — Chased a phantom flaky test for hours because a dispatched implementation agent was still writing the same file

**What happened:** While finishing issue #624's review-fix round, two new
in-crate codegen tests failed together under `cargo llvm-cov`, then passed
seven consecutive times under identical commands. Four root-cause
hypotheses were raised and each disconfirmed with direct evidence: an
unguarded emitter call site (grep proved every refcount call routes through
one helper), a race on the global `Target::initialize_all` (it runs after IR
construction and never touches the module), the release optimization
pipeline rewriting the guard chain (`run_passes` is gated on `release ==
true`, and the tests pass `false`), and a per-process `HashMap` seed
reordering emission (the only iterated map on that path is a `BTreeMap`).
The actual cause was that `issue-implement` step 4's dispatched background
implementation agent was **still alive and editing
`crates/pycc_codegen/src/lib.rs` in the same worktree** the orchestrating
session was debugging in. It was detected only when a three-line `eprintln!`
debug patch, confirmed present earlier in the session, vanished from disk
without being removed, and the file's mtime was later than both failing
coverage logs at a time no edit had been made. The agent's own final status
was "Clippy and the full test suite are green. Waiting on coverage" —
proving it was running gates against the same tree concurrently.

**Root cause:** The orchestrating session took over the dispatched agent's
work directly — reading, editing, and running gates on the shared worktree —
without first confirming the agent had terminated. `issue-implement` and
`AGENTS.md` bound how long to *wait* on a stalled subagent, but neither says
to kill a dispatched implementation agent before assuming ownership of its
files. Two writers on one file makes every compile a race against an
arbitrary intermediate state, which presents exactly as a nondeterministic
test.

**What fixed it:** `TaskStop` on the dispatched agent, then verifying tree
coherence (`git status`, `git diff` against the index, grep for debug
residue) and re-running every gate from a single-writer baseline. No test or
production code changed — the "failure" was never in the diff.

**Lesson:** Before debugging a file the current session did not just write
itself, enumerate live background tasks and terminate any that share the
worktree. A dispatched agent that has reported its result may still be
running; a report is not a termination. And once two writers have shared a
tree, **every** gate verdict taken during that window is void — including
the green ones — so re-run the full set, not just the one that failed.

---

## 2026-08-19 — Reintroduced a Windows access violation that already had its own accepted decision entry (D-029)

**What happened:** While implementing issue #148, new codegen tests called
`module.print_to_string()` and let the returned `LLVMString` temporary drop
normally. Local macOS and Linux runs were green; Windows CI failed with
`0xC0000005 STATUS_ACCESS_VIOLATION`. The repository already had an accepted
decision entry describing exactly this failure — `inkwell`'s `LLVMString`
`Drop` calls `LLVMDisposeMessage`, which faults against the prebuilt LLVM the
Windows runner uses — and an existing in-tree remedy, `llvm_string_to_owned`
(`.to_string()` then `std::mem::forget`). The fix in commit `7434e205` was to
route the new call sites through that helper, i.e. to apply a remedy that had
been written, accepted, and merged before the offending code was typed.

**Root cause:** The D-021 preflight reads `docs/SPEC.md` and the
specifications owning the affected area, but an accepted decision entry about
a *host-platform hazard in a dependency* is not owned by any area
specification — it is discoverable only by searching `docs/decisions/` for the
API being called. Nothing in the workflow prompts that search at the moment a
new call to a third-party API is introduced, so the hazard is invisible until
the one Tier-1 platform that manifests it runs, which is always after the
local gates are already green.

**What fixed it:** Commit `7434e205`, replacing the direct `print_to_string()`
drops with `llvm_string_to_owned`.

**Lesson:** When introducing a call to a third-party API that returns an
owned handle — anything whose `Drop` runs foreign code — grep
`docs/decisions/` for that API's own name before writing the call, not after
CI fails. A green local run on one platform is not evidence for a hazard whose
accepted decision entry says it only manifests on another. This class of
defect cannot be caught by the local gate set at all, so the search is the
only cheap rung available.

## 2026-08-19 — Treated `ci-watch.sh`'s terminal line as authoritative and nearly reported a still-running PR as green

**What happened:** While waiting on CI, the bundled
`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh` emitted its terminal
"all checks completed with no failures" line, twice in one session, while
`gh pr checks` on the same head still listed jobs in a pending state. Taking
that line at face value would have reported a pull request as fully green
while required checks were still running.

**Root cause:** The watcher's terminal line is a summary of the checks it has
observed reach a conclusion, not an assertion that every required check has
started and concluded. A required check that has not yet been created for the
head — a workflow that is queued but not yet materialised as a check run —
is absent from the watcher's view rather than pending in it, so "no failures
among what I can see" reads identically to "green".

**What fixed it:** Confirming the watcher's verdict against `gh pr checks`
directly before acting on it, and treating the watcher as a wake-up mechanism
rather than as the verdict itself.

**Lesson:** A watcher that polls a remote system reports what it has observed,
not what exists; its terminal signal is a prompt to check, not a result to
act on. Before merging or reporting a pull request green on any watcher's
say-so, re-query the authoritative surface and confirm the required-check set
is complete as well as passing. The general form: never let a convenience
wrapper's summary be the last read of a gate whose verdict decides an
irreversible action.

## 2026-08-19 — Misread llvm-cov's summary arithmetic twice, and shipped a "fix" that every merged *and* per-range view called complete while CI stayed red

**What happened:** PR #615 (issue #603, general unary `-`/`+` on non-literal
operands) failed `build-test-coverage`. The new `HirExpr::UnaryOp` arms in
`pycc_hir`, `pycc_mir`, and `pycc_types` were exercised end to end by
`tests/issue_603_unary_general_operand.rs` (25 passing tests, confirmed
running under the coverage build), but `cargo llvm-cov --show-missing-lines`,
LCOV, a JSON `segments` walk, and annotated text all reported the touched
crates as fully covered. Aggregating the JSON per-function `regions` arrays
*per source range across instantiations* found zero uncovered ranges, so a
first round of inline unit tests was pushed as complete — and CI came back red
again at 99.95%, with 16 missed regions still in
`crates/pycc_types/src/lib.rs`. Six further arithmetic models were tried
against the data and ruled out (per-function zero regions; union of ranges;
min of ranges; region sum by unique function name; count of fully-uncovered
instantiation groups) before the right one was found.

**Root cause:** LLVM's per-file summary is neither the union nor the sum
across compilations. `RegionCoverageInfo::merge` in `CoverageSummaryInfo.h`
takes `Covered = max(Covered, RHS.Covered)` and
`NumRegions = max(NumRegions, RHS.NumRegions)` over each *instantiation group*
— functions keyed by definition location (file, line, column), which is how
the plain and `--cfg test` compilations of a crate group together — and then
sums those per-group maxima per file. So a function whose regions are covered
by *different* instantiations still shows
`NumRegions - max(Covered)` missed, while every union-based view shows it
fully covered. Here `collect_expr_constraints`
(`crates/pycc_types/src/lib.rs:1168`, 549 regions) had 533 regions covered by
the `--cfg test` instantiation and the remaining 16 — the deferred-constraint
branch of its `HirExpr::UnaryOp` arm — covered only by the `pycc` binary's
instantiation, via an integration test.

**What fixed it:** a group-max deficit computation over
`cargo llvm-cov --workspace --json` (group `data[].functions[]` by
`min((r[0], r[1]))` over the target file's regions; per group,
`max(len(regions)) - max(count of regions with count > 0)`), which reproduced
CI's figure of 16 exactly from local data and named the function and lines.
Then three inline `pycc_types` tests driving that branch from the crate's own
unit-test binary, so a single instantiation covers all 549 regions. Earlier
commits `3ceb334` (inline tests in each crate) and one `?` → `let _ =` in
`rewrite_generic_calls_in_expr`'s unary arm — matching the identical decision
already commented on the `isinstance` arm above it — were necessary but not
sufficient.

**Side lessons from the same session:** a stray `default_*.profraw` from a
coverage run got picked up by `git add -A` and had to be amended out;
`rm -rf target/debug` to free disk silently broke every `pycc build`
integration test (`error: no pycc_rt build found`) until `cargo build -p
pycc_rt -p pycc_std` restored it, wasting a whole coverage run misread as a
real regression; and the container hit ENOSPC twice because the `pycc build`
integration harness leaks a temp directory per run — 12,706 `/tmp/pycc_*`
directories totalling ~25 GB, cleared with `rm -rf /tmp/pycc_*` (100% → 34%
disk). Check for that leak before concluding the disk allowance itself is
exhausted.

**Lesson:** when the coverage summary disagrees with *any* other view, the
disagreement is about instantiation grouping, not about report format — do not
try successive formats, and do not trust a per-source-range aggregation of the
JSON regions either, because that is just the union in another shape. Compute
the group-max deficit and confirm it reproduces the gate's own number before
believing a fix is complete. And treat "an integration test covers it" as
insufficient by construction: an arm reachable only through the `pycc` binary
needs its own inline unit test, because coverage does not compose across a
crate's two compilations. Written up as a durable rule in `docs/TESTING.md`'s
coverage practical-notes list.

---

## 2026-08-09 — `ci-watch.sh` covered `mergeStateStatus=BEHIND` but not the rest of GitHub's non-`CLEAN` enum, so a legitimately blocked PR polled silently forever

**What happened:** PR #417 (a docs-only session-log checkpoint) reached a
state where every required check had completed and passed, but GitHub's
`mergeStateStatus` was `BLOCKED` — an automated Codex reviewer had left an
unresolved review thread, and this repository's branch protection has
`required_conversation_resolution` enabled. `scripts/ci-watch.sh`, running
under `Monitor` per the `autopilot-async-monitoring` skill, never emitted a
line: its `poll_once` function checks for `state != OPEN`, `mergeable ==
CONFLICTING`, `mergeStateStatus == BEHIND`, failed/timed-out/cancelled
checks, and `pending == 0 && mergeStateStatus == CLEAN` — with no branch for
"all checks completed, none failing, but `mergeStateStatus` is something
else." The user noticed the block first (asking about it in chat) and, in
the same turn, guessed a script bug was responsible for the merge being
blocked — which conflated two independent things: the block itself was a
legitimate, separately-real finding (see below), but the *monitoring
silence* about it was indeed a genuine gap the user's instinct correctly
flagged.

**Root cause:** the script's terminal-state coverage was built out
incrementally from the specific failure modes actually observed in past
sessions (`CONFLICTING`/`DIRTY` prompted the fix behind the 2026-07-26 "CI
monitoring started before checking the pull-request state" entry above;
`BEHIND` and failed-checks branches followed similarly) rather than against
the complete set of values GitHub's `mergeStateStatus` field can actually
take (`CLEAN`, `BEHIND`, `BLOCKED`, `DIRTY`, `DRAFT`, `HAS_HOOKS`,
`UNKNOWN`, `UNSTABLE`). Each fix closed the one gap that had just caused
pain, leaving the untested remainder of the enum — including `BLOCKED`,
arguably the single most common "everything passed but you still can't
merge" state — silently unhandled. `scripts/test-ci-watch.sh`'s fixtures
mirrored the same incremental coverage, so nothing caught the gap before it
was hit live.

**What fixed it:** added a catch-all branch — `pending == 0 && merge_state
!= "CLEAN"` (reached only after the `BEHIND` and failed-checks branches
above it have already handled their own cases) — that reports `PR #$pr:
BLOCKED -- all checks completed with no failures, but
mergeStateStatus=$merge_state (not CLEAN) -- ...` and stops polling that
PR, plus a new fixture asserting this exact line instead of a hang.
Independently, the PR's actual block (the Codex thread) was a real,
separate finding worth fixing on its own merits — a session-log entry had
told a future session to run a plain `issue-implement #416`, which would
have closed a multi-phase issue prematurely after only its first phase
merged.

**Lesson:** when a polling/watch script's terminal-state branches are
derived from "the specific failure we just hit" rather than from the
target API's actual enum of possible values, audit the full enum once and
add an explicit catch-all for "recognized-terminal-but-uncategorized"
rather than trusting the branch list to stay complete by accretion. A
script whose job is specifically to replace silent waiting with a reported
signal is worse than no script at all in exactly the states it fails to
recognize — silence there reads as "still working," not "nothing to
report."

---

## 2026-08-07 — Proved a check "unreachable" by varying only one dimension of a two-dimensional equality; nearly deleted live code

**What happened:** diagnosing the D-014 coverage gap regression on `main` (introduced
by PR #358, `f4b3978`), the session found that `check_and_resolve`'s post-resolution
call to `check_incompatible_redefinitions` was the one uncovered branch. It wrote one
test — a 1-parameter `Ty::Infer` function redefined with a 1-parameter `Ty::Int`
function (same arity, different element type) — observed the redefinition silently
accepted, concluded the post-resolution call "can never fire," filed it as such in a
P1 issue (#402), and staged a diff deleting the call as dead code together with
rewritten doc comments asserting the same. The predicate the call actually evaluates,
`check_incompatible_redefinitions`'s `prev != &current` on `(Vec<Ty>, Ty)`, has two
independent dimensions: the parameter *types* and the `Vec`'s *length* (arity). The one
test varied only the first dimension. `check_and_resolve`'s resolution loop
(`params.iter_mut().zip(resolved_params)`) overwrites each item's own parameter types
in place but never changes an item's parameter count, so same-arity redefinitions
converge to identical resolved signatures (masking the mismatch, as observed) while
different-arity redefinitions keep their own distinct lengths post-resolution and the
comparison still catches them. The mistake was caught only because a concurrent
automated actor (PR #403, `db2f9cf`) independently fixed the same coverage gap by
adding a test that exercises exactly the untested arity-mismatch dimension, and the
D-021 preflight's mandatory `git fetch` immediately before commit surfaced that
commit's conflicting fix before the deletion was pushed — this was luck in the timing
of a concurrent write, not a safeguard the session itself had in place.

**Root cause:** treated one passing/failing test case as proof of a branch's universal
(un)reachability without checking that the test varied every dimension the branch's
own comparison logic reads.

**What fixed it:** discarded the staged deletion, independently re-verified PR #403's
test against a fresh worktree before trusting its commit message, corrected the
now-falsified "can never fire" claims in issue #402 and the misleading doc comments
that had encoded the same overclaim, and landed a narrower doc/comment-only fix
describing the real three-way boundary (both concrete: rejected any arity; one
inferred, arities differ: rejected post-resolution; one inferred, arities match:
silently collapses — #402).

**Lesson:** before concluding a branch is unreachable from empirical test results,
enumerate every independent variable the branch's own comparison or guard condition
reads (here: both element-wise content and container length), and construct at least
one test case that isolates each one. A single test that happens to vary only one
dimension of a multi-dimensional predicate proves nothing about the others.

---

## 2026-08-05 — Used `sleep 240` to wait on CI instead of `ci-watch.sh`; missed `autopilot-async-monitoring` skill at the CI-wait fork

**What happened:** during the `issue-implement` run for #345 (PR #348), the session
reached the CI-monitoring step and waited on the pull request's check suite using
`sleep 240` followed by a manual `gh pr view` re-check — exactly the fixed-interval
polling pattern the `autopilot-async-monitoring` skill (and its `scripts/ci-watch.sh`
mechanism) exists to replace. The user pointed this out ("а чего ты не используешь
скил autopilot-async-monitoring"). The skill was available and its description
directly covered the situation ("deciding how to wait on async state such as a pull
request, a CI run"), but the session did not re-scan the skill list at the CI-wait
fork — it had applied skill-selection discipline once at session start (invoking
`issue-implement`) and then stopped re-evaluating at each subsequent sub-step.

**Root cause:** trigger gap. `issue-implement`'s step 7 (Monitor) already said
"Before waiting on CI, query the pull request's current state" but did not
cross-reference `autopilot-async-monitoring` or name `ci-watch.sh` as the mechanism
for the wait itself. The skill that should have been invoked was discoverable but
not pointed at from the skill the session was actively running — so the agent reached
for the familiar `sleep` pattern instead. This is the same failure mode the
`autopilot-async-monitoring` skill's own creation history documents (four
`.ievo/evolution/project.md` entries with `Trigger: user-observed mistake during PR
monitoring` → extracted into the skill), but the extraction did not close the loop
back from `issue-implement` to the extracted skill.

**What fixed it:** PR #349 added a cross-reference from `issue-implement` step 7 to
`autopilot-async-monitoring` and `scripts/ci-watch.sh`, so a future session reaching
that step picks up the right tooling directly from the skill text it is already
following. This same session then used `ci-watch.sh` for the remaining CI waits
(PR #348 merge, PR #349 CI, and PR #350 for this skill's own delivery) — all three
reported terminal state within seconds of it happening, with no fixed-interval dead
time.

**Lesson:** skill selection is not a one-time event at session start — re-scan the
skill list at each fork where a new kind of work begins (waiting on async state,
writing tests, designing a module, reporting a bug). A skill that exists but is not
pointed at from the skill currently running is invisible at exactly the moment it
would have helped. When a user corrects a process choice, that is the strongest
signal a trigger gap exists — diagnose which artifact failed to surface the right
skill at the fork, do not just fix the one instance. This lesson is now encoded in
the `process-error-postmortem` skill (PR #350), which fires at exactly this moment
(self-caught or user-caught process mistake) and walks the diagnosis-to-fix loop
explicitly.

## 2026-08-02 — Five plan-review rounds spent before a one-grep check would have killed the pick at selection

**What happened:** issue #243 (add subprocess/CLI-boundary tests to
`scripts/test_check_search_visibility_audit.py`) passed `issue-select`'s
premise-verification and adversarial-advisor round cleanly, then went
through 4 rounds of `issue-to-plan`'s adversarial review loop fixing real
but comparatively minor issues (wrong citations, a wrong decision number, a
Gates-section restructure) before round 5 found the actual blocker: the
target file is itself a `tests/fixtures/policy-successor-manifest.json`
(D-103) protected entry, so a direct single-PR edit would fail the
required `audit` check outright. That fact is checkable in one command
(`grep test_check_search_visibility_audit.py tests/fixtures/policy-successor-manifest.json`)
and does not depend on anything in the plan's own content — it would have
been true on round 0, before a single word of the plan was drafted.

**Root cause:** neither `issue-select`'s blocker screen nor
`issue-implement`'s staged-pattern detection ever checked the manifest at
all — both only knew about the narrower, `ci.yml`-specific D-080
digest-allowlist mechanism (see this session's own fix, PR #279). So
nothing in the selection or early-planning path was positioned to catch
this before real planning effort had already gone into a single-PR shape
that could never land. The four earlier review rounds were not wasted in
isolation — their fixes were real — but all of that work was downstream of
an unverified premise (a manifest-protected file can be edited directly)
that a single grep would have refuted immediately.

**What fixed it:** the issue was set aside (denylisted, no code changed;
see `docs/SESSION_LOG.md`'s 2026-08-02 entry), and the actual gap — no
manifest check anywhere in the selection or planning path — was folded
back into `issue-select` and `issue-implement` directly (PR #279), so a
future run's baseline/preflight step now checks the manifest before
selecting or planning anything.

**Lesson:** when a repository has a structural, mechanically-checkable
precondition for "can this file be edited in a single PR at all" (a
digest pin, a protected-manifest entry, a generated-file marker), that
check belongs in the *selection* or *earliest preflight* step, checked
against the literal target file list, not discovered organically partway
through plan review. A multi-round adversarial review loop is good at
catching reasoning errors in a plan's content; it is a comparatively
expensive way to discover a precondition that a one-line structural query
would have settled before the plan had any content to review.

---

## 2026-07-31 — A rerun with identical replicate medians is a cached duplicate, not a second data point

**What happened:** while investigating D-109's `frontend-perf-gate` regression, a `gh run rerun` of a passing CI run (30613065177) was treated as producing "two independent, genuinely fresh" measurements, and `docs/DECISIONS.md`/`docs/ROADMAP.md`/`docs/SESSION_LOG.md` were committed and pushed recording both a 1.8430% and a -0.4454% delta as separate confirming evidence that the regression was closed. Neither attempt's job log was actually diffed against the other before writing "confirmed closed." When a later, unrelated investigation prompted pulling both attempts' raw logs directly, they turned out to report byte-identical replicate medians and an identical -0.4454% delta — attempt 2 had reused attempt 1's cached artifacts rather than remeasuring, and the 1.8430% figure matched no retrievable log at all. The false "confirmed closed" claim then had to be withdrawn across four documentation files days into the branch's life, alongside a second, worse finding it surfaced (a pre-fix commit passing at 0.81% right next to another pre-fix commit failing at 6.52% with zero code change between them — undermining the original "confirmed regression" finding too, not just its closure).

**Root cause:** this project already has an explicit, named methodology for this exact trap (D-095/D-096/D-101's "check whether the rerun actually remeasured," first learned from an earlier `--failed`-only rerun in this same investigation), but it was applied by checking `frontend-perf-measure`'s *timestamp* for freshness, not by checking whether the *comparison output* (replicate medians, delta) actually differed between the two attempts. A fresh timestamp only proves the job re-executed; it does not prove it produced a new measurement if, e.g., the "current" artifact was re-fetched from an unchanged upstream branch tip while only the "previous" side moved, or any other path that leaves the recorded numbers unchanged. The doc claim was written from the two attempts' *existence*, not from a diff of their *content*.

**What fixed it:** re-fetching both attempts' full job logs with `gh run view --job <id> --log` and comparing the actual `previous replicate medians` / `current replicate medians` / delta lines character-for-character, which immediately showed the duplication no timestamp check had caught.

**Lesson:** when treating two CI attempts as independent measurements, diff their actual reported numbers (replicate medians and delta), not just their timestamps or attempt IDs — a fresh timestamp with identical output numbers is still a cached duplicate. Do this check before writing any doc claim of the form "N independent measurements confirm X," not after a later session stumbles onto the discrepancy by accident.

## 2026-07-31 — A `cargo llvm-cov` region gap with no uncovered line means a per-instantiation gap, not a mystery

**What happened:** PR-10 Task 11b (`pycc_codegen`'s `list[int]` wiring) is
the first commit on that branch where `cargo build --workspace` goes green,
so it is also the first time D-014's coverage gate could run there. It
reported `crates/pycc_codegen/src/lib.rs` at 99.68% regions / 99.73% lines
— but every drill-down disagreed: `--show-missing-lines` named a single
line, the merged `--text` and `--html` reports contained no zero-count line
at all, and summing the JSON export's region counts by source span gave
zero uncovered regions against a total that exactly matched the summary's
own. Roughly an hour went into reconciling those views (including two
throwaway baseline worktrees, the first checked out at a commit that
predated the gate breakage but was itself still red).

**Root cause:** `pycc_codegen` is compiled more than once in a workspace
coverage run — once for its own `#[cfg(test)]` unit-test binary, and again
as an rlib for the integration tests and the `pycc` binary they spawn. The
mangled names differ per compilation, so llvm-cov's file summary accounts
for those copies separately even though every human-readable report merges
them. Code exercised only through `tests/slice1_codegen_depth.rs` (which
drives the separate `pycc` binary) can therefore leave the unit-test copy's
regions unexecuted, and the summary counts that — with nothing to point at
in any per-line view, because the merged view really is fully covered.

**What fixed it:** adding two `pycc_codegen` unit tests that exercise the
same paths the integration suite already covered — a `ForList` loop run to
completion (the increment-and-branch-back block; the existing unit test
returned on the first iteration and never reached it) and a module-level
`list[int]` global. That took the workspace to 100%/100% with no production
change. A third such test was added later for `MirExpr::ListAppend`'s body.

**Lesson:** when the coverage summary reports a gap that no per-line view
can locate, stop looking for the missing line — it does not exist. Ask
instead which *binary* fails to reach the new code, and add a test in the
crate's own `#[cfg(test)]` module rather than only an end-to-end one. As a
default for this repository: any new `pycc_codegen` arm needs a unit test
in that crate, even when `tests/slice1_codegen_depth.rs` already proves the
behavior from real source. Related trap from the same session: `cargo fmt`
with no `-p` swept seven unrelated files that were already unformatted on
the branch into the working tree (CI runs no `fmt` check, so the drift was
pre-existing) — scope it to the crate being edited, then check
`git diff --stat` before staging.

## 2026-07-30 — A digest-pinned file has no "comment-only, no functional change" exemption

**What happened:** PR-9 Task 10's docs sweep edited three stale test-count
comments in `.github/workflows/ci.yml` ("two" → "11"), then a same-day
follow-up commit corrected "11" to "12" after the pinned reviewer caught
the undercount. Both commits pushed clean locally but failed `audit` and
`build-test-coverage` on CI: `scripts/check_roadmap_evidence.rb`'s D-100
composed-workflow check hashes `ci.yml`'s exact bytes against a reviewed,
pinned SHA-256 digest, with no carve-out for comment-only or
"no functional change" edits — the check has no way to distinguish those
from a substantive change, by design (AGENTS.md's CI-privilege-boundary
section states this file is a security trust anchor for exactly this
reason). The plan document itself (`docs/superpowers/plans/2026-07-30-v0-2-pr9-conformance-harness.md`,
Task 10 Step 5) had explicitly called the edit "comment-only... no
functional change" and treated that as sufficient justification — it
wasn't.

**Root cause:** treated "no functional change" as equivalent to "safe to
edit freely," without checking whether the target file carried its own
independent integrity gate. The digest pin is a property of the *file*,
not of the *diff's* runtime effect.

**What fixed it:** reverted both edits (`git checkout origin/main --
.github/workflows/ci.yml`), restoring the exact pinned blob (verified via
`git rev-parse` blob-hash equality and a clean local
`check_roadmap_evidence.rb` + `test_check_roadmap_evidence.rb` run). The
stale comment counts remain in `ci.yml` as a deliberately deferred
cosmetic gap, to be fixed only by a future PR that already legitimately
re-stages the file's digest for some functional reason.

**Lesson:** before editing any file governed by a whole-file digest pin
(check `docs/DECISIONS.md`'s D-090/091/092/099/100 lineage and
`scripts/check_roadmap_evidence.rb` for the current list — as of this
entry, `.github/workflows/ci.yml`), assume there is no such thing as a
trivial edit. Either route the change through the project's existing
stage-then-activate re-pinning process first, or don't make the edit at
all and defer it to a PR that already pays that cost for another reason.
"It's just a comment" is not a reason to skip this check.

---

## 2026-07-29 — Whole-process wall-clock timing has no signal once the workload is a few milliseconds

**What happened:** PR-8 Task 5's first pass at `tests/nbody_bench.rs`
(D-094's same-machine paired nbody benchmark, `pyperformance`'s own
`DEFAULT_ITERATIONS = 20000`) measured a ~10-11x pycc-vs-CPython speedup
ratio, reported as a genuine, investigated shortfall against the design
spec's ≥20x gate (the task's own untracked working notes -- not a repo
file, see `docs/DECISIONS.md`'s D-093 for the tracked, full write-up). A
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
18.24x at 525000) -- full details in D-093.

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
