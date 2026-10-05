//! Emission for `MirExpr::ObjCompare` and `MirExpr::ObjIsInstance` (Part 1
//! of #1371): comparisons, identity tests and `isinstance` with a CPython
//! object operand.
//!
//! **Identity** (`is`/`is not`) is a pointer compare and emits no shim
//! call except to name CPython's `None` ([`EXT_OBJ_NONE_SYMBOL`], a
//! borrowed pointer to an immortal object). It cannot fail, so it has no
//! failure edge, and `exception::expression_can_set_exception` answers
//! `false` for it.
//!
//! **A rich comparison** is `PyObject_RichCompare` behind
//! [`EXT_OBJ_RICHCOMPARE_SYMBOL`]. A scalar operand is boxed by the same
//! `pycc_ext_obj_pack_*` helper a method-call argument uses
//! (`foreign_call::packer_for`), and the shim helper consumes every packed
//! operand on every path -- the packer contract `foreign_call.rs` records --
//! so a failed packer needs no edge of its own and the operation has
//! exactly one: `foreign_fail.rs`'s, on a `NULL` result. The result is a
//! *new* reference that is deliberately never released, on the leak-only
//! rule `docs/RUNTIME.md` records for this boundary; a truth context tests
//! it through `foreign_len::emit_truthy` like any other object.
//!
//! **`isinstance`** is `PyObject_IsInstance` behind
//! [`EXT_OBJ_ISINSTANCE_SYMBOL`], whose `-1` takes the same failure edge.

use super::*;
use crate::foreign_attr::expect_object_pointer;
use crate::foreign_call::{packer_for, shim_fn};
use crate::foreign_fail::{ForeignFailEdge, route_negative, route_null};
use inkwell::builder::Builder;
use inkwell::values::PointerValue;
use pycc_mir::CmpOpKind;

/// CPython's rich-comparison selector (`Py_LT` .. `Py_GE`) for `op`, or
/// `None` for an identity test, which is no rich comparison at all.
fn rich_compare_selector(op: CmpOpKind) -> Option<u64> {
    match op {
        CmpOpKind::Lt => Some(0),
        CmpOpKind::LtE => Some(1),
        CmpOpKind::Eq => Some(2),
        CmpOpKind::NotEq => Some(3),
        CmpOpKind::Gt => Some(4),
        CmpOpKind::GtE => Some(5),
        CmpOpKind::Is | CmpOpKind::IsNot => None,
    }
}

/// A borrowed pointer to CPython's `None`.
fn none_pointer<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
) -> PointerValue<'ctx> {
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let none_fn = shim_fn(module, EXT_OBJ_NONE_SYMBOL, ptr.fn_type(&[], false));
    builder
        .build_call(none_fn, &[], "foreign_none")
        .expect("build_call should not fail for pycc_ext_obj_none")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_none returns PyObject *")
        .into_pointer_value()
}

/// The `PyObject *` for one operand, and whether a packer produced it (and
/// so the shim helper owns it). `None` is the unevaluated `None` literal of
/// an identity test.
fn operand_pointer<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    operand: Option<Scalar<'ctx>>,
) -> (PointerValue<'ctx>, bool) {
    match operand {
        None => (none_pointer(context, builder, module), false),
        Some(Scalar::Object(pointer)) => (pointer, false),
        Some(scalar) => {
            let ptr = context.ptr_type(inkwell::AddressSpace::default());
            let (symbol, value) = packer_for(scalar);
            let packer = shim_fn(
                module,
                symbol,
                ptr.fn_type(&[value.get_type().into()], false),
            );
            let packed = builder
                .build_call(packer, &[value.into()], "foreign_compare_operand")
                .unwrap_or_else(|_| panic!("build_call should not fail for {symbol}"))
                .try_as_basic_value()
                .expect_basic("a pycc_ext_obj_pack_* helper returns PyObject *")
                .into_pointer_value();
            (packed, true)
        }
    }
}

