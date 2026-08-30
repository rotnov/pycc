# Session handoff: issue #854 — `__init_subclass__` guard unification (own-override precedence + `@classmethod` blind spot)

- Date: 2026-08-30
- Issue: [#854](https://github.com/rotnov/pycc/issues/854) (v0.4, filed by the D-068 pinned reviewer's pass on #855)
- Decision entry filed this session: [D-214](../decisions/D-214-unify-the-init-subclass-guard-into-a.md)
- Branch: `claude/issue-854-init-subclass`
- Base commit: `6e68888e` (origin/main at task start, confirmed up to date; no open PRs competed for the D-214 number)

## Status

Implemented per the posted plan comment
(`https://github.com/rotnov/pycc/issues/854#issuecomment-5468022949`), with
one deliberate, evidence-backed deviation from that plan's Test E (see
below). All local gates green as of this writing; PR not yet opened / CI not
yet run as of this snapshot (see "Next steps").

## What was built

`crates/pycc_hir/src/class.rs`'s `__init_subclass__` guard (previous
`if`/`else if` structure, ~2076-2134) validated the *current class's own*
`__init_subclass__` body whenever both the current class and some base
defined the hook, and only ran the corrected ancestor lookup when the
current class had no override of its own. This is backwards relative to
CPython: `super(new_cls, new_cls).__init_subclass__(**kwargs)`'s MRO lookup
starts immediately after `new_cls`, so `new_cls`'s own definition is never
invoked at its own creation. A second, independent blind spot: a
`@classmethod`-decorated `__init_subclass__` lives in the `class_methods`
table, never `methods`, so both HIR-table-based checks in the old guard
missed it entirely.

The fix (per the plan's §3.1-3.2): replaced the `if`/`else if` with a single
unconditional `if let Some(base_ast) = mro.iter().skip(1).find_map(...)`
block that runs identically for every class regardless of what it defines
itself — nearest MRO ancestor with an introspectable `__init_subclass__`
(via a decorator-agnostic raw-AST scan, closing the `@classmethod` blind
spot structurally, not via a second check) has its body validated against
the current class's own creation site. `validate_init_subclass_body` lost
its `inherited: bool` parameter (one message variant now) and its generic
`<R>` range parameter (now a concrete `std::ops::Range<u32>`, since exactly
one call site remains); `pycc_ast` deliberately does not re-export
`ruff_text_size::TextRange`, so the call site converts via `.into()` rather
than naming `TextRange` in `pycc_hir`.

Full rationale, alternatives considered, and the new consequence discovered
during implementation are recorded in
[D-214](../decisions/D-214-unify-the-init-subclass-guard-into-a.md).

## Deviation from the posted plan: Test E

The plan's Test E fixture (`B` side-effecting / `D(B)`'s own override
trivial / `Grandchild(D)` no override, "expect accepted") is
**unrealizable** under the corrected unconditional model: because the guard
now runs at *every* class's own creation against *that class's own* nearest
ancestor, `D`'s own creation is checked against `B` (its nearest ancestor)
independent of `D`'s own override — `B` being side-effecting already
rejects `D` on its own, before `Grandchild` is ever lowered. This is
consistent with the plan's own truth table (§8: "B side-effecting / D
trivial → reject") applied to the `D`/`B` pair inside the three-level
fixture; the plan's author did not carry that table's implication forward
to the middle class of its own three-level test. This was caught
empirically: implementing the fix exactly as specified and running the
posted Test E fixture returned `Err` where the plan asserted `Ok`. Traced
CPython's actual `super(new_cls, new_cls)` semantics by hand and confirmed
the rejection is the *correct* behavior — reproduced against real CPython
semantics, not just against pycc's own logic. This was independently
confirmed by this session's advisor tool before being resolved (per D-127)
by revising the test, not the production code, since accepting the plan's
original fixture would reintroduce the exact soundness gap D-213/D-214
close.

Resolution: replaced Test E with two fixtures (see D-214's Decision section
for the exact reasoning and code):
- `grandchild_validates_parents_own_override_not_grandparents_hook` — a
  realizable reject-direction three-level fixture (`B` trivial / `D`'s own
  override side-effecting / `Grandchild(D)` no override → rejected, since
  the grandchild's nearest ancestor is `D`'s own override, not `B`'s
  grandparent hook).
- `multiple_inheritance_nearest_mro_hook_wins_over_farther_side_effecting_one`
  — the genuine "nearest ancestor in MRO order, not any ancestor" positive
  discriminator, using C3 multiple inheritance (`class D(M, B)`, MRO
  `[D, M, B, object]`) rather than a linear chain, since a linear chain
  cannot produce a realizable accept-direction case here.

## Test changes (per the plan's exact enumeration, §4.2-4.3)

