//! Type checking for builtin exceptions (#382).

use super::{Environment, HirExpr, HirStmt, Ty, infer_expr_in, join_if_branches, join_loop_body};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{
    EXCEPTION_INIT_MANGLED_NAME, HirClassDef, HirExceptHandler, except_handler_binding_type_name,
};

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
    check_stmt_sequence_shared(&mut body_env, local_names, body, return_ty)?;

    let mut handler_envs = Vec::with_capacity(handlers.len());
    for handler in handlers {
        let mut handler_env = env.clone();
        // Issue #769 follow-up (D-068 re-review of #780, third round): a
        // handler runs only after *some* prefix of `body` already
        // executed -- unlike `else_env` (which starts from `body_env`'s
        // full post-execution state because it only runs after `body`
        // completes normally), a handler can be entered after any
        // partial execution, so no single point-in-time snapshot of
        // `body`'s narrowing state is safe to hand it. Conservatively
        // prescan-drop every name `body` kills *anywhere* from
        // `handler_env`'s narrowing overlay before checking the handler,
        // rather than starting from the pre-try `env` clone unmodified
        // (which is what let a read of a name `body` reassigned to
        // `None` right before raising still see it narrowed). See
        // `narrow::apply_kill_prescan`'s doc comment in
        // `crates/pycc_types/src/narrow.rs` for the full rationale.
        super::narrow::apply_kill_prescan(&mut handler_env, body);
        handler_env.in_except_handler = true;
        if let Some(exc_types) = &handler.exc_type {
            // PEP 758 (#740): a handler may name more than one exception
            // type (`except A, B:` / `except (A, B):`). Every named type is
            // validated independently, in source order, failing on the
            // *first* invalid name -- no diagnostic in this codebase
            // batches multiple errors together.
            for exc_type in exc_types {
                let builtin = is_unshadowed_builtin_exception(&body_env, local_names, exc_type);
                if !builtin {
                    // Part 2 of #541 (D-189): a user-declared class whose MRO
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
                        // instance; until then this must stay rejected. This
                        // applies per-name: any user-defined class among a
                        // multi-type handler's names with an `as` binding is
                        // rejected, naming the specific offending class.
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
            }
            if let Some(name) = &handler.name {
                let binding_type = except_handler_binding_type_name(exc_types);
                handler_env.bind(name.clone(), Ty::Instance(Box::new(binding_type)));
                // D-068 re-review of #780 (fourth round): `bind` only
                // overwrites `bindings`, never `narrowed` (see `bind`'s own
                // doc comment in `env.rs`), so a name previously narrowed by
                // an enclosing `if <name> is not None:` stayed narrowed
                // here even though it is now bound to the caught exception
                // instance -- a later read inside the handler body would be
                // type-checked as the narrowed `int`/whatever, not
                // `Instance(binding_type)`, via `HirExpr::Name`'s
                // `narrowed_ty` preference (`expr.rs`). Kill it explicitly,
                // mirroring `check_assignment`'s own unconditional
                // `env.narrowed.remove(target)` for every other reassignment
                // kind -- this handler binding is not routed through
                // `check_assignment` itself (it must skip that function's
                // previous-type compatibility check, which would wrongly
                // reject `except X as name:` reusing a name whose earlier
                // type is incompatible with the exception instance type).
                handler_env.narrowed.remove(name);
            }
        }
        check_stmt_sequence_shared(&mut handler_env, local_names, &handler.body, return_ty)?;
        handler_envs.push(handler_env);
    }

    // `else` runs only after the try body completes successfully, so it sees
    // bindings established by that successful path. Starting from the
    // pre-try environment silently rejected valid reads such as
    // `try: x = 1; ...; else: print(x)`.
    let mut else_env = body_env.clone();
    check_stmt_sequence_shared(&mut else_env, local_names, orelse, return_ty)?;

    let mut joined = env.clone();
    join_loop_body(&mut joined, &body_env);
    for handler_env in &handler_envs {
        let previous = joined.clone();
        join_if_branches(&mut joined, &previous, handler_env)?;
    }
    let previous = joined.clone();
    let _ = join_if_branches(&mut joined, &previous, &else_env);
    *env = joined;
    check_stmt_sequence_shared(env, local_names, finalbody, return_ty)?;
    Ok(())
}

