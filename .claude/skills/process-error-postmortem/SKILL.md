---
name: process-error-postmortem
description: Use when the agent catches itself having made a process mistake (wasted meaningful time, produced a wrong intermediate result, violated a convention, used the wrong tool for the job) or the user points one out. Diagnose the root cause, identify which existing artifact (a skill's SKILL.md, AGENTS.md, an ADR, or the absence of one) failed to prevent it, and either patch that artifact directly or propose the patch — then record the entry in docs/AGENT_RETROSPECTIVE.md and, if the lesson is durable, promote it into the owning artifact per the existing AGENTS.md rule. Do not use for code bugs (those belong in issues and tests), ambiguous design calls (those belong in docs/decisions/ as a new decision entry), or routine debugging that self-corrected within the same turn with no lasting effect.
---

<!-- ievo:start -->
**Before applying the instructions below**, read `.ievo/evolution/skills/process-error-postmortem.md`
if it exists, and apply ALL rules from its sections IN ADDITION to the skill's instructions.
<!-- ievo:end -->

# Process-error postmortem

A process mistake is a mistake in *how the work was done*, not in *what the code does*:
the agent used the wrong tool when a better one existed and was discoverable, waited
inefficiently when a cheaper mechanism was available, skipped a preflight step, violated
a documented convention, or followed a skill's instructions into a dead end the skill's
own text should have prevented. Code bugs belong in issues, tests, and fixes — not here.
Ambiguous design calls belong in `docs/decisions/` as decisions with alternatives, not
here as mistakes. Routine debugging that self-corrected within the same turn with no
lasting effect is not a process mistake. The bar is the same one
`docs/AGENT_RETROSPECTIVE.md` already sets: the mistake cost meaningful time or produced
a wrong intermediate result, and the lesson would help a future session avoid repeating it.

This skill fires at two moments:

1. **Self-caught** — the agent realizes mid-task that it took a wrong approach, used the
   wrong tool, or skipped a step it should have taken. The realization may come from its
   own observation or from re-reading instructions it failed to follow.
2. **User-caught** — the user points out that the agent did something the wrong way,
   asks "why did you do X instead of Y", or corrects a process choice. A user correction
   is the strongest signal this skill has a real case to work on — do not dismiss it as a
   style preference without first diagnosing whether an artifact gap made the wrong path
   look equally valid to the agent.

## Workflow

### 1. Confirm it is a process mistake

Before running the full postmortem, confirm the event meets the bar. Ask:

- Did it cost meaningful time, produce a wrong intermediate result, or violate a
  documented convention? (If it self-corrected in one turn with no lasting effect, stop
  — this is not a process mistake, it is routine course-correction.)
- Is it about *how the work was done*, not *what the code does*? (If it is a code bug,
  route to an issue or test, not this skill.)
- Is it a genuine mistake, not a disagreement about an ambiguous design call? (If
  reasonable agents could disagree on the approach, it belongs in `docs/decisions/` as a new
  decision entry, not here.)

If any check fails, do not run the rest of this workflow. A false postmortem wastes as
much time as the mistake it claims to analyze.

### 2. Diagnose the root cause

Identify which existing artifact failed to prevent the mistake. The failure modes are
specific and mutually exclusive — pick the one that fits, do not hedge:

- **Trigger gap** — a skill exists that would have prevented the mistake, but its
  `description` frontmatter does not fire at the moment the agent reached the fork. The
  skill was never invoked because its trigger wording did not match the situation. Fix:
  strengthen the skill's description to cover this moment, or add a cross-reference from
  the skill the agent *was* running (the one whose step led into the mistake) to the
  skill that should have been invoked.

- **Content gap** — a skill was invoked (or an AGENTS.md rule was active) but its
  instructions did not cover this specific case, or covered it ambiguously enough that
  the agent read them as permitting the wrong path. Fix: add the missing case or sharpen
  the wording in that skill's SKILL.md or the AGENTS.md section.

- **Absence gap** — no skill, ADR, or AGENTS.md rule addresses this situation at all.
  The agent had no instruction to follow and took a plausible but wrong default. Fix:
  create a new skill, a new ADR, or a new AGENTS.md rule — see step 3's branch for
  which.

- **Compliance gap** — a clear rule existed and the agent simply did not follow it. This
  is the rarest case and the hardest to fix with more text — if the rule was clear and
  the agent skipped it, the problem is in the agent's own discipline, not in the
  artifact. Record the entry in `AGENT_RETROSPECTIVE.md` so the pattern is visible, but
  do not add redundant text restating a rule that already exists; instead, consider
  whether the rule is buried in a place the agent's context did not surface at the right
  moment (which makes it a trigger gap in disguise — the rule exists but was not
  reachable at the fork).

### 3. Classify and apply the fix

The fix path depends on which artifact needs to change:

- **Patch to an existing SKILL.md or AGENTS.md** (trigger gap, content gap): apply the
  edit directly in the current task branch or a fresh branch from `main`. These are
  process-text changes, not code changes — they do not need their own PR unless the
  current task is already a PR with a different scope. If the current task is a PR that
  this mistake occurred during, add the fix to that PR's branch. If the mistake is
  post-merge or unrelated to an open PR, open a small dedicated PR.

- **New skill** (absence gap, and the lesson is a reusable workflow, not a one-off
  rule): create the skill under `.claude/skills/<name>/SKILL.md` with a `.agents/skills/`
  thin pointer, following the existing convention. Open a dedicated PR — a new skill is
  architecturally consequential and needs its own review, not a drive-by edit inside
  another PR.

- **New ADR** (absence gap, and the lesson is an irreversible or project-wide design
  choice): create `docs/decisions/D-1NN-<slug>.md` per `docs/decisions/TEMPLATE.md`,
  regenerate the index (`scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md`), and open a dedicated PR.

- **New AGENTS.md rule** (absence gap, and the lesson is a binding process rule, not a
  full skill or ADR): add the rule to the relevant section of `AGENTS.md` and ship it in
  the same PR as the retrospective entry, or a dedicated PR if the mistake is
  post-merge.

### 4. Record the entry in AGENT_RETROSPECTIVE.md

Every postmortem — regardless of fix path — produces an entry in
`docs/AGENT_RETROSPECTIVE.md`, following that file's existing format exactly:

- Date, one-line title.
- **What happened:** factual, specific, citing the actual commit/PR/file.
- **Root cause:** which artifact failed and which failure mode (trigger/content/absence/
  compliance).
- **What fixed it:** the edit applied, the issue/PR opened, or the rule added.
- **Lesson:** actionable, in a form a future session can follow ("when X, do Y", not "be
  more careful").

Newest entries first. Do not log a mistake that produced no entry-worthy lesson — the
file is not a diary of every wrong turn, it is a curated set of lessons that change
future behavior.

### 5. Promote if the lesson is durable

Per the existing AGENTS.md rule: "Promote any rule discovered there into `AGENTS.md`,
`docs/decisions/`, or the owning specification before relying on it as policy." A
retrospective entry alone is informational, not binding. If the lesson should govern
future sessions, step 3's fix is the promotion — the artifact edit, ADR, or AGENTS.md
rule *is* the binding version, and the retrospective entry records how it was discovered.
Do not leave a lesson stranded in the retrospective file if it should be policy; do not
duplicate a lesson into the retrospective if the artifact edit already carries it
completely — the entry's value is the discovery narrative, not restating the rule.

## What this skill does not do

- It does not fix code bugs. A code bug is a defect in *what the code does*, and it
  belongs in an issue, a failing test, and a fix — not in a process postmortem. If a
  process mistake *caused* a code bug (e.g., skipped a test step that would have caught
  it), the postmortem addresses the skipped step, not the bug itself.

- It does not resolve design disagreements. If the user and the agent reasonably
  disagree on which approach to take, that is a design call — record it as a new file in
  `docs/decisions/` with alternatives, not in the retrospective as a mistake.

- It does not run on every minor course-correction. A wrong turn caught and fixed
  within the same turn, with no lasting effect and no lesson a future session would
  benefit from, is not a process mistake — it is normal iterative work. Running a full
  postmortem on it would itself be a process mistake (wasting time on a non-event).

- It does not blame. The retrospective is for learning across sessions, not for
  assigning fault. "The agent used `sleep` instead of `ci-watch.sh`" is a factual
  description; "the agent was careless" is not.
