//! The compiler-provided module-level `__name__` binding (W0 of #882,
//! [#1156](https://github.com/rotnov/pycc/issues/1156)).
//!
//! THE RULE, stated once here and quoted by `docs/STDLIB_PLAN.md`'s
//! "Tier 0 — builtins" section:
//!
//! > `__name__` is a compiler-provided module-level `str` binding, seeded as
//! > the module's first top-level statement. It is provided only when the
//! > module references the name and *no module of the program* binds the name
//! > `__name__` at its own top level -- neither the entry module nor any
//! > dependency, because Part 1 of #881 links every module into one flat
//! > namespace in which the seed and a user binding would be the same global.
//! > A top-level user binding wins outright: nothing is seeded and every
//! > `__name__` resolves through the ordinary name path, exactly as before
//! > this change. A value-less annotation (`__name__: str`) is not such a
//! > binding -- it only declares a type and emits no store, exactly as in
//! > CPython -- so the seed survives it. A binding inside a function body is
//! > an ordinary local and shadows the module binding only within that
//! > function, matching CPython.
//! >
//! > Deviation from CPython, deliberate and documented: in CPython a read that
//! > textually precedes a module-level `__name__ = ...` still sees the
//! > interpreter-provided module name. Here the seed is withheld for the whole
//! > module, so such a read resolves to the user's binding — in practice a
//! > `T0021` "name `__name__` is not defined" when the read precedes the
//! > assignment. This is fail-closed (a diagnostic, never a silently wrong
//! > value) and mirrors the all-or-nothing shape D-188 already established for
//! > the builtin exception hierarchy.
//!
//! No new HIR, MIR, type, or codegen node exists for `__name__`: the seed is an
//! ordinary `HirItem::TopLevelStmt(HirStmt::Assign)` of a `str` literal, so
//! every downstream pass sees the module global it already knows how to
//! compile. `pycc_types`, `pycc_mir`, `pycc_codegen` and `pycc_rt` are
//! untouched by this feature.

use super::{HirExpr, HirItem, HirStmt};
use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{Expr, ModModule, Stmt};

/// The one name this module is about.
pub const DUNDER_NAME: &str = "__name__";

/// Whether `module_name` should be seeded into `module`, and with what value.
///
/// Both gates below must pass, and the driver must have supplied a name at
/// all. A non-entry module of a multi-file program is given `None`: Part 1 of
/// #881 links every module into one flat namespace, so a per-module
/// `__name__` global would collide, and withholding the seed there is the
/// fail-closed choice until per-module namespaces land.
pub(crate) fn seed_item(module: &ModModule, module_name: Option<&str>) -> Option<HirItem> {
    let module_name = module_name?;
    (references_dunder_name(module) && !binds_dunder_name_at_top_level(module)).then(|| {
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: DUNDER_NAME.to_string(),
            value: HirExpr::StringLiteral(module_name.to_string()),
        })
    })
}

