//! The `del` statement (#1244, Part 1 of #1216).
//!
//! `docs/TYPE_SYSTEM.md`'s "`del` statement" section is the canonical
//! statement of the rule. This module owns the HIR half of it:
//!
//! - [`lower_delete`] expands `del a, (b, [c])` left to right into one
//!   [`HirStmt::Delete`] per name and refuses every other target kind;
//! - [`deleted_names`] is the recursive walk the type checker's deletion
//!   prescan and the import resolver use;
//! - [`check_module_deletions`] closes the module-level late-binding hole: a
//!   function or class body is checked against the environment *after* all
//!   top-level code (D-041), so a module-level `del x` of a name any function
//!   or class mentions would let that body read a global the codegen still
//!   marks initialized;
//! - [`mentioned_names`] feeds `program::link`'s cross-module rule, since the
//!   linked program shares one flat top-level namespace.

use crate::{HirItem, HirStmt, unsupported};
use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{Expr, ModModule, Stmt, StmtDelete};
use pycc_diag::{Diagnostic, Span};
use std::collections::{BTreeSet, HashSet};

/// Lowers `del t1, t2, ...` into one [`HirStmt::Delete`] per deleted name, in
/// CPython's left-to-right order. A tuple or list target (`del (a, b)`,
/// `del [a, [b]]`) is flattened the same way; an empty one (`del ()`) deletes
/// nothing, exactly as in CPython. Every other target kind is refused: the
/// parser already rejects starred, call and literal targets (`L0001`), so what
/// reaches here is a name, a tuple/list, a subscript or an attribute.
pub(crate) fn lower_delete(del: &StmtDelete) -> Result<Vec<HirStmt>, Diagnostic> {
    let mut lowered = Vec::new();
    for target in &del.targets {
        lower_target(target, &mut lowered)?;
    }
    Ok(lowered)
}

fn lower_target(target: &Expr, lowered: &mut Vec<HirStmt>) -> Result<(), Diagnostic> {
    match target {
        Expr::Name(name) => {
            if name.id.as_str() == crate::DUNDER_NAME {
                return Err(unsupported(
                    "a `del` of the module's `__name__` is not supported",
                    name.range,
                ));
            }
            lowered.push(HirStmt::Delete {
                name: name.id.to_string(),
            });
        }
        Expr::Tuple(tuple) => {
            for elt in &tuple.elts {
                lower_target(elt, lowered)?;
            }
        }
        Expr::List(list) => {
            for elt in &list.elts {
                lower_target(elt, lowered)?;
            }
        }
        Expr::Subscript(subscript) if matches!(subscript.slice.as_ref(), Expr::Slice(_)) => {
            return Err(unsupported(
                "a `del` of a slice (`del xs[a:b]`) is not supported yet",
                subscript.range,
            ));
        }
        Expr::Subscript(subscript) => {
            return Err(unsupported(
                "a `del` of a subscript (`del d[k]`, `del xs[i]`) is not supported yet \
                 (#1245 for `dict`, #1246 for `list`)",
                subscript.range,
            ));
        }
        other => {
            return Err(unsupported(
                format!(
                    "a `del` of {} is not supported yet; only a bare name can be deleted",
                    pycc_ast::expr_kind_name(other)
                ),
                pycc_ast::expr_range(other),
            ));
        }
    }
    Ok(())
}

/// Every name a [`HirStmt::Delete`] anywhere in `body` deletes, at any
/// nesting depth. The match is exhaustive on purpose, as `killed_names`'s is:
/// a new statement kind with a nested body must be taught to this walk rather
/// than silently hiding a deletion from the type checker's prescan.
pub fn deleted_names(body: &[HirStmt]) -> HashSet<String> {
    let mut deleted = HashSet::new();
    collect_deleted_names(body, &mut deleted);
    deleted
}

fn collect_deleted_names(body: &[HirStmt], deleted: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            HirStmt::Delete { name } => {
                deleted.insert(name.clone());
            }
            HirStmt::If { body, orelse, .. } => {
                collect_deleted_names(body, deleted);
                collect_deleted_names(orelse, deleted);
            }
            HirStmt::While { body, .. }
            | HirStmt::ForRange { body, .. }
            | HirStmt::ForList { body, .. }
            | HirStmt::ForObject { body, .. } => collect_deleted_names(body, deleted),
            HirStmt::Match { cases, .. } => {
                for case in cases {
                    collect_deleted_names(&case.body, deleted);
                }
            }
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
            | HirStmt::TryStar {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                collect_deleted_names(body, deleted);
                for handler in handlers {
                    collect_deleted_names(&handler.body, deleted);
                }
                collect_deleted_names(orelse, deleted);
                collect_deleted_names(finalbody, deleted);
            }
            HirStmt::ExprStmt(_)
            | HirStmt::Assign { .. }
            | HirStmt::AnnAssign { .. }
            | HirStmt::DictSet { .. }
            | HirStmt::ListCompAssign { .. }
            | HirStmt::DictCompAssign { .. }
            | HirStmt::SetCompAssign { .. }
            | HirStmt::Return(_)
            | HirStmt::AttrSet { .. }
            | HirStmt::Raise { .. } => {}
        }
    }
}

