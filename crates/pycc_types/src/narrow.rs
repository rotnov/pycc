//! Issue #769 (Part 2 of #747): flow-sensitive `Optional[T]` narrowing.
//!
//! Extracted into its own module per the repository's source-file
//! decomposition rule (AGENTS.md "Keep source files decomposable") rather
//! than growing `lib.rs`'s `HirStmt::If` arms in place.
//!
//! ## Design: an overlay, not a join-time mutation
//!
//! `join_if_branches` (`lib.rs`) implements first-assignment-wins joining
//! of two branch-local `Environment` clones' `bindings` back into the
//! parent `env`. If narrowing mutated a branch clone's own `bindings` entry
//! for the narrowed name (e.g. rebinding `x` to `Ty::Int` inside an `if x
//! is not None:` body), the join would either leak that narrowed type past
//! the `if` (since `join_if_branches` is oblivious to *why* a binding
//! changed) or spuriously reject the join on a mismatch with the other
//! branch's still-`Optional` binding, depending on `is_assignable`'s
//! direction. Neither is correct: outside the `if`, `x` must still be
//! `Optional[int]`.
//!
//! Instead, narrowing lives entirely in a side table,
//! [`crate::env::Environment::narrowed`], populated only on branch-local
//! `env` clones (in-branch narrowing, [`narrowing_target`] /
//! [`apply_branch_narrowing`]) or directly on the real `env` for the
//! early-return continuation shape ([`apply_post_if_narrowing`]). This
//! table is *never* merged into `bindings` and `join_if_branches` never
//! reads it, so it cannot structurally leak across the join -- there is no
//! join step for it to leak through. `crate::expr::infer_expr_in`'s
//! `HirExpr::Name` arm is the only place a *read* of a name consults the
//! overlay; `check_assignment`'s target-checking path never does (it
//! checks a `name = value` assignment against `name`'s real, un-narrowed
//! type, and separately clears the overlay entry for that name -- killing
//! the narrowing the moment the name is reassigned).
//!
//! ## `definitely_terminates`: a strict predicate, not `contains_return`
//!
//! `constraints.rs::contains_return` answers "does a `return` occur
//! *anywhere* in this body, including nested inside an unrelated inner
//! `if`?" -- true for `if flag: return 0` even when `flag` is false on some
//! paths. That is unsound as a narrowing-eligibility guard: naively using
//! it for
//!
//! ```python
//! if x is None:
//!     if flag:
//!         return 0
//! print(x + 1)
//! ```
//!
//! would incorrectly narrow `x` after the outer `if` even though `x` can
//! still be `None` when `flag` is `False` (the outer `if`'s body does not
//! terminate on every path). [`definitely_terminates`] is a new, strictly
//! narrower predicate: true only when the body's *last* statement is
//! itself unconditionally terminating -- a bare `return`, or an `if` whose
//! `body` **and** non-empty `orelse` both recursively terminate. `raise` is
//! deliberately *not* a terminator here (a documented scope cut -- see its
//! own doc comment), and `match` is not analyzed at all (also a documented
//! scope cut, kept simple and sound by omission rather than by an
//! exhaustiveness heuristic that could be wrong).
//!
//! ## Deliberate scope cuts (recorded in D-204)
//!
//! - Only a *top-level* `if name is None:` / `if name is not None:` test is
//!   recognized -- no narrowing through a compound `and`/`or` test.
//! - `raise` in a branch does not count as a terminator for the
//!   early-return continuation shape; only `return` (directly, or via an
//!   exhaustive nested `if`) does.
//! - No narrowing-to-`None` shape: `if name is not None: ... else: <use
//!   name as None>` is not implemented (there is no `Ty::None`-typed
//!   *narrowing* target in this design -- `Ty::None` already exists as a
//!   type but is not what a "narrowed" name would become here), and the
//!   post-`if` early-return continuation shape only ever narrows to the
//!   Optional's inner type, never to `Ty::None`.

use crate::env::Environment;
use crate::{check_stmt, check_stmt_in_function};
use pycc_diag::Diagnostic;
use pycc_hir::{HirStmt, NoneTestPolarity, Ty, optional_none_test};
use std::collections::HashMap;

/// The result of recognizing an `if` statement's `test` as a
/// narrowing-eligible shape: the bare name, the `Optional`'s inner type
/// (from `env`'s *current* knowledge of the name, before the `if`), and
/// the test's polarity.
pub(crate) struct NarrowingTarget {
    pub(crate) name: String,
    pub(crate) inner: Ty,
    pub(crate) polarity: NoneTestPolarity,
}

