//! Compiled-instance `object` attributes own their reference (Part 4 of
//! #1499, [#1504](https://github.com/rotnov/pycc/issues/1504)).
//!
//! **The invariant.** An instance slot of declared type `object` holds
//! exactly one owned `PyObject *` reference, or is unassigned. The slot
//! words carry no type at run time, and `MirStmt::AttrSet` carries only a
//! slot index, so both sides key on the *value's* type: `pycc_mir` boxes
//! every value stored into an `object` slot (`object_box::box_into`), so a
//! store's value is `object`-typed exactly when the slot is.
//!
//! - **Store** ([`store`]). A borrowed source is retained
//!   (`object_slot::retain_if_borrowed`); the old word is read with the
//!   unchecked `pycc_rt_instance_get_slot` (`0`, a null pointer, for an
//!   unassigned slot); the new word is stored; then the old one is released
//!   through `pycc_ext_obj_release` (`Py_XDECREF`). That is the
//!   `Py_XSETREF` order: a finalizer the release runs may read the
//!   attribute, and it sees the new value, never a freed one. Storing first
//!   also makes `self.a = self.a` correct.
//! - **Read** ([`retain_read`]). An attribute is reachable from every
//!   activation and from the host, so a nested call -- or a host callback
//!   inside any foreign operation -- can rebind it while a borrowed copy of
//!   the old word is still in use (`self.a.m(self.reset())`). The read
//!   therefore returns a new reference, as CPython's `LOAD_ATTR` does: it
//!   retains the word through `pycc_ext_obj_retain` (`Py_XINCREF`, so the
//!   `0` an unassigned slot's raising read answers is a no-op), and
//!   `object_release::is_produced` lists the read, so every consumer holds
//!   and releases it and every binding site moves it.
//! - **Host stores and `del`.** The descriptor setter goes through the
//!   shim's `pycc_ext_instance_store_slot`/`pycc_ext_instance_delete_slot`
//!   (`src/ext/pycc_ext_module.c`), which release a replaced or deleted
//!   `object` word the same way: `pycc_rt` keeps no CPython dependency
//!   (D-244 rule 2).
//!
//! **Native builds.** Both helpers act only in a module compiled for the
//! CPython host -- an `--ext` module or an embedded executable, both of
//! which link the shim (`object_frame::enabled`). A fully native executable
//! still has `object`-typed attributes -- a generic class's `T`-typed field
//! -- but links no retain or release shim and owns no `object` reference,
//! so there the store is a plain slot write and the read a plain borrow,
//! and `object_frame::is_unowned_discard` keeps a discarded read
//! unreleased. (Part 1's store-side retain was unconditional and so broke
//! the native link of `self.v = v` in a generic class; gating it here fixes
//! that too.)
//!
//! A `PyInstanceObj` is never freed (D-107, D-154), so an unreachable
//! instance keeps its slots' references: that is the instance-lifetime
//! policy, not a store leak.

use super::*;
use inkwell::builder::Builder;

/// An attribute read's `scalar`, of declared type `ty`, as the new
/// reference the read answers: an `object` read is retained through
/// `pycc_ext_obj_retain` (see the module doc); every other scalar is
/// returned unchanged.
pub(super) fn retain_read<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> Scalar<'ctx> {
    if !crate::object_frame::enabled(rt) {
        return scalar;
    }
    if let Scalar::Object(pointer) = scalar {
        crate::object_slot::retain(context, builder, module, pointer);
    }
    scalar
}

/// `MirStmt::AttrSet`'s store of the `object` `pointer`, evaluated from
/// `value`, into slot `slot_index` of the instance `base_ptr`, in the order
/// the module doc gives.
#[allow(clippy::too_many_arguments)]
pub(super) fn store<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    base_ptr: PointerValue<'ctx>,
    slot_index: IntValue<'ctx>,
    value: &MirExpr,
    pointer: PointerValue<'ctx>,
) {
    if !crate::object_frame::enabled(rt) {
        let word = builder
            .build_ptr_to_int(pointer, context.i64_type(), "object_attr_word")
            .expect("build_ptr_to_int should not fail reinterpreting a pointer as i64");
        builder
            .build_call(
                rt.instance_set_slot,
                &[base_ptr.into(), slot_index.into(), word.into()],
                "instance_set_slot",
            )
            .expect("build_call should not fail for a well-formed attribute write");
        return;
    }
    let new = crate::object_slot::retain_if_borrowed(context, builder, module, value, pointer);
    let old = builder
        .build_call(
            rt.instance_get_slot,
            &[base_ptr.into(), slot_index.into()],
            "object_attr_old",
        )
        .expect("build_call should not fail for a well-formed attribute read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_instance_get_slot returns a non-void i64")
        .into_int_value();
    let word = builder
        .build_ptr_to_int(new, context.i64_type(), "object_attr_word")
        .expect("build_ptr_to_int should not fail reinterpreting a pointer as i64");
    builder
        .build_call(
            rt.instance_set_slot,
            &[base_ptr.into(), slot_index.into(), word.into()],
            "instance_set_slot",
        )
        .expect("build_call should not fail for a well-formed attribute write");
    let old = builder
        .build_int_to_ptr(
            old,
            context.ptr_type(inkwell::AddressSpace::default()),
            "object_attr_old_ptr",
        )
        .expect("build_int_to_ptr should not fail reinterpreting an i64 as a pointer");
    builder
        .build_call(
            crate::object_frame::release_fn(context, module),
            &[old.into()],
            "object_attr_release",
        )
        .expect("build_call should not fail for pycc_ext_obj_release");
}

#[cfg(test)]
#[path = "object_attr_tests.rs"]
mod tests;
