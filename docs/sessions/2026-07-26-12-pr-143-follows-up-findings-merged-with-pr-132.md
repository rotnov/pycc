# 2026-07-26 — PR #143 follows up findings merged with PR #132

**Snapshot evidence:** the follow-up branch starts from exact default branch
`origin/main@03c6472362d1d6d2211b7cf4e7bb132ffe86295f`, the merge commit for
[#132](https://github.com/rotnov/pycc/pull/132) at its published head
`d30e6a6c787de39e7e761d44d44cbf3e6cad3353`. The repair was independently
reviewed and committed as `a67ad05` on the source branch, but another process
merged #132 at `d30e6a6` before that push became part of the PR. This branch
cherry-picks the verified repair onto the resulting fresh `main` rather than
pretending the post-merge source-branch update entered the merge commit. The
branch now also integrates exact
`origin/main@c240cbdd0a3d42257d1c9c769260957cfb23ef90`, which adds PR #144's
independent post-merge handoff entry; the conflict resolution preserves both
chronological snapshots. The remote's Unix-only exit-101 repair and regression
test remain, while D-075
supersedes its documented-open-gap approach and D-076 generalizes the exit
mapping to every unsuccessful child on every platform. The exact `@codex
review` for `50e36e8` produced two new actionable P2 threads: a valid `None`-typed parameter reached
`ty_to_basic_type`'s backend panic, and a generated-program abort was converted
to exit 1 instead of CLI_SPEC.md's portable runtime-failure code 101.

**Local repair:** D-075 gives `None`-typed user-function parameters a canonical
LLVM `i8 0` unit carrier while retaining LLVM `void` returns. Parameter name
reads, `return value`, `print(value)`, f-string interpolation, and passing a
`None`-returning call into a `None` parameter now compile and run end to end;
D-072's explicit nested-`print()` boundary and general `None` assignment gap
remain unchanged. D-076 maps every unsuccessful generated child of `pycc run`
to 101 without changing compiler-owned build or invocation failures. The type,
runtime, CLI, roadmap, historical implementation-plan scope note, and ADRs are
updated with the implementation.

**Local evidence:** focused regressions and the complete 123-test codegen,
57-test slice-0 suite active on the local Darwin host, and 30-test slice-1
suite pass. The exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passes with 16,318/16,318 regions and 11,696/11,696 lines. Clippy, fresh Rust
API docs, and roadmap-evidence checks pass. The first independent deep-review
pass found one documentation-inventory blocker: the accepted D-075/D-076
sections were absent from DECISIONS.md's summary table and SPEC.md's ADR map.
Both indexes now include the decisions. The follow-up pass found two stale
direct-call-only `None` descriptions in the historical plan and code comments;
those descriptions now include D-075's parameter-carried paths. The next pass
found and corrected the same stale wording in the runtime API comment plus the
pre-integration slice-0 count in this snapshot. The final independent
deep-review verification is clean across all 11 checklist areas. The repair
was committed, pushed, and opened as follow-up
[#143](https://github.com/rotnov/pycc/pull/143). Its exact `@codex review` at
head `24f1a5b` found that this handoff still listed the completed commit step;
head `adeb557` corrected the stale state, passed the full required CI matrix,
and received a clean Codex re-review with no unresolved thread. Because `main`
then advanced through PR #144 before merge, publishing this integration commit,
requesting one Codex re-review for its new head, and completing its remote CI
are required before merge.

**Monitoring scope correction:** PR #119 and PR #125 are historical governance
evidence only, not live monitoring targets. Monitor PR #143 plus newly opened
PRs and newly merged default-branch commits.
