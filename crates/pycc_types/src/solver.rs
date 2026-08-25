//! Private-helper constraint solver: definite-assignment join helpers.
//!
//! Issue #359 (Part 2 of #118) extends the validation pass's
//! definite-assignment tracking (T0041, D-147) to the private-helper
//! constraint solver. This submodule holds the solver's control-flow join
//! helpers, extracted from [`lib.rs`] per the repository's source-file
//! decomposition rule (AGENTS.md "Keep source files decomposable"). The
//! [`ConstraintEnvironment`] struct and the [`collect_block_constraints`](crate::constraints::collect_block_constraints)
//! arms that call these helpers live in [`constraints`](crate::constraints)
//! because they are deeply intertwined with the rest of the type-inference solver
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
    //
    // Issue #771 join-site follow-up, second reviewer pass: a name that was
    // pre-existing but only *opaquely* bound (in `pre_existing` via
    // `env.opaque_bindings`, not yet in `env.bindings`) must be skipped here
    // when only ONE branch reassigns it to a real term -- otherwise a
    // branch's term would get written into `env.bindings` unconditionally
    // (unlike a pre-existing *real* binding, which already blocks the
    // insert because `env.bindings` already holds an entry for it), even
    // though the other (untouched) branch's actual value is still the old
    // opaque one; since such a name is not newly introduced by either
    // branch, it would not land in `maybe_bindings` below either, so the
    // unguarded insert would make `HirExpr::Name`'s lookup return that one
    // branch's term as if it always applied on every path -- a genuine
    // unmasking, not just an imprecision.
    //
    // Issue #771 join-site follow-up, THIRD reviewer pass: the naive form of
    // this guard (skip whenever `pre_existing.contains(name) &&
    // !env.bindings.contains_key(name)`, independent of what the *other*
    // branch did) is too broad -- it also fires when BOTH branches reassign
    // the same pre-existing-opaque name to a real term (e.g. `y = d.get(...)`
    // then `if cond: y = 1 else: y = 2`), skipping both branches' real terms
    // in that case too. Because `env.opaque_bindings` already carries the
    // name from before the `if` and the opaque-merge loop below only ever
    // inserts (never removes), the outcome is not an `unbound_local`
    // misdiagnosis -- `HirExpr::Name` still resolves the name via the
    // surviving stale opaque marker. The bug is a silent masking: both
    // branches' concrete terms get dropped and the name is reported as
    // "opaque, no term available" even though every path through the `if`
    // actually assigned it a real, solver-representable type -- for a name
    // that is not branch-conditional at all. The guard therefore only skips
    // when the *other* branch's environment does
    // not also carry a real term for the same name: that is precisely the
    // "reassigned in exactly one branch" case the comment above describes.
    // When both branches independently reassign a pre-existing-opaque name,
    // this guard does not fire in either loop, so the merge proceeds via the
    // ordinary first-body-then-orelse `entry().or_insert()` below --
    // identical first-wins semantics to a name that is genuinely new to both
    // branches.
    for (name, term) in &body_env.bindings {
        if pre_existing.contains(name) && !orelse_env.bindings.contains_key(name) {
            continue;
        }
        env.bindings.entry(name.clone()).or_insert(term.clone());
    }
    for (name, term) in &orelse_env.bindings {
        if pre_existing.contains(name) && !body_env.bindings.contains_key(name) {
            continue;
        }
        env.bindings.entry(name.clone()).or_insert(term.clone());
    }
    // Issue #771 join-site follow-up: merge opaque markers too. A name
    // assigned in a branch from an initializer the solver can't represent
    // as a term (e.g. `if c: x = cast(D, b)`) lives only in that branch's
    // `opaque_bindings`, never in `bindings` -- without this it was
    // silently dropped by the join above, so a name opaquely assigned in
    // *both* branches ended up bound nowhere at all post-join and a later
    // read misfired as an unbound local, exactly the diagnostic this
    // module exists to avoid. A name with a real term in `env.bindings`
    // always takes priority over a stale/duplicate opaque marker for the
    // same name (see `HirExpr::Name`'s lookup order in `constraints.rs`),
    // so it is harmless for a name to end up in both sets here.
    for name in body_env
        .opaque_bindings
        .iter()
        .chain(orelse_env.opaque_bindings.iter())
    {
        env.opaque_bindings.insert(name.clone());
    }
    // Update maybe_bindings for names introduced by the branches, whether
    // via a real term or an opaque marker.
    let body_new: HashSet<&String> = body_env
        .bindings
        .keys()
        .chain(body_env.opaque_bindings.iter())
        .filter(|k| !pre_existing.contains(*k))
        .collect();
    let orelse_new: HashSet<&String> = orelse_env
        .bindings
        .keys()
        .chain(orelse_env.opaque_bindings.iter())
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
    // Issue #771 join-site follow-up: mirror opaque bindings the same way.
    // The body may execute zero times, so a name assigned only via a
    // solver-unrepresentable initializer inside the body (e.g. a `cast` to
    // a class) is maybe-bound afterward, not definitely bound -- exactly
    // like a real-term binding introduced there. Without this, such a name
    // was dropped entirely by this join and a later read outside the loop
    // misfired as an unbound local.
    for name in &body_env.opaque_bindings {
        if !pre_existing.contains(name) {
            env.opaque_bindings.insert(name.clone());
            env.maybe_bindings.insert(name.clone());
        }
    }
}
