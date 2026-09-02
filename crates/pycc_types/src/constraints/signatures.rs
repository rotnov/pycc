//! Module-level signature entry points of the constraint solver: the
//! fully-annotated fast path (`concrete_function_signatures`,
//! `concrete_function_environment`) and the solver walk that infers every
//! private-helper signature (`infer_function_signatures_with_solver`).
//!
//! Extracted from `constraints.rs` per AGENTS.md's file-decomposition rule
//! (issue #868, tracked by #544), laid out beside it the way `class.rs` and
//! `class/` already are. `constraints.rs` re-exports everything here with
//! `pub(crate) use signatures::*`, so every crate-root path is unchanged.
//! The driver that sequences these entry points (`checked_function_signatures`
//! and the public `check*` functions) lives in [`crate::module`].

use super::*;

pub(crate) fn concrete_function_signatures(
    hir: &HirModule,
) -> Option<HashMap<String, (Vec<Ty>, Ty)>> {
    let mut signatures = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if *return_ty == Ty::Infer || params.iter().any(|(_, ty)| *ty == Ty::Infer) {
            return None;
        }
        signatures.insert(
            name.clone(),
            (
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_ty.clone(),
            ),
        );
    }
    Some(signatures)
}

/// Builds the function registry for a fully annotated module directly from
/// HIR. Unlike [`concrete_function_signatures`] followed by
/// [`check_with_signatures`], this creates each owned name and parameter vector
/// only once. `check` does not need to materialize a second signature map for a
/// downstream consumer, so its overwhelmingly common concrete, valid path can
/// validate with this registry directly.
pub(crate) fn concrete_function_environment(hir: &HirModule) -> Option<Environment> {
    let mut functions = HashMap::new();
    let mut generics = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if *return_ty == Ty::Infer || params.iter().any(|(_, ty)| *ty == Ty::Infer) {
            return None;
        }
        if is_generic_signature(params, return_ty) {
            generics.insert(name.clone(), item.clone());
        }
        functions.insert(
            name.clone(),
            (
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_ty.clone(),
            ),
        );
    }
    let mut env = Environment {
        bindings: HashMap::new(),
        declared: HashMap::new(),
        functions: Arc::new(functions),
        def_rebound: HashSet::new(),
        defined_functions: HashSet::new(),
        generics: Arc::new(generics),
        classes: Arc::new(HashMap::new()),
        synthetic_classes: Arc::new(HashSet::new()),
        own_type_param: None,
        current_class: None,
        finals: HashSet::new(),
        in_except_handler: false,
        narrowed: HashMap::new(),
    };
    // Part 1 of #541: register the class table through `bind_class` (via
    // `bind_classes`) rather than by populating `classes` directly, so this
    // second `Environment` constructor cannot drift from the first on which
    // entries are marked synthetic. `bind_class` and `bind_synthetic_class`
    // are together the sole mutators of both tables precisely so that
    // invariant holds by construction.
    crate::class::bind_classes(&mut env, hir);
    Some(env)
}

