//! Foreign (CPython-object) import bindings in the type environment
//! (Part 1 of #1026, PR 1c of #1080).
//!
//! `import numpy` binds `numpy` to [`Ty::Object`], an opaque `PyObject *`
//! whose shape pycc knows nothing about. Part 1 deliberately implements
//! *only* the binding: the containment invariant it rests on is that the
//! single producer of a `Ty::Object` value in expression position is a
//! read of such a binding, so refusing that read refuses every derived
//! operation -- attribute access, calls, iteration, rendering, arithmetic
//! -- without one refusal per operation. Every refusal is `I0404`.
//!
//! Three choke points read a bound name's type and are therefore the
//! complete set of sites a `Ty::Object` can escape through:
//!
//! 1. `expr::infer_expr_in`'s `HirExpr::Name` arm (every expression-position
//!    read),
//! 2. `lookup_bound_name` (D-105's `ForList`/`ListAppend` HIR shape carries
//!    its list as a plain `String`, so it never becomes a `HirExpr::Name`),
//! 3. `expr::infer_expr_in`'s `HirExpr::Call` arm, whose value-binding gate
//!    would otherwise report the generic `non_callable_binding` `T0021`.

use crate::Environment;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{ImportBinding, Ty};

/// The diagnostic every operation on a foreign object gets in Part 1.
///
/// `name` is the local name as written, which for a foreign import is
/// always the module path too (Part 1 admits only the unaliased, undotted
/// `import X` shape -- see `src/modules.rs`'s `missing`).
pub(crate) fn object_operation_unsupported(name: &str) -> Diagnostic {
    Diagnostic::error(
        "I0404",
        format!(
            "`{name}` is a CPython object; operations on a CPython object are not supported yet -- \
             Part 1 of #1026 binds the imported module but implements no operation on it"
        ),
        Span::new(0, 0),
    )
}

/// `Err(I0404)` when `ty` is the opaque object type, `Ok(())` otherwise.
/// Written as a guard the three choke points call rather than an arm each
/// open-codes, so "which operations are refused" has exactly one answer.
pub(crate) fn reject_object_read(name: &str, ty: &Ty) -> Result<(), Diagnostic> {
    if matches!(ty, Ty::Object) {
        return Err(object_operation_unsupported(name));
    }
    Ok(())
}

/// The local names a module's import table binds to a CPython object, in
/// source order.
pub(crate) fn foreign_object_names(imports: &[ImportBinding]) -> Vec<&str> {
    imports
        .iter()
        .filter_map(|binding| match binding {
            ImportBinding::Foreign { local_name, .. } => Some(local_name.as_str()),
            ImportBinding::Module { .. }
            | ImportBinding::Symbol { .. }
            | ImportBinding::Project { .. } => None,
        })
        .collect()
}

/// Seeds every foreign import as a definitely-bound `Ty::Object` global.
///
/// Seeding happens *before* the source-order top-level pass (D-041 pass 2)
/// so that a read placed above the `import` is refused too: an `import`
/// statement produces no `HirItem`, so there is no item at its own position
/// for the pass to seed from. The observable consequence is that a
/// module-body read of a foreign name placed *above* its `import` is
/// `I0404` rather than the `T0021` CPython's own `NameError` would justify
/// -- a fail-closed divergence: both are compile errors, and the program is
/// refused either way. Recorded in `docs/TYPE_SYSTEM.md`.
///
/// The seed alone is not enough, because it cannot supersede a binding the
/// source-order pass makes *later*: a `def numpy()` above the import used
/// to leave `numpy` def-rebound, which made the call gate skip the `I0404`
/// refusal and let the artifact call a function CPython would have replaced
/// with a module object (PR 1c of #1080 review finding 2). The pass
/// therefore re-applies each binding at its recorded position, via
/// [`bind_foreign_objects_at`].
///
/// D-040's sticky-representation rule then does the rest: a later
/// `numpy = 3` in the same module is `T0023`, because the name's recorded
/// representation is `object` and `int` is not assignable to it.
pub(crate) fn bind_foreign_objects(env: &mut Environment, imports: &[ImportBinding]) {
    for name in foreign_object_names(imports) {
        env.bind(name.to_string(), Ty::Object);
    }
}

/// Applies every foreign import recorded at `position` in the item list,
/// at the point the source-order pass reaches that position.
///
/// `ImportBinding::Foreign::item_index` is the item count at the moment the
/// `import` lowered, so the statement runs immediately *before* item
/// `position`; a trailing import records the item count itself, which the
/// caller applies once the loop is done. `Environment::bind` clears the
/// name's `def_rebound` mark, which is precisely what makes a later call
/// reach the `I0404` refusal instead of the stale function pointer -- and,
/// symmetrically, a `def` *below* the import re-marks the name and keeps
/// working, matching CPython's own last-binding-wins order.
pub(crate) fn bind_foreign_objects_at(
    env: &mut Environment,
    imports: &[ImportBinding],
    position: usize,
) {
    for binding in imports {
        if let ImportBinding::Foreign {
            local_name,
            item_index,
            ..
        } = binding
            && *item_index == position
        {
            env.bind(local_name.clone(), Ty::Object);
        }
    }
}

#[cfg(test)]
mod tests;
