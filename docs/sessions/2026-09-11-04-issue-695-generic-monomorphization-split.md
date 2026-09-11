# 2026-09-11 (04) — #695: the generic-monomorphization lineage out of `pycc_types` `tests.rs`

## Overall status

Task base: `9b610738` (`origin/main`, "Define issue-to-plan's review-loop
termination in one place (#261) (#1009)"). Branch:
`autopilot/iter-2026-09-11-02`.

This session delivered one pull request against
[#695](https://github.com/rotnov/pycc/issues/695), the D-185 tracking issue for
the oversized `crates/pycc_types/src/tests.rs`. The generic-monomorphization
test lineage was relocated verbatim into five new sibling child modules under
`crates/pycc_types/src/tests/`, following the sibling-`tests.rs`-plus-`tests/`
layout (no `mod.rs`) established by PR #995.

`#695` stays open: at 21,431 lines the file is still far above the ~1,000-line
decomposability threshold, so it is narrowed by comment after each merge rather
than closed. The pull request carries no `Fixes #695`.

## What was extracted

| new file | lines | `#[test]` |
|---|---|---|
| `crates/pycc_types/src/tests/generic_class_instantiation.rs` | 739 | 9 |
| `crates/pycc_types/src/tests/generic_class_substitution.rs` | 511 | 15 |
| `crates/pycc_types/src/tests/generic_method_dedup.rs` | 676 | 12 |
| `crates/pycc_types/src/tests/generic_method_instantiation.rs` | 516 | 8 |
| `crates/pycc_types/src/tests/generic_monomorphization_arms.rs` | 785 | 14 |

`crates/pycc_types/src/tests.rs`: 24,766 → 21,431 lines; its own `#[test]`
count went 1,045 → 975. Five children rather than one or two was a deliberate
plan decision (resolved with this session's advisor per D-127): a single
`generic_*` module would itself have landed well over the ~1,000-line
threshold the extraction exists to respect, so the cut was chosen so that no
emitted file exceeds it. The second advisor-mandated decision was to defer
`constraints.rs`-adjacent material with a stated trigger rather than fold it in
here.

`crates/pycc_types/src/tests/enum_unrolling.rs` also grew 115 → 313 lines and
5 → 17 tests: the `#379` cluster that belonged with it was folded in during the
same pass rather than left orphaned in the parent.

The one external cross-reference was retargeted:
`crates/pycc_types/src/module/tests.rs` now names
`tests/generic_method_instantiation.rs` as the home of
`check_and_resolve_rejects_generic_class_instantiate_for_non_generic_class`.

## Verification

The correctness proof for this issue class is unchanged coverage plus an
unchanged test total: a pull request that reduces the measured line or region
count is an extraction that lost tests. Both held.

`#[test]` lineage total: **1,337 before and after** (parent 1,045 → 975,
children 292 → 362), counted at column 0 only (`grep -c '^#\[test\]'`) so the
prose at `tests.rs:13467` cannot inflate it. A sorted line-multiset comparison
of `tests.rs` plus every file under `tests/`, before against after, differs only
by module doc headers, `use super::*;` lines, `mod` declarations, relocated
banners and a handful of deliberate comment rewordings — no test-body line
differs. Banner integrity was checked at every cut seam; the first surviving
item below the big removal is a real banner
(`// -- PEP 591 (#383): Final[X] reassignment diagnostic T0045 ------------`).
All 58 relocated test names were searched tree-wide to confirm each landed
exactly once.

Gates on the review-clean head, each run so its exit status survived
(`cmd > log 2>&1; echo $?`):

| gate | exit |
|---|---|
| `cargo fmt --check` | 0 |
| `cargo build --workspace` | 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `cargo test -p pycc_types` | 0 (1,625 passed) |
| `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` | 0 (100.00% of 55,928 lines / 2,672 functions / 37,050 regions) |
| `python3 -B scripts/validate_agent_assets.py` | 0 |
| `ruby scripts/check_ci_permissions.rb` | 0 |
| `ruby scripts/check_roadmap_evidence.rb` | 0 (under `LC_ALL=en_US.UTF-8`) |
| `python3 -B scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check` | 0 |
| `python3 -B scripts/check_harden_findings.py .harden/findings/issue-695.jsonl` | 0 |

The coverage figures are byte-identical to the pre-change baseline, which is
the proof that no test was lost. The coverage gate ran after the same
preparatory builds CI performs: `cargo build --target x86_64-apple-darwin -p
pycc_rt`, `cargo build --workspace`, `cargo build --release -p pycc_rt`.
`scripts/check-site.sh` is not needed — the diff touches no document listed in
`site/llms-txt-context-manifest.json`. `python3 -B -m unittest discover -s
scripts` exits 1 on only the known machine-local
`test_check_harden_findings.FindingsCheckerTests.test_real_repository_piles_conform`,
which fails on ignored-but-untracked `.harden/findings/` files.

`ruby scripts/check_roadmap_evidence.rb` exits 1 with `invalid byte sequence in
US-ASCII` under a C locale — a locale artifact, not a gate failure; the diff
touches no roadmap file.

## Review

The D-068 pinned local reviewer ran three rounds over the full committed range
from the merge base. Round 1 raised a cohesion finding: the three tests
covering `is_assignable`'s `Ty::Param` clause at line 3229 had been split
across two children, each carrying its own banner citing the same source
region. Fixed in `c00ad0d7` by consolidating them into
`tests/generic_method_dedup.rs` under that file's existing banner. Round 2
raised two doc-accuracy findings against the new headers, both fixed in
`5447e0f5`: `generic_class_instantiation.rs` claimed unqualified that it holds
the tests driving the full `check_and_resolve` → `monomorphize` →
`instantiate_generic_class_methods` pipeline, when its last two stop at `check`;
and the round-1 fix's own wording dropped the `(line 3229)` qualifier its
adjacent banner carries, so it read as covering the distinct `from == Ty::Param`
clause at line 3240 that lives in `tests/generic_monomorphization_arms.rs`.
Round 3 was mandated to check exactly that risk — that a header fix can itself
overclaim — and returned no actionable findings, terminating the loop. All five
compiler gates were re-run green on the round-3 head.

Three reviewer observations were deliberately deferred with recorded reasons,
all because acting on them means moving tests this increment does not otherwise
touch: the `class not in class_defs` continue-path theme appearing in two
children under different framings; `generic_monomorphization_arms.rs` holding
two protocol-specialization tests and three `bind_local_types_in_stmt_*` tests
despite its filename; and a pre-existing banner/header gap at
`generic_method_dedup.rs:99-142` that the extraction moved verbatim rather than
created. They are the next increment's candidates and are recorded in the
narrowing comment on #695.

## Hardening (`/harden batch`)

The three findings this run produced were batched into two classes.

Class A (the cohesion finding) is a first occurrence and terminated at **build
nothing**, with the reason measured rather than asserted: in the directory the
finding came from, 5 line-qualified banners have 0 same-line duplicates, so a
duplicate-banner gate has no signal; tree-wide, 173 banners across 138 distinct
texts include three legitimate duplicates in frozen code, so such a gate would
fire false. New topic folder
`.harden/incidents/extraction-splits-one-code-arms-test-set/`, counter seeded,
`verdict: pending`.

Class B (both doc-drift findings) is the fifth occurrence of
`doc-comment-overclaims-unqualified-scope`, so the full ladder ran. Gap type
`trigger`: round 4 *did* catch it, but nothing fired at authoring time. It
terminated at `.claude/skills/issue-implement/SKILL.md`'s D-185 block — the fork
where a child module's header is actually written — with a paragraph requiring
an extraction's doc comments to be checked against what the file holds *and*
against sibling files' claims about the same subject, quantifiers enumerated or
dropped, and a narrowing edit forbidden from shedding a qualifier its
neighbourhood already carried. `.agents/skills/issue-implement/SKILL.md` is a
thin delegating pointer, so the one edit binds both platforms.

A weaker static rung was falsified by measurement, not judgment:
`tests/generic_monomorphization_arms.rs`'s header correctly omits a qualifier
its banner carries `(line 3240)`, so a "header must carry its banner's line
qualifier" gate would fail on correct code this very branch touches. The
governance file is disqualified by the three-files-in-one-topic rule.

The arena verdict is `pending` on both entries: `scripts/arena.py` is a
multi-agent campaign and this run was executed under a no-subagents constraint,
so no verdict was synthesized rather than one being invented.

**Recorded coverage boundary:** the artefact governs the *extraction* arm only.
The four prior occurrences of this class are prose written or amended under
review pressure outside any decomposition; that arm stays uncovered and its
counter keeps running.

**Sweep:** all 41 files under `crates/**/src/tests/` carry a `//!` header, 20 of
them quantified. One hit, `crates/pycc_mir/src/tests/slice.rs:3`, claims "every
present/absent combination of the start, stop, and step bounds" and holds 6 of
8. It is frozen code outside #695's file set and cannot cause an incorrect merge
decision or hide a compiler defect, so under D-192's filing bar it is a journal
line, not an issue. Three other enumerable claims (`exception.rs`,
`collection.rs`, `protocol.rs`) were read but not individually enumerated; the
incident entry says so rather than reporting a clean sweep.

## Follow-ups

- `crates/pycc_types/src/tests.rs` is still 21,431 lines. #695 remains open for
  further cohesion-driven extractions; the three deferred reviewer observations
  above are the next increment's candidates.
- The `// -- ` prose false positive (`// -- an undeclared/global name would
  instead resolve silently via the`) is a wrapped sentence, not a banner. A
  future extraction in that region must not treat it as a cut boundary. Its line
  number moves with every extraction — a prior session's entry records it at
  23745 and this branch's plan at 22153; it now sits at 18818, so re-derive it
  by text rather than trusting any recorded number.
- `crates/pycc_mir/src/tests/slice.rs:3`'s over-quantified header, per the sweep
  above.
- Still out of scope from earlier runs: extending
  `scripts/check_source_links_registry.rb` to cover `readme_entities`' URLs;
  `docs/WEBSITE.md`'s two stale "#202's domain" deferrals; the #989 doc drift
  where `docs/TYPE_SYSTEM.md` and the ROADMAP entry still claim `Box.LIMIT`
  diverges although `5b238d8c` fixed it.

## Paused autopilot

- **Directive scope:** open-ended (`/goal fix all opened issues`), milestone
  scope **v0.4**.
- **Active milestone:** v0.4.
- **Last iteration outcome:** #695 narrowed by one more partial-decomposition
  pull request; the issue stays open by design.
- **Next autopilot step:** re-enter `issue-select` scoped to v0.4, after
  `next-milestone`'s step-2 evidence check.
- **In-run denylist:** empty.

## Where to resume

Read the most recent files under `docs/sessions/` in filename order, then
`gh issue view 695 --repo rotnov/pycc` for the narrowing comments recording what
each merged pull request removed.
