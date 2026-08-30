# Session handoff: #834 track a duplicate int's retain on the exception edge

## Status: PR opened, awaiting CI / merge

This session implemented the plan published at
[#834's own comment](https://github.com/rotnov/pycc/issues/834#issuecomment-5466183269)
exactly, on branch `session-20260830-autopilot`
(worktree `/Users/denis/projects/pycc-worktrees/session-20260830`), then
opened a pull request. Per the dispatching instructions for this task, this
session did not watch CI or merge -- the coordinating session does that.

## What changed (commit `16b394e4`)

`retain_if_int_duplicate`'s own extra retain of a *borrowed* `int` (at the
`MirExpr::TupleLiteral` element loop and `build_call_to_with_leading_args`'s
argument loop -- the only two of the classifier's seven call sites that
bracket the retain with a D-208 `mark`/`truncate` pair on
`rt.exceptions.pending_int_releases`) was invisible to that pending-release
mechanism: a later sibling's raising evaluation abandoned the retained
reference with nothing to release it on the exception edge -- a pure leak,
never a use-after-free, since the original owner's own reference is
untouched.

- `crates/pycc_codegen/src/bigint_rc.rs`: split the classification out of
  `retain_if_int_duplicate` into `retain_if_int_duplicate_reporting` (returns
  `(Scalar, bool)`, the `bool` telling whether it retained);
  `retain_if_int_duplicate` is now a thin wrapper.  Added
  `retain_if_int_duplicate_and_track_for_exception_edge`, which pushes the
  retained word onto `pending_int_releases` (via a new shared
  `push_word_onto_pending_int_releases` primitive, also now used by
  `push_pending_int_release_if_temporary`) when the reporting call says a
  retain happened. Corrected a stale doc comment that miscited this as
  D-180 residual item 3.
- `crates/pycc_codegen/src/lib.rs`: swapped in the new tracking call at
  exactly the two sites named above; no other of the seven
  `retain_if_int_duplicate` call sites were touched, and `lib.rs` was not
  decomposed.
- `tests/issue_638_bigint_exception_release.rs`: extended (not replaced)
  with two correctness tests (one per fixed site) and two peak-RSS
  marginal-ratio tests (`< 1.15`, matching the file's existing convention)
  proving the fix actually stops the leak at both sites, reusing the file's
  existing `marginal_rss`/`built_program_peak_rss`/`peak_rss` helpers.
- `docs/decisions/D-212-track-a-duplicate-int-s-retain-on-the.md` (new):
  records the fix and explicitly corrects D-208's own prose, which
  overstated that all six of its protected sites pair with
  `retain_if_int_duplicate` -- only two actually call it
  (`BinOp`/`Compare`/the `range()` preheater never call it at all). D-208
  itself is left unedited per AGENTS.md's "never silently rewrite an
  accepted decision" rule; D-212 supersedes that one claim.
  `docs/decisions/README.md` regenerated.
- `docs/RUNTIME.md` and `docs/ROADMAP.md`: both updated to describe the
  closure and the corrected two-of-six framing; both cited #834 by name
  before this change.

## Plan verification and deviations

Every factual premise in the plan checked out against the tree with no
drift since the plan was authored (same `005af24e` baseline): the exact
seven-call-site inventory and line numbers, the two-of-six D-208 site
pairing, the pre-existing #834 citations already in the comments, and the
coverage `false`-arm-already-covered claim (confirmed directly by the
100.00%-regions result rather than assumed). No plan refutation was found;
implementation followed it exactly. The only deviation was operational, not
substantive: the D-014 coverage gate was run directly via `cargo llvm-cov`
under a freshly created isolated `TMPDIR`, rather than through CI's
`sudo -u nobody env -i` sandbox wrapper -- that sandbox exists for CI's
untrusted-PR-code trust boundary, not to change the measurement, for a
trusted local edit.

### A double-release concern raised and refuted before opening the PR

An independent review pass (this session's own advisor tool) flagged a
plausible-looking risk: pushing the duplicate's word via the new function
*and* the pre-existing `push_pending_int_release_if_scalar_temporary` at
the same site could double-push the same word onto
`pending_int_releases`, causing a double-release on the exception edge --
strictly worse than the leak being fixed -- if the retain-side classifier
(`retain_if_int_duplicate_reporting`) and the release-side classifier
(`int_value_is_a_duplicate_reference`) ever disagreed on a shape, since the
two are deliberately *not* shared and the file's own doc comments say they
"fail in opposite directions."

Verified directly by reading both match arms side by side
(`bigint_rc.rs`): both classify `Name`/`AttrGet`/`NamedExpr { ty: Int }` and
`OptionalUnwrap` as `true`, both classify `Subscript { base, .. }` as
exactly `matches!(base.ty(), Ty::Tuple(_))`, and both fall through to
`false` otherwise -- the two predicates agree on every arm that exists
today. `push_pending_int_release_if_temporary`/`_scalar_temporary` push
only when `int_temporary_word` (which requires
`!int_value_is_a_duplicate_reference(source_expr)`) is true, so whenever
the new function's retain fires (`retained == true`), the owning-temporary
push at the same site is always suppressed. No double-push exists at either
fixed site today. This is a property of the current arm sets, not a
structural guarantee the two functions being separate provides for free --
worth re-checking if either classifier's arms are ever extended
independently.

## Local gates (commit `16b394e4`)

- `cargo build --workspace`: pass.
- `cargo test --workspace`: pass, every suite `test result: ok`, `0 failed`.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass, clean.
- D-014 coverage gate (`cargo llvm-cov --workspace --fail-under-lines 100
  --fail-under-regions 100`, isolated `TMPDIR`): pass -- TOTAL 49012/49012
  lines (100.00%), 2116/2116 functions (100.00%), 31655/31655 regions
  (100.00%).
- `cargo doc --workspace --no-deps`: pass; one pre-existing, unrelated
  warning (private intra-doc link in `crates/pycc_types/src/env.rs:308`,
  untouched by this change).
- `RUBYOPT="-E UTF-8" ruby scripts/check_roadmap_evidence.rb`: pass.
- `RUBYOPT="-E UTF-8" ruby scripts/test_check_roadmap_evidence.rb`: pass,
  237 runs / 1222 assertions / 0 failures / 0 errors.
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md --check`: pass, up to date.

## Known limitation: pinned local reviewer not run from this session

AGENTS.md's "Local pinned review loop" section requires a fresh
`ievo:deep-reviewer` pass over the committed range before merge. This
session's runtime does not expose an `Agent`/`Task` dispatch tool, and the
`deep-review` skill itself refuses model invocation
(`disable-model-invocation`, "reserved for explicit user invocation") --
so this session could not run that step itself, unlike the
`docs/AGENT_TOOLING.md`-documented local CLI flow. This is a tooling gap in
this session's environment, not a decision to skip the gate: **the pinned
reviewer must still run over this PR's committed range before it merges.**
The coordinating session (or whichever session merges this PR) should run
it and address any actionable findings first.

## Follow-ups / known non-issues

- No new issues were filed or narrowed as a side effect of this task.
- `docs/decisions/D-208-*.md` line 142 still cites #834 as open -- expected
  and correct: that file is intentionally left unedited (superseded, not
  rewritten) per AGENTS.md; D-212 is the authoritative correction.
