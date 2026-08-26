//! PEP 435 enum lowering: enum-member attribute typing and enum-loop
//! unrolling (#379 / PR-19).
//!
//! Extracted verbatim from `crates/pycc_types/src/lib.rs` per AGENTS.md's
//! file-decomposition rule and D-185's per-file tracking issue (#544): this
//! is one cohesion-driven seam of that file, not a rewrite. Every diagnostic
//! message, every check, and every comment is unchanged -- the only edits
//! are the ones the module boundary forces (visibility keywords and `use`
//! lines). [`monomorphize`](crate::monomorphize)'s own module doc comment
//! already named this exact seam when it was extracted, calling out
//! [`enum_member_attr_type`] and the `unroll_enum_loops` family as
//! "the enum-lowering pass, a separate seam with its own boundary" that it
//! deliberately left behind for a later extraction.
//!
//! The seam is cohesive because every item here exists to serve PEP 435
//! enum support end to end: typing an enum member access
//! (`enum_member_attr_type`, called from [`infer_expr_in`](crate::infer_expr_in)),
//! type-checking an enum-iteration loop body in both module and function
//! scope ([`check_enum_loop_body_module`] / [`check_enum_loop_body_function`],
//! called from `check_stmt` and its function-scope counterpart in
//! `lib.rs`, so both stay `pub(crate)` and are re-exported at the crate
//! root), and the
//! post-type-check, post-monomorphization rewrite that expands every
//! `HirStmt::ForList` over an enum class into its unrolled equivalent
//! ([`unroll_enum_loops`], called once from
//! [`check_and_resolve`](crate::check_and_resolve) -- see that function's
//! own doc comment for why the ordering after `monomorphize` matters).
//! `build_enum_member_table` and `unroll_enum_loops_in_stmts` are private
//! helpers of `unroll_enum_loops` alone.
//!
//! Everything else in `lib.rs` that these functions call into --
//! `check_assignment`, `join_loop_body`, the `narrow` module's
//! re-entrant-loop helpers -- stays where it was; this module reaches them
//! as an ordinary descendant of the crate root, exactly like
//! `class::binding` reaches `class`'s own private `check_call_args`.

use crate::narrow;
use crate::{BindingState, Environment};
use pycc_diag::Diagnostic;
use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt, Ty};
use std::collections::HashMap;

use super::{check_assignment, join_loop_body};

/// #379 (PR-19): Return the type of an enum member accessed by name
/// (`Color.RED`), or `None` if `class_def` is not an enum class or `attr`
/// is not one of its members. Extracted from `infer_expr_in` to isolate
/// the enum-specific code paths (see cargo-llvm-cov#276 for the coverage
/// instantiation issue).
pub(crate) fn enum_member_attr_type(
    class_def: &crate::HirClassDef,
    class_name: &str,
    attr: &str,
) -> Option<Ty> {
    if !class_def.enum_members.is_empty()
        && class_def.enum_members.iter().any(|(name, _)| name == attr)
    {
        Some(Ty::Instance(Box::new(class_name.to_string())))
    } else {
        None
    }
}

/// #379 (PR-19): Type-check an enum class iteration loop body. Binds `var`
/// to `Ty::Instance(list)`, checks each body statement, then joins the body
/// environment back. Extracted from `check_stmt` and
/// `check_stmt_in_function` to isolate the enum-specific code paths (see
/// cargo-llvm-cov#276 for the coverage instantiation issue). Two variants
/// exist (module-scope and function-scope) to avoid generic closure
/// monomorphization producing separate coverage records per call site.
pub(crate) fn check_enum_loop_body_module(
    env: &mut Environment,
    var: &str,
    list: &str,
    body: &[HirStmt],
) -> Result<(), Diagnostic> {
    let var_ty = Ty::Instance(Box::new(list.to_string()));
    let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
    check_assignment(env, var, var_ty)?;
    let mut body_env = env.clone();
    // Issue #769 follow-up (D-068 re-review round 3): an enum loop with
    // more than one member re-runs `body` once per member, same
    // re-entrant shape as any other loop -- see `narrow::apply_kill_prescan`.
    narrow::apply_kill_prescan(&mut body_env, body);
    narrow::check_stmt_sequence(&mut body_env, body)?;
    join_loop_body(env, &body_env);
    if !was_definite && let Some(ty) = env.lookup_any(var) {
        env.bind_maybe(var.to_string(), ty);
    }
    Ok(())
}

