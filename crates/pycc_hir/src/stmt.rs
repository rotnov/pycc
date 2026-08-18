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

mod exception;

use crate::expr::{
    is_zero_arg_super_call, lower_dict_comp_assign, lower_expr, lower_list_comp_assign,
    lower_range_call, lower_set_comp_assign,
};
use crate::{
    HirExpr, HirMatchCase, HirPattern, HirStmt, Ty, annotation_to_ty, context_invalid, unsupported,
};
use exception::lower_except_handler;
use pycc_ast::{ElifElseClause, Expr, Pattern, Singleton, Stmt, StmtMatch};
use pycc_diag::Diagnostic;

pub(crate) fn lower_stmt(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[(String, bool)],
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
                // (`crates/pycc_hir/src/lib.rs`) used to enforce at the
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
                // `crates/pycc_hir/src/lib.rs`.
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
        Stmt::If(if_stmt) => HirStmt::If {
            test: lower_expr(&if_stmt.test, in_function, class_name)?,
            body: lower_body(
                &if_stmt.body,
                aliases,
                in_loop,
                in_function,
                class_name,
                type_param,
                class_defs,
            )?,
            orelse: lower_elif_else_clauses(
                &if_stmt.elif_else_clauses,
                aliases,
                in_loop,
                in_function,
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
                    class_name,
                    type_param,
                    class_defs,
                )?,
            }
        }
        Stmt::Return(ret) => HirStmt::Return(
            ret.value
                .as_deref()
                .map(|e| lower_expr(e, in_function, class_name))
                .transpose()?,
        ),
        Stmt::Break(_) => {
            return Err(if in_loop {
                // A real enclosing loop -- valid Python, break/continue
                // control-flow codegen is just not implemented yet.
                unsupported(
                    "statement kind not supported yet",
                    pycc_ast::stmt_range(stmt),
                )
            } else {
                // No enclosing loop -- CPython rejects this as a
                // `SyntaxError`, not "valid but unimplemented" (D-148).
                context_invalid("'break' outside loop", pycc_ast::stmt_range(stmt))
            });
        }
        Stmt::Continue(_) => {
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
            class_name,
            type_param,
            class_defs,
        )?,
        Stmt::Try(try_stmt) => {
            if try_stmt.is_star {
                return Err(unsupported(
                    "except* (exception groups) is not supported yet",
                    try_stmt.range,
                ));
            }
            let body = lower_body(
                &try_stmt.body,
                aliases,
                in_loop,
                in_function,
                class_name,
                type_param,
                class_defs,
            )?;
            let handlers = try_stmt
                .handlers
                .iter()
                .map(|h| {
                    lower_except_handler(
                        h,
                        aliases,
                        in_loop,
                        in_function,
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
                class_name,
                type_param,
                class_defs,
            )?;
            let finalbody = lower_body(
                &try_stmt.finalbody,
                aliases,
                in_loop,
                in_function,
                class_name,
                type_param,
                class_defs,
            )?;
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }
        }
        Stmt::Raise(raise_stmt) => {
            let exc = raise_stmt
                .exc
                .as_deref()
                .map(|e| lower_expr(e, in_function, class_name))
                .transpose()?;
            let cause = raise_stmt
                .cause
                .as_deref()
                .map(|e| lower_expr(e, in_function, class_name))
                .transpose()?;
            HirStmt::Raise { exc, cause }
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
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[(String, bool)],
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
                class_name,
                type_param,
                class_defs,
            )
        })
        .collect()
}

pub(crate) fn lower_elif_else_clauses(
    clauses: &[ElifElseClause],
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[(String, bool)],
) -> Result<Vec<HirStmt>, Diagnostic> {
    let Some((first, rest)) = clauses.split_first() else {
        return Ok(vec![]);
    };
    match &first.test {
        Some(test) => Ok(vec![HirStmt::If {
            test: lower_expr(test, in_function, class_name)?,
            body: lower_body(
                &first.body,
                aliases,
                in_loop,
                in_function,
                class_name,
                type_param,
                class_defs,
            )?,
            orelse: lower_elif_else_clauses(
                rest,
                aliases,
                in_loop,
                in_function,
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
fn lower_match(
    match_stmt: &StmtMatch,
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[(String, bool)],
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
    fn lower_match_value_pattern_expr_error_propagates() {
        // `case -1:` is a value pattern with a unary minus expression;
        // `lower_expr` does not handle unary operators and rejects with C0001.
        let module = pycc_parser::parse(
            "x = 1\nmatch x:\n    case -1:\n        pass\n    case _:\n        pass\n",
        )
        .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
    }

    #[test]
    fn lower_match_mapping_key_expr_error_propagates() {
        // A mapping key `-1` is a unary minus expression that `lower_expr`
        // rejects with C0001, propagating through the `?` on the key.
        let module = pycc_parser::parse(
            "x = {1: 2}\nmatch x:\n    case {-1: v}:\n        pass\n    case _:\n        pass\n",
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
