---
id: D-204
title: "Optional[T] flow-sensitive is/is not None narrowing (Part 2 of #747)"
status: accepted
---

## D-204: Optional[T] flow-sensitive is/is not None narrowing (Part 2 of #747)
- Status: accepted
- Context: D-197 (issue #763, Part 1 of #747) shipped `Optional[int]`
  presence testing (`is`/`is not None`) but deliberately deferred narrowing
  the value itself back to plain `int` inside a presence-tested branch,
  scoping that out as its own architectural seam (Part 2). Issue #769 is
  that Part 2. This entry records the resulting design: an overlay rather
  than a join-time mutation at the checker layer, a strict termination
  predicate shared with the MIR layer, and the deliberate scope cuts.

- Decision:
  1. **Narrowing lives in a side table, not in `bindings` itself.**
     `crate::env::Environment` gains a new `narrowed: HashMap<String, Ty>`
     field, populated only on a branch-local `Environment` clone (in-branch
     narrowing) or directly on the real `env` for the early-return
     continuation shape (item 3 below). `join_if_branches` (`lib.rs`) never
     reads or writes it, and `check_assignment`'s target-checking path never
     consults it (a name's real, un-narrowed type is what gets reassigned
     against). Only `crate::expr::infer_expr_in`'s `HirExpr::Name` arm reads
     it, via `Environment::narrowed_ty`, falling back to the real binding
     when no overlay entry exists. This keeps narrowing structurally
     incapable of leaking past an `if`/`else` join: there is no join step
     for a side table nothing else consults to leak through.
  2. **Scope is restricted to a top-level `if name is None:`/
     `if name is not None:` test.** No narrowing through a compound
     `and`/`or` test, and no narrowing through a test embedded in a larger
     boolean expression. The recognizer (`pycc_hir::optional_none_test`) is a
     pure syntactic match on `HirExpr::Compare` with one operand a bare
     `HirExpr::Name` and the other `HirExpr::NoneLiteral`; it does not
     descend into `BoolOp` at all.
  3. **A new strict `definitely_terminates` predicate, not the existing
     `contains_return`.** `constraints.rs::contains_return` answers "does a
     `return` occur *anywhere* in this body, including nested inside an
     unrelated inner `if`?" — true for `if flag: return 0` even when `flag`
     is `False` on some paths. Using that directly as a narrowing-eligibility
     guard for the early-return continuation shape

     ```python
     if x is None:
         if flag:
             return 0
     print(x + 1)
     ```

     would incorrectly narrow `x` after the outer `if` even though `x` can
     still be `None` when `flag` is `False`. `definitely_terminates` is
     strictly narrower: true only when a body's *last* statement is itself
     unconditionally terminating — a bare `return`, or an `if` whose `body`
     **and** non-empty `orelse` both recursively terminate. `raise` is
     deliberately *not* a terminator (a scope cut, not an oversight — see
     Consequences), and `match` is not analyzed at all (also a scope cut,
     kept sound by omission rather than by an exhaustiveness heuristic that
     could be wrong).
  4. **The syntactic recognizer and the termination predicate are shared via
     `pycc_hir`, not duplicated.** `pycc_mir` cannot depend on `pycc_types`
     (the dependency runs the other direction: `pycc_types` depends on
     `pycc_hir`, and `pycc_mir` also depends on `pycc_hir` but not on
     `pycc_types`). Both `optional_none_test` and `definitely_terminates`
     are therefore pure, environment-independent functions hosted in
     `pycc_hir`, consumed by `pycc_types::narrow` (as thin re-exports) and
     directly by `pycc_mir`. Each crate keeps its own
     environment/scope-specific *application* logic separate: the checker's
     `Environment.narrowed` overlay versus MIR's `$narrowed:{name}` scope
     sentinel (`crates/pycc_mir/src/lib.rs`'s `push_narrowing`/
     `kill_narrowing`/`narrowed_ty`), each consulting the shared recognizers
     to decide *when* to act.
  5. **MIR represents a narrowed read as a new `MirExpr::OptionalUnwrap`
     node**, the read-side counterpart of the existing `MirExpr::OptionalWrap`
     write-side node. Its `.ty()` reports the `Optional`'s inner type (always
     `Ty::Int` today, per D-197 item 3's `Optional[int]`-only codegen scope),
     fixing downstream slot-typing the same way `OptionalWrap` does for
     writes. Codegen emits it as a single `build_extract_value` on the
     `{ inner, present }` struct's payload field (field 0) — a borrowed read,
     not an ownership transfer, exactly mirroring a plain `Ty::Int`
     `MirExpr::Name` read: no retain at the unwrap site itself, refcount
     correctness comes entirely from the existing retain/release
     classification at the point the unwrapped value is actually stored or
     consumed (`bigint_rc.rs`).
  6. **MIR's narrowing state needs its own isolation mechanism distinct from
     the checker's clone-and-discard `Environment`.** MIR's `scopes: Vec<
     HashMap<String, Ty>>` is one shared, mutable, per-function frame
     (pushed once at function entry, never per-block), not a
     per-branch-clone like the checker's `Environment`. A direct, naive port
     of "push the sentinel before a narrowed body, pop it after" is
     correct *only* for the exact body it wraps — it does not, by itself,
     prevent a narrowing fact established *inside* that body (e.g. a nested
     early-return guard) from persisting in the shared frame past that
     body's own close. `pycc_mir::lower_scoped_body` closes this gap: it
     snapshots the frame's entire `$narrowed:` sentinel subset before
     lowering a *nested* body (an `if`/`while`/`for` arm, a
     `try`/`except`/`finally` handler, a `match` arm) and restores exactly
     that snapshot afterward, discarding anything the nested body's own
     lowering added — recreating the checker's clone-and-discard effect
     without actually cloning the rest of `scopes`. The *top-level* sequence
     of a function or module body is lowered directly through
     `lower_stmt_sequence` (no isolation), since narrowing established there
     is meant to persist for the remainder of that same sequence — exactly
     the early-return continuation shape's own point.

- Alternatives:
  - *Mutating the branch-local `Environment.bindings` entry directly*
    (rebinding `x` to `Ty::Int` inside the body) — rejected: `join_if_branches`
    would then either leak the narrowed type past the `if` or spuriously
    reject the join against the other branch's still-`Optional` binding,
    since it is oblivious to *why* a binding changed. The overlay design
    (decision 1) avoids this by construction.
  - *Reusing `contains_return` for the early-return continuation shape* —
    rejected per decision 3: unsound, as the nested-`if` counterexample
    above demonstrates.
  - *A general narrowing-to-`None` shape* (`if name is not None: ... else:
    <use name as `None`>`, or a `return`-terminated `if name is not None:`
    guard narrowing the continuation to `None`) — not implemented. There is
    no `Ty::None`-typed narrowing *target* in this design; deferred to a
    future extension if it proves load-bearing.
  - *Treating `raise` as a terminator alongside `return`* — deferred, not
    rejected outright: `raise` is semantically a valid "does not fall
    through" signal for this purpose, but including it correctly requires
    reasoning about exception-handling control flow (a `try`/`except`
    around the narrowed test could catch the exception and fall through
    after all) that `definitely_terminates`'s current single-function,
    syntax-only analysis does not model. Scoped out rather than risking an
    unsound extension for a narrower initial win.
  - *A single mutable `Environment`/`scopes` frame at the MIR layer, ported
    naively from the checker's push/pop pattern with no isolation wrapper*
    — this was the initial implementation, and it was wrong: end-to-end
    execution of the extended `pep_0604_union.py` fixture against the
    compiled `pycc` binary (not any unit test) surfaced a codegen panic
    (`expected an int-or-bool operand, got optional`) caused by exactly the
    leak decision 6 describes — a narrowing fact from a nested early-return
    guard was not isolated to its own enclosing body. `lower_scoped_body`
    (decision 6) and a dedicated regression test
    (`a_nested_early_return_narrowing_does_not_leak_past_the_enclosing_if`
    in `crates/pycc_mir/src/tests/narrow.rs`) were added once the gap was
    found. Recorded here as a worked example of why this issue's own
    completion criteria required exercising the extended fixture through
    the actual compiled binary, not only through unit tests at each layer.

- Consequences:
  - `Optional[int]` narrows to plain `int` inside a top-level `is`/
    `is not None` presence-tested branch — module-global and
    function-local, both operand orders, both polarities, with
    kill-on-reassignment inside the narrowed branch and the early-return
    continuation shape all working end-to-end from source through codegen.
  - `docs/PYTHON_STANDARDS.md`'s PEP 604 row's `◐` status marker is
    unchanged by this PR — the row was already `◐` (partial) under D-197
    and stays `◐`: general unions (`T0048`), non-`int` `Optional[T]`
    (`T0049`), compound-test narrowing, and narrowing-to-`None` remain
    real, versioned capability gaps, not silently accepted regressions.
  - A narrowing fact never crosses a function boundary and never survives a
    reassignment of the narrowed name, matching the scope any reasonable
    reader would expect from a presence-tested branch.
  - The MIR-layer isolation mechanism (`lower_scoped_body`) is now the
    canonical way to lower any nested statement body in `pycc_mir` whenever
    narrowing state must not leak past it; a future MIR feature adding new
    kinds of flow-sensitive per-scope facts should reuse (or explicitly
    extend) this mechanism rather than re-deriving the same isolation
    problem from scratch.
