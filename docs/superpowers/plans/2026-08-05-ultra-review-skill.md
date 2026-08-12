# ultra-review skill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `ultra-review` alpha skill — a periodic, evidence-gated codebase
review that reuses the pinned D-068 deep-reviewer against the diff since a
GitHub-native checkpoint, maps its `blocker`/`warning`/`note` findings to
`P1`/`P2`/`P3` GitHub issues with milestone-at-filing and dedup, and files them
autonomously within a bounded evidence bar.

**Architecture:** One new Claude Code skill + Codex thin-pointer mirror + deterministic
`evals.json`, wired into both alpha-skill governance scripts
(`run_alpha_skill_evals.py`, `validate_agent_assets.py`), documented in
`docs/AGENT_TOOLING.md`/`docs/ROADMAP.md`, and recorded as a new `docs/DECISIONS.md`
entry — following exactly the pattern D-144 used to register `next-milestone` in the
same apparatus.

**Tech Stack:** No new dependencies. `gh` CLI, Python 3 (`scripts/*.py`), the pinned
`ievo:deep-reviewer` agent.

## Global Constraints

- Design source of truth: [`docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`](../specs/2026-08-05-ultra-review-skill-design.md) — every task below implements a section of it; do not deviate without updating that doc first.
- D-068: only the exact pinned reviewer artifact may be used as a review engine.
- `AGENTS.md` "Support Codex and Claude Code": every skill needs a Claude Code `SKILL.md` under `.claude/skills/` and a thin `.agents/skills/` pointer mirror.
- Alpha-skill governance (`docs/AGENT_TOOLING.md`): register in `scripts/run_alpha_skill_evals.py`'s `EXPECTED_RUNNERS` and `scripts/validate_agent_assets.py`'s `ALPHA_EVAL_RUNNERS` + `validate_alpha_skill_contracts` tuple, with `evals.json` binding at least two (here: four) executable runner cases.
- D-021: start from a freshly fetched `origin/main` in a clean task branch (already done — branch `feat/ultra-review-skill-design`, commit `af50520` on top of `origin/main`); never merge/rebase over uncommitted work.
- D-014: 100% line/region coverage is the Rust workspace's own gate (`llvm-cov --workspace`) and does not extend to `scripts/*.py`; the Python test suite (`python3 -m unittest` under `scripts/`) is still required to pass and every new function gets a direct unit test regardless.
- `docs/DECISIONS.md` D-146 is the current tip; this plan's new entry is drafted as **D-147 (indicative)** — re-resolve the actual next-free number immediately before opening the PR, since a concurrent PR could claim it first (this repository has a documented concurrent actor).
- Every behavior change ships with its documentation update in the same commit set (`AGENTS.md`'s "Keep documentation current").
- YAGNI: no new pinned-reviewer artifact, no scheduling infrastructure, no batching machinery beyond the single guard threshold this plan defines.

---

### Task 1: The skill itself — `SKILL.md`, Codex mirror, `evals.json`

**Files:**
- Create: `.claude/skills/ultra-review/SKILL.md`
- Create: `.agents/skills/ultra-review/SKILL.md`
- Create: `.claude/skills/ultra-review/evals/evals.json`

**Interfaces:**
- Produces: three literal contract phrases that Task 2's `ULTRA_REVIEW_CONTRACT` tuple checks against this file's raw text (normalized whitespace, i.e. `" ".join(text.split())`) — they must appear **verbatim**, or Task 2's oracle fails closed:
  - `` a concrete `file:line` ``
  - `GitHub-native checkpoint`
  - `stop short of filing any of them`
- Produces: four `evals.json` cases whose `runner` values exactly match Task 2's `EXPECTED_RUNNERS["ultra-review"]` set (below) and whose `expected_output` strings contain the exact substrings Task 2's `run_ultra_review_case` checks for each runner.

- [ ] **Step 1: Write `.claude/skills/ultra-review/SKILL.md`**

```markdown
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
[D-021](../../../docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh).
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

Dispatch the pinned deep reviewer from the digest-recorded artifact (per D-068) — the
exact one `issue-implement`'s own review loop uses; if it cannot be bound, this is a
stop condition — report and do not substitute a weaker reviewer. Give it the input
shape the pinned `deep-review/SKILL.md` entrypoint itself defines (`diff`,
`changed_files`, `repo_context`), which `docs/AGENT_TOOLING.md`'s "Local review
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
and [D-022](../../../docs/DECISIONS.md#d-022-autonomous-public-ievo-bug-reporting)
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
```

- [ ] **Step 2: Write `.agents/skills/ultra-review/SKILL.md`**

```markdown
---
name: ultra-review
description: Use this alpha project skill when the user wants a periodic, evidence-gated codebase review that files prioritized (`P1`/`P2`/`P3`), milestone-scoped GitHub issues for what it finds — "run an ultra review", "do a periodic code review and file issues", a standing recurring-review directive, or a scheduled/automated invocation with no issue named. Reads a GitHub-native checkpoint to review only the diff since the last run, dispatches the pinned D-068 deep-reviewer once, maps its `blocker`/`warning`/`note` findings to `P1`/`P2`/`P3`, deduplicates against already-filed `ultra-review`-labeled issues, and files the rest autonomously within a bounded evidence bar — without a human approving each payload. Does not implement anything itself and does not pick an issue to work (`issue-select`'s job) or plan one (`issue-to-plan`'s job).
---

# ultra-review (Alpha)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/ultra-review/SKILL.md` from that repository completely and
follow it as the canonical workflow. If the file is missing, stop and report
the missing project instruction instead of substituting a cached copy.
```

Note: the `description` frontmatter above must be **byte-identical** to Step 1's — Task 2's `canonical_skill("codex", ...)` resolution and this project's existing parity convention both depend on it.

- [ ] **Step 3: Write `.claude/skills/ultra-review/evals/evals.json`**

```json
{
  "skill_name": "ultra-review",
  "evals": [
    {
      "id": 1,
      "prompt": "The pinned deep-reviewer's checklist reports a `blocker`-severity finding during a run.",
      "expected_output": "A blocker-severity finding always maps to P1: severity determines the filed issue's priority prefix directly.",
      "runner": "blocker-severity-maps-to-p1"
    },
    {
      "id": 2,
      "prompt": "The diff between the checkpoint commit and the current default-branch tip is empty -- nothing merged since the last run.",
      "expected_output": "Report that there is nothing new since the checkpoint and stop: no dispatch, no checkpoint update -- advancing the checkpoint on an empty diff would be a no-op that only obscures the last real review.",
      "runner": "empty-diff-checkpoint-not-advanced"
    },
    {
      "id": 3,
      "prompt": "A candidate finding has concrete file:line evidence, but the dedup pass (which searches `--state all`, so a closed issue counts as tracked too) finds an existing `ultra-review`-labeled issue already tracking the same file and description.",
      "expected_output": "Skip filing: a finding the dedup pass confirms is already tracked is never re-filed, even though it independently satisfies the file:line evidence bar on its own.",
      "runner": "deduped-finding-never-refiled"
    },
    {
      "id": 4,
      "prompt": "27 candidate issues survive the dedup pass in a single run.",
      "expected_output": "When more than roughly 15 candidate issues survive dedup in one run, stop short of filing any of them and report the batch for a human to look at, rather than auto-filing a flood.",
      "runner": "oversized-batch-stops-before-filing"
    }
  ]
}
```

- [ ] **Step 4: Verify the files parse and the Codex mirror resolves**

Run:

```bash
python3 -c "
import json, re
from pathlib import Path
claude = Path('.claude/skills/ultra-review/SKILL.md').read_text()
codex = Path('.agents/skills/ultra-review/SKILL.md').read_text()
assert claude.splitlines()[1] == codex.splitlines()[1], 'description frontmatter must match'
assert '.claude/skills/ultra-review/SKILL.md' in codex, 'codex mirror must point at the canonical file'
evals = json.loads(Path('.claude/skills/ultra-review/evals/evals.json').read_text())
assert evals['skill_name'] == 'ultra-review'
assert len(evals['evals']) == 4
print('OK')
"
```

Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add .claude/skills/ultra-review .agents/skills/ultra-review
git commit -m "Add ultra-review skill definition, Codex mirror, and evals"
```

---

### Task 2: `scripts/run_alpha_skill_evals.py` — oracle functions, contract tuple, case dispatch, wiring

**Files:**
- Modify: `scripts/run_alpha_skill_evals.py`
- Modify: `scripts/test_run_alpha_skill_evals.py`

**Interfaces:**
- Consumes: `.claude/skills/ultra-review/SKILL.md` (Task 1), `.claude/skills/ultra-review/evals/evals.json` (Task 1).
- Produces: `ultra_review_severity_priority(severity: str) -> str`, `ultra_review_checkpoint_should_advance(*, diff_is_empty: bool) -> bool`, `ultra_review_may_file(*, has_file_line_evidence: bool, already_tracked: bool) -> bool`, `ultra_review_batch_within_guard(*, candidate_count: int) -> bool`, `run_ultra_review_case(case: dict, skill_text: str) -> None` — consumed by Task 3's structural check via the same names' presence in `ALPHA_EVAL_RUNNERS`.

- [ ] **Step 1: Write the failing test for the four new oracle functions**

Append to `scripts/test_run_alpha_skill_evals.py`, directly after `test_next_milestone_loop_continues` (after its closing line, currently line 505):

```python
    def test_ultra_review_severity_priority(self) -> None:
        self.assertEqual(evals.ultra_review_severity_priority("blocker"), "P1")
        self.assertEqual(evals.ultra_review_severity_priority("warning"), "P2")
        self.assertEqual(evals.ultra_review_severity_priority("note"), "P3")
        with self.assertRaises(evals.EvalError):
            evals.ultra_review_severity_priority("critical")

    def test_ultra_review_checkpoint_should_advance(self) -> None:
        self.assertFalse(
            evals.ultra_review_checkpoint_should_advance(diff_is_empty=True)
        )
        self.assertTrue(
            evals.ultra_review_checkpoint_should_advance(diff_is_empty=False)
        )

    def test_ultra_review_may_file(self) -> None:
        self.assertTrue(
            evals.ultra_review_may_file(
                has_file_line_evidence=True, already_tracked=False
            )
        )
        self.assertFalse(
            evals.ultra_review_may_file(
                has_file_line_evidence=True, already_tracked=True
            )
        )
        self.assertFalse(
            evals.ultra_review_may_file(
                has_file_line_evidence=False, already_tracked=False
            )
        )

    def test_ultra_review_batch_within_guard(self) -> None:
        self.assertTrue(evals.ultra_review_batch_within_guard(candidate_count=15))
        self.assertFalse(evals.ultra_review_batch_within_guard(candidate_count=16))
```

- [ ] **Step 2: Run the tests to verify they fail**

Run `PYTHONPATH=scripts python3 -m unittest scripts.test_run_alpha_skill_evals -k ultra_review -v` from the repository root (the test class is `AlphaSkillEvalTests`; `PYTHONPATH=scripts` is what lets the module's own top-level `import run_alpha_skill_evals` resolve outside `unittest discover -s scripts`). Capture the command's own exit status — never read a verdict off a piped `head`/`tail` summary.
Expected: FAIL with `AttributeError: module 'run_alpha_skill_evals' has no attribute 'ultra_review_severity_priority'` (and similarly for the other three).

- [ ] **Step 3: Add the `EXPECTED_RUNNERS` entry**

In `scripts/run_alpha_skill_evals.py`, in the `EXPECTED_RUNNERS` dict (starts line 29), insert immediately after the `"next-milestone": {...}` entry (currently lines 64-67):

```python
    "ultra-review": {
        "blocker-severity-maps-to-p1",
        "empty-diff-checkpoint-not-advanced",
        "deduped-finding-never-refiled",
        "oversized-batch-stops-before-filing",
    },
```

- [ ] **Step 4: Add the `ULTRA_REVIEW_CONTRACT` tuple**

Immediately after the `ISSUE_SELECT_CONTRACT` tuple's closing `)` (currently line 121), insert:

```python
ULTRA_REVIEW_CONTRACT = (
    "a concrete `file:line`",
    "GitHub-native checkpoint",
    "stop short of filing any of them",
)
```

- [ ] **Step 5: Add the four oracle functions**

Immediately after `next_milestone_loop_continues`'s closing line (currently line 284), insert:

```python
_ULTRA_REVIEW_SEVERITY_PRIORITY = {"blocker": "P1", "warning": "P2", "note": "P3"}


def ultra_review_severity_priority(severity: str) -> str:
    """ultra-review step 5's fixed severity-to-priority mapping -- the same
    blocker/warning/note scale the pinned deep-reviewer already returns."""
    try:
        return _ULTRA_REVIEW_SEVERITY_PRIORITY[severity]
    except KeyError as error:
        raise EvalError(f"unknown ultra-review severity {severity!r}") from error


def ultra_review_checkpoint_should_advance(*, diff_is_empty: bool) -> bool:
    """ultra-review step 3/8: an empty diff since the last checkpoint is a
    clean no-op -- no dispatch, and the checkpoint issue is left untouched."""
    return not diff_is_empty


def ultra_review_may_file(
    *, has_file_line_evidence: bool, already_tracked: bool
) -> bool:
    """ultra-review step 7's publish gate: a finding is only ever filed when
    it carries concrete file:line evidence AND step 6's dedup pass found no
    existing `ultra-review`-labeled issue already tracking it."""
    return has_file_line_evidence and not already_tracked


ULTRA_REVIEW_BATCH_GUARD_THRESHOLD = 15


def ultra_review_batch_within_guard(*, candidate_count: int) -> bool:
    """ultra-review step 7's batch-size guard: a run whose dedup-survived
    candidate count exceeds this threshold stops short of filing any of them
    and reports the batch instead of auto-filing a flood."""
    return candidate_count <= ULTRA_REVIEW_BATCH_GUARD_THRESHOLD
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `python3 -m unittest scripts.test_run_alpha_skill_evals -k ultra_review -v` from the repository root.
Expected: 4 tests, all `ok`.

- [ ] **Step 7: Add `run_ultra_review_case` and wire it into `run_evals`**

Immediately after `run_next_milestone_case`'s closing line (currently line 839, right before `def run_evals`), insert:

```python
def run_ultra_review_case(case: dict[str, Any], skill_text: str) -> None:
    normalized = " ".join(skill_text.split())
    for contract in ULTRA_REVIEW_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"ultra-review skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    if runner_name == "blocker-severity-maps-to-p1":
        priority = ultra_review_severity_priority("blocker")
        required = ("blocker", "P1")
        if priority != "P1":
            raise EvalError(f"{runner_name} did not map blocker severity to P1")
    elif runner_name == "empty-diff-checkpoint-not-advanced":
        advances_empty = ultra_review_checkpoint_should_advance(diff_is_empty=True)
        advances_nonempty = ultra_review_checkpoint_should_advance(diff_is_empty=False)
        required = ("nothing new", "no dispatch, no checkpoint update")
        if advances_empty or not advances_nonempty:
            raise EvalError(
                f"{runner_name} did not gate the checkpoint update on a "
                f"non-empty diff"
            )
    elif runner_name == "deduped-finding-never-refiled":
        may_file = ultra_review_may_file(
            has_file_line_evidence=True, already_tracked=True
        )
        required = ("already tracked", "never re-filed")
        if may_file:
            raise EvalError(
                f"{runner_name} filed a finding the dedup pass already found tracked"
            )
    elif runner_name == "oversized-batch-stops-before-filing":
        within_guard = ultra_review_batch_within_guard(candidate_count=16)
        required = ("stop short of filing any of them", "report the batch")
        if within_guard:
            raise EvalError(f"{runner_name} let an oversized batch pass the guard")
    else:
        raise EvalError(f"unknown ultra-review runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")


```

Then, in `run_evals` (currently lines 842-884), immediately after the existing `next_milestone_skill`/`load_cases("next-milestone", ...)` block (currently lines 882-884), append:

```python
    ultra_review_skill = canonical_skill(client, "ultra-review", root)
    for case in load_cases("ultra-review", root):
        run_ultra_review_case(case, ultra_review_skill)
```

- [ ] **Step 8: Run the full offline eval suite for both clients**

Run:
```bash
python3 scripts/run_alpha_skill_evals.py --client claude
python3 scripts/run_alpha_skill_evals.py --client codex
```
(check the script's own `--help` for its exact pycc-binary argument if these fail on that missing prerequisite; the `ultra-review` block runs regardless of that argument, since Task 1's `Read`-only checks add no subprocess dependency).
Expected: both invocations exit 0 and report the `ultra-review` cases passing alongside every existing skill's cases.

- [ ] **Step 9: Extend the unknown-runner coverage test**

In `scripts/test_run_alpha_skill_evals.py`, in `test_unknown_runner_fails_closed_for_each_new_skill` (currently ~line 545), add `"ultra-review"` to both dicts:

```python
        skill_by_name = {
            "issue-to-plan": evals.canonical_skill("claude", "issue-to-plan"),
            "issue-implement": evals.canonical_skill("claude", "issue-implement"),
            "issue-select": evals.canonical_skill("claude", "issue-select"),
            "ultra-review": evals.canonical_skill("claude", "ultra-review"),
        }
        dispatch = {
            "issue-to-plan": evals.run_issue_to_plan_case,
            "issue-implement": evals.run_issue_implement_case,
            "issue-select": evals.run_issue_select_case,
            "ultra-review": evals.run_ultra_review_case,
        }
```

- [ ] **Step 10: Run the full Python test suite**

Run: `python3 -m unittest discover -s scripts -p "test_*.py" -v 2>&1 | tail -20`
Expected: all tests pass, including the new and modified ones.

- [ ] **Step 11: Commit**

```bash
git add scripts/run_alpha_skill_evals.py scripts/test_run_alpha_skill_evals.py
git commit -m "Register ultra-review in run_alpha_skill_evals.py's offline contract suite"
```

---

### Task 3: `scripts/validate_agent_assets.py` — structural contract registration

**Files:**
- Modify: `scripts/validate_agent_assets.py`

**Interfaces:**
- Consumes: `ALPHA_EVAL_RUNNERS["ultra-review"]` must equal Task 2's `EXPECTED_RUNNERS["ultra-review"]` set exactly (this repository's own established, hand-kept-in-sync convention — see `docs/AGENT_TOOLING.md`'s existing note that these two constants must match by hand).
- Produces: `ultra-review` participates in `validate_alpha_skill_contracts`'s structural check (evals count, runner-set equality, "alpha" substring) the same way the other six alpha skills already do.

- [ ] **Step 1: Add the `ALPHA_EVAL_RUNNERS` entry**

In `scripts/validate_agent_assets.py`, in the `ALPHA_EVAL_RUNNERS` dict (starts line 53), insert immediately after the `"next-milestone": {...}` entry (currently lines 88-91):

```python
    "ultra-review": {
        "blocker-severity-maps-to-p1",
        "empty-diff-checkpoint-not-advanced",
        "deduped-finding-never-refiled",
        "oversized-batch-stops-before-filing",
    },
```

- [ ] **Step 2: Extend `validate_alpha_skill_contracts`'s skill tuple**

At line 3239, change:

```python
    for name in ("pycc", "pycc-feedback", "issue-to-plan", "issue-implement", "issue-select", "next-milestone"):
```

to:

```python
    for name in (
        "pycc",
        "pycc-feedback",
        "issue-to-plan",
        "issue-implement",
        "issue-select",
        "next-milestone",
        "ultra-review",
    ):
```

- [ ] **Step 3: Run the full validator against the live repository**

Run: `python3 scripts/validate_agent_assets.py`
Expected: exits 0 with no failures mentioning `ultra-review`.

- [ ] **Step 4: Run the validator's own test suite**

Run: `python3 -m unittest scripts.test_validate_agent_assets -v 2>&1 | tail -20`
Expected: all tests pass unchanged (this task adds no new branch to `validate_alpha_skill_contracts`'s own logic, only one more tuple entry it already knows how to iterate — the existing generic test coverage for that function already exercises every code path the new entry runs through).

- [ ] **Step 5: Commit**

```bash
git add scripts/validate_agent_assets.py
git commit -m "Register ultra-review in validate_agent_assets.py's structural alpha-skill check"
```

---

### Task 4: Documentation — `docs/AGENT_TOOLING.md`, `docs/DECISIONS.md`, `docs/SPEC.md`, `docs/ROADMAP.md`

**Files:**
- Modify: `docs/AGENT_TOOLING.md`
- Modify: `docs/DECISIONS.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/ROADMAP.md`

**Interfaces:**
- Consumes: nothing from earlier tasks besides the fact that the skill now exists — this task is purely descriptive.
- Produces: the count strings ("all six" → "all seven" etc.) that a later `ultra-review` run reading these files itself would need to already be correct (dogfooding note for Task 5).

- [ ] **Step 1: `docs/AGENT_TOOLING.md` — skill-list line**

At line 82-83, change:

```markdown
`pycc`, `pycc-feedback`, `issue-to-plan`, `issue-implement`,
`issue-select`, and `next-milestone` follow the
```

to:

```markdown
`pycc`, `pycc-feedback`, `issue-to-plan`, `issue-implement`,
`issue-select`, `next-milestone`, and `ultra-review` follow the
```

- [ ] **Step 2: `docs/AGENT_TOOLING.md` — new descriptive paragraph**

Immediately after the `next-milestone` paragraph's closing line (currently line 212, ending "...GitHub milestone close."), insert a new paragraph:

```markdown

`ultra-review` periodically re-reviews the codebase for drift a single pull
request's own D-068 gate cannot see and files prioritized, milestone-scoped
issues for what it finds. It reads a GitHub-native checkpoint (a dedicated
tracking issue, not a tracked file — this project's own ephemeral-worktree
lifecycle ruled that out directly), computes the diff since that checkpoint,
dispatches the same pinned D-068 deep-reviewer once (a live empirical
comparison against a broader two-pass architecture-review design found the
second pass did not earn its cost — see
`docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`), maps its
`blocker`/`warning`/`note` findings to `P1`/`P2`/`P3` issues with
milestone-at-filing, deduplicates against already-`ultra-review`-labeled
issues, and files the survivors autonomously within a bounded evidence bar —
mirroring D-022's standing-authority precedent rather than `pycc-feedback`'s
per-payload gate. It mutates no tracked file and implements nothing itself.
```

- [ ] **Step 3: `docs/AGENT_TOOLING.md` — count bumps**

Three occurrences, each "six" → "seven" (verify each edit's surrounding text still reads correctly after the substitution):

- Line 215: `` `issue-select` seven, `` — unrelated existing "seven" (issue-select's own eval count), do **not** touch.
- Line 226: `` (`EXPECTED_RUNNERS` in that script names all six alpha skills). `` → `` (`EXPECTED_RUNNERS` in that script names all seven alpha skills). ``
- Line 228: `structural check now also iterates all six alpha skills (not just` → `structural check now also iterates all seven alpha skills (not just`
- Line 234: `One thing remains deferred for all six: authenticated model-response evals` → `One thing remains deferred for all seven: authenticated model-response evals`

- [ ] **Step 4: `docs/ROADMAP.md` — Agent tooling status row**

At line 30 (the "Agent tooling" table row), within the same cell:

1. After the existing clause `` and the `next-milestone` scoping skill (walks `docs/ROADMAP.md`'s ordered milestone sections to find the first one whose Accept criteria are not yet met with real evidence, adopts it as the standing goal, and hands off to `issue-select`) `` and before `, are discoverable in both Codex and Claude Code.`, insert:

```
and the `ultra-review` skill (periodically reviews the diff since a GitHub-native checkpoint with the same pinned D-068 reviewer, files `P1`/`P2`/`P3` GitHub issues for confirmed findings within a bounded, dedup-gated, autonomous evidence bar)
```

2. Change both occurrences of `All six alpha skills` / `now covers all six as well` in that same cell to `All seven alpha skills` / `now covers all seven as well`.

- [ ] **Step 5: `docs/SPEC.md` — DECISIONS.md range**

At line 18, change `` D-070…D-146 `` to `` D-070…D-147 `` (re-verify this number against Task 5's actual re-resolved decision number before committing — see Global Constraints), and append one clause to the long parenthetical list, after `` preserving list bindings during private-helper constraint collection via a destructured `Ty::List` element-type carrier) `` becomes:

```
preserving list bindings during private-helper constraint collection via a destructured `Ty::List` element-type carrier, and adding the `ultra-review` periodic-review-and-file skill with its empirically-settled single-pass design)
```

- [ ] **Step 6: `docs/DECISIONS.md` — new table row**

Immediately after the `| D-146 | ... |` row (currently line 146 area), insert:

```markdown
| D-147 | Add the `ultra-review` skill: a periodic, evidence-gated codebase review that reuses the pinned D-068 deep-reviewer (single pass, chosen over a broader architecture pass after a live empirical bake-off) against the diff since a GitHub-native checkpoint, maps `blocker`/`warning`/`note` findings to `P1`/`P2`/`P3` GitHub issues with milestone-at-filing and dedup, and files them autonomously within a bounded evidence bar, mirroring D-022's standing-authority precedent | accepted |
```

(Renumber to whatever the actual next-free number is if D-147 was claimed by a concurrent PR in the meantime — see Global Constraints.)

- [ ] **Step 7: `docs/DECISIONS.md` — new long-form entry**

Immediately after the `## D-146: ...` section's own closing line (before the next `## D-1XX:` heading or end of file, whichever comes first), insert:

```markdown

## D-147: Add the `ultra-review` skill for periodic, evidence-gated codebase review

- Status: accepted
- Context: This repository's only pre-merge review gate (D-068's pinned `deep-reviewer`) is diff-scoped by design — it reviews one pull request's changes against the files that pull request touches. Drift that only becomes visible once several merges have accumulated, or that lives entirely in a file no recent diff happened to touch, has no existing mechanism to surface it. The user asked for a periodic "ultra review" that files prioritized, milestone-scoped GitHub issues for what it finds, without duplicating existing open issues and staying consistent with `docs/ROADMAP.md`.
- Decision: Add `ultra-review`, a new alpha skill that reads a GitHub-native checkpoint (a dedicated tracking issue, not a tracked file), computes the diff since that checkpoint, dispatches the exact pinned D-068 `deep-reviewer` once against it, maps its `blocker`/`warning`/`note` findings to `P1`/`P2`/`P3` GitHub issues (milestone-at-filing per the existing convention, `ultra-review`-labeled for dedup), deduplicates against already-filed issues carrying that label, and files the survivors autonomously — without per-payload human confirmation, mirroring D-022's standing-authority precedent — within a bounded evidence bar (concrete `file:line` required, dedup pass mandatory, secrets redacted by the reviewer's own existing checklist point 11, a roughly-15-candidate batch-size guard). The single-pass architecture (reusing the pinned reviewer, no new reviewer artifact) was chosen over an originally-proposed second, broader architecture/roadmap-alignment pass after a live empirical bake-off: two review variants were run against the same real accumulated diff, and an independent judge verified that the second pass's only genuinely-out-of-scope finding was weak and non-blocking, while its other findings either duplicated what the single pass already caught or were an outright false causal claim that distracted from a real drift bug sitting in the same file region — recorded in full in `docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md`. Recurrence is external (the existing `schedule`/Routines mechanism or its Codex equivalent) — the skill is only the review procedure, not a scheduler.
- Alternatives: a second "architecture alignment" review pass alongside the pinned mechanical one (empirically tested and rejected — see Decision above); a full-repository re-sweep every run instead of incremental diff-since-checkpoint (rejected — unbounded cost, re-discovers the same finding on unrelated code every pass); the skill self-scheduling its own recurrence via a GitHub Actions cron workflow invoking a headless agent (rejected — needs a new CI secret and its own D-021/CI-privilege-boundary review, and no other skill in this repository self-schedules); per-payload human approval before filing, mirroring `pycc-feedback`'s own gate (rejected — would silently defeat unattended periodic operation, since nobody would be present to approve a scheduled run's findings); a local tracked checkpoint file (rejected — this session directly observed its own worktree being recycled mid-session, which a tracked-file checkpoint would not survive, and a commit-per-run just to bump a counter is unnecessary churn under this project's protected-`main` rule).
- Consequences: a new skill `.claude/skills/ultra-review/SKILL.md` with a `.agents/skills/` thin pointer and a four-case `evals.json` is added, registered in `scripts/run_alpha_skill_evals.py`'s `EXPECTED_RUNNERS`/`ULTRA_REVIEW_CONTRACT`/`run_ultra_review_case` and `scripts/validate_agent_assets.py`'s `ALPHA_EVAL_RUNNERS`/`validate_alpha_skill_contracts` tuple, exactly mirroring D-144's own registration of `next-milestone`. `docs/AGENT_TOOLING.md` and `docs/ROADMAP.md`'s "Agent tooling" row count bump from "all six" to "all seven" alpha skills. `docs/SPEC.md`'s DECISIONS.md range extends to this entry. This skill introduces this repository's second standing autonomous-write authority (after D-022) — GitHub issue creation without per-payload confirmation — bounded by the evidence bar in the Decision above; a future session finding that bar too loose or too tight in practice should record that finding and its own resolution as a correction to this entry, not silently relax or tighten it in the skill's own prose alone.
```

- [ ] **Step 8: Verify no other stale count references remain**

Run:

```bash
grep -rn "all six alpha skills\|all six as well\|for all six:" docs/ scripts/ .claude/ .agents/
```

Expected: no output (every occurrence was already updated in Steps 1-4 above).

- [ ] **Step 9: Commit**

```bash
git add docs/AGENT_TOOLING.md docs/DECISIONS.md docs/SPEC.md docs/ROADMAP.md
git commit -m "Document the ultra-review skill: AGENT_TOOLING.md, D-147, SPEC.md, ROADMAP.md"
```

---

### Task 5: Final integration — gates, D-068 review, dogfood runs, pull request

**Files:** none new; verification and process only.

**Interfaces:** none.

- [ ] **Step 1: Run the full local gate set**

```bash
python3 -m unittest discover -s scripts -p "test_*.py" -v 2>&1 | tail -30
python3 scripts/validate_agent_assets.py; echo "validate_agent_assets exit: $?"
python3 scripts/validate_agent_policies.py; echo "validate_agent_policies exit: $?"
python3 -m unittest scripts.test_validate_agent_policies -v 2>&1 | tail -10
python3 scripts/run_alpha_skill_evals.py --client claude; echo "claude evals exit: $?"
python3 scripts/run_alpha_skill_evals.py --client codex; echo "codex evals exit: $?"
```

Expected: every command exits 0 (capture each `$?` explicitly per this project's own documented lesson about pipeline exit codes lying about gate verdicts — never trust a piped/tail'd summary alone).

- [ ] **Step 2: Stage everything and run the pinned D-068 reviewer**

```bash
git add -A
git status --short
```

Dispatch the pinned deep reviewer from the digest-recorded artifact (`docs/AGENT_TOOLING.md`'s D-068 pin) against the full staged diff from `origin/main` to the current tree. Fix every actionable finding, restage, and re-review until a round reports nothing actionable — per this repository's own D-068 loop discipline (`AGENTS.md`'s "Local pinned review loop" section).

- [ ] **Step 3: Commit any review-loop fixes**

```bash
git add -A
git commit -m "Address D-068 pinned review findings"
```
(Skip this step entirely if the first review round was already clean.)

- [ ] **Step 4: Dogfood run 1 — invoke the new skill for real**

Invoke the `ultra-review` skill directly against `rotnov/pycc`'s live default branch (this is the skill's own first-ever run: expect the checkpoint-bootstrap path from its step 2 — it will create the `ultra-review-checkpoint` tracking issue and report a clean no-op, since there is no prior checkpoint to diff against). Confirm:
- the `ultra-review-checkpoint` and `ultra-review` GitHub labels now exist;
- the tracking issue was created with the exact body format from the skill's step 8;
- no other public write happened on this bootstrap run.

- [ ] **Step 5: Dogfood run 2 — a real incremental review**

Immediately re-invoke `ultra-review`. Since Steps 2-4 above pushed no commits to `origin/main` yet (this task's own commits are still local, on the task branch), the diff since the bootstrap checkpoint may legitimately be empty — if so, this run should report the same "nothing new" no-op from its own step 3, which is itself a valid, useful confirmation that the empty-diff path works correctly against the live repository, not just in the offline eval. If any other repository activity landed on `main` in the meantime (this project has a documented concurrent actor), this run instead performs a real review of that diff and may file real `P1`/`P2`/`P3` issues — if it does, read every filed issue back via `gh issue view` and confirm each one actually has a concrete file:line, is not a duplicate of something already open, and is milestone-assigned or explicitly left unassigned consistent with step 5's rule.

- [ ] **Step 6: Fix anything the dogfood runs surfaced**

If either dogfood run surfaced a defect in the skill's own text (a `gh` command that doesn't do what the prose says, a checkpoint-parsing edge case, a wrong dedup search) — fix `.claude/skills/ultra-review/SKILL.md` directly, re-verify with a fresh dogfood invocation, and repeat until two consecutive dogfood runs behave correctly. If the dogfood runs surfaced a real, unrelated finding about the repository itself (not a bug in this skill) — that is the skill working as designed; leave the filed issue as this task's own first real output and do not fold fixing it into this PR's scope.

- [ ] **Step 7: Add a `docs/sessions/` checkpoint entry**

Re-fetch `origin/main` immediately before this commit (per D-066's own currency requirement, as narrowed by [D-130](../../DECISIONS.md#d-130-decompose-the-session-handoff-log-into-per-session-files)), then add a new dated checkpoint file `docs/sessions/YYYY-MM-DD-NN-<slug>.md` (never append to an existing one) summarizing: the `ultra-review` skill added, the empirical single-pass-vs-hybrid decision and where it's recorded, the D-147 (or renumbered) decision entry, and the two dogfood runs' outcomes including any issue numbers filed.

- [ ] **Step 8: Push and open the pull request**

```bash
git push -u origin feat/ultra-review-skill-design
gh pr create --title "Add ultra-review: periodic evidence-gated review that files prioritized issues" --body "$(cat <<'EOF'
## Summary
- Adds the `ultra-review` alpha skill: reads a GitHub-native checkpoint, reviews the diff since it with the pinned D-068 deep-reviewer (single pass -- an originally-proposed second architecture-review pass was empirically tested and rejected, see docs/superpowers/specs/2026-08-05-ultra-review-skill-design.md), maps findings to P1/P2/P3 GitHub issues with milestone-at-filing and dedup, and files them autonomously within a bounded evidence bar (mirrors D-022's standing-authority precedent).
- Registers the skill in both alpha-skill governance scripts (run_alpha_skill_evals.py, validate_agent_assets.py) and documents it in AGENT_TOOLING.md/ROADMAP.md/DECISIONS.md (D-147, indicative -- renumbered if claimed by a concurrent PR).
- Dogfooded twice against the live repository before opening this PR (see this branch's newest docs/sessions/ entry for the exact outcome of each run).

## Test plan
- [ ] `python3 -m unittest discover -s scripts -p "test_*.py"` passes
- [ ] `python3 scripts/validate_agent_assets.py` exits 0
- [ ] `python3 scripts/run_alpha_skill_evals.py --client claude` and `--client codex` both exit 0
- [ ] D-068 pinned review loop reports no actionable findings
- [ ] Two dogfood runs against the live repository behaved correctly (see this branch's newest docs/sessions/ entry)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 9: Monitor CI and merge**

Follow this project's own `gha-watch-ci-pr` skill for the CI wait (not a fixed `sleep`). Once every required check is green, re-read the full diff one final time, then merge with a merge commit and delete the task branch, per this repository's own standard merge discipline (`AGENTS.md`'s D-024/D-078 sections).
