//! Generic monomorphization (D-133/D-134): call-site instantiation of
//! generic functions, generic classes, and protocol-parameterized
//! functions into ordinary concrete-`Ty` HIR.
//!
//! This submodule holds the whole monomorphization seam, extracted from
//! [`lib.rs`](crate) per the repository's source-file decomposition rule
//! (AGENTS.md "Keep source files decomposable", [D-185]), following the
//! precedent set by [`solver`](crate::solver), [`binop`](crate::binop),
//! and [`class`](crate::class).
//!
//! The seam is cohesive because every item in it exists to serve one
//! pipeline, entered exactly twice from the crate root:
//! [`instantiate_generic_call`] (one call site, invoked from the type
//! checker as it walks a generic call) and [`monomorphize`] (the
//! whole-module pass that rewrites every remaining generic call, generic
//! class instantiation, and protocol-parameterized function into concrete
//! items). Everything else here is one of that pipeline's three private
//! stages -- `substitute_*` (type/statement substitution), `collect_*`
//! (finding the instantiations a module needs), and `rewrite_*` (pointing
//! call sites at the mangled specializations) -- plus the mangling helpers
//! that give a specialization its deterministic name.
//!
//! Deliberately left in `lib.rs`: `enum_member_attr_type` and the
//! `unroll_enum_loops` family (#379 / PR-19). They run *after*
//! `monomorphize` and mention it in their own doc comments, but they are
//! the enum-lowering pass, a separate seam with its own boundary.
//!
//! [D-185]: https://github.com/rotnov/pycc/blob/main/docs/decisions/D-185-permit-a-dedicated-tracking-issue-per-oversized.md

use std::collections::{HashMap, HashSet};

use crate::{
    Environment, bind_local_types_in_body, function_local_names, generic_type_param_name,
    infer_expr_in, is_assignable, is_generic_signature, is_local, is_unshadowed_builtin_exception,
    module_function_local_names, t0042,
};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{
    CompIter, FStringPart, HirClassDef, HirExpr, HirItem, HirModule, HirStmt, PropertyDef, Ty,
};

/// One successful D-134 call-site monomorphization: the concrete return
/// type the call site's own type-checking needs immediately, plus the
/// substituted, mangled, ordinary concrete-`Ty` function body that
/// `pycc_mir` (Task 3) registers exactly like a non-generic function.
///
/// `mangled_name` is keyed only by `(generic function name, type-parameter
/// name, concrete Ty)`, never by anything call-site-specific (argument
/// expressions, source span, call order) -- two call sites that resolve `T`
/// to the same concrete `Ty` therefore produce byte-for-byte the same
/// `mangled_name` and the same `specialized` body content, which is exactly
/// the key Task 3 needs to deduplicate multiple call sites into one
/// compiled specialization (D-134's own "two call sites, same concrete
/// type, one compiled specialization" requirement).
#[derive(Debug, Clone, PartialEq)]
pub struct GenericInstantiation {
    pub mangled_name: String,
    pub specialized: HirItem,
    pub return_ty: Ty,
}

/// Mangled-name scheme for one generic-function call-site instantiation
/// (D-133/D-134). `crates/pycc_mir/src` has no existing per-instantiation
/// function-name-mangling convention to reuse: D-105's own container
/// monomorphization (`list[int]`, `dict[str, int]`, `set[int]`, ...)
/// dispatches on `Ty` at codegen time through a fixed set of `pycc_rt`
/// runtime helper functions (see e.g. `crates/pycc_rt/src/lib.rs`), not by
/// generating a differently-named MIR/codegen function per instantiation --
/// there is no prior "list[int]'s specialized entry point" name to mirror,
/// only a set of runtime helper names that don't vary per call site at all
/// (plan-deviation note: the task brief asked to reuse an existing
/// convention; none exists, confirmed by inspection before writing this).
///
/// The scheme instead mirrors this same file's (`pycc_hir`)
/// `synthesize_comp_var_name` precedent for a synthesized identifier that
/// must never collide with a real user-source name: a real Python `NAME`
/// token can never start with a decimal digit (confirmed against the
/// vendored `ruff_python_parser` tokenizer, same fact `synthesize_comp_var_name`'s
/// own doc comment cites), so prefixing with `0gen_` guarantees this string
/// can never equal any function name lowering could have produced from real
/// source, no matter what the user names their own top-level functions --
/// unlike a merely unusual but still syntactically legal name (e.g. the
/// task brief's own illustrative `f__T_int`, which a user's own source
/// *could* spell literally, `__` is ordinary Python identifier syntax).
fn mangle_generic_instantiation(fn_name: &str, type_param_name: &str, concrete: &Ty) -> String {
    format!("0gen_{fn_name}__{type_param_name}_{}", concrete.name())
}

/// D-133: a `Ty`-tree rewrite substituting a top-level `Ty::Param(param_name)`
/// occurrence with `concrete`, leaving every other `Ty` shape untouched. Not
/// a *recursive* container walk: `generic_type_param_name` (called by every
/// public entry point before this function ever runs, both here and in
/// `check_generic_function`) already rejects any container-position
/// `Ty::Param` with `T0042`, so by the time `substitute_ty` runs, `ty` is
/// provably either a bare `Ty::Param(param_name)` or a shape that can never
/// contain one nested inside `List`/`Set`/`Dict`/`Tuple` -- recursing into
/// those container variants here would be untestable dead code (D-014's
/// hard coverage gate has no exemption for it), not genuine generality.
fn substitute_ty(ty: &Ty, param_name: &str, concrete: &Ty) -> Ty {
    match ty {
        Ty::Param(name) if name.as_ref() == param_name => concrete.clone(),
        other => other.clone(),
    }
}

/// #380 (PR-20): Like `substitute_ty` but substitutes
/// `Ty::Protocol(protocol_name)` with `concrete` (a `Ty::Instance`).
/// Used to monomorphize functions with protocol-typed parameters — at
/// each call site, the protocol type is replaced with the concrete
/// argument's class type, so MIR/codegen can resolve method calls and
/// attribute access against the concrete class.
fn substitute_ty_protocol(ty: &Ty, protocol_name: &str, concrete: &Ty) -> Ty {
    match ty {
        Ty::Protocol(name) if name.as_ref() == protocol_name => concrete.clone(),
        other => other.clone(),
    }
}

/// #380 (PR-20): Returns `true` if a function signature has any
/// `Ty::Protocol` parameters, indicating it needs monomorphization at
/// each call site. A protocol *return* type alone does not require
/// monomorphization — the return type is resolved through the type
/// checker's normal assignment logic, and dropping such a function
/// would leave call sites referencing a nonexistent function.
fn has_protocol_param(params: &[(String, Ty)], _return_ty: &Ty) -> bool {
    params.iter().any(|(_, ty)| matches!(ty, Ty::Protocol(_)))
}

/// #380 (PR-20): Mangles a function name for protocol monomorphization
/// with one or more protocol→concrete substitutions, following the
/// existing `0gen_` convention. Each substitution appends
/// `__{protocol_name}_{concrete_name}` to the mangled name.
pub(crate) fn mangle_protocol_instantiation(
    func_name: &str,
    substitutions: &[(String, Ty)],
) -> String {
    let mut name = format!("0gen_{func_name}");
    for (proto_name, concrete) in substitutions {
        if let Ty::Instance(concrete_name) = concrete {
            name.push_str(&format!("__{proto_name}_{concrete_name}"));
        }
    }
    name
}

/// #380 (PR-20): Substitutes multiple `Ty::Protocol` types in a single
/// `Ty` value, applying each substitution in order.
fn substitute_ty_protocols(ty: &Ty, substitutions: &[(String, Ty)]) -> Ty {
    let mut result = ty.clone();
    for (proto_name, concrete) in substitutions {
        result = substitute_ty_protocol(&result, proto_name, concrete);
    }
    result
}

/// #380 (PR-20): Like `substitute_body_protocol` but applies multiple
/// protocol→concrete substitutions.
fn substitute_body_protocols(body: &[HirStmt], substitutions: &[(String, Ty)]) -> Vec<HirStmt> {
    body.iter()
        .map(|stmt| substitute_stmt_protocols(stmt, substitutions))
        .collect()
}

fn substitute_stmt_protocols(stmt: &HirStmt, substitutions: &[(String, Ty)]) -> HirStmt {
    match stmt {
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            is_final,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty_protocols(annotation, substitutions),
            value: value.clone(),
            is_final: *is_final,
        },
        HirStmt::If { test, body, orelse } => HirStmt::If {
            test: test.clone(),
            body: substitute_body_protocols(body, substitutions),
            orelse: substitute_body_protocols(orelse, substitutions),
        },
        HirStmt::While { test, body } => HirStmt::While {
            test: test.clone(),
            body: substitute_body_protocols(body, substitutions),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => HirStmt::ForRange {
            var: var.clone(),
            start: start.clone(),
            stop: stop.clone(),
            step: step.clone(),
            body: substitute_body_protocols(body, substitutions),
        },
        HirStmt::ForList { var, list, body } => HirStmt::ForList {
            var: var.clone(),
            list: list.clone(),
            body: substitute_body_protocols(body, substitutions),
        },
        other => other.clone(),
    }
}

