//! The release protocol for CPython object temporaries (Part 1 of #1092).
//!
//! Every shim producer -- an attribute load, a call, a subscript, a rich
//! comparison, `type(o)`, a slice, a list display, a tuple unpack -- hands
//! compiled code a *new* `PyObject *` reference. Before this module every
//! one of them was leaked on purpose (the leak-only rule `docs/RUNTIME.md`
//! recorded), which made a foreign operation inside a loop leak one
//! reference per trip.
//!
//! This module releases the references that are **temporaries**: a
//! produced value that a borrowing operation consumes and that is bound
//! nowhere. That is the operand of another object operation (the base of
//! `o.a.b`, the argument of `o.m(p.q)`, the operand of `len(o.x)`), a
//! condition (`if o.ready():`), the operand of a conversion (`float(o.x)`),
//! and a discarded statement value (`o.update()`). Each such site is a
//! *consumer*: it evaluates the operand, calls [`hold`] on it, runs the
//! operation, and then calls [`Held::release`], which emits one call to the
//! shim's `pycc_ext_obj_release` (`Py_XDECREF`).
//!
//! **Ownership is decided by MIR shape, never by type.** [`is_produced`] is
//! an allowlist of the shim's new-reference producers, the one
//! classification `foreign_call::callee_is_produced` also answers through.
//! Everything else that evaluates to a `Scalar::Object` is a *borrow* --
//! a `Name` read of a module global or a slot, a function parameter, a
//! `for` target, a boxed value, a user function's `object` result -- and is
//! never released here: named slots and function locals carry borrowed or
//! moved pointers with no reference-count traffic of their own, so
//! releasing one would underflow a reference the slot still uses. An
//! unlisted node therefore defaults to the old leak, never to a
//! use-after-free.
//!
//! **The exception edge.** An operand that is held while a later sibling
//! or the consuming operation itself can fail sits on
//! `ExceptionCodegenState::pending_object_releases` for exactly that
//! window. Both ways out of a statement on a failure -- the post-node guard
//! (`exception::guard_statement_effects`, through
//! `exception::jump_to_exception_target`) and a foreign failure edge
//! (`foreign_fail::emit_failure`, including its direct
//! `EXT_MODULE_EXEC_FAILED` return) -- release a snapshot of that stack
//! before they leave, exactly as they already release
//! `pending_int_releases` (#638, D-208). The fallthrough pops the entry and
//! releases it once, after the operation, so no path releases twice.
//!
//! A reference the operation **consumes** -- the bound method
//! `pycc_ext_obj_call` releases, or a produced callee the consuming call
//! helper takes -- is held across the argument evaluations, where an
//! argument's failure would orphan it, and then [`Held::consumed`] pops it
//! without a release before the call that takes it over.
//!
//! An empty stack costs nothing: every guard site keeps its two-block
//! shape, so a function that performs no object operation (the D-084/D-140
//! nbody hot loop) emits exactly the code it always did.
//!
//! **Not yet released** (later parts of #1092): a produced value bound to a
//! name or slot, passed to a user function, returned, or boxed; the
//! iterator and per-trip item of `for x in <object>:` and of a
//! comprehension over one, and the comprehension's result; an operand of
//! `print`, an f-string, `hash`, `raise`, a conditional expression or a
//! boolean operator. The *iterable* of such a loop or comprehension is
//! released, right after `iter()` has taken its own reference. The read of
//! a narrowed `object` name
//! (`MirExpr::ObjectUnbox`) needs no release: its operand is always a
//! borrowed slot.

use super::*;
use crate::ext::EXT_OBJ_RELEASE_SYMBOL;
use inkwell::builder::Builder;

/// One held reference: the pointer and the release helper declared in the
/// module it was emitted into.
///
/// The helper travels with the pointer because the unwind
/// (`exception::jump_to_exception_target`) has no module in scope to
/// declare it from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct PendingObject<'ctx> {
    pointer: PointerValue<'ctx>,
    release: FunctionValue<'ctx>,
}

/// A produced operand on `pending_object_releases`, or nothing when the
/// operand is a borrow. Every `Held` is retired exactly once, by
/// [`Held::release`] or [`Held::consumed`], before the statement ends.
#[must_use = "a held operand must be released or consumed, or the pending stack drifts"]
pub(super) struct Held<'ctx>(Option<PendingObject<'ctx>>);

/// Whether `expr` evaluates to a new reference that nothing else holds.
///
/// An allowlist of the shim's own producers; see the module doc for why an
/// unlisted node is treated as a borrow. A comparison is a producer only
/// when it is a rich comparison: `is`/`is not` answers a native `bool`.
pub(super) fn is_produced(expr: &MirExpr) -> bool {
    match expr {
        MirExpr::ObjAttrGet { .. }
        | MirExpr::ObjMethodCall { .. }
        | MirExpr::ObjCall { .. }
        | MirExpr::ObjKeywordCall(_)
        | MirExpr::ObjSubscript { .. }
        | MirExpr::ObjType { .. }
        | MirExpr::ObjSlice { .. }
        | MirExpr::ObjList { .. }
        | MirExpr::ObjUnpack { .. } => true,
        MirExpr::ObjCompare { .. } => expr.ty() == pycc_mir::Ty::Object,
        _ => false,
    }
}