pub(crate) fn infer_function_signatures_with_solver(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
    let mut parents = Vec::new();
    let mut concrete = Vec::new();
    let mut signatures = HashMap::new();
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        {
            signatures.insert(
                name.clone(),
                (
                    params.iter().map(|(name, _)| name.clone()).collect(),
                    params
                        .iter()
                        .map(|(_, ty)| term_for_type(ty.clone(), &mut parents, &mut concrete))
                        .collect(),
                    term_for_type(return_ty.clone(), &mut parents, &mut concrete),
                ),
            );
        }
    }

    let mut constraints = SolverConstraints::default();
    let mut globals = ConstraintEnvironment {
        bindings: HashMap::new(),
        local_names: &[],
        defs_rebound: HashSet::new(),
        maybe_bindings: HashSet::new(),
        opaque_bindings: HashSet::new(),
    };
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                collect_block_constraints(
                    &signatures,
                    &mut parents,
                    &mut concrete,
                    &mut constraints,
                    &mut globals,
                    std::slice::from_ref(stmt),
                    None,
                )?;
            }
            // Mirror of pass 2's source-order `def` rebinding (D-110): the
            // `def` marks the name def-rebound in the accumulated globals
            // (without erasing its term, which representation tracking may
            // still need), so helper-body environments seeded from them see
            // the net binding, not a stale shadowed primitive.
            HirItem::Function { name, .. } => {
                globals.defs_rebound.insert(name.clone());
            }
        }
    }
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        let HirItem::Function {
            name, body, params, ..
        } = item
        else {
            continue;
        };
        let signature = &signatures[name];
        let mut env = ConstraintEnvironment {
            bindings: globals.bindings.clone(),
            local_names,
            defs_rebound: globals.defs_rebound.clone(),
            maybe_bindings: globals.maybe_bindings.clone(),
            opaque_bindings: globals.opaque_bindings.clone(),
        };
        for local_name in local_names.iter().copied() {
            env.bindings.remove(local_name);
            // A local name (parameter or body-assigned) re-binds within this
            // body, so a stale module-level def-rebound fact must not
            // survive for it (D-110, PR #252's round-6 review): a parameter
            // colliding with a def-rebound module name would otherwise skip
            // the mirror gate and be mislabeled "not bound before this use".
            env.defs_rebound.remove(local_name);
            env.maybe_bindings.remove(local_name);
            // Issue #771: same reasoning as `maybe_bindings` above — a
            // local name re-binds within this function body, so a stale
            // module-level opaque marker must not survive for it either.
            env.opaque_bindings.remove(local_name);
        }
        // Use the current item's own parameter names, not the last-inserted
        // signature's names (#386): a redefined method shares its mangled
        // name but has its own parameter names, and checking its body against
        // the wrong names would report false T0021 "not bound" errors. The
        // type terms (signature.1) and return type (signature.2) come from
        // the last definition, which is correct — compatible redefinitions
        // have the same raw type shape (already validated by
        // check_incompatible_redefinitions), and the last definition is the
        // one bound at call sites.
        for (param_name, param_ty) in params.iter().map(|(n, _)| n).zip(&signature.1) {
            env.bindings.insert(param_name.clone(), param_ty.clone());
        }
        // #380 (PR-20): skip the constraint solver for abstract method
        // bodies. An abstract method's HIR body is just `Return(None)`,
        // but its declared return type may be non-`None` (e.g. `-> int`).
        // Running the solver on it would unify `None` with the declared
        // type and produce a spurious `T0022`. The type checker
        // (`check_and_resolve`) also skips abstract method bodies.
        let is_abstract_method = name
            .split('.')
            .next()
            .filter(|class_name| *class_name != name)
            .and_then(|class_name| {
                hir.class_defs
                    .iter()
                    .find(|(n, _)| n == class_name)
                    .map(|(_, cd)| cd)
            })
            .is_some_and(|class_def| {
                let method_name = name.split('.').nth(1).unwrap_or("");
                class_def.abstract_methods.iter().any(|m| m == method_name)
            });
        if is_abstract_method {
            continue;
        }
        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            body,
            Some(signature.2.clone()),
        )?;
        if signature.2.is_err() && !contains_return(body) {
            unify_terms(
                signature.2.clone(),
                Ok(Ty::None),
                &mut parents,
                &mut concrete,
                "T0022",
                "private helper implicit return",
            )?;
        }
    }

    // Annotation bounds are directional defaults, not hard equalities. Let
    // every call/operator fact settle first, aggregate all remaining bounds
    // per union-find root, then propagate any selected fallback back through
    // operators. This keeps inference independent of body/declaration order.
    propagate_binop_constraints(&constraints.binops, &mut parents, &mut concrete)?;
    apply_annotation_defaults(
        &constraints.annotation_defaults,
        &mut parents,
        &mut concrete,
    )?;
    propagate_binop_constraints(&constraints.binops, &mut parents, &mut concrete)?;

    let non_scalar_local_roots = constraints
        .non_scalar_local_terms
        .iter()
        .map(|&var| root(&mut parents, var))
        .collect::<HashSet<_>>();

    let mut resolved = HashMap::new();
    for (name, signature) in &signatures {
        let param_tys = signature
            .0
            .iter()
            .zip(signature.1.iter().cloned())
            .map(|(param_name, term)| {
                resolved_private_signature_term(
                    term,
                    &mut parents,
                    &concrete,
                    &non_scalar_local_roots,
                )
                .ok_or_else(|| {
                    Diagnostic::error(
                        "T0021",
                        format!(
                            "cannot infer type of parameter `{param_name}` in private helper `{name}`; add an annotation"
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("add a type annotation to parameter `{param_name}`"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = resolved_private_signature_term(
            signature.2.clone(),
            &mut parents,
            &concrete,
            &non_scalar_local_roots,
        )
        .ok_or_else(|| {
            Diagnostic::error(
                "T0021",
                format!("cannot infer return type of private helper `{name}`; add an annotation"),
                Span::new(0, 0),
            )
            .with_help(format!("add a return type annotation to `{name}`"))
        })?;
        resolved.insert(name.clone(), (param_tys, return_ty));
    }
    Ok(resolved)
}
