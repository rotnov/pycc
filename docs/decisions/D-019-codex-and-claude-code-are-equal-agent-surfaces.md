---
id: D-019
title: "Codex and Claude Code are equal agent surfaces"
status: accepted
---

## D-019: Codex and Claude Code are equal agent surfaces

- Status: accepted
- Context: pycc development uses both OpenAI Codex and Claude Code. Letting repository-owned automation exist on only one surface would make the effective development contract depend on which client happened to open a task, while copying complete instruction files per client would let them drift.
- Decision: `AGENTS.md` is the canonical shared repository instruction source and `CLAUDE.md` imports it. Every new or changed repository-owned agent or skill ships discoverable entrypoints and equivalent behavior, safety gates, inputs, and outputs for both Codex and Claude Code in the same pull request. Platform-specific adapters may differ, but they remain thin and share the underlying implementation where practical.
- Alternatives: choose one agent platform (rejected because both are active development surfaces); duplicate all rules and implementations independently (rejected because parity would be unverifiable and drift-prone); permit silent single-platform gaps (rejected because task behavior would become client-dependent).
- Consequences: agent/skill changes are incomplete until discovery plus primary success and failure paths work on both surfaces. A platform API gap needs a safe documented fallback, not an implicit reduction in support.

