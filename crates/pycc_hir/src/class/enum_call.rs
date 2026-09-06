//! Spanned rejection of a call to an enum class (#921, #944).
//!
//! An enum class is constructor-less by design (`lower_enum_class`
//! early-returns before `class::init::ensure_init`, D-225): its members are
//! compile-time singletons, and CPython's `EnumType.__call__` value lookup
//! (`Color(1)`) is not implemented. #921 added a span-less guard in
//! `pycc_types::class::resolve_instantiation`, which renders the `C0001` at
//! `1:1` because `HirExpr` carries no spans. The one place a diagnostic can
//! still point at the call expression without threading spans through
//! `pycc_types` (#877) is an AST-level scan, so `lower_module` runs this
//! module once per top-level item, right after that item is lowered (`Ok`
//! or `Err` alike), and appends its diagnostics after the item's own so the
//! collection order stays the loop order D-217 rule 3 pins (see the D-233
//! amendment of D-219). `resolve_instantiation` keeps its guard behind this
//! scan (defense in depth), so the two "no `__init__` in the MRO" panics
//! describe an invariant that holds; the two rejections share
//! [`enum_class_call_message`] so they render identically.
//!
//! # Scope-local bindings
//!
//! The scan keys on a bare callee *name*, so it has to know when that name
//! is not the enum class at all: `def f(Color: int) -> None: Color()` is a
//! call on an `int` (`T0021` from `pycc_types`), and a bare-name scan would
//! turn that accurate diagnostic into a misleading enum-call `C0001`. The
//! walk therefore keeps a stack of *frames* -- the set of names bound
//! directly in one scope -- and reports a call only when no frame binds
//! the callee. There is one frame for the module body (computed once by
//! `lower_module` through [`module_bindings`]), one pushed around each
//! `def` (its parameter names plus the names its body binds), and one
//! pushed around each `lambda` (its parameter names). A frame records every
//! `Expr::Name` in `Store` context (assignment, augmented and annotated
//! assignment, `for`/`with`/walrus targets, tuple and starred targets,
//! comprehension targets), every `except ... as name`, and every `match`
//! capture (`case Color:`, `case [*Color]:`, `case {**Color}:`) -- those
//! last two groups are `Identifier`s rather than `Expr::Name`s, so the
//! `Store` rule alone would miss them. It records nothing else: not the
//! names of nested `def`/`class` statements and not `import` aliases (limit
//! (ii) below says why neither can matter on an item that lowers), and it
//! does not descend into a nested `def`, `class`, or `lambda`, so
//! `def g(): Color = 1` never suppresses a module-level `Color()`.
//!
//! # `TYPE_CHECKING` guards
//!
//! `lower_stmt` constant-folds an `if TYPE_CHECKING:` / `elif
//! TYPE_CHECKING:` body away as dead code (#790, D-223: a capability gap
//! inside such a body is deliberately not an error), and the scan walks the
//! original AST, so it has to fold the same bodies or it would reject a
//! module the lowering accepts (`if TYPE_CHECKING: Color(1)`, caught in
//! review of PR #971). Both walkers route every `if` through
//! `walk_if_as_lowered`, which skips a guarded body -- calls *and*
//! bindings, so a dead `Color = 1` does not shadow either -- using the very
//! `is_type_checking_guard` predicate the fold uses, against the same
//! import bindings the fold saw for that item. The live clauses around a
//! guard (a non-guard `if`, an `elif`, an `else`) are scanned normally.
//!
//! # Limits
//!
//! All of these are stated here rather than discovered later:
//!
//! - (i) **For an item that lowers, over-suppression is the only failure
//!   mode, never a false report.** A scope that binds the name *anywhere*
//!   suppresses the scan for the whole scope -- Python's own rule for a
//!   function; for the module frame it also hides a `Color()` that precedes
//!   a later module-level `Color = 1`, and a comprehension target
//!   `[Color for Color in range(3)]` suppresses a sibling `Color(1)` in the
//!   same `def`. Every suppressed call still fails in `pycc_types` (the
//!   span-less guard at `1:1`, or `T0021`). The one false-kind report is
//!   confined to a **class body**, which gets no frame: `class K:` with
//!   `Color = 1` and `X = Color()` reports the class-attribute `C0001` for
//!   `X = Color()` *and* an enum-call `C0001` at `Color()`, although Python
//!   resolves that name to the class-body `int`. Every class-body-level
//!   call already sits in a failing position (class attribute, base, or
//!   decorator), so this is only ever a second diagnostic on an item that
//!   fails anyway; a class frame was rejected because method bodies would
//!   inherit it and `class K: Color = 1` plus `def m(self): Color()` --
//!   correctly reported -- would be suppressed.
//! - (ii) `def`/`class` names and import aliases are not bindings in any
//!   frame, and that loses nothing on an item that lowers: at module level
//!   a `def Color`, a non-enum `class Color`, a `type Color = ...`, or an
//!   `import ... as Color` next to `class Color(Enum)` is a collision
//!   `C0001` in either source order, which poisons `Color` and so filters
//!   the scan; a `Color = 1` after `from colors import Color` is the
//!   "already defined by" `C0001`; and inside a function a nested
//!   `def`/`class`/`import` is itself a failing item, so the only effect of
//!   not modelling it is a possible second diagnostic on an item that
//!   already fails -- the same residual class as the class body. An
//!   imported enum (the `state.class_defs` half of `lower_module`'s name
//!   set) stays scannable.
//! - (iii) Decorators, default values, annotations, and the return
//!   annotation are scoped to the function they belong to rather than to
//!   the enclosing scope (the frame is pushed around ruff's whole
//!   `FunctionDef` walk). The only observable consequence is a missed
//!   *second* diagnostic on an item that already fails: a decorator or a
//!   default expression is `C0001 ... not supported yet` on its own.
//! - (iv) **Poison is order-dependent.** A `def` that calls `Color(1)` and
//!   *precedes* a failing `class Color(Enum): pass` is scanned before the
//!   class item fails and poisons `Color` (`poisoned` is filled only in the
//!   `Err` arm of `lower_module`'s loop), so that program yields two
//!   diagnostics -- the enum `C0001` at `Color(1)` first, then the
//!   class-body `C0001`. The extra report is true (the call *is* an enum
//!   call), so this is an accepted, pinned limit; the reverse order stays
//!   suppressed as a D-219 cascade. Likewise an enum imported by a
//!   statement *after* a `def` that calls it is unknown when that `def` is
//!   scanned and falls through to `pycc_types`' span-less guard.
//! - (v) `global`/`nonlocal` are not consulted (pycc does not lower them),
//!   and PEP 695 type parameters (`def f[Color](x: Color)`,
//!   `class K[Color]`) are not bindings -- they are `Identifier`s, not
//!   `Store` names. Both shapes lower and already reported the enum-call
//!   guard at `1:1`, so reporting at the call is the same diagnostic kind.
//! - (vi) **The module frame sees only the imports known before the
//!   loop.** `module_bindings` runs once, before `lower_module` lowers the
//!   module's own `import` statements, so a module-level guard spelled
//!   through an alias (`import typing as t` then `if t.TYPE_CHECKING:`)
//!   is not recognized there and a dead `Color = 1` inside it still lands
//!   in the module frame -- over-suppression only (the call fails in
//!   `pycc_types`). The bare `TYPE_CHECKING` and `typing.TYPE_CHECKING`
//!   spellings need no import binding and fold in the module frame too;
//!   inside a `def` the scan runs with the item's own import view, so the
//!   aliased spelling folds there.
//! - (vii) **A name bound by more than one module-level `def`, `class`,
//!   `import`, or `type` statement is never scanned.** `def Color()`,
//!   an ordinary `class Color`, `from m import Color`, or
//!   `type Color = int` beside `class Color(Enum)` is a collision that the
//!   class item reports itself (`... collides with a function / an import /
//!   a type alias of the same name ...`, `... defined more than once ...`),
//!   whichever comes second. A module-level `Color()` between the two
//!   resolves to the earlier binding, so scanning it against the syntactic
//!   pre-collection would report a *false-kind* `C0001` ahead of the real
//!   one (PR #971 review, two rounds). `lower_module` therefore drops every
//!   name in `module_rebound_names` from the scan's name set for the whole
//!   module, in either order: the collision diagnostic owns such a
//!   program, and a `def`-body call to the rebound name falls through to
//!   `pycc_types`' span-less guard as before #944. A plain assignment
//!   rebinding the name is the module frame's case already.
//! - A call with a keyword argument (`Color(value=1)`) is skipped:
//!   `lower_expr` already reports exactly one `C0001 keyword call arguments
//!   are not supported yet` at that call, and the scan runs on failed items
//!   too, so without the skip that program would carry two `C0001`s at one
//!   span. The skip is *not* extended to a starred argument (`Color(*xs)`):
//!   the starred `C0001` sits at `*xs`, a different span, and the call is a
//!   genuine enum call.

