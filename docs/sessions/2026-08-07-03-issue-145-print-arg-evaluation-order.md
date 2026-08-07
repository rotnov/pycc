# Session 2026-08-07-03 — Issue #145: Print argument evaluation order

## Goal

Fix #145: `print(1, side_effect())` produces output in the wrong order (pycc: `1 2\n3\n`, CPython: `2\n1 3\n`). The print codegen interleaves argument evaluation and output instead of evaluating all arguments first.

## Context

- Standing goal: v0.3 milestone (classes & pattern matching)
- issue-select selected #145 as the top P1 candidate (smallest independent fix, no PR collision, no D-103 protection)
- Plan published at https://github.com/rotnov/pycc/issues/145#issuecomment-5216960311
- PR #358 merged during planning, shifting line numbers in pycc_codegen by ~200 lines; task branch rebased

## Work completed

- Split `emit_print_arg` into `emit_eval_print_arg` (phase 1: evaluate all args) and `emit_write_print_arg` (phase 2: write all args with separators)
- Modified the print arm in `emit_stmt` to use two sequential loops
- Added codegen-level regression test for side-effect reordering
- Added codegen-level regression test for failing-later-argument (no partial output)
- Added `tests/regress/issue_145.py` permanent fixture
- Added end-to-end test in `tests/slice1_codegen_depth.rs`
- D-068 deep review: 2 rounds (round 1 found 1 stale reference warning, round 2 clean)

## Branch

`fix/issue-145-print-argument-evaluation-order` based on `main@f4b3978`

## Next

- Open PR for #145
- After merge, continue issue-select loop for v0.3
