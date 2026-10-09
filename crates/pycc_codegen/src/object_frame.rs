//! Function-frame `object` slots own their reference (Part 2 of #1499,
//! [#1502](https://github.com/rotnov/pycc/issues/1502)).
//!
//! **The invariant.** An `object` parameter or local of a compiled function
//! -- including a synthetic `ObjUnpack` temporary -- holds exactly one owned
//! `PyObject *` reference, or null. The slots are registered when the
//! function's entry block allocates them ([`register`]); the same list
//! feeds the scope-exit epilogue's releases ([`release_slots`]), so a slot
//! is registered exactly when the epilogue releases it.
//!
//! - **Entry.** A local slot starts null (`storage_slot_at_entry`), so a
//!   path that never binds it releases nothing. A parameter slot receives
//!   the argument, which is an owned reference by the calling convention
//!   below.
//! - **Rebind.** [`assign`] retains a borrowed source
//!   (`object_slot::retain_if_borrowed`), loads the old value, stores the
//!   new one and raises the `initialized` flag, then releases the old value:
//!   the `Py_XSETREF` order, so a finalizer the release runs never sees a
//!   freed pointer in the slot, and `x = x` is correct. There is no
//!   activation gate (unlike a module global, `object_slot.rs`): no other
//!   activation can name a frame slot, and every borrowed copy this
//!   activation made of the old value -- an operand of the statement that
//!   rebinds it -- is dead once that statement's value exists.
//! - **Exit.** The owned-slot epilogue releases every registered slot on
//!   every way out, the normal returns and the exception exit alike.
//!
//! **The calling convention (caller-incref).** Every `object` argument to a
//! compiled function is an owned reference the callee's parameter slot
//! takes over ([`owned_argument`]): a produced value or a boxed native value
//! moves; a borrowed one (a name read, a global, a boxed `None`, which is
//! CPython's borrowed `Py_None`) is retained first. Each owned argument is
//! held on `pending_object_releases` until every later argument has been
//! evaluated, so an exception in a later argument releases it, and is
//! retired without a release before the call. The host side matches: the
//! export wrapper's `pycc_ext_unpack_object` hands over a new reference,
//! and its bail path releases the earlier object arguments when a later
//! one fails to unpack.
//!
//! **Returns are owned.** An `object` return value is a new reference the
//! caller owns ([`owned_return`]): a produced value moves, anything else is
//! retained. A compiled caller therefore classifies an `object`-typed
//! `MirExpr::Call` as produced (`object_release::is_produced`), and the
//! export wrapper's `pycc_ext_pack_object` steals the reference.
//!
//! **Accepted leaks.** A `return` whose value a raising `finally` abandons,
//! or a `finally` overrides with a `return` of its own, leaks that value:
//! the exception exit stores a null carrier into the pending return slot
//! and the override replaces it, so the epilogue never sees the first one.
//! A leak, never a use-after-free.

use super::*;
use crate::ext::EXT_OBJ_RELEASE_SYMBOL;
use crate::object_release::Held;
use inkwell::builder::Builder;

/// Makes frame `object` slots own their reference for the module being
/// emitted. Called only when the module is compiled for the CPython host
/// (`CompileOptions::ext`: an `--ext` artifact or an embedded executable,
/// both of which link the shim): a fully native executable has no host to
/// provide `pycc_ext_obj_retain`/`pycc_ext_obj_release`, and no
/// `object` value ever reaches one of its frames (a type-variable parameter
/// is never handed a native value, `docs/TYPE_SYSTEM.md`, "Generics"), so
/// there every helper below keeps the pass-through it had before this part.
pub(super) fn enable(rt: &RtFns<'_>) {
    rt.object_slots.frame_owned.set(true);
}

/// Whether frame `object` slots own their reference ([`enable`]).
pub(super) fn enabled(rt: &RtFns<'_>) -> bool {
    rt.object_slots.frame_owned.get()
}

/// Records `slot`, an `object` slot the function being emitted owns, as a
/// frame slot, and answers whether it did -- so the caller adds it to the
/// epilogue's release list -- which is exactly when frame slots own their
/// reference ([`enabled`]).
pub(super) fn register<'ctx>(rt: &RtFns<'ctx>, slot: PointerValue<'ctx>) -> bool {
    if !enabled(rt) {
        return false;
    }
    rt.object_slots.frame.borrow_mut().insert(slot);
    true
}

/// Whether `slot` is a registered frame `object` slot.
pub(super) fn is_frame_slot<'ctx>(rt: &RtFns<'ctx>, slot: &StorageSlot<'ctx>) -> bool {
    rt.object_slots.frame.borrow().contains(&slot.ptr)
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

