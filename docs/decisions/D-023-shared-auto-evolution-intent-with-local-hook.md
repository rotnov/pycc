---
id: D-023
title: "Shared auto-evolution intent with local hook execution"
status: accepted
---

## D-023: Shared auto-evolution intent with local hook execution

- Status: accepted
- Context: the repository wants correction-driven evolution to survive across contributors, but iEvo generates hook scripts under gitignored `.ievo/hooks/`. Committing shared hook entries that invoke those absent targets made every clean clone emit hook errors (issue #28; upstream `ievo-ai/skills#446`).
- Decision: `.ievo/evo-auto.flag` records shared intent with `signal: corrections-only` and `auto_write_scope: project-wide-only`. Generated scripts and their hook wiring are machine-local: `.ievo/hooks/` and `.claude/settings.local.json` are ignored, while shared `.claude/settings.json` may reference only tracked targets or tracked fail-silent wrappers. Inline interpreter forms such as `sh -c`, `python -c`, and `node --eval` are forbidden in shared hooks because their executable targets cannot be validated statically. Agents validate the tracked-file view in CI.
- Authority and scope: automatic capture may record confirmed user corrections under the configured project-wide scope. It does not broaden authority for external writes, source-code changes, secrets collection, or arbitrary per-agent behavior changes. Candidate lessons outside the auto-write scope remain reviewable candidates.
- Privacy and failure behavior: local hook inputs and generated logs remain local/ignored. Missing local setup must be a silent no-op in shared state, never a failing committed command. Validation failure blocks merge.
- Rollback: run the iEvo auto-disable workflow locally, remove or supersede the shared flag, remove local hook wiring/scripts, and verify the clean tracked view. A future tracked-wrapper design requires a superseding decision and security review.