use crate::ImportBinding;
use crate::stmt::is_type_checking_guard;
use pycc_ast::visitor::{self, Visitor};
use pycc_ast::{ExceptHandler, Expr, ExprContext, Pattern, Stmt, StmtClassDef, StmtIf};
use pycc_diag::Diagnostic;

/// Walks an `if` statement the way `lower_stmt` lowers it (#790): a
/// `TYPE_CHECKING`-guarded body -- the leading `if` or any `elif` clause,
/// recognized by the very same `is_type_checking_guard` the fold uses,
/// against the same `imports` -- is dead code that never reaches HIR, so
/// neither the calls nor the bindings inside it exist for the scan; the
/// guard's own test expression is folded to `False` and is skipped too.
/// Every other clause (a live `if`, `elif`, or `else`) is walked normally.
/// Both walkers below route `Stmt::If` through here, because ruff's
/// `walk_stmt` visits an `if` body and every clause unconditionally.
fn walk_if_as_lowered<'a, V: Visitor<'a>>(
    visitor: &mut V,
    if_stmt: &'a StmtIf,
    imports: &[ImportBinding],
) {
    if !is_type_checking_guard(&if_stmt.test, imports) {
        visitor.visit_expr(&if_stmt.test);
        visitor.visit_body(&if_stmt.body);
    }
    for clause in &if_stmt.elif_else_clauses {
        if let Some(test) = &clause.test
            && is_type_checking_guard(test, imports)
        {
            continue;
        }
        visitor::walk_elif_else_clause(visitor, clause);
    }
}

