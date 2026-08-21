---
id: D-158
title: "Property as HIR-level attribute-access rewrite (no new MIR/codegen variant)"
status: accepted
---

## D-158: Property as HIR-level attribute-access rewrite (no new MIR/codegen variant)
- Status: accepted
- Context: `@property` getters and `@<name>.setter` setters (PEP 3107's decorator-based property protocol) require `base.attr` to invoke a getter method and `base.attr = value` to invoke a setter method, transparently — the call syntax is invisible at the Python source level. The compiler already has method-call infrastructure (`HirExpr::MethodCall` → `MirExpr::Call` with a mangled function symbol), attribute-access infrastructure (`HirExpr::AttrGet`/`HirStmt::AttrSet` → `MirExpr::AttrGet`/`MirStmt::AttrSet` with a compile-time slot index), and a class-definition shape (`HirClassDef` with `attrs` and `methods` tables). The question is where to intercept property access: at the MIR/codegen level (new MIR variant or codegen branch), or at the HIR/type-checking level (rewrite to existing constructs).

- Decision: implement `@property` entirely at the HIR and type-checking level, with no new MIR or codegen variant. A `@property`-decorated method is lowered into a `PropertyDef { name, getter, setter }` entry on `HirClassDef.properties` (not the `methods` table — the getter/setter are accessed via attribute syntax, not method-call syntax, so they must not be reachable through `base.method(...)` resolution). The getter and setter functions are still emitted as ordinary `HirItem::Function`s with mangled names (`<ClassName>.<attr>` for the getter, `<ClassName>.<attr>.setter` for the setter), so they flow through the existing codegen path unchanged.

  At the type-checking level (`pycc_types::class.rs`):
  - `resolve_attr_get` checks `class_def.properties` before `class_def.attrs` (matching CPython's descriptor protocol precedence — a property descriptor intercepts attribute access before `__dict__`/slot table), returning the getter's return type.
  - `check_attr_set` checks `class_def.properties` before `class_def.attrs`: a read-only property (no setter) is rejected with `T0044`; a property with a setter checks the assigned value against the setter's own parameter type (not the getter's return type — the two may differ), producing `T0021` on mismatch.

  At the MIR-lowering level (`pycc_mir`'s `expr.rs` and `stmt.rs`):
  - `lower_expr`'s `HirExpr::AttrGet` arm (in `expr.rs` since #546's decomposition) checks `class_def.properties` before the slot table, rewriting `base.attr` to `MirExpr::Call { callee: getter, args: [base] }`.
  - `lower_stmt`'s `HirStmt::AttrSet` arm (in `stmt.rs` since #546's decomposition) checks `class_def.properties` before the slot table, rewriting `base.attr = value` to `MirStmt::ExprStmt(MirExpr::Call { callee: setter, args: [base, value] })`.

  This means MIR and codegen never see a property-specific construct — they only see ordinary `MirExpr::Call` / `MirStmt::ExprStmt(MirExpr::Call)`, which they already handle correctly for method calls.

- Alternatives:
  1. **New MIR variant (`MirExpr::PropertyGet`/`MirStmt::PropertySet`):** rejected — it would duplicate the existing `MirExpr::Call` codegen path with no behavioral difference, adding a codegen branch and a MIR variant for no semantic gain.
  2. **Descriptor-protocol-level implementation (a `__get__`/`__set__` protocol on a `property` builtin type):** rejected — pycc has no runtime type protocol dispatch (D-006's static-dispatch framing), and implementing one just for `@property` would be a disproportionate scope expansion. The HIR-level rewrite is simpler and produces identical observable behavior.
  3. **Rewrite at the AST level (before HIR lowering):** rejected — the AST is a thin re-export of `ruff_python_ast` and is not mutated by the compiler. HIR is the first compiler-owned IR where transformations are appropriate.

- Consequences:
  - **Easier:** no new MIR variant, no new codegen branch, no new runtime representation — the entire feature is implemented in a handful of files (`pycc_hir/src/class.rs`, `pycc_types/src/class.rs`, and `pycc_mir`'s `expr.rs` and `stmt.rs`) by adding property-table lookups before existing slot-table lookups. The getter/setter functions are ordinary functions, so they benefit from all existing function-level infrastructure (monomorphization, redefinition checking, codegen).
  - **Harder:** the property table is checked before the slot table in three separate places (`resolve_attr_get`, `check_attr_set`, MIR `lower_expr` in `pycc_mir/src/expr.rs` and `lower_stmt` in `pycc_mir/src/stmt.rs`), which must stay in sync — a property added to the table but not checked in one of these places would produce inconsistent behavior between type checking and codegen. This is mitigated by the unit tests covering all three paths.
  - **Irreversible:** the `PropertyDef` struct and `HirClassDef.properties` field are now part of the HIR public API; removing them would require updating all `HirClassDef` construction sites. The mangled naming convention (`<ClassName>.<attr>` for getter, `<ClassName>.<attr>.setter` for setter) is baked into the HIR lowering and would be difficult to change after codegen depends on it.
