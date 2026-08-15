---
id: D-171
title: "Merged-instantiation coverage gate"
status: accepted
---

## D-171: Merged-instantiation coverage gate

- Status: accepted
- Supersedes: D-014 (gate mechanism only — the 100% coverage *requirement* is unchanged)

### Context

`llvm-cov`'s summary statistics (used by `cargo llvm-cov --fail-under-lines 100
--fail-under-regions 100`) count regions and lines **per instantiation**.  When
a generic Rust function is monomorphized into multiple instantiations, a
source region that is covered by at least one instantiation but not by all of
them appears as "missed" in the summary even though the merged (HTML / LCOV)
view shows it as fully covered.

This was discovered during issue #382 (exceptions) implementation: the
`llvm-cov` summary reported 7 missed lines and 19 missed regions in
`pycc_types/src/lib.rs` and `pycc_codegen/src/lib.rs`, but:

1. The HTML report (`cargo llvm-cov --html`) showed **0 uncovered lines**
   across all files.
2. The LCOV output had **0 zero-count DA records** (line coverage entries).
3. The `llvm-cov export` JSON's file-level `segments` array had **0 segments
   with `hasCount=True` and `count=0`**.
4. When function-level regions were merged across all instantiations, **0
   unique source regions had all-zero counts**.

The per-instantiation summary therefore overcounts misses and can report
< 100% coverage for code that is, in fact, fully covered.  This makes the
`--fail-under-lines 100 --fail-under-regions 100` gate unreliable for codebases
that use generics heavily (which pycc does — the type checker, solver, and
code generator are all generic over `Environment` and other parameters).

### Decision

Replace the `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`
gate with a two-step process:

1. Generate the `llvm-cov export` JSON:
   ```sh
   cargo llvm-cov --workspace --json --output-path "$TMPDIR/cov.json"
   ```

2. Run `scripts/check_coverage_merged.py` on the JSON:
   ```sh
   python3 scripts/check_coverage_merged.py "$TMPDIR/cov.json"
   ```

The script computes coverage the same way the HTML report does — by merging
across all instantiations:

- **Line coverage**: a line is missed when *every* segment on that line
  (across all instantiations) has count 0.
- **Region coverage**: a region is missed when *every* instantiation of that
  source region has count 0.

Test files (`tests/`, `*_tests.rs`, `tests.rs`) are excluded from the check,
matching `cargo-llvm-cov`'s default denominator.

The 100% coverage *requirement* from D-014 is unchanged — every executable
line and region of product code must still be covered by at least one test.
Only the *measurement mechanism* changes: from per-instantiation summary to
merged-instantiation detail.

### Alternatives

- **Lower the threshold** (e.g., `--fail-under-lines 99.99`): rejected — D-014
  explicitly rejected percentage targets below 100%, and the phantom misses
  are a tooling artifact, not a real coverage gap.

- **Add tests to cover phantom misses**: rejected — the HTML report shows 0
  uncovered lines, so there is nothing to test.  Adding contrived tests solely
  to exercise every instantiation of every generic function is impractical and
  would not fix the root cause (per-instantiation counting).

- **Restructure code to avoid generics**: rejected — the type checker, solver,
  and code generator are generic by design (D-006 monomorphization).  Removing
  generics to work around a tooling limitation would be a major architectural
  regression.

- **Report upstream and wait**: the per-instantiation counting is a known
  behavior of `llvm-cov`'s summary, not a bug per se — the HTML report
  correctly merges.  An upstream fix would not help the current project, which
  pins `cargo-llvm-cov` 0.8.7 and `rustc` 1.97.1.

### Consequences

- The coverage gate now accurately reflects actual merged coverage.
- The 100% coverage invariant is maintained for real executable code.
- The gate is slightly more complex (two steps instead of one), but the
  script is small, tested, and self-documenting.
- If `cargo-llvm-cov` or `llvm-cov` is updated in the future and the summary
  statistics are fixed to merge instantiations, the script can be replaced
  with the simpler `--fail-under-lines 100 --fail-under-regions 100` gate.