/// #379 (PR-19): Function-scope variant of `check_enum_loop_body_module`.
/// Checks each body statement via `check_stmt_in_function` with the
/// enclosing function's `local_names` and `return_ty`.
pub(crate) fn check_enum_loop_body_function(
    env: &mut Environment,
    var: &str,
    list: &str,
    body: &[HirStmt],
    local_names: &[&str],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    let var_ty = Ty::Instance(Box::new(list.to_string()));
    let was_definite = matches!(env.binding_state(var), Some(BindingState::Definitely(_)));
    check_assignment(env, var, var_ty)?;
    let mut body_env = env.clone();
    // Issue #769 follow-up (D-068 re-review round 3): see
    // `check_enum_loop_body_module`'s identical comment.
    narrow::apply_kill_prescan(&mut body_env, body);
    narrow::check_stmt_sequence_in_function(&mut body_env, local_names, body, return_ty.clone())?;
    join_loop_body(env, &body_env);
    if !was_definite && let Some(ty) = env.lookup_any(var) {
        env.bind_maybe(var.to_string(), ty);
    }
    Ok(())
}

/// #379 (PR-19): Build a lookup table mapping enum class name to its
/// member names (in source order). Extracted from `unroll_enum_loops` to
/// isolate the enum-specific code paths (see cargo-llvm-cov#276 for the
/// coverage instantiation issue).
fn build_enum_member_table(
    class_defs: &[(String, crate::HirClassDef)],
) -> HashMap<&str, Vec<&String>> {
    class_defs
        .iter()
        .filter(|(_, cd)| !cd.enum_members.is_empty())
        .map(|(name, cd)| {
            (
                name.as_str(),
                cd.enum_members.iter().map(|(mn, _)| mn).collect(),
            )
        })
        .collect()
}

/// The rewrite walks both top-level items and function bodies (a
/// `ForList`-over-enum can appear inside a function). Inside a function
/// body, the enclosing `Vec<HirStmt>` is spliced in place: the unrolled
/// statements replace the original `ForList` statement at its position.
///
/// Limitations (matching the plan's risk-1): no `break`/`continue`/`else`
/// in an enum-loop body (already unimplemented for all v0.3 loops, so no
/// real loss); code size is linear in member-count × body-size (acceptable
/// for v0.3 fixtures). A module with no enum classes is returned unchanged.
pub(crate) fn unroll_enum_loops(mut hir: HirModule) -> Result<HirModule, Diagnostic> {
    // Fast path: if no class is an enum class, there is nothing to unroll.
    let has_enum = hir
        .class_defs
        .iter()
        .any(|(_, cd)| !cd.enum_members.is_empty());
    if !has_enum {
        return Ok(hir);
    }
    // Build a lookup table: enum class name -> member names (in source order).
    let enum_members = build_enum_member_table(&hir.class_defs);
    // Walk top-level items and function bodies, splicing unrolled statements.
    let mut new_items: Vec<HirItem> = Vec::with_capacity(hir.items.len());
    for item in hir.items.drain(..) {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                let mut unrolled =
                    unroll_enum_loops_in_stmts(std::slice::from_ref(&stmt), &enum_members);
                for s in unrolled.drain(..) {
                    new_items.push(HirItem::TopLevelStmt(s));
                }
            }
            HirItem::Function {
                name,
                params,
                return_ty,
                body,
            } => {
                let body = unroll_enum_loops_in_stmts(&body, &enum_members);
                new_items.push(HirItem::Function {
                    name,
                    params,
                    return_ty,
                    body,
                });
            }
        }
    }
    hir.items = new_items;
    Ok(hir)
}

