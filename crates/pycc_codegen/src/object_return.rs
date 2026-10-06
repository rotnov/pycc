//! `None` returned from a function whose declared return type is the
//! opaque CPython `object` (#1387, D-258's return-position amendment).
//!
//! `pycc_types` admits exactly two spellings into an `object` return slot
//! that would otherwise be `T0022`: a bare `return` and `return None` (the
//! literal). Both must hand the caller CPython's `None`, but the MIR carries
//! them as `MirStmt::Return(None)` and `MirStmt::Return(Some(NoneLiteral))`,
//! and neither generic path produces a `PyObject *`: the bare form emits the
//! type's zero default (a null pointer, which the export wrapper reports as
//! `SystemError`), and `NoneLiteral` emits the native `Optional` carrier.
//!
//! [`object_return_value`] folds the bare form into the literal one, and
//! [`object_return_none`] turns the literal into
//! [`foreign_pack::none_pointer`]'s borrowed `Py_None`. Borrowed is the
//! leak-only ownership model's convention for a compiled body
//! (`docs/RUNTIME.md`): the export wrapper's `pycc_ext_pack_object` takes
//! the new reference the host receives, exactly as it does for a borrowed
//! object parameter returned by name.

use super::*;
use crate::foreign_pack::none_pointer;

/// The value a `return` statement hands back: `value` itself, except that a
/// bare `return` in an `object`-returning function is `return None`, spelled
/// as `bare_none` (a caller-owned `MirExpr::NoneLiteral`).
pub(crate) fn object_return_value<'a>(
    expected_return_ty: &pycc_mir::Ty,
    value: &'a Option<MirExpr>,
    bare_none: &'a MirExpr,
) -> Option<&'a MirExpr> {
    match value {
        None if *expected_return_ty == pycc_mir::Ty::Object => Some(bare_none),
        other => other.as_ref(),
    }
}

/// CPython's `None` as the returned object when `expr` is the `None` literal
/// and the function returns `object`; `None` (emitting nothing) otherwise,
/// so the caller evaluates `expr` the ordinary way.
pub(crate) fn object_return_none<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    expected_return_ty: &pycc_mir::Ty,
    expr: &MirExpr,
) -> Option<Scalar<'ctx>> {
    (*expected_return_ty == pycc_mir::Ty::Object && matches!(expr, MirExpr::NoneLiteral))
        .then(|| Scalar::Object(none_pointer(context, builder, module)))
}
