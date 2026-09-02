//! Module-level check driver: the two-phase concrete/solver selection and the
//! public `check*` entry points.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #868, tracked by #544), mirroring `pycc_hir::module`: the crate root keeps
//! the annotation-driven statement and expression checker; the driver that
//! sequences the redefinition pre-checks, the concrete fast path, the solver,
//! and the signature-validated check pass lives here. `lib.rs` re-exports
//! `check`, `check_and_resolve`, and the crate-private helpers the tests and
//! `monomorphize` reach through the crate root, so no path changed.
//! `checked_function_signatures` moved here from `constraints.rs` with them:
//! it is `check_and_resolve`'s half of the same driver.

use super::*;

pub(super) fn module_function_local_names(hir: &HirModule) -> Vec<Vec<&str>> {
    hir.items
        .iter()
        .map(|item| match item {
            HirItem::Function { params, body, .. } => function_local_names(params, body),
            HirItem::TopLevelStmt(_) => Vec::new(),
        })
        .collect()
}

/// Type-checks a module and returns a cloned HIR whose function signatures
/// contain only the concrete types resolved by private-helper inference.
/// Consumers after the type boundary must use this module rather than the
/// unresolved lowering result so `Ty::Infer` can never leak into MIR or code
/// generation. PR-13 Task 3 (D-133/D-134): also monomorphizes every PEP 695
/// generic function call site into a call to a concrete, mangled
/// specialization (see `monomorphize`) -- the returned module contains only
/// ordinary concrete-`Ty` functions, exactly what `pycc_mir::build` expects.
pub fn check_and_resolve(hir: &HirModule) -> Result<HirModule, Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    let signatures = checked_function_signatures(hir, &function_local_names)?;

    let mut resolved_hir = hir.clone();
    for item in &mut resolved_hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        let (resolved_params, resolved_return) = signatures
            .get(name)
            .expect("every HIR function received an inferred signature");
        for ((_, param_ty), resolved_ty) in params.iter_mut().zip(resolved_params) {
            *param_ty = resolved_ty.clone();
        }
        *return_ty = resolved_return.clone();
    }

    // Issue #22/#402: no post-resolution redefinition recheck is needed
    // here. `checked_function_signatures` (called above) already runs
    // `check_incompatible_redefinitions` pre-resolution, and that function
    // now compares the full raw shape unconditionally, including any
    // `Ty::Infer` position (see its own doc comment) -- so every
    // redefinition pair that reaches this point is already raw-shape-
    // identical and is guaranteed to resolve to the same concrete
    // signature. A second check here would be unreachable dead code.
    let monomorphized = monomorphize(&resolved_hir)?;
    unroll_enum_loops(monomorphized)
}

pub(super) fn check_with_signatures(
    hir: &HirModule,
    signatures: &HashMap<String, (Vec<Ty>, Ty)>,
    function_local_names: &[Vec<&str>],
) -> Result<(), Diagnostic> {
    let mut env = Environment::new();
    // D-154 (Part 1 of #375): register every declared class before
    // checking any statement body -- a class must be usable (instantiated,
    // its instances passed around) from anywhere in the module, the same
    // "visible regardless of source position" requirement functions
    // already get from pass 1 below.
    class::bind_classes(&mut env, hir);
    // Pass 1: register every function's signature before checking any
    // statement body, matching Python's own "a module runs top to bottom,
    // but any def already executed is callable" semantics -- top-level
    // code and other function bodies (D-040) both need to see every
    // function regardless of its position in the file.
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty: item_return_ty,
            ..
        } = item
        {
            let (param_tys, return_ty) = signatures
                .get(name)
                .expect("every HIR function received an inferred signature");
            // D-133/D-134: a generic function's *original* body (still
            // carrying `Ty::Param`) is registered separately so a call
            // site can be resolved via `instantiate_generic_call` --
            // `signatures` itself already carries the same `Ty::Param`
            // entries (never `Ty::Infer`, so it survives both the
            // concrete-fast-path and solver-inferred paths unchanged), but
            // has no room for the body substitution needs.
            if is_generic_signature(params, item_return_ty) {
                env.bind_generic(name.clone(), item.clone());
            }
            env.bind_function(name.clone(), param_tys.clone(), return_ty.clone());
        }
    }
    check_with_environment(hir, env, function_local_names)
}

