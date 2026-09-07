# 2026-09-07 (06) — #695: four more test clusters out of `pycc_types` `tests.rs`

## Overall status

Task base: `0cc4e0ce` (`origin/main`, "Extract the constraint-collection tests
into their own module (part of #695) (#995)"). Branch:
`autopilot/iter-2026-09-07-32`.

This session delivered one pull request against
[#695](https://github.com/rotnov/pycc/issues/695), the D-185 tracking issue for
the oversized `crates/pycc_types/src/tests.rs`. Four cohesion-driven test
clusters were relocated verbatim into new sibling child modules under
`crates/pycc_types/src/tests/`, following the sibling-`tests.rs`-plus-`tests/`
layout (no `mod.rs`) that PR #995 established with `tests/constraints.rs`.

`#695` stays open: at 24,744 lines the file is still far above the ~1,000-line
decomposability threshold, so it is narrowed by comment after each merge rather
than closed.

## What was extracted

| new file | lines | `#[test]` |
|---|---|---|
| `crates/pycc_types/src/tests/typing_cast.rs` | 674 | 39 |
| `crates/pycc_types/src/tests/pattern_matching.rs` | 559 | 43 |
| `crates/pycc_types/src/tests/exception_handling.rs` | 420 | 31 |
| `crates/pycc_types/src/tests/optional_narrowing.rs` | 559 | 36 |

`crates/pycc_types/src/tests.rs`: 26,906 → 24,744 lines, a net removal of
2,162: 2,168 lines were removed, 2,164 of them reproduced verbatim in the new
children (one trailing blank line was dropped at each of the four cut
boundaries), against six lines added in the parent — four `mod` declarations,
one net line in the child-module comment, and one net line in the reworded
`#382` banner. Its own `#[test]` count went 1,192 → 1,043.

The child-module declaration block in `tests.rs` gained `exception_handling`,
`optional_narrowing`, `pattern_matching` and `typing_cast`, kept alphabetical,
and the comment above it was updated so it still honestly describes which
clusters now live in child files.

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
294 after the four new ones). `cargo test -p pycc_types` reports 1,625 passing
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
Commit `12304fbb` qualifies both cross-references with the
`exception_handling::` path; it is the pull request's delivering head. No P0/P1
and no other actionable finding was raised. `cargo fmt --check` exits 0 on that
head, and the commit changes only two doc-comment lines in place, so every line
and test count above is unaffected by it.

## Follow-ups

- `crates/pycc_types/src/tests.rs` is still 24,744 lines. #695 remains open for
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
