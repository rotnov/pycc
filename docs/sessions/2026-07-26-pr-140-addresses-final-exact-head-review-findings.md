# 2026-07-26 — PR #140 addresses final exact-head review findings

**Snapshot evidence:** immediately before the containing repair commit, draft
[PR #140](https://github.com/rotnov/pycc/pull/140) was inspected `OPEN`, draft,
and `BLOCKED` at remote head
`d359685939098693f41cc1f66de5a3179c720f6c`. That head merges the previous PR
head `4cd93f1bf10b3f1d4d3020261a834d31527b7114` with exact refreshed default
branch `main@18ef34105a4f57c63e77c76dffa1948b29e32161`. Every exact-head CI job,
including hard coverage and `ci-gate`, passed; two actionable GitHub Codex
threads remained unresolved and are fixed by the containing repair. The change
is not yet merged into `main`, and issue
[#34](https://github.com/rotnov/pycc/issues/34) remains open until it lands. A
resuming agent must inspect the authoritative remote head and state rather than
assume this snapshot has already been published.

**Scope and review state:** D-077 adds one repository helper for exact iEvo
hook relocation, validation/smoke, and clone-local disable across Claude Code
and Codex. The lifecycle preserves unrelated configuration, restores the
project's whole-directory `.ievo/hooks/` ignore policy after upstream
tracked-shim mutations, writes the local destination before removing shared
wiring, removes local hook entries before their targets, and preserves the
tracked `.ievo/evo-auto.flag` project-wide intent. Review fixes make refreshed
shared metadata win over stale local copies, preserve unrelated empty hook
structures, reject unsupported managed-target references before any mutation,
and recursively cover shim, companion, and vendor targets even in future hook
shapes. Pinned local deep-review passes found two remaining fail-closed gaps:
lexical, separator, quoted, and case-only aliases could evade the unknown
managed-reference check, and length-changing Unicode case folding could shift
the located path offset. Original-string path matching, static alias
normalization, and localize/disable before-mutation regressions close both
gaps; the final independent rerun is clean across all 11 checklist areas.
Upstream `ievo-ai/skills#446` and merged PR #455 remain linked; their tracked
dispatcher design does not supersede D-023/D-025's local-execution policy.

**Local evidence:** all 238 discovered Python tests, the agent-policy and
agent-assets validators, ruff format/check, roadmap evidence (99 runs and 432
assertions), `cargo fmt`, workspace build/test, clippy, fresh Rust API
documentation, and `git diff --check` pass on the integrated tree. The
CI-equivalent prerequisite
builds (`pycc_rt` for `x86_64-apple-darwin`, then the workspace) followed by the
exact hard command `cargo llvm-cov --workspace --fail-under-lines 100
--fail-under-regions 100` cover 16,318/16,318 regions and 11,696/11,696 lines.
The earlier exact-head GitHub reviews have no unresolved thread, but they cover
only the superseded `4cd93f1` head and cannot approve this containing commit.

**Remaining task-specific review and merge gates:** the user-requested GitHub
Codex review of `d359685` found that POSIX shell escapes could still hide a
managed target and that this snapshot incorrectly described the external
review as repository-required. The containing repair recognizes both Windows
separators and POSIX escapes, adds a fail-before-mutation regression, and
clarifies that each new head receives one user-requested `@codex review` in
this task without making the asynchronous service a repository merge gate.
The pinned local reviewer remains the required review loop. Address findings
from the completed task-specific GitHub review, keep the PR draft until all
required CI checks including hard 100% coverage are green, then mark it ready,
re-confirm that the branch is current and the head is unchanged, and merge
through branch protection.
