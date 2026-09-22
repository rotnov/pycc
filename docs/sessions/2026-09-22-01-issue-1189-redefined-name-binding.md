# 2026-09-22-01 — #1189 review round: a redefined name was bound against the wrong signature

## Status

Branch `issue-1189-default-params` (Part 2 of #884, default parameter values
on a module-level `def`) is not yet merged. Its base is `main` at
`53c3ed84`. The fix described below is committed on the branch; nothing in
this snapshot has reached `main`. Read the branch head, the pull request
state and the CI results from git and GitHub rather than from this file.

## Why this snapshot exists

D-242 rule 5 gates `docs/sessions/` on an incident, and this is one: a
defect reached `main`. Part 1 of #884 (issue #1125, commit `a600ec3f`)
bound keyword call arguments against a static, module-wide `SignatureTable`
in which a later `def` of a name replaced an earlier one. pycc dispatches a
redefined module-level name in source order (issue #22), so a keyword call
made before the redefinition was bound against the later `def`'s parameter
order:

```python
def foo(a: int, b: int) -> None:
    print(a - b)
foo(a=10, b=1)
def foo(b: int, a: int) -> None:
    print(a - b)
foo(a=10, b=1)
```

`main` prints `-9` then `9`; CPython prints `9` twice. Part 2 inherited the
same table for default filling (`def foo(a: int = 1)`, `foo()`, then
`def foo(a: int = 2)`, `foo()` printed `2 2` for CPython's `1 2`). The Part 2
review round caught both. The process lesson is in
`docs/AGENT_RETROSPECTIVE.md` (2026-09-22, "A static, module-wide signature
table bound calls to a name the runtime dispatches in source order").

## The fix on this branch

`SignatureTable::collect` now admits only a `def` whose name is bound exactly
once in module scope. `crates/pycc_hir/src/expr/keyword_bind/rebound.rs`
counts every module-scope binding form. A redefined name's keyword call keeps
`C0001` "keyword call arguments are not supported yet". A short call to it
gets `pycc_types`' ordinary `T0021` arity error. A fully positional call
still dispatches in source order. The canonical rule is in
`docs/TYPE_SYSTEM.md`, "Keyword arguments and default parameter values on a
redefined name". Positional-source-order dispatch is not a sound basis for
binding, because a call inside a function body runs at a time its source
position does not fix, so the rule withdraws binding outright instead of
choosing a `def` by position.

## Known follow-ups

Probing the binding forms turned up two pre-existing wrong-output defects.
Neither involves keyword binding or default filling, both involve fully
positional calls, and neither is fixed here:

- `def sqrt(a: float = 4.0) -> float`, then `from math import sqrt`, then
  `print(sqrt(9.0))` prints `9.0`. The later import does not rebind the name
  in pycc; CPython prints `3.0`. The unsupported bare `from math import sqrt`
  is #1157; the repro is recorded there as a comment (it also reproduces
  with a `def sqrt` that has no default).
- A module-level `match 1:` with `case foo:` does not rebind a
  zero-parameter `def foo() -> None`, so a later `foo()` still calls the
  function. CPython raises `TypeError` because `foo` is then the integer
  `1`. Filed as #1195 (v0.4).

Both reproduce on this branch's rebuilt binary exactly as written.

## Where to resume

- `crates/pycc_hir/src/expr/keyword_bind.rs` and its `rebound` submodule
- `tests/issue_1189_default_params.rs`: the three redefinition tests
- `tests/issue_22_execution_order.rs`: source-order dispatch of a redefined
  `def`
