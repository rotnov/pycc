# Sweeping the repo for the pattern, and extracting rule clusters into a skill

### 6.5 Sweep existing repo for the pattern the new rule forbids

Before recording the incident and considering the rule shipped, run the rule's diagnostic check across the **entire repo** (not just the diff that motivated the rule) and fix any pre-existing instances in the **same PR** as the rule addition. A rule that ships in `AGENTS.md` while the repo already violates the rule sits as latent technical debt — every future `/deep-review` will surface the same failures, the rule loses authority on the next reader who notices the violations, and a "rule landed but old code still violates" state is the canonical marker of half-finished `/harden`.

Concrete workflow:

1. **Extract the diagnostic check from the rule's text.** The check is whatever `grep` / canary / verification command catches violations. Examples seen in this repo:
   - Path-leak rule → `git grep -nE '/Users/[^/]+/|/home/[^/]+/|C:\\\\Users\\\\|/Volumes/|/private/var/folders/'`
   - tool-surface rule → a loop over agent files, flagging any that declare tools
     the rule forbids
   - Source-attribution rule → grep the governance and agent files for the
     citation pattern, then check each hit against the source it claims

2. **Run it against the entire repo.** Capture the full hit-list, not just a sample.

3. **For each pre-existing violation, decide explicitly:**
   - **Fix in same PR** (default for ≤ 10 violations or when fix is mechanical) — sweep alongside the rule addition.
   - **Defer with an explicit follow-up incident** (when the sweep is large or risky — it touches generated artefacts, or files a runtime may re-read). Open a follow-up incident in the journal and reference it from the `Sweep result` line, so the deferral is visible rather than implied.
   - **Annotate as known carve-out** (rare — when the violation is intentional and should remain). The carve-out reason MUST live in the rule itself or in an inline comment at the violation site, not in the incident journal only.

4. **The the incident journal entry MUST include a "Sweep result" line.** Three valid forms:
   - `Sweep result: 0 pre-existing violations found.` (clean introduction)
   - `Sweep result: N pre-existing violations swept in this PR (file list: …).` (rule + cleanup landed together)
   - `Sweep result: N pre-existing violations deferred — see <follow-up incident>.` (cleanup explicitly out of scope)

   Silent "rule added without sweep mention" lets the next reader see the rule as vapor and creates ambiguity about whether the existing repo is already compliant or not.

5. **If the rule lands in a reviewer checklist with a canary**, also add the canary's exact command-line text to the rule body, so the next review runs the same diagnostic without re-deriving it. The canary becomes part of the rule's wire format, not a one-off PR-time invocation.

6. **PR creation is not `/harden`'s job.** Every `/harden` PR is docs-only by construction (`AGENTS.md`, the incident journal, occasionally a sister `.claude/agents/*.md` or `.claude/skills/*/SKILL.md` cross-ref). Once the commit lands and the user approves the push, hand PR creation to whatever the project uses for it, so its conventions apply uniformly.

This step is the difference between a rule shipped *and applied* vs a rule shipped *as future technical debt*. The marginal cost is one `grep` run and one paragraph in the the incident journal; the marginal benefit is "the next review passes on the new rule" instead of "the next review reports the same violations the rule was supposed to prevent."

### 6.6 Decomposition opportunity check — extract rule clusters into a skill or agent

After the rule is applied and the sweep is complete (steps 6 + 6.5), examine the section that now hosts the new rule (and its surrounding bullets) for **rule clusters** that could be encapsulated by a dedicated skill or agent and referenced from `AGENTS.md` instead of inlined. The default reflex of "add another bullet" makes `AGENTS.md` grow monotonically; per-session context cost rises every `/harden` cycle. Decomposition keeps `AGENTS.md` size sub-linear in rule count — clusters compress into one-line pointers, while the rule bodies live in skill / agent files loaded only on invocation.

Extractions land in one of **three destinations**, picked by what the rule is about:

