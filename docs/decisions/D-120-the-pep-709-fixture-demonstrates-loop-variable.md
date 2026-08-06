---
id: D-120
title: "The PEP-709 fixture demonstrates loop-variable non-leakage, not bytecode inlining; container `to_str`/`truthy` stay explicitly out of scope"
status: accepted
---

## D-120: The PEP-709 fixture demonstrates loop-variable non-leakage, not bytecode inlining; container `to_str`/`truthy` stay explicitly out of scope

- Status: accepted
- Context: the design doc's own PEP table (§2) assigns PEP 709 ("Comprehension inlining semantics") to this PR without specifying what a pycc conformance fixture for it should actually assert -- CPython 3.12's real PEP 709 change is an *implementation* detail (comprehensions stop running in an implicit nested function/frame, for a performance win) that pycc, an AOT compiler with no bytecode, no frames, and no `locals()`, has no analog for; the design doc's own PEP-649/749 row already established the precedent that "not empirically reachable given this architecture" is a real, first-class finding to record rather than force a fixture that doesn't actually test anything. The one CPython-observable, PEP-709-relevant behavior that *is* both real and statically testable in pycc's current scope is that a comprehension's own loop variable does not leak into or clobber an enclosing binding of the same name -- true both before and after PEP 709 (comprehensions have had their own scope since their introduction; PEP 709 preserves this while changing how it's implemented), and, per D-117's synthesized-name mechanism, now genuinely exercised by this compiler for the first time (an ordinary `for` loop, by contrast, *does* leak/overwrite, matching CPython's own bare-`for` behavior, confirmed already true of `ForRange`/`ForList` before this PR). Separately: printing a container value directly (`print(a_list)`) still panics in `pycc_codegen`'s `to_str` (D-107/D-124/D-116's own already-shipped honest panics), so a fixture asserting comprehension output must print element-wise, never the container itself.
- Decision: `tests/fixtures/pep_0709_comp_inline.py` asserts loop-variable non-leakage, not literal bytecode-inlining behavior:
  ```python
  i = 100
  xs = [i * 2 for i in range(3)]
  print(i)
  for v in xs:
      print(v)
  ```
  Real CPython prints `100` (the outer `i` survives untouched) followed by `0`, `2`, `4` (the comprehension's own result) -- if this compiler's comprehension lowering used the *source* loop-variable name directly instead of D-117's synthesized name, the shared flat-namespace slot model would make the outer `i` end up overwritten (to `2`, the loop's final value) exactly like a bare `for i in range(3):` already does, and this fixture would fail to match CPython. This is a real, non-trivial, CPython-verified assertion this compiler's own architecture makes genuinely easy to get wrong, not a token gesture at the PEP's name.
- Alternatives: assert on `locals()`/frame-introspection differences (rejected outright -- neither exists in this compiler at all, and CPython's own PEP 709 change is precisely about *not* creating a frame, which pycc never did for comprehensions to begin with, since it never runs comprehensions as nested functions in the first place). Use the walrus operator (`:=`) inside the comprehension, since its interaction with enclosing scope is the single most distinctive real PEP-709-adjacent CPython behavior (rejected -- pycc has no assignment-expression support at all, in or out of a comprehension, so this is not reachable at any scope this compiler currently has). Print the resulting container directly instead of element-wise (rejected -- `to_str` on a container panics today by design (D-107/D-124/D-116); a fixture that reached that panic would fail to compile at all, not merely fail to match CPython).
- Consequences: this decision also reaffirms, in one place, that container `to_str`/`truthy` remain unimplemented after this PR -- not a silent gap this plan's own fixtures could be mistaken for having closed. `docs/ROADMAP.md`'s v0.2 corpus acceptance bullet (PR-14's own scope) needs the same element-wise-printing discipline for the same reason, flagged there as a follow-up (Task 13).

