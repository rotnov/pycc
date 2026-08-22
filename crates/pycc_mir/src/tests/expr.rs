//! Expression lowering (`expr::lower_expr`) for operators.
//!
//! Covers comparison, f-string interpolation, the `binop_result_ty` result
//! table (string concatenation and repetition, true division, int/float
//! promotion), and the unary-operator rewrites.

use crate::*;
use pycc_hir::{
    BinOpKind, CmpOpKind, FStringPart, HirExpr, HirItem, HirModule, HirStmt, Ty, UnaryOpKind,
};

#[test]
fn builds_a_compare_expression_with_bool_type() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(MirExpr::IntLiteral(1)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Bool,
            },
        })]
    );
}

#[test]
fn builds_an_f_string_with_a_literal_and_an_interpolation() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::FString(vec![
                    FStringPart::Literal("n=".to_string()),
                    FStringPart::Interpolation(Box::new(HirExpr::Name("x".to_string()))),
                ]),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::FString(vec![
                MirFStringPart::Literal("n=".to_string()),
                MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                })),
            ]),
        })
    );
}

#[test]
fn string_concatenation_infers_str() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::StringLiteral("a".to_string())),
                right: Box::new(HirExpr::StringLiteral("b".to_string())),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::StringLiteral("a".to_string())),
                right: Box::new(MirExpr::StringLiteral("b".to_string())),
                ty: Ty::Str,
            },
        })]
    );
}

// #574 (Part 1 of #123): `binop_result_ty`'s repetition clause. One
// test per operand order and per accepted count type, because each
// side of the clause's `||` is its own region under the D-014
// coverage gate, as is each arm of the two `matches!` expressions --
// including their `_ => false` fallbacks, which the negative controls
// at the end of this group reach.

#[test]
fn string_repetition_by_an_int_infers_str() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(HirExpr::StringLiteral("a".to_string())),
                right: Box::new(HirExpr::IntLiteral(3)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::StringLiteral("a".to_string())),
                right: Box::new(MirExpr::IntLiteral(3)),
                ty: Ty::Str,
            },
        })]
    );
}

#[test]
fn string_repetition_with_the_count_on_the_left_infers_str() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(HirExpr::IntLiteral(3)),
                right: Box::new(HirExpr::StringLiteral("a".to_string())),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::IntLiteral(3)),
                right: Box::new(MirExpr::StringLiteral("a".to_string())),
                ty: Ty::Str,
            },
        })]
    );
}

#[test]
fn string_repetition_by_a_bool_infers_str() {
    // `bool <: int`, so a `bool` count is accepted in either order --
    // matching `pycc_types::numeric_result_type` exactly.
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(HirExpr::StringLiteral("a".to_string())),
                    right: Box::new(HirExpr::BoolLiteral(true)),
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(HirExpr::BoolLiteral(true)),
                    right: Box::new(HirExpr::StringLiteral("a".to_string())),
                },
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    // Asserted against the full expected items rather than destructuring
    // each one with a `let ... else { unreachable!() }`: that `else` arm
    // would be an unexercised line and region under the D-014 gate.
    assert_eq!(
        mir.items,
        vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(MirExpr::StringLiteral("a".to_string())),
                    right: Box::new(MirExpr::BoolLiteral(true)),
                    ty: Ty::Str,
                },
            }),
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(MirExpr::BoolLiteral(true)),
                    right: Box::new(MirExpr::StringLiteral("a".to_string())),
                    ty: Ty::Str,
                },
            }),
        ]
    );
}

#[test]
fn multiplying_two_ints_still_infers_int_after_the_repetition_clause() {
    // Negative control for the repetition clause: `Mul` alone must not
    // route a purely numeric pair into `Ty::Str`.
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(HirExpr::IntLiteral(2)),
                right: Box::new(HirExpr::IntLiteral(3)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::IntLiteral(2)),
                right: Box::new(MirExpr::IntLiteral(3)),
                ty: Ty::Int,
            },
        })]
    );
}

#[test]
fn multiplying_a_str_by_a_float_does_not_take_the_repetition_clause() {
    // Defensive shape: `pycc_types` rejects `str * float` with T0021,
    // so this HIR can never arrive through `pycc check` -- it is built
    // directly here (`build` does not type-check) purely to exercise
    // the `_ => false` fallback of the repetition clause's
    // `matches!(right, Ty::Int | Ty::Bool)`, which no reachable
    // program covers. The clause must decline, leaving the ordinary
    // numeric rule to answer `Ty::Float`.
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(HirExpr::StringLiteral("a".to_string())),
                right: Box::new(HirExpr::FloatLiteral(2.0)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::StringLiteral("a".to_string())),
                right: Box::new(MirExpr::FloatLiteral(2.0)),
                ty: Ty::Float,
            },
        })]
    );
}

