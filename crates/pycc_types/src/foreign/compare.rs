//! Comparisons, identity tests and `isinstance` with a CPython object
//! operand (Part 1 of #1371).
//!
//! The rules, each matching CPython 3.14 for what it admits:
//!
//! * **Identity** (`is`/`is not`) is admitted between two objects and
//!   between an object and the `None` literal; it is pointer identity and
//!   cannot raise, so the result is `bool`. An object against any other
//!   native value is refused (`I0404`): pycc would have to box the native
//!   value into a fresh object, whose identity CPython's program never
//!   compared against. Two native non-`None` operands keep the `C0001` the
//!   HIR gate gives a literal operand (`pycc_hir::compare_chain`).
//! * **Rich comparison** (`==`, `!=`, `<`, `<=`, `>`, `>=`) is admitted
//!   when one operand is an object and the other is an object or one of the
//!   four packable scalars (`int`, `float`, `bool`, `str`). Its type is the
//!   **object** CPython's `PyObject_RichCompare` returns, not `bool`: the
//!   comparison's own result is an arbitrary object (an array, say, or one
//!   whose `__bool__` raises), and a truth context truth-tests it exactly as
//!   CPython does. A `-> bool` return or a `bool` annotation therefore
//!   refuses it with the ordinary assignability diagnostic; `bool(...)`
//!   converts it explicitly.
//! * **A chained comparison** with an object operand is refused: its
//!   short-circuit over raising, object-valued links is a later part of the
//!   #1371 series.
//! * **`isinstance(o, C)`** with an object `o` is a run-time
//!   `PyObject_IsInstance` (`pycc_mir`'s `MirExpr::ObjIsInstance`), never
//!   the compile-time fold every other first argument gets. `C` must be an
//!   object-typed expression (a foreign class such as `mod.Cls`) or one of
//!   the builtin type names `int`, `float`, `bool`, `str`. A pycc class has no CPython
//!   type object compiled code can reach yet, and a tuple of classes has no
//!   lowering yet; both are refused.

use super::object_operation_unsupported;
use crate::Environment;
use crate::infer_expr_in;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{CmpOpKind, HirExpr, Ty, is_builtin_type_name};

/// Whether `ty` can sit opposite a CPython object in a rich comparison:
/// another object, or a scalar one of the `pycc_ext_obj_pack_*` helpers
/// boxes.
fn is_comparable_with_object(ty: &Ty) -> bool {
    matches!(ty, Ty::Object | Ty::Int | Ty::Float | Ty::Bool | Ty::Str)
}

/// Types an `is`/`is not` between two operands neither of which is the
/// `None` literal (see the module doc).
pub(crate) fn general_identity_ty(
    op: CmpOpKind,
    left_ty: &Ty,
    right_ty: &Ty,
) -> Result<Ty, Diagnostic> {
    match (left_ty, right_ty) {
        (Ty::Object, Ty::Object) => Ok(Ty::Bool),
        (Ty::Object, other) | (other, Ty::Object) => Err(object_operation_unsupported(&format!(
            "testing a CPython object's identity against a `{}` value",
            other.name()
        ))),
        _ => Err(Diagnostic::error(
            "C0001",
            format!("comparison operator not supported yet: {op:?}"),
            Span::new(0, 0),
        )),
    }
}

/// Types a rich comparison when either operand is a CPython object, and
/// answers `None` when neither is, so the caller's native rules apply.
pub(crate) fn rich_compare_ty(left_ty: &Ty, right_ty: &Ty) -> Option<Result<Ty, Diagnostic>> {
    let other = match (left_ty, right_ty) {
        (Ty::Object, other) | (other, Ty::Object) => other,
        _ => return None,
    };
    Some(if is_comparable_with_object(other) {
        Ok(Ty::Object)
    } else {
        Err(object_operation_unsupported(&format!(
            "comparing a CPython object with a `{}` value",
            other.name()
        )))
    })
}

/// `Err(I0404)` when any operand of a chained comparison is an object.
pub(crate) fn reject_object_in_chain(operand_tys: &[Ty]) -> Result<(), Diagnostic> {
    if operand_tys.iter().any(|ty| matches!(ty, Ty::Object)) {
        return Err(object_operation_unsupported(
            "a chained comparison with a CPython object operand",
        ));
    }
    Ok(())
}

/// Types `isinstance(o, class_arg)` once `o` is known to be an object (see
/// the module doc for what `class_arg` may be).
pub(crate) fn check_object_isinstance(
    env: &Environment,
    local_names: &[&str],
    class_arg: &HirExpr,
) -> Result<Ty, Diagnostic> {
    if let HirExpr::TupleLiteral(_) = class_arg {
        return Err(object_operation_unsupported(
            "testing a CPython object with `isinstance` against a tuple of classes",
        ));
    }
    if let HirExpr::Name(name) = class_arg {
        if is_builtin_type_name(name) {
            return Ok(Ty::Bool);
        }
        if env.lookup_class(name).is_some() {
            return Err(object_operation_unsupported(&format!(
                "testing a CPython object with `isinstance` against the pycc class `{name}`"
            )));
        }
    }
    match infer_expr_in(env, local_names, class_arg)? {
        Ty::Object => Ok(Ty::Bool),
        other => Err(object_operation_unsupported(&format!(
            "testing a CPython object with `isinstance` against a `{}` value",
            other.name()
        ))),
    }
}

#[cfg(test)]
mod tests;
