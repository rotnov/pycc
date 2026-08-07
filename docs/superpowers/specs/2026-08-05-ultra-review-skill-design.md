# `ultra-review` skill — design

**Goal:** A new project-local skill that periodically re-reviews the codebase for
quality/architecture problems beyond what the D-068 pre-merge gate catches on any
single diff, and files prioritized, milestone-scoped, deduplicated GitHub issues for
what it finds — autonomously, without a human approving each payload.

**Architecture:** A thin orchestration skill around the *existing* pinned D-068
`ievo:deep-reviewer` (same SHA-256, no new reviewer artifact to pin/audit). Each run
reads a GitHub-native checkpoint, computes the diff since that checkpoint, dispatches
the pinned reviewer once, triages its findings into `P1`/`P2`/`P3` GitHub issues after
a dedup pass, and advances the checkpoint. Recurrence is external (existing
`schedule`/Routines on Claude Code, a Codex-side equivalent, or manual invocation) —
the skill itself is only the procedure, not a scheduler.

**Tech stack:** No new dependencies. Reuses `gh` CLI (already used by every autopilot
skill in this repo), the pinned `ievo:deep-reviewer` agent, and this project's own
`P1:`/`P2:`/`P3:` issue-title convention and milestone-at-filing convention
(`AGENTS.md` D-021 step 9, strengthened by D-144).

## Global Constraints

- D-068: only the exact pinned reviewer artifact may be used as a review engine; no
  new marketplace/global reviewer substitution.
- `AGENTS.md` "Support Codex and Claude Code": needs a Claude Code skill plus a
  Codex thin-pointer mirror, discoverable and behaviorally equivalent on both.
- Alpha-skill governance (`docs/AGENT_TOOLING.md`): a new project-local skill this
  shape (deterministic offline contract, not model-judgment-only) registers in
  `scripts/run_alpha_skill_evals.py`'s `EXPECTED_RUNNERS` and
  `scripts/validate_agent_assets.py`'s `validate_alpha_skill_contracts` tuple,
  following exactly the pattern `next-milestone` used in D-144.
- Session's own safety policy: opening a GitHub issue is a "publish public content"
  action that normally requires per-instance approval — this design's autonomous-write
  authorization must be as explicit and bounded as this project's one existing
  precedent, D-022 (autonomous iEvo bug reporting), not a broader blanket grant.
- YAGNI: no new pinned-reviewer artifact, no new scheduling infrastructure, no
  speculative batching/rate-limiting machinery beyond what real found issues warrant.

---

## An empirical detour, and why it changed the design

Before finalizing the architecture, the two live candidate engines were actually run
side by side against the same real accumulated diff (`3f6998c..origin/main` on
`rotnov/pycc`, 8 merged commits, 21 files) rather than decided on paper:

- **Variant A** — the exact pinned `ievo:deep-reviewer`, unmodified, given the diff.
- **Variant C-addon** — a second, custom-prompted agent checking the same diff against
  `docs/ARCHITECTURE.md`/`docs/ROADMAP.md`/`docs/SPEC.md`/cross-skill governance, i.e.
  everything the diff-scoped 11-point checklist structurally cannot see.
- An independent **judge** agent then re-verified every C-addon finding against the
  live repository before trusting it.

Result: Variant A alone found 4 real findings, including a genuine blocker (a diff
that deletes the entire Codex plugin-marketplace mechanism while
`docs/AGENT_TOOLING.md`, D-068's own consequences clause, and
`validate_agent_assets.py`'s cross-surface check all kept describing/enforcing the
now-removed mechanism) — caught because the reviewer's own existing protocol already
reads the *complete* current content of every changed file, not just diff hunks.
Variant C-addon found 3 findings; the judge confirmed one was real but a duplicate of
what A already found, one was **not real** (a false causal claim — it attributed
pre-existing content to this diff), and one was real but explicitly weak/non-blocking.
Worse, C-addon's false claim sat in the exact same file region as a real,
more-severe drift bug (a stale explanatory sentence still describing v0.1 after the
adjacent "Current milestone" line was updated to v0.2) that **neither variant caught**.

**Decision, on this evidence: single pass, Variant A only.** The second architecture
pass did not earn its added cost (a second full agent dispatch every run) in this
real test, and introduced a verification burden of its own. Two concrete lessons from
the miss both variants shared are folded into ultra-review's dispatch prompt instead
of a second pass (see "Review dispatch" below): explicitly ask the reviewer to check
whether prose adjacent to a changed status/summary line still describes the prior
state, and to check `docs/sessions/`/session-log freshness against `AGENTS.md`'s own
checkpoint convention as a cheap explicit checklist line, not a reason for a second
dispatch.

---

## Workflow

### 1. Baseline

Fetch and record the default-branch tip, exactly as every other autopilot skill in
this repo does (D-021-style). Confirm the `gh` CLI is authenticated and the target
repository resolves.

### 2. Read the checkpoint

