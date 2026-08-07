# Session 2026-08-07-02 — Issue #162 Part 1: claim/source model

## Goal

Implement Part 1 of issue #162 (Validate the factual claims in the source-backed compiler comparison): create a structured claim/source model (`claims.json`), extend the website validator to bind HTML table cells to the model, add mutation tests, and update documentation.

## Context

- Standing goal: v0.3 milestone (classes & pattern matching)
- Issue-select identified #162 as the top P1 issue after a systemic CI block (D-103 manifest mid-transition) was resolved via ci-temporary-bypass (PR #393 merged, audit check relaxed and restored)
- #162 was decomposed into 4 parts per AGENTS.md's multi-seam rule; sub-issues #395 (Part 2), #396 (Part 3), #397 (Part 4) opened
- Plan published at https://github.com/rotnov/pycc/issues/162#issuecomment-5215996690

## Work completed

- Created `site/python-aot-compilers/claims.json` with 6 entity records (pycc, LPython, Codon, Nuitka, mypyc, Cython)
- Fixed mypyc maturity omission (added "Alpha" label to positioning cell)
- Extended `scripts/check-site.sh` with claim/source binding validation (table parsing, entity set match, cell value comparison, source URL presence, maturity non-empty)
- Added 15 new mutation tests to `scripts/test-check-site.sh`
- Updated `docs/WEBSITE.md` with claim model documentation
- Updated `docs/ROADMAP.md` discoverability row
- D-068 deep review: 2 rounds (round 1 found 2 date-mismatch warnings, round 2 clean)

## Branch

`fix/issue-162-comparison-claim-validation` based on `main@1f40796`

## Next

- Open PR for Part 1
- After merge, plan and implement Parts 2-4 (#395, #396, #397)