/// Emits `left op right` once both operands are evaluated. `None` stands
/// for an unevaluated `None` literal (identity tests only).
pub(super) fn emit_compare<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    op: CmpOpKind,
    left: Option<Scalar<'ctx>>,
    right: Option<Scalar<'ctx>>,
) -> Scalar<'ctx> {
    let Some(selector) = rich_compare_selector(op) else {
        let (l, _) = operand_pointer(context, builder, module, left);
        let (r, _) = operand_pointer(context, builder, module, right);
        let predicate = if op == CmpOpKind::Is {
            inkwell::IntPredicate::EQ
        } else {
            inkwell::IntPredicate::NE
        };
        let same = builder
            .build_int_compare(predicate, l, r, "foreign_identity")
            .expect("build_int_compare should not fail on two pointers");
        let as_bool = builder
            .build_int_z_extend(same, context.i8_type(), "foreign_identity_bool")
            .expect("build_int_z_extend should not fail");
        return Scalar::Bool(as_bool);
    };
    let edge = ForeignFailEdge::for_current(builder);
    let (l, l_owned) = operand_pointer(context, builder, module, left);
    let (r, r_owned) = operand_pointer(context, builder, module, right);
    let owned = u64::from(l_owned) | (u64::from(r_owned) << 1);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let i32_type = context.i32_type();
    let richcompare = shim_fn(
        module,
        EXT_OBJ_RICHCOMPARE_SYMBOL,
        ptr.fn_type(
            &[ptr.into(), ptr.into(), i32_type.into(), i32_type.into()],
            false,
        ),
    );
    let result = builder
        .build_call(
            richcompare,
            &[
                l.into(),
                r.into(),
                i32_type.const_int(selector, false).into(),
                i32_type.const_int(owned, false).into(),
            ],
            "foreign_compare",
        )
        .expect("build_call should not fail for pycc_ext_obj_richcompare")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_richcompare returns PyObject *")
        .into_pointer_value();
    route_null(
        context,
        builder,
        module,
        rt,
        edge,
        result,
        "foreign_compare",
    );
    Scalar::Object(result)
}

/// The evaluated class operand of an `isinstance` test.
pub(super) enum IsInstanceClass<'ctx> {
    /// A builtin class, by its `pycc_ext_obj_isinstance` selector.
    Builtin(u64),
    /// An evaluated object-typed class expression.
    Object(Scalar<'ctx>),
}

/// Emits `isinstance(value, class)` once both operands are evaluated.
pub(super) fn emit_isinstance<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    value: Scalar<'ctx>,
    class: IsInstanceClass<'ctx>,
) -> Scalar<'ctx> {
    let edge = ForeignFailEdge::for_current(builder);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let i32_type = context.i32_type();
    let value_ptr = expect_object_pointer(value);
    let (class_ptr, builtin) = match class {
        IsInstanceClass::Builtin(selector) => (ptr.const_null(), selector),
        IsInstanceClass::Object(class) => (expect_object_pointer(class), 0),
    };
    let isinstance = shim_fn(
        module,
        EXT_OBJ_ISINSTANCE_SYMBOL,
        i32_type.fn_type(&[ptr.into(), ptr.into(), i32_type.into()], false),
    );
    let status = builder
        .build_call(
            isinstance,
            &[
                value_ptr.into(),
                class_ptr.into(),
                i32_type.const_int(builtin, false).into(),
            ],
            "foreign_isinstance",
        )
        .expect("build_call should not fail for pycc_ext_obj_isinstance")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_isinstance returns int")
        .into_int_value();
    route_negative(
        context,
        builder,
        module,
        rt,
        edge,
        status,
        "foreign_isinstance",
    );
    let as_bool = builder
        .build_int_truncate(status, context.i8_type(), "foreign_isinstance_bool")
        .expect("build_int_truncate should not fail");
    Scalar::Bool(as_bool)
}
