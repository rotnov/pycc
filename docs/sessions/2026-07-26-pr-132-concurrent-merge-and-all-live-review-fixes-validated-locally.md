# 2026-07-26 — PR #132 concurrent merge and all live review fixes validated locally

**Snapshot evidence:** local task branch `codex/fix-pr132-review-0764` is at
`c461edac12d0f4fc1e1fd3c464f22dc892ef6555`, which already combines review-fix
patch `0f19f225f81ebca5166708cec74b010d2d47336e` with exact default branch
`origin/main@78f5dcc0c3fd7c88fdc87e716e294fb0fc5cdb53`. A staged merge with
`c63de02be35321b4a8b66821fb5cd04774056558` is in progress. A final fetch left
the remote default branch unchanged and showed published
[PR #132](https://github.com/rotnov/pycc/pull/132) at
`5ff10f1ecd619bde410dfbf2ad3997f0d382cfeb`, a merge whose only parents are that
same `c63de02` and `origin/main@78f5dcc`; it contains no unique non-merge commit.
GitHub reports the PR open, non-draft, and blocked on conversations. All required
checks on `5ff10f1` are green. Fourteen review threads are unresolved, eight of
them non-outdated; all eight describe behavior covered by the staged local tree.

**Validated local merge:** functions see completed module bindings; globals
and maybe-bound non-parameter locals carry runtime initialization flags;
parameters remain initialized and reassignable; local allocations dominate
their uses; accepted `bool`→`int` boundaries use the tagged representation; a
`for` uses hidden SSA induction state so empty ranges, post-loop targets,
negative steps, and body reassignment match Python; two-return merges are
terminated; and `None` in an f-string renders as `None` while malformed
`None`-typed non-call interpolation fails explicitly. The newest numeric fixes
promote an out-of-range product of two smallints, implement CPython's adjusted
float divmod algorithm (including signed zero and the `1.0 // 0.1 == 9.0`
case), and route true division through a zero-divisor guard. Multiplication with
an already-promoted bigint operand remains the documented boundary.

**Local evidence:** the exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passed with 15,844/15,844 regions and 11,391/11,391 lines. Workspace tests,
Clippy with `-D warnings`, fresh `cargo doc`, site checks and mutation
self-tests, 220 Python policy tests, Ruby CI-permission and roadmap-evidence
suites, agent policy/assets validation, Codex/Claude alpha-skill evals, both
marketplace checks, and `git diff --check` passed. A final independent pinned
iEvo review found one non-blocking conflict-resolution artifact: imported test
names and comments still described allocation helpers removed by the merged
implementation. Those descriptions now cover the actual module-global and
preclassified function-local storage paths, the focused 119-test codegen suite
passes, and the required follow-up deep review is clean with no findings. The
known iEvo stale-catalog defect remains deduplicated in
upstream [`ievo-ai/skills#459`](https://github.com/ievo-ai/skills/issues/459);
no new confirmed iEvo defect was found.

**Required next steps:** commit the independently reviewed staged `c63de02`
merge, then record `5ff10f1` as an additional merge parent without
replacing the independently reviewed resolution (the remote merge has no
unique non-merge input). Push normally to `feat/v0-1-pr5-codegen-depth`, resolve
only threads verified against the resulting remote head, and request the
user-required exact `@codex review` once for that new head. Merge only after the
new head's required CI is green and no actionable thread remains. Monitor
current open PRs, new merges, current checks, and current review threads; PR
#119/#125 references are historical governance records, not live monitoring
targets.