/// The node an operand's value really comes from:
/// `object_unbox::emit_pack_operand` evaluates the object inside an
/// `ObjectUnbox` rather than the unboxed read, so a held operand must be
/// classified the same way.
fn operand_source(expr: &MirExpr) -> &MirExpr {
    match expr {
        MirExpr::ObjectUnbox(object, _) => object,
        other => other,
    }
}

/// Declares `void pycc_ext_obj_release(PyObject *)` once per module.
fn release_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    crate::foreign_pack::shim_fn(
        module,
        EXT_OBJ_RELEASE_SYMBOL,
        context.void_type().fn_type(
            &[context.ptr_type(inkwell::AddressSpace::default()).into()],
            false,
        ),
    )
}

/// The pending entry for `scalar`, evaluated from `expr`, when it is a
/// produced object.
fn produced<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
    expr: &MirExpr,
    scalar: &Scalar<'ctx>,
) -> Option<PendingObject<'ctx>> {
    match scalar {
        Scalar::Object(pointer) if is_produced(operand_source(expr)) => Some(PendingObject {
            pointer: *pointer,
            release: release_fn(context, module),
        }),
        _ => None,
    }
}

fn emit_release<'ctx>(builder: &Builder<'ctx>, object: PendingObject<'ctx>) {
    builder
        .build_call(object.release, &[object.pointer.into()], "obj_release")
        .expect("build_call should not fail for pycc_ext_obj_release");
}

/// Holds `scalar`, the already-evaluated operand `expr`, on the pending
/// stack when it is a produced object, so that every failure edge emitted
/// until the matching [`Held`] is retired releases it.
pub(super) fn hold<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    expr: &MirExpr,
    scalar: &Scalar<'ctx>,
) -> Held<'ctx> {
    let pending = produced(context, module, expr, scalar);
    if let Some(object) = pending {
        rt.exceptions
            .pending_object_releases
            .borrow_mut()
            .push(object);
    }
    Held(pending)
}

/// Holds `pointer`, a reference the shim just produced (a bound method
/// from `foreign_call::emit_lookup`), without a MIR node to classify.
pub(super) fn hold_new_reference<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    pointer: PointerValue<'ctx>,
) -> Held<'ctx> {
    let object = PendingObject {
        pointer,
        release: release_fn(context, module),
    };
    rt.exceptions
        .pending_object_releases
        .borrow_mut()
        .push(object);
    Held(Some(object))
}

impl<'ctx> Held<'ctx> {
    /// Removes this entry from the stack. Usually the top one, but not
    /// always: a call's bound method is retired before the arguments held
    /// above it, which stay held across the call that consumes it.
    fn pop(&self, rt: &RtFns<'ctx>) {
        if let Some(object) = self.0 {
            let mut stack = rt.exceptions.pending_object_releases.borrow_mut();
            let position = stack.iter().rposition(|entry| *entry == object).expect(
                "pycc_codegen: internal error: a held object is missing from \
                 pending_object_releases",
            );
            stack.remove(position);
        }
    }

    /// Retires the hold on the fallthrough path and releases the operand,
    /// now that the operation borrowing it is done.
    pub(super) fn release(self, builder: &Builder<'ctx>, rt: &RtFns<'ctx>) {
        self.pop(rt);
        if let Some(object) = self.0 {
            emit_release(builder, object);
        }
    }

    /// Retires the hold without a release: the operation emitted next takes
    /// the reference over and releases it on every path itself.
    pub(super) fn consumed(self, rt: &RtFns<'ctx>) {
        self.pop(rt);
    }
}

/// Releases `scalar`, evaluated from `expr`, at once when it is a produced
/// object -- for a consumer whose own operation cannot fail after the value
/// exists (a discarded statement value), so no hold is needed.
pub(super) fn release_if_produced<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    expr: &MirExpr,
    scalar: &Scalar<'ctx>,
) {
    if let Some(object) = produced(context, module, expr, scalar) {
        emit_release(builder, object);
    }
}

/// Emits a release of every currently held operand, innermost first, at
/// the builder's position -- the unwind half of the protocol, emitted on a
/// block that is about to leave the statement for an exception target or
/// the module-exec failure return. The stack itself is untouched: the
/// fallthrough still owns each entry's pop.
pub(super) fn release_pending<'ctx>(builder: &Builder<'ctx>, rt: &RtFns<'ctx>) {
    let snapshot: Vec<PendingObject<'ctx>> = rt.exceptions.pending_object_releases.borrow().clone();
    for object in snapshot.into_iter().rev() {
        emit_release(builder, object);
    }
}

#[cfg(test)]
#[path = "object_release_tests.rs"]
mod tests;
