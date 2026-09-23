//! The one mapping from a Python binary operator to the [`BinOpKind`] pycc
//! lowers. `Expr::BinOp` and augmented assignment (`x op= e`, #1209) both
//! call it, so the operators augmented assignment admits are exactly the
//! operators a plain binary expression admits, and widening one widens both.

use crate::BinOpKind;
use pycc_ast::Operator;

/// The [`BinOpKind`] for `op`, or `None` for an operator pycc does not lower
/// yet: the bitwise and shift operators and `@`.
pub(crate) fn bin_op_kind(op: Operator) -> Option<BinOpKind> {
    match op {
        Operator::Add => Some(BinOpKind::Add),
        Operator::Sub => Some(BinOpKind::Sub),
        Operator::Mult => Some(BinOpKind::Mul),
        Operator::Div => Some(BinOpKind::Div),
        Operator::FloorDiv => Some(BinOpKind::FloorDiv),
        Operator::Mod => Some(BinOpKind::Mod),
        Operator::Pow => Some(BinOpKind::Pow),
        Operator::MatMult
        | Operator::LShift
        | Operator::RShift
        | Operator::BitOr
        | Operator::BitXor
        | Operator::BitAnd => None,
    }
}

#[cfg(test)]
mod tests {
    use super::bin_op_kind;
    use crate::BinOpKind;
    use pycc_ast::Operator;

    #[test]
    fn every_operator_maps_as_the_binary_expression_lowering_expects() {
        for (op, expected) in [
            (Operator::Add, Some(BinOpKind::Add)),
            (Operator::Sub, Some(BinOpKind::Sub)),
            (Operator::Mult, Some(BinOpKind::Mul)),
            (Operator::Div, Some(BinOpKind::Div)),
            (Operator::FloorDiv, Some(BinOpKind::FloorDiv)),
            (Operator::Mod, Some(BinOpKind::Mod)),
            (Operator::Pow, Some(BinOpKind::Pow)),
            (Operator::MatMult, None),
            (Operator::LShift, None),
            (Operator::RShift, None),
            (Operator::BitOr, None),
            (Operator::BitXor, None),
            (Operator::BitAnd, None),
        ] {
            assert_eq!(bin_op_kind(op), expected, "{op:?}");
        }
    }
}
