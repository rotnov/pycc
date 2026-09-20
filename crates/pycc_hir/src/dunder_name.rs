//! The compiler-provided module-level `__name__` binding (W0 of #882,
//! [#1156](https://github.com/rotnov/pycc/issues/1156)).
//!
//! THE RULE, stated once here and quoted by `docs/STDLIB_PLAN.md`'s
//! "Tier 0 — builtins" section:
//!
//! > `__name__` is a compiler-provided module-level `str` binding, seeded as
//! > the module's first top-level statement. It is provided only when the
//! > module references the name and *no dependency of the program mentions the
//! > name at all* and the entry module itself does not bind the name
//! > `__name__` anywhere in its own module scope, because Part 1 of #881 links
//! > every module into one flat namespace in which the seed and a user binding
//! > would be the same global. Module scope includes a binding nested inside a
//! > top-level compound statement, a `match` case capture, and a walrus, none
//! > of which is a function-body local. A dependency is held to the stricter
//! > test -- any mention, a read as much as a binding, anywhere in the file --
//! > because linking places every dependency's top-level statements *ahead* of
//! > the entry module's seed, so a dependency's read runs before the seed
//! > stores anything. That includes a read reached indirectly, through a
//! > dependency's top-level call to one of its own functions, which no
//! > binding-only test can see. A user binding wins outright:
//! > nothing is seeded and every
//! > `__name__` resolves through the ordinary name path, exactly as before
//! > this change. A value-less annotation (`__name__: str`) is not such a
//! > binding -- it only declares a type and emits no store, exactly as in
//! > CPython -- so the seed survives it. A binding inside a function body is
//! > an ordinary local and shadows the module binding only within that
//! > function, matching CPython. Neither test counts a module-scope
//! > `if TYPE_CHECKING:` body, in either the entry module or a dependency:
//! > #790 constant-folds that body away, so it binds nothing and reads
//! > nothing at run time.
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
use crate::hir_module::ImportBinding;
use crate::stmt::is_type_checking_guard;
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
pub(crate) fn seed_item(
    module: &ModModule,
    module_name: Option<&str>,
    imports: &[ImportBinding],
) -> Option<HirItem> {
    let module_name = module_name?;
    (references_dunder_name(module, imports) && !binds_dunder_name_at_module_scope(module, imports))
        .then(|| {
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
fn references_dunder_name(module: &ModModule, imports: &[ImportBinding]) -> bool {
    struct ReferenceScan<'i> {
        found: bool,
        imports: &'i [ImportBinding],
    }
    impl<'a> Visitor<'a> for ReferenceScan<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.found {
                return;
            }
            if let Some(orelse) = folded_type_checking_orelse(stmt, self.imports) {
                for clause in orelse {
                    visitor::walk_elif_else_clause(self, clause);
                }
                return;
            }
            visitor::walk_stmt(self, stmt);
        }

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
    let mut scan = ReferenceScan {
        found: false,
        imports,
    };
    scan.visit_body(&module.body);
    scan.found
}

/// Whether `module` mentions `__name__` at all -- a read as much as a binding,
/// at any depth, including inside a function or class body.
///
/// This is the *dependency* half of the cross-module gate, published on
/// [`crate::LoweredModule`] and applied by the driver (`src/modules.rs`). It is
/// deliberately stricter than [`binds_dunder_name_at_module_scope`], which
/// decides the entry module's own seed: `program::link` concatenates every
/// dependency's top-level statements *ahead* of the entry module's items, and
/// the seed is the first of those, so every dependency statement runs before
/// the seed stores anything. A dependency that merely reads the name therefore
/// reads an uninitialized global, and the read need not be textually top-level
/// -- a top-level call to one of the dependency's own functions reaches a
/// function-body read just the same, which is invisible to any scan that
/// classifies by binding form. Withholding the seed for the whole program
/// restores the pre-#1156 behavior there: the name is undefined and the read is
/// a `T0021`, a diagnostic rather than an artifact that traps at run time.
pub(crate) fn mentions_dunder_name(module: &ModModule, imports: &[ImportBinding]) -> bool {
    references_dunder_name(module, imports) || binds_dunder_name_at_module_scope(module, imports)
}

/// Gate 2: whether `module` binds the name `__name__` anywhere in *module
/// scope*.
///
/// This is the *per-module* half of the shadowing gate. The cross-module half
/// -- another module of the same program binding the name, which is the same
/// global in the flat namespace Part 1 of #881 links every module into -- is
/// decided by the driver in `src/modules.rs` from the very same predicate,
/// published on `LoweredModule::binds_dunder_name`. One scan, one answer, both
/// halves: exactly the shape `exception::shadowed_builtin_exception_name`
/// already has for the builtin exception hierarchy.
///
/// Module scope, not "a direct child of `module.body`". Those are different
/// sets, and an earlier revision of this scan conflated them: it classified
/// bindings by the *statement* that introduces the name and looked only at
/// direct children, then documented the gap as benign on the grounds that the
/// seed is the module's first statement, so a later binding merely rebinds an
/// existing `str` global. That reasoning holds for a `str` value and fails for
/// every other type. `if flag:\n    __name__ = 7` at module level, a `match`
/// capture over a non-`str` subject, and a walrus `(__name__ := 7)` -- all
/// three bind a module global, all three were invisible here, and all three
/// were therefore seeded and then rejected with
///
/// ```text
/// error[T0023]: cannot assign `int` to `__name__`, previously inferred as `str`
/// ```
///
/// which is the user's own top-level binding losing to the seed -- precisely
/// what THE RULE says cannot happen. So the scan walks module scope in full:
/// into every compound statement's body and header, into `match` patterns, and
/// into `except ... as` handlers.
///
/// It stops at `FunctionDef`/`ClassDef`, whose *names* it still checks (a `def`
/// nested in a top-level `if` binds a module global) but whose *bodies* it does
/// not enter: a binding there is an ordinary local that shadows the module
/// binding only within that scope, matching CPython, which is the main reason
/// this design needs no scope machinery of its own. Skipping the whole node
/// also skips its decorators, bases, and parameter defaults, which do evaluate
/// in module scope; no binding form reaches them, because #774 admits a walrus
/// only in an `if`/`while` condition or as a bare expression statement and
/// rejects every other placement with `C0001`.
///
/// A reference, by contrast, counts at any depth -- hence two separate scans
/// rather than one fused pass, exactly as
/// `exception::shadowed_builtin_exception_name` and
/// `exception::module_references_builtin_exception_name` are kept separate.
///
/// Every binding form is covered: `Assign` (including tuple/list/starred
/// unpacking targets), `AnnAssign` *with* a value, `AugAssign`, `FunctionDef`,
/// `ClassDef`, `TypeAlias`, `For` targets (`async` included -- ruff carries that
/// as a flag on the same node, not a separate variant), `With` `as`-targets,
/// `Import`/`ImportFrom` alias bindings (`import __name__`, `import x as
/// __name__`, `from m import y as __name__`), `except ... as __name__`,
/// `match` captures (`case __name__:`, `case [*__name__]:`, `case {**__name__}:`),
/// and the walrus. Over-reporting would only cost the module its seed, which is
/// the pre-#1156 behavior; under-reporting is the defect above.
///
/// A value-less `__name__: str` is deliberately *not* a binding: it only
/// declares a type and emits no store, exactly as in CPython, so the seed
/// survives it.
pub(crate) fn binds_dunder_name_at_module_scope(
    module: &ModModule,
    imports: &[ImportBinding],
) -> bool {
    struct BindingScan<'i> {
        found: bool,
        imports: &'i [ImportBinding],
    }
    impl BindingScan<'_> {
        fn record(&mut self, bound: bool) {
            if bound {
                self.found = true;
            }
        }
    }
    impl<'a> Visitor<'a> for BindingScan<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.found {
                return;
            }
            if let Some(orelse) = folded_type_checking_orelse(stmt, self.imports) {
                for clause in orelse {
                    visitor::walk_elif_else_clause(self, clause);
                }
                return;
            }
            match stmt {
                // Name checked, body deliberately not entered.
                Stmt::FunctionDef(function_def) => {
                    self.record(function_def.name.as_str() == DUNDER_NAME);
                    return;
                }
                Stmt::ClassDef(class_def) => {
                    self.record(class_def.name.as_str() == DUNDER_NAME);
                    return;
                }
                Stmt::TypeAlias(type_alias) => {
                    self.record(target_binds_dunder_name(&type_alias.name));
                }
                Stmt::AnnAssign(ann_assign) => {
                    self.record(
                        ann_assign.value.is_some() && target_binds_dunder_name(&ann_assign.target),
                    );
                }
                Stmt::AugAssign(aug_assign) => {
                    self.record(target_binds_dunder_name(&aug_assign.target));
                }
                Stmt::Assign(assign) => {
                    self.record(assign.targets.iter().any(target_binds_dunder_name));
                }
                Stmt::For(for_stmt) => {
                    self.record(target_binds_dunder_name(&for_stmt.target));
                }
                Stmt::With(with_stmt) => {
                    self.record(
                        with_stmt
                            .items
                            .iter()
                            .filter_map(|item| item.optional_vars.as_deref())
                            .any(target_binds_dunder_name),
                    );
                }
                Stmt::Import(import) => {
                    self.record(import.names.iter().any(alias_binds_dunder_name));
                }
                Stmt::ImportFrom(import) => {
                    self.record(import.names.iter().any(alias_binds_dunder_name));
                }
                _ => {}
            }
            if self.found {
                return;
            }
            visitor::walk_stmt(self, stmt);
        }

        fn visit_except_handler(&mut self, handler: &'a pycc_ast::ExceptHandler) {
            if self.found {
                return;
            }
            let pycc_ast::ExceptHandler::ExceptHandler(handler_inner) = handler;
            self.record(
                handler_inner
                    .name
                    .as_ref()
                    .is_some_and(|name| name.as_str() == DUNDER_NAME),
            );
            if self.found {
                return;
            }
            visitor::walk_except_handler(self, handler);
        }

        fn visit_pattern(&mut self, pattern: &'a pycc_ast::Pattern) {
            if self.found {
                return;
            }
            // `walk_pattern` recurses through the nested patterns but never
            // surfaces a capture *identifier*, which is an `Identifier` rather
            // than an `Expr::Name` and so is invisible to `visit_expr`.
            let captured = match pattern {
                pycc_ast::Pattern::MatchAs(as_pattern) => as_pattern.name.as_ref(),
                pycc_ast::Pattern::MatchStar(star_pattern) => star_pattern.name.as_ref(),
                pycc_ast::Pattern::MatchMapping(mapping_pattern) => mapping_pattern.rest.as_ref(),
                _ => None,
            };
            self.record(captured.is_some_and(|name| name.as_str() == DUNDER_NAME));
            if self.found {
                return;
            }
            visitor::walk_pattern(self, pattern);
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.found {
                return;
            }
            if let Expr::Named(named) = expr {
                self.record(target_binds_dunder_name(&named.target));
                if self.found {
                    return;
                }
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut scan = BindingScan {
        found: false,
        imports,
    };
    scan.visit_body(&module.body);
    scan.found
}

/// The `orelse` of a module-scope `if TYPE_CHECKING:` whose body `lower_stmt`
/// constant-folds away (#790), and `None` for every other statement.
///
/// Both scans consult it, and neither may count what the fold discards. A
/// `TYPE_CHECKING`-guarded body never executes: it binds nothing and reads
/// nothing, so a `__name__ = 7` there is not a user binding that could collide
/// with the seed, and a `print(__name__)` there is not a read that could
/// observe an uninitialized global. Counting either one withholds the seed from
/// a program that would have compiled -- exactly the false `T0021` this helper
/// removes. The `orelse` (an `elif`/`else` chain) *is* live whenever the guard
/// is skipped, so it is walked normally, matching `lower_stmt`.
///
/// `imports` reaches `is_type_checking_guard` unchanged, so the recognized
/// spellings are exactly the ones lowering folds. The bare `TYPE_CHECKING` and
/// the qualified `typing.TYPE_CHECKING` resolve with no import binding at all;
/// an aliased `import typing as t` then `t.TYPE_CHECKING` needs the binding, so
/// passing a short import slice only under-recognizes, which withholds the seed
/// -- the pre-#1156 behavior, and the safe direction, exactly as
/// `class::enum_call::module_bindings` already documents for the same fold.
fn folded_type_checking_orelse<'a>(
    stmt: &'a Stmt,
    imports: &[ImportBinding],
) -> Option<&'a [pycc_ast::ElifElseClause]> {
    match stmt {
        Stmt::If(if_stmt) if is_type_checking_guard(&if_stmt.test, imports) => {
            Some(&if_stmt.elif_else_clauses)
        }
        _ => None,
    }
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
