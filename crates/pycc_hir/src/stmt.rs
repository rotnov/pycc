//! Statement lowering: `lower_stmt`, `lower_body`, and
//! `lower_elif_else_clauses`, the recursive-descent core that walks a
//! function or module body and produces `HirStmt` nodes (or a capability /
//! context-invalidity diagnostic).
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #141): this is the one part of `crates/pycc_hir/src/lib.rs` this change
//! touches, and it was already a well-defined, low-fan-in cohesion unit (11
//! call expressions total -- 9 within this module's own recursive descent
//! plus 2 from `lib.rs` -- all in production code, zero from the inline test
//! module -- every existing test reaches lowering exclusively through the
//! public `lower_checked` entry point in `lib.rs`).
//!
//! `in_loop: bool` (D-148) is the module's own piece of state: `true`
//! exactly when the statement being lowered is (transitively) inside a
//! `while`/`for`/`for-range` loop body reached via one of *this* module's
//! own `lower_body` calls -- entering a function body via `lower_function`
//! resets it to `false`, and an `if`/`elif`/`else` branch passes its caller's
//! value through unchanged (a conditional does not introduce or reset loop
//! nesting). It exists so `Stmt::Break`/`Stmt::Continue` can distinguish a
//! context-invalid occurrence (no enclosing loop -- CPython raises
//! `SyntaxError`, so this now reuses `L0001`) from a valid-but-unimplemented
//! occurrence (a real enclosing loop -- still `C0001`, loop control-flow
//! codegen remains out of scope).
//!
//! `in_function: bool` (D-149, issue #361's own follow-up) is this module's
//! second piece of threaded context, but purely as a pass-through: none of
//! this module's own `in_loop`-driven logic reads it. `lower_stmt`/
//! `lower_body`/`lower_elif_else_clauses` all gained it because they sit on
//! the only route from the two places that decide it (`lower_checked`'s
//! module-level dispatch and `lower_function`'s body dispatch, both in
//! `lib.rs`) to `expr.rs`'s `lower_expr`, which every one of these three
//! functions calls directly (test expressions, `Return` values, `AnnAssign`
//! values, assignment/subscript values) -- D-148's own framing ("the two
//! lowering passes stay independently parameterized") remains true in the
//! sense that the two flags never share a variable or collapse into one
//! enum, but is refined by D-149: this module is not left untouched by an
//! `expr.rs`-side flag the way that framing might suggest.
//!
//! `in_finally: bool` (D-193, PEP 765, issue #738 -- Part 1 of #543) is this
//! module's third piece of threaded context, and it governs `Stmt::Return`,
//! `Stmt::Break`, and `Stmt::Continue` uniformly. It becomes `true` while
//! lowering the `finalbody` of a `try`/`finally` (`Stmt::Try`'s own `body`,
//! `handlers`, and `orelse` inherit whatever value was already in scope --
//! entering a `finally` clause is the only thing that sets it), propagates
//! unchanged through `if`/`elif`/`else`, nested `try`'s non-`finally` parts,
//! and `match`/`case`, and is reset to `false` while lowering the body of a
//! `while`/`for` loop reached from inside that `finally` (mirroring
//! `in_loop`'s own reset on loop entry) -- entering a function body via
//! `lower_function` resets it to `false` too, exactly like `in_loop`.
//!
//! The finally-specific diagnostic only fires when a valid escape target
//! also exists: `in_finally && in_function` for `Stmt::Return`, `in_finally
//! && in_loop` for `Stmt::Break`/`Stmt::Continue`. Verified directly against
//! `python3.14 -W all`: a `return`/`break`/`continue` directly in a
//! `finally` with NO valid target anywhere (e.g. a bare `break` at module
//! scope) makes CPython raise the pre-existing `SyntaxError` for that
//! missing target (`'break' outside loop`, `'continue' not properly in
//! loop`, `'return' outside function`) as the actual fatal error --
//! CPython's finally-specific `SyntaxWarning` prints alongside it, but is
//! not itself fatal and does not override the missing-target error. Only
//! once a real target exists outside the `finally` does CPython's fatal
//! failure become the finally-specific one (with exit code 0, since a
//! `SyntaxWarning` alone does not fail compilation). pycc mirrors that
//! precedence: without a valid target, this module keeps deferring to its
//! own pre-existing `in_loop`/`T0024` handling instead of reporting the
//! finally-specific message.
//!
//! This exact shape -- one flag shared by all three statement kinds, reset
//! by the nearest enclosing loop but not by a plain conditional or a nested
//! non-`finally` `try` part -- was verified empirically against CPython
//! 3.14's actual `SyntaxWarning` behavior (not assumed from the PEP text
//! alone): `return`, `break`, and `continue` are equally suppressed by an
//! intervening loop inside the `finally`, even though a `return` inside that
//! inner loop still exits the function exactly as it would without the
//! loop. A naive reading of PEP 765 predicts `return` should NOT be shielded
//! by an intervening loop the way `break`/`continue` are (a `return` always
//! escapes the `finally` regardless of loop nesting) -- CPython's compiler
//! does not implement it that way, so a two-flag design mirroring that naive
//! reading would silently diverge from upstream's own diagnostic surface.
//! `while`/`for`'s own `else:` clause is untested here because both are
//! unsupported syntax in this compiler today (`while/else`/`for/else` reject
//! with `C0001` before either loop kind's body is lowered).

mod exception;

use crate::class::ClassAnnotationInfo;
use crate::expr::{
    contains_named_expr, is_zero_arg_super_call, lower_dict_comp_assign, lower_expr,
    lower_list_comp_assign, lower_range_call, lower_set_comp_assign,
};
use crate::{
    CompIter, HirExpr, HirMatchCase, HirPattern, HirStmt, Ty, annotation_to_ty, context_invalid,
    unsupported,
};
use exception::lower_except_handler;
use pycc_ast::{ElifElseClause, Expr, Pattern, Singleton, Stmt, StmtMatch};
use pycc_diag::Diagnostic;