/// The one `C0001` message for every positional-argument call shape on an
/// enum class (`Color()`, `Color(1)`, `Color(1, 2)`). Shared with
/// `pycc_types::class::resolve_instantiation` so the spanned and the
/// span-less rejection render identically (#942's wording, unchanged by
/// #944). The "not supported yet" clause attaches only to the by-value
/// lookup, which a later slice can implement; `Color()` is a CPython error
/// too and no slice will accept it (`docs/DIAGNOSTICS.md`'s `C0001` is a
/// versioned capability code).
pub fn enum_class_call_message(class_name: &str) -> String {
    format!(
        "cannot call enum class `{class_name}` -- enum members are accessed \
         by name (`{class_name}.MEMBER`); looking a member up by value \
         (`{class_name}(1)`) is not supported yet, and a zero-argument call \
         (`{class_name}()`) is a `TypeError` in CPython as well"
    )
}

/// The names of every top-level class in `body` whose header is exactly one
/// bare-name enum marker base (`class Color(Enum):`, `class S(StrEnum):`),
/// the same predicate `lower_class` uses to route a class to
/// `lower_enum_class`. Computed syntactically *before* the top-level loop so
/// a call inside a `def` that precedes the class definition in source is
/// still found. A class whose base is not a bare name
/// (`class Color(enum.Enum):`) is not an enum class to `lower_class` either
/// and is left to its own `C0001`.
pub(crate) fn syntactic_enum_class_names(body: &[Stmt]) -> Vec<String> {
    body.iter()
        .filter_map(|stmt| match stmt {
            Stmt::ClassDef(def) if has_single_enum_marker_base(def) => Some(def.name.to_string()),
            _ => None,
        })
        .collect()
}

/// Every name that two or more module-level `def`, `class`, `import`, or
/// `type` statements bind (limit (vii)): `lower_module` drops these from
/// the enum-call name set, because a module that binds one name twice
/// that way is reported by the collision diagnostic, never by the scan.
/// An `import a.b` binds `a`; a `from m import *` binds nothing nameable
/// here and is skipped.
pub(crate) fn module_rebound_names(body: &[Stmt]) -> Vec<String> {
    let mut bound: Vec<&str> = Vec::new();
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(def) => bound.push(def.name.as_str()),
            Stmt::ClassDef(def) => bound.push(def.name.as_str()),
            Stmt::TypeAlias(alias) => {
                if let Some(name) = alias.name.as_name_expr() {
                    bound.push(name.id.as_str());
                }
            }
            Stmt::Import(import) => bound.extend(import.names.iter().map(|alias| {
                alias.asname.as_ref().map_or_else(
                    || alias.name.split('.').next().unwrap_or_default(),
                    |asname| asname.as_str(),
                )
            })),
            Stmt::ImportFrom(import) => bound.extend(
                import
                    .names
                    .iter()
                    .filter(|alias| alias.name.as_str() != "*")
                    .map(|alias| {
                        alias
                            .asname
                            .as_ref()
                            .map_or(alias.name.as_str(), |a| a.as_str())
                    }),
            ),
            _ => {}
        }
    }
    let mut rebound: Vec<String> = Vec::new();
    for (index, name) in bound.iter().enumerate() {
        if bound[..index].contains(name) && !rebound.iter().any(|seen| seen == name) {
            rebound.push((*name).to_string());
        }
    }
    rebound
}

fn has_single_enum_marker_base(def: &StmtClassDef) -> bool {
    let Some(arguments) = def.arguments.as_deref() else {
        return false;
    };
    let [base] = &*arguments.args else {
        return false;
    };
    let Expr::Name(name) = base else {
        return false;
    };
    arguments.keywords.is_empty() && crate::is_enum_base_name(name.id.as_str())
}

/// The module frame: every name the module body binds directly (see the
/// module doc's binding rule). Because the binder records no `def`, `class`,
/// or `import` name, `class Color(Enum)` itself never puts `Color` here --
/// only a plain module-level `Color = 1`, a `for`/`with`/walrus target, an
/// except-handler name, or a `match` capture does.
pub(crate) fn module_bindings(body: &[Stmt], imports: &[ImportBinding]) -> Vec<String> {
    scope_bindings(body, imports)
}