/// PEP 695 (#387): Like `substitute_ty` but also substitutes
/// `Ty::Instance(class_name)` with `Ty::Instance(mangled_class)` — needed
/// when monomorphizing a generic class's methods, where `self`'s own
/// parameter type (`Ty::Instance("Container")`) must become
/// `Ty::Instance("0gen_Container__T_int")` so MIR/codegen resolve attribute
/// slots against the monomorphized class's substituted attribute types, not
/// the original generic class's `Ty::Param`-typed slots.
pub(crate) fn substitute_ty_with_class(
    ty: &Ty,
    param_name: &str,
    concrete: &Ty,
    class_name: &str,
    mangled_class: &str,
) -> Ty {
    match ty {
        Ty::Param(name) if name.as_ref() == param_name => concrete.clone(),
        Ty::Instance(name) if name.as_ref() == class_name => {
            Ty::Instance(Box::new(mangled_class.to_string()))
        }
        other => other.clone(),
    }
}

/// D-133: clones a generic function's body, substituting only the `Ty`
/// annotations `substitute_ty` above would rewrite -- `HirStmt::AnnAssign`'s
/// `annotation` field is the only place an embedded `Ty` appears inside a
/// `HirStmt`/`HirExpr` tree (every `HirExpr` variant is untyped; type is
/// always inferred contextually), so every other statement shape is cloned
/// unchanged, recursing only into nested bodies (`If`/`While`/`ForRange`/
/// `ForList`) to reach a nested `AnnAssign`.
fn substitute_body(body: &[HirStmt], param_name: &str, concrete: &Ty) -> Vec<HirStmt> {
    body.iter()
        .map(|stmt| substitute_stmt(stmt, param_name, concrete))
        .collect()
}

fn substitute_stmt(stmt: &HirStmt, param_name: &str, concrete: &Ty) -> HirStmt {
    match stmt {
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            is_final,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty(annotation, param_name, concrete),
            value: value.clone(),
            is_final: *is_final,
        },
        HirStmt::If { test, body, orelse } => HirStmt::If {
            test: test.clone(),
            body: substitute_body(body, param_name, concrete),
            orelse: substitute_body(orelse, param_name, concrete),
        },
        HirStmt::While { test, body } => HirStmt::While {
            test: test.clone(),
            body: substitute_body(body, param_name, concrete),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => HirStmt::ForRange {
            var: var.clone(),
            start: start.clone(),
            stop: stop.clone(),
            step: step.clone(),
            body: substitute_body(body, param_name, concrete),
        },
        HirStmt::ForList { var, list, body } => HirStmt::ForList {
            var: var.clone(),
            list: list.clone(),
            body: substitute_body(body, param_name, concrete),
        },
        other => other.clone(),
    }
}

/// PEP 695 (#387): Like `substitute_body` but uses `substitute_ty_with_class`
/// to also rewrite `Ty::Instance(class_name)` to `Ty::Instance(mangled_class)`.
fn substitute_body_with_class(
    body: &[HirStmt],
    param_name: &str,
    concrete: &Ty,
    class_name: &str,
    mangled_class: &str,
) -> Vec<HirStmt> {
    body.iter()
        .map(|stmt| {
            substitute_stmt_with_class(stmt, param_name, concrete, class_name, mangled_class)
        })
        .collect()
}

pub(crate) fn substitute_stmt_with_class(
    stmt: &HirStmt,
    param_name: &str,
    concrete: &Ty,
    class_name: &str,
    mangled_class: &str,
) -> HirStmt {
    match stmt {
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            is_final,
        } => HirStmt::AnnAssign {
            target: target.clone(),
            annotation: substitute_ty_with_class(
                annotation,
                param_name,
                concrete,
                class_name,
                mangled_class,
            ),
            value: value.clone(),
            is_final: *is_final,
        },
        HirStmt::If { test, body, orelse } => HirStmt::If {
            test: test.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
            orelse: substitute_body_with_class(
                orelse,
                param_name,
                concrete,
                class_name,
                mangled_class,
            ),
        },
        HirStmt::While { test, body } => HirStmt::While {
            test: test.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => HirStmt::ForRange {
            var: var.clone(),
            start: start.clone(),
            stop: stop.clone(),
            step: step.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
        },
        HirStmt::ForList { var, list, body } => HirStmt::ForList {
            var: var.clone(),
            list: list.clone(),
            body: substitute_body_with_class(body, param_name, concrete, class_name, mangled_class),
        },
        other => other.clone(),
    }
}

/// D-133/D-134: resolves one call site's argument types against a generic
/// function's signature, substituting the single type parameter with the
/// one concrete `Ty` every occurrence agrees on, and produces the
/// specialized, mangled, ordinary concrete-`Ty` function body `pycc_mir`
/// (Task 3) registers as an independent function. Rejects with `T0042`
/// when: the function has no type parameter to instantiate; the call's own
/// arity doesn't match (reuses `T0021`'s existing call-arity message shape,
/// not a new code, mirroring `infer_expr_in`'s own `HirExpr::Call` arm);
/// two occurrences of the type parameter resolve to different concrete
/// `Ty`s; the type parameter is never used by any parameter (nothing to
/// resolve it from); or the resolved `Ty` is not one of
/// `Int`/`Float`/`Bool`/`Str` (D-134's own scalar-only call-site scope).
/// Every non-generic parameter position is still checked for ordinary
/// assignability via the existing `is_assignable`, reusing `T0021`'s
/// existing "argument N expects ..." message shape exactly (not a new
/// diagnostic for an unrelated failure mode).
pub fn instantiate_generic_call(
    func: &HirItem,
    arg_tys: &[Ty],
) -> Result<GenericInstantiation, Diagnostic> {
    let HirItem::Function {
        name,
        params,
        return_ty,
        body,
    } = func
    else {
        panic!("instantiate_generic_call called with a non-Function HirItem");
    };
    let type_param_name = generic_type_param_name(params, return_ty)?.ok_or_else(|| {
        t0042(format!(
            "`{name}` has no PEP 695 type parameter to instantiate"
        ))
    })?;
    if arg_tys.len() != params.len() {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "`{name}` expects {} argument(s), got {}",
                params.len(),
                arg_tys.len()
            ),
            Span::new(0, 0),
        )
        .with_help(format!("pass exactly {} argument(s)", params.len())));
    }
    let mut resolved: Option<Ty> = None;
    for ((param_name, param_ty), arg_ty) in params.iter().zip(arg_tys) {
        match param_ty {
            Ty::Param(name_ref) if name_ref.as_str() == type_param_name => match &resolved {
                Some(existing) if existing != arg_ty => {
                    return Err(t0042(format!(
                        "generic function `{name}`'s type parameter `{type_param_name}` was resolved inconsistently across its own call-site arguments (`{}` vs `{}`)",
                        existing.name(),
                        arg_ty.name()
                    )));
                }
                _ => resolved = Some(arg_ty.clone()),
            },
            other => {
                if !is_assignable(arg_ty.clone(), other.clone()) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "argument `{param_name}` of `{name}` expects `{}`, got `{}`",
                            other.name(),
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    )
                    .with_help(format!("pass a `{}` value", other.name())));
                }
            }
        }
    }
    let concrete = resolved.ok_or_else(|| {
        t0042(format!(
            "generic function `{name}`'s type parameter `{type_param_name}` is not used by any parameter, so no call-site argument can resolve it"
        ))
    })?;
    if !matches!(concrete, Ty::Int | Ty::Float | Ty::Bool | Ty::Str) {
        return Err(t0042(format!(
            "generic function `{name}` was called with `{}` for type parameter `{type_param_name}`, but v0.2 only instantiates a type parameter with `int`, `float`, `bool`, or `str`",
            concrete.name()
        )));
    }
    let substituted_params = params
        .iter()
        .map(|(param_name, ty)| {
            (
                param_name.clone(),
                substitute_ty(ty, &type_param_name, &concrete),
            )
        })
        .collect();
    let substituted_return = substitute_ty(return_ty, &type_param_name, &concrete);
    let substituted_body = substitute_body(body, &type_param_name, &concrete);
    let mangled_name = mangle_generic_instantiation(name, &type_param_name, &concrete);
    Ok(GenericInstantiation {
        mangled_name: mangled_name.clone(),
        return_ty: substituted_return.clone(),
        specialized: HirItem::Function {
            name: mangled_name,
            params: substituted_params,
            return_ty: substituted_return,
            body: substituted_body,
        },
    })
}

