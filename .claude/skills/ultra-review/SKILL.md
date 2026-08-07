---
name: ultra-review
description: Use this alpha project skill when the user wants a periodic, evidence-gated codebase review that files prioritized (`P1`/`P2`/`P3`), milestone-scoped GitHub issues for what it finds — "run an ultra review", "do a periodic code review and file issues", a standing recurring-review directive, or a scheduled/automated invocation with no issue named. Reads a GitHub-native checkpoint to review only the diff since the last run, dispatches the pinned D-068 deep-reviewer once, maps its `blocker`/`warning`/`note` findings to `P1`/`P2`/`P3`, deduplicates against already-filed `ultra-review`-labeled issues, and files the rest autonomously within a bounded evidence bar — without a human approving each payload. Does not implement anything itself and does not pick an issue to work (`issue-select`'s job) or plan one (`issue-to-plan`'s job).
---

# ultra-review (Alpha)

Periodically re-review the codebase for the class of problem that survives any single
pull request's own D-068 pre-merge gate — drift that only shows up once several merges
have accumulated, or a doc/config file nobody happened to touch in the diff that
introduced the drift — and turn confirmed findings into prioritized, milestone-scoped
GitHub issues. This skill mutates no tracked file and writes no implementation code; its
only public writes are the GitHub issues (and the checkpoint issue) described below.

This project-local skill is alpha, matching `issue-select`, `issue-to-plan`,
`issue-implement`, and `next-milestone`'s own alpha status.

Design background, including the empirical comparison that ruled out a broader
two-pass review architecture in favor of the single pinned pass this skill actually
uses, is recorded in
[`docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`](../../../docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md).

## Scope

Use it when asked to run a periodic/ultra code review, when a standing recurring-review
directive is in effect, or when an external scheduler (Claude Code Routines, a Codex
cron equivalent) invokes it. Do not use it to plan or implement anything found —
`issue-to-plan`/`issue-implement` pick those issues up separately, the same way they
pick up any other filed issue. This skill does not schedule its own recurrence; wiring
an actual recurring trigger is a separate setup step using the existing `schedule`
mechanism, outside this skill's own definition.

## Workflow

### 1. Baseline

Fetch and record the default-branch tip, per
[D-021](../../../docs/decisions/D-021-agent-task-preflight-and-documentation-refresh.md).
Confirm `gh` resolves the target repository. No Rust build or `cargo doc` step is needed
— this skill makes no code changes.

### 2. Read the checkpoint

Ensure the `ultra-review-checkpoint` and `ultra-review` labels exist
(`gh label list --repo <owner>/<repo> --limit 200 --json name`; create either missing
one with `gh label create`), the same way `next-milestone`'s own step 3 ensures its
GitHub milestone exists before using it. Two `gh` behaviors this step depends on,
both confirmed against the live repository on this skill's first run:

- pass `--limit` explicitly — bare `gh label list` returns only the first 30 labels,
  so a repository whose label set has grown past that could report an existing label
  as missing;
- `gh label create` **exits 1** on a label that already exists
  (`label with name "..." already exists`). That is this step's success condition, not
  a failure: the goal is "the label exists", so treat an already-exists error as
  satisfied and continue. Only an error of any other kind stops the run.

This label check must run before the checkpoint search below, not be folded into it:
`gh issue list --label <name>` returns an empty list and exit 0 for a label that does
not exist at all, so it cannot distinguish "no checkpoint yet" from "the label is
missing".

Search for the checkpoint tracking issue:
`gh issue list --repo <owner>/<repo> --label ultra-review-checkpoint --state all --json number,body`.

- **Exactly one found:** parse its body's `Last reviewed commit: <sha>` line (the
  fixed, machine-parseable block described in step 8).
- **None found (first run ever):** create it —
  `gh issue create --repo <owner>/<repo> --title "Ultra-review checkpoint — do not close" --label ultra-review-checkpoint --body "<bootstrap block, below>"`.
  Step 8's block has two fields with no meaning on a bootstrap, where no review has
  run, so the bootstrap body is fixed here rather than left to invention: keep
  step 8's exact delimiters and `Last reviewed commit:` line (set to the current
  default-branch tip), set `Reviewed at:` to the bootstrap timestamp, and write
  `Last run:` literally as
  `bootstrap -- no review performed; checkpoint initialized at the current default-branch tip`.
  Report this bootstrap explicitly: the very first run reviews an empty range and is a
  deliberate one-time no-op, not a full historical sweep — reviewing the repository's
  entire history in one pass was explicitly rejected in the design doc's alternatives.
- **More than one found:** stop and report — a race between two concurrent runs (this
  project has a documented concurrent actor that can push to `main` mid-session, so
  concurrent ultra-review runs are a real possibility). Do not create a third
  checkpoint issue or silently pick one as authoritative.

This is a **GitHub-native checkpoint**, not a tracked file: a local checkpoint file
would not survive this project's own ephemeral-worktree lifecycle, and committing a
checkpoint bump would need its own pull request every run purely to move a counter.

### 3. Compute the incremental diff

`git diff <checkpoint-sha>..<default-branch-tip>`. If empty, report "nothing new since
`<checkpoint-sha>`" and stop — no dispatch, no checkpoint update; advancing the
checkpoint on an empty diff would be a no-op that only obscures the timestamp of the
last real review.

Save the diff to a scratch file (outside the working tree) rather than inlining it
directly into the dispatch prompt below — a large diff embedded as literal prompt text
is unreliable to construct correctly; handing the pinned reviewer a file path and
letting it `Read` the file, exactly as this skill's own design bake-off did, is the
proven approach.