Search for the tracking issue: `gh issue list --repo <owner>/<repo> --label ultra-review-checkpoint --state all --json number,body`. Deliberately unbounded rather than `--limit 1`: a limit would truncate the result set before the run could ever observe a second checkpoint issue, making the duplicate-checkpoint branch below unreachable.

- **Exactly one found:** parse the last-reviewed commit SHA and timestamp from its body
  (a small, fixed key: value block at the top, machine-parseable, human-readable).
- **None found (first run ever):** create it — title fixed (e.g. `Ultra-review
  checkpoint — do not close`), label `ultra-review-checkpoint`, body initialized with
  the *current* default-branch tip as the checkpoint (so the very first run reviews an
  empty range rather than the entire repository history — a full historical sweep was
  explicitly rejected in the incremental-vs-full-sweep decision below). The two block
  fields that describe a completed run (`Reviewed at:`, `Last run:`) have no bootstrap
  meaning, so the shipped skill fixes their bootstrap values literally rather than
  leaving them to invention — a gap found by the skill's own first live run against
  this repository. Report this bootstrap explicitly; it is a one-time, deliberate
  no-op run.
- **More than one found:** stop and report — two concurrent runs raced. Do not create a
  third checkpoint issue or silently pick one as authoritative.

A tracked file was considered and rejected for this role: this session hit the exact
failure mode directly — a recycled/ephemeral worktree loses local, uncommitted state,
and committing a checkpoint bump would need its own PR every run purely to move a
counter, entangling review-filing with unrelated code changes under `AGENTS.md`'s
protected-`main` rule. A GitHub-native checkpoint needs no PR and survives worktree
churn by construction.

### 3. Compute the incremental diff

`git diff <checkpoint-sha>..<default-branch-tip>`. If empty, report "nothing new since
`<checkpoint-sha>`" and stop — no dispatch, no checkpoint update (advancing it would
be a no-op; recording the checkpoint is only meaningful when it moves).

Chosen over a full-repository re-sweep on every run per the first design decision of
this brainstorm: incremental is bounded in cost, and does not re-surface the same
already-filed finding on unrelated code it happens to graze on every pass — the
dedup step below still exists as a second line of defense, but incremental scope
means it rarely has to do real work.

### 4. Review dispatch

