# Auditing existing rules: ambiguity, collisions, duplicates, recurrence

### 4a. Audit AGENTS.md for Ambiguity, Collisions & Duplicates

Before writing the new rule, **scan the existing AGENTS.md for the patterns that allowed the just-observed failure to slip past existing rules**. A new rule that lives next to an unaddressed gap will be bypassed the same way next time. For each existing rule that touches the failure's territory, ask:

1. **Definition mismatches.** Does one rule define a key term ("explicit approval", "low-risk", "destructive") more loosely than another rule that requires a stricter version? Which wording would an authority-seeking agent latch onto? Identify the looser one and pin it to the stricter one explicitly.

2. **Compound / batched directives.** Do the rules cover atomic actions ("commit", "delete file") but stay silent on combined ones ("commit and push", "ship it", "delete and recreate")? An agent under momentum will naturally combine. Add explicit guidance that a compound directive does NOT collapse per-step ceremonies.

3. **Pre-emption / out-of-order user directives.** Do the rules prescribe a sequence (propose → wait → execute) that breaks if the user pre-empts step 1? Add a sub-clause: "if the user pre-empts, pause and run the missed step retroactively before proceeding."

4. **Mode tension.** Does an external mode (auto-mode, plan-mode, sandbox-mode) carry implicit permissions that conflict with the rule? Promote the mode-clarification to the section's top bullet so it can't be missed by a sleepy agent.

5. **Reversibility framing as escape hatch.** If the failed action was reversible (local commit, file edit, branch creation), did the agent self-justify it as "low-risk because reversible"? Add language that pins required-ceremony-actions to the ceremony regardless of reversibility.

6. **Section locality.** Is the rule that *would* have prevented the failure buried inside a long bullet, far from where the agent's attention falls? Move it to the section's top-of-list, or duplicate a one-line summary at the natural decision point.

7. **Duplicate / overlapping coverage.** Scan existing rules for ones that already describe the same behaviour from a different angle. Two rules saying "the same thing" with slightly different wording are worse than one — they (a) bloat the context window for every future session, (b) drift apart over time as one gets updated and the other doesn't, and (c) let an agent satisfy the looser one while ignoring the stricter. Flag every such overlap.

Document each ambiguity / collision / duplicate found in a short bulleted list *before* drafting the new rule. The new rule should:

- **Close every identified gap explicitly**, not just add more general guidance. A new bullet that re-states an existing principle without closing the specific loophole is wasted text.
- **Cross-reference the related rules** by name, so the agent cannot follow one
  while ignoring its companion.
- **Be testable**: the new wording must let a reviewer say "the agent did X; per the new rule, that's a violation". Vague exhortations ("be careful with commits") fail this test.

If the audit shows the existing rules are *correct* but the agent ignored them through carelessness rather than ambiguity, do NOT add yet another rule — the failure is execution, not specification, and piling on rules will just add noise. Instead, log the lapse in the incident journal so the pattern is visible across sessions.

1. **Recurrence check — run this before writing anything.** List the journal
   topics: `ls .harden/incidents/`. Does a topic for this failure already exist?

   A hit means this is **not a new lesson — it is the failure of the artefact
   chosen last time.** Then:

   - **do not reword the previous rule.** "Reinforcing" the same text was tried
     in the upstream project on a `from __future__ import annotations` ban —
     written, then rewritten five months later, still not held;
   - **escalate one rung** on the detectability ladder:
     `doc` → `rule` → `review-check` → `precommit` → `hook`;
   - link the earlier incident in `related:` and let the folder carry the count.

   Three or more files in one topic folder **disqualify a textual artefact
   outright**. Repetition is the proof that text does not hold here.

   This differs from 4a.7 above: that one finds two wordings of the same rule
   (a duplicate). This one finds one wording that failed (a recurrence).

### 4b. Decide: Add, Replace, or Consolidate?

Based on the audit from step 4a, choose the **smallest** intervention that closes the failure pattern without leaving residue:

**Add a new rule** when:

- No existing rule covers the failure's territory.
- The new lesson is orthogonal to existing rules (closes a different category of failure).
- Step 4a found gaps but no overlapping rules.

**Replace an existing rule** when:

- An existing rule covers roughly the same territory but is too loose, ambiguous, or buried to have prevented the failure.
- Strengthening the existing wording in place is clearer than adding a new bullet that says "see also rule X".
- The old rule's wording, taken alone, would let a future agent rationalise the same failure again.

**Consolidate two or more existing rules** when:

- Step 4a #7 (duplicate detection) found rules that describe the same behaviour from different angles.
- The rules' edges overlap enough that an agent can satisfy one while ignoring the other.
- The consolidated form is strictly clearer than the sum of the parts — covers the same ground in fewer lines, with no gap introduced by the merge.

When consolidating, the merged rule MUST:

- Cite which existing rules it replaces (in the incident record, not the rule body).
- Preserve every concrete prohibition / requirement of the originals (no silent loosening — if rule A forbade X and rule B required Y, the merged rule forbids X AND requires Y).
- Land in the section that best matches its scope, even if that means moving content across §-boundaries.

The default reflex of "add a new bullet" is the wrong reflex when 4a already found overlapping coverage — that path produces a third bullet that overlaps with the first two, and the next failure will slip through the joint between any two of them. Always favour replace-or-consolidate when overlap exists; add-new only when the audit confirms the territory is genuinely uncovered.