/// `try: ... except* T: ...` (PEP 654, Part 3 of #382, #542).
///
/// Structurally mirrors [`check_try_stmt`] -- same body/handler/orelse/
/// finalbody scoping and environment-joining shape -- but differs in what an
/// `as` binding resolves to. Plain `except E as e:` binds `e` to `E` itself;
/// `except* E as e:` always binds `e` to an `ExceptionGroup` containing the
/// matched subgroup, never to `E`, because `except*` always operates on
/// groups (PEP 654 section "Runtime Semantics"). pycc has no first-class
/// generic-group type distinct from the flat `ExceptionGroup` name, so the
/// binding is `Ty::Instance("ExceptionGroup")` unconditionally.
pub(super) fn check_try_star_stmt(
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
        // A bare `except*:` is rejected by ruff's own parser as a syntax
        // error (PEP 654 requires every `except*` clause to name a type),
        // so `handler.exc_type` is always `Some` by the time an
        // `HirStmt::TryStar` reaches type-checking -- see
        // `pycc_hir::stmt`'s own comment on the matching lowering site for
        // why no defensive `None` re-check belongs here either.
        if let Some(exc_types) = &handler.exc_type {
            for exc_type in exc_types {
                let builtin = is_unshadowed_builtin_exception(&body_env, local_names, exc_type);
                if !builtin {
                    let Some(def) = user_exception_class(&body_env, local_names, exc_type) else {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "`{exc_type}` is not a recognized exception class — only builtin exception classes and classes derived from them are supported in `except*` handlers"
                            ),
                            Span::new(0, 0),
                        ));
                    };
                    reject_own_constructor(&body_env, def)?;
                }
            }
        }
        if let Some(name) = &handler.name {
            handler_env.bind(
                name.clone(),
                Ty::Instance(Box::new("ExceptionGroup".to_string())),
            );
        }
        for stmt in &handler.body {
            check_stmt_shared(&mut handler_env, local_names, stmt, return_ty)?;
        }
        handler_envs.push(handler_env);
    }

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

