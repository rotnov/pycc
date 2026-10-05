//! A list display bound to a CPython object slot (Part 2d of #1371, D-258
//! rule 4 and its 2026-10-05 amendment).
//!
//! `x: object = []` and, after `s = value_stack[-size:]` bound `s` to an
//! object, `s = []` are both CPython lists in the source program. pycc has
//! no native `list[T]` that could stand in an object slot, so the display is
//! rewritten into [`HirExpr::ObjectList`], which builds a fresh CPython
//! `list` with every element boxed. Running here, inside the same pre-pass
//! that types `[]` from its slot, keeps the empty-container pass's key
//! property: `pycc check` and `pycc build` consume the identical node.
//!
//! **Sources.** The slot is an object when
//!
//! 1. the `AnnAssign` annotation is [`Ty::Object`] (`object`, `Any`, and in
//!    an `--ext` build every annotation that lowers to it), or
//! 2. there is no annotation and the target's flat whole-function binding
//!    (source 2 of the parent module) is [`Ty::Object`].
//!
//! An annotation of any other type wins over the binding, exactly as it does
//! for a native resolution, and is left to the parent's own sources.
//!
//! **Which displays.** Source 1 rewrites any list display, empty or not:
//! the annotation is written next to it and says what the display is.
//! Source 2 rewrites only the *empty* `[]`. A non-empty `[1, 2]` already has
//! a native type of its own, and the flat binding is order-insensitive (the
//! parent module's "Source 2 is order-insensitive" paragraph): rewriting
//! `xs = [1]` into an object because some *other* assignment binds `xs` to
//! an object would change the type of a display that compiles today, on the
//! strength of a binding that may follow it. Leaving it native keeps the
//! check phase's D-040 sticky-representation rule in charge of that mix. An
//! empty `[]` has no native type at all, so typing it from the binding can
//! only turn a `T0003` into an accepted program, never change one. A dict
//! display `{}` bound to an object stays `T0003`: D-258 rule 4's dict half
//! is not implemented yet.
//!
//! **Safety.** As for the parent's own source 2, this pass resolves but
//! never accepts: the check phase re-validates every assignment in program
//! order, so a rewritten `s = []` is accepted only where an object is
//! assignable to `s`, and `crate::foreign::list_display` refuses an element
//! with no boxing helper (`I0404`).

use super::*;

/// The rewritten node for `value` when it is a list display bound to an
/// object slot (see the module documentation for which), `None` otherwise.
pub(super) fn object_list(
    value: &HirExpr,
    target: &str,
    annotation: Option<&Ty>,
    env: &Environment,
) -> Option<HirExpr> {
    let HirExpr::ListLiteral(elements) = value else {
        return None;
    };
    let slot_is_object = match annotation {
        Some(annotation) => *annotation == Ty::Object,
        None => {
            elements.is_empty()
                && env
                    .binding_state(target)
                    .is_some_and(|state| *state.ty() == Ty::Object)
        }
    };
    slot_is_object.then(|| HirExpr::ObjectList(elements.clone()))
}

/// Whether `stmt` is an annotated assignment of a list display to an object
/// slot -- the one shape [`object_list`] rewrites that the parent's
/// empty-literal fast path does not already see, because the display may
/// be non-empty.
pub(super) fn is_annotated_object_list(stmt: &HirStmt) -> bool {
    matches!(
        stmt,
        HirStmt::AnnAssign {
            annotation: Ty::Object,
            value: Some(HirExpr::ListLiteral(_)),
            ..
        }
    )
}
