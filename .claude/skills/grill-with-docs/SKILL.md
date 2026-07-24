---
name: grill-with-docs
description: A relentless interview to sharpen a plan or design, which also creates docs (ADR's and glossary) as we go.
disable-model-invocation: true
---

<!-- ievo:start -->
**Before applying the instructions below**, read `.ievo/evolution/skills/grill-with-docs.md`
if it exists and apply all rules from its sections.
<!-- ievo:end -->

# Grill with docs

Sharpen a plan or design through a relentless but constructive interview, while
keeping the repository's durable documentation current.

## Workflow

1. Read the relevant repository documentation, `CONTEXT.md` when present, the
   canonical project decision log at `docs/DECISIONS.md`, and any supporting records
   under `docs/adr/`.
2. State the decision currently being tested, its known constraints, and the largest
   unresolved risk.
3. Ask one focused question at a time. Challenge assumptions, ambiguous terminology,
   hidden dependencies, failure modes, rollback, observability, and alternatives.
   Prefer concrete scenarios over abstract preference questions.
4. After each answer, summarize what became decided and what remains uncertain. Do
   not treat an untested assumption as a decision.
5. Maintain the domain glossary directly in repository-root `CONTEXT.md`. Create it
   lazily when a new durable term, relationship, or invariant first needs recording.
6. When a load-bearing choice becomes stable and is project-wide or difficult to
   reverse, record it as the next `D-NNN` entry in the canonical
   `docs/DECISIONS.md` log. Use the
   [ADR creation workflow](../create-architectural-decision-record/SKILL.md) only
   when a longer supporting analysis is useful, and link that ADR from the canonical
   decision entry. The ADR supplements the decision log; it never replaces it.
7. Finish with the accepted decisions, rejected alternatives and reasons, remaining
   risks, and the next mechanically verifiable step.

Do not create an ADR for a temporary scheduling preference or an easily reversible
implementation detail.