/// `MirStmt::Assign`'s store into a frame `object` slot: retains a borrowed
/// `scalar` (evaluated from `value`), stores it, raises the slot's
/// `initialized` flag, and releases the value the slot held before -- see
/// the module doc for the order. Answers `false`, emitting nothing, for
/// every other target.
#[allow(clippy::too_many_arguments)]
pub(super) fn assign<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    target: &str,
    value: &MirExpr,
    scalar: Scalar<'ctx>,
) -> bool {
    let (Some(slot), Scalar::Object(pointer)) = (locals.get(target), scalar) else {
        return false;
    };
    if !is_frame_slot(rt, slot) {
        return false;
    }
    let new = crate::object_slot::retain_if_borrowed(context, builder, module, value, pointer);
    let old = builder
        .build_load(
            context.ptr_type(inkwell::AddressSpace::default()),
            slot.ptr,
            "frame_old",
        )
        .expect("build_load should not fail for a frame slot this function allocated")
        .into_pointer_value();
    builder
        .build_store(slot.ptr, new)
        .expect("build_store should not fail for a frame slot this function allocated");
    if let Some(initialized) = slot.initialized {
        builder
            .build_store(initialized, context.i8_type().const_int(1, false))
            .expect("build_store should not fail for a frame slot flag");
    }
    builder
        .build_call(release_fn(context, module), &[old.into()], "frame_release")
        .expect("build_call should not fail for pycc_ext_obj_release");
    true
}

/// Emits one `pycc_ext_obj_release` per registered frame slot in `slots`,
/// at the builder's position in the owned-slot epilogue. A null slot -- a
/// local no path bound -- is the shim's documented no-op (`Py_XDECREF`).
pub(super) fn release_slots<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    slots: &[PointerValue<'ctx>],
) {
    if slots.is_empty() {
        return;
    }
    let release = release_fn(context, module);
    for slot in slots {
        let live = builder
            .build_load(
                context.ptr_type(inkwell::AddressSpace::default()),
                *slot,
                "object_epilogue_live",
            )
            .expect("build_load should not fail for a frame slot this function allocated")
            .into_pointer_value();
        builder
            .build_call(release, &[live.into()], "object_epilogue_release")
            .expect("build_call should not fail for pycc_ext_obj_release");
    }
}

/// Whether the discarded value of the statement `expr` is an `object` value
/// a native executable must not release: there frame slots own nothing
/// ([`enabled`]), so a compiled function's `object` result is a borrow, and
/// so is a conditional expression or `and`/`or` selecting one, which never
/// retains its other arm there (`object_release::retains_borrowed_arm`).
pub(super) fn is_unowned_discard(rt: &RtFns<'_>, expr: &MirExpr) -> bool {
    !enabled(rt)
        && matches!(
            expr,
            MirExpr::Call { .. } | MirExpr::IfExp { .. } | MirExpr::BoolOp { .. }
        )
}

/// `pointer`, evaluated from `value` (boxed into a new object first when
/// `boxed`), made an owned reference: unchanged when it is already one,
/// retained through `pycc_ext_obj_retain` otherwise. A boxed `None` is
/// CPython's borrowed `Py_None`; every other boxed value is the packer's new
/// reference.
fn owned<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    value: &MirExpr,
    pointer: PointerValue<'ctx>,
    boxed: bool,
) -> PointerValue<'ctx> {
    if boxed {
        if value.ty() == pycc_mir::Ty::None {
            crate::object_slot::retain(context, builder, module, pointer);
        }
        pointer
    } else {
        crate::object_slot::retain_if_borrowed(context, builder, module, value, pointer)
    }
}

/// An `object` argument to a compiled function as the owned reference the
/// callee's parameter slot takes over (see the module doc), held on the
/// pending stack until the caller retires the hold with [`Held::consumed`]
/// just before the call.
#[allow(clippy::too_many_arguments)]
pub(super) fn owned_argument<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    value: &MirExpr,
    pointer: PointerValue<'ctx>,
    boxed: bool,
) -> Held<'ctx> {
    if !enabled(rt) {
        return Held::nothing();
    }
    let pointer = owned(context, builder, module, value, pointer, boxed);
    crate::object_release::hold_new_reference(context, module, rt, pointer)
}

/// The value of a `return` in a function whose return type is
/// `expected_return_ty`: an `object` result is made an owned reference (see
/// the module doc), every other scalar -- and every scalar where frame slots
/// do not own their reference ([`enabled`]) -- is returned unchanged.
#[allow(clippy::too_many_arguments)]
pub(super) fn owned_return<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    expected_return_ty: &pycc_mir::Ty,
    value: &MirExpr,
    boxed: bool,
    scalar: Scalar<'ctx>,
) -> Scalar<'ctx> {
    match scalar {
        Scalar::Object(pointer) if *expected_return_ty == pycc_mir::Ty::Object && enabled(rt) => {
            Scalar::Object(owned(context, builder, module, value, pointer, boxed))
        }
        other => other,
    }
}

#[cfg(test)]
#[path = "object_frame_tests.rs"]
mod tests;