/// True exactly when `test` is CPython's standard `typing.TYPE_CHECKING`
/// guard expression, spelled either as the bare name (`from typing import
/// TYPE_CHECKING`) or the qualified attribute access (`import typing`,
/// then `typing.TYPE_CHECKING`) (#790). Checked purely syntactically,
/// matching this module's existing bare-name `Final` precedent (`is_final`,
/// a local `let` binding below in the `Stmt::AnnAssign` arm, not a separate
/// function) and `expr.rs`'s textual, non-import-gated resolution of
/// `math.sqrt`-shaped attribute access -- `lower_stmt`/`lower_body` have no
/// access to the module-level import side-table `lower_checked` builds, so
/// this recognizes the two concrete spellings the real-world idiom uses
/// rather than resolving a general expression through `pycc_std`.
/// Deliberately does not recognize a negated or compound test (`if not
/// TYPE_CHECKING:`, `if TYPE_CHECKING and x:`) -- only the bare guard
/// itself has the real-world precedent (bodies containing constructs pycc
/// doesn't support) this fold exists to unblock; a compound test is left
/// to ordinary lowering, which type-checks both branches as it would any
/// other `if`. Since #790 registered `TYPE_CHECKING` as a resolvable
/// marker symbol, using it inside such a test now reaches the dedicated
/// `type_checking_marker_is_not_a_value` diagnostic (T0021) rather than
/// the unresolved-import error (C0002) it produced before #790 -- a
/// different diagnostic at a later pipeline stage, not unchanged behavior.
///
/// Known gap (tracked in #798, deliberately not fixed here): this check is
/// not import-gated and not shadow-aware. A module that never imports
/// `typing`/`TYPE_CHECKING` but happens to bind its own truthy module-level
/// `TYPE_CHECKING` name would, under CPython, execute the guarded body --
/// this compiler still folds it away as dead code, silently diverging from
/// CPython for that (contrived) program. #798 tracks gating the fold on an
/// actual `typing` import.
fn is_type_checking_guard(test: &Expr) -> bool {
    match test {
        Expr::Name(name) => name.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(attr) => {
            attr.attr.as_str() == "TYPE_CHECKING"
                && matches!(
                    attr.value.as_ref(),
                    Expr::Name(receiver) if receiver.id.as_str() == "typing"
                )
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_stmt(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    in_finally: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[ClassAnnotationInfo],
) -> Result<HirStmt, Diagnostic> {
    let lowered = match stmt {
        Stmt::Expr(expr_stmt) => {
            HirStmt::ExprStmt(lower_expr(&expr_stmt.value, in_function, class_name)?)
        }
        Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                return Err(unsupported(
                    format!(
                        "only a single assignment target is supported so far: {:?}",
                        assign.targets
                    ),
                    assign.range,
                ));
            };
            match target {
                Expr::Name(name) => match assign.value.as_ref() {
                    // Comprehension expressions are recognized only in this
                    // one position: the direct RHS of a bare-name
                    // `Stmt::Assign` (PR-12, D-117). Every other position
                    // (function args, nested expressions, `return`,
                    // `Expr::Subscript` assignment targets) still routes
                    // through plain `lower_expr`, which has no arm for
                    // `Expr::ListComp`/`SetComp`/`DictComp`/`GeneratorExp`
                    // and falls through to that function's existing
                    // generic "expression kind not supported yet"
                    // catch-all.
                    Expr::ListComp(comp) => {
                        lower_list_comp_assign(name.id.as_str(), comp, class_name)?
                    }
                    Expr::SetComp(comp) => {
                        lower_set_comp_assign(name.id.as_str(), comp, class_name)?
                    }
                    Expr::DictComp(comp) => {
                        lower_dict_comp_assign(name.id.as_str(), comp, class_name)?
                    }
                    _ => HirStmt::Assign {
                        target: name.id.as_str().to_string(),
                        value: lower_expr(&assign.value, in_function, class_name)?,
                    },
                },
                // `<bare name>[key] = value`, PR-11 Task 3 (D-123): unlike
                // `list[int]`'s own read-only-indexing consequence (D-105),
                // `dict[str, int]` ships `d[k] = v`. This lowering step has
                // no type information (mirroring `ForList`'s own bare-name
                // iterable, which is resolved to `Ty::List`, `Ty::Dict`, or
                // rejected downstream), so a `list[int]` subscript-assignment target
                // also reaches `HirStmt::DictSet` here -- `pycc_types`
                // rejects it with `T0033` once the base's real type is
                // known, relocating (not removing) the invariant
                // `subscript_assignment_to_a_non_bare_name_base_is_unsupported`
                // (`crates/pycc_hir/src/tests.rs`) used to enforce at the
                // lowering level.
                Expr::Subscript(sub) => {
                    let Expr::Name(base_name) = sub.value.as_ref() else {
                        return Err(unsupported(
                            "only assigning to a bare-name subscript target (`name[key] = value`) is supported so far",
                            pycc_ast::expr_range(target),
                        ));
                    };
                    HirStmt::DictSet {
                        dict: base_name.id.as_str().to_string(),
                        key: lower_expr(&sub.slice, in_function, class_name)?,
                        value: lower_expr(&assign.value, in_function, class_name)?,
                    }
                }
                // `base.attr = value` (D-154, Part 1 of #375): structurally
                // recognized for any base expression, exactly like
                // `HirExpr::AttrGet`'s own `base` (no type information is
                // available at this lowering step to narrow it to only
                // `self` or only an instance-typed receiver -- `pycc_types`
                // rejects a non-instance base or an undeclared attribute
                // name). This supersedes the older, narrower invariant that
                // used to reject any non-bare-name `Stmt::Assign` target
                // outright ("only assigning to a bare name is supported so
                // far"). The remaining unsupported `Stmt::Assign` target
                // shape -- multi-target tuple unpacking, e.g. `a, b = 1, 2`
                // -- still reaches the `other => ..` catch-all just below
                // and is covered by
                // `assigning_to_a_tuple_unpacking_target_is_unsupported` in
                // `crates/pycc_hir/src/tests.rs`.
                Expr::Attribute(attr) => {
                    // #448: `super().attr = value` — super() attribute
                    // assignment is not implemented in this version. Without
                    // this special case, `super().attr = value` would lower
                    // the `super()` base through the generic `lower_expr`
                    // path, which rejects a bare `super()` with a confusing
                    // "a bare `super()` expression is not supported" message
                    // that doesn't name the actual unsupported operation
                    // (attribute assignment through super()). Emit a dedicated
                    // C0001 diagnostic instead.
                    if is_zero_arg_super_call(&attr.value) {
                        return Err(unsupported(
                            "super().attr = value is not supported yet — super() attribute \
                             assignment is not implemented in this version",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    }
                    HirStmt::AttrSet {
                        base: lower_expr(&attr.value, in_function, class_name)?,
                        attr: attr.attr.to_string(),
                        value: lower_expr(&assign.value, in_function, class_name)?,
                    }
                }
                other => {
                    return Err(unsupported(
                        format!("only assigning to a bare name is supported so far: {other:?}"),
                        pycc_ast::expr_range(other),
                    ));
                }
            }
        }
        Stmt::AnnAssign(ann) => {
            let Expr::Name(name) = ann.target.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only assigning to a bare name is supported so far: {:?}",
                        ann.target
                    ),
                    pycc_ast::expr_range(&ann.target),
                ));
            };
            // `ann.simple` is false either when the target isn't a bare name
            // (already rejected above) or when a bare name target is itself
            // parenthesized, e.g. `(x): int = 1` -- upstream's own parser
            // sets `simple = target.is_name_expr() && !target.is_parenthesized`
            // (verified against the pinned ruff_python_parser = "0.0.6"
            // registry source). CPython treats a parenthesized target as not
            // "simple" (it doesn't record a `__annotations__` entry the same
            // way), a real semantic difference this compiler doesn't model
            // yet -- reject explicitly instead of silently treating it the
            // same as the unparenthesized form.
            if !ann.simple {
                return Err(unsupported(
                    "a parenthesized annotated-assignment target is not supported yet",
                    pycc_ast::expr_range(&ann.target),
                ));
            }
            let annotation =
                annotation_to_ty(&ann.annotation, type_param, class_name, aliases, class_defs)?;
            let value = ann
                .value
                .as_deref()
                .map(|e| lower_expr(e, in_function, class_name))
                .transpose()?;
            // PEP 591 (#383): detect `Final[X]` at the AST level (before
            // `annotation_to_ty` unwrapped it to `X`) so the type checker
            // can track this binding as non-reassignable. `Final` is
            // recognized as a bare name without requiring `from typing
            // import Final`, matching the existing `TypeAlias`/`Any`
            // precedent.
            let is_final = matches!(
                ann.annotation.as_ref(),
                Expr::Subscript(sub) if matches!(
                    sub.value.as_ref(),
                    Expr::Name(base) if base.id.as_str() == "Final"
                )
            );
            HirStmt::AnnAssign {
                target: name.id.as_str().to_string(),
                annotation,
                value,
                is_final,
            }
        }
        Stmt::If(if_stmt) if is_type_checking_guard(&if_stmt.test) => {
            // #790: `if TYPE_CHECKING:` is CPython's standard idiom for
            // guarding imports/statements meant only for static type
            // checkers -- `typing.TYPE_CHECKING` is always `False` at
            // runtime, so the guarded body never executes. Constant-fold it
            // here, before either `lower_expr` or `lower_body` ever sees the
            // body: the guard commonly wraps constructs this compiler
            // doesn't support elsewhere (forward-reference-only imports,
            // typing-only names), and since the body is genuinely dead code
            // it must never block compilation of the rest of the module.
            // Only `orelse` (an `elif`/`else` chain, live at runtime
            // whenever the `TYPE_CHECKING` guard itself is skipped) is
            // lowered normally. The synthesized `HirExpr::BoolLiteral(false)`
            // test documents the fold in the HIR itself rather than
            // silently keeping the original (never evaluated) test
            // expression around.
            HirStmt::If {
                test: HirExpr::BoolLiteral(false),
                body: vec![],
                orelse: lower_elif_else_clauses(
                    &if_stmt.elif_else_clauses,
                    aliases,
                    in_loop,
                    in_function,
                    in_finally,
                    class_name,
                    type_param,
                    class_defs,
                )?,
            }
        }
        Stmt::If(if_stmt) => HirStmt::If {
            test: lower_expr(&if_stmt.test, in_function, class_name)?,
            body: lower_body(
                &if_stmt.body,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?,
            orelse: lower_elif_else_clauses(
                &if_stmt.elif_else_clauses,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?,
        },
        Stmt::While(while_stmt) => {
            if !while_stmt.orelse.is_empty() {
                return Err(unsupported(
                    "while/else is not supported yet",
                    while_stmt.range,
                ));
            }
            HirStmt::While {
                test: lower_expr(&while_stmt.test, in_function, class_name)?,
                body: lower_body(
                    &while_stmt.body,
                    aliases,
                    true,
                    in_function,
                    // A loop entered from inside a `finally` shields its own
                    // body from the outer `finally`'s PEP 765 restriction --
                    // verified against CPython 3.14 (see this module's own
                    // doc comment above).
                    false,
                    class_name,
                    type_param,
                    class_defs,
                )?,
            }
        }
        Stmt::For(for_stmt) => {
            if for_stmt.is_async {
                // `async for` is only valid Python syntax inside an `async
                // def` body, but `lower_function` unconditionally rejects
                // any `async def` (D-141's own `def.is_async` check, earlier
                // in this file) before its body is ever lowered -- so this
                // arm can only ever be reached from a synchronous function
                // body or from module scope, never from inside a real async
                // function. There is therefore no reachable "valid Python,
                // just not implemented yet" case here today: every
                // occurrence is context-invalid, exactly like a top-level
                // `break`/`continue` (see the `Stmt::Break`/`Stmt::Continue`
                // arms above). Revisit this once/if async function support
                // lands -- it would reopen a genuine valid-but-unimplemented
                // case this arm cannot distinguish from today (D-148).
                return Err(context_invalid(
                    "'async for' outside async function",
                    for_stmt.range,
                ));
            }
            if !for_stmt.orelse.is_empty() {
                return Err(unsupported("for/else is not supported yet", for_stmt.range));
            }
            let Expr::Name(var) = for_stmt.target.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only a bare name for-target is supported so far: {:?}",
                        for_stmt.target
                    ),
                    pycc_ast::expr_range(&for_stmt.target),
                ));
            };
            // A bare-name iterable is `for v in some_list:` (D-105) or
            // `for k in some_dict:` (PR-11 Task 3, D-123) -- resolved to
            // `Ty::List`, `Ty::Dict`, or rejected by pycc_types, not here;
            // HIR only records the syntactic shape.
            if let Expr::Name(list_name) = for_stmt.iter.as_ref() {
                return Ok(HirStmt::ForList {
                    var: var.id.to_string(),
                    list: list_name.id.as_str().to_string(),
                    body: lower_body(
                        &for_stmt.body,
                        aliases,
                        true,
                        in_function,
                        // See the `Stmt::While` arm above -- the same
                        // CPython-verified shielding rule applies to a
                        // `for` loop's body.
                        false,
                        class_name,
                        type_param,
                        class_defs,
                    )?,
                });
            }
            let Expr::Call(call) = for_stmt.iter.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only `for x in range(...)` or `for x in <list>` is supported so far: {:?}",
                        for_stmt.iter
                    ),
                    pycc_ast::expr_range(&for_stmt.iter),
                ));
            };
            let Expr::Name(callee) = call.func.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only `for x in range(...)` is supported so far: {:?}",
                        call.func
                    ),
                    pycc_ast::expr_range(&call.func),
                ));
            };
            if callee.id.as_str() != "range" {
                return Err(unsupported(
                    format!(
                        "only iterating over `range(...)` is supported so far, got `{}`",
                        callee.id
                    ),
                    call.range,
                ));
            }
            if !call.arguments.keywords.is_empty() {
                return Err(unsupported(
                    "keyword arguments to range() are not supported yet",
                    call.range,
                ));
            }
            let (start, stop, step) = lower_range_call(call, in_function, class_name)?;
            HirStmt::ForRange {
                var: var.id.to_string(),
                start,
                stop,
                step,
                body: lower_body(
                    &for_stmt.body,
                    aliases,
                    true,
                    in_function,
                    // See the `Stmt::While` arm above -- the same
                    // CPython-verified shielding rule applies here too.
                    false,
                    class_name,
                    type_param,
                    class_defs,
                )?,
            }
        }
        Stmt::Return(ret) => {
            if in_finally && in_function {
                // PEP 765 (issue #738, Part 1 of #543): a `return` that
                // would exit a `finally` block is rejected outright, not
                // merely a lint -- CPython treats it as valid syntax up
                // through 3.13 (a `SyntaxWarning` in 3.14) but this compiler
                // never accepted the permissive reading. `L0001` is reused
                // (not a new code) for exactly this class of post-parse
                // context violation, matching the sibling `break`/`continue`
                // checks below.
                //
                // The `in_function` guard matches CPython's own precedence,
                // verified directly against `python3.14 -W all`: a `return`
                // in a `finally` with NO enclosing function at all (e.g. at
                // module scope) makes CPython raise the pre-existing
                // `SyntaxError: 'return' outside function` as the actual
                // fatal error -- the finally-specific `SyntaxWarning` prints
                // too, but does not by itself block compilation, and CPython
                // still fails the module on the "outside function" error
                // regardless of the finally warning's wording. Without this
                // guard, pycc would report the wrong diagnostic (the
                // finally-specific message) instead of deferring to the
                // pre-existing `T0024` ("`return` outside a function") path
                // this module already relies on for every other top-level
                // `return`.
                return Err(context_invalid(
                    "'return' in a 'finally' block",
                    pycc_ast::stmt_range(stmt),
                ));
            }
            HirStmt::Return(
                ret.value
                    .as_deref()
                    .map(|e| lower_expr(e, in_function, class_name))
                    .transpose()?,
            )
        }
        Stmt::Break(_) => {
            if in_finally && in_loop {
                // See the `Stmt::Return` arm above for the general PEP 765
                // rationale. The `in_loop` guard matches CPython's own
                // precedence, verified directly against `python3.14 -W all`:
                // a `break` in a `finally` with NO enclosing loop at all
                // makes CPython raise the pre-existing `SyntaxError: 'break'
                // outside loop` as the actual fatal error (the
                // finally-specific `SyntaxWarning` prints too, but is not by
                // itself fatal) -- so this diagnostic only applies once a
                // valid loop target genuinely exists for the `break` to
                // escape to (the classic `while: try: finally: break` case).
                // When that loop is instead defined *inside* the `finally`
                // (shielding it), `in_finally` is already reset to `false`
                // by the time this arm runs, so this branch is not reached
                // and the classic `in_loop` handling below applies as usual.
                return Err(context_invalid(
                    "'break' in a 'finally' block",
                    pycc_ast::stmt_range(stmt),
                ));
            }
            return Err(if in_loop {
                // A real enclosing loop -- valid Python, break/continue
                // control-flow codegen is just not implemented yet.
                unsupported(
                    "statement kind not supported yet",
                    pycc_ast::stmt_range(stmt),
                )
            } else {
                // No enclosing loop -- CPython rejects this as a
                // `SyntaxError`, not "valid but unimplemented" (D-148). This
                // is also the fatal error CPython reports for a `break`
                // directly in a `finally` with no loop anywhere (see above).
                context_invalid("'break' outside loop", pycc_ast::stmt_range(stmt))
            });
        }
        Stmt::Continue(_) => {
            if in_finally && in_loop {
                // See the `Stmt::Break` arm above -- identical rationale and
                // CPython precedence (`SyntaxError: 'continue' not properly
                // in loop` is the fatal error when no loop target exists).
                return Err(context_invalid(
                    "'continue' in a 'finally' block",
                    pycc_ast::stmt_range(stmt),
                ));
            }
            return Err(if in_loop {
                unsupported(
                    "statement kind not supported yet",
                    pycc_ast::stmt_range(stmt),
                )
            } else {
                context_invalid(
                    "'continue' not properly in loop",
                    pycc_ast::stmt_range(stmt),
                )
            });
        }
        Stmt::Match(match_stmt) => lower_match(
            match_stmt,
            aliases,
            in_loop,
            in_function,
            in_finally,
            class_name,
            type_param,
            class_defs,
        )?,
        Stmt::Try(try_stmt) => {
            let body = lower_body(
                &try_stmt.body,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?;
            let handlers = try_stmt
                .handlers
                .iter()
                .map(|h| {
                    // Part 3 of #382 (#542, PEP 654): `except*` requires
                    // every clause to name a type. Unlike PEP 758's
                    // "at least one name in a tuple" rule (checked in
                    // `lower_except_handler`, since ruff's parser accepts an
                    // empty `except ():`), a completely typeless
                    // `except*:` is rejected by ruff's own parser as a
                    // syntax error before this lowering pass ever runs (see
                    // `lower_try_star_bare_except_star_is_rejected_at_parse_time`),
                    // so no defensive re-check belongs here -- one would be
                    // unreachable and untestable under this repository's
                    // 100%-region coverage gate.
                    lower_except_handler(
                        h,
                        aliases,
                        in_loop,
                        in_function,
                        in_finally,
                        class_name,
                        type_param,
                        class_defs,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let orelse = lower_body(
                &try_stmt.orelse,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?;
            // Entering a `finally` clause always sets `in_finally` to `true`
            // for its own body -- unconditionally, regardless of the
            // incoming value -- since a `return`/`break`/`continue` directly
            // inside it always escapes this `finally` (a nested `finally`
            // inside an outer `finally` is still a violation of its own).
            let finalbody = lower_body(
                &try_stmt.finalbody,
                aliases,
                in_loop,
                in_function,
                true,
                class_name,
                type_param,
                class_defs,
            )?;
            if try_stmt.is_star {
                HirStmt::TryStar {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                }
            } else {
                HirStmt::Try {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                }
            }
        }
        Stmt::Raise(raise_stmt) => {
            let exc = raise_stmt
                .exc
                .as_deref()
                .map(|e| lower_expr(e, in_function, class_name))
                .transpose()?;
            // PEP 409: `raise X from None` suppresses the implicit
            // `__context__` chain. Its only observable effect in CPython is
            // traceback rendering, and pycc neither populates `__context__`
            // nor emits traceback frames yet, so a `None` cause is recorded
            // as "no cause" rather than lowered as an expression -- `HirExpr`
            // has no `None` variant to lower it into, and adding one solely
            // for a value nothing downstream can observe would put an
            // unreachable arm in every exhaustive `HirExpr` match in the
            // workspace. This deliberately collapses `raise X from None` and
            // `raise X` into the same HIR; the distinction has to be
            // reintroduced here when implicit `__context__` chaining lands.
            let cause = match raise_stmt.cause.as_deref() {
                None | Some(Expr::NoneLiteral(_)) => None,
                Some(cause) => Some(lower_expr(cause, in_function, class_name)?),
            };
            HirStmt::Raise { exc, cause }
        }
        other => {
            return Err(unsupported(
                "statement kind not supported yet",
                pycc_ast::stmt_range(other),
            ));
        }
    };
    // PEP 572 (#774): reject a walrus lowered anywhere other than the three
    // placements the issue permits -- an `if`/`while` test (that arm's own
    // `HirExpr::NamedExpr` node is left exactly as `lower_expr` produced
    // it, whatever nesting it carries) or a bare expression statement.
    // Every other statement kind's own *immediate* expression fields are
    // walked with `contains_named_expr`; a nested `Vec<HirStmt>` body
    // (`body`/`orelse`/`handlers`/`finalbody`) is deliberately not
    // re-walked here, because each of its own statements already ran
    // through this exact same check via its own recursive `lower_stmt`
    // call -- re-walking it here would be redundant, and for `Try`'s
    // `handlers: Vec<HirExceptHandler>` (whose only fields are bare
    // exception-type-name strings, an optional `as` binding name, and its
    // own `body`) there is no expression to walk in the first place.
    //
    // The three permitted placements are folded into this same exhaustive
    // match as `false` arms (never a violation there) rather than guarded
    // by a separate outer `if`, so this stays a single match with no
    // structurally-unreachable arm left for the 100%-region coverage gate
    // to demand a test for (an earlier revision guarded the match with
    // `if !matches!(lowered, If | While | ExprStmt)` and gave those three
    // variants an `unreachable!()` arm inside it -- correct, but provably
    // untestable, since the outer guard makes the compiler's own
    // exhaustiveness requirement the only reason that arm exists).
    //
    // `HirStmt::ForList` joins this same never-a-violation group rather than
    // getting its own arm: unlike every other variant matched below,
    // `lowered` can never actually be a `ForList` here in the first place --
    // the `Stmt::For` arm above returns a `ForList` directly (see its own
    // `return Ok(HirStmt::ForList { .. })`) before this match is ever
    // reached, so a standalone `HirStmt::ForList { .. } => false` arm is
    // dead code no test can reach without faking the earlier control flow.
    // Its own reasoning would still hold if it *were* reachable -- `list` is
    // a bare variable name (D-105), not an expression, so there would be
    // nothing here for a walrus to be nested inside -- folding it into this
    // arm keeps that reasoning on record without leaving an unreachable arm
    // of its own for the coverage gate to flag.
    let violates_walrus_placement = match &lowered {
        HirStmt::If { .. }
        | HirStmt::While { .. }
        | HirStmt::ExprStmt(_)
        | HirStmt::ForList { .. } => false,
        HirStmt::Assign { value, .. } => contains_named_expr(value),
        HirStmt::AnnAssign { value, .. } => value.as_ref().is_some_and(contains_named_expr),
        HirStmt::ForRange {
            start, stop, step, ..
        } => contains_named_expr(start) || contains_named_expr(stop) || contains_named_expr(step),
        HirStmt::DictSet { key, value, .. } => {
            contains_named_expr(key) || contains_named_expr(value)
        }
        HirStmt::ListCompAssign { iter, cond, elt, .. } => {
            comp_iter_contains_named_expr(iter)
                || cond.as_deref().is_some_and(contains_named_expr)
                || contains_named_expr(elt)
        }
        HirStmt::DictCompAssign {
            iter,
            cond,
            key,
            value,
            ..
        } => {
            comp_iter_contains_named_expr(iter)
                || cond.as_deref().is_some_and(contains_named_expr)
                || contains_named_expr(key)
                || contains_named_expr(value)
        }
        HirStmt::SetCompAssign { iter, cond, elt, .. } => {
            comp_iter_contains_named_expr(iter)
                || cond.as_deref().is_some_and(contains_named_expr)
                || contains_named_expr(elt)
        }
        HirStmt::Return(value) => value.as_ref().is_some_and(contains_named_expr),
        HirStmt::AttrSet { base, value, .. } => {
            contains_named_expr(base) || contains_named_expr(value)
        }
        HirStmt::Match { subject, .. } => contains_named_expr(subject),
        // `handlers`' own only expression-shaped content is each handler's
        // own `body: Vec<HirStmt>`, already independently checked (see this
        // block's own doc comment above).
        HirStmt::Try { .. } | HirStmt::TryStar { .. } => false,
        HirStmt::Raise { exc, cause } => {
            exc.as_ref().is_some_and(contains_named_expr)
                || cause.as_ref().is_some_and(contains_named_expr)
        }
    };
    if violates_walrus_placement {
        return Err(unsupported(
            "a walrus assignment (`:=`) is only supported in an `if`/`while` \
             condition or as a bare expression statement (#774)",
            pycc_ast::stmt_range(stmt),
        ));
    }
    Ok(lowered)
}

/// PEP 572 (#774): `CompIter`'s own `contains_named_expr` counterpart --
/// `CompIter::Range`'s `start`/`stop`/`step` are ordinary `HirExpr` fields
/// (e.g. `[x for x in range(n := 5)]`); `CompIter::Name` is a bare variable
/// name with nothing to walk. A walrus found here is rejected for the same
/// reason every other comprehension field is: #774's own explicit
/// permitted-scope-cut leaves comprehension-embedded walrus scoping
/// unimplemented, so this is a `core` gap recorded in the
/// conformance-breadth manifest, not a silent one.
fn comp_iter_contains_named_expr(iter: &CompIter) -> bool {
    match iter {
        CompIter::Range { start, stop, step } => {
            contains_named_expr(start) || contains_named_expr(stop) || contains_named_expr(step)
        }
        CompIter::Name(_) => false,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_body(
    body: &[Stmt],
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    in_finally: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[ClassAnnotationInfo],
) -> Result<Vec<HirStmt>, Diagnostic> {
    // #435: `Stmt::Pass` is a no-op — filter it out rather than lowering it
    // to a statement. This allows method bodies like `def __init_subclass__:
    // pass` and `def __set_name__: pass` to compile, which is required for
    // PEP 487/PEP 487 hook recognition. A body consisting solely of `pass`
    // produces an empty `Vec<HirStmt>`, which is a valid no-op body.
    body.iter()
        .filter(|stmt| !matches!(stmt, Stmt::Pass(_)))
        .map(|stmt| {
            lower_stmt(
                stmt,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_elif_else_clauses(
    clauses: &[ElifElseClause],
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    in_finally: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[ClassAnnotationInfo],
) -> Result<Vec<HirStmt>, Diagnostic> {
    let Some((first, rest)) = clauses.split_first() else {
        return Ok(vec![]);
    };
    match &first.test {
        // #790: `elif TYPE_CHECKING:` gets the same constant-fold as a
        // leading `if TYPE_CHECKING:` (see `lower_stmt`'s `Stmt::If` arm's
        // own doc comment) -- the guarded body is dead at runtime either
        // way, and CPython's `elif` is just sugar for a nested `if` inside
        // the enclosing `else`.
        Some(test) if is_type_checking_guard(test) => Ok(vec![HirStmt::If {
            test: HirExpr::BoolLiteral(false),
            body: vec![],
            orelse: lower_elif_else_clauses(
                rest,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?,
        }]),
        Some(test) => Ok(vec![HirStmt::If {
            test: lower_expr(test, in_function, class_name)?,
            body: lower_body(
                &first.body,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?,
            orelse: lower_elif_else_clauses(
                rest,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )?,
        }]),
        None => {
            assert!(
                rest.is_empty(),
                "pycc_hir: an else clause must be the last elif_else_clause"
            );
            lower_body(
                &first.body,
                aliases,
                in_loop,
                in_function,
                in_finally,
                class_name,
                type_param,
                class_defs,
            )
        }
    }
}

/// PEP 634-636 (#381, PR-21): lowers a `Stmt::Match` into `HirStmt::Match`.
/// The subject is lowered once via `lower_expr`; each case's pattern is
/// lowered via `lower_pattern`, its guard via `lower_expr`, and its body via
/// `lower_body` (which filters `Stmt::Pass`).
#[allow(clippy::too_many_arguments)]
fn lower_match(
    match_stmt: &StmtMatch,
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    in_finally: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[ClassAnnotationInfo],
) -> Result<HirStmt, Diagnostic> {
    let subject = lower_expr(&match_stmt.subject, in_function, class_name)?;
    let mut cases = Vec::with_capacity(match_stmt.cases.len());
    for case in &match_stmt.cases {
        let pattern = lower_pattern(&case.pattern, in_function, class_name)?;
        let guard = case
            .guard
            .as_deref()
            .map(|g| lower_expr(g, in_function, class_name))
            .transpose()?;
        let body = lower_body(
            &case.body,
            aliases,
            in_loop,
            in_function,
            in_finally,
            class_name,
            type_param,
            class_defs,
        )?;
        cases.push(HirMatchCase {
            pattern,
            guard,
            body,
        });
    }
    Ok(HirStmt::Match { subject, cases })
}

/// PEP 634-636 (#381, PR-21): lowers a `ruff_python_ast::Pattern` into an
/// `HirPattern`. See `HirPattern`'s own doc comment for the per-variant
/// mapping. Unsupported pattern sub-shapes (e.g. a non-literal in
/// `MatchValue`, a non-`Expr::Name` class in `MatchClass`) produce `C0001`.
fn lower_pattern(
    pattern: &Pattern,
    in_function: bool,
    class_name: Option<&str>,
) -> Result<HirPattern, Diagnostic> {
    match pattern {
        Pattern::MatchValue(value) => {
            let expr = lower_expr(&value.value, in_function, class_name)?;
            match &expr {
                HirExpr::IntLiteral(_)
                | HirExpr::FloatLiteral(_)
                | HirExpr::StringLiteral(_)
                | HirExpr::BoolLiteral(_) => Ok(HirPattern::Literal(expr)),
                _ => Err(unsupported(
                    "only a literal value pattern is supported so far",
                    pycc_ast::expr_range(&value.value),
                )),
            }
        }
        Pattern::MatchSingleton(singleton) => match singleton.value {
            Singleton::True => Ok(HirPattern::Singleton(true)),
            Singleton::False => Ok(HirPattern::Singleton(false)),
            Singleton::None => Ok(HirPattern::NoneSingleton),
        },
        Pattern::MatchSequence(seq) => {
            let has_star = seq
                .patterns
                .iter()
                .any(|p| matches!(p, Pattern::MatchStar(_)));
            if has_star {
                let mut rest: Option<String> = None;
                let mut fixed: Vec<HirPattern> = Vec::new();
                for p in &seq.patterns {
                    if let Pattern::MatchStar(star) = p {
                        rest = star.name.as_ref().map(|n| n.id.to_string());
                    } else {
                        fixed.push(lower_pattern(p, in_function, class_name)?);
                    }
                }
                Ok(HirPattern::SequenceStar(fixed, rest))
            } else {
                let sub_patterns = seq
                    .patterns
                    .iter()
                    .map(|p| lower_pattern(p, in_function, class_name))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(HirPattern::Sequence(sub_patterns))
            }
        }
        Pattern::MatchMapping(mapping) => {
            let mut pairs = Vec::with_capacity(mapping.keys.len());
            for (key, pat) in mapping.keys.iter().zip(mapping.patterns.iter()) {
                let key_expr = lower_expr(key, in_function, class_name)?;
                let val_pat = lower_pattern(pat, in_function, class_name)?;
                pairs.push((key_expr, val_pat));
            }
            let rest = mapping.rest.as_ref().map(|n| n.id.to_string());
            Ok(HirPattern::Mapping(pairs, rest))
        }
        Pattern::MatchClass(class) => {
            let Expr::Name(name) = class.cls.as_ref() else {
                return Err(unsupported(
                    "only a bare-name class pattern is supported so far",
                    pycc_ast::expr_range(&class.cls),
                ));
            };
            let class_name = name.id.to_string();
            let positional = class
                .arguments
                .patterns
                .iter()
                .map(|p| lower_pattern(p, in_function, None))
                .collect::<Result<Vec<_>, _>>()?;
            let keyword = class
                .arguments
                .keywords
                .iter()
                .map(|kw| {
                    lower_pattern(&kw.pattern, in_function, None).map(|p| (kw.attr.to_string(), p))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HirPattern::Class {
                class_name,
                positional,
                keyword,
            })
        }
        Pattern::MatchStar(_) => Err(unsupported(
            "a `*` pattern is only valid inside a sequence pattern",
            0..0,
        )),
        Pattern::MatchAs(as_pat) => match (&as_pat.pattern, &as_pat.name) {
            (None, None) => Ok(HirPattern::Wildcard),
            (None, Some(name)) => Ok(HirPattern::Capture(name.id.to_string())),
            (Some(inner), name) => {
                let inner_pat = lower_pattern(inner, in_function, class_name)?;
                let name = name.as_ref().map(|n| n.id.to_string()).unwrap_or_default();
                Ok(HirPattern::As(Box::new(inner_pat), name))
            }
        },
        Pattern::MatchOr(or_pat) => {
            let sub = or_pat
                .patterns
                .iter()
                .map(|p| lower_pattern(p, in_function, class_name))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HirPattern::Or(sub))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HirItem;

    #[test]
    fn lower_pattern_rejects_bare_match_star() {
        let star = Pattern::MatchStar(pycc_ast::PatternMatchStar {
            node_index: Default::default(),
            range: Default::default(),
            name: None,
        });
        let err = lower_pattern(&star, false, None).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(
            err.message
                .contains("a `*` pattern is only valid inside a sequence pattern")
        );
    }

    #[test]
    fn lower_pattern_as_with_no_name_produces_empty_name() {
        let inner = Pattern::MatchValue(pycc_ast::PatternMatchValue {
            node_index: Default::default(),
            range: Default::default(),
            value: Box::new(Expr::NumberLiteral(pycc_ast::ExprNumberLiteral {
                node_index: Default::default(),
                range: Default::default(),
                value: pycc_ast::Number::Float(1.0),
            })),
        });
        let as_pat = Pattern::MatchAs(pycc_ast::PatternMatchAs {
            node_index: Default::default(),
            range: Default::default(),
            pattern: Some(Box::new(inner)),
            name: None,
        });
        let result = lower_pattern(&as_pat, false, None).unwrap();
        assert_eq!(
            result,
            HirPattern::As(
                Box::new(HirPattern::Literal(HirExpr::FloatLiteral(1.0))),
                String::new(),
            )
        );
    }

    #[test]
    fn lower_match_with_unsupported_body_emits_c0001() {
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case 1:\n        while True:\n            pass\n        else:\n            pass\n    case _:\n        pass\n",
        ).expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_subject_expr_error_propagates() {
        // `{**x}` is a dict-unpacking expression that `lower_expr` rejects
        // with C0001; used as the match subject it propagates through the
        // `?` on the subject expression.
        let module = pycc_parser::parse("match {**x}:\n    case _:\n        pass\n")
            .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_guard_expr_error_propagates() {
        // `{**y}` as a guard expression causes `lower_expr` to fail with
        // C0001, propagating through the `?` on the guard.
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case 1 if {**y}:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_value_pattern_folds_a_negative_literal() {
        // #602: `case -1:` is a value pattern whose expression is `USub`
        // applied to the literal `1`. The fold makes it an ordinary
        // `HirExpr::IntLiteral(-1)`, so it is an accepted literal pattern.
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case -1:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("a negative literal pattern must lower");
        assert!(matches!(
            &hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Match { cases, .. })
                if cases[0].pattern == HirPattern::Literal(HirExpr::IntLiteral(-1))
        ));
    }

    #[test]
    fn lower_match_value_pattern_expr_error_propagates() {
        // A magnitude past `i64`'s range still fails in `lower_expr`, so the
        // `?` on the value pattern's own expression keeps its error path
        // covered after #602 made `case -1:` succeed.
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case -99999999999999999999999:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_mapping_key_folds_a_negative_literal() {
        // #602: a mapping key `-1` folds to `HirExpr::IntLiteral(-1)`.
        let module = pycc_parser::parse(
            "x = {1: 2}\nmatch x:\n    case {-1: v}:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("a negative mapping key must lower");
        assert!(matches!(
            &hir.items[1],
            HirItem::TopLevelStmt(HirStmt::Match { cases, .. })
                if matches!(
                    &cases[0].pattern,
                    HirPattern::Mapping(entries, _)
                        if entries[0].0 == HirExpr::IntLiteral(-1)
                )
        ));
    }

    #[test]
    fn lower_match_mapping_key_expr_error_propagates() {
        // As above, an out-of-range magnitude keeps the mapping key's own
        // `?` error path covered now that `-1` folds successfully.
        let module = pycc_parser::parse(
            "x = {1: 2}\nmatch x:\n    case {-99999999999999999999999: v}:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_sequence_subpattern_error_propagates() {
        let module = pycc_parser::parse(
            "x = [1]\nmatch x:\n    case [foo.bar]:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_sequence_star_subpattern_error_propagates() {
        let module = pycc_parser::parse(
            "x = [1]\nmatch x:\n    case [foo.bar, *rest]:\n        pass\n    case _:\n        pass\n",
        ).expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_mapping_value_pattern_error_propagates() {
        let module = pycc_parser::parse(
            "x = {\"k\": 1}\nmatch x:\n    case {\"k\": foo.bar}:\n        pass\n    case _:\n        pass\n",
        ).expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_class_positional_subpattern_error_propagates() {
        let module = pycc_parser::parse(
            "class P:\n    def __init__(self):\n        pass\nx = P()\nmatch x:\n    case P(foo.bar):\n        pass\n    case _:\n        pass\n",
        ).expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_class_keyword_subpattern_error_propagates() {
        let module = pycc_parser::parse(
            "class P:\n    def __init__(self):\n        pass\nx = P()\nmatch x:\n    case P(a=foo.bar):\n        pass\n    case _:\n        pass\n",
        ).expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_as_pattern_inner_error_propagates() {
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case foo.bar as y:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_or_pattern_subpattern_error_propagates() {
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case foo.bar | 1:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }
}