/// Every name a module's top-level statements delete, at any nesting depth
/// inside them -- [`deleted_names`] over the module's
/// `HirItem::TopLevelStmt`s. `import::bind_project_name` refuses importing
/// one of these names (#1244).
pub(crate) fn top_level_deleted_names(items: &[HirItem]) -> HashSet<String> {
    let mut deleted = HashSet::new();
    for item in items {
        if let HirItem::TopLevelStmt(stmt) = item {
            collect_deleted_names(std::slice::from_ref(stmt), &mut deleted);
        }
    }
    deleted
}

/// Checks every module-scope `del` in `module` (outside any `def`/`class`
/// body) against the late-binding rule and returns the deleted names with the
/// span of each `del` statement, for `program::link`'s cross-module rule.
///
/// The rule: a module-scope `del x` is refused when `x` is mentioned anywhere
/// inside any `def` or `class` statement of the module -- its body, decorators,
/// parameter defaults, annotations or bases. Those bodies are type-checked
/// against the module environment as it stands after *all* top-level code
/// (D-041), not at the point they are called, so the checker cannot see that a
/// call made after the `del` reads an unbound global; and codegen would still
/// find the global's initialized flag set, printing a stale value where
/// CPython raises `NameError`. The rule is conservative -- a function that only
/// binds its own local `x` also triggers it -- and sound.
pub(crate) fn check_module_deletions(
    module: &ModModule,
) -> Result<Vec<(String, Span)>, Diagnostic> {
    let mut scan = ModuleDeleteScan::default();
    scan.visit_body(&module.body);
    for (name, span) in &scan.deleted {
        if scan.imported.contains(name) {
            return Err(unsupported(
                format!("a `del` of the imported name `{name}` is not supported yet"),
                span.start..span.end,
            ));
        }
        if scan.mentioned_in_defs.contains(name) {
            return Err(unsupported(
                format!(
                    "a module-level `del {name}` is not supported when a function or class of \
                     this module mentions `{name}`: its body is checked after all top-level \
                     code, so it could read `{name}` after the deletion"
                ),
                span.start..span.end,
            ));
        }
    }
    Ok(scan.deleted)
}

/// One pass over a module's statements: `del` targets and `import` bindings
/// at module scope, and every name mentioned inside a `def`/`class`
/// statement.
#[derive(Default)]
struct ModuleDeleteScan {
    deleted: Vec<(String, Span)>,
    imported: HashSet<String>,
    mentioned_in_defs: HashSet<String>,
}

impl<'a> Visitor<'a> for ModuleDeleteScan {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                let mut names = NameScan::default();
                names.visit_stmt(stmt);
                self.mentioned_in_defs.extend(names.names);
            }
            // An import binds a module marker, a registry symbol, a project
            // definition or a CPython object -- none of them a plain value
            // binding the checker can demote -- so a module-scope `del` of
            // one is refused outright.
            Stmt::Import(import) => {
                self.imported
                    .extend(import.names.iter().map(import_local_name));
            }
            Stmt::ImportFrom(import) => {
                self.imported
                    .extend(import.names.iter().map(import_local_name));
            }
            Stmt::Delete(del) => {
                let range = pycc_ast::stmt_range(stmt);
                let span = Span::new(range.start, range.end);
                for target in &del.targets {
                    collect_target_names(target, &mut |name| {
                        self.deleted.push((name.to_string(), span));
                    });
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
}

/// The name an `import` alias binds: its `as` name, or else the first
/// component of the dotted module path (`import a.b` binds `a`).
fn import_local_name(alias: &pycc_ast::Alias) -> String {
    match &alias.asname {
        Some(asname) => asname.to_string(),
        None => alias.name.split('.').next().unwrap_or_default().to_string(),
    }
}

/// Calls `record` for every bare name a `del` target deletes, flattening
/// tuples and lists. Other target kinds are refused by [`lower_delete`].
pub(crate) fn collect_target_names(target: &Expr, record: &mut impl FnMut(&str)) {
    match target {
        Expr::Name(name) => record(name.id.as_str()),
        Expr::Tuple(tuple) => tuple
            .elts
            .iter()
            .for_each(|elt| collect_target_names(elt, record)),
        Expr::List(list) => list
            .elts
            .iter()
            .for_each(|elt| collect_target_names(elt, record)),
        _ => {}
    }
}

/// Every `Expr::Name` id a statement or module mentions, at any depth. A read
/// and a store of a flat top-level global are both an `Expr::Name`, which is
/// all the `del` rules need: the other binding forms (`def`/`class` names, an
/// import alias, an `except ... as` or `match` capture) are either not
/// deletable (`pycc_types`' allowlist refuses functions and classes, and
/// [`check_module_deletions`] refuses a module-scope `del` of an import alias)
/// or already reach `definition_spans` and `program::link`'s collision guard.
#[derive(Default)]
struct NameScan {
    names: HashSet<String>,
}

impl<'a> Visitor<'a> for NameScan {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr {
            self.names.insert(name.id.to_string());
        }
        visitor::walk_expr(self, expr);
    }
}

/// Every `Expr::Name` id `module` mentions anywhere, published on
/// `LoweredModule::mentioned_names` for `program::link`'s cross-module `del`
/// rule.
pub(crate) fn mentioned_names(module: &ModModule) -> BTreeSet<String> {
    let mut scan = NameScan::default();
    scan.visit_body(&module.body);
    scan.names.into_iter().collect()
}
