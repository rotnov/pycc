# 2026-08-22-08 — #727 milestone-scope ordering: PR #728 opened, awaiting checks

## Where the tree is

- Default branch: `origin/main` at `21bea2354d2f9bf661116bbef0959c17ec675dcc`
  (re-fetched immediately before writing this entry).
- Task branch `issue-727`, pushed, several commits on top of that base. This
  entry deliberately does not name the branch head: the commit carrying this
  entry, and any reviewer-follow-up commit after it, advance it. Read
  `headRefOid` from the pull request rather than from here.
- Open pull requests at this checkpoint: **#728 only** (OPEN, not draft,
  `MERGEABLE` when last resolved). No other pull request is open.

## What was delivered

Issue #727 — `issue-select`'s step 5 ordering starved the active milestone
under a milestone directive — taken end to end into **PR #728**.

- `.claude/skills/issue-select/SKILL.md` step 5: under a milestone scope,
  membership in that scope ranks first, ahead of the priority marker and ahead
  of size; marker-then-size inside each group; out-of-scope issues reached only
  when the scope contributes no survivor at all; leaving the scope is a
  reportable event, and step 8's hand-off report carries that record. With no
  scope in effect the ordering is unchanged. The pool is never restricted — the
  mechanism is ordering, not filtering, so D-144's rejection of a hard pool
  restriction stands.
- **D-191** (`docs/decisions/D-191-milestone-membership-ranks-first-in-issue-select.md`),
  superseding **only D-144 decision (a)**. D-144 untouched. Index regenerated,
  `--check` clean.
- Machine bindings: `issue_select_higher_ranked` gains an inert
  `milestone_scope_in_effect` parameter; `ISSUE_SELECT_CONTRACT` keeps the
  existing priority-marker literal (relocated into the no-scope branch where it
  stays true) and pins two literals from the new rule; a fifth eval case is
  registered in `EXPECTED_RUNNERS`, `validate_agent_assets.py`, and
  `evals.json`; case 4's `expected_output` and `required` tuple were corrected
  because they asserted the superseded rule.
- `/harden` incident, fixture, and arena campaign under
  `.harden/incidents/milestone-scope-starvation/`; `/harden batch` counter at
  `.harden/incidents/own-change-falsifies-adjacent-prose/2026-08-22-issue-727.md`.
- `docs/AGENT_RETROSPECTIVE.md` entry on why a selection rule cannot be
  validated one selection at a time.

## Evidence, with its limits

The arena campaign (`.harden/arena/20260822-080436-fixture/`, 24 runs) reports
"the patch works", but that headline is computed from 2 of 4 harnesses and only
one of those two carries usable data. **codex 1/3 → 3/3** is the real signal.
**claude 0/3 → 1/3 is not evidence**: five of its six runs made zero tool calls
and the judge's notes record expired sessions — infrastructure failures the
arena did not classify as such. **devin and grok had no baseline**, because
`task.md`'s standing directive is itself an imperative and those harnesses
follow the prompt over the ambient `AGENTS.md`. Softening the directive was
rejected: it is the patch rule's own trigger. Recorded this way in the incident
and in the PR body rather than reported as a four-harness confirmation.

## Gate state

All local gates green on the pushed head, each captured as
`cmd > log 2>&1; echo rc=$?`: `cargo test`, `cargo llvm-cov` (100.00% lines and
regions), `cargo clippy -D warnings`, `cargo fmt --check`, `cargo doc`, the
`scripts/` unittest suite, both `run_alpha_skill_evals.py` client entrypoints,
`validate_agent_assets.py`, `validate_agent_policies.py`, both Ruby checkers and
their tests, and the decisions-index `--check`. Nothing was weakened to pass.

The pinned iEvo `deep-reviewer` ran over the first commit and returned three
findings — two `docs/AGENT_TOOLING.md` doc-drift warnings and one stale test
name — all fixed in the second commit. Because that dispatch had only `Read`
and `Grep` (the agent is defined without Bash, so it cannot read a diff itself)
and could not enumerate the changed-file set, and because later commits added
the `.harden/` artefacts and this entry, a second review pass runs over the
full `21bea235..HEAD` range with the diff materialized to a file first. Merge
only after that pass is clean.

## Where a fresh session resumes

PR #728's required checks (`audit`, `ci-gate`) had not yet reported when this
snapshot was written. Resume by re-checking #728's state, draft status, head,
mergeability, and check runs before anything else. If green and no unresolved thread remains, merge with
`--match-head-commit`, read back `state` and `mergeCommit`, and delete the
branch only on `MERGED`. `closingIssuesReferences.totalCount` for #728 is **1**
(#727), verified after the PR was opened, so the merge closes exactly that
issue and nothing else.

Nothing else is in flight. No autopilot loop is paused and no denylist carries
forward from this session.
