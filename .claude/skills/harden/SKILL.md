---
name: harden
compatibility: Requires uv (astral.sh/uv) — bundled scripts are PEP 723 files that declare their own interpreter, so no system Python is needed. Arena runs additionally need the CLIs of the harnesses being compared (claude, devin, codex, grok) and network access; missing harnesses are skipped, not fatal. Not verified on Windows.
description: Use when a failure has just happened — the user points out a mistake, a check fails, or you realize an assumption was wrong — and it should not happen again. Traces the failure to the artefact that owns it, picks an artefact type by how the failure is detectable, builds it, and proves it with the arena before it ships.
---

# Harden: turn a failure into a proven guard

Analyze a failure, trace it to the artefact that could have prevented it, build a guard
of the right kind, and prove the guard works before shipping it.

Not "evolve the rules": on measured data only ~4% of corrections are logic errors,
and a textual rule that has already failed three times will fail a fourth. The output
is usually a hook, a check or a fix to the system — a rule is one option of six.

## Installing a practice

`/harden install <name…>` sets up a practice the project does not have yet — a
decision log, a handoff log — rather than repairing a failure; `/harden
uninstall <name>` removes one. You execute the whole flow; the python
installer inside it is the engine that resolves dependencies, writes the lock
and seeds write-once files:

1. **Unpack if needed.** Practice skills ride inside this skill under
   `skills/` (a catalogue install delivers only harden). Make them siblings:
   `uv run scripts/unpack-skills.py` — idempotent, and a project's own skill
   by the same name is never replaced.
2. **Audit before touching anything — all requested practices, then act per
   practice.** The project may already run its own edition — search the
   governance file and the target directories, and read its decisions log for
   policies a feature would contradict. A practice with a hit is skipped
   whole and named in the report with its evidence; the practices with no
   hit install normally. Replacing a skipped one is the user's explicit call
   (`--force` on the file layer, their typed word on the flow layer) — and
   force replaces the mechanism, never their data (incident:
   step0-blind-to-peer-practice).
3. **Install**: `uv run scripts/install-feature.py <name…>` (`--list` to see
   the menu). Then perform the printed after-steps and verify with `--check`.

Procedure, bundles, injection and uninstall rules:
[references/install.md](references/install.md)

## Dispatch — read the arguments before step 1

- Arguments start with `install` or `uninstall` → this is **not a failure to
  trace**. Run the Installing-a-practice flow above; a bare `install` means
  show the menu (`--list`) and stop. Never ask what went wrong.
- Arguments start with `batch <findings-file>` → one pass over a pile of
  review findings, clustered into classes first:
  [references/batch.md](references/batch.md). Not a failure to trace either.
- Anything else, or no arguments → the failure workflow below.

Arguments outrank the workflow (incident: arguments-lost-to-the-workflow).

## Workflow

### 1. Identify the Error

(`install`/`uninstall` in the arguments never reach this step — see Dispatch.)

If not already clear, ask the user to describe or reference the mistake. Review recent conversation context. Classify:

- **Code error**: incorrect logic, syntax, wrong API usage
- **Process error**: skipped validation, didn't run tests, ignored pre-commit
- **Communication error**: misunderstood requirements, made assumptions
- **Safety error**: destructive action, lost work, broke existing functionality
- **Style error**: violated project conventions

### 2. Root Cause Analysis

For **non-trivial failures** (two or more hops upstream are plausible, or the
termination point is not obvious from a single read of the symptom), dispatch a
subagent with the tracer role:

```
Agent(subagent_type="general-purpose", prompt="""
Read references/failure-tracer.md in full as your role definition, then trace:

  symptom:   <what was observed, verbatim where possible>
  last acts: <the agent's last tool calls, with arguments>
  user said: <exact words, if this started as a correction>
  model:     <model / effort / harness>
  project:   <absolute path>
""")
```

**Fill every field.** The subagent runs in a fresh context and cannot see this
conversation — it has this prompt and the filesystem, nothing else. A missing
field is not a small gap: the trace starts at the last action, so without it the
tracer reconstructs, and a reconstructed step points the fix at the wrong
artefact. It fetches the governance surface, git history and the journal itself.

