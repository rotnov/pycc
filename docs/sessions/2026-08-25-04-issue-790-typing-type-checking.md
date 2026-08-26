# 2026-08-25-04 -- Issue #790: register `typing.TYPE_CHECKING` and fold its dead `if`-branch

## Status: delivered by the pull request that carries this file

Branch `feat/issue-790-tc-23e19060`, based on `origin/main`. Worktree
`/Users/denis/projects/pycc-worktrees/issue-790-tc-23e19060`, created after an
earlier session detected and decoupled from a worktree contaminated by another
concurrent actor (see "Known follow-ups" below). The pull request opened from
this branch carries `Fixes #790`.

## What shipped

`typing.TYPE_CHECKING` is CPython's standard idiom for guarding imports and
statements meant only for static type checkers -- it is always `False` at
runtime, so a guarded `if TYPE_CHECKING:` body never executes. Before this
change `from typing import TYPE_CHECKING` was rejected outright with `C0002`.
Three seams:

- `crates/pycc_std/src/lib.rs` -- registers `typing.TYPE_CHECKING` under a new
  `StdSymbolKind::TypeCheckingMarker` so the import resolves.
- `crates/pycc_hir/src/stmt.rs` -- the substantive fix, `is_type_checking_guard`:
  recognizes the bare `TYPE_CHECKING` name or the qualified
  `typing.TYPE_CHECKING` spelling *syntactically*, as an `if`/`elif` test only,
  independent of whether the corresponding import actually happened (matching
  the existing bare-name `Final` precedent). A matching `if`/`elif` lowers to
  `HirStmt::If { test: HirExpr::BoolLiteral(false), body: vec![], orelse: ... }`
  -- the guarded body is erased entirely rather than lowered to a no-op, so it
  never reaches `lower_body`/type-checking and may contain otherwise-unsupported
  constructs. A following `else`/`elif` stays live and is lowered/checked
  normally.
- `crates/pycc_types/src/lib.rs`, `expr.rs`, `constraints.rs` -- a new
  `type_checking_marker_is_not_a_value` `T0021` diagnostic, mirroring the
  existing `cast_marker_is_not_a_value`, wired into the same marker-rejection
  arms every other `StdSymbolKind` marker already uses.

## Decisions made autonomously (D-127)

- **Erase the dead body entirely rather than lower it to a no-op.** No
  flow-sensitive narrowing analysis exists on `main` today, so a no-op branch
  would still be visible to (a currently nonexistent) flow analysis for no
  benefit, while erasure keeps the 1:1 `lower_stmt` contract and is what lets
  the guarded body contain constructs pycc does not support elsewhere --
  exactly what the idiom is for (deferred-evaluation type-only imports).
- **Bare `TYPE_CHECKING` is not bound into the environment for general value
  use; only the qualified form is.** Initially wrote tests expecting the
  dedicated "compile-time marker" `T0021` message for `x = TYPE_CHECKING`
  after a bare `from typing import TYPE_CHECKING`. Both failed empirically --
  the actual message is the generic "name `TYPE_CHECKING` is not defined"
  `T0021`. Root cause: `is_type_checking_guard` recognizes the bare name only
  syntactically, in the `if`/`elif` test position; there is no binding step
  that puts it into scope for value use, matching the pre-existing precedent
  for every other bare marker name (`Final`, etc. -- none of them are bound
  for value use either). Only the qualified `typing.TYPE_CHECKING` spelling
  resolves through `pycc_std::resolve_symbol` for value use and hits the
  dedicated marker diagnostic. Confirmed this is correct, existing-precedent
  behavior, not a bug; documented the reasoning in the test's own doc comment
  rather than "fixing" it into an inconsistency with every other bare marker.
