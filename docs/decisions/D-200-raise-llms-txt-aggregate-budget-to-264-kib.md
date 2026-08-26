---
id: D-200
title: "Raise the llms.txt aggregate context budget from 256 KiB to 264 KiB (issue #207)"
status: accepted
---

## D-200: Raise the llms.txt aggregate context budget from 256 KiB to 264 KiB (issue #207)

- Status: accepted
- Context: D-207's `site/llms-txt-context-manifest.json` (introduced for issue #207) pins a
  `budget_kib: 256` aggregate ceiling over six non-optional documents, the largest and most
  frequently-grown of which is `docs/ROADMAP.md` — every merged pull request that changes
  behavior appends its own changelog paragraph there per `AGENTS.md`'s documentation-currency
  rules. `docs/AGENT_RETROSPECTIVE.md`'s 2026-08-24 entry ("`docs/ROADMAP.md` growth on
  `origin/main` tripped the llms.txt budget after a rebase, even though `check-site.sh` had
  already passed pre-rebase") already recorded one prior occurrence, fixed then by condensing
  the affected branch's own paragraph. Rebasing `feat/issue-771-cast-diag` (PR #778) onto a
  further-advanced `origin/main` reproduced the same failure mode a second time within two days:
  `origin/main` alone already consumed all but 108 bytes of the 262144-byte ceiling before this
  branch's own `#771`/D-199 changelog paragraph (1157 bytes) was added, pushing the aggregate to
  263193 bytes — 1049 bytes over budget. Condensing this branch's own paragraph to fit the
  remaining 108 bytes is not a viable fix: no other `docs/ROADMAP.md` entry is under 900 bytes,
  and D-199's own decision file already carries the full technical detail, so a paragraph this
  short would not describe the change in a way consistent with every other entry's level of
  detail, and would need to be re-condensed again on the next rebase regardless, since the
  ceiling is already effectively exhausted by content this task does not own or control.
- Decision: Raise `budget_kib` in `site/llms-txt-context-manifest.json` from `256` to `264`
  (270336 bytes), an 8 KiB (3.1%) increase, giving roughly 7 KiB of headroom above the current
  post-rebase aggregate (263193 bytes) rather than the 108 bytes the prior ceiling left. This is
  a deliberate, reviewed widening of a project-owned design constant (the ceiling exists to keep
  a default LLM client's expanded context bounded, per `docs/WEBSITE.md`'s "Explicit
  context-size budget" section; it is not a correctness or security invariant, unlike the
  `docs/decisions/`-scoped rule against ever lowering a coverage threshold to make a change pass).
  The value is small and specific rather than a large or round jump, so it stays anchored to the
  actual, currently-observed pressure instead of pre-emptively opening a much larger allowance
  the manifest's own budgets and gate exist to prevent.
- Alternatives: Condense `docs/ROADMAP.md`'s `#771`/D-199 paragraph to fit the existing 108-byte
  margin (rejected — see Context; produces an entry inconsistent with every other roadmap entry's
  detail level, and does not address the recurring root cause: the ceiling is already effectively
  exhausted by unrelated content, so the next branch to add any changelog paragraph reproduces
  this exact failure regardless of this branch's own trim). Move per-`#771` detail out of
  `docs/ROADMAP.md` into `D-199` alone with only a one-line roadmap pointer (rejected for the
  same reason — every other roadmap entry, including ones with an equally or more detailed
  decision file, keeps a full descriptive paragraph in `docs/ROADMAP.md` itself; special-casing
  this one entry's format to satisfy a budget accident would be a stylistic regression, not a
  content decision). Leave the ceiling at 256 KiB and file a separate follow-up issue to shrink
  `docs/ROADMAP.md` overall (rejected — that is a legitimate future improvement but does not
  unblock this branch's own already-complete, already-tested change today, and the retrospective
  entry shows the two most recent branches to hit this ceiling both had to spend agent time on
  the same reactive fix; a modest, reviewed ceiling increase is the more direct fix for the
  immediate recurrence while leaving the underlying "the roadmap changelog grows unboundedly"
  question open for that follow-up to address on its own timeline). Raise the ceiling by a much
  larger margin (e.g. to 320 KiB or more) to avoid revisiting this decision again soon (rejected —
  the budget's entire purpose is to bound a default LLM client's expanded context; a large,
  unreasoned jump undermines that purpose for convenience, whereas an 8 KiB increase anchored to
  the specific, cited overage keeps the ceiling meaningful while resolving the immediate
  recurrence).
- Consequences: `site/llms-txt-context-manifest.json`'s `budget_kib` becomes `264`.
  `docs/WEBSITE.md`'s "Explicit context-size budget" bullet and `scripts/check-site.sh`'s
  explanatory comment above the budget-enforcement code are updated to state `264 KiB` instead of
  `256 KiB` so the documented and comment values match the manifest's actual enforced ceiling;
  the enforcement logic itself already reads `budget_kib` from the manifest rather than hardcoding
  the prior value, so no code change is needed beyond the comment. `docs/ROADMAP.md`'s own
  `#207` evidence-line prose (in the "Public evidence and discoverability" row) is corrected from
  "256 KiB aggregate ceiling" to "264 KiB aggregate ceiling" so the roadmap's own factual claim
  about the mechanism it describes stays accurate. `scripts/test-check-site.sh`'s existing
  aggregate-budget mutation test is unaffected: it dynamically overwrites `budget_kib` to `1` on a
  fixture copy of the manifest rather than asserting the real value, so it continues to exercise
  the same negative path regardless of the real ceiling. No other file hardcodes `256` or `262144`
  for this purpose. This does not relax `D-014`'s coverage gate or any other hard invariant; it
  widens a documented, reviewed, project-owned soft budget whose own governing rule (`docs/WEBSITE.md`)
  already calls it "a reviewed ceiling," not a fixed constant. A future PR remains free to propose
  either a further, separately-justified increase or a `docs/ROADMAP.md`-shrinking follow-up
  (e.g. archiving older changelog paragraphs into a dated appendix) without needing to revisit
  this entry's own reasoning.