It walks a structured backward-trace, terminates at one artefact, and returns a
single structured report (trace + termination point + recommended placement +
concrete edit + confidence). Observe-and-report only — Step 5 is the gate that
lands anything.

The role lives in `references/`, not in `.claude/agents/`: agent files are not
part of the agentskills.io spec and would only travel with a plugin. Everything
this skill needs sits inside the skill directory, which is what a catalogue
installs.

For **trivial failures** with a single obvious cause (typo, mis-cited line,
dropped import), answer four questions inline instead: what assumption was wrong,
what step would have caught it, what information was missing, was there a pattern.

Say which path you took before proceeding — a trace delegated silently and a trace
skipped silently look identical in the output.

The subagent has the full tool set, wider than the role needs. The role's own hard
rules — never modify a file, report only — are the boundary that matters; keep them
intact if the role file is ever revised.

### 3. Formulate the Lesson

Write a concise, actionable rule. It must be:

- **General** — describes the failure *pattern*, not the specific incident. Strip every concrete identifier (file paths, line numbers, function/variable names, ticket IDs, dataset names, locale codes, the specific value that was wrong) from the rule body. The incident motivates the rule; the rule must read identically when applied to a different codebase next year.
- **Actionable** — clear steps the agent can execute without consulting the original incident.
- **Testable** — compliance can be verified by inspecting future changes.
- **Minimal** — no unnecessary process, no enumerated examples in the rule body, no "this caught:" anecdotes. The incident lives in the incident journal; the rule lives in the target governance surface.

**Anti-pattern to avoid:** rules that read like incident reports. If the rule body names a specific file, function, parameter, or value from the triggering session, rewrite it. The incident is the lesson's *trigger*, not the lesson's *content*. Long-lived governance surfaces are reloaded by future agents; concrete examples bloat future context without adding behavioural value.

### 3.5 Choose the artefact type — before opening any file

Step 4 answers **who owns** the failure; this step answers **what kind of guard**
to build. Skipping it is how everything ends up as prose in `AGENTS.md`.

Frame: where is the source · what removes the cause rather than the symptom ·
who is entitled to do it · what will show it helped. No metric → do not ship.

Name the gap before naming the guard — why did the existing defence not fire:
**trigger** (exists, did not fire at the fork → fix the trigger, or plant a
pointer at the fork inside the running procedure), **content** (fired, did
not cover → edit its body), **absence** (nothing exists → build new),
**compliance** (a clear rule ignored → text cannot fix it; a mechanical rung
can). Full table with routing:
[references/failure-tracer.md](references/failure-tracer.md), §Gap
classification.

The criterion the type choice usually reduces to is **what detects this
failure**: a static command → pre-commit/CI; reading the diff → a reviewer
check; the fact of a tool call → a **seam**; nothing mechanical →
documentation; not reproducible → **build nothing**.

Before building anything, check whether the class is already solved:
`uv run scripts/discover.py "<failure class>" "<another phrasing>"`. Taking a
maintained skill beats writing your own — but only through find → audit → prove
→ install: [references/external-skill.md](references/external-skill.md).

Ladder, seam subtypes and their hard limits:
[references/artefact-types.md](references/artefact-types.md)

### 4. Review the target governance surface

Read the analyst's *Termination point* (or the inline-analysis result from Step 2 for trivial failures). The flow downstream depends on which artefact the trace landed at:

- **Project rule** → open the governance file the project actually uses
  (`AGENTS.md`, `CLAUDE.md`, or whatever it keeps), and place the rule in the
  section that already covers the topic. A rule filed away from its neighbours is
  read as trivia.
- **Local-skill / Local-agent** → open that file directly; it *is* the target.
- **Upstream-plugin** → skip to Step 4c.
- **User-discipline** → skip to Step 5, informational only; the skill exits there.

Steps 4a / 4b apply in every case — substitute the target file for `AGENTS.md`
throughout. The audit's questions are as relevant inside a skill body as inside a
governance file.

**Check whether the artefact reaches far enough.** A skill or a harness hook binds
one harness; a `precommit` gate binds every agent and every human who commits. If
the arena's *control* column failed on every harness, the class is general
and a single-harness fix is knowingly partial — reconsider the rung before
editing. Reach table: [references/artefact-types.md](references/artefact-types.md).

Check if a similar rule already exists that should be updated instead.

