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
//! ## Deliberate scope cuts (recorded in D-199)
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
/// example this predicate exists to replace. Sound-by-omission for any
/// statement shape not explicitly handled below (an unhandled last
/// statement makes the whole body report `false`, never `true`): a `try`,
/// a `while`, a `for`, and `match` are all treated as non-terminating here,
/// even if a human reader could prove some of them exhaustive, because
/// proving that soundly is out of scope for this predicate (documented
/// scope cut, D-199).
pub(crate) fn definitely_terminates(body: &[HirStmt]) -> bool {
    match body.last() {
        Some(HirStmt::Return(_)) => true,
        // An `if` terminates the body only when *both* branches do, and
        // only when there is a non-empty `orelse` at all -- an `if` with no
        // `else` can never be exhaustive (the "no-op" implicit-else path
        // falls straight through to the statement after the `if`, which is
        // exactly the case the unsound `contains_return`-based design would
        // have wrongly accepted).
        Some(HirStmt::If { body, orelse, .. }) => {
            !orelse.is_empty() && definitely_terminates(body) && definitely_terminates(orelse)
        }
        // `raise` is deliberately not a terminator here (documented scope
        // cut, D-199): a program that raises out of the narrowed branch
        // does structurally guarantee the same "narrow the continuation"
        // soundness a `return` does, but recognizing it correctly would
        // additionally have to account for `try`/`except` catching it
        // before it propagates out of the enclosing function -- out of
        // scope for this PR.
        _ => false,
    }
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