| Destination | When | Existing examples |
| --- | --- | --- |
| **Skill** (`.claude/skills/<name>/SKILL.md`) | A cluster of 3+ rules sharing a common trigger / workflow / decision point. The agent encountering the trigger invokes the skill via `Skill(skill="<name>")`. | a PR-creation skill (extracted from §11 PR-creation discipline), a workflow skill |
| **Agent** (`.claude/agents/<name>.md`) | A cluster of 3+ rules forming a coherent role / persona — the rules describe HOW someone of that role thinks and decides, not just a workflow. The agent is invoked via `Agent(subagent_type="<name>")`. | a role agent |
| **Docstring on the targeted code surface** (Pydantic field, function signature, class docstring) | A SINGLE rule about **how to fill a specific field** or **how to write a specific function** — the rule's natural reader is whoever is editing that surface, who looks at the field's `Field(description=...)` or the class's docstring before writing the value. AGENTS.md is too far from the edit point; the rule decays into drift the moment the field is renamed. | a field's own `description=` text (extracted from §18 #7 "Descriptions describe clinical meaning") |

A rule cluster's **common trigger** is what activates the skill / agent in practice: "when opening a PR", "when adding a clinical citation", "when handling a PR review", "when running tests", "when writing a translatable string". Without a clear trigger an extracted skill won't auto-route and the rules go unenforced.

For docstring-on-code-surface extractions, the trigger is implicit: the rule fires when the developer edits the targeted field / class. No skill invocation needed — the rule lives where it'll be read.

Extraction is **monotonic** if and only if ALL of these hold (regardless of destination):

1. The cluster (or single rule for docstring extractions) can be expressed in the chosen destination form (workflow for skill, persona for agent, field-/class-level prose for docstring).
2. The destination encodes EVERY concrete prohibition / requirement — no silent loosening (same anti-pattern as step 4b consolidation).
3. The `AGENTS.md` replacement is a one-line pointer **plus** a short rationale ("Use `/X` when …", or "When filling field Y, follow the rules encoded on `Class.field` Field doc — see `<path>`"), NOT a duplicate body that would drift apart from the canonical surface.
4. The destination is mechanically reachable from the trigger: `Skill(skill="<name>")` / `Agent(subagent_type="<name>")` for skill / agent extractions; the field's own `Field(description=...)` text or class docstring for docstring extractions.

Docstring-on-code-surface extraction has a unique advantage over skill / agent: **the rule cannot drift from the field it documents**, because renaming the field forces touching the docstring on the same line. AGENTS.md prose, by contrast, is N file-jumps away from the field declaration and routinely goes stale (the §18 #7 extraction motivating this addition cited `hidden: bool`, `color`, `opacity`, `source` / `modified` — none of which existed on the actual class anymore; the docstring extraction closes the drift in the same edit).

When the cluster does NOT meet the monotonic criteria, leave the rules inlined. Specific anti-cases:

- **Fewer than 3 rules** — the abstraction overhead (a new file + invocation surface) outweighs the inlining cost.
- **Rules without a common trigger** — extracting them into a skill creates a non-obvious activation surface; the next agent won't know when to invoke the skill, so the rules effectively go unenforced.
- **Rules that must fire on an event the skill never sees** — anything keyed to
  every commit, every PR, every session start. A skill only runs when invoked,
  so such a rule has to live where that event is handled: the governance file,
  or better, the hook or gate that owns the event.
- **Rules that are pure invariants / single-line policy** — "no Cyrillic in committed prose", "no magic numbers", "no `from __future__ import annotations`" — these are already minimal; wrapping them in a skill adds overhead without simplification.

Concrete workflow for an identified cluster:

1. **Audit cluster size and trigger.** Count the rules; identify the common trigger / workflow / decision point. Confirm 3+ rules and a clear shared activation surface.
2. **Propose the extraction** to the user. Show:
   - The N rules to extract (with line numbers in `AGENTS.md`).
   - The proposed skill / agent name + description (the description must clearly name the trigger so the skill auto-routes).
   - The post-extraction `AGENTS.md` replacement (one-line pointer + rationale).
   - A worked example of how a future session would encounter the trigger and route to the skill / agent.
3. **Get explicit user approval** for the decomposition (same gate as step 5 — silent extraction is the same kind of mistake the workflow is preventing).
4. **Apply** if approved: create the skill / agent file (encoding every prohibition / requirement); replace the `AGENTS.md` cluster with the one-line pointer; verify the new diagnostic surface holds (the rule is still testable, now via "is the skill / agent invoked when the trigger fires?" rather than "does the diff violate the inlined bullet?").
5. **Log the decomposition** in the incident record alongside the original rule addition. The entry MUST cite the post-decomposition file paths so a future reader can locate the encoded rules without diffing `AGENTS.md` history.

The shape that makes an extraction worth doing: the destination is reachable
from the trigger without anyone remembering to reach for it, and the pointer
left behind is one line plus a reason, never a second copy of the body.

A docstring extraction has one advantage the others lack — **the rule cannot
drift from what it documents**, because renaming the field forces touching the
rule on the same line. Governance prose sitting several files away goes stale
silently, which is how a rule ends up citing fields that no longer exist.

Decomposition is **complementary** to Step 4b: when the audit finds 3+ overlapping
rules, prefer extraction over consolidation — consolidation merges duplicates in
place, extraction moves the merged surface into a dedicated skill or agent and
leaves a one-line pointer behind. With fewer than 3 rules, or no shared trigger,
fall back to consolidation.