One dispatch, to the pinned `ievo:deep-reviewer`, exactly as `docs/AGENT_TOOLING.md`'s
existing D-068 loop does — same repo-context/changed-files/diff shape the existing
`deep-review` skill already uses. Ultra-review's own dispatch prompt adds two short,
explicit checklist reminders on top of the standard 11 points (informational context
supplied by the *caller*, not a modification of the pinned agent artifact itself,
exactly as this design's own bake-off dispatch already did):

- when a changed line updates a status/summary statement, check whether adjacent
  *unchanged* prose still describes the prior state;
- check whether files under `docs/sessions/` (or the session-log convention
  `AGENTS.md` names) were updated to checkpoint the merges in this diff range, per
  that convention's own non-blocking note.

### 5. Triage findings into issues

For each finding, in severity order (`blocker` first):

- **Severity → priority:** `blocker` → `P1:`, `warning` → `P2:`, `note` → `P3:`.
- **Milestone:** read `docs/ROADMAP.md`'s ordered `## vX.Y` sections the same way
  `next-milestone` does, to find the currently active milestone, then apply the same
  judgment convention `AGENTS.md` D-021 step 9 (as strengthened by D-144) already
  states for any issue filer: assign that milestone unless the finding is clearly
  cross-cutting or genuinely not `vX.Y`-scoped work, and record the
  `no milestone — cross-cutting` reasoning in the body when it is left unassigned.
- **Title:** `<P-prefix>: <one-line summary>` (this repo's existing convention).
- **Body:** the finding's file:line, the concrete issue description, the reviewer's
  suggestion, the diff range reviewed, the unassigned-milestone reasoning note when
  applicable, and a fixed footer identifying it as filed by
  `ultra-review` with the run's checkpoint range — so a human or a later `issue-select`
  run can trace exactly which review pass produced it.
- **Label:** every filed issue gets `ultra-review`, in addition to any priority label
  convention already in use.

### 6. Deduplicate before publishing

Before filing each candidate issue: `gh issue list --repo <owner>/<repo> --label
ultra-review --state all --search "<file path>"` (narrowed further by a keyword from
the finding's own description). A match with the same file and a materially
overlapping description is treated as already-tracked — skip filing, and instead leave
a `gh issue comment` on the existing issue only if the new finding adds concrete new
evidence (a different line, a recurrence after the file changed again); otherwise skip
silently. This mirrors the duplicate-search discipline `pycc-feedback` and D-022
already require for public writes in this repository.

### 7. Publish (autonomous, bounded)

Per this brainstorm's explicit decision: no per-payload human confirmation gate,
mirroring D-022's precedent for the one other standing autonomous-write authority this
project has already accepted. The bound that keeps this safe, mirroring D-022's own
evidence bar:

- every filed issue must cite a concrete `file:line` — no vague finding is ever filed;
- the dedup pass (step 6) always runs first;
- secrets are never included verbatim (`blocker`-tier point 11 of the pinned
  reviewer's own checklist already flags and redacts credential-shaped content before
  it reaches the finding text);
- if triage would produce an unusually large batch in one run (a first-run bootstrap
  gone wrong, a reviewer mis-triggering) — more than roughly 15 candidate issues after
  dedup — stop short of filing any of them and report the batch for a human to look at
  once, rather than auto-filing a flood. This is judgment-shaped, not a hard product
  requirement to build size-limiting machinery; ordinary incremental runs on this
  repository's typical diff size are not expected to approach it.

### 8. Advance the checkpoint

Edit the tracking issue's body (`gh issue edit`) to the new default-branch tip SHA and
timestamp, plus a one-line run summary (N findings, M filed, K deduped-as-existing).
No repository commit, no PR.

---

## Data flow

```
default-branch tip ──┐
                      ├─► diff(checkpoint, tip) ─► deep-reviewer ─► findings
checkpoint issue ─────┘                                              │
                                                          severity → P1/P2/P3
                                                          ROADMAP.md → milestone
                                                                       │
                                                          dedup vs `ultra-review`-labeled issues
                                                                       │
                                                          gh issue create (bounded, autonomous)
                                                                       │
                                                          checkpoint issue updated to new tip
```

## Error handling

- **`gh` unauthenticated / repo unresolved:** stop before any read, report clearly —
  same failure shape every other autopilot skill in this repo already has.
- **Pinned reviewer cannot be bound** (D-068's own systemic condition): stop, report —
  do not substitute a different reviewer.
- **More than one open issue carries the `ultra-review-checkpoint` label** (a race
  between two concurrent runs — this project already has a documented concurrent
  actor that pushes to `main` mid-session, so concurrent ultra-review runs are a real
  possibility, not a hypothetical): stop and report rather than guessing which is
  authoritative; do not create a third or silently pick one.
- **Empty diff:** clean no-op (step 3).
- **`gh issue edit` on the checkpoint fails after findings were already filed:** the
  filed issues are real and already public; retry the checkpoint update once, and if
  it still fails, report the exact new tip SHA in the run's final report so a human (or
  the next run's own diff-against-stale-checkpoint) is not silently lost — the next
  run would otherwise redundantly re-review already-triaged commits, which the dedup
  pass (step 6) is specifically there to absorb.
- **Batch-size guard trips (step 7):** report and stop; no partial autonomous filing of
  an oversized batch.

## Testing

Same alpha-skill governance shape as `issue-select`/`next-milestone`
(`docs/AGENT_TOOLING.md`'s existing convention): a small `evals.json` with
deterministic, offline, non-model-judgment oracles — e.g. "severity `blocker` always
maps to `P1:`", "an empty diff produces zero dispatches and an unchanged checkpoint",
"a finding whose file:line already appears in an existing `ultra-review`-labeled issue
— open or closed, matching the dedup search's own `--state all` scope — is not
re-filed" — registered in `scripts/run_alpha_skill_evals.py`'s `EXPECTED_RUNNERS`
and `scripts/validate_agent_assets.py`'s `validate_alpha_skill_contracts` tuple,
mirroring exactly how D-144 registered `next-milestone`. Model-judgment-quality
questions (is a given finding *actually* worth filing) are not eval-testable the same
way, same acknowledged gap this repo's other alpha skills already carry.

## Alternatives considered

- **Full-repository re-sweep every run** (this brainstorm's first decision point) —
  rejected: unbounded cost, re-discovers the same finding on unrelated code every
  pass; dedup alone would be doing the full job incremental scoping does for free.
- **Skill self-schedules its own recurrence** (GitHub Actions cron headless agent, or
  a built-in scheduler) — rejected for this design: a scheduled headless-agent
  workflow needs a new CI secret and its own D-021/CI-privilege-boundary review, and
  every other skill in this repository is invoked, not self-scheduling; recurrence is
  layered on externally via the existing `schedule`/Routines mechanism instead.
- **Second "architecture alignment" review pass (Approach C)** — empirically tested
  and rejected above; not worth its per-run cost on the one real comparison run.
- **Per-payload human approval before filing** (`pycc-feedback`'s own gate) —
  rejected per this brainstorm's explicit decision: it would silently defeat
  unattended periodic operation, since a run with nobody watching would never publish
  anything until a human happened to check back in.
- **Local tracked checkpoint file** — rejected: fragile under this project's own
  ephemeral-worktree lifecycle (observed directly this session), and would need a PR
  every run purely to bump a counter.

## Out of scope for this design

- Authenticated model-response evals (same deferred item every alpha skill in this
  repo already carries per `docs/AGENT_TOOLING.md`).
- Actually wiring the recurring trigger (a `schedule`/Routines invocation, or its
  Codex equivalent) — a follow-up setup step once the skill exists, not part of the
  skill's own definition.
- Re-litigating the milestone-at-filing convention itself (D-144's own scope).
- A UI/dashboard for reviewing what `ultra-review` has filed over time — `gh issue
  list --label ultra-review` already serves that role.
