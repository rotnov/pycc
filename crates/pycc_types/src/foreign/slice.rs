//! Slicing a CPython object, `o[a:b:c]` (Part 2b of #1371).
//!
//! Only the **load** is admitted, exactly as for `o[k]`: `del o[a:b]` and
//! `o[a:b] = v` are separate HIR shapes refused before this crate. Each
//! present bound must be one of the packable operands
//! ([`super::is_packable_operand`]: `int`, `float`, `bool`, `str` or another
//! object), because codegen boxes it through a `pycc_ext_obj_pack_*` helper
//! into the `slice` object CPython's `PySlice_New` builds; an absent bound
//! is CPython's `None`, as in `o[:b]`. Any other bound type -- a container,
//! an instance or `None` itself -- is refused with
//! [`super::object_operation_unsupported`].
//!
//! The result is [`Ty::Object`]: whatever the object's own `__getitem__`
//! answers for a `slice` key. A bound CPython's `__getitem__` would refuse
//! (a `float` bound on a list, say) is admitted here and raises CPython's
//! own `TypeError` at run time, the same split `o[k]` makes for its key.

use super::{is_packable_operand, object_operation_unsupported};
use crate::Environment;
use crate::infer_expr_in;
use pycc_diag::Diagnostic;
use pycc_hir::{HirExpr, Ty};

/// Types `base[start:stop:step]` once `base` is known to be a CPython
/// object, inferring each present bound left to right.
pub(crate) fn object_slice_ty(
    env: &Environment,
    local_names: &[&str],
    bounds: [Option<&HirExpr>; 3],
) -> Result<Ty, Diagnostic> {
    for bound in bounds.into_iter().flatten() {
        let bound_ty = infer_expr_in(env, local_names, bound)?;
        if !is_packable_operand(&bound_ty) {
            return Err(object_operation_unsupported(&format!(
                "slicing a CPython object with a `{}` bound",
                bound_ty.name()
            )));
        }
    }
    Ok(Ty::Object)
}

#[cfg(test)]
mod tests;
