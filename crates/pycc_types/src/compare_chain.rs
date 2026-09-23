//! Comparison typing shared by a single comparison and a chained
//! comparison `a < b < c` (#1212, Part 4 of #1018).
//!
//! A chain admits exactly what a single comparison admits, link by link:
//! each adjacent operand pair is typed by [`compare_link_ty`] exactly as
//! `a op b` would be, and the chain's result is `bool`.

use crate::Environment;
use crate::expr::infer_expr_in;
use crate::numeric_or_bool_compatible;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{CmpOpKind as CmpOp, CompareLink, HirExpr, Ty};

/// Types one comparison link `left op right` whose operand types are
/// already inferred. The `is`/`is not` arm needs the left operand
/// expression, not only its type, because it keys on whether that side is
/// the `None` literal.
pub(crate) fn compare_link_ty(
    env: &Environment,
    op: CmpOp,
    left: &HirExpr,
    left_ty: &Ty,
    right_ty: &Ty,
) -> Result<Ty, Diagnostic> {
    // `is`/`is not` (D-197, #763, Part 1 of #747): HIR lowering
    // (`crates/pycc_hir/src/expr.rs`'s `Expr::Compare` arm) already
    // guarantees one operand is syntactically `HirExpr::NoneLiteral`
    // whenever `op` is `Is`/`IsNot` -- this is the type-level half
    // of that scoping: the *other* operand's static type must be
    // `Ty::Optional(_)` or `Ty::None` itself. Every other `is`/`is
    // not` shape never reaches this arm at all (still `C0001` at
    // HIR-lowering), so this deliberately does not implement
    // general object-identity comparison.
    if matches!(op, CmpOp::Is | CmpOp::IsNot) {
        let other_ty = if matches!(left, HirExpr::NoneLiteral) {
            &right_ty
        } else {
            &left_ty
        };
        return match other_ty {
            Ty::Optional(_) | Ty::None => Ok(Ty::Bool),
            other => Err(Diagnostic::error(
                "T0021",
                format!(
                    "cannot compare `{}` and `None` with `is`/`is not` -- only an `Optional[T]` (or `None`) operand is supported",
                    other.name()
                ),
                Span::new(0, 0),
            )),
        };
    }
    // #378 (PR-18): `==`/`!=` between same-class dataclass instances
    // is accepted -- the compiler-synthesized `__eq__` method has a
    // known-correct signature `(self, other: SameClass) -> bool`.
    // This is restricted to dataclass classes (not any class with a
    // user-defined `__eq__`) because the MIR rewrite assumes the
    // synthesized signature; a user-defined `__eq__` with wrong
    // arity or return type would reach codegen and panic. Ordering
    // operators (`<`, `<=`, `>`, `>=`) between instances are always
    // rejected with T0021 -- pycc has no `__lt__`/`__le__`/`__gt__`/
    // `__ge__` dispatch. Different-class comparisons also stay T0021.
    if matches!(op, CmpOp::Eq | CmpOp::NotEq)
        && let (Ty::Instance(left_class), Ty::Instance(right_class)) = (left_ty, right_ty)
        && left_class == right_class
        && let Some(class_def) = env.lookup_class(left_class)
        && class_def.is_dataclass
    {
        return Ok(Ty::Bool);
    }
    if numeric_or_bool_compatible(left_ty.clone(), right_ty.clone()) {
        Ok(Ty::Bool)
    } else {
        Err(Diagnostic::error(
            "T0021",
            format!(
                "cannot compare `{}` and `{}`",
                left_ty.name(),
                right_ty.name()
            ),
            Span::new(0, 0),
        ))
    }
}

/// Types a [`HirExpr::CompareChain`]: every operand is inferred once, left
/// to right, then each adjacent pair is typed by [`compare_link_ty`]. The
/// first failing link's diagnostic wins, and the result is `bool`.
pub(crate) fn infer_compare_chain(
    env: &Environment,
    local_names: &[&str],
    first: &HirExpr,
    links: &[CompareLink],
) -> Result<Ty, Diagnostic> {
    let mut operands = Vec::with_capacity(links.len() + 1);
    operands.push((first, infer_expr_in(env, local_names, first)?));
    for link in links {
        operands.push((&link.right, infer_expr_in(env, local_names, &link.right)?));
    }
    for (index, link) in links.iter().enumerate() {
        let (left, left_ty) = &operands[index];
        let (_, right_ty) = &operands[index + 1];
        compare_link_ty(env, link.op, left, left_ty, right_ty)?;
    }
    Ok(Ty::Bool)
}
