//! Private-helper constraint solver: definite-assignment join helpers.
//!
//! Issue #359 (Part 2 of #118) extends the validation pass's
//! definite-assignment tracking (T0041, D-147) to the private-helper
//! constraint solver. This submodule holds the solver's control-flow join
//! helpers, extracted from [`lib.rs`] per the repository's source-file
//! decomposition rule (AGENTS.md "Keep source files decomposable"). The
//! [`ConstraintEnvironment`] struct and the [`collect_block_constraints`]
//! arms that call these helpers remain in [`lib.rs`] because they are
//! deeply intertwined with the rest of the type-inference solver
//! (`collect_expr_constraints`, `unify_terms`, `fresh_term`, etc.).

use std::collections::HashSet;

use crate::ConstraintEnvironment;

/// Issue #359 (Part 2 of #118): joins two if-branch environments back
/// into `env` after cloning and running each branch independently.
/// Mirrors the validation pass's `join_if_branches` (D-147) but works
/// with the solver's `TypeTerm`-based `ConstraintEnvironment.bindings`
/// and tracks maybe-bound names in `maybe_bindings` instead of wrapping
/// each binding in a `BindingState` variant.
///
/// `pre_existing` is the set of binding names that were in `env.bindings`
/// before the `if` — names introduced by only one branch are maybe-bound,
/// names introduced by both branches are definitely bound.
pub(crate) fn join_if_branches_solver(
    env: &mut ConstraintEnvironment,
    body_env: &ConstraintEnvironment,
    orelse_env: &ConstraintEnvironment,
    pre_existing: &HashSet<String>,
) {
    // Merge bindings: first-binding-wins (body first, then orelse).
    // `entry().or_insert()` preserves the existing binding for pre-existing
    // names and takes the body's term for new names introduced by the body.
    for (name, term) in &body_env.bindings {
        env.bindings.entry(name.clone()).or_insert(term.clone());
    }
    for (name, term) in &orelse_env.bindings {
        env.bindings.entry(name.clone()).or_insert(term.clone());
    }
    // Update maybe_bindings for names introduced by the branches.
    let body_new: HashSet<&String> = body_env
        .bindings
        .keys()
        .filter(|k| !pre_existing.contains(*k))
        .collect();
    let orelse_new: HashSet<&String> = orelse_env
        .bindings
        .keys()
        .filter(|k| !pre_existing.contains(*k))
        .collect();
    for name in body_new.iter().chain(orelse_new.iter()) {
        if body_new.contains(name) && orelse_new.contains(name) {
            // Both branches bind it → definitely bound.
            env.maybe_bindings.remove(*name);
        } else {
            // Only one branch binds it → maybe bound.
            env.maybe_bindings.insert((*name).clone());
        }
    }
}

/// Issue #359 (Part 2 of #118): joins a loop body environment back into
/// `env` after cloning and running the body. Mirrors the validation pass's
/// `join_loop_body` (D-147): a loop body may execute zero times, so every
/// body-only binding is maybe-bound. Pre-existing bindings stay as-is
/// (their maybe/definite status is unchanged).
pub(crate) fn join_loop_body_solver(
    env: &mut ConstraintEnvironment,
    body_env: &ConstraintEnvironment,
    pre_existing: &HashSet<String>,
) {
    for (name, term) in &body_env.bindings {
        if !pre_existing.contains(name) {
            env.bindings.entry(name.clone()).or_insert(term.clone());
            env.maybe_bindings.insert(name.clone());
        }
    }
}
