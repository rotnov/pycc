# 2026-09-05 — #937 residual: README and D-229 still described the PEP 563 flip as pending

## Context

[#937](https://github.com/rotnov/pycc/issues/937) was delivered by PR
[#939](https://github.com/rotnov/pycc/pull/939) (merged as `a275fbaa`) from a
parallel session while this session's own implementation of the same issue
was in review. This session's branch was reset onto that `main` and narrowed
to the two factual corrections #939 did not carry; see the 2026-09-05
`docs/AGENT_RETROSPECTIVE.md` entry "Implemented an issue a parallel session
was already delivering" for the process lesson.

## What this pull request changes

- `README.md`: the v0.3 sentence said conformance "reaches 38 rows/39 PEPs"
  in the present tense; after #939 the matrix has 39 rows at `◐` or better
  encompassing 40 distinct PEP numbers (`scripts/check_conformance_breadth.py`
  on `a275fbaa`). The sentence now records 38/39 as the figure reached at
  v0.3's acceptance and 39/40 since #937.
- `docs/decisions/D-229-reserve-from-future-import-as-a-compile-time-directive.md`:
  the consequence bullet said the row "stays `☐` under rule 5 until #937
  flips it"; a dated inline update records that #937 flipped it in #939 after
  `main` run 33972731538. The decision's substance is unchanged.
- `.harden/findings/issue-937.jsonl`: the D-068 review findings from this
  session's full (now superseded) implementation, kept because both findings
  were real and one of them (the stale comment in `tests/conformance/classes.rs`)
  #939 also fixed independently.
- `docs/AGENT_RETROSPECTIVE.md`: the duplicate-work entry.

No Rust change. This pull request closes no issue: #937 is already closed.

## Where a fresh session should look

`origin/main` after this merge; the next `issue-select` pass starts from the
v0.4 milestone list with #937 gone. Candidates seen during this iteration,
smallest first: #921 (Enum instantiation panics instead of diagnosing) and
#931 (subscripted annotation with an unrecognized base silently discards its
type arguments).