/// Recognizes `test` as a top-level `name is None` / `name is not None`
/// shape (via the shared `pycc_hir::optional_none_test` recognizer) where
/// `name` is *currently* known to be `Ty::Optional(_)` in `env`. Returns
/// `None` for every other test shape, including one where the named
/// variable's current type is not `Ty::Optional` (e.g. never narrows a
/// plain `int`) -- this is deliberately re-checked against `env` on every
/// call rather than cached, since the same syntactic test can be reused (a
/// nested `if` inside a narrowed region) against a different current type.
pub(crate) fn narrowing_target(env: &Environment, test: &pycc_hir::HirExpr) -> Option<NarrowingTarget> {
    let (name, polarity) = optional_none_test(test)?;
    match env.lookup_any(name) {
        Some(Ty::Optional(inner)) => Some(NarrowingTarget {
            name: name.to_string(),
            inner: *inner,
            polarity,
        }),
        _ => None,
    }
}

/// Applies in-branch narrowing (item 1 of the design) to a `if`
/// statement's two branch-local `env` clones, per `target`'s polarity:
/// `name is not None` narrows only `body_env` (the body only runs when
/// `name` is present); `name is None` narrows only `orelse_env` (the
/// `orelse` only runs when `name` is present, since the body already
/// claimed the "`name` is absent" case). The other clone is left
/// untouched -- it keeps resolving `name` as `Optional[inner]`, exactly as
/// it already would with no overlay entry at all.
pub(crate) fn apply_branch_narrowing(
    body_env: &mut Environment,
    orelse_env: &mut Environment,
    target: &NarrowingTarget,
) {
    match target.polarity {
        NoneTestPolarity::IsNot => {
            body_env
                .narrowed
                .insert(target.name.clone(), target.inner.clone());
        }
        NoneTestPolarity::Is => {
            orelse_env
                .narrowed
                .insert(target.name.clone(), target.inner.clone());
        }
    }
}

/// True only when `body`'s control flow unconditionally terminates the
/// enclosing function on every path through it -- see this module's own
/// doc comment for the full rationale and the unsound `contains_return`
/// example this predicate exists to replace.
///
/// A thin re-export of `pycc_hir::definitely_terminates`, not an
/// independent copy: that predicate is shared with `pycc_mir`'s own
/// `OptionalUnwrap` lowering, for the identical "shared dependency of both,
/// `pycc_mir` cannot depend on `pycc_types`" reason
/// `pycc_hir::optional_none_test` is shared -- see its own doc comment for
/// the full soundness rationale.
pub(crate) fn definitely_terminates(body: &[HirStmt]) -> bool {
    pycc_hir::definitely_terminates(body)
}

/// Issue #769 (Part 2 of #747), the early-return continuation shape: if
/// `stmt` is `if name is None: <body that definitely terminates>`, `name`
/// is known to be present (the `Optional`'s inner type) for every
/// statement *after* `stmt` in the same sequential statement list --
/// reaching that point is only possible via the implicit "`name` is not
/// `None`" else path, since the body's own `return` never falls through to
/// here. Mutates `env` directly (not a clone): this is not an in-branch
/// overlay, it is a fact now true of the *rest of the enclosing block*,
/// exactly like `join_if_branches`' own bindings would be if this body
/// unconditionally bound something -- except this shape narrows a name
/// `join_if_branches` was never going to touch at all (the `if` itself
/// binds nothing new).
///
/// Deliberately one-directional: `if name is not None: <body that
/// terminates>` is *not* handled the mirror way (narrowing the
/// continuation to a `None` type) -- see this module's own "no
/// narrowing-to-`None`" scope-cut note.
pub(crate) fn apply_post_if_narrowing(env: &mut Environment, stmt: &HirStmt) {
    let HirStmt::If { test, body, .. } = stmt else {
        return;
    };
    let Some(target) = narrowing_target(env, test) else {
        return;
    };
    if matches!(target.polarity, NoneTestPolarity::Is) && definitely_terminates(body) {
        env.narrowed.insert(target.name, target.inner);
    }
}

