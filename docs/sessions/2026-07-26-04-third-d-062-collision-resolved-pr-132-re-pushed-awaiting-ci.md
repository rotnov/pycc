# 2026-07-26 — Third D-062 collision resolved; PR #132 re-pushed, awaiting CI

**Snapshot evidence:** direct work on `feat/v0-1-pr5-codegen-depth`,
merging `origin/main` at `841048e` (PR #128, which added this file and
`docs/AGENT_RETROSPECTIVE.md` under D-066) into this branch and pushing
the result as commit `1b68e21` (superseded by a second merge commit
resolving the immediately-following conflict described below). Local
`cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo doc --workspace --no-deps`, and
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
all passed (100.00% lines and regions across every crate) before pushing.

**What changed since the entry below:** the prior entry's "Known
follow-up required before PR-5 merges" predicted a colliding tail between
this branch's D-062 (str-leak correction) and `main`'s new D-062
(fixed-replicate perf-gate stabilization). Resolved by keeping D-057–061
as `main` had already reserved them, ceding D-062 (and `main`'s
subsequently added D-066, this file's own decision) to `main`'s
decisions, and renumbering this branch's remaining four entries — str-leak
correction, the renumbering-record itself, the `print()`-nested-expression
boundary, and the `RelocMode::PIC` fix — to D-070 through D-073, a gap
ahead of `main`'s reach chosen so future `main` advances stop colliding
with this branch's own IDs before it merges. The renumbering-record entry
(now D-071) was also frozen to a single dense table row instead of a full
section, since three collisions made it the highest-churn entry in the
file for no technical content. A second, smaller conflict round
immediately followed (`main` advanced again mid-resolution, touching the
same `ROADMAP.md`/`SPEC.md` table rows this branch had just edited); it
required no further ID changes, only combining both sides' additive text.

**Known follow-up required before PR-5 merges:** re-check
`gh pr view 132 --json mergeable,mergeStateStatus` and `gh pr checks 132`
after this push lands, since `main` has advanced during every prior
verification window on this branch. Re-verify the live ADR tail
immediately before picking any new ID; later IDs are candidates, not
reservations, and this has now happened four times.
