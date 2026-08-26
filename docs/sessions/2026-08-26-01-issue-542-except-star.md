# Session handoff: issue #542 (PEP 654 `except*`/`ExceptionGroup`) — PR #794 review and merge

## Status

**Correction (a later session on the same branch):** an earlier draft of
this file, committed to this same branch before this correction, stated
"Reviewed and merged" and cited a `closingIssuesReferences` confirmation.
That was premature -- PR [#794](https://github.com/rotnov/pycc/pull/794)
was still `state: OPEN`, `mergedAt: null`, `mergeStateStatus: BLOCKED` when
this correction was made, re-verified directly against the GitHub API
rather than trusted from the prior draft. This file is being kept accurate
in place (not superseded by a second same-day file) because it describes
this same still-open PR/issue, not a separate completed unit of work; see
the "Second finding pass" section below for what changed after the
original draft.

This session's task was specifically to review and merge #794, which had
already been implemented (Part 3 of #382) by a prior session/branch. All
implementation work below (the original two fixes plus the second pass)
is corrective work found during review, layered on top of that existing
implementation.

## What this session found and fixed

Ran the D-068 pinned `ievo:deep-reviewer` against the full
`merge-base(origin/main, HEAD)..HEAD` diff. Two warnings and one note came
back; all three were addressed before merge:

1. **`TryStar`'s constraint join-site omitted the `opaque_bindings` chain**
   (`crates/pycc_types/src/constraints.rs`). All six pre-existing join-site
   arms (`If`, `While`, `ForRange`, `ForList`, `Match`, `Try`) compute
   `pre_existing` via `env.bindings.keys().chain(env.opaque_bindings.iter())`
   per the #771 join-site fix; the new `TryStar` arm this PR added used the
   pre-#771 `env.bindings.keys()` alone. Fixed to match its six siblings, with
   a new regression test
   (`collect_block_constraints_try_star_body_reassigns_pre_existing_opaque_binding`
   in `crates/pycc_types/src/tests.rs`) mirroring the existing `If`-side
   pinning test for the same bug class.
2. **Nested exception groups type-checked but could not be partitioned
   correctly at runtime** (`crates/pycc_types/src/exception.rs`). D-202
   documents five deliberate PEP 654 simplifications; the reviewer found a
   sixth, undocumented one: `check_exception_group_member_operand` accepted
   any existing-value member whose type is a builtin exception class,
   including `ExceptionGroup`/`BaseExceptionGroup` itself (which is exactly
   what an `except* ... as eg:` binding's type always is). But
   `pycc_rt_exception_group_partition` only matches a member by its own
   top-level `type_tag`, never recursing into a member that is itself a
   group — unlike CPython's `split()`. Fixed by rejecting an
   `ExceptionGroup`/`BaseExceptionGroup`-typed member with `T0021`, added a
   regression test
   (`an_exception_group_valued_binding_is_not_a_valid_group_member` in
   `crates/pycc_types/src/exception/except_star_tests.rs`), and extended
   D-202 to record this as its sixth simplification (title, context, the new
   numbered decision, a new alternatives entry, and the consequences
   section's "five" -> "six" count), regenerating
   `docs/decisions/README.md` afterward.
3. **No `docs/sessions/` handoff entry existed for this PR** — this file is
   that entry, per D-066/D-130/D-192.

Also verified independently (not just trusted from the PR description):
D-202's internal consistency, `docs/ROADMAP.md`/`docs/PYTHON_STANDARDS.md`'s
honest treatment of PEP 654's conformance status (deliberately left at `☐`
rather than flipped, because the new `pep_0654_except_star_matches_cpython_
3_14_7_byte_for_byte` conformance test is `#[ignore]`d pending a completed
CI run with the CPython oracle on `PATH` — flips happen in a later,
by-hand doc update citing a specific green run ID, per rule 5/6 in
`docs/PYTHON_STANDARDS.md`, never within the introducing PR itself; every
existing precedent row follows this same pattern), and
`tests/fixtures/conformance-breadth-manifest.json`'s updated gap
description for PEP 758's unparenthesized-`except*`-with-comma row (honest
about #542 now existing while still recording the specific unproven gap).
`python3 scripts/generate_decisions_index.py docs/decisions
docs/decisions/README.md --check` and `ruby scripts/check_roadmap_evidence.rb`
(run with `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8`; the ambient locale is
`C`/`POSIX` in this environment, which crashes the script on non-ASCII
roadmap bytes — an environment issue, not a script or PR defect) both pass.

## A mid-review worktree hazard

The task's original working location, an existing `.claude/worktrees/`
checkout already at the PR's head commit, turned out to be actively
shared with another concurrent process: partway through this session's
review (after the first two fixes above were already drafted there), that
worktree's branch moved out from under this session — `git log` showed a
new `Merge remote-tracking branch 'origin/main' into issue-542-except-star`
commit neither this session nor its edits produced, and `origin/
issue-542-except-star`'s `headRefOid` had already advanced to match it.
Per AGENTS.md's "one writer per worktree" rule, this session treated every
local finding as unverified after that point, abandoned the shared
worktree without committing or pushing anything from it, and redid the
review's fixes from scratch in a freshly created isolated worktree
(`git worktree add --detach` from the refreshed `origin/issue-542-except-
star` tip) before building, testing, or committing anything. The PR's own
diff was unaffected by the concurrent merge (still 32 files, +4838/-105
against the refreshed `origin/main`), and CI had already re-run green on
the new head by the time this was noticed.

## Second finding pass (this correction's own session)

After the two fixes above landed on the branch (commit `345dbac6`), this
session independently reviewed PR #794's five *external* review-bot
(`chatgpt-codex-connector`, not the project's own pinned D-068/D-155 gate)
comment threads, which were still unresolved. Verified each against the
actual source rather than trusting the bot's wording:

1. **P1, real bug, fixed.** `emit_try_star`'s per-clause dispatch loop
   (`crates/pycc_codegen/src/exception.rs`) reloads `current_group_slot` at
   the top of every clause's dispatch block and passes it straight into
   `pycc_rt_exception_group_partition`, whose safety contract requires a
   non-null `group` and which unconditionally dereferences it. When an
   earlier clause's tag set (e.g. `except* Exception:`, the universal
   catch-all) matches every remaining member, the runtime's own
   `build_group_or_null` correctly reports an empty remainder as null --
   but nothing stopped the next clause's dispatch from feeding that null
   pointer straight back into the same unconditionally-dereferencing
   runtime call, which is undefined behavior. Fixed by skipping a clause's
   dispatch entirely once the threaded pointer is null, falling through to
   the next clause (or the final reraise) instead. New regression test
   `except_star_broad_first_clause_consuming_the_whole_group_does_not_
   crash_the_next_clause` in `tests/issue_542_except_star.rs`, confirmed to
   reproduce the original panic against the pre-fix code (verified by
   temporarily reverting the fix and re-running the test before committing
   it).
2. **P2, real gap, already fixed by the prior pass on this branch.** The
   bot independently reported the same nested-exception-group issue the
   pinned reviewer's pass above already fixed; no further action needed
   beyond confirming the fix covers it.
3. **P2, real gap, deferred to a new issue.** PEP 654 disallows
   `return`/`break`/`continue` directly inside an `except*` clause body;
   pycc does not currently reject them. A new HIR/type-check validation
   seam, independent of this session's codegen fix, so it is tracked
   separately rather than folded in here.
4. **P2, false positive (an existing, documented, deliberate decision).**
   The bot observed that a bare `raise` inside an `except*` handler body
   abandons later clauses instead of letting them process the remainder --
   correct, but this is exactly D-202's existing fifth simplification (a
   handler-body raise, new or bare re-raise, takes the same direct-to-
   `finally` path `Try`'s own handler bodies already take). D-202 updated
   to state explicitly that a bare re-raise takes the identical path as a
   fresh raise, rather than a separate one, since the bot's framing implied
   otherwise.
5. **P2, real gap, deferred to the same new issue as (3).** `except*
   ExceptionGroup:` / `except* BaseExceptionGroup:` should be rejected
   (CPython raises `TypeError`), but pycc currently accepts them since both
   classes are ordinary `BUILTIN_EXCEPTION_CLASSES` entries.

Findings 3 and 5 are filed as
[#795](https://github.com/rotnov/pycc/issues/795) (milestone v0.3): both
are new type-check validation surfaces distinct from this PR's codegen
null-guard fix, matching the "independently mergeable seam" bar for
deferring rather than scope-creeping this PR.

This second pass also hit the same "shared worktree" hazard the first pass
documented above, from the opposite direction: `origin/issue-542-except-
star` had already advanced (to `345dbac6`) by the time this session
fetched, past the commit this session's own uncommitted local edits were
based on. Resolved by diffing the two independently-produced fixes
byte-for-byte (found near-identical, confirming both were correct
independent discoveries of the same real bug), then `git rebase
origin/issue-542-except-star` to layer this session's own commit
(the P1 null-guard fix plus its own copies of the two prior fixes) cleanly
on top, resolving three trivial comment-wording conflicts and one D-202
attribution conflict (resolved by crediting both the pinned reviewer and
the external bot, since both independently found the same issue) by hand,
then re-verifying `cargo build --workspace` and the affected test files
before pushing as a fast-forward (`345dbac6..83bbb0e9`).

## Gates

- `cargo build --workspace`, `cargo test --workspace`, `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo doc --workspace
  --no-deps` all green against `83bbb0e9`.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: **100.00% lines / 100.00% regions** (45958 regions, 29652 lines, 0
  missed) against `83bbb0e9`.
- CI on `345dbac6` (the pre-P1-fix head) had gone green previously; CI on
  the current head `83bbb0e9` needs a fresh run before merge -- not yet
  observed by this session at the time of this writing.

## Where to resume

Not yet merged. Remaining steps: watch CI go green on `83bbb0e9`, resolve
the five external-bot review threads (reply confirming what was fixed vs.
what was deferred to #795, per the second-pass findings above), confirm
no unresolved review threads remain, merge PR #794 (verify via GraphQL
`closingIssuesReferences` that it closes exactly #542 first), then run
`python3 scripts/check_conformance_breadth.py` against the merged
`origin/main` tip to check v0.3's Accept criteria.

Once the `pep_0654_except_star_matches_cpython_3_14_7_byte_for_byte`
conformance fixture is observed green under a CPython-oracle-enabled CI
run, a follow-up doc-only change should flip the PEP 654 row in
`docs/PYTHON_STANDARDS.md` from `☐` to `◐`/`✅` per that observation, citing
the specific run, mirroring every other conformance-status flip's
precedent.
