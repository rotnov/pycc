# Session handoff: issue #416 implementation plan drafted, reviewed, and published

**Checkpoint reason:** end of a `superpowers:brainstorming` → `writing-plans` →
`issue-to-plan` chain, immediately before handing off to a fresh session per
D-127's context-bounding guidance. Nothing left in flight for this specific
task.

## Baseline at this checkpoint

- `origin/main` tip: `eaf99389b9bafa3ed6ff7fa29dac3326aa12e0ba` (re-fetched and
  re-verified immediately before writing this entry; matches the commit the
  published plan itself cites as its baseline — no drift since drafting).
- Working tree: clean, `main...origin/main`, nothing uncommitted.

## What happened this session

The nbody CI-noise reduction work (`docs/superpowers/specs/2026-08-08-nbody-*`,
merged via #415 into the baseline above) had already been decomposed into
issue #414 (parent, P2, stays open for the non-`ubuntu-latest`/x86_64 legs)
and issue #416 ("Part 1 of #414", the `native-build-test`
`ubuntu-latest`/x86_64 leg specifically). This session:

1. Drafted an implementation plan for #416 via `writing-plans`/`issue-to-plan`
   (baseline, corrections to the issue's and spec's own premises, phased work
   items, gates, risks, explicit out-of-scope).
2. Ran it through an adversarial review loop (5 total rounds across this and
   a prior session) that surfaced three real findings — a bundled spec §4.5
   check that is actually two independent checks, a missing `ir_ratio`
   raw-fd-bypass application, and an under-scoped CI-workflow edit — all
   folded into the plan's numbered corrections and work items.
3. Rewrote five spots in the draft that referenced "the independent review"
   as if the reader had access to this conversation's own multi-agent
   pipeline; a reader of the public GitHub issue has no such access, so each
   was rewritten as a standalone statement carrying the same technical
   content.
4. Showed the exact final comment body to the user in chat, obtained
   explicit approval (`AskUserQuestion`: "Да, опубликовать как есть"), and
   published it: [issue #416, comment
   5229068376](https://github.com/rotnov/pycc/issues/416#issuecomment-5229068376).

This satisfies `issue-to-plan`'s Non-negotiable #3 (per-payload publish
approval) and completes that skill's entire workflow for #416. No source
code was touched this session — the only repository-visible effect is the
one GitHub issue comment above.

## Current state of related issues/PRs (re-verified at this checkpoint)

- **Issue #416** (open): plan published, Phase 1 (raw-fd-bypass retrieval
  mechanism in `tests/nbody_bench.rs`) not started. This is the only
  near-term-mergeable phase of the plan — Phase 4 is a calendar-bound
  observation window (weeks), not a single PR.
- **Issue #414** (open, parent): narrowed to the non-`ubuntu-latest`/x86_64
  `native-build-test` legs and `build-test-coverage`'s macOS leg of Gate 2,
  per the comment that split off #416. It has also accumulated two further
  corroborating flakiness occurrences (PRs #412 and #413) from work
  concurrent with this session — read those comments before treating #414's
  own scope or evidence as settled; they were not evaluated as part of this
  session's own plan-authoring work.
- **PR #391** (`docs/session-log-2026-08-07-01`, open, mergeable): a prior,
  unrelated docs-only session-log checkpoint PR, still unmerged as of this
  writing and now two days stale relative to this entry. Not touched this
  session; worth checking on the next preflight since D-078 monitoring
  applies to it as a task-active-adjacent open PR.

## Where a fresh session should resume

`issue-to-plan`'s Non-negotiable #4 explicitly withholds repository-mutation
authority beyond the one published comment — this session could not and did
not start implementing #416. The natural next unit of autonomous work is
Phase 1 of the published plan (the raw-fd-bypass retrieval mechanism in
`tests/nbody_bench.rs`) — **but do not invoke a plain `issue-implement #416`
and let it run to its ordinary conclusion.** That skill's default path opens
one pull request carrying `Fixes #416` and treats the issue as closed on
merge; #416's own plan is explicitly phased (Phase 1 mergeable now, Phases
2-5 spanning a Cachegrind spike, Valgrind CI enablement, a multi-week
observation window, and a final decision), so a Phase-1-only PR that closes
#416 would be premature — this is exactly the defect an automated Codex
review caught on this session-log entry's first draft (see PR #417). Have
the implementing session tag Phase 1's PR to reference #416 without closing
it (e.g. "Progresses #416", not `Fixes #416`) and leave #416 open until the
phase that actually satisfies the plan's Phase 5 decision closes it —
mirroring, by extension, `issue-implement`'s own existing rule that
"`Fixes #N` goes only on the composed sequence's own final PR, never on an
intermediate one," today written for its D-080/D-103 stage-then-activate
pairs specifically but equally applicable to any multi-PR plan this issue
produces. Standard D-021 preflight applies as always: re-fetch, re-resolve
the default branch, re-check `docs/ROADMAP.md`/milestone issue list, and
re-read this plan's own baseline section before trusting it verbatim if any
meaningful time has passed.

Separately, PR #391 is stale enough to be worth a status check (not a fix)
on the next preflight, and issue #414's two new corroborating comments are
worth reading before any future session treats its scope as settled.
