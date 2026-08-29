# Session handoff: #604 (Part 3 of #573) -- unary `not`/`~`

Status: **merged and closed**. This snapshot reflects `origin/main` at the
exact merge commit inspected: `867ac101eacb5afa9d7dd2334709fb517fc0f802`
(2026-08-29T20:36:03Z), confirmed as `origin/main`'s tip via a fresh
`git fetch origin main` immediately before writing this file.

## Summary

- Issue: [#604](https://github.com/rotnov/pycc/issues/604), "Part 3 of #573:
  unary `not` and `~`" (milestone v0.4). Selected because it was the last
  open sub-issue of #573's decomposition (Part 1 = #602, Part 2 = #603, both
  already closed); completing it also closes the parent #573.
- PR: [#839](https://github.com/rotnov/pycc/pull/839), squash-merged.
  Merge commit: `867ac101eacb5afa9d7dd2334709fb517fc0f802`.
- Issues closed by the merge (verified via
  `closingIssuesReferences` before merging, `totalCount: 2`): **#604** and
  **#573**.
- Branch `issue-604-unary-not-invert` deleted on merge (`--delete-branch`).

## What shipped

- `not x` over a non-literal operand types as the `bool` projection of the
  operand's truthiness -- `bool`/`int`/`float`/`str`/`None`/
  `Optional`/a class instance are accepted (anything else is `T0021`) --
  and lowers to a dedicated `MirExpr::Not` node in `pycc_mir` that reuses
  the same `truthy` codegen helper `if`/`while` conditions already call.
- `~x` types as `int -> int` (`bool` included) and rewrites to `-x - 1` at
  the MIR level (two chained `BinOpKind::Sub` nodes), inheriting
  `int_sub`'s bigint/smallint-boundary handling exactly like plain unary
  negation (#603).
- New CPython-oracle differential fixture
  (`tests/fixtures/unary_not_invert.py`) and CLI integration suite
  (`tests/issue_604_unary_not_invert.rs`, 27 tests) covering `not`/`~` over
  every accepted type, composition, and the `T0021` rejections.

## Fix rounds during this task (see `docs/AGENT_RETROSPECTIVE.md` for the
process lessons; this is only the outcome record)

1. **D-014 coverage gap-closing** (pre-existing entries in the plan; not
   repeated here in full): targeted inline unit tests were added to
   `pycc_codegen`, `pycc_hir`, and `pycc_types` to reach `?`-operator
   error-propagation sub-line regions and the new `MirExpr::Not` codegen
   arm that only this crate's own `cfg(test)` compilation unit exercises.
   Final local gate: `cargo llvm-cov --workspace --fail-under-lines 100
   --fail-under-regions 100` at 100.00%/100.00%/100.00% (48792 regions,
   31498 lines, 0 missed in both).
2. **CI `Pages` workflow failure, self-caused and fixed in-branch**: the
   `docs/ROADMAP.md` prose this PR added (documenting `not`/`~` as shipped)
   pushed the `llms.txt` non-optional aggregate expansion (issue #207) from
   ~270.2 KiB to 270898 bytes, 562 bytes over the 270336-byte (264 KiB)
   ceiling enforced by `scripts/check-site.sh`. Confirmed self-caused (not
   pre-existing) by diffing against `origin/main`'s own `docs/ROADMAP.md`,
   where the same Pages workflow was green immediately before this
   branch's push. This check is **not** a required branch-protection
   context (only `ci-gate` and `audit` are), so it did not block merge on
   its own, but was fixed rather than left broken: the added prose was
   trimmed to the same facts in fewer words, landing the aggregate at
   270270 bytes (66-byte margin under budget), verified directly against
   `site/llms-txt-context-manifest.json`'s six `non_optional_documents`
   entries rather than by re-running the full Pages workflow.
3. **Pinned `deep-reviewer` round (D-068)** found two real gaps, both
   fixed in a follow-up commit on the same PR:
   - `not None` incorrectly rejected with `T0021` even though
     `pycc_codegen::truthy`'s existing `Bool`/`Optional` arms already
     compute the correct always-`False` truthiness for both of
     `Ty::None`'s representations (a bare `Ty::None`-typed `Name` read
     lowers to `Scalar::Bool` backed by an always-zero `i8` slot;
     `MirExpr::NoneLiteral` lowers to `Scalar::Optional`) -- no codegen
     change needed, just widening `unary_result_type`'s `Not` arm.
   - Two comments in `crates/pycc_mir/src/expr.rs` asserting "pycc's MIR
     has no unary node" were left stale by this same diff's own
     `MirExpr::Not` addition; corrected to scope the claim to
     `USub`/`UAdd`/`Invert` and to stop citing the disproven reason for
     the dataclass `!=`-as-`Compare`-against-`BoolLiteral` rewrite.
4. **Independent automated GitHub review** (`chatgpt-codex-connector`, not
   a required gate per AGENTS.md but its thread blocked merge under
   `required_conversation_resolution`) found the same class of gap for
   `Ty::Instance(_)`: `truthy`'s `Scalar::Instance` arm already returns a
   constant always-true `1` (D-154, no `__bool__`/`__len__` shipped in this
   version), so `not <instance>` should type-check and always be `False`.
   Fixed in the same commit as the `deep-reviewer` fixes; replied to and
   resolved the review thread via `gh api graphql` (`addPullRequestReviewThreadReply`
   / `resolveReviewThread`) once the fix commit was actually pushed.
5. Full local gate re-run and green after every fix round: `cargo test
   --workspace` (0 failed across all crates and integration suites),
   `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
   100` (100%/100%/100%, exit 0), `cargo clippy --workspace --all-targets
   -- -D warnings` (exit 0, only pre-existing unrelated warnings present),
   `cargo doc --workspace --no-deps` (succeeds, one pre-existing unrelated
   rustdoc warning in `pycc_types::env`), `ruby
   scripts/test_check_roadmap_evidence.rb` / `ruby
   scripts/check_roadmap_evidence.rb` (pass under a UTF-8 locale; the
   local shell's own `LANG=""` default is a known non-CI artifact, not a
   real defect).

## Denylist / tracker state

- No stale issues found or closed at the tracker level during this task
  beyond #604/#573 themselves.
- No milestone or issue-metadata changes were needed: #604 already carried
  milestone v0.4 at filing.
- No new umbrella-issue or D-192 non-milestone-work bookkeeping was
  triggered by this task.

## Hand-off for the next session

- #573's full decomposition (Parts 1-3: #602, #603, #604) is now closed
  end-to-end. No open sub-issue remains under #573.
- Next autopilot iteration should re-run `.claude/skills/issue-select/SKILL.md`
  from a freshly fetched `origin/main` (now at
  `867ac101eacb5afa9d7dd2334709fb517fc0f802`) to pick the next issue --
  nothing about this task leaves a specific follow-up issue pending, beyond
  the ordinary open backlog.
- One process observation worth carrying forward (already actionable, not
  merely a retrospective line): a PR that adds prose to `docs/ROADMAP.md`
  should check the `llms.txt` aggregate budget
  (`site/llms-txt-context-manifest.json`'s `non_optional_documents` sizes
  vs. `budget_kib`) before pushing, the same way it already checks
  `scripts/check_roadmap_evidence.rb` -- this task discovered the failure
  reactively via a non-required CI check rather than proactively.
