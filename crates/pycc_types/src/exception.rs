//! Type checking for builtin exceptions (#382).

use super::{
    Environment, HirExpr, HirStmt, Ty, check_stmt, check_stmt_in_function, infer_expr_in,
    join_if_branches, join_loop_body,
};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{EXCEPTION_INIT_MANGLED_NAME, HirClassDef, HirExceptHandler};

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
            let builtin = is_unshadowed_builtin_exception(&body_env, local_names, exc_type);
            if !builtin {
                // Part 2 of #541 (D-190): a user-declared class whose MRO
                // reaches a builtin exception is catchable too.
                let Some(def) = user_exception_class(&body_env, local_names, exc_type) else {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`{exc_type}` is not a recognized exception class — only builtin exception classes and classes derived from them are supported in `except` handlers"
                        ),
                        Span::new(0, 0),
                    ));
                };
                reject_own_constructor(&body_env, def)?;
                if handler.name.is_some() {
                    // Deliberately *not* supported in Part 2, and not merely
                    // for want of the feature: binding the caught value would
                    // give it a `Ty::Instance(exc_type)`, and every consumer
                    // of that type reads an instance as a `PyInstanceObj`.
                    // The value the runtime actually holds is a
                    // `PyExceptionObj` — a different layout entirely — so the
                    // binding would be a type confusion, not a missing
                    // capability. Part 3 of #541 (#703) materializes a real
                    // instance; until then this must stay rejected.
                    return Err(Diagnostic::error(
                        "C0001",
                        format!(
                            "binding a caught `{exc_type}` with `as` is not supported \
                             yet; pycc does not materialize an exception instance \
                             for a user-defined exception class (Part 3 of #541)"
                        ),
                        Span::new(0, 0),
                    ));
                }
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

    // Part 2 of #541 (D-190): `raise MyError("boom")` for a user-declared class
    // whose MRO reaches a builtin exception.
    //
    // This acceptance is keyed *structurally*, on the `HirExpr::Call` shape
    // matched above, and never on the inferred type. `e = MyError("x"); raise e`
    // infers the identical `Ty::Instance("MyError")` as `raise MyError("boom")`,
    // so widening the `Ty::Instance` predicate below would silently admit the
    // bound-value form -- and MIR lowers that form to
    // `MirExceptionValue::Existing`, which codegen hands to
    // `pycc_rt_exception_raise` as a `*mut PyExceptionObj` while the value is
    // really a `*mut PyInstanceObj`. That is memory corruption, not a
    // diagnostic. `raise <bound value>` therefore stays `T0021`.
    if let HirExpr::Call { callee, .. } = expr
        && let Some(def) = user_exception_class(env, local_names, callee)
    {
        // Inference runs first so a malformed constructor call reports its own
        // argument diagnostic (`T0021` from `check_call_args`) rather than the
        // generic "can only raise exception instances" message.
        infer_expr_in(env, local_names, expr)?;
        reject_own_constructor(env, def)?;
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

/// The [`HirClassDef`] of a user-declared, unshadowed exception class -- one
/// HIR lowering assigned a runtime type tag to, which it does exactly when the
/// class's MRO reaches one of the seeded builtin exception classes (Part 2 of
/// #541, D-190). `None` for anything else, including a class that never
/// touches the exception hierarchy and a name rebound by a local, a parameter,
/// or a function.
fn user_exception_class<'e>(
    env: &'e Environment,
    local_names: &[&str],
    name: &str,
) -> Option<&'e HirClassDef> {
    if env.lookup_any(name).is_some()
        || local_names.contains(&name)
        || env.functions.contains_key(name)
        || !is_user_defined_class(env, name)
    {
        return None;
    }
    env.classes
        .get(name)
        .filter(|def| def.exception_type_tag.is_some())
}

/// Rejects a raisable user exception class that declares (or inherits from a
/// non-synthetic ancestor) its own `__init__`.
///
/// Part 2 of #541 raises and catches by type tag alone: the message string is
/// the only payload `PyExceptionObj` carries, and it is filled from the single
/// argument the synthetic `Exception.__init__` accepts. A class with its own
/// constructor has state that never reaches the raised object, so accepting it
/// would silently drop the user's fields. Part 3 of #541 (#703) materializes a
/// real instance; until then this is a capability gap, reported as `C0001`.
fn reject_own_constructor(env: &Environment, def: &HirClassDef) -> Result<(), Diagnostic> {
    let init = def.mro.iter().find_map(|ancestor| {
        env.classes.get(ancestor.as_str()).and_then(|ancestor_def| {
            ancestor_def
                .methods
                .iter()
                .find(|(method, _)| method == "__init__")
                .map(|(_, mangled)| mangled.as_str())
        })
    });
    if init == Some(EXCEPTION_INIT_MANGLED_NAME) {
        return Ok(());
    }
    Err(Diagnostic::error(
        "C0001",
        format!(
            "exception class `{}` defines its own `__init__`; pycc supports only exception \
             classes that inherit `Exception`'s single-message constructor (Part 3 of #541)",
            def.name
        ),
        Span::new(0, 0),
    ))
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

#[cfg(test)]
mod user_class_tests;
