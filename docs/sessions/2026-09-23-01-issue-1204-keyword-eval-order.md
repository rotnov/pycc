# 2026-09-23-01 — #1204: keyword binding reordered side-effecting argument values

## Status

Branch `issue-1204-keyword-eval-order` holds the fix for #1204. It was cut
from `main` at `98f75d8b`. The fix is committed on the branch, but when this
snapshot was written it had not been pushed, had no pull request and had
not reached `main`. Read the branch head, the pull request state and CI
results from git and GitHub rather than from this file.

## Why this snapshot exists

D-242 rule 5 gates `docs/sessions/` on an incident. This is one, because a
defect reached `main`. Parts 1 and 2 of #884 (#1125, and #1189 / PR #1196)
lower a keyword call to a positional vector in parameter order. That vector
is evaluated left to right, so `f(b=g(2), a=g(1))` printed `1 2 3` where
CPython prints `2 1 3`. The process lesson is in
`docs/AGENT_RETROSPECTIVE.md` under 2026-09-23, "Keyword binding moved
side-effecting argument values without an evaluation-order rule".

## The fix on this branch

`keyword_bind::is_bindable_call`, the single predicate for every keyword
call path, now also asks `keyword_bind/eval_order.rs` whether binding
would observably reorder an argument value. A call whose values land out of
parameter order keeps the unchanged `C0001` rejection, unless every value is
a literal or a bare name. The canonical rule is in `docs/TYPE_SYSTEM.md`,
"Keyword argument evaluation order". The end-to-end tests are in
`tests/issue_1204_keyword_eval_order.rs`. The temporaries alternative, which
evaluates out-of-order values into locals first, was considered and not
adopted.

## In flight

- The #1204 branch still has to be pushed, reviewed and merged.

## Known follow-ups

- #1191 (Part 4 of #884, keyword arguments and defaults for method calls,
  `super()` and instantiation): the plan draft is parked at an impasse after
  five review rounds, with only P2 findings remaining. It already adopts
  this evaluation-order rule for constructor calls, so it should be
  re-checked against `docs/TYPE_SYSTEM.md`'s canonical statement once this
  branch merges.
- #1190 (Part 3 of #884, keyword arguments and defaults for an imported
  `def`): plan v3 is parked. Its binding must apply the same rule.
- #1205 (v0.4) was filed: constructing a generic class without a type
  argument panics in codegen.

## Where to resume

- `crates/pycc_hir/src/expr/keyword_bind.rs` (`is_bindable_call`) and its
  `eval_order` submodule
- `tests/issue_1204_keyword_eval_order.rs`
- #1191's and #1190's parked plan drafts, which are unpublished because an
  impasse forbids publishing; re-run `issue-to-plan` on each with this rule
  stated in the brief