/// Helper for `unroll_enum_loops`: walks a `Vec<HirStmt>`, splicing any
/// `ForList`-over-enum into its unrolled equivalent. Recurses into nested
/// statement bodies (if/while/for bodies) so an enum loop inside a nested
/// block is also unrolled. Returns a new `Vec<HirStmt>` with all enum loops
/// expanded.
fn unroll_enum_loops_in_stmts(
    stmts: &[HirStmt],
    enum_members: &HashMap<&str, Vec<&String>>,
) -> Vec<HirStmt> {
    let mut result: Vec<HirStmt> = Vec::new();
    for stmt in stmts {
        match stmt {
            HirStmt::ForList { var, list, body } => {
                // Check if `list` is an enum class name.
                if let Some(members) = enum_members.get(list.as_str()) {
                    // Unroll: for each member, emit `var = <enum>.<member>`
                    // then the body (cloned).
                    let body = body.clone();
                    for member_name in members {
                        result.push(HirStmt::Assign {
                            target: var.clone(),
                            value: HirExpr::AttrGet {
                                base: Box::new(HirExpr::Name(list.clone())),
                                attr: (*member_name).clone(),
                            },
                        });
                        result.extend(body.iter().cloned());
                    }
                } else {
                    // Not an enum loop -- keep as-is, but recurse into the
                    // body in case it contains a nested enum loop.
                    result.push(HirStmt::ForList {
                        var: var.clone(),
                        list: list.clone(),
                        body: unroll_enum_loops_in_stmts(body, enum_members),
                    });
                }
            }
            // Recurse into nested statement bodies.
            HirStmt::If { test, body, orelse } => {
                result.push(HirStmt::If {
                    test: test.clone(),
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                    orelse: unroll_enum_loops_in_stmts(orelse, enum_members),
                });
            }
            HirStmt::While { test, body } => {
                result.push(HirStmt::While {
                    test: test.clone(),
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                });
            }
            HirStmt::ForRange {
                var,
                start,
                stop,
                step,
                body,
            } => {
                result.push(HirStmt::ForRange {
                    var: var.clone(),
                    start: start.clone(),
                    stop: stop.clone(),
                    step: step.clone(),
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                });
            }
            // Other statement kinds don't contain nested ForList loops.
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                result.push(HirStmt::Try {
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                    handlers: handlers
                        .iter()
                        .map(|h| pycc_hir::HirExceptHandler {
                            exc_type: h.exc_type.clone(),
                            name: h.name.clone(),
                            body: unroll_enum_loops_in_stmts(&h.body, enum_members),
                        })
                        .collect(),
                    orelse: unroll_enum_loops_in_stmts(orelse, enum_members),
                    finalbody: unroll_enum_loops_in_stmts(finalbody, enum_members),
                });
            }
            // `except*` (PEP 654, #542) shares `Try`'s recursion shape --
            // an enum `for` loop nested in a `try*` body, an `except*`
            // handler, `else`, or `finally` must be unrolled exactly like
            // its `Try` counterpart above, or a `ForList`-over-enum left
            // inside a `try*` would reach MIR lowering unexpanded.
            HirStmt::TryStar {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                result.push(HirStmt::TryStar {
                    body: unroll_enum_loops_in_stmts(body, enum_members),
                    handlers: handlers
                        .iter()
                        .map(|h| pycc_hir::HirExceptHandler {
                            exc_type: h.exc_type.clone(),
                            name: h.name.clone(),
                            body: unroll_enum_loops_in_stmts(&h.body, enum_members),
                        })
                        .collect(),
                    orelse: unroll_enum_loops_in_stmts(orelse, enum_members),
                    finalbody: unroll_enum_loops_in_stmts(finalbody, enum_members),
                });
            }
            other => result.push(other.clone()),
        }
    }
    result
}