fn check_with_environment(
    hir: &HirModule,
    mut env: Environment,
    function_local_names: &[Vec<&str>],
) -> Result<(), Diagnostic> {
    // Issue #22: clear `defined_functions` before the top-level source-order
    // pass. `bind_function` (called by `check_with_signatures`'s pass 1 or
    // `concrete_function_environment`) adds every function to this set, but
    // for top-level checking we need to track which `def`s have actually
    // been *executed* in source order -- a call before the `def`'s position
    // is a NameError in CPython and must be rejected here. Function bodies
    // (pass 3) get a fresh seed of all function names via
    // `child_for_function`, so they're unaffected.
    env.defined_functions.clear();
    // Pass 2: check every top-level statement in source order, growing
    // `env`'s bindings as module-level assignments are encountered --
    // ordinary top-level code is still checked top-to-bottom (a top-level
    // forward reference to a not-yet-assigned name is a genuine error).
    // A `def` executes at its own position in that order and rebinds its
    // name to the function (D-110, refined by PR #252's review): later
    // `helper()` calls resolve the function exactly as CPython does, while
    // a value assignment *after* the `def` shadows it again. The `def` only
    // marks the name def-rebound -- it must NOT erase the representation
    // record in `bindings`, which D-040's sticky-representation rule keeps
    // consulting so an incompatible later reassignment still fails T0023.
    // The gate therefore tests the net source-order binding, in this pass
    // and in pass 3's final environment alike.
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                check_stmt(&mut env, stmt)?;
                // Issue #769 (Part 2 of #747): applied uniformly with every
                // other sequential-statement-list call site in this crate
                // for consistency, even though `HirStmt::Return` (the only
                // terminator `definitely_terminates` recognizes) cannot
                // syntactically appear at module top level -- so this is a
                // structural no-op here today, not dead functionality.
                narrow::apply_post_if_narrowing(&mut env, stmt);
            }
            HirItem::Function { name, .. } => {
                env.def_rebound.insert(name.clone());
                // Issue #22: a `def` at its source position makes the
                // function name callable from this point forward in
                // top-level code. Calls before this point (the name is
                // not yet in `defined_functions`) are rejected by
                // `infer_expr_in`'s `HirExpr::Call` arm.
                env.defined_functions.insert(name.clone());
            }
        }
    }
    // Pass 3: check every function body against a clone of `env` as it
    // stands once the whole module's top-level code has been processed
    // (D-041) -- a function can read any module-level global regardless of
    // whether its own `def` appears before or after that global's
    // assignment in the file, since real Python only evaluates a function
    // body when it's *called*, typically after the module has finished
    // running top to bottom.
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        if let HirItem::Function {
            params, return_ty, ..
        } = item
        {
            // D-133/D-134: a generic function's body is checked through
            // `check_generic_function_in` -- the shape gate, the Critical
            // self/mutual-generic-recursion rejection, and then the same
            // ordinary sibling-aware `check_function_in` body check every
            // non-generic function gets (PR-13 final review I3: the earlier
            // env-less variant could not see any sibling function, so a
            // generic body calling an ordinary sibling wrongly reported
            // "call to undefined function").
            if is_generic_signature(params, return_ty) {
                check_generic_function_in(&env, item, local_names)?;
            } else {
                check_function_in(&env, item, local_names)?;
            }
        }
    }
    Ok(())
}

/// Type-checks a module without materializing a resolved HIR clone.
///
/// Use [`check_and_resolve`] when a downstream compiler stage needs concrete
/// private-helper signatures in the returned HIR.
pub fn check(hir: &HirModule) -> Result<(), Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    // Issue #22: reject incompatible redefinitions before trying either the
    // concrete or solver path -- including a same-arity, `Ty::Infer`-
    // involving mismatch (see `check_incompatible_redefinitions`'s own doc
    // comment). Calling it here (not inside `check_with_environment`)
    // ensures the error is returned directly rather than being masked by
    // the concrete-path fallback to the solver path.
    check_incompatible_redefinitions(hir)?;
    // #676 (D-210): reject a cross-MRO attribute redeclaration with a
    // differing declared type before any expression using that attribute
    // is type-checked -- see the function's own doc comment for why this
    // must be a class-definition-time rejection rather than a coercion.
    check_incompatible_attribute_redeclarations(hir)?;
    // The public validation-only API has no resolved-signature result to
    // return. Avoid building a temporary concrete signature map and then
    // cloning it into an `Environment`: construct that environment directly.
    // On validation failure, preserve the historical solver-first diagnostic
    // selection exactly as `checked_function_signatures` does.
    if let Some(env) = concrete_function_environment(hir)
        && check_with_environment(hir, env, &function_local_names).is_ok()
    {
        return Ok(());
    }
    let signatures = infer_function_signatures_with_solver(hir, &function_local_names)?;
    check_with_signatures(hir, &signatures, &function_local_names)
}

pub(crate) fn checked_function_signatures(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
    // Issue #22: reject incompatible redefinitions before trying either the
    // concrete or solver path (same rationale as `check`'s own call).
    check_incompatible_redefinitions(hir)?;
    // #676 (D-210): same rationale and call-site timing as `check`'s own
    // call -- this entry point (via `check_and_resolve`) is also reachable
    // from `pycc build` without an earlier `pycc check`/`check` call, so it
    // needs its own guard against a cross-MRO attribute redeclaration.
    check_incompatible_attribute_redeclarations(hir)?;
    // Fully annotated valid modules have no inference variables to constrain.
    // Validate them once and avoid the preceding constraint-collection walk.
    // If validation fails, deliberately fall back to the historical
    // solver-first sequence so modules with multiple errors retain the same
    // first diagnostic as before this fast path existed.
    if let Some(signatures) = concrete_function_signatures(hir)
        && check_with_signatures(hir, &signatures, function_local_names).is_ok()
    {
        return Ok(signatures);
    }

    let signatures = infer_function_signatures_with_solver(hir, function_local_names)?;
    check_with_signatures(hir, &signatures, function_local_names)?;
    Ok(signatures)
}
