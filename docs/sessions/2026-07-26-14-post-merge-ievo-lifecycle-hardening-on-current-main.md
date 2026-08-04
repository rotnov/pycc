# 2026-07-26 — Post-merge iEvo lifecycle hardening on current main

**Authoritative snapshot and priority:** a fresh fetch resolved
`origin/main@2d9c2c4599f9c07b74404d14e0efc361aa4f5c50`, the merge of
[PR #140](https://github.com/rotnov/pycc/pull/140) at source head
`1682cc1aeebfe8f3f1b074c6788113fc654e6b3a`. The PR is merged, issue
[#34](https://github.com/rotnov/pycc/issues/34) is closed, every required check
on that head passed, and all eleven review threads are resolved. The follow-up
branch starts directly from that current main rather than pushing to the closed
PR. Treat the confirmed follow-up as P1 before selecting another issue: an
ambiguous destructive disable could violate the repository's fail-closed
contract, and Windows-only safety branches had no required native execution.
All five open pull requests (#112, #92, #91, #59, and #36) were inspected for
overlap; none owns this lifecycle hardening.

**Follow-up repair:** D-081 strengthens D-077 with complete corrections-only
intent validation before every lifecycle transition; conservative detection of
lexical, case, shell-expansion, glob, PowerShell-expression, and DOS 8.3
managed-path aliases; symlink/reparse/mount/device rejection; regular-file and
complete vendor snapshots; per-entry ancestry/identity revalidation; and
crash-released per-worktree advisory locking. Disable never uses broad
`rmtree`, preserves unrelated configuration, and documents the remaining
non-atomic external-writer limitation. The pinned independent review's latest
two warnings are addressed: `disable` now validates missing/conflicting intent
before mutation, and a Windows-only Rust integration test runs the lifecycle
and policy-parser suites inside the existing required native Windows matrix
without changing D-062's byte-pinned workflow.

**Review and evidence:** both pinned reviewer artifacts still match their
recorded SHA-256 digests. The staged follow-up tree based on
`origin/main@2d9c2c4599f9c07b74404d14e0efc361aa4f5c50` passes 268 Python
discovery tests (four platform-only tests skipped on macOS), agent-policy and
agent-assets validation, ruff format/check, workflow permission policy, 99
roadmap-policy tests with 432 assertions, roadmap evidence validation,
workspace build, 581 Rust tests, clippy with warnings denied, rustdoc with
warnings denied, and `git diff --check`. The pinned independent staged review
completed all eleven checklist points: implementation, tests, error paths,
security, and normative contracts were clean; its only warning was this
snapshot's former attribution of follow-up evidence to main and its stale
instruction to rerun the completed review, corrected in the containing change.
During that review, iEvo `deep-review --working` was confirmed to omit untracked
files even in upstream 0.70.1; duplicate search found no report, so
[ievo-ai/skills#483](https://github.com/ievo-ai/skills/issues/483) records the
public bug and the local D-068 instructions now require every intended new file
to enter the reviewed diff.

**Required next steps:** commit the corrected staged snapshot, refresh current
main, repeat a committed merge-base range review, push the new follow-up branch,
and open a draft PR that links #34 and upstream #483. Treat the new exact-head
CI and Windows lifecycle execution as fresh evidence; do not reuse #140's green
checks. Merge only through protected main after required checks pass and no
actionable thread remains.
