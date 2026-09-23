//! Chained comparisons `a < b < c` (#1212, Part 4 of #1018): the per-link
//! operator mapping shared with the single comparison, and the lowering of
//! an AST comparison with two or more operators into
//! [`HirExpr::CompareChain`].
//!
//! A chain admits exactly the per-link operators a single comparison
//! admits: `==`, `!=`, `<`, `<=`, `>`, `>=`, and `is`/`is not` when one of
//! *that link's* two operands is syntactically the `None` literal (D-197).
//! `in`/`not in` and general object-identity `is` keep their `C0001`.

use pycc_ast::{CmpOp, Expr, ExprCompare};
use pycc_diag::Diagnostic;

use crate::expr::keyword_bind::SignatureTable;
use crate::expr::{contains_named_expr, lower_expr};
use crate::{CmpOpKind, HirExpr, ImportBinding, unsupported};

/// One `op right` link of a [`HirExpr::CompareChain`]. The link's left
/// operand is the previous link's `right`, or the chain's `first` for link
/// 0.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareLink {
    pub op: CmpOpKind,
    pub right: HirExpr,
}

/// Every operand of a [`HirExpr::CompareChain`] in evaluation order:
/// `first`, then each link's `right`.
pub fn compare_chain_operands<'a>(
    first: &'a HirExpr,
    links: &'a [CompareLink],
) -> impl Iterator<Item = &'a HirExpr> {
    std::iter::once(first).chain(links.iter().map(|link| &link.right))
}

/// Maps one AST comparison operator to its HIR kind, applying the D-197
/// syntactic gate to `is`/`is not`: one of `left` and `right` -- the two
/// operands of this link -- must be the `None` literal. `range` is the
/// whole comparison's range, which every rejection reports.
pub(crate) fn lower_cmp_op(
    op: CmpOp,
    left: &Expr,
    right: &Expr,
    range: std::ops::Range<u32>,
) -> Result<CmpOpKind, Diagnostic> {
    // `is`/`is not` (D-197, #763, Part 1 of #747): this compiler's first
    // support of any kind for either operator, deliberately scoped at this
    // syntactic gate to exactly the case #763 needs -- one operand is
    // literally `Expr::NoneLiteral`. Every other `is`/`is not` use falls
    // through to the `other =>` rejection below. The *type* of the
    // non-`None` operand (must be `Ty::Optional(_)` or `Ty::None`) is
    // `pycc_types`' job, not this lowering step's (D-105).
    let is_none_operand_shape =
        matches!(left, Expr::NoneLiteral(_)) || matches!(right, Expr::NoneLiteral(_));
    Ok(match op {
        CmpOp::Eq => CmpOpKind::Eq,
        CmpOp::NotEq => CmpOpKind::NotEq,
        CmpOp::Lt => CmpOpKind::Lt,
        CmpOp::LtE => CmpOpKind::LtE,
        CmpOp::Gt => CmpOpKind::Gt,
        CmpOp::GtE => CmpOpKind::GtE,
        CmpOp::Is if is_none_operand_shape => CmpOpKind::Is,
        CmpOp::IsNot if is_none_operand_shape => CmpOpKind::IsNot,
        other => {
            return Err(unsupported(
                format!("comparison operator not supported yet: {other:?}"),
                range,
            ));
        }
    })
}

/// Lowers an AST comparison with `ops.len() >= 2` into
/// [`HirExpr::CompareChain`], lowering every operand exactly once, left to
/// right.
///
/// Operands 0 and 1 always run; an operand at index 2 or later runs only
/// when every earlier link was true. A walrus there would bind
/// conditionally, which the binding walkers (`pycc_types`' and `pycc_mir`'s)
/// cannot represent, so it is refused with `C0001` -- the same rule
/// `and`/`or` applies to its later operands.
pub(crate) fn lower_compare_chain(
    cmp: &ExprCompare,
    in_function: bool,
    class_name: Option<&str>,
    imports: &[ImportBinding],
    signatures: &SignatureTable,
) -> Result<HirExpr, Diagnostic> {
    debug_assert!(cmp.ops.len() >= 2);
    let range = std::ops::Range::<u32>::from(cmp.range);
    let mut ops = Vec::with_capacity(cmp.ops.len());
    for (index, op) in cmp.ops.iter().enumerate() {
        let left = if index == 0 {
            cmp.left.as_ref()
        } else {
            &cmp.comparators[index - 1]
        };
        ops.push(lower_cmp_op(
            *op,
            left,
            &cmp.comparators[index],
            range.clone(),
        )?);
    }
    let first = lower_expr(&cmp.left, in_function, class_name, imports, signatures)?;
    let mut links = Vec::with_capacity(ops.len());
    for (index, (op, right)) in ops.into_iter().zip(cmp.comparators.iter()).enumerate() {
        let lowered = lower_expr(right, in_function, class_name, imports, signatures)?;
        if index >= 1 && contains_named_expr(&lowered) {
            return Err(unsupported(
                "a walrus assignment (`:=`) in a short-circuited chained-comparison operand is not supported",
                pycc_ast::expr_range(right),
            ));
        }
        links.push(CompareLink { op, right: lowered });
    }
    Ok(HirExpr::CompareChain {
        first: Box::new(first),
        links,
    })
}

#[cfg(test)]
mod tests;
