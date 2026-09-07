# 2026-09-07 (06) — #695: six more test clusters out of `pycc_types` `tests.rs`

## Overall status

Task base: `0cc4e0ce` (`origin/main`, "Extract the constraint-collection tests
into their own module (part of #695) (#995)"). Branch:
`autopilot/iter-2026-09-07-32`.

This session delivered one pull request against
[#695](https://github.com/rotnov/pycc/issues/695), the D-185 tracking issue for
the oversized `crates/pycc_types/src/tests.rs`. Six cohesion-driven test
clusters were relocated verbatim into new sibling child modules under
`crates/pycc_types/src/tests/`, following the sibling-`tests.rs`-plus-`tests/`
layout (no `mod.rs`) that PR #995 established with `tests/constraints.rs`.

`#695` stays open: at 24,783 lines the file is still far above the ~1,000-line
decomposability threshold, so it is narrowed by comment after each merge rather
than closed.

## What was extracted

| new file | lines | `#[test]` |
|---|---|---|
| `crates/pycc_types/src/tests/typing_cast.rs` | 526 | 31 |
| `crates/pycc_types/src/tests/pattern_matching.rs` | 559 | 43 |
| `crates/pycc_types/src/tests/exception_handling.rs` | 322 | 26 |
| `crates/pycc_types/src/tests/optional_narrowing.rs` | 559 | 36 |
| `crates/pycc_types/src/tests/type_checking_marker.rs` | 124 | 6 |
| `crates/pycc_types/src/tests/enum_unrolling.rs` | 115 | 5 |

`crates/pycc_types/src/tests.rs`: 26,906 → 24,783 lines. The first four
extractions removed 2,168 lines, 2,164 of them reproduced verbatim in the new
children (one trailing blank line was dropped at each of the four cut
boundaries), against six lines added in the parent — four `mod` declarations,
one net line in the child-module comment, and one net line in the reworded
`#382` banner. The review round below then moved two strays back *into* the
parent (+39 lines: 36 test lines, one blank line, and the two further `mod`
declarations), leaving 24,783. Its own `#[test]` count went 1,192 → 1,045.

The child-module declaration block in `tests.rs` gained `enum_unrolling`,
`exception_handling`, `optional_narrowing`, `pattern_matching`,
`type_checking_marker` and `typing_cast`, kept alphabetical, and the comment
above it was updated so it still honestly describes which clusters now live in
child files.

## Deviation: the exception-handling helpers stay in the parent

The `// -- #382 exception handling tests --` run opens with four non-test
helpers — `parse_check_resolve`, `parse_check`, `expect_top_level_try` and
`expect_top_level_raise`. `parse_check_resolve` and `parse_check` are called
from roughly forty tests that remain in `tests.rs`, and a parent module cannot
see a child module's private items, so relocating them verbatim would not
compile. The helpers therefore stayed in `tests.rs` under a reworded banner
that points at the new child module, and only the tests below them moved.
`expect_top_level_try`/`expect_top_level_raise` are now used exclusively from
the child; that is still a real use for dead-code analysis, which the clean
`clippy -D warnings` run confirms. The fork was resolved with this session's
advisor per D-127: shared helpers belong at the level that shares them, and
pushing them into a child would invert the dependency and require widening
visibility on relocated code.

## Verification

Banner integrity (the recurring defect class for this issue) was checked on
every cut, before and after removal: each range started at a real `// --`
banner preceded by a closing `}` and a blank line; no interior `// --` run
inside any moved span (each span contained exactly one, its own leading
banner); and the first surviving item below each seam still carries its own
banner — `// -- #380 W1: protocol monomorphization ...`,
`// -- direct unit tests for defense-in-depth paths ...`,
`// -- #382 coverage tests --` and
`// -- #911 (Part 1 of #885): class-level attributes ...`.

Test-count invariant: the `#[test]` total across `tests.rs` plus every file in
`tests/` is 1,337 both before and after (145 in the pre-existing five children,
292 after the six new ones). `cargo test -p pycc_types` reports 1,625 passing
tests before and after, unchanged.

Gates, each run so its exit status survived (`cmd > log 2>&1; echo $?`):

| gate | exit |
|---|---|
| `cargo fmt --check` | 0 |
| `cargo build --workspace` | 0 |
| `cargo test -p pycc_types` | 0 (1,625 passed) |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100` | 0 (100.00% lines and regions, 55,928 lines / 37,050 regions) |

The coverage gate ran after the same preparatory builds CI performs:
`cargo build --target x86_64-apple-darwin -p pycc_rt`, `cargo build --workspace`
and `cargo build --release -p pycc_rt`.

## Review

The D-068 pinned local reviewer (iEvo `deep-reviewer`) reviewed the full
committed range from the merge base through `d4c396a3` and returned a single
`note`-severity finding: the retained `expect_top_level_try` /
`expect_top_level_raise` doc comments still named their panic-arm coverage
tests unqualified, even though those tests had moved into the child module.
Commit `12304fbb`, the head that finding was raised against, qualifies both
cross-references with the `exception_handling::` path. No P0/P1 and no other
actionable finding was raised. `cargo fmt --check` exits 0 on that head, and
the commit changes only two doc-comment lines in place, so every line and test
count above is unaffected by it.

The automated pull-request reviewer then raised three P2 cohesion findings on
the pull request, all resolved in this branch. (1) A stale cross-reference:
`crates/pycc_types/src/expr.rs` named `tests.rs` as the home of
`cast_without_its_import_is_currently_accepted`, which had moved — the comment
now names `tests/typing_cast.rs`, rewrapped in place. (2) Two coverage-driven
strays, `protocol_argument_mismatch_emits_t0046` and
`check_skips_abstract_method_body_checking`, had ridden along into
`tests/typing_cast.rs` despite testing nothing about `typing.cast`; they are
back in `tests.rs` at the end of the `// -- direct unit tests for
defense-in-depth paths` run, which holds exactly that kind of test. The same
finding noted that the six #790 `typing.TYPE_CHECKING` marker tests are a
separate cluster from the #767 `typing.cast` tests; they are now
`tests/type_checking_marker.rs`. (3) Five enum-loop unrolling tests in
`tests/exception_handling.rs` exercise `unroll_enum_loops` recursing through
ordinary statement nesting, not exception handling; they are now
`tests/enum_unrolling.rs`. The two `try`-flavored enum tests above them and the
non-enum `for` test below them stay in `exception_handling.rs`, whose module
doc remains accurate — every helper it names is still called from it.
`parse_check_resolve` is now called from two child modules, so the parent's
`// -- #382 exception handling test helpers` banner was reworded to name both;
that is the only deliberately altered line of prose in the move. Every relocated
test moved verbatim: a sorted line-multiset comparison of `tests.rs` plus all
children, before against after, differs only by the two new module doc headers,
their two `use super::*;` lines, the two new `mod` declarations, the two new
banners in the new files, that one reworded banner, and four blank lines. The
`#[test]` total is 1,337 in both, and all five gates above were re-run green on
the result.

A second D-068 pinned-reviewer pass over the whole range then raised two
doc-accuracy findings, both of the same reattached-prose class this issue keeps
producing, and both fixed: the parent's child-module orientation comment still
enumerated five #695 clusters after this branch had created seven, and three
comments in the new `tests/type_checking_marker.rs` named their `typing.cast`
counterparts unqualified even though those tests stayed behind in
`tests/typing_cast.rs`. It also observed that this file's own slug said "four
clusters" while its contents describe six; the file was renamed accordingly
before the first merge, so no committed session entry was edited. That pass
found nothing else actionable and confirmed every quantitative claim here
against the tree.

## Follow-ups

- `crates/pycc_types/src/tests.rs` is still 24,783 lines. #695 remains open for
  further cohesion-driven extractions; the next obvious candidates are the
  remaining large banner runs in the 13k–20k region.
- The `// -- ` prose false positive at what was line 23745 (`// -- an
  undeclared/global name would instead resolve silently via the`) is a wrapped
  sentence, not a banner. A future extraction in that region must not treat it
  as a cut boundary — this is why the candidate cluster around it was dropped
  here.
- `expect_top_level_try` and `expect_top_level_raise` now have no remaining
  call site in `tests.rs` itself — only `parse_check` and `parse_check_resolve`
  genuinely had to stay behind. A later exception-handling extraction in this
  region can move those two helpers down into the child module with the tests
  that use them.

- Selection flagged [#355](https://github.com/rotnov/pycc/issues/355)
  ("Ultra-review checkpoint — do not close") as an apparent D-192
  milestone-at-filing gap. Verdict, recorded here rather than acted on: it is
  not tracked work. Its body is an owner-authored, marker-delimited state record
  (`<!-- ultra-review-checkpoint -->`) that the ultra-review tooling parses by
  that marker, so D-192's filing rule has no purchase on it, and every
  conformance-restoring mutation — assigning a milestone, retitling it into an
  umbrella, folding it into #806 — would either state a fiction or break the
  parser. #806 already holds the agent-tooling umbrella slot, so a second one is
  not an option either. Nothing turns on the classification: 10 non-milestone
  issues are open against D-192's ceiling of 20.

## Where to resume

Read the most recent files under `docs/sessions/` in filename order, then
`gh issue view 695 --repo rotnov/pycc` for the narrowing comments recording
what each merged pull request removed.