/// The names bound directly by the statements of one scope, without
/// descending into a nested `def`, `class`, or `lambda` (each of those is
/// its own scope and gets its own frame, or none).
fn scope_bindings(body: &[Stmt], imports: &[ImportBinding]) -> Vec<String> {
    struct Binder<'i> {
        imports: &'i [ImportBinding],
        names: Vec<String>,
    }
    impl<'a> Visitor<'a> for Binder<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                // A nested `def`/`class` is neither a binding this frame
                // models (limit (ii)) nor a scope it descends into.
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
                // A `TYPE_CHECKING`-guarded body binds nothing at runtime.
                Stmt::If(if_stmt) => walk_if_as_lowered(self, if_stmt, self.imports),
                _ => visitor::walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Name(name) if matches!(name.ctx, ExprContext::Store) => {
                    self.names.push(name.id.to_string());
                }
                // A lambda is its own scope: a walrus inside it binds there.
                Expr::Lambda(_) => return,
                _ => {}
            }
            visitor::walk_expr(self, expr);
        }
        fn visit_except_handler(&mut self, handler: &'a ExceptHandler) {
            let ExceptHandler::ExceptHandler(except) = handler;
            if let Some(name) = &except.name {
                self.names.push(name.to_string());
            }
            visitor::walk_except_handler(self, handler);
        }
        fn visit_pattern(&mut self, pattern: &'a Pattern) {
            let captured = match pattern {
                Pattern::MatchAs(p) => p.name.as_ref(),
                Pattern::MatchStar(p) => p.name.as_ref(),
                Pattern::MatchMapping(p) => p.rest.as_ref(),
                _ => None,
            };
            if let Some(name) = captured {
                self.names.push(name.to_string());
            }
            visitor::walk_pattern(self, pattern);
        }
    }
    let mut binder = Binder {
        imports,
        names: Vec::new(),
    };
    binder.visit_body(body);
    binder.names
}

/// Every call in `stmt` (at any depth -- a nested `print(Color(1))`, a call
/// inside a `def` body or a comprehension) whose callee is a bare name in
/// `enum_class_names`, whose argument list carries no keyword, and whose
/// name no enclosing frame binds (`module_frame` is the bottom of the
/// stack), as one `C0001` per call at the call expression's own span, in
/// walk order.
///
/// The walk is [`pycc_ast::visitor::Visitor`], for the reason
/// `exception::module_references_builtin_exception_name` gives: a
/// hand-rolled match misses positions silently. `visit_stmt` pushes a frame
/// around each `def` (ruff's `walk_stmt` descends into a class body, so a
/// method gets its frame the same way and a `class` itself gets none --
/// limit (i)); `visit_expr` pushes one around each `lambda`.
///
/// `lower_module` calls this only with a non-empty `enum_class_names`: the
/// walk is the whole item plus, again, each `def` body for its frame, and a
/// module with no enum class in sight must not pay it (`pycc check`'s
/// frontend bench measured ~7% for the unconditional walk).
///
/// `enum_class_names` is assembled by `lower_module` from the syntactic
/// pre-collection plus every `HirClassDef` with `is_enum` known at scan
/// time (an enum pulled in by a project import), minus the names currently
/// poisoned (D-219). The poison filter inherits D-219's rule that *any*
/// failing `class` statement poisons its name: an enum class redefined
/// under the same name (`class Color(Enum): ...` then `class Color: pass`)
/// is reported once, for the duplicate definition, and a later `Color()`
/// is suppressed with it even though the first `Color` is a real enum. The
/// module still fails on the duplicate, so nothing is accepted; the
/// enum-call diagnostic surfaces once the duplicate is removed.
pub(crate) fn reject_enum_class_calls(
    stmt: &Stmt,
    module_frame: &[String],
    enum_class_names: &[&str],
    imports: &[ImportBinding],
) -> Vec<Diagnostic> {
    struct CallScan<'n> {
        enum_class_names: &'n [&'n str],
        module_frame: &'n [String],
        imports: &'n [ImportBinding],
        frames: Vec<Vec<String>>,
        diagnostics: Vec<Diagnostic>,
    }
    impl CallScan<'_> {
        fn is_bound(&self, name: &str) -> bool {
            self.module_frame.iter().any(|n| n == name)
                || self.frames.iter().flatten().any(|n| n == name)
        }
    }
    impl<'a> Visitor<'a> for CallScan<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(def) => {
                    let mut frame: Vec<String> = def
                        .parameters
                        .iter()
                        .map(|parameter| parameter.name().to_string())
                        .collect();
                    frame.extend(scope_bindings(&def.body, self.imports));
                    self.frames.push(frame);
                    visitor::walk_stmt(self, stmt);
                    self.frames.pop();
                }
                // A `TYPE_CHECKING`-guarded body is dead code (#790): a call
                // inside it is never lowered, so it is never an enum call.
                Stmt::If(if_stmt) => walk_if_as_lowered(self, if_stmt, self.imports),
                _ => visitor::walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Call(call)
                    if let Expr::Name(callee) = call.func.as_ref()
                        && call.arguments.keywords.is_empty()
                        && self.enum_class_names.contains(&callee.id.as_str())
                        && !self.is_bound(callee.id.as_str()) =>
                {
                    self.diagnostics.push(crate::unsupported(
                        enum_class_call_message(callee.id.as_str()),
                        call.range,
                    ));
                }
                Expr::Lambda(lambda) => {
                    // `lambda: Color()` has no parameters at all; the frame
                    // is then empty, and the walk below still sees the body.
                    let frame: Vec<String> = lambda
                        .parameters
                        .as_deref()
                        .map(|parameters| {
                            parameters
                                .iter()
                                .map(|parameter| parameter.name().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    self.frames.push(frame);
                    visitor::walk_expr(self, expr);
                    self.frames.pop();
                    return;
                }
                _ => {}
            }
            // Keep walking: an argument may nest another call.
            visitor::walk_expr(self, expr);
        }
    }
    let mut scan = CallScan {
        enum_class_names,
        module_frame,
        imports,
        frames: Vec::new(),
        diagnostics: Vec::new(),
    };
    scan.visit_stmt(stmt);
    scan.diagnostics
}

