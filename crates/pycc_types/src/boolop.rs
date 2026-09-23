//! `and`/`or` typing (#1211, Part 3 of #1018).
//!
//! HIR lowering already decided each node's context (`pycc_hir::boolop`):
//!
//! * a **truth-context** node (an `if`/`elif`/`while` test, a comprehension
//!   filter, the operand of `not`) is typed `bool`, and each operand only has
//!   to be truth-testable;
//! * a **value-context** node yields the selected operand, so its type is the
//!   join [`bool_op_result_ty`] computes, which `pycc_mir` recomputes with the
//!   same function. A pair with no common type is refused with `T0021`: pycc
//!   has no union types.
//!
//! In both contexts every operand must pass [`admit_operand`], so a boolean
//! operator never admits an operand `not` refuses, and additionally refuses
//! two operand kinds `not` still admits today:
//!
//! * an instance whose class, or any class in its MRO, defines `__bool__` or
//!   `__len__`. Codegen's `truthy` treats every instance as truthy, so `and`/
//!   `or` would pick the wrong operand. (A subclass that adds the dunder
//!   behind a base-typed value is unreachable today: an upcast is refused at
//!   the call.)
//! * the opaque CPython object (`I0404`): `if obj:` is admitted, but joining a
//!   foreign object as a value has no representation yet.
//!
//! Value context also refuses a `None`-typed operand, which has no value
//! pycc can join.

use crate::Environment;
use crate::expr::infer_expr_in;
use crate::foreign::object_operation_unsupported;
use crate::unop::is_truth_testable;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{BoolOpKind, HirExpr, Ty, bool_op_result_ty};

/// The dunders whose presence makes an instance's truth value
/// class-defined rather than the constant `True` codegen assumes.
const TRUTH_DUNDERS: [&str; 2] = ["__bool__", "__len__"];

/// Types one `and`/`or` node (see the module doc comment).
pub(crate) fn infer_bool_op(
    env: &Environment,
    local_names: &[&str],
    op: BoolOpKind,
    left: &HirExpr,
    right: &HirExpr,
    truth_only: bool,
) -> Result<Ty, Diagnostic> {
    let left_ty = infer_expr_in(env, local_names, left)?;
    admit_operand(env, op, &left_ty, truth_only)?;
    let right_ty = infer_expr_in(env, local_names, right)?;
    admit_operand(env, op, &right_ty, truth_only)?;
    if truth_only {
        return Ok(Ty::Bool);
    }
    bool_op_result_ty(op, &left_ty, &right_ty)
        .ok_or_else(|| no_common_type(op, &left_ty, &right_ty))
}

/// `Ok(())` when a value of type `ty` may be an operand of `op` in the given
/// context.
fn admit_operand(
    env: &Environment,
    op: BoolOpKind,
    ty: &Ty,
    truth_only: bool,
) -> Result<(), Diagnostic> {
    if matches!(ty, Ty::Object) {
        return Err(object_operation_unsupported(&format!(
            "using a CPython object as an `{}` operand",
            op.as_str()
        )));
    }
    if !is_truth_testable(ty) {
        return Err(operand_error(format!(
            "`{}` operand of type `{}` has no truth value pycc can test",
            op.as_str(),
            ty.name()
        )));
    }
    if !truth_only && matches!(ty, Ty::None) {
        return Err(operand_error(format!(
            "`{}` operand `None` has no value pycc can join (pycc has no union types)",
            op.as_str()
        )));
    }
    if let Ty::Instance(class_name) = ty
        && let Some((owner, dunder)) = class_truth_dunder(env, class_name)
    {
        return Err(operand_error(format!(
            "`{}` operand of type `{class_name}` is not supported: class `{owner}` defines \
             `{dunder}`, and pycc does not call it for a truth test yet",
            op.as_str()
        )));
    }
    Ok(())
}

/// The first class in `class_name`'s MRO that defines a truth dunder, with
/// that dunder's name. `None` for a class pycc has no definition of (a
/// monomorphized generic instance's own name) and for a class with neither.
fn class_truth_dunder(env: &Environment, class_name: &str) -> Option<(String, &'static str)> {
    let class_def = env.lookup_class(class_name)?;
    class_def.mro.iter().find_map(|mro_class| {
        let mro_def = env.lookup_class(mro_class)?;
        TRUTH_DUNDERS
            .into_iter()
            .find(|dunder| mro_def.methods.iter().any(|(name, _)| name == dunder))
            .map(|dunder| (mro_class.clone(), dunder))
    })
}

fn no_common_type(op: BoolOpKind, left: &Ty, right: &Ty) -> Diagnostic {
    operand_error(format!(
        "`{}` operands have no common type: {} and {} (pycc has no union types)",
        op.as_str(),
        left.name(),
        right.name()
    ))
}

fn operand_error(message: String) -> Diagnostic {
    Diagnostic::error("T0021", message, Span::new(0, 0))
}