- Unit tests (`crates/pycc_hir/src/class.rs`): deleted
  `init_subclass_with_non_string_expr_in_subclass_of_base_with_init_subclass_is_rejected`,
  `init_subclass_with_return_in_subclass_of_base_with_init_subclass_is_rejected`
  (both flipped rejected→accepted; coverage duty reassigned to new Test A),
  and `init_subclass_before_init_in_body_validates_correctly` (vacuous —
  its walked code path is deleted) and
  `init_subclass_with_empty_body_in_subclass_of_base_is_accepted`
  (confirmed byte-identical duplicate of the `..._pass_body_..._accepted`
  test). Updated doc comments on
  `init_subclass_with_pass_body_in_subclass_of_base_with_init_subclass_is_accepted`
  and `init_subclass_with_docstring_in_subclass_of_base_with_init_subclass_is_accepted`
  (docstring moved onto `B`'s body) to describe which body the unified path
  now validates. Added Tests A-D exactly per the plan (`side_effecting_base_hook_with_trivial_subclass_override_is_rejected`,
  `classmethod_decorated_ancestor_hook_with_trivial_subclass_override_is_rejected`,
  `classmethod_decorated_ancestor_hook_with_no_subclass_override_is_rejected`,
  `trivial_base_hook_with_classmethod_decorated_subclass_override_is_accepted`)
  and Test E per the corrected fixtures above.
- Integration tests (`tests/issue_435_isinstance_issubclass.rs`): repurposed
  `init_subclass_with_non_trivial_body_is_rejected` by swapping which class
  carries the side effect (now `B`'s body does `print("hello")`, `D`'s own
  override is `pass`) so it still rejects post-fix while exercising the
  corrected code path. Comment-only updates on
  `init_subclass_with_pass_body_is_accepted` and `init_subclass_inherited_from_base`;
  `init_subclass_with_docstring_body_is_accepted`'s fixture already carried
  the docstring on `B` (no change needed there beyond the comment).
- `grep -rln "__init_subclass__"` across `.py`/`tests/`/`examples/` found no
  other affected fixture outside the two files above.

## Documentation updates

- `docs/ROADMAP.md:207` and `docs/TYPE_SYSTEM.md:198` rewritten to describe
  the corrected unconditional nearest-ancestor rule, dropping the
  "#854 tracks the rest" / "separate, still-open gap" framing.
- `docs/PYTHON_STANDARDS.md`: confirmed needing no change (PEP 487 row stays
  `☐`, no fixture, per D-213) — stated explicitly in the PR description too.
- `docs/decisions/D-214-unify-the-init-subclass-guard-into-a.md` filed;
  `docs/decisions/README.md` regenerated via
  `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md`
  and confirmed fresh with `--check`.

## Gates run locally (this session)

- `cargo build --workspace`: clean.
- `cargo test -p pycc_hir class::tests::`: 211 passed, 0 failed (unit tests
  for this guard).
- `cargo test -p pycc_hir`: 736 passed, 0 failed (full crate).
- `cargo test --test issue_435_isinstance_issubclass`: 33 passed, 0 failed.
- `cargo test --workspace`: exit code 0 (all crates, all test binaries).
- `cargo clippy --workspace --all-targets -- -D warnings`: exit code 0 (the
  3 rustc "multiple lines skipped by escaped newline" notes in
  `tests/slice1_codegen_depth.rs` are pre-existing and unrelated to this
  change — confirmed they do not fail the `-D warnings` gate).
- `cargo doc --workspace --no-deps`: succeeds; the one warning present
  (`pycc_types::env::bind_class` linking to a private item) is pre-existing
  and unrelated to `pycc_hir`.
- `cargo llvm-cov -p pycc_hir --fail-under-lines 100 --fail-under-regions 100`:
  **100.00% lines, 100.00% regions** across every file in the crate,
  including `class.rs` (3052/3052 lines, 3901/3901 regions) and
  `class/mro.rs` (201/201 lines, 266/266 regions).
- `ruby scripts/check_roadmap_evidence.rb`: passes under `LANG=en_US.UTF-8
  LC_ALL=en_US.UTF-8` (the ASCII locale produced an unrelated, pre-existing
  `invalid byte sequence in US-ASCII` crash reproduced identically on the
  unmodified base commit via `git stash` — not a regression from this
  change).

## Next steps for a resuming session

1. Run the D-068 pinned local reviewer (`ievo:deep-reviewer`) against the
   staged/committed diff and address actionable findings.
2. Commit, push `claude/issue-854-init-subclass`, open the PR (`gh pr
   create`, body via `--body-file`), decide on "Fixes #854" (plan implies
   yes — both gaps described in the issue are structurally closed by this
   change).
3. Wait for CI via `.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh`;
   diagnose and fix any failures.
4. Independently verify mergeability via `gh api graphql`
   (`mergeStateStatus`, `closingIssuesReferences`, `reviewThreads`,
   `statusCheckRollup`) before self-merging per repo convention.
