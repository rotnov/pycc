# 2026-07-26 — PR #51 pre-commit hook awaiting final CI and merge

**Snapshot evidence:** the checked-out `codex/pre-commit-hook` branch was at
commit `171eceb` with a clean tree before integrating refreshed
`origin/main@841048ec37e20d85a5a0406778f9ec8b66224b04`. The integration was in
progress with its documentation conflicts resolved but not yet committed at
this snapshot. [PR #51](https://github.com/rotnov/pycc/pull/51) is open and no
longer a draft.

**Overall status:** the pull request publishes `pycc-check` from the main
repository as a serial, read-only `language: rust` pre-commit hook; extends
`pycc check` to aggregate diagnostics across native input paths and supported
source encodings; and replaces required asynchronous GitHub review comments
with the immutable pinned local-review loop. D-067 and D-068 record the two
project-wide choices after confirming PR #132's reconciled D-057…D-061 and
D-070…D-073 allocations.

**Validation already observed:** the Rust workspace tests, clippy, generated
API documentation, agent-policy and marketplace checks, roadmap checks, and
100% line/region coverage passed before the latest default-branch integration.
An isolated `pre-commit try-repo` install selected exact revision `10a0502`
and passed `pycc check`; the final merged revision still needs the same check,
the pinned full-range local review, required pull-request CI, and normal
protected-branch merge.

**Where to resume:** finish and review the `841048e` merge, rerun affected
checks, push `codex/pre-commit-hook`, wait for every required PR #51 check and
conversation to clear, then merge normally and verify the post-merge
`main-history-audit`. Do not request `@codex review`; D-068 makes that external
service optional rather than a required gate.