### 4a/4b. Audit existing rules, then decide add / replace / consolidate

Before writing anything, scan the target surface for the patterns that let this
failure slip past the rules already there — a new rule next to an unaddressed gap
gets bypassed the same way next time. Seven patterns, plus the recurrence check.

**The recurrence check is the one that changes the outcome:** `ls .harden/incidents/`.
An existing topic means this is not a new lesson but the failure of the artefact
chosen last time — escalate a rung instead of rewording. Three or more files in
one topic disqualify a textual artefact outright.

Checklist and decision rules: [references/rule-audit.md](references/rule-audit.md)

### 4c. External-plugin escape hatch

When the trace terminates at an upstream plugin, never edit upstream. Fork it
locally with attribution, apply the change to the fork, and record the divergence.

Procedure: [references/upstream-fork.md](references/upstream-fork.md)

### 5. Propose the Update

Show the user, whatever the termination:

- **Which decision** from 4b applies — add / replace / consolidate — and why.
- **The proposed text**, in the target file named at Step 4, with its placement.
- **The audit findings** from 4a: which gaps it closes, which rules it supersedes.
- **For replace / consolidate — the diff.** Removing a clause nobody approved is
  the same class of mistake the rule is meant to prevent.
- **Why a weaker version would not have prevented the failure.** If a vaguer
  wording would have worked equally well, the specific one is over-fitted to
  this incident.

For an `Upstream-plugin` termination add the Step 4c fork plan: which upstream
file, which licence and commit, and the attribution header. Divergences go in the
fork, never upstream.

A `User-discipline` termination reaches this step for information only — trace,
termination point, recommended action outside the codebase. No edit is proposed
and the skill exits here.

**Approval is required before editing** — for removals or merges, of the
diff, not the new text alone. Unattended, or under a host no-questions law:
record the proposal in the incident entry and the PR body and proceed — PR
review is the approval surface (incident: approval-pause-strands-autonomous-run).

### 6. Apply the Update

The edit lands at the artefact named as the *Termination point*. Step 4's routing
decision and this step must target the same file: a trace that terminated at a
skill, an agent or a hook, patched in the governance file instead, is a routing
failure and not a safe default — the failure keeps its owner either way.

1. **Edit the target file**, per termination:
   - `Project rule` → the project's governance file
   - `Local-skill` / `Local-agent` → that file
   - `hook` → the harness hook configuration plus the hook script
   - `precommit` / `review-check` → the gate config; these bind every harness
   - `external-skill` → installed per references/external-skill.md, after an
     audit and an arena verdict; the project gets it last
   - `Upstream-plugin` → the local fork from Step 4c; upstream stays untouched
   - `User-discipline` → no file edit; that branch ended at Step 5

   Where a project keeps the same skill duplicated per harness, both copies are
   one change. Copies drift — if that is happening often, the artefact belongs on
   a rung that binds all of them at once.

2. **Run the artefact through the arena** — `uv run scripts/arena.py <fixture> --runs 3` (see [references/arena.md](references/arena.md)).

   Mandatory for artefacts that change **agent behaviour**: `hook`,
   `review-check`, `rule`. Not applicable to `doc`, `system`, `none`.

   **Static gates are proven differently.** A `precommit` artefact is a command
   with a binary outcome, not a behavioural change — the arena measures agents
   across harnesses and has no way to exercise a git hook. Prove it by feeding
   it deliberate violators and checking both directions: violations rejected
   with a non-zero exit, clean tree accepted with zero. Record that as
   `verify: manual` with the violator output pasted into the incident.
   Forcing such an artefact through the arena produces ceremony, not evidence.

   **Generate a fixture for this incident** — `uv run scripts/new-fixture.py <topic-slug>`
   scaffolds it under `.harden/incidents/<topic>/fixture/`, beside the incident it belongs to.
   One fixture per incident — someone else's fixture proves their case, not yours.

   It reproduces the incident: `task.md` (the prompt, and nothing but the prompt —
   if it has to mention the trap, you are testing the prompt, not the artefact),
   `control.md` and `patch.md` differing **only** by the artefact under test
   (never a file named `AGENTS.md` — agents pick those up from surprising places;
   the arena renames on copy), and `verify.py` checking the result *and the workaround*.
   Every installed harness, two conditions.

   `fixtures/example-uv-vs-pip/` is a worked example kept for its shape — read it,
   do not aim the arena at it.

   **Review the fixture before aiming the arena at it** — dispatch
   [references/fixture-review.md](references/fixture-review.md) with the
   fixture and the incident's stated goal; it returns run or fix-first with
   defects named. A campaign on an unreviewed fixture is spend, not
   evidence. arena.py refuses one class on its own: a verify that passes on
   an empty workdir.

   - **profit** → ship it
   - **zero** → do not ship, and do not reword it — the model read the artefact
     and was unmoved, so the mechanism is too weak for this class. Return to 3.5
     and take the next rung (rule → review-check → precommit → seam). Same signal
     as a recurrence in 4a, bought early
   - **harm** → do not ship; the model is not indifferent but resisting, so a
     stronger version of the same type makes it worse. Change type
   - **not enough data** → fix what the report lists, then re-run. A verdict on
     contaminated data is worse than no verdict, because it looks like a result
   - **no baseline** → the control passed; nothing was measured. Fix the fixture,
     not the artefact

   **Stop after two escalations.** If the top of the ladder still reports `zero`,
   the failure is not addressable by an artefact — leave the incident open with
   `verdict: pending` and say so.

   Reviewing the edit is not a substitute: review checks whether it *reads*
   well, the arena checks whether it *works*.

