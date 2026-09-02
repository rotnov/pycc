# Review-round brief template (issue-implement step 5)

Fill every slot; leave the fixed paragraphs verbatim. The exclusions below are the
artefact of `reviewer-flags-a-later-phase-deliverable` (three occurrences of a brief
typed freehand that omitted one of them), so they travel with the template, not with
the orchestrator's memory.

```
Review round <ROUND> for issue #<N> on branch `<BRANCH>` (worktree `<WORKTREE>`).

Range: the full committed range from the merge base with `origin/<DEFAULT>` through
`HEAD`; the diff is at `<DIFF_PATH>`.

Plan: <PLAN_PATH_OR_COMMENT_URL>. Acceptance criteria, quoted:
<ACCEPTANCE_CRITERIA>

History: <ROUND_HISTORY — one line per earlier round: findings, fix commit>.

Out of scope for this round, by design of the workflow that dispatches you:
- the `docs/sessions/` handoff file and the pull-request body: step 6 writes them after
  this loop ends;
- the `.harden/findings/issue-<N>.jsonl` pile: it is appended as each round's verdicts
  land and cannot exist before the first round;
- gate results (coverage, clippy, fmt, validators), GitHub state (issue comments,
  pull-request state), and any claim that needs `git` to check — a pure-move commit's
  behaviour-neutrality, a rename's line-set identity, a commit's ancestry. The
  orchestrating session verifies these; your inability to verify them with `Read` and
  `Grep` is not a finding. Already verified by the session this round:
  <VERIFIED_CLAIMS>.

Report every finding with file, line, category, severity, and the evidence a reader
needs to reproduce it. A round with no actionable findings says so explicitly.
```
