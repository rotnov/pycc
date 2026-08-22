//! Type checking for builtin exceptions (#382).

use super::{
    Environment, HirExpr, HirStmt, Ty, check_stmt, check_stmt_in_function, infer_expr_in,
    join_if_branches, join_loop_body,
};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::HirExceptHandler;

pub(super) fn check_try_stmt(
    env: &mut Environment,
    local_names: &[&str],
    body: &[HirStmt],
    handlers: &[HirExceptHandler],
    orelse: &[HirStmt],
    finalbody: &[HirStmt],
    return_ty: Option<&Ty>,
) -> Result<(), Diagnostic> {
    let mut body_env = env.clone();
    for stmt in body {
        check_stmt_shared(&mut body_env, local_names, stmt, return_ty)?;
    }

    let mut handler_envs = Vec::with_capacity(handlers.len());
    for handler in handlers {
        let mut handler_env = env.clone();
        handler_env.in_except_handler = true;
        if let Some(exc_type) = &handler.exc_type {
            if !is_unshadowed_builtin_exception(&body_env, local_names, exc_type) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "`{exc_type}` is not a recognized exception class — only builtin exception classes are supported in `except` handlers"
                    ),
                    Span::new(0, 0),
                ));
            }
            if let Some(name) = &handler.name {
                handler_env.bind(name.clone(), Ty::Instance(Box::new(exc_type.clone())));
            }
        }
        for stmt in &handler.body {
            check_stmt_shared(&mut handler_env, local_names, stmt, return_ty)?;
        }
        handler_envs.push(handler_env);
    }

    // `else` runs only after the try body completes successfully, so it sees
    // bindings established by that successful path. Starting from the
    // pre-try environment silently rejected valid reads such as
    // `try: x = 1; ...; else: print(x)`.
    let mut else_env = body_env.clone();
    for stmt in orelse {
        check_stmt_shared(&mut else_env, local_names, stmt, return_ty)?;
    }

    let mut joined = env.clone();
    join_loop_body(&mut joined, &body_env);
    for handler_env in &handler_envs {
        let previous = joined.clone();
        join_if_branches(&mut joined, &previous, handler_env)?;
    }
    let previous = joined.clone();
    let _ = join_if_branches(&mut joined, &previous, &else_env);
    *env = joined;
    for stmt in finalbody {
        check_stmt_shared(env, local_names, stmt, return_ty)?;
    }
    Ok(())
}

pub(super) fn check_raise_stmt(
    env: &Environment,
    local_names: &[&str],
    exc: &Option<HirExpr>,
    cause: &Option<HirExpr>,
) -> Result<(), Diagnostic> {
    if let Some(exc) = exc {
        check_raise_operand(env, local_names, exc, "can only raise exception instances")?;
    } else if !env.in_except_handler {
        return Err(Diagnostic::error(
            "T0021",
            "bare `raise` is only valid inside an except handler",
            Span::new(0, 0),
        ));
    }
    if let Some(cause) = cause {
        check_raise_operand(
            env,
            local_names,
            cause,
            "cause must be an exception instance",
        )?;
    }
    Ok(())
}

fn check_raise_operand(
    env: &Environment,
    local_names: &[&str],
    expr: &HirExpr,
    error_prefix: &str,
) -> Result<(), Diagnostic> {
    if let HirExpr::Call { callee, args } = expr
        && is_unshadowed_builtin_exception(env, local_names, callee)
    {
        if args.len() != 1 {
            return Err(Diagnostic::error(
                "T0021",
                format!(
                    "`{callee}` expects exactly 1 argument (the message string), got {}",
                    args.len()
                ),
                Span::new(0, 0),
            ));
        }
        let argument_type = infer_expr_in(env, local_names, &args[0])?;
        if argument_type != Ty::Str {
            return Err(Diagnostic::error(
                "T0021",
                format!(
                    "`{callee}` expects a `str` message argument, got `{}`",
                    argument_type.name()
                ),
                Span::new(0, 0),
            ));
        }
        return Ok(());
    }

    let ty = infer_expr_in(env, local_names, expr)?;
    if matches!(&ty, Ty::Instance(class_name) if pycc_hir::is_builtin_exception_class(class_name) && !is_user_defined_class(env, class_name))
    {
        return Ok(());
    }
    Err(Diagnostic::error(
        "T0021",
        format!("{error_prefix}, got `{}`", ty.name()),
        Span::new(0, 0),
    ))
}

pub(super) fn is_unshadowed_builtin_exception(
    env: &Environment,
    local_names: &[&str],
    name: &str,
) -> bool {
    pycc_hir::is_builtin_exception_class(name)
        & env.lookup_any(name).is_none()
        & !local_names.contains(&name)
        & !env.functions.contains_key(name)
        & !is_user_defined_class(env, name)
}

/// Whether `name` is registered as a *user-authored* class (Part 1 of
/// #541). Since HIR lowering now seeds a `HirClassDef` for each of the
/// seven builtin exception names, mere presence in `Environment::classes`
/// no longer means the user shadowed the name -- only a non-synthetic
/// entry does. Without this distinction every `except ValueError:` and
/// `raise ValueError("x")` in the language would start being rejected.
fn is_user_defined_class(env: &Environment, name: &str) -> bool {
    env.classes.contains_key(name) && !env.is_synthetic_class(name)
}

fn check_stmt_shared(
    env: &mut Environment,
    local_names: &[&str],
    stmt: &HirStmt,
    return_ty: Option<&Ty>,
) -> Result<(), Diagnostic> {
    if let Some(return_ty) = return_ty {
        check_stmt_in_function(env, local_names, stmt, return_ty.clone())
    } else {
        check_stmt(env, stmt)
    }
}

#[cfg(test)]
mod synthetic_class_tests;
