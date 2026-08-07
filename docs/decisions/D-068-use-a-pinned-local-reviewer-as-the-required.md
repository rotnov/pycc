---
id: D-068
title: "Use a pinned local reviewer as the required review loop"
status: superseded
---

## D-068: Use a pinned local reviewer as the required review loop

- Status: superseded by D-155 (the exact-commit pin this entry's "verify its immutable artifact digests" decision line establishes was never actually enforced on a real, non-isolated machine; D-155 replaces it with structural verification)
- Context: asynchronous GitHub `@codex review` requests can be delayed or unavailable across pull requests, so they cannot reliably serve as a merge gate. The repository has one human maintainer, whose self-approval does not satisfy GitHub approving-review requirements. The project still needs an independent, broad correctness, contract, security, test, and documentation pass before significant work is completed or merged.
- Decision: select only from the repository's explicitly security-reviewed reviewer dependencies in `docs/AGENT_TOOLING.md`, prefer the eligible read-only reviewer with the broadest checklist, verify its immutable artifact digests, and launch it directly in a fresh independent local context. Review staged or working changes before commit and the full merge-base-to-`HEAD` range for a clean pull-request branch. Fix actionable findings and rerun when the diff changes materially. GitHub review comments are optional and are requested only when the user explicitly asks for them.
- Alternatives: keep `@codex review` as a required gate (rejected because asynchronous service availability would block unrelated pull requests); install or select an arbitrary global or marketplace reviewer (rejected because mutable, unreviewed instructions are a supply-chain risk); require an approving GitHub review with one maintainer (rejected because author self-approval cannot satisfy the rule and deadlocks ordinary work); rely only on tests and CI (rejected because deterministic checks do not replace cross-contract review).
- Consequences: local review no longer depends on GitHub comment turnaround and remains reproducible across Codex and Claude Code through the same pinned artifacts and contract. Reviewer pin updates require the documented security review and marketplace validation. If a client cannot bind the exact pinned reviewer, the local review is reported unavailable rather than silently weakened; branch protection, required CI, 100% coverage, specifications, resolved conversations, and future independent human review remain separate gates.