### 4. Review dispatch

Dispatch the pinned deep reviewer from a structurally verified `ievo@ievo-skills`
install (`scripts/check_claude_reviewer_binding.py`, per D-155) — the exact one
`issue-implement`'s own review loop uses; if no such install can be bound, the review
gate is unavailable — report that and stop rather than substituting a weaker reviewer.
Give it the input shape the pinned `deep-review/SKILL.md` entrypoint itself defines
(`diff`, `changed_files`, `repo_context`), which `docs/AGENT_TOOLING.md`'s "Local review
workflow" section is the policy record for:

```
Review the following diff for gaps, drift, and consistency issues.

## Repo context
<one-line repo description>

## Changed files
<git diff <checkpoint-sha>..<default-branch-tip> --name-only>

## Diff
The full diff is saved at <scratch file path> (git diff <checkpoint-sha>..<default-branch-tip>). Read that file with your Read tool to get the complete diff text before starting the 11-point checklist.
```

Add two short, explicit reminders on top of the standard 11 points — informational
context supplied by this skill's own dispatch prompt, not a modification of the pinned
agent artifact itself — learned directly from this skill's own design bake-off, where
both compared review variants missed the same real bug:

- when a changed line updates a status/summary statement, check whether adjacent
  *unchanged* prose still describes the prior state;
- check whether files under `docs/sessions/` were updated to checkpoint the merges in
  this diff range, per that convention's own non-blocking note in `AGENTS.md`.

### 5. Triage findings into candidate issues

For each finding the reviewer reports, in severity order (`blocker` first):

- **Severity → priority:** `blocker` → `P1:`, `warning` → `P2:`, `note` → `P3:`. Every
  filed issue must cite a concrete `file:line` — a finding without one is never filed,
  regardless of severity.
- **Milestone:** read `docs/ROADMAP.md`'s ordered `## vX.Y` sections the same way
  `next-milestone` does, to find the currently active milestone, then apply
  `AGENTS.md`'s D-021 step 9 milestone-at-filing convention unchanged: assign that
  milestone unless the finding is clearly cross-cutting (CI governance, website/SEO,
  agent-tooling infrastructure) or genuinely not `vX.Y`-scoped work. Leaving a
  candidate unassigned is the exception, not the default.
- **Title:** `<P-prefix>: <one-line summary>`.
- **Body:** the finding's file:line, the concrete issue description, the reviewer's own
  suggestion, the diff range reviewed (`<checkpoint-sha>..<default-branch-tip>`), a
  one-line `no milestone — cross-cutting` reasoning note whenever the milestone was
  left unassigned (D-021 step 9 requires that omission to be deliberate and visible
  rather than accidental), and a fixed footer:
  `Filed by the ultra-review skill from range <checkpoint-sha>..<tip>.` — so a human
  or a later `issue-select` run can trace which review pass produced it.
- **Label:** every candidate issue gets `ultra-review`.

### 6. Deduplicate before publishing

For each candidate, before filing:
`gh issue list --repo <owner>/<repo> --label ultra-review --state all --search "<file path>"`,
narrowed further by a keyword from the finding's own description. A match against the
same file with a materially overlapping description is already tracked — skip filing
it; leave a `gh issue comment` on the existing issue only if the new finding adds
concrete new evidence (a different line, a recurrence after the file changed again),
otherwise skip silently. This mirrors the duplicate-search discipline `pycc-feedback`
and [D-022](../../../docs/decisions/D-022-autonomous-public-ievo-bug-reporting.md)
already require for public writes in this repository.

### 7. Publish (autonomous, bounded)

File every candidate that survives step 6 without per-payload human confirmation,
mirroring D-022's precedent for the one other standing autonomous-write authority this
project has already accepted (`gh issue create`, `P1:`/`P2:`/`P3:` title prefix,
`ultra-review` label, milestone from step 5 when assigned). The bound that keeps this
safe, mirroring D-022's own evidence bar:

- every filed issue cites a concrete file:line (step 5);
- the dedup pass (step 6) always runs first;
- secrets are never included verbatim — the pinned reviewer's own point 11 already
  flags and redacts credential-shaped content before it reaches the finding text;
- if the dedup-survived candidate count in one run exceeds roughly 15 (a first-run
  bootstrap gone wrong, a reviewer mis-triggering), stop short of filing any of them
  and report the batch for a human to look at once, rather than auto-filing a flood.

### 8. Advance the checkpoint

Edit the checkpoint tracking issue's body
(`gh issue edit <number> --repo <owner>/<repo> --body "<new block>"`) to:

```
<!-- ultra-review-checkpoint -->
Last reviewed commit: <new default-branch tip sha>
Reviewed at: <ISO 8601 timestamp>
Last run: <N findings, M filed, K deduped-as-existing>
<!-- /ultra-review-checkpoint -->
```

No repository commit, no pull request. If this edit fails after findings were already
filed, retry once; if it still fails, report the exact new tip sha in this run's final
report — the filed issues are already real and public, and the next run's own diff
against the stale checkpoint would only redundantly re-review already-triaged commits,
which step 6's dedup pass is specifically there to absorb.

## Output

The checkpoint range reviewed, the reviewer's raw findings, which were filed (with
issue numbers and URLs) versus deduped as already-tracked versus dropped for missing
evidence, the new checkpoint state, and — if the batch-size guard tripped — the full
undispatched batch for a human to review.