#[cfg(test)]
mod tests {
    use super::enum_class_call_message;
    use crate::lower_all;
    use pycc_diag::{Diagnostic, Span};

    fn lower_err(source: &str) -> Vec<Diagnostic> {
        let module = crate::pycc_parser_test_helper::parse(source);
        lower_all(&module).expect_err("test fixture should be rejected")
    }

    /// The shapes whose call is suppressed by a frame lower `Ok` here; the
    /// CLI then reaches `pycc_types`, which reports the accurate `T0021`
    /// (or, for the module-frame cases, the span-less guard).
    fn lower_ok(source: &str) {
        let module = crate::pycc_parser_test_helper::parse(source);
        lower_all(&module).expect("the frame must suppress the enum-call scan");
    }

    /// Byte offset of the `occurrence`-th (0-based) `needle` in `source`.
    fn nth_offset(source: &str, needle: &str, occurrence: usize) -> u32 {
        source
            .match_indices(needle)
            .nth(occurrence)
            .map(|(start, _)| start as u32)
            .expect("the call text must occur in the source that many times")
    }

    fn assert_enum_call_at(diagnostic: &Diagnostic, class_name: &str, call_text: &str, start: u32) {
        assert_eq!(diagnostic.code, "C0001");
        assert_eq!(diagnostic.message, enum_class_call_message(class_name));
        assert_eq!(
            diagnostic.span,
            Some(Span::new(start, start + call_text.len() as u32))
        );
    }

    fn assert_enum_call(diagnostic: &Diagnostic, class_name: &str, call_text: &str, source: &str) {
        assert_enum_call_at(
            diagnostic,
            class_name,
            call_text,
            nth_offset(source, call_text, 0),
        );
    }

    const COLOR: &str = "class Color(Enum):\n    RED = 1\n    GREEN = 2\n";

    // -- the reference shapes (#921) --

