---
id: D-231
title: "Lower stdlib `import X as Y` to canonical names with an alias-aware receiver shadow check"
status: accepted
---

## D-231: Lower stdlib `import X as Y` to canonical names with an alias-aware receiver shadow check
- Status: accepted
- Context: [D-137](D-137-stdlib-imports-bind-module-qualified-names-via-a.md) point 2
  rejected `import x as y` with `C0001` unconditionally, and
  [D-222](D-222-project-module-imports-resolve-from-the-entry-file.md)'s
  poisonable-names clause encoded that: a `Stmt::Import` lowered "exactly when
  it has one alias, no `asname`, and a module name `pycc_std` resolves". The
  meddylib sweep ([#883](https://github.com/rotnov/pycc/issues/883)) showed
  `import X as Y` is the idiom real code writes, and #883 was decomposed by
  seam into [#962](https://github.com/rotnov/pycc/issues/962) (a stdlib
  module behind an alias, this decision), [#963](https://github.com/rotnov/pycc/issues/963)
  (`from X import a as b`), and [#964](https://github.com/rotnov/pycc/issues/964)
  (a project module behind an alias). Two facts shape the design. First,
  `pycc_mir` and `pycc_codegen` key on the literal strings `"math.sqrt"` and
  `"math.pi"` that `pycc_hir` emits, so whatever the user writes, the HIR must
  keep spelling the module canonically. Second, the receiver shadow check
  ([D-136](D-136-pycc-std-is-a-plain-data-crate-math-sys-symbols.md)'s
  post-review finding, in `pycc_types`) compared the *textual* receiver of that
  canonical string against the bindings in scope; once `m.sqrt(m)` lowers to
  `"math.sqrt"`, the name the user actually wrote is gone by the time the
  check runs, and a parameter named `m` would silently reach libm instead of
  the `AttributeError` CPython raises.
- Decision:
  1. **Lowering.** `import <module> as <alias>` for a `pycc_std`-resolvable
     module lowers to `ImportBinding::Module { local_name: <alias>, module }`.
     `pycc_hir::expr::std_receiver` resolves a receiver name against the
     module's import table first (the last binding for a name wins, matching
     Python's rebinding order) and only then falls back to
     `pycc_std::resolve_module`'s textual lookup, so `import enum as math`
     makes `math.Enum` mean `enum.Enum`. The HIR always carries the canonical
     spelling (`pycc_std::module_name(module)`): `m.sqrt(x)` is
     `Call { callee: "math.sqrt" }`, byte-identical to the unaliased program,
     so MIR, codegen, and every existing diagnostic downstream are untouched.
  2. **Diagnostics.** An unregistered symbol reached through an alias reports
     the canonical module and the alias -- ``module `math` (imported as `m`)
     has no importable symbol named `tan` `` -- and appends the parenthetical
     only when the spelling differs, so every existing canonical diagnostic
     stays byte-stable.
  3. **Visibility follows source order.** The import table is threaded
     through lowering as a trailing `imports: &[ImportBinding]` parameter, so
     a statement or function body sees exactly the bindings made above it; an
     alias bound *after* a function's `def` is an ordinary name inside that
     function (`m.sqrt(x)` there is a `MethodCall` on a local `m`, reported by
     the type checker like any other), matching CPython's late binding of
     module globals at the granularity pycc models. The
     `typing.TYPE_CHECKING` guard fold ([#790](https://github.com/rotnov/pycc/issues/790))
     recognises `<alias>.TYPE_CHECKING` when the alias is bound to `typing`.
  4. **Alias-aware shadow check.** Both `pycc_types::Environment` and
     `ConstraintEnvironment` carry `std_module_aliases: Vec<(String, StdModule)>`,
     built by one `std_receiver::bind_std_module_aliases(&hir.imports)` called
     from exactly two sites -- the entry of `check_with_environment_all` (the
     common sink of both `Environment` constructors) and the solver's globals
     environment. All four shadow-check sites ask
     `shadowed_std_receiver(module, aliases, is_bound)`, which tests the
     canonical spelling and then every alias bound to that module, and reports
     the first spelling that is bound. `is_bound` consults
     `binding_state(..).is_some()` (not `lookup`, which hides a `Maybe`
     binding) plus the source-order `def_rebound`/`defs_rebound` sets (not the
     position-blind `lookup_function`), plus the function's syntactic locals.
     Two holes this closes for the *canonical* spelling too are deliberate
     behaviour changes: a module-level conditionally bound `math` and a
     `def math()` above the use are now `C0001`, as CPython's `AttributeError`
     demands, while a `def math()` *below* a module-level use stays accepted.
  5. **Recorded residuals** (both fail closed, a false reject, never a
     miscompile): (a) the HIR string cannot say which spelling the user wrote,
     so a local named `math` while the module only ever writes `m.sqrt` is
     rejected -- closed by [#768](https://github.com/rotnov/pycc/issues/768)'s
     binding-aware resolution; (b) `pycc_hir::program::link` flattens every
     module's imports into one `HirModule`, so the alias table is program-wide
     and `import math as m` in `a.py` makes a parameter `m` around a canonical
     `math.sqrt` call in `b.py` a false reject -- closed by
     [#901](https://github.com/rotnov/pycc/issues/901)'s per-module
     namespaces. Both are pinned by tests so their closure is observable.
  6. **Unchanged.** Attribute-form class bases, decorators, and annotations
     (`class C(enum.Enum)`, `@dataclasses.dataclass`, `x: typing.Final[int]`)
     were already `C0001` in the canonical spelling and stay so through an
     alias; `from X import a as b` (#963) and an alias on a project or
     unregistered module (#964) stay `C0001` with their existing messages.
     D-222's poisonable-names clause is restated, not changed in kind: a
     `Stmt::Import` lowers -- and poisons nothing -- exactly when it has one
     alias and a `pycc_std`-resolvable module, with or without an `asname`;
     a rejected `import X as Y` poisons `Y`, the name it would have bound.
- Alternatives:
  - *Rewrite the alias to the canonical name in an AST pre-pass.* Rejected: it
    loses the alias spelling that the diagnostics need, and it does nothing
    for the shadow check, which is the actual correctness hazard.
  - *Carry the alias into the HIR string (`"m.sqrt"`).* Rejected: MIR and
    codegen key on the literal canonical strings, so every downstream match
    would have to learn the module's whole alias table for no semantic gain.
  - *Leave the shadow check textual (check only the canonical name).*
    Rejected: `def f(m: float) -> float: return m.sqrt(m)` after `import math
    as m` would compile to a libm call where CPython raises -- a silent
    divergence, the one outcome D-136's finding exists to prevent.
  - *Bind aliases per function instead of per module.* Rejected: the alias
    table is a module-level fact and the residual it leaves is a false reject,
    not a miscompile; #901's per-module namespaces are the right seam.
- Consequences: real-world `import math as m` code compiles unchanged, and
  `pycc_std` grows a `module_name` inverse of `resolve_module` that every
  canonical emission goes through. The trailing `imports` parameter reaches
  every lowering entry point (`lower_expr`, `lower_stmt`, `lower_function`,
  `lower_class`, and their helpers), which is the cost of source-order
  visibility without a global. `ConstraintEnvironment::empty` is the
  struct-update base for unit tests so the next field does not touch a
  hundred literals. D-137 point 2's `import x as y` clause and D-222's
  no-`asname` condition are superseded for stdlib modules only; the
  `from ... as ...` rejection D-229 cites for `__future__` is no longer "the
  generic aliasing gap" but the `from`-form gap #963 owns. The two closed
  shadow holes are stricter than before for canonical `math`, which is the
  CPython-faithful direction.
