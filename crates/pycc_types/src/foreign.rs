//! Foreign (CPython-object) import bindings in the type environment
//! (Part 1 of #1026, PR 1c of #1080).
//!
//! `import numpy` binds `numpy` to [`Ty::Object`], an opaque `PyObject *`
//! whose shape pycc knows nothing about. Every refusal of an operation on
//! such a value is `I0404`.
//!
//! **Part 2 of #1026 (#1081) moved the refusal from the producer to the
//! consumers, and this paragraph replaces the containment invariant Part 1
//! rested on.** Part 1 refused the *read* of a foreign binding, which
//! refused every derived operation at once without a refusal per
//! operation. That works only while no operation is supported. Part 2
//! supports `numpy.pi`, and `infer_expr_in` is context-free -- it cannot
//! see whether the read it is answering feeds an attribute load or a
//! `print` -- so a conditional relaxation of the read is not expressible.
//! The read is therefore unconditionally admitted and each *consuming*
//! site refuses on its own.
//!
//! Two consequences follow, and both are load-bearing:
//!
//! 1. A `Ty::Object` value no longer has a name. The diagnostic builder
//!    below names the **operation** instead ([`object_operation_unsupported`]),
//!    because the consumer knows what it was about to do while the value
//!    it holds is an anonymous temporary.
//! 2. There are now two producer shapes, not one: `o.attr`
//!    (`HirExpr::AttrGet` over a `Ty::Object` base) and a call to an
//!    unannotated private helper whose inferred return is `Ty::Object`
//!    (`constraints.rs`'s `AttrGet` term). Every refusal must therefore
//!    key on the **type**, never on the producing expression shape.
//!
//! [`reject_object_read`] survives for the two sites that still key on a
//! *named* binding and are not expressible as a consumer:
//!
//! 1. `lookup_bound_name` (D-105's `ForList`/`ListAppend` HIR shape carries
//!    its list as a plain `String`, so it never becomes a `HirExpr::Name`),
//! 2. `expr::infer_expr_in`'s `HirExpr::Call` arm, whose value-binding gate
//!    would otherwise report the generic `non_callable_binding` `T0021`.
//!    Calling a CPython object stays refused in Part 2 deliberately: the
//!    environment does not record whether a `Ty::Object` came from a
//!    foreign global or from an attribute load, so admitting `f(2.0)`
//!    would also admit `numpy(1)`, which CPython itself answers with
//!    `TypeError: 'module' object is not callable`.

use crate::Environment;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{ImportBinding, Ty};

/// The diagnostic every unsupported operation on a CPython object gets.
///
/// `operation` is a noun phrase naming what the *consumer* was about to
/// do, written so the message reads as a sentence: "printing a CPython
/// object is not supported yet". Part 1 interpolated the binding's local
/// name here instead; Part 2's refusals sit at consuming sites that hold
/// an anonymous temporary and have no name to report (see the module doc).
pub(crate) fn object_operation_unsupported(operation: &str) -> Diagnostic {
    Diagnostic::error(
        "I0404",
        format!(
            "{operation} is not supported yet -- pycc models a CPython object as an opaque \
             value, and Part 2 of #1026 implements attribute access on it and nothing else"
        ),
        Span::new(0, 0),
    )
}

/// `Err(I0404)` when `ty` is the opaque object type, `Ok(())` otherwise.
///
/// The guard for the two sites that still key on a *named* binding rather
/// than on a consumed value -- `lookup_bound_name` and the `HirExpr::Call`
/// value-binding gate (module doc). A consuming site calls
/// [`object_operation_unsupported`] directly instead, because it knows the
/// operation and its operand has no name.
pub(crate) fn reject_object_read(name: &str, ty: &Ty) -> Result<(), Diagnostic> {
    if matches!(ty, Ty::Object) {
        return Err(object_operation_unsupported(&format!(
            "using `{name}`, which is bound to a CPython object, in this position"
        )));
    }
    Ok(())
}

/// `Err(I0404)` when `ty` is the opaque object type, naming `operation`.
///
/// The consumer-side counterpart of [`reject_object_read`]: one helper so
/// the ten condition sites, the renderer, `check_assignment`,
/// `check_isinstance` and `check_match` cannot drift into ten spellings of
/// the same rule.
pub(crate) fn reject_object_operand(ty: &Ty, operation: &str) -> Result<(), Diagnostic> {
    if matches!(ty, Ty::Object) {
        return Err(object_operation_unsupported(operation));
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
