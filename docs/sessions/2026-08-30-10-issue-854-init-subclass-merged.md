# Session handoff: issue #854 — `__init_subclass__` guard unification, merged

- Date: 2026-08-30
- Issue: [#854](https://github.com/rotnov/pycc/issues/854) — CLOSED (stateReason COMPLETED)
- PR: [#856](https://github.com/rotnov/pycc/pull/856) — MERGED (squash)
- Merge commit: `53456405c4d88351985fe92845747f464d8216bb` on `main`
- This entry supersedes the pre-merge status recorded in
  [2026-08-30-09-issue-854-init-subclass-own-override.md](2026-08-30-09-issue-854-init-subclass-own-override.md),
  which was written before the PR was opened; that file's own "Next steps"
  are now all complete and are not repeated here except where their outcome
  differs from what that file anticipated.

## Status

Fully complete and merged. No follow-up work remains against #854 itself.

## What happened after the -09 snapshot

- **Concurrent-writer incident (twice).** While finishing this task, two
  other agent instances independently picked up #854 in the same shared
  worktree and pushed their own commit (`61ec032b`, opening PR #856) before
  the coordinating session ("main") could stop them. This produced
  confusing git-state symptoms (index/working-tree/HEAD mismatches, a
  `docs/ROADMAP.md` edit reverting itself between a passing `check-site.sh`
  run and `git add`). Both incidents were escalated to and resolved by
  "main" rather than acted on unilaterally; per AGENTS.md's "one writer per
  worktree" rule, no gate result taken during either overlap window was
  trusted — every gate re-run below was executed after the tree was
  independently reconfirmed single-writer.
- **`docs/ROADMAP.md` llms.txt aggregate-byte-budget overrun.** The #854
  prose landed in `61ec032b` pushed the llms.txt non-optional-document
  aggregate (D-200, 264 KiB budget, issue #207) from ~264,239 bytes to
  270,790 bytes — 454 bytes over. `main` only had ~97 bytes of headroom
  before this change. Fixed by trimming the added wording (commit
  `91075578`) until `./scripts/check-site.sh` passed again; residual margin
  is now only ~11 bytes. Flagged for whoever next touches any of the
  non-optional documents:
  [issue #207 comment](https://github.com/rotnov/pycc/issues/207#issuecomment-5468222832).
- **D-068 pinned reviewer round.** This session's own toolset had no
  dispatch path to `ievo:deep-reviewer` (`Skill({skill:
  "ievo:deep-review"})` refused with `disable-model-invocation`); the
  coordinating session ran it and reported 3 findings, addressed in commit
  `0bf4a765`:
  - Two stale test comments left over from the guard unification: one
    referenced `init_subclass_before_init_in_body_validates_correctly`, a
    test this change deleted; the other named `base_has_init_subclass`, a
    variable D-214 removed outright. Both rewritten to describe the current
    unconditional `find_map` lookup.
  - A real coverage gap: no existing fixture proved the nearest-MRO-ancestor
    search *skips* a hookless-but-introspectable ancestor to reach a
    farther one that does define `__init_subclass__` — every prior
    multi-inheritance fixture's `skip(1)` MRO candidate either already
    defined the hook or ended the search, so a regression narrowing the
    lookup to just `mro.get(1)` would have passed every other test. Added
    `multiple_inheritance_skips_hookless_ancestor_to_reject_farther_side_effecting_one`
    to close it: `class D(M, B)` where `M` defines no
    `__init_subclass__` (must be skipped) and `B`'s is side-effecting (must
    be reached and rejected on).

## Final gates (personally observed, head `0bf4a765`)

- `cargo build --workspace`: clean.
- `cargo test -p pycc_hir --lib`: 737 passed, 0 failed.
- `cargo test --test issue_435_isinstance_issubclass`: 33 passed, 0 failed.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0 (only the
  pre-existing, unrelated escaped-newline warnings in
  `tests/slice1_codegen_depth.rs`).
- `./scripts/check-site.sh`: "Website checks passed."
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`:
  **100.00% lines / 100.00% regions workspace-wide**, 1466 tests passed;
  `crates/pycc_hir/src/class.rs` 3913/3913 regions, 3060/3060 lines;
  `crates/pycc_hir/src/class/mro.rs` 266/266 regions, 201/201 lines.
- Final `gh api graphql` mergeability check before merge: `mergeable:
  MERGEABLE`, `mergeStateStatus: CLEAN`, `reviewThreads.totalCount: 0`, all
  19 status contexts `SUCCESS`/expected-`SKIPPED` (including `ci-gate`),
  `closingIssuesReferences.totalCount: 1` (`{854}`).

## Cleanup performed

- Remote branch `claude/issue-854-init-subclass` deleted after merge
  (`gh pr merge --delete-branch=false` had left it; deleted explicitly with
  `git push origin --delete claude/issue-854-init-subclass`).
- Worktree at `/Users/denis/projects/pycc-worktrees/issue-854-init-subclass`
  removed (confirmed clean via `git status --short` first).

## Next steps for a resuming session

None specific to #854. The only open follow-up is the llms.txt
aggregate-byte-budget margin noted in issue #207 — not blocking, but worth
picking up before the next non-optional-document edit trips the gate again.