/// Gate 1: whether `module` references the name `__name__` anywhere, at any
/// depth.
///
/// A module that never mentions the name pays nothing — no extra item, no
/// change to any existing test's expected HIR — which is the whole reason this
/// gate exists.
///
/// The walk is [`pycc_ast::visitor::Visitor`], ruff's own generic AST
/// traversal, for the same reason
/// `exception::module_references_builtin_exception_name` uses it: a hand-rolled
/// `Stmt`/`Expr` match needs a `_ =>` arm, and a spelling missed there fails
/// silently. Overriding only `visit_expr` makes every name-bearing position
/// reachable by construction and keeps new upstream AST nodes covered
/// automatically. A reference inside a function or class body counts, because
/// the seeded module global is exactly what such a read resolves to.
fn references_dunder_name(module: &ModModule) -> bool {
    struct ReferenceScan {
        found: bool,
    }
    impl<'a> Visitor<'a> for ReferenceScan {
        fn visit_expr(&mut self, expr: &'a Expr) {
            // Once the name is seen the answer cannot change, so stop
            // descending rather than walking the rest of the module.
            if self.found {
                return;
            }
            if let Expr::Name(name) = expr
                && name.id.as_str() == DUNDER_NAME
            {
                self.found = true;
                return;
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut scan = ReferenceScan { found: false };
    scan.visit_body(&module.body);
    scan.found
}

/// Gate 2: whether `module`'s own top level binds the name `__name__`.
///
/// This is the *per-module* half of the shadowing gate. The cross-module half
/// -- a dependency's own top-level binding, which is the same global in the
/// flat namespace Part 1 of #881 links every module into -- is decided by the
/// driver in `src/modules.rs`, which passes `module_name: None` and so never
/// reaches this scan.
///
/// Shadowing is a property of a module's *top level* only: a `__name__ = "x"`
/// inside a function body is an ordinary local that shadows the module binding
/// only within that function (CPython's own rule), which is the main reason
/// this design needs no scope machinery of its own. A reference, by contrast,
/// counts at any depth — hence two separate scans rather than one fused pass,
/// exactly as `exception::shadowed_builtin_exception_name` and
/// `exception::module_references_builtin_exception_name` are kept separate.
///
/// Every top-level binding form whose *statement* introduces the name is
/// covered, not just plain assignment: `Assign` (including tuple/list/starred
/// unpacking targets), `AnnAssign`, `AugAssign`, `FunctionDef`, `ClassDef`,
/// `TypeAlias`, `For` targets (`async` included — ruff carries that as a flag on
/// the same node, not a separate variant), `With` `as`-targets, and
/// `Import`/`ImportFrom` alias bindings (`import x as __name__`, `from m import
/// y as __name__`). Over-reporting only costs the module its seed, which is the
/// pre-#1156 behavior.
///
/// Two further top-level forms bind a name through an *expression* rather than
/// through the statement's own target, and neither is scanned. That is
/// deliberate: in both, the type system — not this scan — already makes the
/// outcome either a diagnostic or a well-typed rebind, so neither can race the
/// seed into a silently wrong value.
///
/// * A `match` case capture (`match x:` / `case __name__:`). The seed is the
///   module's first statement, so a capture rebinds an existing `str` global
///   instead of introducing the name. A `str` subject compiles and the capture
///   overwrites the seeded value; a subject of any other type is rejected with
///   `T0023` ("cannot assign `int` to `__name__`, previously inferred as
///   `str`"). Both are pinned by `tests/issue_1156_dunder_name.rs`.
/// * A walrus (`(__name__ := "custom")`). A walrus value of type `str` is not
///   supported at all — `T0050`, #774 — so this form cannot bind a module name
///   today. Should #774 lift that restriction, the `match` reasoning above
///   applies to it unchanged.
///
/// Documented limit, mirroring the flat scan
/// `exception::shadowed_builtin_exception_name` performs: only *direct*
/// children of `module.body` are inspected. A binding nested inside a top-level
/// compound statement (`if flag:\n    __name__ = "x"`) does not trip this gate,
/// so the module is seeded *and* the user's statement still executes and still
/// wins at runtime — benign, because the seed is the module's first statement
/// and a later assignment simply rebinds the same `str` global. The one visible
/// consequence is that such a binding must be `str`-compatible, since it now
/// rebinds a `str` rather than introducing the name. Recursing into every
/// compound body while stopping at `FunctionDef`/`ClassDef` was rejected as
/// more surface than the case earns.
fn binds_dunder_name_at_top_level(module: &ModModule) -> bool {
    module.body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(function_def) => function_def.name.as_str() == DUNDER_NAME,
        Stmt::ClassDef(class_def) => class_def.name.as_str() == DUNDER_NAME,
        Stmt::TypeAlias(type_alias) => target_binds_dunder_name(&type_alias.name),
        // A value-less `__name__: str` only *declares* a type; CPython emits no
        // store for it and the interpreter-provided module name survives, so it
        // must not withhold the seed the way an annotated *assignment* does.
        Stmt::AnnAssign(ann_assign) => {
            ann_assign.value.is_some() && target_binds_dunder_name(&ann_assign.target)
        }
        Stmt::AugAssign(aug_assign) => target_binds_dunder_name(&aug_assign.target),
        Stmt::Assign(assign) => assign.targets.iter().any(target_binds_dunder_name),
        Stmt::For(for_stmt) => target_binds_dunder_name(&for_stmt.target),
        Stmt::With(with_stmt) => with_stmt
            .items
            .iter()
            .filter_map(|item| item.optional_vars.as_deref())
            .any(target_binds_dunder_name),
        Stmt::Import(import) => import.names.iter().any(alias_binds_dunder_name),
        Stmt::ImportFrom(import) => import.names.iter().any(alias_binds_dunder_name),
        _ => false,
    })
}

/// Whether `expr`, used as an assignment or loop target, binds `__name__`.
/// Recurses through the unpacking shapes (`a, b = ...`, `[a, b] = ...`,
/// `*rest`) so a name buried in one is still seen. Any other target shape (an
/// attribute, a subscript) rebinds something other than a bare module-level
/// name, so it cannot shadow the seed.
fn target_binds_dunder_name(expr: &Expr) -> bool {
    match expr {
        Expr::Name(name) => name.id.as_str() == DUNDER_NAME,
        Expr::Tuple(tuple) => tuple.elts.iter().any(target_binds_dunder_name),
        Expr::List(list) => list.elts.iter().any(target_binds_dunder_name),
        Expr::Starred(starred) => target_binds_dunder_name(&starred.value),
        _ => false,
    }
}

/// Whether one `import`/`from ... import ...` alias binds `__name__`. The
/// bound name is the `as` clause when there is one, and otherwise the
/// imported name's first dotted segment (`import a.b` binds `a`).
fn alias_binds_dunder_name(alias: &pycc_ast::Alias) -> bool {
    match &alias.asname {
        Some(asname) => asname.as_str() == DUNDER_NAME,
        None => alias
            .name
            .as_str()
            .split('.')
            .next()
            .is_some_and(|root| root == DUNDER_NAME),
    }
}

#[cfg(test)]
mod tests;