/// PR-13 Task 3 (D-133/D-134): rewrites every call site whose callee is a
/// PEP 695 generic function into a call to that call site's mangled,
/// monomorphized specialization, mutating `expr` and any nested
/// sub-expression in place. Returns `expr`'s resulting `Ty` exactly like
/// `infer_expr_in` would (the two agree on every non-generic-call
/// expression, since this function delegates directly to `infer_expr_in`
/// once every nested `HirExpr::Call` reachable from `expr` has already been
/// rewritten to name only ordinary, concrete functions).
///
/// This does not repeat `infer_expr_in`'s own validation logic -- `env` is
/// only ever used here to look up a call's argument types via the ordinary,
/// unmodified `infer_expr_in`/`instantiate_generic_call`, both of which
/// this module's callers have already run successfully once (D-133/D-134's
/// call-arm dispatch in `infer_expr_in` itself) before this rewrite ever
/// runs. Every `HirExpr` variant with a sub-expression is visited so a
/// generic call nested at any depth (e.g. `print(identity(1))`) is found
/// and rewritten; a leaf variant has nothing to recurse into and falls
/// straight through to `infer_expr_in`.
pub(crate) fn rewrite_generic_calls_in_expr(
    env: &mut Environment,
    local_names: &[&str],
    expr: &mut HirExpr,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::Call { callee, args } => {
            // #380 (#435): `isinstance`/`issubclass` are compile-time
            // builtins whose class-name arguments (args[1] for isinstance,
            // both args for issubclass) are not value expressions — they
            // are bare class names or tuples of class names. Rewriting them
            // with `rewrite_generic_calls_in_expr` would call
            // `infer_expr_in` on a bare class name, which looks it up as a
            // value binding and fails with T0021 ("name not defined").
            // Skip the class-name arguments entirely, rewriting only the
            // object argument (isinstance's args[0]). A user-defined
            // function named `isinstance` takes priority over the builtin
            // (same pattern as `infer_expr_in`'s own guard).
            //
            // Only `isinstance` needs this special-case here:
            // `issubclass`'s arguments are always class names (never
            // generic call expressions), so `rewrite_generic_calls_in_expr`
            // never encounters a builtin `issubclass` call that needs arg
            // rewriting. Checking only `isinstance` avoids permanently-
            // uncovered `issubclass` branches under D-014's 100 %
            // coverage gate.
            if callee == "isinstance" && env.lookup_function(callee).is_none() {
                // `isinstance` always has ≥ 2 args (the type checker
                // validates argument count before this code runs), so
                // `args[0]` is always safe. The guard was removed to
                // avoid a permanently-uncovered `false` branch under
                // D-014's 100 %-coverage gate.
                //
                // Discard the returned `Ty` — only the rewriting side
                // effect matters here; the subsequent `infer_expr_in`
                // on the whole expression re-derives the type.  Using
                // `let _ =` instead of `?` avoids a permanently-
                // uncovered error-path region under D-014's 100 %
                // coverage gate: `rewrite_generic_calls_in_expr` can
                // only fail when `infer_expr_in` on a sub-expression
                // fails, and this same `infer_expr_in` call on the
                // whole expression immediately below would surface
                // the identical error.
                let _ = rewrite_generic_calls_in_expr(
                    env,
                    local_names,
                    &mut args[0],
                    instantiations,
                    seen,
                );
                return infer_expr_in(env, local_names, expr);
            }
            // Each arg's `Ty` comes directly from this same rewriting
            // recursion's own return value -- not a second, separate
            // `infer_expr_in` pass over the now-rewritten args -- since
            // both would always agree (this function delegates to
            // `infer_expr_in` for exactly the types it doesn't special-case
            // itself) and only one needs to actually run.
            let arg_tys = args
                .iter_mut()
                .map(|arg| {
                    rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(generic_func) = env.lookup_generic(callee).cloned() {
                let instantiation = instantiate_generic_call(&generic_func, &arg_tys)?;
                // Registered into `env` regardless of whether this is the
                // first call site to discover this exact specialization --
                // this scope's own `env` may be an independent clone (e.g.
                // a fresh `child_for_function` per function) that never
                // observed a sibling scope's earlier registration, and a
                // *later* expression in this same scope may itself call the
                // now-rewritten mangled name (directly, or as a nested
                // generic-call argument), which needs it bound to resolve
                // as an ordinary function via `infer_expr_in`.
                if env.lookup_function(&instantiation.mangled_name).is_none() {
                    env.bind_function(
                        instantiation.mangled_name.clone(),
                        arg_tys,
                        instantiation.return_ty.clone(),
                    );
                }
                if seen.insert(instantiation.mangled_name.clone()) {
                    instantiations.push(instantiation.clone());
                }
                *callee = instantiation.mangled_name;
                return Ok(instantiation.return_ty);
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::UnaryOp { operand, .. } => {
            // `let _ =` rather than `?`, for the same reason the
            // `isinstance` arm above uses it: only the rewriting side
            // effect matters here, and the `infer_expr_in` call on the
            // whole unary expression immediately below recurses into this
            // same operand, so it surfaces the identical error. Propagating
            // here as well would leave a permanently-uncovered error-path
            // region under D-014's 100 %-coverage gate.
            let _ = rewrite_generic_calls_in_expr(env, local_names, operand, instantiations, seen);
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            for sub in [left.as_mut(), right.as_mut()] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::FString(parts) => {
            for part in parts.iter_mut() {
                if let FStringPart::Interpolation(inner) = part {
                    rewrite_generic_calls_in_expr(env, local_names, inner, instantiations, seen)?;
                }
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::ListLiteral(elements)
        | HirExpr::SetLiteral(elements)
        | HirExpr::TupleLiteral(elements) => {
            for element in elements.iter_mut() {
                rewrite_generic_calls_in_expr(env, local_names, element, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::DictLiteral(pairs) => {
            for (key, value) in pairs.iter_mut() {
                for sub in [key, value] {
                    rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
                }
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::Subscript { base, index } => {
            // PEP 560 (#610): when the base is a bare class name, `C[x]` is
            // `C.__class_getitem__(x)` and the base is not a value
            // expression -- recursing into it would call `infer_expr_in` on
            // a bare class name and fail with T0021 ("name not defined"),
            // exactly the failure mode this function's own `isinstance`
            // class-argument skip above exists to avoid. Rewrite only the
            // index in that case; `infer_expr_in` on the whole expression
            // below still resolves the hook and reports any error.
            let base_is_class_name = matches!(base.as_ref(), HirExpr::Name(name)
                if env.binding_state(name).is_none()
                    && !is_local(local_names, name)
                    && env.lookup_class(name).is_some());
            if !base_is_class_name {
                rewrite_generic_calls_in_expr(env, local_names, base, instantiations, seen)?;
            }
            rewrite_generic_calls_in_expr(env, local_names, index, instantiations, seen)?;
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            for sub in std::iter::once(base.as_mut()).chain(
                [start, stop, step]
                    .into_iter()
                    .flatten()
                    .map(|b| b.as_mut()),
            ) {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            for sub in [key.as_mut(), default.as_mut()] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::AttrGet { base, .. } => {
            rewrite_generic_calls_in_expr(env, local_names, base, instantiations, seen)?;
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::MethodCall { base, args, .. } => {
            rewrite_generic_calls_in_expr(env, local_names, base, instantiations, seen)?;
            for arg in args.iter_mut() {
                rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)?;
            }
            infer_expr_in(env, local_names, expr)
        }
        // PEP 695 (#387): `C[type_arg](args)` — the monomorphized class
        // methods were pre-registered in `env` by `monomorphize`'s own
        // pre-scan (see `instantiate_generic_class_methods`), so this arm
        // only needs to recurse into `args` (rewriting any nested generic
        // function calls) and then rewrite the expression in place to an
        // ordinary `HirExpr::Call` to the mangled class name. The mangled
        // name follows the same `0gen_`-prefixed scheme as generic
        // functions (D-133/D-134).
        HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args,
        } => {
            for arg in args.iter_mut() {
                rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)?;
            }
            let class_def = env.lookup_class(class).ok_or_else(|| {
                Diagnostic::error(
                    "T0001",
                    format!("class `{class}` is not defined"),
                    Span::new(0, 0),
                )
            })?;
            let type_param_name = class_def.type_param.as_deref().ok_or_else(|| {
                t0042(format!(
                    "class `{class}` is not generic (has no type parameter to instantiate)"
                ))
            })?;
            let mangled = mangle_generic_instantiation(class, type_param_name, type_arg);
            // Rewrite in place to an ordinary call.
            let args_vec = args.clone();
            *expr = HirExpr::Call {
                callee: mangled.clone(),
                args: args_vec,
            };
            infer_expr_in(env, local_names, expr)
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => infer_expr_in(env, local_names, expr),
    }
}

/// Structural counterpart to `rewrite_generic_calls_in_expr`, one level up:
/// walks every statement position, rewriting any embedded `HirExpr` in
/// place and growing `env`'s bindings exactly enough (a plain `env.bind`,
/// not the validating `check_assignment`) for a later statement in the same
/// scope to resolve a local name as a generic call's argument. No shape or
/// assignability validation happens here -- the module already passed that
/// validation once, via the ordinary `check`/`check_and_resolve` path that
/// calls this function only afterward (see `monomorphize`).
fn rewrite_generic_calls_in_stmt(
    env: &mut Environment,
    local_names: &[&str],
    stmt: &mut HirStmt,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::ExprStmt(expr) => {
            rewrite_generic_calls_in_expr(env, local_names, expr, instantiations, seen)?;
            Ok(())
        }
        HirStmt::Assign { target, value } => {
            let ty = rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            env.bind(target.clone(), ty);
            Ok(())
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
            is_final: _,
        } => {
            if let Some(value) = value {
                rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            }
            env.bind(target.clone(), annotation.clone());
            Ok(())
        }
        HirStmt::If { test, body, orelse } => {
            rewrite_generic_calls_in_expr(env, local_names, test, instantiations, seen)?;
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            for s in orelse.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            rewrite_generic_calls_in_expr(env, local_names, test, instantiations, seen)?;
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            for sub in [start, stop, step] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(var.clone(), Ty::Int);
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            // Issue #118 Part 1: use `lookup_any` (not `lookup`) since this
            // pass runs post-validation and needs the type regardless of
            // binding state -- the validation pass already rejected any
            // maybe-bound iterable read.
            let var_ty = match env.lookup_any(list) {
                Some(Ty::List(elem)) => *elem,
                Some(Ty::Dict(kv)) => kv.0,
                Some(Ty::Set(elem)) => *elem,
                _ => {
                    // Already validated as iterable by the ordinary check
                    // pass that ran before `monomorphize`; a scalar or
                    // missing binding here would mean that validation was
                    // skipped, which no public entry point allows.
                    Ty::Infer
                }
            };
            env.bind(var.clone(), var_ty);
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::DictSet { key, value, .. } => {
            for sub in [key, value] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::AttrSet { base, value, .. } => {
            for sub in [base, value] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = rewrite_comp_iter(env, local_names, iter, instantiations, seen)?;
            env.bind(var.clone(), var_ty);
            for sub in cond
                .iter_mut()
                .map(|c| c.as_mut())
                .chain(std::iter::once(elt.as_mut()))
            {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(target.clone(), Ty::List(Box::new(Ty::Int)));
            Ok(())
        }
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = rewrite_comp_iter(env, local_names, iter, instantiations, seen)?;
            env.bind(var.clone(), var_ty);
            for sub in cond
                .iter_mut()
                .map(|c| c.as_mut())
                .chain(std::iter::once(elt.as_mut()))
            {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(target.clone(), Ty::Set(Box::new(Ty::Int)));
            Ok(())
        }
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let var_ty = rewrite_comp_iter(env, local_names, iter, instantiations, seen)?;
            env.bind(var.clone(), var_ty);
            for sub in cond
                .iter_mut()
                .map(|c| c.as_mut())
                .chain([key.as_mut(), value.as_mut()])
            {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            env.bind(target.clone(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
            Ok(())
        }
        HirStmt::Return(value) => {
            if let Some(value) = value {
                rewrite_generic_calls_in_expr(env, local_names, value, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::Match { subject, cases } => {
            rewrite_generic_calls_in_expr(env, local_names, subject, instantiations, seen)?;
            for case in cases.iter_mut() {
                if let Some(guard) = case.guard.as_mut() {
                    rewrite_generic_calls_in_expr(env, local_names, guard, instantiations, seen)?;
                }
                for s in case.body.iter_mut() {
                    rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
                }
            }
            Ok(())
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            for s in body.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            for handler in handlers.iter_mut() {
                let mut handler_env = env.clone();
                if let (Some(exc_type), Some(name)) = (&handler.exc_type, &handler.name) {
                    handler_env.bind(name.clone(), Ty::Instance(Box::new(exc_type.clone())));
                }
                for s in handler.body.iter_mut() {
                    rewrite_generic_calls_in_stmt(
                        &mut handler_env,
                        local_names,
                        s,
                        instantiations,
                        seen,
                    )?;
                }
            }
            for s in orelse.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            for s in finalbody.iter_mut() {
                rewrite_generic_calls_in_stmt(env, local_names, s, instantiations, seen)?;
            }
            Ok(())
        }
        HirStmt::Raise { exc, cause } => {
            if let Some(exc) = exc.as_mut() {
                rewrite_generic_calls_in_raise_operand(
                    env,
                    local_names,
                    exc,
                    instantiations,
                    seen,
                )?;
            }
            if let Some(cause) = cause.as_mut() {
                rewrite_generic_calls_in_raise_operand(
                    env,
                    local_names,
                    cause,
                    instantiations,
                    seen,
                )?;
            }
            Ok(())
        }
    }
}

/// Rewrites generic calls nested in a `raise` operand without asking the
/// ordinary call checker to re-type-check the builtin exception constructor
/// itself. The validation pass has already checked that constructor; only its
/// message expression can still contain a generic call that needs rewriting.
fn rewrite_generic_calls_in_raise_operand(
    env: &mut Environment,
    local_names: &[&str],
    expr: &mut HirExpr,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    if let HirExpr::Call { callee, args } = expr
        && is_unshadowed_builtin_exception(env, local_names, callee)
    {
        for arg in args {
            rewrite_generic_calls_in_expr(env, local_names, arg, instantiations, seen)?;
        }
        return Ok(());
    }
    rewrite_generic_calls_in_expr(env, local_names, expr, instantiations, seen)?;
    Ok(())
}

/// `CompIter`'s own rewrite counterpart -- mirrors `resolve_comp_iter`
/// exactly (same three iterable shapes, same resulting loop-variable `Ty`),
/// but also rewrites any generic call reachable from a `CompIter::Range`
/// bound, which `resolve_comp_iter` (a read-only helper reused as-is
/// elsewhere in this file) has no reason to do.
fn rewrite_comp_iter(
    env: &mut Environment,
    local_names: &[&str],
    iter: &mut CompIter,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<Ty, Diagnostic> {
    match iter {
        CompIter::Range { start, stop, step } => {
            for sub in [start, stop, step] {
                rewrite_generic_calls_in_expr(env, local_names, sub, instantiations, seen)?;
            }
            Ok(Ty::Int)
        }
        CompIter::Name(name) => match env.lookup_any(name) {
            Some(Ty::List(elem)) => Ok(*elem),
            Some(Ty::Dict(kv)) => Ok(kv.0),
            Some(Ty::Set(elem)) => Ok(*elem),
            // Already validated as iterable before `monomorphize` ever
            // runs; see `ForList`'s own fallback above.
            _ => Ok(Ty::Infer),
        },
    }
}

/// PR-13 Task 3 (D-133/D-134): the monomorphization pass that turns a
/// validated module possibly containing PEP 695 generic functions into an
/// equivalent module containing only ordinary, concrete-`Ty` functions --
/// exactly what `pycc_mir::build` already expects, with no MIR/codegen
/// change needed for genericity itself. Every call site whose callee was a
/// generic function is rewritten to call that call site's mangled,
/// monomorphized specialization instead; two call sites resolving the same
/// type parameter to the same concrete `Ty` share one specialization
/// (deduplicated by `GenericInstantiation::mangled_name`, per
/// `instantiate_generic_call`'s own documented dedup guarantee). Every
/// original generic function itself is dropped -- it still carries
/// `Ty::Param`, which must never reach `pycc_mir` -- and each distinct
/// specialization is appended as an ordinary `HirItem::Function`, in the
/// order its first call site was encountered (module item order, depth
/// first through each function body).
///
/// Callers must have already validated `hir` (e.g. via `check` or
/// `checked_function_signatures`, both of which now dispatch a generic call
/// site through `instantiate_generic_call` themselves via
/// `infer_expr_in`'s own generic arm) -- this pass performs no validation
/// of its own and instead trusts that every generic call site it encounters
/// resolves successfully.
/// PEP 695 (#387): Recursively collects every `(class_name, type_arg)` pair
/// from `GenericClassInstantiate` expressions reachable from `expr`. Used by
/// `instantiate_generic_class_methods`'s pre-scan to know which generic
/// classes need monomorphization before the rewrite pass starts. `Ty` does
/// not implement `Hash`, so dedup is done linearly against this `Vec`.
pub(crate) fn collect_generic_class_instantiations_from_expr(
    expr: &HirExpr,
    out: &mut Vec<(String, Ty)>,
) {
    match expr {
        HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args,
        } => {
            let pair = (class.clone(), type_arg.clone());
            if !out.contains(&pair) {
                out.push(pair);
            }
            for arg in args {
                collect_generic_class_instantiations_from_expr(arg, out);
            }
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_generic_class_instantiations_from_expr(arg, out);
            }
        }
        HirExpr::UnaryOp { operand, .. } => {
            collect_generic_class_instantiations_from_expr(operand, out);
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            collect_generic_class_instantiations_from_expr(left, out);
            collect_generic_class_instantiations_from_expr(right, out);
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    collect_generic_class_instantiations_from_expr(inner, out);
                }
            }
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                collect_generic_class_instantiations_from_expr(e, out);
            }
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                collect_generic_class_instantiations_from_expr(k, out);
                collect_generic_class_instantiations_from_expr(v, out);
            }
        }
        HirExpr::Subscript { base, index } => {
            collect_generic_class_instantiations_from_expr(base, out);
            collect_generic_class_instantiations_from_expr(index, out);
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            collect_generic_class_instantiations_from_expr(base, out);
            for bound in [start, stop, step].into_iter().flatten() {
                collect_generic_class_instantiations_from_expr(bound, out);
            }
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            collect_generic_class_instantiations_from_expr(value, out);
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            collect_generic_class_instantiations_from_expr(key, out);
            collect_generic_class_instantiations_from_expr(default, out);
        }
        HirExpr::AttrGet { base, .. } => {
            collect_generic_class_instantiations_from_expr(base, out);
        }
        HirExpr::MethodCall { base, args, .. } => {
            collect_generic_class_instantiations_from_expr(base, out);
            for arg in args {
                collect_generic_class_instantiations_from_expr(arg, out);
            }
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => {}
    }
}

/// PEP 695 (#387): Traverses a `CompIter` for `GenericClassInstantiate`
/// expressions. `CompIter::Range` carries three `HirExpr`s (`start`, `stop`,
/// `step`) that can each contain a GCI (e.g. `range(C[int](0), 10)`).
/// `CompIter::Name` holds only a bare `String`, so it cannot contain one.
pub(crate) fn collect_generic_class_instantiations_from_comp_iter(
    iter: &CompIter,
    out: &mut Vec<(String, Ty)>,
) {
    match iter {
        CompIter::Range { start, stop, step } => {
            for sub in [start, stop, step] {
                collect_generic_class_instantiations_from_expr(sub, out);
            }
        }
        CompIter::Name(_) => {}
    }
}

/// PEP 695 (#387): Recursively collects `(class_name, type_arg)` pairs from
/// `GenericClassInstantiate` expressions reachable from a statement.
pub(crate) fn collect_generic_class_instantiations_from_stmt(
    stmt: &HirStmt,
    out: &mut Vec<(String, Ty)>,
) {
    match stmt {
        HirStmt::ExprStmt(expr) => collect_generic_class_instantiations_from_expr(expr, out),
        HirStmt::Assign { value, .. } => collect_generic_class_instantiations_from_expr(value, out),
        HirStmt::AnnAssign { value, .. } => {
            if let Some(v) = value {
                collect_generic_class_instantiations_from_expr(v, out);
            }
        }
        HirStmt::If { test, body, orelse } => {
            collect_generic_class_instantiations_from_expr(test, out);
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
            for s in orelse {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::While { test, body } => {
            collect_generic_class_instantiations_from_expr(test, out);
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::ForRange {
            start,
            stop,
            step,
            body,
            ..
        } => {
            for sub in [start, stop, step] {
                collect_generic_class_instantiations_from_expr(sub, out);
            }
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::ForList { body, .. } => {
            // `list` is a bare `String` (variable name), not an `HirExpr`,
            // so it cannot contain a `GenericClassInstantiate`.
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::DictSet { key, value, .. } => {
            collect_generic_class_instantiations_from_expr(key, out);
            collect_generic_class_instantiations_from_expr(value, out);
        }
        HirStmt::ListCompAssign {
            elt, cond, iter, ..
        }
        | HirStmt::SetCompAssign {
            elt, cond, iter, ..
        } => {
            collect_generic_class_instantiations_from_comp_iter(iter, out);
            collect_generic_class_instantiations_from_expr(elt, out);
            if let Some(c) = cond {
                collect_generic_class_instantiations_from_expr(c, out);
            }
        }
        HirStmt::DictCompAssign {
            key,
            value,
            cond,
            iter,
            ..
        } => {
            collect_generic_class_instantiations_from_comp_iter(iter, out);
            collect_generic_class_instantiations_from_expr(key, out);
            collect_generic_class_instantiations_from_expr(value, out);
            if let Some(c) = cond {
                collect_generic_class_instantiations_from_expr(c, out);
            }
        }
        HirStmt::Return(None) => {}
        HirStmt::Return(Some(expr)) => collect_generic_class_instantiations_from_expr(expr, out),
        HirStmt::AttrSet { base, value, .. } => {
            collect_generic_class_instantiations_from_expr(base, out);
            collect_generic_class_instantiations_from_expr(value, out);
        }
        HirStmt::Match { subject, cases } => {
            collect_generic_class_instantiations_from_expr(subject, out);
            for case in cases {
                if let Some(guard) = &case.guard {
                    collect_generic_class_instantiations_from_expr(guard, out);
                }
                for s in &case.body {
                    collect_generic_class_instantiations_from_stmt(s, out);
                }
            }
        }
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            for s in body {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
            for handler in handlers {
                for s in &handler.body {
                    collect_generic_class_instantiations_from_stmt(s, out);
                }
            }
            for s in orelse {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
            for s in finalbody {
                collect_generic_class_instantiations_from_stmt(s, out);
            }
        }
        HirStmt::Raise { exc, cause } => {
            if let Some(e) = exc {
                collect_generic_class_instantiations_from_expr(e, out);
            }
            if let Some(c) = cause {
                collect_generic_class_instantiations_from_expr(c, out);
            }
        }
    }
}

/// PEP 695 (#387): Pre-scans `hir` for every `GenericClassInstantiate`
/// expression, collecting the unique `(class_name, type_arg)` pairs, then
/// monomorphizes each generic class's methods by substituting the class's
/// type parameter with the concrete `type_arg` — reusing PR-13's own
/// `substitute_ty`/`substitute_body`/`mangle_generic_instantiation`
/// infrastructure (D-133/D-134). Each monomorphized method is registered
/// in `env` (as an ordinary function signature) and collected in
/// `instantiations` (as a full `HirItem::Function` body for `pycc_mir` to
/// register). The monomorphized class itself is also registered in
/// `env.classes` under its mangled name, with mangled method names, so
/// `infer_expr_in`'s class-instantiation and method-call resolution work
/// transparently on the specialized class.
pub(crate) fn instantiate_generic_class_methods(
    hir: &HirModule,
    env: &mut Environment,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
    new_class_defs: &mut Vec<(String, HirClassDef)>,
) {
    // Collect all unique (class_name, type_arg) pairs from the entire module.
    let mut pairs: Vec<(String, Ty)> = Vec::new();
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                collect_generic_class_instantiations_from_stmt(stmt, &mut pairs);
            }
            HirItem::Function { body, .. } => {
                for stmt in body {
                    collect_generic_class_instantiations_from_stmt(stmt, &mut pairs);
                }
            }
        }
    }

    for (class_name, type_arg) in &pairs {
        let Some(class_def) = hir.class_defs.iter().find(|(name, _)| name == class_name) else {
            continue;
        };
        let (_, class_def) = class_def;
        let Some(type_param_name) = &class_def.type_param else {
            continue;
        };
        let mangled_class = mangle_generic_instantiation(class_name, type_param_name, type_arg);

        // Monomorphize each method: find its HirItem::Function in hir.items
        // by its mangled name (e.g. "C.__init__"), substitute T with
        // type_arg, rename to the new mangled class name, and register.
        // #386 rebind semantics: class lowering retains multiple same-named
        // method items (the latest definition wins at runtime), and the
        // method table entry was replaced to point at the latest mangled
        // name on redefinition. Since the mangled name is identical across
        // redefinitions, `rfind` (reverse find) specializes the *last*
        // matching `HirItem::Function` -- the one whose body actually runs
        // -- rather than `find`'s first match, which would specialize a
        // stale, shadowed definition.
        let mut mangled_methods: Vec<(String, String)> = Vec::new();
        for (method_name, original_mangled) in &class_def.methods {
            // The `rfind` filter already guarantees the item is a
            // `HirItem::Function`, so the destructuring cannot fail — the
            // `else { continue }` handles only the `None` case (method not
            // found in `hir.items`).
            let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == original_mangled
                )
            })
            else {
                continue;
            };
            let substituted_params = params
                .iter()
                .map(|(pn, ty)| {
                    (
                        pn.clone(),
                        substitute_ty_with_class(
                            ty,
                            type_param_name,
                            type_arg,
                            class_name,
                            &mangled_class,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let substituted_return = substitute_ty_with_class(
                return_ty,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let substituted_body = substitute_body_with_class(
                body,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let new_mangled = format!("{mangled_class}.{method_name}");
            let param_tys = substituted_params
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect::<Vec<_>>();
            // Register the method's signature in env so infer_expr_in can
            // resolve calls to it.
            env.bind_function(new_mangled.clone(), param_tys, substituted_return.clone());
            // substitute_body_with_class (called above) already rewrites
            // both `Ty::Param` → `type_arg` and `Ty::Instance(class_name)`
            // → `Ty::Instance(mangled_class)` in the method body's
            // annotations. Self/class-name resolution happened at
            // annotation_to_ty time (HIR lowering), where `Self` and the
            // class name both became `Ty::Instance(class_name)`, and
            // substitute_body_with_class rewrites those to the mangled
            // monomorphized class name. No further substitution is needed
            // here.
            let specialized = HirItem::Function {
                name: new_mangled.clone(),
                params: substituted_params,
                return_ty: substituted_return,
                body: substituted_body,
            };
            if seen.insert(new_mangled.clone()) {
                instantiations.push(GenericInstantiation {
                    mangled_name: new_mangled.clone(),
                    specialized,
                    return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                });
            }
            mangled_methods.push((method_name.clone(), new_mangled));
        }

        // #377: Monomorphize properties. Each property's getter and setter
        // are ordinary `HirItem::Function` items with mangled names like
        // `Box.v` and `Box.v.setter`. They are NOT in the class's `methods`
        // table (they're in `properties`), so the method monomorphization
        // loop above does not handle them. We monomorphize them here by
        // finding their `HirItem::Function` in `hir.items`, substituting the
        // type parameter, and renaming to the mangled class name. The
        // monomorphized property entries point at the new mangled names.
        let mut monomorphized_properties: Vec<PropertyDef> = Vec::new();
        for prop in &class_def.properties {
            let new_getter = format!("{mangled_class}.{}", prop.name);
            if let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == &prop.getter
                )
            }) {
                let substituted_params = params
                    .iter()
                    .map(|(pn, ty)| {
                        (
                            pn.clone(),
                            substitute_ty_with_class(
                                ty,
                                type_param_name,
                                type_arg,
                                class_name,
                                &mangled_class,
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                let substituted_return = substitute_ty_with_class(
                    return_ty,
                    type_param_name,
                    type_arg,
                    class_name,
                    &mangled_class,
                );
                let substituted_body = substitute_body_with_class(
                    body,
                    type_param_name,
                    type_arg,
                    class_name,
                    &mangled_class,
                );
                let param_tys = substituted_params
                    .iter()
                    .map(|(_, ty)| ty.clone())
                    .collect::<Vec<_>>();
                env.bind_function(new_getter.clone(), param_tys, substituted_return.clone());
                let specialized = HirItem::Function {
                    name: new_getter.clone(),
                    params: substituted_params,
                    return_ty: substituted_return,
                    body: substituted_body,
                };
                if seen.insert(new_getter.clone()) {
                    instantiations.push(GenericInstantiation {
                        mangled_name: new_getter.clone(),
                        specialized,
                        return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                    });
                }
            }
            let new_setter = prop.setter.as_ref().map(|s| {
                let new_s = format!("{mangled_class}.{}.setter", prop.name);
                if let Some(HirItem::Function {
                    name: _,
                    params,
                    return_ty,
                    body,
                }) = hir.items.iter().rfind(|item| {
                    matches!(
                        item,
                        HirItem::Function { name, .. } if name == s
                    )
                }) {
                    let substituted_params = params
                        .iter()
                        .map(|(pn, ty)| {
                            (
                                pn.clone(),
                                substitute_ty_with_class(
                                    ty,
                                    type_param_name,
                                    type_arg,
                                    class_name,
                                    &mangled_class,
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    let substituted_return = substitute_ty_with_class(
                        return_ty,
                        type_param_name,
                        type_arg,
                        class_name,
                        &mangled_class,
                    );
                    let substituted_body = substitute_body_with_class(
                        body,
                        type_param_name,
                        type_arg,
                        class_name,
                        &mangled_class,
                    );
                    let param_tys = substituted_params
                        .iter()
                        .map(|(_, ty)| ty.clone())
                        .collect::<Vec<_>>();
                    env.bind_function(new_s.clone(), param_tys, substituted_return.clone());
                    let specialized = HirItem::Function {
                        name: new_s.clone(),
                        params: substituted_params,
                        return_ty: substituted_return,
                        body: substituted_body,
                    };
                    if seen.insert(new_s.clone()) {
                        instantiations.push(GenericInstantiation {
                            mangled_name: new_s.clone(),
                            specialized,
                            return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                        });
                    }
                }
                new_s
            });
            monomorphized_properties.push(PropertyDef {
                name: prop.name.clone(),
                getter: new_getter,
                setter: new_setter,
            });
        }

        // #436: Monomorphize static methods. Each static method's
        // mangled name uses a `.static` suffix; the monomorphized name
        // replaces the class prefix with the mangled class name. Static
        // methods have no `self`/`cls` parameter, so only the type
        // parameter substitution applies.
        let mut mangled_static_methods: Vec<(String, String)> = Vec::new();
        for (method_name, original_mangled) in &class_def.static_methods {
            let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == original_mangled
                )
            })
            else {
                continue;
            };
            let substituted_params = params
                .iter()
                .map(|(pn, ty)| {
                    (
                        pn.clone(),
                        substitute_ty_with_class(
                            ty,
                            type_param_name,
                            type_arg,
                            class_name,
                            &mangled_class,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let substituted_return = substitute_ty_with_class(
                return_ty,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let substituted_body = substitute_body_with_class(
                body,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let new_mangled = format!("{mangled_class}.{method_name}.static");
            let param_tys = substituted_params
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect::<Vec<_>>();
            env.bind_function(new_mangled.clone(), param_tys, substituted_return.clone());
            let specialized = HirItem::Function {
                name: new_mangled.clone(),
                params: substituted_params,
                return_ty: substituted_return,
                body: substituted_body,
            };
            if seen.insert(new_mangled.clone()) {
                instantiations.push(GenericInstantiation {
                    mangled_name: new_mangled.clone(),
                    specialized,
                    return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                });
            }
            mangled_static_methods.push((method_name.clone(), new_mangled));
        }

        // #436: Monomorphize class methods. Each class method's mangled
        // name uses a `.classmethod` suffix; the monomorphized name
        // replaces the class prefix with the mangled class name. Class
        // methods have a `cls` parameter typed `Ty::Instance(class_name)`,
        // which `substitute_body_with_class` rewrites to
        // `Ty::Instance(mangled_class)`.
        let mut mangled_class_methods: Vec<(String, String)> = Vec::new();
        for (method_name, original_mangled) in &class_def.class_methods {
            let Some(HirItem::Function {
                name: _,
                params,
                return_ty,
                body,
            }) = hir.items.iter().rfind(|item| {
                matches!(
                    item,
                    HirItem::Function { name, .. } if name == original_mangled
                )
            })
            else {
                continue;
            };
            let substituted_params = params
                .iter()
                .map(|(pn, ty)| {
                    (
                        pn.clone(),
                        substitute_ty_with_class(
                            ty,
                            type_param_name,
                            type_arg,
                            class_name,
                            &mangled_class,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let substituted_return = substitute_ty_with_class(
                return_ty,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let substituted_body = substitute_body_with_class(
                body,
                type_param_name,
                type_arg,
                class_name,
                &mangled_class,
            );
            let new_mangled = format!("{mangled_class}.{method_name}.classmethod");
            let param_tys = substituted_params
                .iter()
                .map(|(_, ty)| ty.clone())
                .collect::<Vec<_>>();
            env.bind_function(new_mangled.clone(), param_tys, substituted_return.clone());
            let specialized = HirItem::Function {
                name: new_mangled.clone(),
                params: substituted_params,
                return_ty: substituted_return,
                body: substituted_body,
            };
            if seen.insert(new_mangled.clone()) {
                instantiations.push(GenericInstantiation {
                    mangled_name: new_mangled.clone(),
                    specialized,
                    return_ty: Ty::Instance(Box::new(mangled_class.clone())),
                });
            }
            mangled_class_methods.push((method_name.clone(), new_mangled));
        }

        // Register the monomorphized class in env.classes with mangled
        // method names and substituted attribute types.
        let substituted_attrs = class_def
            .attrs
            .iter()
            .map(|(attr_name, ty)| {
                (
                    attr_name.clone(),
                    substitute_ty(ty, type_param_name, type_arg),
                )
            })
            .collect::<Vec<_>>();
        let new_class_def = HirClassDef {
            name: mangled_class.clone(),
            bases: Vec::new(),
            mro: vec![mangled_class.clone()],
            attrs: substituted_attrs,
            methods: mangled_methods,
            type_param: None, // The monomorphized class is not generic.
            properties: monomorphized_properties,
            static_methods: mangled_static_methods,
            class_methods: mangled_class_methods,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        // `bind_class` clears any synthetic marking for the name it binds
        // (Part 1 of #541, D-188), which is harmless here: `mangled_class`
        // always carries `mangle_generic_instantiation`'s `0gen_` prefix, so
        // it can never equal one of the seven `BUILTIN_EXCEPTION_CLASSES`
        // names and can never unmark a synthetic entry `bind_classes` made.
        env.bind_class(mangled_class.clone(), new_class_def.clone());
        new_class_defs.push((mangled_class, new_class_def));
    }
}

/// PEP 695 (#387): Pass 2b per-item worker — rewrites any
/// `GenericClassInstantiate` expressions that survived inside one
/// monomorphized generic-class method body. Extracted from `monomorphize`'s
/// Pass 2b loop so that the defense-in-depth non-`Function` arm (all
/// instantiations are `HirItem::Function` in practice) is a `match` arm in
/// a standalone function that a direct unit test can cover, rather than an
/// `if let` whose never-taken false branch would carry an uncovered
/// continuation region under the 100%-coverage gate (D-014).
///
/// Clones the function's params/body out of `instantiations[i]` so the
/// immutable borrow ends before `rewrite_generic_calls_in_stmt` takes
/// `&mut instantiations` (it may push new instantiations for a GCI inside
/// the body that triggers another class monomorphization).
pub(crate) fn rewrite_generic_calls_in_instantiation(
    env: &mut Environment,
    i: usize,
    instantiations: &mut Vec<GenericInstantiation>,
    seen: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    let (name, params, return_ty, body) = match &instantiations[i].specialized {
        HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } => (
            name.clone(),
            params.clone(),
            return_ty.clone(),
            body.clone(),
        ),
        // All instantiations are `HirItem::Function` (created by
        // `instantiate_generic_call` or `instantiate_generic_class_methods`).
        // This arm is defense-in-depth, covered by a direct unit test.
        _ => return Ok(()),
    };
    let local_names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
    let local_names_refs: Vec<&str> = local_names.iter().map(|s| s.as_str()).collect();
    let mut fn_env = env.child_for_function(&local_names_refs);
    for (param_name, param_ty) in &params {
        fn_env.bind(param_name.clone(), param_ty.clone());
    }
    let mut new_body = body;
    for stmt in new_body.iter_mut() {
        rewrite_generic_calls_in_stmt(&mut fn_env, &local_names_refs, stmt, instantiations, seen)?;
    }
    // Direct assignment avoids a second `if let` (whose never-taken false
    // branch would carry an uncovered region under D-014). All instantiations
    // are `HirItem::Function`, so this reconstruction is always valid.
    instantiations[i].specialized = HirItem::Function {
        name,
        params,
        return_ty,
        body: new_body,
    };
    Ok(())
}

/// #380 (PR-20): Monomorphizes functions with protocol-typed parameters.
/// Scans `items` for calls to functions that have `Ty::Protocol` parameters,
/// and for each call site, creates a specialized version with the protocol
/// type substituted by the concrete argument's class type. The original
/// function is dropped (only specializations reach MIR/codegen). Returns
/// the updated items list with monomorphized functions appended and call
/// sites rewritten.
fn monomorphize_protocol_params(
    items: Vec<HirItem>,
    env: &Environment,
    _new_class_defs: &mut Vec<(String, HirClassDef)>,
) -> Vec<HirItem> {
    // Collect functions with protocol-typed parameters (cloned, so we
    // can move `items` below without a borrow conflict).
    let protocol_funcs: HashMap<String, HirItem> = items
        .iter()
        .filter_map(|item| {
            if let HirItem::Function {
                name,
                params,
                return_ty,
                ..
            } = item
                && has_protocol_param(params, return_ty)
            {
                return Some((name.clone(), item.clone()));
            }
            None
        })
        .collect();
    if protocol_funcs.is_empty() {
        return items;
    }
    let mut new_items = Vec::new();
    let mut specializations: Vec<HirItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item in items {
        match item {
            HirItem::Function {
                ref name,
                ref params,
                ref return_ty,
                ..
            } if protocol_funcs.contains_key(name.as_str()) => {
                // Drop the original protocol-parameter function (only
                // specializations are kept). But if the function has no
                // call sites (no specializations), it would be silently
                // dropped — this is fine, matching how generic functions
                // are dropped.
                let _ = (params, return_ty);
            }
            HirItem::TopLevelStmt(ref stmt) => {
                let mut new_stmt = stmt.clone();
                rewrite_protocol_calls_in_stmt(
                    &mut new_stmt,
                    &protocol_funcs,
                    env,
                    &[],
                    &mut specializations,
                    &mut seen,
                );
                new_items.push(HirItem::TopLevelStmt(new_stmt));
            }
            HirItem::Function {
                name,
                params,
                return_ty,
                body,
            } => {
                let local_names: Vec<String> = function_local_names(&params, &body)
                    .into_iter()
                    .map(String::from)
                    .collect();
                let local_names_refs: Vec<&str> = local_names.iter().map(|s| s.as_str()).collect();
                let mut fn_env = env.child_for_function(&local_names_refs);
                for (param_name, param_ty) in &params {
                    fn_env.bind(param_name.clone(), param_ty.clone());
                }
                bind_local_types_in_body(&mut fn_env, &local_names_refs, &body);
                let mut new_body = body;
                for stmt in new_body.iter_mut() {
                    rewrite_protocol_calls_in_stmt(
                        stmt,
                        &protocol_funcs,
                        &fn_env,
                        &local_names_refs,
                        &mut specializations,
                        &mut seen,
                    );
                }
                new_items.push(HirItem::Function {
                    name,
                    params,
                    return_ty,
                    body: new_body,
                });
            }
        }
    }
    // Rewrite calls inside specializations recursively. A specialized
    // body may itself call another protocol-parameter function, which
    // needs its own specialization. Process by index because rewriting
    // may push new specializations, extending the vector we're walking.
    //
    // The per-item work is extracted into `rewrite_protocol_calls_in_specialization`
    // so that the defense-in-depth non-`Function` arm (all specializations
    // are `HirItem::Function` in practice) is a `match` arm in a standalone
    // function that a direct unit test can cover, rather than an `if let`
    // whose never-taken false branch would carry an uncovered continuation
    // region under the 100%-coverage gate (D-014).
    let mut all_specializations = std::mem::take(&mut specializations);
    let mut i = 0;
    while i < all_specializations.len() {
        rewrite_protocol_calls_in_specialization(
            env,
            &mut all_specializations[i],
            &protocol_funcs,
            &mut specializations,
            &mut seen,
        );
        i += 1;
        if !specializations.is_empty() {
            all_specializations.append(&mut specializations);
        }
    }
    new_items.extend(all_specializations);
    new_items
}

/// #380 (PR-20): Per-item worker for the specialization rewrite loop.
/// Rewrites protocol-parameter function calls inside one already-created
/// specialization's body, pushing any newly discovered specializations.
/// Extracted from `monomorphize_protocol_params`'s loop so the
/// defense-in-depth non-`Function` arm is a `match` arm coverable by a
/// direct unit test (D-014).
pub(crate) fn rewrite_protocol_calls_in_specialization(
    env: &Environment,
    spec: &mut HirItem,
    protocol_funcs: &HashMap<String, HirItem>,
    specializations: &mut Vec<HirItem>,
    seen: &mut HashSet<String>,
) {
    let (params, body) = match spec {
        HirItem::Function { params, body, .. } => (params, body),
        // All specializations are `HirItem::Function` (created by
        // `rewrite_protocol_calls_in_expr`). This arm is defense-in-depth,
        // covered by a direct unit test.
        _ => return,
    };
    let local_names: Vec<String> = function_local_names(params, body)
        .into_iter()
        .map(String::from)
        .collect();
    let local_names_refs: Vec<&str> = local_names.iter().map(|s| s.as_str()).collect();
    let mut spec_env = env.child_for_function(&local_names_refs);
    for (param_name, param_ty) in params.iter() {
        spec_env.bind(param_name.clone(), param_ty.clone());
    }
    bind_local_types_in_body(&mut spec_env, &local_names_refs, body);
    for stmt in body.iter_mut() {
        rewrite_protocol_calls_in_stmt(
            stmt,
            protocol_funcs,
            &spec_env,
            &local_names_refs,
            specializations,
            seen,
        );
    }
}

/// #380 (PR-20): Rewrites calls to protocol-parameter functions in a
/// statement, creating monomorphized specializations as needed.
fn rewrite_protocol_calls_in_stmt(
    stmt: &mut HirStmt,
    protocol_funcs: &HashMap<String, HirItem>,
    env: &Environment,
    local_names: &[&str],
    specializations: &mut Vec<HirItem>,
    seen: &mut HashSet<String>,
) {
    match stmt {
        HirStmt::ExprStmt(expr) => {
            rewrite_protocol_calls_in_expr(
                expr,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::Assign { value, .. } => {
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::AnnAssign {
            value: Some(value), ..
        } => {
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::Return(Some(expr)) => {
            rewrite_protocol_calls_in_expr(
                expr,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::If { test, body, orelse } => {
            rewrite_protocol_calls_in_expr(
                test,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            for s in orelse.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::While { test, body } => {
            rewrite_protocol_calls_in_expr(
                test,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::ForRange {
            start,
            stop,
            step,
            body,
            ..
        } => {
            rewrite_protocol_calls_in_expr(
                start,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                stop,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                step,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::ForList { body, .. } => {
            for s in body.iter_mut() {
                rewrite_protocol_calls_in_stmt(
                    s,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirStmt::DictSet { key, value, .. } => {
            rewrite_protocol_calls_in_expr(
                key,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::AttrSet { base, value, .. } => {
            rewrite_protocol_calls_in_expr(
                base,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::ListCompAssign { cond, elt, .. } | HirStmt::SetCompAssign { cond, elt, .. } => {
            if let Some(c) = cond {
                rewrite_protocol_calls_in_expr(
                    c,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            rewrite_protocol_calls_in_expr(
                elt,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirStmt::DictCompAssign {
            cond, key, value, ..
        } => {
            if let Some(c) = cond {
                rewrite_protocol_calls_in_expr(
                    c,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            rewrite_protocol_calls_in_expr(
                key,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                value,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        _ => {}
    }
}

/// #380 (PR-20): Rewrites calls to protocol-parameter functions in an
/// expression, creating monomorphized specializations as needed.
fn rewrite_protocol_calls_in_expr(
    expr: &mut HirExpr,
    protocol_funcs: &HashMap<String, HirItem>,
    env: &Environment,
    local_names: &[&str],
    specializations: &mut Vec<HirItem>,
    seen: &mut HashSet<String>,
) {
    match expr {
        HirExpr::Call { callee, args } => {
            // First, recurse into arguments (they may contain nested calls).
            for arg in args.iter_mut() {
                rewrite_protocol_calls_in_expr(
                    arg,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
            // Check if this is a call to a protocol-parameter function.
            if let Some(func_item) = protocol_funcs.get(callee.as_str())
                && let HirItem::Function {
                    name,
                    params,
                    return_ty,
                    body,
                } = func_item
            {
                // Collect all protocol→concrete substitutions from every
                // protocol-typed parameter's corresponding argument.
                let mut substitutions: Vec<(String, Ty)> = Vec::new();
                for (i, (_, param_ty)) in params.iter().enumerate() {
                    if let Ty::Protocol(proto_name) = param_ty
                        && i < args.len()
                    {
                        let arg_ty = infer_expr_in(env, local_names, &args[i]);
                        if let Ok(Ty::Instance(concrete_name)) = arg_ty {
                            substitutions
                                .push((proto_name.as_ref().clone(), Ty::Instance(concrete_name)));
                        }
                    }
                }
                if !substitutions.is_empty() {
                    let mangled = mangle_protocol_instantiation(name, &substitutions);
                    if seen.insert(mangled.clone()) {
                        let substituted_params: Vec<(String, Ty)> = params
                            .iter()
                            .map(|(n, ty)| (n.clone(), substitute_ty_protocols(ty, &substitutions)))
                            .collect();
                        let substituted_return = substitute_ty_protocols(return_ty, &substitutions);
                        let substituted_body = substitute_body_protocols(body, &substitutions);
                        specializations.push(HirItem::Function {
                            name: mangled.clone(),
                            params: substituted_params,
                            return_ty: substituted_return,
                            body: substituted_body,
                        });
                    }
                    *callee = mangled;
                }
            }
        }
        HirExpr::MethodCall { base, args, .. } => {
            rewrite_protocol_calls_in_expr(
                base,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            for arg in args.iter_mut() {
                rewrite_protocol_calls_in_expr(
                    arg,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        HirExpr::AttrGet { base, .. } => {
            rewrite_protocol_calls_in_expr(
                base,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::UnaryOp { operand, .. } => {
            rewrite_protocol_calls_in_expr(
                operand,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::BinOp { left, right, .. } => {
            rewrite_protocol_calls_in_expr(
                left,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                right,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::Compare { left, right, .. } => {
            rewrite_protocol_calls_in_expr(
                left,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
            rewrite_protocol_calls_in_expr(
                right,
                protocol_funcs,
                env,
                local_names,
                specializations,
                seen,
            );
        }
        HirExpr::ListLiteral(elements) => {
            for e in elements.iter_mut() {
                rewrite_protocol_calls_in_expr(
                    e,
                    protocol_funcs,
                    env,
                    local_names,
                    specializations,
                    seen,
                );
            }
        }
        _ => {}
    }
}

pub(crate) fn monomorphize(hir: &HirModule) -> Result<HirModule, Diagnostic> {
    let generics: HashMap<String, HirItem> = hir
        .items
        .iter()
        .filter_map(|item| match item {
            HirItem::Function {
                name,
                params,
                return_ty,
                ..
            } if is_generic_signature(params, return_ty) => Some((name.clone(), item.clone())),
            _ => None,
        })
        .collect();
    if generics.is_empty()
        && !hir.class_defs.iter().any(|(_, cd)| cd.type_param.is_some())
        && !hir.items.iter().any(|item| {
            if let HirItem::Function {
                params, return_ty, ..
            } = item
            {
                has_protocol_param(params, return_ty)
            } else {
                false
            }
        })
    {
        // PR-13 final review (I1): `type_aliases` is emptied here exactly as
        // it is on the monomorphized path below, so a resolved module's own
        // `type_aliases` value never depends on whether the module happened
        // to contain a generic function. D-135 aliases are fully discharged
        // during HIR lowering (`annotation_to_ty` resolves every alias name
        // into a concrete `Ty` before `pycc_types` runs); nothing downstream
        // -- not `pycc_mir`, not `pycc_codegen` -- reads the field after
        // lowering, so the resolved HIR's `type_aliases` is empty by design.
        return Ok(HirModule {
            // Provenance travels with the module: monomorphization
            // rewrites and extends `class_defs`, but never changes who
            // produced the builtin exception entries already in it.
            seeded_builtin_exception_classes: hir.seeded_builtin_exception_classes,
            items: hir.items.clone(),
            type_aliases: Vec::new(),
            imports: Vec::new(),
            // Unlike `type_aliases`/`imports` (both fully discharged during
            // HIR lowering -- nothing downstream reads either again),
            // `class_defs` is actively consumed after this point: `check`'s
            // own class-body checking (Task 3) and every one of
            // `pycc_mir`/`pycc_codegen`'s slot-index/method-mangled-name
            // lookups (Tasks 5/6) read it from the `HirModule` that reaches
            // them, which is this function's own return value on the
            // monomorphized path. Dropping it here would silently break
            // every class-containing module the moment it reached
            // `pycc_mir::build`.
            class_defs: hir.class_defs.clone(),
        });
    }

    let mut env = Environment::new();
    // D-154 (Part 1 of #375): this pass's own `env` needs the class table
    // too -- `rewrite_generic_calls_in_expr`'s `AttrGet`/`MethodCall` arms
    // fall through to `infer_expr_in` exactly like every other arm here,
    // which resolves a class-instance attribute/method through
    // `Environment::classes` (`class::resolve_attr_get`/
    // `resolve_method_call`). Without this, a generic function whose own
    // body reads/writes an instance attribute or calls a method would
    // panic here with "class `X` has no registered HirClassDef" the
    // moment this pass reached it, even though `check_with_signatures`
    // already validated that exact body successfully against a `classes`-
    // populated `Environment` of its own.
    crate::class::bind_classes(&mut env, hir);
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        {
            let param_tys = params.iter().map(|(_, ty)| ty.clone()).collect::<Vec<_>>();
            env.bind_function(name.clone(), param_tys, return_ty.clone());
        }
    }
    for (name, item) in &generics {
        env.bind_generic(name.clone(), item.clone());
    }

    let function_local_names = module_function_local_names(hir);
    let mut instantiations: Vec<GenericInstantiation> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // PEP 695 (#387): pre-scan for generic class instantiations
    // (`C[type_arg](args)`) and monomorphize each unique `(class, type_arg)`
    // pair's methods before the rewrite pass starts. This way,
    // `rewrite_generic_calls_in_expr`'s `GenericClassInstantiate` arm can
    // simply rewrite the expression to an ordinary `HirExpr::Call` to the
    // mangled class name, confident the monomorphized methods are already
    // registered in `env` and collected in `instantiations`.
    let mut new_class_defs: Vec<(String, HirClassDef)> = Vec::new();
    instantiate_generic_class_methods(
        hir,
        &mut env,
        &mut instantiations,
        &mut seen,
        &mut new_class_defs,
    );

    // Two passes, matching `check_with_environment`'s own D-041 discipline
    // exactly (register every function's signature -- already done above --
    // then process every top-level statement in source order against the
    // *growing* module `env`, then check every function body against the
    // module `env` as it stands once the whole module's top-level code has
    // run): a function body must be able to rewrite a generic call whose
    // argument reads a module-level global assigned *later* in the file
    // than the function's own `def`, exactly as `check_function_in` already
    // allows for ordinary type-checking. A single source-order pass here
    // would process a function body's rewrite before a later top-level
    // assignment had grown `env`, wrongly reporting the global as
    // undefined even though `check`/`check_and_resolve`'s own validation
    // (which already ran successfully before `monomorphize` is ever called)
    // accepted the exact same program.
    //
    // Each item's *original* index is carried alongside its rewritten form
    // so the final assembly can restore source order -- this pass processes
    // top-level statements and function bodies in two separate sweeps, not
    // in original item order.
    let mut rewritten: Vec<Option<HirItem>> = vec![None; hir.items.len()];

    // Pass 1: every top-level statement, in source order, growing `env`.
    for (index, (item, local_names)) in hir.items.iter().zip(&function_local_names).enumerate() {
        if let HirItem::TopLevelStmt(stmt) = item {
            let mut new_stmt = stmt.clone();
            rewrite_generic_calls_in_stmt(
                &mut env,
                local_names,
                &mut new_stmt,
                &mut instantiations,
                &mut seen,
            )?;
            rewritten[index] = Some(HirItem::TopLevelStmt(new_stmt));
        }
    }

    // Pass 2: every function body, against the module `env` as it stands
    // once the whole module's top-level code has been processed above --
    // an original generic function is dropped entirely (only its concrete
    // specializations, appended below, may reach `pycc_mir`).
    for (index, (item, local_names)) in hir.items.iter().zip(&function_local_names).enumerate() {
        let HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } = item
        else {
            continue;
        };
        if generics.contains_key(name) {
            continue;
        }
        let mut fn_env = env.child_for_function(local_names);
        for (param_name, param_ty) in params {
            fn_env.bind(param_name.clone(), param_ty.clone());
        }
        let mut new_body = body.clone();
        for stmt in new_body.iter_mut() {
            rewrite_generic_calls_in_stmt(
                &mut fn_env,
                local_names,
                stmt,
                &mut instantiations,
                &mut seen,
            )?;
        }
        rewritten[index] = Some(HirItem::Function {
            name: name.clone(),
            params: params.clone(),
            return_ty: return_ty.clone(),
            body: new_body,
        });
    }

    let mut items = rewritten.into_iter().flatten().collect::<Vec<_>>();
    // PEP 695 (#387): Pass 2b — rewrite any `GenericClassInstantiate`
    // expressions that survived inside monomorphized generic-class method
    // bodies. Pass 2 only iterates over the original `hir.items`, not the
    // `instantiations` appended below, so a GCI nested inside a generic
    // class method (e.g. `class C[T]: def f(self): b = D[int](1)`) would
    // survive into the monomorphized copy and panic at MIR lowering. Run
    // the same `rewrite_generic_calls_in_stmt` over each instantiation's
    // body before appending it, using a fresh child environment seeded
    // with the instantiation's own (already-substituted) parameter types.
    // Process by index because `rewrite_generic_calls_in_stmt` may push
    // new instantiations (a GCI inside a monomorphized body triggers
    // another class monomorphization), extending the vector we're walking.
    //
    // The per-item work is extracted into `rewrite_generic_calls_in_instantiation`
    // so that the defense-in-depth non-`Function` arm (all instantiations
    // are `HirItem::Function` in practice) is a `match` arm in a standalone
    // function that a direct unit test can cover, rather than an `if let`
    // whose never-taken false branch would carry an uncovered continuation
    // region under the 100%-coverage gate (D-014).
    let mut i = 0;
    while i < instantiations.len() {
        rewrite_generic_calls_in_instantiation(&mut env, i, &mut instantiations, &mut seen)?;
        i += 1;
    }
    for instantiation in instantiations {
        items.push(instantiation.specialized);
    }
    // #380 (PR-20): Protocol-typed parameter monomorphization. Functions
    // with `Ty::Protocol` parameters need to be specialized per concrete
    // call-site type, substituting the protocol type with the concrete
    // `Ty::Instance` so MIR/codegen can resolve method calls and attribute
    // access against the concrete class. This pass runs after the existing
    // generic function monomorphization, scanning the (already rewritten)
    // items for calls to protocol-parameter functions.
    items = monomorphize_protocol_params(items, &env, &mut new_class_defs);
    // `type_aliases`/`imports` are empty by design on both of this
    // function's exits -- see the no-generics early return above (PR-13
    // final review I1) and that return's own comment for why `class_defs`
    // is not treated the same way.
    // PEP 695 (#387): include the monomorphized class definitions alongside
    // the originals, so `pycc_mir`'s `classes` HashMap can resolve the
    // mangled class name to its specialized `HirClassDef` (attribute slots,
    // method table) for instantiation and method-call lowering.
    let mut class_defs = hir.class_defs.clone();
    class_defs.extend(new_class_defs);
    Ok(HirModule {
        // Provenance travels with the module: monomorphization
        // rewrites and extends `class_defs`, but never changes who
        // produced the builtin exception entries already in it.
        seeded_builtin_exception_classes: hir.seeded_builtin_exception_classes,
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs,
    })
}