/// Part 3 of #382 (#542, PEP 654, D-202): the narrow, literal-list-only
/// relaxation for `ExceptionGroup(msg, [exc1, exc2, ...])` /
/// `BaseExceptionGroup(msg, [...])` construction inside a `raise` operand.
///
/// D-105/T0021 makes `list[T]` annotations unsupported in general, so this
/// deliberately does not accept an arbitrary `list[BaseException]`-typed
/// expression as the second argument -- only a literal `[...]` written
/// directly at the call site, each element of which is validated by
/// [`check_exception_group_member_operand`], a narrower check than
/// [`check_raise_operand`]: it requires an *existing* exception value and
/// rejects any fresh constructor-call member, including a nested inline
/// `ExceptionGroup(...)`/`BaseExceptionGroup(...)` call, with `T0021`. A
/// member that fails its own validation reports that specific element's
/// diagnostic rather than a generic group-level one.
fn check_exception_group_operand(
    env: &Environment,
    local_names: &[&str],
    callee: &str,
    args: &[HirExpr],
) -> Result<(), Diagnostic> {
    if args.len() != 2 {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}` expects exactly 2 arguments (a message string and a literal list \
                 of member exceptions), got {}",
                args.len()
            ),
            Span::new(0, 0),
        ));
    }
    let message_ty = infer_expr_in(env, local_names, &args[0])?;
    if message_ty != Ty::Str {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}` expects a `str` message argument, got `{}`",
                message_ty.name()
            ),
            Span::new(0, 0),
        ));
    }
    let HirExpr::ListLiteral(members) = &args[1] else {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{callee}`'s second argument must be a literal list of member exceptions \
                 (`[e1, e2, ...]`); a computed or `list[T]`-typed expression is not supported"
            ),
            Span::new(0, 0),
        ));
    };
    if members.is_empty() {
        return Err(Diagnostic::error(
            "T0021",
            format!("`{callee}` requires at least one member exception"),
            Span::new(0, 0),
        ));
    }
    for member in members {
        check_exception_group_member_operand(env, local_names, member)?;
    }
    Ok(())
}

/// Part 3 of #382 (#542, PEP 654, D-202): validates one `ExceptionGroup`/
/// `BaseExceptionGroup` member expression.
///
/// Deliberately narrower than [`check_raise_operand`]: a member must be an
/// *existing* exception value (e.g. a `except ... as e:` binding, or another
/// name/expression that already evaluates to one), not a fresh
/// `SomeError("msg")` constructor call, even though such a call is a
/// perfectly valid top-level `raise` operand. `pycc_mir::exception::
/// lower_exception_value`'s `ConstructedGroup` arm lowers each member through
/// ordinary expression lowering (`lower_expr`), which has no way to
/// construct a *new* exception object -- only `lower_exception_value` itself
/// (used for the group's own top-level `raise` operand, and for a plain,
/// non-group `raise SomeError(...)`) knows how to do that. Accepting a fresh
/// constructor-call member here would type-check successfully under
/// `check_raise_operand`'s own rules and then panic in codegen (member
/// lowering would resolve `SomeError` as an ordinary user function, which it
/// is not), so this narrower rule is enforced structurally at the type-check
/// boundary rather than left for MIR/codegen to reject.
fn check_exception_group_member_operand(
    env: &Environment,
    local_names: &[&str],
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    if let HirExpr::Call { callee, .. } = expr
        && (is_unshadowed_builtin_exception(env, local_names, callee)
            || user_exception_class(env, local_names, callee).is_some())
    {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "an `ExceptionGroup`/`BaseExceptionGroup` member must be an existing exception \
                 value (e.g. a caught `except ... as e:` binding), not a fresh `{callee}(...)` \
                 construction -- assign it to a name first, then list that name"
            ),
            Span::new(0, 0),
        ));
    }
    let ty = infer_expr_in(env, local_names, expr)?;
    if let Ty::Instance(class_name) = &ty
        && pycc_hir::is_builtin_exception_class(class_name)
        && !is_user_defined_class(env, class_name)
    {
        // D-202's sixth simplification: `pycc_rt_exception_group_partition`
        // matches each member by its own top-level `type_tag` only and never
        // recurses into a member's own `exceptions`/`exceptions_len` array
        // when that member is itself a group -- unlike CPython's `split()`,
        // which does recurse into nested groups. `ExceptionGroup`/
        // `BaseExceptionGroup` are themselves ordinary entries in
        // `BUILTIN_EXCEPTION_CLASSES`, so without this check a value already
        // bound to one type-checks as a member of a freshly constructed
        // *outer* group (e.g. `ExceptionGroup("outer", [eg])` where `eg`
        // came from a prior `except* ValueError as eg:`), silently building
        // a nested group the runtime cannot partition correctly. Rejecting
        // it here keeps the type checker's accepted surface matching what
        // the runtime actually implements, the same enforce-at-type-check-
        // boundary approach this function already takes for a fresh
        // constructor-call member above.
        if class_name.as_str() == "ExceptionGroup" || class_name.as_str() == "BaseExceptionGroup" {
            return Err(Diagnostic::error(
                "T0021",
                "an `ExceptionGroup`/`BaseExceptionGroup` member must not itself be an \
                 exception group -- nested groups are not supported"
                    .to_string(),
                Span::new(0, 0),
            ));
        }
        return Ok(());
    }
    Err(Diagnostic::error(
        "T0021",
        format!(
            "an `ExceptionGroup`/`BaseExceptionGroup` member must be an exception instance, got `{}`",
            ty.name()
        ),
        Span::new(0, 0),
    ))
}

