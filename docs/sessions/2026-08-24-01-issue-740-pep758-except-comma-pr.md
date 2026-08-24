# Session handoff: issue #740 (Part 3 of #543) — PEP 758 multi-type `except` handlers

- Status: implementation and a post-review soundness fix are both complete and
  green on branch `issue-740-pep758-except-comma`, based on `origin/main` tip
  `47908212` (which already includes merged PR #741 for issue #738 / Part 1 of
  #543, and merged PR #742 for issue #739 / Part 2 of #543). This is the third
  and final sub-issue of #543. PR [#743](https://github.com/rotnov/pycc/pull/743)
  is open with all CI checks green (`mergeStateStatus: CLEAN`) and zero
  unresolved review threads; merge, branch deletion, and closing #543 are the
  only steps left, listed below.
- Implementation followed the plan already published and reviewed as a comment
  on issue #740 (posted 2026-08-24T07:32:08Z) verbatim — see that comment for
  the full design rationale, the four corrected premises found during planning
  (parenthesized-flag difference, `except A, B as e:` being a confirmed
  parser-level syntax error, `except ():` needing rejection, `except A,:`
  needing to behave like `except A:`), and the ten-call-site inventory.
- What shipped:
  - `crates/pycc_hir/src/exception.rs`: `HirExceptHandler.exc_type` widened
    from `Option<String>` to `Option<Vec<String>>`. `None` stays bare
    `except:`; `Some(vec)` is never empty (enforced at the one HIR-lowering
    production site). New shared helper
    `except_handler_binding_type_name(names: &[String]) -> String` — the
    single representative-type decision for `as`-binding, applied identically
    at all four binding sites so a future change to the decision only needs to
    touch one function. **Post-review revision:** originally returned
    `names[0]` unconditionally; a bot review on PR #743 found that unsound for
    `isinstance()` (see the "Post-implementation review-loop fix" bullet
    below), so it now returns the handler's own exact name for a single-name
    handler and the literal `"Exception"` for a genuinely multi-type one.
  - `crates/pycc_hir/src/stmt/exception.rs::lower_except_handler`: multi-type
    lowering via `Expr::Tuple` walk, empty-tuple rejection (new `C0001`, "an
    `except` handler must name at least one exception type"), per-element
    non-`Expr::Name` rejection (extended existing `C0001` message).
  - `crates/pycc_types/src/exception.rs::check_try_stmt`: per-name validation
    loop (fails on the first invalid name), per-name "as`-binding a
    user-defined class" rejection extended to the multi-type case, naming the
    specific offending class.
  - `crates/pycc_types/src/{lib.rs,monomorphize.rs,constraints.rs}`: the three
    additional call sites the plan's workspace-wide grep found beyond the
    briefing — `lib.rs`'s pass-through clone needed no behavioral change;
    `monomorphize.rs` and `constraints.rs` both apply the same
    representative-name helper as the primary type-checking and MIR sites.
  - `crates/pycc_mir/src/stmt.rs`: the `HirStmt::Try` arm unions
    `handler_type_tags` across every named type, then `.sort_unstable();
    .dedup();` the combined vector before storing it as `exc_type_tag` —
    required because overlapping builtin families (e.g. `OSError` +
    `ConnectionError`) would otherwise double-count shared descendant tags.
    `handler_type_tags` itself needed no change.
  - `crates/pycc_codegen/src/exception.rs`: no source change — the dispatch
    loop already ORs generically over the tag vector's length; verified the
    `accumulated.expect(...)` invariant still holds given the HIR-side
    non-empty guarantee.
  - New decision entry:
    `docs/decisions/D-195-pep-758-multi-type-except-representation.md`
    (the `Option<Vec<String>>` representation choice and rejected
    `Vec<String>`-alone alternative, the empty-tuple rejection, the
    representative-binding decision), indexed in `docs/decisions/README.md`.
    Revised in place (still within this same, not-yet-merged PR) after the
    post-review fix below — see its "Revision (2026-08-24)" paragraph and its
    now-three-way "Alternatives" list (dropping the `Option`, a real
    nearest-common-ancestor computation, and the original unconditional
    first-listed-name choice).
  - Docs: `docs/PYTHON_STANDARDS.md` (PEP 758 row's Test cell corrected from
    the stale `py314/`-prefixed placeholder to the real flat fixture path;
    status stays `☐` per D-102 pending a Tier-1 CI observation),
    `docs/ROADMAP.md` (new dated update paragraph on the existing v0.3
    #382/#540 exception-handling entry, following the Part 1/Part 2 style).
  - Tests: new `tests/issue_740_multi_type_except.rs` (bare-comma and
    parenthesized forms, `as`-binding + re-raise, non-matching
    fall-through/propagation, subclass-of-one-of-several-types including the
    overlapping-family dedup case, single-name regression, 3+-type handlers,
    user-defined-class-with-`as`-rejection, and its positive complement
    without `as`), two new `tests/diagnostics/` fixtures (`C0001` empty-tuple,
    `L0001` bare-comma-plus-`as`), a new CPython-oracle conformance fixture
    `tests/fixtures/pep_0758_except_noparens.py` registered dual
    debug/release `#[ignore]`d in `tests/conformance.rs`, plus unit tests in
    `crates/pycc_hir/src/exception.rs`, `crates/pycc_mir/src/tests/exception.rs`,
    and `crates/pycc_codegen/src/tests.rs`.
- Review: the pinned iEvo `deep-reviewer` (D-068/D-155) ran two rounds against
  the full diff.
  - Round 1 (against commit `02b723d2`, the implementation): 1 warning + 2
    notes. Warning: the MIR union+dedup logic was only tested via a hand
    reimplementation in the test body, never through the real
    `HirStmt::Try` -> MIR lowering call site. Notes: the shared
    representative-name helper's empty-slice panic branch was untested at its
    consumption site; no e2e test exercised a user-defined class alongside a
    builtin in a multi-type handler with **no** `as` binding (only the
    `as`-rejection case was covered). All three verified against source and
    fixed in commit `26828a70` — a MIR-level test now drives a real
    overlapping multi-name handler through `build()` and asserts the deduped,
    sorted `exc_type_tag`; a `#[should_panic]` test pins the helper's panic
    message; a new e2e test confirms the no-`as`-binding case still compiles
    and catches.
  - Round 2 (against `02b723d2` + `26828a70`): confirmed all three round-1
    fixes genuinely close their findings (re-derived that the new MIR test
    fails if either `.sort_unstable()` or `.dedup()` is removed; confirmed the
    `should_panic` message matches source verbatim; confirmed the C0001
    rejection is gated strictly on `handler.name.is_some()` so the no-binding
    complement test is meaningful). One new note: `HirExceptHandler`'s doc
    comment said the empty-list invariant is enforced "at the one production
    site," which overclaims — a second, pre-existing non-test constructor
    exists in `pycc_types::unroll_enum_loops_in_stmts`'s HIR-to-HIR rewrite,
    harmless today (pure clone-through) but not what the doc literally said.
    Fixed in commit `7cd4fe34` (doc wording only, no behavioral change,
    rebuilt clean).
  - Post-implementation review-loop fix (against the open pull request,
    commit `9c727844`): `chatgpt-codex-connector[bot]`'s review on PR #743
    filed a P1 correctness finding after the branch was pushed and the PR
    opened. Root cause: `except_handler_binding_type_name` bound a
    multi-type handler's `as` name to `names[0]` unconditionally, and
    `isinstance()` folds entirely at compile time from the object's static
    `Ty::Instance` type — so `isinstance(e, names[0])` inside
    `except (ValueError, TypeError) as e:` compiled to a constant `True`
    even when the object actually caught at runtime was a `TypeError` and
    `names[0]` was `"ValueError"`. That is a soundness bug (a false
    positive), qualitatively worse than the pre-existing imprecision of a
    single-type handler's binding (which can only ever produce false
    negatives). Fix: the helper now returns the handler's own exact name
    for a single-name handler and the literal `"Exception"` (the universal
    root, tag 0) for a genuinely multi-type one, so `isinstance()` against
    any specific declared type of a multi-type handler is never
    compile-time-`True` unless the object's static type really is that
    type. Verified directly (manual `pycc build`/run against a scratch
    fixture) that `except (ValueError, TypeError) as e:` catching a raised
    `TypeError` now folds `isinstance(e, TypeError)` and
    `isinstance(e, ValueError)` to `False` and `isinstance(e, Exception)` to
    `True`. Added two regression tests to
    `tests/issue_740_multi_type_except.rs`:
    `isinstance_on_a_multi_type_handler_binding_never_produces_a_false_positive`
    and
    `isinstance_on_a_single_type_handler_binding_still_folds_true_for_its_own_type`.
    Updated `docs/decisions/D-195-pep-758-multi-type-except-representation.md`
    (new "Revision (2026-08-24)" paragraph, rewritten Alternatives list) and
    the `docs/ROADMAP.md` D-195 paragraph to describe the corrected logic and
    the bot finding. Replied to and resolved the bot's review thread
    (comment id `3842098437`, reply to review comment `3841900808`) after
    confirming its author is `type: "Bot"`. Finding and fix both appended to
    `.harden/findings/issue-740.jsonl` as a fifth entry (`round: 2`, severity
    `P1`, source `chatgpt-codex-connector[bot] review on PR #743`), bringing
    the file to five findings total across the pinned local reviewer's two
    rounds plus this one bot-authored round on the open PR.
  - Findings from the two pinned-reviewer rounds (the first four of the five
    total) were run through `/harden batch`: all four are distinct
    root-cause classes of size 1 (no journal match, no blocker-severity
    finding), so per the batch process each became an open `verdict:
    pending` incident counter (`dedup-logic-tested-only-by-hand-reimplementation`,
    `internal-invariant-panic-untested-at-consumption-site`,
    `conditional-rejection-missing-its-negative-complement-test`) with no
    durable artefact built — below the batch's frequency/cost threshold for
    escalating to a static gate. The fifth (bot-authored, isinstance
    soundness) finding was fixed directly rather than run through
    `/harden batch`, since it was found and resolved within this same
    session's own review-response loop.
- Local gates run (all green): `cargo build --workspace`, `cargo test
  --workspace` (0 `FAILED`, 0 panics — the only `error[...]` lines in the log
  are expected diagnostics from tests that intentionally trigger compiler
  rejections, unrelated to this change), `cargo clippy --workspace
  --all-targets -- -D warnings` (exit 0; the only warnings are pre-existing,
  in an unrelated test file), `cargo doc --workspace --no-deps` (exit 0; one
  pre-existing unrelated private-intra-doc-link warning in `pycc_types`),
  `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  — **100.00% lines, 100.00% regions, 100.00% functions** workspace-wide,
  `ruby scripts/check_roadmap_evidence.rb` (pass, needed
  `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8` — a pre-existing, unrelated locale
  quirk reproduced identically on clean `origin/main`), `python3
  scripts/check_conformance_breadth.py` (pass — 33 evidence-backed rows,
  unchanged; PEP 758 correctly stays non-evidence-backed), `python3
  scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md
  --check` (pass, up to date). All of the above were re-run and reconfirmed
  green against the isinstance-fix commit `9c727844` as well, not only
  against the earlier implementation/doc-fix commits.
- Not yet done (see the `issue-implement` skill's own remaining steps): the
  branch is pushed, PR [#743](https://github.com/rotnov/pycc/pull/743) is
  open with `Fixes #740` in the body (confirmed via `gh api graphql` that
  `closingIssuesReferences.totalCount == 1`), CI is green (including a
  known-flaky Windows nbody-speedup perf gate that was diagnosed as
  pre-existing CI infra noise unrelated to this diff and resolved via job
  retry), and the bot's review thread was replied to and resolved. Only the
  following remain:
  - Immediately before merging: re-verify `mergeStateStatus`,
    `closingIssuesReferences.totalCount`, and unresolved-review-thread count
    one final time (pushing this session-handoff-doc commit changes the PR
    head again).
  - Finish the final full re-read of the PR diff (only part of it had been
    re-read in this pass as of the last update to this file).
  - Merge PR #743 with a merge commit, delete the branch
    `issue-740-pep758-except-comma`, confirm issue #740 closed and the merge
    commit present on `origin/main`.
  - Close issue #543 once #740 is merged — confirmed via `gh issue view 543`
    that its three named sub-issues are #738 (closed), #739 (closed), and
    #740 (this work); no other lingering checklist item was found in its
    body. Cite the merged PR in the closing comment.
- Where to resume: this file, plus `git log` on `issue-740-pep758-except-comma`
  starting at commit `02b723d2` (implementation) through `9c727844` (the
  post-review isinstance-soundness fix) — four commits total (`02b723d2`,
  `26828a70`, `7cd4fe34`, `9c727844`), working tree clean.