/// Join step for the `narrowed` overlay (blocker fix, D-068 review of #780,
/// Part 2 of #747/#769): every branch-joining construct in `lib.rs`
/// (`join_if_branches`, `join_loop_body`, `join_match_branches`) reconciles
/// `.bindings` but historically left `.narrowed` completely untouched --
/// whatever the target `env`'s overlay held *before* the branch/loop/match
/// ran was simply left in place afterward, regardless of what any branch
/// actually did to it. That silently reverted a `kill_narrowing`-equivalent
/// event (an assignment inside exactly one nested branch, via
/// `check_assignment`'s `env.narrowed.remove(target)`) the moment control
/// returned to the enclosing block, and a subsequent read of the
/// "reassigned to `None`" name still saw the stale narrowed (non-`None`)
/// type.
///
/// The fix applies the same sound, conservative rule uniformly everywhere
/// two or more possible-path `Environment`s are reconciled into one: a name
/// stays narrowed to `ty` after the join only if *every* supplied map
/// narrows it to that exact `ty`. A name absent from any one map (killed on
/// that path, or never narrowed there to begin with -- e.g. a narrowing
/// newly established inside only one arm) drops out of the intersection
/// entirely. This is deliberately not "maximally precise" (it does not
/// attempt to prove two structurally different narrowed types are still
/// compatible, or to special-case an unconditionally-terminating branch the
/// way `join_if_branches` does for `.bindings`) -- per this repository's
/// D-127 judgment call recorded in the #780 review response, a narrower
/// conservative join that can only ever *drop* a valid narrowing is
/// preferable to a more precise one that risks keeping an invalid one.
/// Every call site has at least one map by construction (the branch/loop
/// body's own end-state, or -- for `join_match_branches` -- `env`'s
/// pre-match state standing in for the implicit "no case matched" path), so
/// the signature takes `first` separately from `rest` rather than a
/// possibly-empty slice: that keeps the join total without an unreachable
/// empty-input branch.
pub(crate) fn join_narrowed(
    first: &HashMap<String, Ty>,
    rest: &[&HashMap<String, Ty>],
) -> HashMap<String, Ty> {
    let mut joined: HashMap<String, Ty> = first.clone();
    for map in rest {
        joined.retain(|name, ty| map.get(name) == Some(ty));
    }
    joined
}

/// Issue #769 follow-up (D-068 re-review of #780, third round): the
/// kill-prescan. Drops every name `body` reassigns anywhere within it
/// (per `pycc_hir::killed_names`, which walks `body` recursively) from
/// `env`'s narrowing overlay, for the entire body -- not just from the
/// kill's own source position onward.
///
/// A single left-to-right source-order pass (what every other narrowing
/// entry point in this module and `lib.rs` still does) is unsound
/// whenever the body it checks can be *entered or re-entered* such that a
/// read earlier in source order than a kill can nonetheless execute
/// *after* that kill at runtime:
///
/// - A `while`/`for` loop body checked/lowered once but executed
///   repeatedly: a read on line 2 followed by a kill on line 3 is fine on
///   the first iteration, but the second iteration's read on line 2 runs
///   after the first iteration's kill on line 3 already fired.
/// - An `except` handler, entered partway through the `try` body it
///   guards: a handler read is checked against the *pre-try* narrowing
///   state, but at runtime the handler only ever runs after some prefix
///   of the try body already executed -- a kill anywhere in that body may
///   have already invalidated the narrowing before the handler starts.
///
/// The prescan is deliberately whole-body and non-fixpoint: it does not
/// attempt to determine *which* reads are actually reachable after a
/// given kill (that would need a real control-flow reachability analysis
/// per read), it just drops the narrowing for the entire body once, unconditionally,
/// whenever the body contains any kill of that name at all. This is sound
/// (it can only under-narrow, never over-narrow) and requires no fixpoint
/// iteration, at the cost of also dropping narrowing for a read that
/// (looking only at that one execution) precedes every kill -- a loop
/// body that reads a narrowed name but never kills it anywhere is
/// unaffected (its kill set is empty), so this conservative rule only
/// costs precision on bodies that actually do kill the name somewhere.
///
/// Call this before checking/lowering a loop body (`While`/`ForRange`/
/// `ForList`, both module and function scope, every fast- and slow-path
/// call site) and before checking each `except` handler body (against the
/// pre-try `env` clone, with the *try body's* kill set). A straight-line
/// body or an `if`/`else` with no enclosing loop or `try` needs no
/// prescan at all: execution order there already equals source order, so
/// the existing sequential pass is already sound.
pub(crate) fn apply_kill_prescan(env: &mut Environment, body: &[HirStmt]) {
    for name in pycc_hir::killed_names(body) {
        env.narrowed.remove(&name);
    }
}

/// Checks `stmts` sequentially against `env` (module scope), applying the
/// early-return continuation narrowing ([`apply_post_if_narrowing`]) after
/// each statement -- the narrowing-aware replacement for the raw `for stmt
/// in stmts { check_stmt(env, stmt)?; }` loop every sequential body in
/// `lib.rs` used before this issue.
pub(crate) fn check_stmt_sequence(env: &mut Environment, stmts: &[HirStmt]) -> Result<(), Diagnostic> {
    for stmt in stmts {
        check_stmt(env, stmt)?;
        apply_post_if_narrowing(env, stmt);
    }
    Ok(())
}

/// Function-scope counterpart of [`check_stmt_sequence`].
pub(crate) fn check_stmt_sequence_in_function(
    env: &mut Environment,
    local_names: &[&str],
    stmts: &[HirStmt],
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        check_stmt_in_function(env, local_names, stmt, return_ty.clone())?;
        apply_post_if_narrowing(env, stmt);
    }
    Ok(())
}