    #[test]
    fn a_module_level_zero_argument_call_is_rejected_at_the_call() {
        let source = format!("{COLOR}c = Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_value_lookup_call_inside_a_function_body_is_rejected() {
        let source = format!("{COLOR}def f() -> None:\n    c = Color(1)\n    print(c.value)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", &source);
    }

    #[test]
    fn calls_before_and_after_the_class_are_both_found_in_loop_order() {
        // The `def` precedes the class in source, so only the syntactic
        // pre-collection can know `Color` when the `def` is scanned.
        let source = format!("def f() -> None:\n    Color(1)\n{COLOR}Color(2)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2);
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", &source);
        assert_enum_call(&diagnostics[1], "Color", "Color(2)", &source);
    }

    #[test]
    fn a_nested_call_is_found() {
        let source = format!("{COLOR}print(Color(1))\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", &source);
    }

    #[test]
    fn a_non_enum_class_call_and_a_member_access_are_untouched() {
        let source = format!(
            "class P:\n    def __init__(self, x: int) -> None:\n        self.x = x\n{COLOR}p = P(1)\nc = Color.RED\nprint(c.value)\n"
        );
        lower_ok(&source);
    }

    #[test]
    fn a_str_enum_class_call_is_rejected_the_same_way() {
        let source = "class S(StrEnum):\n    A = \"a\"\ns = S(\"a\")\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "S", "S(\"a\")", source);
    }

    #[test]
    fn a_docstring_only_enum_class_call_is_rejected() {
        // #744 accepts a member-less enum; `enum_members` is empty, so only
        // the provenance marker (`is_enum`/the syntactic set) can catch it.
        let source = "class E(Enum):\n    \"doc\"\ne = E()\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1);
        assert_enum_call(&diagnostics[0], "E", "E()", source);
    }

    #[test]
    fn a_call_to_a_poisoned_enum_class_is_a_suppressed_cascade() {
        // `class E(Enum): pass` fails to lower and poisons `E` (D-219); the
        // later `E()` is a consequence of that skip, not a second gap.
        let source = "class E(Enum):\n    pass\ne = E()\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, "C0001");
        assert_ne!(diagnostics[0].message, enum_class_call_message("E"));
    }

    #[test]
    fn a_call_to_a_redefined_enum_class_is_suppressed_with_the_duplicate() {
        // D-219 poisons the name of *any* failing `class` statement, so the
        // duplicate `class Color:` poisons `Color` and the later `Color()`
        // is filtered as a cascade even though the first definition is a
        // real enum. Pinned as a documented limit (see the function doc),
        // not as the preferred outcome: the module still fails to compile.
        let source = format!("{COLOR}class Color:\n    pass\nc = Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let message = &diagnostics[0].message;
        assert!(message.contains("defined more than once"), "{message}");
    }

    #[test]
    fn a_keyword_call_keeps_exactly_the_keyword_diagnostic() {
        let source = format!("{COLOR}c = Color(value=1)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, "C0001");
        let message = &diagnostics[0].message;
        assert!(message.contains("keyword call arguments"), "{message}");
    }

    #[test]
    fn a_class_whose_base_is_not_a_bare_name_is_not_an_enum_class() {
        // `class Color(enum.Enum):` takes the pre-collection's non-`Name`
        // arm and is rejected by `lower_class`'s own base-shape `C0001`;
        // the later `Color()` is then a poisoned-name cascade.
        let source = "class Color(enum.Enum):\n    RED = 1\nc = Color()\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let message = &diagnostics[0].message;
        assert!(
            message.contains("a base class must be a bare name"),
            "{message}"
        );
    }

    // -- scope-local bindings keep their `T0021` (#944): the item lowers --

    #[test]
    fn a_parameter_that_shadows_the_enum_suppresses_the_scan() {
        lower_ok(&format!("{COLOR}def f(Color: int) -> None:\n    Color()\n"));
    }

    #[test]
    fn a_local_assignment_that_shadows_the_enum_suppresses_the_scan() {
        lower_ok(&format!(
            "{COLOR}def f() -> None:\n    Color = 1\n    Color()\n"
        ));
    }

    #[test]
    fn an_except_handler_name_suppresses_a_call_after_it() {
        lower_ok(&format!(
            "{COLOR}def f() -> None:\n    try:\n        pass\n    except ValueError as Color:\n        Color()\n"
        ));
    }

    #[test]
    fn an_except_handler_name_suppresses_a_call_before_it() {
        // The frame is the whole scope, not position-aware (limit (i)).
        lower_ok(&format!(
            "{COLOR}def f() -> None:\n    Color()\n    try:\n        pass\n    except ValueError as Color:\n        pass\n"
        ));
    }

    #[test]
    fn a_match_capture_suppresses_the_scan() {
        lower_ok(&format!(
            "{COLOR}def f(x: int) -> None:\n    match x:\n        case Color:\n            Color()\n"
        ));
    }

    #[test]
    fn a_match_star_capture_suppresses_the_scan() {
        lower_ok(&format!(
            "{COLOR}def f(xs: list[int]) -> None:\n    match xs:\n        case [*Color]:\n            Color()\n"
        ));
    }

    #[test]
    fn a_match_mapping_rest_capture_suppresses_the_scan() {
        lower_ok(&format!(
            "{COLOR}def f(d: dict[str, int]) -> None:\n    match d:\n        case {{**Color}}:\n            Color()\n"
        ));
    }

    #[test]
    fn a_module_level_assignment_that_shadows_the_enum_suppresses_the_scan() {
        // Side by side with `a_module_level_zero_argument_call_is_rejected_at_the_call`:
        // `class Color(Enum)` itself is not a module-frame binding, a plain
        // `Color = 1` is.
        lower_ok(&format!("{COLOR}Color = 1\nColor()\n"));
    }

    #[test]
    fn a_module_level_for_target_that_shadows_the_enum_suppresses_the_scan() {
        lower_ok(&format!("{COLOR}for Color in range(3):\n    Color()\n"));
    }

    #[test]
    fn a_module_level_call_before_a_later_shadowing_assignment_is_suppressed() {
        // Limit (i): the module frame is not position-aware, so the later
        // `Color = 1` hides the earlier `Color()`; `pycc_types`' span-less
        // guard still rejects it.
        lower_ok(&format!("{COLOR}Color()\nColor = 1\n"));
    }

    #[test]
    fn a_comprehension_target_suppresses_a_sibling_call_in_the_same_def() {
        // Limit (i): the comprehension target is recorded in the enclosing
        // `def`'s frame, so a sibling `Color(1)` outside the comprehension
        // is suppressed too; `pycc_types` still rejects it.
        lower_ok(&format!(
            "{COLOR}def f() -> None:\n    xs = [Color for Color in range(3)]\n    Color(1)\n"
        ));
    }

    // -- `TYPE_CHECKING` guards (#790 fold, PR #971 review): dead bodies --

    const TC: &str = "from typing import TYPE_CHECKING\n";

    #[test]
    fn a_call_under_a_type_checking_guard_is_dead_code_and_not_scanned() {
        lower_ok(&format!("{TC}{COLOR}if TYPE_CHECKING:\n    Color(1)\n"));
    }

    #[test]
    fn a_call_under_an_elif_type_checking_guard_is_dead_code_and_not_scanned() {
        lower_ok(&format!(
            "{TC}{COLOR}x = 1\nif x == 2:\n    pass\nelif TYPE_CHECKING:\n    Color(1)\n"
        ));
    }

    #[test]
    fn a_qualified_type_checking_guard_inside_a_def_is_folded_the_same_way() {
        lower_ok(&format!(
            "import typing\n{COLOR}def f() -> None:\n    if typing.TYPE_CHECKING:\n        Color(1)\n"
        ));
    }

    #[test]
    fn the_live_clauses_around_a_type_checking_guard_are_still_scanned() {
        // The leading `if` is live (not a guard), the `elif` is folded, the
        // `else` is live: exactly the two live calls, in source order.
        let source = format!(
            "{TC}{COLOR}x = 1\nif x == 2:\n    Color()\nelif TYPE_CHECKING:\n    Color(1)\nelse:\n    Color(2)\n"
        );
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
        assert_enum_call(&diagnostics[1], "Color", "Color(2)", &source);
    }

    #[test]
    fn the_else_of_a_leading_type_checking_guard_is_live() {
        let source = format!("{TC}{COLOR}if TYPE_CHECKING:\n    pass\nelse:\n    Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_module_level_binding_under_a_type_checking_guard_does_not_shadow() {
        // The guarded `Color = 1` never runs, so the runtime `Color` is the
        // enum and the call is reported -- the frame skips the dead body.
        let source = format!("{TC}{COLOR}if TYPE_CHECKING:\n    Color = 1\nColor()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_def_level_binding_under_a_type_checking_guard_does_not_shadow() {
        let source = format!(
            "{TC}{COLOR}def f() -> None:\n    if TYPE_CHECKING:\n        Color = 1\n    Color()\n"
        );
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn an_aliased_module_level_guard_binding_is_the_documented_frame_residual() {
        // Limit (iv): the module frame is computed before the loop lowers
        // `import typing as t`, so `t.TYPE_CHECKING` is not recognized there
        // and the dead `Color = 1` still suppresses the call (over-suppression;
        // `pycc_types`' guard rejects it). Inside a `def` the scan runs with
        // the import known, so the same alias folds -- see the next test.
        lower_ok(&format!(
            "import typing as t\n{COLOR}if t.TYPE_CHECKING:\n    Color = 1\nColor()\n"
        ));
    }

    #[test]
    fn an_aliased_guard_inside_a_def_is_folded_once_the_import_is_lowered() {
        let source = format!(
            "import typing as t\n{COLOR}def f() -> None:\n    if t.TYPE_CHECKING:\n        Color = 1\n    Color()\n"
        );
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    // -- frame boundaries and item ordering (#944): exact counts --

    #[test]
    fn a_lambda_parameter_shadow_keeps_exactly_the_lambda_diagnostic() {
        let source = format!("{COLOR}g = lambda Color: Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let message = &diagnostics[0].message;
        assert!(message.contains("a `lambda`"), "{message}");
    }

    #[test]
    fn a_parameter_less_lambda_body_call_is_reported_after_the_lambda_diagnostic() {
        let source = format!("{COLOR}g = lambda: Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("a `lambda`"));
        assert_enum_call(&diagnostics[1], "Color", "Color()", &source);
    }

    #[test]
    fn a_walrus_inside_a_lambda_does_not_bind_the_module_frame() {
        // The binder does not descend into a lambda: the walrus binds in the
        // lambda's own scope, so the module-level `Color()` is still found
        // (after the lambda's own `C0001`, the item's first diagnostic).
        let source = format!("{COLOR}g = lambda: (Color := 1)\nColor()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("a `lambda`"));
        assert_enum_call(&diagnostics[1], "Color", "Color()", &source);
    }

    #[test]
    fn a_shadow_in_one_function_does_not_leak_into_a_sibling() {
        let source = format!(
            "{COLOR}def f() -> None:\n    Color = 1\n    Color()\ndef g() -> None:\n    Color()\n"
        );
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call_at(
            &diagnostics[0],
            "Color",
            "Color()",
            nth_offset(&source, "Color()", 1),
        );
    }

    #[test]
    fn a_shadow_inside_a_function_does_not_bind_the_module_frame() {
        let source = format!("{COLOR}def g() -> None:\n    Color = 1\nColor()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_raised_enum_call_is_rejected_at_the_call() {
        let source = format!("{COLOR}raise Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_failed_item_carries_its_own_diagnostic_before_the_scan_s() {
        let source =
            format!("{COLOR}def f() -> None:\n    with open(\"x\") as y:\n        Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, "C0001");
        let message = &diagnostics[0].message;
        assert!(message.contains("with"), "{message}");
        assert_enum_call(&diagnostics[1], "Color", "Color()", &source);
    }

    #[test]
    fn a_call_scanned_before_its_class_fails_is_reported_first() {
        // Limit (iv): the `def` is scanned before the class item poisons
        // `Color`, so the true enum-call report precedes the class's own.
        let source = "def f() -> None:\n    Color(1)\nclass Color(Enum):\n    pass\n";
        let diagnostics = lower_err(source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color(1)", source);
        assert_eq!(diagnostics[1].code, "C0001");
        assert_ne!(diagnostics[1].message, enum_class_call_message("Color"));
    }

    #[test]
    fn a_starred_argument_call_reports_the_starred_diagnostic_then_the_call() {
        let source = format!("{COLOR}def f(xs: list[int]) -> None:\n    Color(*xs)\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        let starred = nth_offset(&source, "*xs", 0);
        assert_eq!(diagnostics[0].code, "C0001");
        assert_eq!(diagnostics[0].span, Some(Span::new(starred, starred + 3)));
        assert_enum_call(&diagnostics[1], "Color", "Color(*xs)", &source);
    }

    #[test]
    fn a_class_body_call_reports_the_attribute_diagnostic_then_the_call() {
        let source = format!("{COLOR}class K:\n    X = Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        let message = &diagnostics[0].message;
        assert!(message.contains("class attribute `X`"), "{message}");
        assert_enum_call(&diagnostics[1], "Color", "Color()", &source);
    }

    #[test]
    fn a_class_body_shadow_is_the_documented_false_kind_residual() {
        // Limit (i): a class body gets no frame, so `Color = 1` there does
        // not suppress the scan. Pinned so the residual cannot drift.
        let source = format!("{COLOR}class K:\n    Color = 1\n    X = Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        let message = &diagnostics[0].message;
        assert!(message.contains("class attribute `X`"), "{message}");
        assert_enum_call(&diagnostics[1], "Color", "Color()", &source);
    }

    #[test]
    fn a_method_body_call_is_rejected_at_the_call() {
        let source = format!("{COLOR}class K:\n    def m(self) -> None:\n        Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    #[test]
    fn a_class_body_binding_does_not_reach_a_method_body() {
        let source =
            format!("{COLOR}class K:\n    Color = 1\n    def m(self) -> None:\n        Color()\n");
        let diagnostics = lower_err(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_enum_call(&diagnostics[0], "Color", "Color()", &source);
    }

    /// Limit (vii): a module that binds one name through two module-level
    /// `def`/`class`/`import`/`type` statements (one of them the enum
    /// class) is reported by the collision diagnostic alone -- the scan
    /// never claims a call to that name, in either definition order
    /// (PR #971 review: `def Color()` / `Color()` / `class Color(Enum)`,
    /// and then an ordinary `class Color` in the same position, used to
    /// report a false-kind enum-call `C0001` first).
    fn assert_only_the_collision_is_reported(source: &str) {
        let diagnostics = lower_err(source);
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.contains("collides with") || m.contains("defined more than once"))
                .count(),
            1,
            "exactly one collision diagnostic expected, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .all(|m| !m.contains("cannot call enum class")),
            "the scan must not claim a rebound name, got {messages:?}"
        );
    }

    #[test]
    fn a_def_bound_name_is_never_scanned_when_the_def_comes_first() {
        assert_only_the_collision_is_reported(
            "from enum import Enum\n\ndef Color() -> int:\n    return 1\n\nColor()\n\nclass Color(Enum):\n    RED = 1\n",
        );
    }

    #[test]
    fn a_def_bound_name_is_never_scanned_when_the_class_comes_first() {
        assert_only_the_collision_is_reported(
            "from enum import Enum\n\nclass Color(Enum):\n    RED = 1\n\ndef Color() -> int:\n    return 1\n\nColor()\n",
        );
    }

    #[test]
    fn a_def_body_call_to_a_def_bound_enum_name_is_not_scanned_either() {
        assert_only_the_collision_is_reported(
            "from enum import Enum\n\ndef use() -> None:\n    Color()\n\ndef Color() -> int:\n    return 1\n\nclass Color(Enum):\n    RED = 1\n",
        );
    }

    #[test]
    fn an_ordinary_class_bound_name_is_never_scanned_when_the_class_comes_first() {
        assert_only_the_collision_is_reported(
            "from enum import Enum\n\nclass Color:\n    pass\n\nColor()\n\nclass Color(Enum):\n    RED = 1\n",
        );
    }

    #[test]
    fn a_type_alias_bound_name_is_never_scanned_when_the_alias_comes_first() {
        assert_only_the_collision_is_reported(
            "from enum import Enum\n\ntype Color = int\n\nColor()\n\nclass Color(Enum):\n    RED = 1\n",
        );
    }

    #[test]
    fn module_rebound_names_lists_names_bound_by_two_or_more_statements() {
        let module = crate::pycc_parser_test_helper::parse(concat!(
            "import a.b\n",
            "import c as a\n",
            "from m import x, y as z\n",
            "from n import *\n",
            "def z() -> None:\n    def inner() -> None:\n        pass\n",
            "class K:\n    def method(self) -> None:\n        pass\n",
            "type K = int\n",
            "async def once() -> None:\n    pass\n",
            "class Once(Enum):\n    pass\n",
            "def x() -> None:\n    pass\n",
            "def x() -> None:\n    pass\n",
        ));
        assert_eq!(
            super::module_rebound_names(&module.body),
            vec!["a", "z", "K", "x"]
        );
    }
}
