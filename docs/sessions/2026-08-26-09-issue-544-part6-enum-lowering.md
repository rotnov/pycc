# Session handoff: issue #544 Part 6 — enum-lowering seam extracted

## Status

Part 6 of issue #544 (D-185 per-file decomposition tracker for
`crates/pycc_types/src/lib.rs`), implemented against `origin/main` at
`6582c880` (branched point) then rebased-by-merge onto `4906ba58` (PR #813,
Batch C of #782) before merge, delivered as PR #814, merged as `a81359f9`.
This entry lands with that merge (D-192). Issue #544 stays open — `lib.rs`
is 3,550 lines after this extraction, still well above the ~1,000-line
threshold.

## How this issue was selected (autopilot run)

This was one full iteration of the standing "fix all opened issues"
autopilot directive, run via `.claude/skills/issue-select/SKILL.md`.

- **Milestone scope resolved via the GitHub API, not roadmap prose.** The
  v0.3 GitHub milestone is `"state":"closed"` (`gh api
  repos/rotnov/pycc/milestones --paginate -X GET -f state=all`), which is
  stronger evidence than `docs/ROADMAP.md`'s "Update: met" note. v0.4 (the
  next milestone) has zero open issues, so scope contributed no survivor
  and selection fell through to ranking every open issue by
  priority-marker-then-size, per the skill's own step 5 fallback.