- **`docs/ROADMAP.md`'s #790 entry is a single short line, unlike neighboring
  entries.** `scripts/check-site.sh` enforces a 256 KiB aggregate `llms.txt`
  non-optional-expansion budget (issue #207) across a fixed manifest of files
  including `docs/ROADMAP.md`'s full byte size; that budget was already tight
  before this change and any paragraph-length entry (matching e.g. #767's or
  #774's style) pushed it over by up to ~1.2 KiB. `docs/STDLIB_PLAN.md` is not
  in the manifest and carries the fuller rationale instead; the ROADMAP entry
  points there. Confirmed via direct measurement
  (`site/llms-txt-context-manifest.json` sums six tracked files' literal
  on-disk byte sizes; `docs/STDLIB_PLAN.md` edits measurably had zero effect on
  the reported total, confirming it is excluded) rather than guessed.

## Test evidence

- `crates/pycc_hir/src/tests.rs` -- 6 new tests: bare and qualified guard
  folding to an empty dead `HirStmt::If`, the `else` branch of a
  `TYPE_CHECKING` guard lowering normally, `elif TYPE_CHECKING:` folding
  inside a 3-way if/elif/else chain, and two tests closing an error-propagation
  gap the first coverage run surfaced (a failing `else`/`elif` clause *after*
  a `TYPE_CHECKING` guard must still propagate its lowering error through the
  `?` on the recursive `lower_elif_else_clauses` call in both the leading-`if`
  and the `elif` arms of the fold).
- `crates/pycc_types/src/tests.rs` -- 3 new tests: bare `TYPE_CHECKING` used as
  a value is the generic "not defined" `T0021`; the qualified
  `typing.TYPE_CHECKING` used as a value is the dedicated "compile-time
  marker" `T0021`, both at top level and inside a private helper (exercising
  the constraint-solver path).
- `tests/issue_790_typing_type_checking.rs` (new file, 5 CLI-level end-to-end
  tests through the public `pycc` binary): bare and qualified guards with an
  otherwise-uncompileable body `check` successfully; a `build`+`run` proves
  only the live `else` branch ever executes for both a leading `if
  TYPE_CHECKING:` and an `elif TYPE_CHECKING:` chain (stdout asserted
  byte-identical to `"else\n"`); qualified `TYPE_CHECKING` used as a value is
  rejected by `pycc check` with `T0021` and the "compile-time marker" message.
- `cargo test --workspace`: exit 0, 61 `test result: ok` blocks, 0 failures.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: exactly 100.00% lines (28404/28404) and 100.00% regions (44027/44027)
  workspace-wide, 0 missed of either. The first run before the two
  error-propagation tests above reported 99.80%/99.73% for
  `crates/pycc_hir/src/stmt.rs` specifically; `cargo llvm-cov report
  --show-missing-lines` (run from inside `crates/pycc_hir/`, since `--workspace`
  is invalid for the `report` subcommand) pinpointed the two exact uncovered
  lines (332, 868), both the `?` on `lower_elif_else_clauses` inside the two
  new fold arms.
- `ruby scripts/check_roadmap_evidence.rb` (needs an explicit UTF-8 locale in
  this shell: `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby -E UTF-8 ...`, an
  environment quirk unrelated to this change) and `GITHUB_PAGES=true sh
  scripts/check-site.sh` both pass.
- D-068 pinned `ievo:deep-reviewer` run against the full working-tree diff;
  see the fix-round bullet below for its findings and resolution.

## Documentation updated in the same PR

- `docs/ROADMAP.md` -- new, deliberately terse v0.3 entry for #790 (see the
  budget-driven decision above).
- `docs/STDLIB_PLAN.md` -- `typing.TYPE_CHECKING` added to the Tier-1 typing
  row's shipped-symbols list and issue list, with a sentence describing the
  registration + fold mechanism and pointing at `pycc_hir::stmt::is_type_checking_guard`.

## Known follow-ups / non-actions

- `T0041` (or any future flow-sensitive narrowing/reachability diagnostic) is
  not made aware of a constant-`false` `if`/`else` by this change -- a
  pre-existing gap (no flow analysis exists yet) that this change surfaces but
  does not fix, since fixing it is out of scope for a stdlib-registration
  issue.
- A negated (`if not TYPE_CHECKING:`) or compound (`if TYPE_CHECKING and
  cond:`) test is not folded -- `is_type_checking_guard` matches only the bare
  test position exactly, matching the existing `Final` bare-name-recognition
  precedent's own scope.
- During an earlier session on this task, disk-space exhaustion from several
  concurrent worktrees' `target/` directories caused one `cargo llvm-cov`
  attempt to fail with an unrelated linker `ENOSPC`; resolved by deleting one
  long-abandoned worktree's build cache only, not touched again this session.
  A third concurrent actor's worktree (`issue-790-solo`) was also observed
  apparently working the same issue; this worktree's isolation was reconfirmed
  clean (`git status --short --branch`, single writer) before every gate run
  in this session, and no contamination occurred.

## Where to resume

Nothing outstanding from this task beyond the PR's own CI and review cycle.
`docs/sessions/` listing sorted by filename remains the resume mechanism; this
is the newest entry for 2026-08-25.
