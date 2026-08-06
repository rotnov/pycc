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

use crate::expr::{
    lower_dict_comp_assign, lower_expr, lower_list_comp_assign, lower_range_call,
    lower_set_comp_assign,
};
use crate::{HirStmt, Ty, annotation_to_ty, context_invalid, unsupported};
use pycc_ast::{ElifElseClause, Expr, Stmt};
use pycc_diag::Diagnostic;

pub(crate) fn lower_stmt(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
) -> Result<HirStmt, Diagnostic> {
    let lowered = match stmt {
        Stmt::Expr(expr_stmt) => HirStmt::ExprStmt(lower_expr(&expr_stmt.value, in_function)?),
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
                    Expr::ListComp(comp) => lower_list_comp_assign(name.id.as_str(), comp)?,
                    Expr::SetComp(comp) => lower_set_comp_assign(name.id.as_str(), comp)?,
                    Expr::DictComp(comp) => lower_dict_comp_assign(name.id.as_str(), comp)?,
                    _ => HirStmt::Assign {
                        target: name.id.as_str().to_string(),
                        value: lower_expr(&assign.value, in_function)?,
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
                // known, relocating (not removing) the invariant this file's
                // own `subscript_assignment_target_is_unsupported` test used
                // to enforce at the lowering level.
                Expr::Subscript(sub) => {
                    let Expr::Name(base_name) = sub.value.as_ref() else {
                        return Err(unsupported(
                            "only assigning to a bare-name subscript target (`name[key] = value`) is supported so far",
                            pycc_ast::expr_range(target),
                        ));
                    };
                    HirStmt::DictSet {
                        dict: base_name.id.as_str().to_string(),
                        key: lower_expr(&sub.slice, in_function)?,
                        value: lower_expr(&assign.value, in_function)?,
                    }
                }
                // `base.attr = value` (D-154, Part 1 of #375): structurally
                // recognized for any base expression, exactly like
                // `HirExpr::AttrGet`'s own `base` (no type information is
                // available at this lowering step to narrow it to only
                // `self` or only an instance-typed receiver -- `pycc_types`
                // rejects a non-instance base or an undeclared attribute
                // name). This supersedes the older, narrower invariant
                // `assigning_to_a_non_name_target_is_unsupported` used to
                // lock in ("only assigning to a bare name is supported so
                // far") -- that test now documents a genuinely different,
                // still-unsupported target shape instead (see its own
                // updated body).
                Expr::Attribute(attr) => HirStmt::AttrSet {
                    base: lower_expr(&attr.value, in_function)?,
                    attr: attr.attr.to_string(),
                    value: lower_expr(&assign.value, in_function)?,
                },
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
            let annotation = annotation_to_ty(&ann.annotation, None, aliases)?;
            let value = ann
                .value
                .as_deref()
                .map(|e| lower_expr(e, in_function))
                .transpose()?;
            HirStmt::AnnAssign {
                target: name.id.as_str().to_string(),
                annotation,
                value,
            }
        }
        Stmt::If(if_stmt) => HirStmt::If {
            test: lower_expr(&if_stmt.test, in_function)?,
            body: lower_body(&if_stmt.body, aliases, in_loop, in_function)?,
            orelse: lower_elif_else_clauses(
                &if_stmt.elif_else_clauses,
                aliases,
                in_loop,
                in_function,
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
                test: lower_expr(&while_stmt.test, in_function)?,
                body: lower_body(&while_stmt.body, aliases, true, in_function)?,
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
                    body: lower_body(&for_stmt.body, aliases, true, in_function)?,
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
            let (start, stop, step) = lower_range_call(call, in_function)?;
            HirStmt::ForRange {
                var: var.id.to_string(),
                start,
                stop,
                step,
                body: lower_body(&for_stmt.body, aliases, true, in_function)?,
            }
        }
        Stmt::Return(ret) => HirStmt::Return(
            ret.value
                .as_deref()
                .map(|e| lower_expr(e, in_function))
                .transpose()?,
        ),
        Stmt::Break(_) => {
            return Err(if in_loop {
                // A real enclosing loop -- valid Python, break/continue
                // control-flow codegen is just not implemented yet.
                unsupported("statement kind not supported yet", pycc_ast::stmt_range(stmt))
            } else {
                // No enclosing loop -- CPython rejects this as a
                // `SyntaxError`, not "valid but unimplemented" (D-148).
                context_invalid("'break' outside loop", pycc_ast::stmt_range(stmt))
            });
        }
        Stmt::Continue(_) => {
            return Err(if in_loop {
                unsupported("statement kind not supported yet", pycc_ast::stmt_range(stmt))
            } else {
                context_invalid(
                    "'continue' not properly in loop",
                    pycc_ast::stmt_range(stmt),
                )
            });
        }
        other => {
            return Err(unsupported(
                "statement kind not supported yet",
                pycc_ast::stmt_range(other),
            ));
        }
    };
    Ok(lowered)
}

pub(crate) fn lower_body(
    body: &[Stmt],
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
) -> Result<Vec<HirStmt>, Diagnostic> {
    body.iter()
        .map(|stmt| lower_stmt(stmt, aliases, in_loop, in_function))
        .collect()
}

pub(crate) fn lower_elif_else_clauses(
    clauses: &[ElifElseClause],
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
) -> Result<Vec<HirStmt>, Diagnostic> {
    let Some((first, rest)) = clauses.split_first() else {
        return Ok(vec![]);
    };
    match &first.test {
        Some(test) => Ok(vec![HirStmt::If {
            test: lower_expr(test, in_function)?,
            body: lower_body(&first.body, aliases, in_loop, in_function)?,
            orelse: lower_elif_else_clauses(rest, aliases, in_loop, in_function)?,
        }]),
        None => {
            assert!(
                rest.is_empty(),
                "pycc_hir: an else clause must be the last elif_else_clause"
            );
            lower_body(&first.body, aliases, in_loop, in_function)
        }
    }
}
