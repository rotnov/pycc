---
id: D-104
title: "Defer the generic function type-parameter `Ty` placeholder to PR-13"
status: accepted
---

## D-104: Defer the generic function type-parameter `Ty` placeholder to PR-13

- Status: accepted
- Context: D-089 decided `Ty`'s new container variants (`List`/`Dict`/`Set`/`Tuple`) but explicitly left "whatever case PR-13 needs for a generic function's type parameter (a placeholder/parameter marker distinct from `Infer`)" unresolved, naming both PR-10 and PR-13 as candidate owners. PR-10's own actual scope (`docs/DELIVERY_PLAN.md` row 10; `docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`'s PR breakdown item 3) is `Ty` migration + monomorphization foundation + a `list[T]` thin slice with concrete, already-known element types (a literal's element type, a function argument's already-annotated type) — it never emits or consumes a type *parameter* (`def f[T](x: T) -> T`'s `T`), only concrete types. PEP 695 user-defined generic functions are PR-13's own scope (`docs/DELIVERY_PLAN.md` row 13).
- Decision: PR-10 adds no `Ty` variant for generic type parameters. `Ty`'s new recursive variants are exactly `List(Box<Ty>)`, `Dict(Box<Ty>, Box<Ty>)`, `Set(Box<Ty>)`, `Tuple(Vec<Ty>)` — nothing else. PR-13's own implementer designs the type-parameter placeholder against the real constraint-solving code (`collect_expr_constraints`/`infer_expr_in` in `pycc_types`) once PEP 695 syntax parsing actually exists to feed it, rather than PR-10 guessing its shape with no call site to exercise it.
- Alternatives: add a speculative `Ty::TypeParam(String)`-style variant now, unused until PR-13 (rejected — this project's own D-057 "simplest correct thing for the stated scope" precedent argues against speculative variants with no code path exercising them; an unexercised variant is also invisible to every exhaustive match this plan's own Task 2-5 fix, meaning it would silently need re-auditing anyway once PR-13 actually uses it, buying nothing over adding it in PR-13 directly).
- Consequences: `Ty`'s recursive shape after this PR is exactly the four container variants plus the six pre-existing scalars — ten variants total, all fully concrete. PR-13 supersedes nothing here; it simply adds its own variant when it has a real consumer for it.

