---
id: D-218
title: "Raise the llms.txt aggregate context budget from 264 KiB to 272 KiB (issue #207)"
status: accepted
---

## D-218: Raise the llms.txt aggregate context budget from 264 KiB to 272 KiB (issue #207)

- Status: accepted
- Context: [D-200](./D-200-raise-llms-txt-aggregate-budget-to-264-kib.md) raised
  `site/llms-txt-context-manifest.json`'s `budget_kib` from `256` to `264` on 2026-08-24 for
  exactly the failure mode that recurred here: `docs/ROADMAP.md`, the largest of the six
  non-optional documents `scripts/check-site.sh` expands for `site/llms.txt`, grows with every
  behavior-changing merge under `AGENTS.md`'s documentation-currency rules, so the aggregate
  ceiling is consumed by content no single branch owns. Between D-200 and this entry the
  session logs under `docs/sessions/` record the ceiling being hit or grazed on at least eight
  further tasks (#796, #800, #803, #604, #676, #707, #834, #854 and the v0.3 milestone
  completion), each resolved by condensing that task's own paragraph into whatever headroom
  `origin/main` happened to leave — 90, 30, 6 and 4 bytes at various points. The umbrella
  website issue #802 already carries a checklist item naming the choice this entry makes
  ("either raise `budget_kib` — a policy call — or shrink the budgeted documents").
  Rebasing `feat/issue-864-multi-diag-part1` (PR #871, issue #866) onto
  `origin/main` at `751f10c7` reproduced the mode a third time in the D-200 form: `main` alone
  expands to 270327 bytes against the 270336-byte ceiling (9 bytes of headroom), and this
  branch's only non-optional change is a 113-byte clause in `docs/ROADMAP.md`'s "Compiler
  pipeline" row stating that `pycc check` now reports every parser diagnostic — pushing the
  aggregate to 270440 bytes, 104 over. Condensing a 113-byte clause into 9 bytes is not
  possible, and trimming unrelated rows to make room is the same reactive, per-branch fix the
  D-200 alternatives already rejected.
- Decision: Raise `budget_kib` from `264` to `272` (278528 bytes), the same 8 KiB (3.0%) step
  D-200 took, leaving roughly 8 KiB of headroom above the post-rebase aggregate. The step size
  is deliberately identical to D-200's rather than larger: the ceiling exists to bound a default
  LLM client's expanded context (`docs/WEBSITE.md`, "Explicit context-size budget"), it is a
  reviewed soft budget rather than a correctness invariant, and an increase anchored to the
  cited overage keeps it meaningful while resolving the immediate recurrence. The same pull
  request adds `sh scripts/check-site.sh` to `issue-implement`'s step 4 local gate list for any
  diff touching a manifest-listed document, so the next recurrence is found on the rebased tree
  locally rather than by the Pages workflow after the pull request opens.
- Alternatives: Condense this branch's clause (rejected — 9 bytes of headroom cannot hold any
  statement of the change, and the clause is already the shortest form that names what changed
  and which passes still report their first diagnostic). Trim unrelated `docs/ROADMAP.md` rows
  by ~104 bytes (rejected — every prior task that did this left the next task the same problem;
  the D-200 context and the eight later session logs are the evidence that per-branch trimming
  does not converge). Shrink `docs/ROADMAP.md` structurally, e.g. by archiving older changelog
  paragraphs into a dated appendix outside the non-optional set (rejected here, kept open —
  that is the durable fix and remains the #802 checklist item's second branch, but it is a
  website-scope change with its own review surface, not something to bundle into a compiler
  diagnostics pull request; D-200 reached the same conclusion). A larger jump (e.g. 320 KiB)
  (rejected for D-200's reason — it would stop the recurrence for longer only by making the
  budget stop meaning anything).
- Consequences: `site/llms-txt-context-manifest.json`'s `budget_kib` becomes `272`.
  `docs/WEBSITE.md`'s "Explicit context-size budget" bullet, `scripts/check-site.sh`'s
  explanatory comment above the budget enforcement, and `docs/ROADMAP.md`'s "#207" evidence
  prose (which cited "264 KiB ... (D-200)") are updated to `272 KiB` so every documented value
  matches the enforced one; the enforcement code reads `budget_kib` from the manifest and
  needs no change, and `scripts/test-check-site.sh`'s aggregate mutation test overrides
  `budget_kib` on a fixture copy, so it is unaffected. `.claude/skills/issue-implement/SKILL.md`
  step 4 gains the `check-site.sh` gate line described above. This does not touch any per-resource
  budget, D-014's coverage gate, or any other hard invariant. Issue #802's checklist item stays
  open for its structural-shrink branch; a future entry may supersede this one either by another
  cited-overage increase or by moving the changelog out of the non-optional set.