- **D-192 non-milestone merge quota (1-in-5), computed with real commands
  by walking the actual first-parent merge history** (`git log
  origin/main --first-parent`, then `gh pr view <n> --json
  closingIssuesReferences` and `gh issue view <n> --json milestone` for
  each merge that actually closed an issue): the four most recent
  slot-filling merges were PR #791→#790 (v0.3), #807→#800 (v0.3),
  #780→#769 (v0.3), #797→#796 (v0.3). Every PR that closed no issue and
  named no umbrella issue (#811, #808, #804, #805, #801, #799) was
  correctly skipped rather than counted as a slot. All four counted slots
  are milestone-assigned, so the quota was **not spent** (0/4
  non-milestone) — a non-milestone P1 pick was permitted this run.
  Re-verified after PR #811 and #808 merged mid-session; the count did not
  change (both were skipped for the same reason).
- **D-192 non-milestone open-issue ceiling**: 72 open issues carry no
  milestone (`gh issue list --state open --json milestone`, filtered),
  over the 20 ceiling — confirming no *new* non-milestone issue could be
  filed this run. This did not block picking up an *existing* D-185
  tracker, which AGENTS.md explicitly sanctions as legitimately selectable
  bounded work.
- **P1 non-milestone pool screened (15 issues + #20)**, each disposed with
  a recorded reason:
  - CI-workflow / D-080 (two-PR stage-then-activate) / D-172 review
    weight, deprioritized: #14, #44, #45, #82, #558.
  - Open-PR (#810, the concurrently-running website-update work)
    collision on `site/`, excluded: #162, #563, #565, #566, #569.
  - Blocked transitively via #631: #20.
  - Large / architecturally consequential, needing D-021 step 10
    decomposition into sub-issues before a clean single-session pickup:
    #53, #259 (Codex/Claude cross-client test infrastructure — verified
    Codex CLI is actually present in this environment via `which
    codex`/`codex --version`, so this is a size/complexity judgment, not
    an environment-capability exclusion).
  - **Viable survivors, all D-185 file-decomposition trackers**: #544
    (`pycc_types/src/lib.rs`, 3,818 lines), #545 (`pycc_codegen/src/lib.rs`,
    8,503 lines), #695 (`pycc_types/src/tests.rs`, 29,514 lines).
    "Smaller wins" tie-break selected **#544**.
- **Advisor round**: consulted before finalizing the v0.3-milestone-closure
  reasoning and again on the specific seam to extract from `lib.rs` (the
  enum-lowering pass, independently confirmed by `monomorphize.rs`'s own
  module doc comment, which had explicitly named and deliberately deferred
  this exact seam back in Part 2).
- No stale issues were closed during the screen (none of the screened
  issues showed evidence of an already-resolved premise). No milestones
  were assigned during triage (none of the touched issues needed one).

## What this PR delivers

- Extracted enum-member attribute typing (`enum_member_attr_type`) and the
  whole PEP 435 enum-loop-unrolling pass (`check_enum_loop_body_module`,
  `check_enum_loop_body_function`, `build_enum_member_table`,
  `unroll_enum_loops`, `unroll_enum_loops_in_stmts`, #379 / PR-19) from
  `crates/pycc_types/src/lib.rs` into a new sibling module
  `crates/pycc_types/src/enum_lower.rs` (306 lines including its module doc
  comment and `use` lines). `lib.rs`: 3,818 → 3,550 lines (`wc -l`).
- Pure structural move, verified by diffing the pre-move block against the
  new module's function bodies with only the `pub(crate) fn` prefix
  normalized away: the only difference is the pre-move section-level doc
  comment being folded into the new module's top-of-file doc comment;
  every line of code and every per-function doc comment is byte-identical.
- Visibility: `enum_member_attr_type`, `check_enum_loop_body_module`,
  `check_enum_loop_body_function`, and `unroll_enum_loops` are `pub(crate)`
  and re-exported at the crate root so every existing call site
  (`expr.rs`, `lib.rs`'s own `check_stmt`/`check_stmt_in_function`/
  `check_and_resolve`) is unchanged. `build_enum_member_table` and
  `unroll_enum_loops_in_stmts` stay private, used only inside the new
  module, which reaches `lib.rs`'s private `check_assignment`/
  `join_loop_body` as an ordinary descendant of the crate root (the same
  pattern `class::binding` uses for `class`'s private `check_call_args`).
- Follow-up commit in the same PR fixed two deep-reviewer findings:
  `monomorphize.rs`'s module doc still claimed this code lived in
  `lib.rs` (now points at `enum_lower`), and the new module's own doc
  pointed to `check_and_resolve`'s doc comment for an ordering rationale
  that comment doesn't state (inlined directly instead).
- Mid-PR, the branch went stale (PRs #811-#813 landed on `main` from
  concurrent unrelated work — the `#782` `ScratchDir` migration batches).
  Updated via a plain merge of `origin/main` (verified conflict-free, no
  shared files) rather than a rebase, since a merge equally satisfies
  GitHub's up-to-date requirement without a force-push.

## Documentation review (verified, not skipped)

- `docs/ROADMAP.md`: no update required — pure internal code motion, no
  behavior/platform/milestone-evidence change. Left untouched, also
  respecting this run's explicit avoidance of colliding with the
  concurrently open PR #810 website-update work.
- `docs/SPEC.md`: no specification added/removed/repurposed.
- No documentation anywhere in `docs/` enumerates `lib.rs`'s internal
  function/module list (checked via grep across `docs/`), so nothing else
  needed updating.
- Issue #544 received its usual per-part narrowing comment (see below).

## Gates (all green)

- `cargo build --workspace`: clean.
- `cargo test -p pycc_types`: 1441 passed, 0 failed (identical to
  pre-move).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: **green**, 46,828/46,828 regions covered, 0 missed lines, 0 missed
  regions, 1996/1996 functions.
- `cargo fmt -p pycc_types -- --check`: introduces no new diffs (confirmed
  identical pre-existing diff set before/after via `git stash`, modulo
  line-number shifts).
- `cargo doc --workspace --no-deps`: succeeds, no new warnings (one
  pre-existing unrelated `env.rs` intra-doc-link warning, unchanged).
- Pinned `ievo:deep-reviewer` (D-068): one dispatch, 2 actionable findings
  (both doc-drift), both fixed in a follow-up commit in the same PR before
  merge.
- CI on PR #814: fully green after the branch update
  (`classify-changes`/`audit`/`status-page-freshness`/`ci-gate` and every
  required check), `mergeStateStatus: CLEAN`, `mergeable: MERGEABLE`.
- `gh api graphql` confirmed PR #814's `closingIssuesReferences.totalCount`
  is `0` before merge — it correctly closes nothing, since #544 stays
  open.

## Where to resume

Pick up issue #544's Part 7 next: `crates/pycc_types/src/lib.rs` is 3,550
lines, still the smallest of the three D-185 survivor trackers (vs. #545 at
8,503 and #695 at 29,514) if the same "smaller wins" tie-break applies
again — but re-verify file sizes fresh rather than trusting this figure,
since other work may touch these files first. `lib.rs`'s current top-level
structure (module declarations, diagnostic constructors, statement/pattern/
match checking, function and generic-function checking, the
`check`/`check_and_resolve`/`check_with_signatures`/`check_with_environment`
entry points) should be re-dumped via `grep -n "^pub fn \|^fn \|^pub struct
\|^struct \|^impl \|^pub enum \|^enum \|^mod \|^pub mod "` to find the next
cohesion-driven seam — no specific one was pre-identified for Part 7 at the
end of this session.