3. **Summarise the change** — which artefact, which files, what behaviour changed,
   and the arena verdict. The summary feeds the incident record (Step 7).

4. **Commit the artefact and its journal entry, and carry on.** State the message
   you used; do not stop for approval of it. A local commit is reversible, so a
   confirmation step here buys nothing and costs the one thing that matters for
   unattended runs: the ability to finish.

   Commit on a branch the host's flow allows: when HEAD is the default branch
   of a project that reviews through pull requests, create a branch first —
   an unpushed commit straight to the default branch still bypasses the
   host's review gates, and only some harnesses carry a built-in
   branch-before-committing net (field-measured on one that does not).

### 6.5 Sweep, and 6.6 decomposition check

Before considering the artefact shipped, run its diagnostic across the whole repo
and record a `Sweep result` line — a rule that ships without a sweep is future
technical debt, not a fix.

Then check whether the accumulated rules have grown into a cluster that belongs
in its own skill or agent rather than in prose.

Both procedures: [references/sweep-and-decompose.md](references/sweep-and-decompose.md)

### 7. Record the incident

One file per incident, in a folder named after the **topic** — the folder is the
recurrence counter, so `ls` answers "has this happened before" with no tooling.

Routing follows where the fix physically landed: project artefacts →
`<project>/.harden/incidents/`, global ones → `~/.harden/incidents/`.
Project journals are committed, so **scrub before writing**.

**When the host project keeps its own process-mistake journal** (a
retrospective log, a postmortem skill — audit for one the way Step 0 audits
at install), do not write narrative prose into a file this skill does not
own — and do not stop at naming the handoff either: READ the host's
postmortem skill and WALK its steps against the structured facts (symptom,
gap type, termination, artefact, journal path), so the entry lands in their
journal through their own admission bar. Facts written beside their journal
instead of through it left the allowlist their CI reads empty twice
(incident: step0-blind-to-peer-practice).

`fixture`, `artifact` and `verify` are mandatory fields — without them nothing
can be reproduced or proven, and the entry is a diary, not a journal.
An incident with `verdict: pending` is **open**.

**A finding about this skill itself does not stay in the field.** When the fix
lands in the skill's own files, deliver it upstream as a feature proposal —
the fixture and the arena verdict packed into one issue:
[references/upstream.md](references/upstream.md).

Full layout, field list and rationale: [references/journal.md](references/journal.md)

### 8. Confirm Understanding

Restate the lesson learned. Acknowledge the error and commit to following the new rule.

## Example

User says "/harden" after forgetting to run tests before proposing a change.

Lesson: "Always run relevant tests before proposing code changes that modify behavior. If tests cannot be run, explicitly state why and outline a validation plan."

Note that the rule body says nothing about which tests, which file, or which session — it would read identically if a different test framework caught a different change in a different repo. The incident-specific context (the file path, the test name, the user's original message) goes in the incident journal, not in the AGENTS.md rule.