fn check_raise_operand(
    env: &Environment,
    local_names: &[&str],
    expr: &HirExpr,
    error_prefix: &str,
) -> Result<(), Diagnostic> {
    if let HirExpr::Call { callee, args } = expr
        && (callee == "ExceptionGroup" || callee == "BaseExceptionGroup")
        && is_unshadowed_builtin_exception(env, local_names, callee)
    {
        return check_exception_group_operand(env, local_names, callee, args);
    }
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

    // Part 2 of #541 (D-189): `raise MyError("boom")` for a user-declared class
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

/// Part 2 of #543 (#739): whether `name` counts as an "unshadowed builtin
/// exception" -- catchable in `except <name>:` and raisable in
/// `raise <name>("...")` without a `HirClassDef` behind it.
///
/// This is `true` for the original flat seven purely by name-table
/// membership and ordinary shadow checks: `resolve_exception_tag`
/// (`pycc_mir::exception`) resolves them by name, independent of whether the
/// class table actually seeded a definition, so an unseeded module (see
/// `pycc_hir::exception::module_shadows_builtin_exception_name`'s
/// all-or-nothing gate) still behaves correctly for them.
///
/// For the 16-member `OSError` family (Part 2 of #543, #739) this is *not*
/// enough. Those names have no name-based fallback -- deliberately, so that
/// `pycc_mir::exception::handler_type_tags`'s MRO-containment scan never
/// needs special-casing -- so a name outside the flat seven must also be
/// *actually present* in `env.classes` to count as unshadowed. Without this
/// conjunct, a module shadowing one family member (e.g. `class OSError:
/// pass`) at top level withholds seeding for *all* 23 names (the shadow
/// gate is all-or-nothing), and a separate, unrelated `except
/// FileNotFoundError:` would then reach `handler_type_tags`'s `.expect()`
/// and panic instead of producing the `T0021` this function's `false` result
/// routes to. See the Part 2 of #543 implementation plan's "shadow-gate
/// crash risk" section for the full trigger and the two rejected
/// alternatives.
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
        & (pycc_hir::is_flat_builtin_exception_class(name) || env.classes.contains_key(name))
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
/// #541, D-189). `None` for anything else, including a class that never
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
            "exception class `{}` declares or inherits an `__init__` other than \
             `Exception`'s; pycc supports only exception classes that inherit \
             `Exception`'s single-message constructor (Part 3 of #541)",
            def.name
        ),
        Span::new(0, 0),
    ))
}

/// D-068 re-review of #780 (third round, warning finding): a
/// return-type-generic sequence-checking helper, routing through
/// `narrow::check_stmt_sequence[_in_function]` instead of
/// `check_stmt`/`check_stmt_in_function` directly so a nested early-return
/// guard inside a `try`/`except`/`else`/`finally` body narrows the rest of
/// that same body -- `check_try_stmt`'s four body loops (`body`, each
/// handler's `body`, `orelse`, `finalbody`) previously called a
/// per-statement `check_stmt`/`check_stmt_in_function` dispatcher in a raw
/// loop, which never ran `apply_post_if_narrowing` and so silently dropped
/// this narrowing, exactly like the `if`/`while` fast-path helpers and
/// `check_match`'s own case-body loop had before their own fixes for the
/// same finding.
fn check_stmt_sequence_shared(
    env: &mut Environment,
    local_names: &[&str],
    stmts: &[HirStmt],
    return_ty: Option<&Ty>,
) -> Result<(), Diagnostic> {
    if let Some(return_ty) = return_ty {
        super::narrow::check_stmt_sequence_in_function(env, local_names, stmts, return_ty.clone())
    } else {
        super::narrow::check_stmt_sequence(env, stmts)
    }
}

#[cfg(test)]
mod synthetic_class_tests;

#[cfg(test)]
mod user_class_tests;

#[cfg(test)]
mod except_star_tests;