#[test]
fn multiplying_a_float_by_a_str_does_not_take_the_repetition_clause() {
    // The mirror of the test above, and equally unreachable through
    // `pycc check` (T0021): it exists to exercise the `_ => false`
    // fallback of the repetition clause's
    // `matches!(left, Ty::Int | Ty::Bool)`.
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(HirExpr::FloatLiteral(2.0)),
                right: Box::new(HirExpr::StringLiteral("a".to_string())),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::FloatLiteral(2.0)),
                right: Box::new(MirExpr::StringLiteral("a".to_string())),
                ty: Ty::Float,
            },
        })]
    );
}

#[test]
fn true_division_of_two_ints_infers_float() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Div,
                left: Box::new(HirExpr::IntLiteral(5)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Div,
                left: Box::new(MirExpr::IntLiteral(5)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Float,
            },
        })]
    );
}

#[test]
fn adding_a_float_left_operand_and_an_int_infers_float() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::FloatLiteral(1.5)),
                right: Box::new(HirExpr::IntLiteral(2)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::FloatLiteral(1.5)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Float,
            },
        })]
    );
}

#[test]
fn adding_an_int_and_a_float_right_operand_infers_float() {
    // Distinct region from the left-operand case above: exercises
    // `right == Ty::Float` specifically (`left == Ty::Float` is false
    // here), not just `binop_result_ty`'s overall `Float` outcome.
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::IntLiteral(2)),
                right: Box::new(HirExpr::FloatLiteral(1.5)),
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::IntLiteral(2)),
                right: Box::new(MirExpr::FloatLiteral(1.5)),
                ty: Ty::Float,
            },
        })]
    );
}

/// Lowers `-p` / `+p` over a single parameter of type `ty` and asserts the
/// whole lowered function equals `expected` in its return position, so each
/// unary case below states only the arithmetic rewrite it expects.
fn assert_unary_over_param_lowers_to(op: UnaryOpKind, ty: Ty, expected: MirExpr) {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("p".to_string(), ty.clone())],
            return_ty: ty.clone(),
            body: vec![HirStmt::Return(Some(HirExpr::UnaryOp {
                op,
                operand: Box::new(HirExpr::Name("p".to_string())),
            }))],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::Function {
            name: "f".to_string(),
            params: vec![("p".to_string(), ty.clone())],
            return_ty: ty,
            body: vec![MirStmt::Return(Some(expected))],
        }]
    );
}

#[test]
fn unary_minus_on_an_int_operand_lowers_to_zero_minus_operand() {
    assert_unary_over_param_lowers_to(
        UnaryOpKind::USub,
        Ty::Int,
        MirExpr::BinOp {
            op: BinOpKind::Sub,
            left: Box::new(MirExpr::IntLiteral(0)),
            right: Box::new(MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Int,
            }),
            ty: Ty::Int,
        },
    );
}

#[test]
fn unary_plus_on_an_int_operand_lowers_to_zero_plus_operand() {
    assert_unary_over_param_lowers_to(
        UnaryOpKind::UAdd,
        Ty::Int,
        MirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(MirExpr::IntLiteral(0)),
            right: Box::new(MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Int,
            }),
            ty: Ty::Int,
        },
    );
}

#[test]
fn unary_minus_on_a_float_operand_lowers_to_a_multiply_by_minus_one() {
    assert_unary_over_param_lowers_to(
        UnaryOpKind::USub,
        Ty::Float,
        MirExpr::BinOp {
            op: BinOpKind::Mul,
            left: Box::new(MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Float,
            }),
            right: Box::new(MirExpr::FloatLiteral(-1.0)),
            ty: Ty::Float,
        },
    );
}

#[test]
fn unary_plus_on_a_float_operand_lowers_to_a_multiply_by_one() {
    assert_unary_over_param_lowers_to(
        UnaryOpKind::UAdd,
        Ty::Float,
        MirExpr::BinOp {
            op: BinOpKind::Mul,
            left: Box::new(MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Float,
            }),
            right: Box::new(MirExpr::FloatLiteral(1.0)),
            ty: Ty::Float,
        },
    );
}
