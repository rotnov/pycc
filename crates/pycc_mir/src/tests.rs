//! Unit tests for the crate-root HIR-to-MIR lowering surface.
//!
//! Extracted verbatim from `lib.rs`'s inline `mod tests` (issue #546): the
//! module is still a direct child of the crate root, so `use super::*` and the
//! private items it reaches resolve exactly as they did inline.

use super::*;
use pycc_hir::{
    BinOpKind, CmpOpKind, FStringPart, HirClassDef, HirExpr, HirItem, HirMatchCase, HirModule,
    HirPattern, HirStmt, Ty, UnaryOpKind,
};

#[test]
fn builds_an_assignment_and_a_later_name_reference() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("x".to_string())],
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(1),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                }],
                ty: Ty::None,
            })),
        ]
    );
}

#[test]
fn builds_a_function_with_typed_params_and_return() {
    let hir = HirModule {
        items: vec![HirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
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
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(MirExpr::Name {
                    name: "a".to_string(),
                    ty: Ty::Int
                }),
                right: Box::new(MirExpr::Name {
                    name: "b".to_string(),
                    ty: Ty::Int
                }),
                ty: Ty::Int,
            }))],
        }]
    );
}

#[test]
fn a_top_level_call_to_a_function_defined_later_resolves_via_two_pass_registration() {
    // Exercises `build`'s first pass directly: `helper`'s signature must
    // be registered before the top-level call to it is lowered, even
    // though `helper`'s `HirItem::Function` comes *after* the call in
    // `hir.items` -- exactly the forward-reference case D-038/D-039
    // already fixed on the `pycc_types::check` side.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "helper".to_string(),
                args: vec![],
            })),
            HirItem::Function {
                name: "helper".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "helper".to_string(),
            args: vec![],
            ty: Ty::Int,
        }))
    );
}

#[test]
fn a_function_can_call_itself_recursively_and_resolves_its_own_return_type() {
    let hir = HirModule {
        items: vec![HirItem::Function {
            name: "fact".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "fact".to_string(),
                args: vec![HirExpr::Name("n".to_string())],
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
            name: "fact".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::Call {
                callee: "fact".to_string(),
                args: vec![MirExpr::Name {
                    name: "n".to_string(),
                    ty: Ty::Int
                }],
                ty: Ty::Int,
            }))],
        }]
    );
}

#[test]
fn builds_an_if_statement_lowering_both_branches() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(2)],
            })],
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::If {
            test: MirExpr::BoolLiteral(true),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(1)],
                ty: Ty::None,
            })],
            orelse: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(2)],
                ty: Ty::None,
            })],
        })]
    );
}

#[test]
fn builds_a_while_loop() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::While {
            test: MirExpr::BoolLiteral(true),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(1)],
                ty: Ty::None,
            })],
        })]
    );
}

#[test]
fn builds_a_for_range_loop_binding_its_variable_as_int() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("i".to_string())],
            })],
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![MirItem::TopLevelStmt(MirStmt::ForRange {
            var: "i".to_string(),
            start: MirExpr::IntLiteral(0),
            stop: MirExpr::IntLiteral(3),
            step: MirExpr::IntLiteral(1),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "i".to_string(),
                    ty: Ty::Int
                }],
                ty: Ty::None,
            })],
        })]
    );
}

#[test]
fn builds_a_return_with_no_value() {
    let hir = HirModule {
        items: vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
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
            params: vec![],
            return_ty: Ty::None,
            body: vec![MirStmt::Return(None)],
        }]
    );
}

#[test]
fn an_annotated_assignment_whose_value_type_already_matches_the_annotation_lowers_unchanged() {
    // `x: int = 1` -- the initializer's own inferred type (`Ty::Int`)
    // already matches the annotation, so this is `lower_stmt`'s
    // "no widening needed" branch and `value` passes through
    // unchanged. This case cannot by itself distinguish binding the
    // annotation's type from binding the value's type (they're equal
    // here) -- the sibling test below, where they differ, is what
    // actually proves `lower_stmt` binds the annotation.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::AnnAssign {
                is_final: false,
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntLiteral(1),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int,
            })),
        ]
    );
}

#[test]
fn an_annotated_assignment_with_a_bool_value_under_an_int_annotation_widens_and_binds_int() {
    // `x: int = True` -- `pycc_types::is_assignable` accepts a `bool`
    // initializer under an `int` annotation as its one widening case,
    // and `pycc_types` itself binds its checker `env` to `Ty::Int`
    // (the annotation), not `Ty::Bool` (the initializer's own type) --
    // see its own comment citing this exact invariant. `lower_stmt`
    // must agree (D-074's "first assignment fixes a binding's
    // representation" rule): it wraps the lowered `BoolLiteral` in an
    // `IntBoundary` reporting `Ty::Int`, preserving D-141 runtime
    // identity without manufacturing arithmetic, and binds `x` to
    // `Ty::Int`, so a later
    // `Name` reference -- and any later plain reassignment -- agrees.
    // Before this fix, `lower_stmt` bound `Ty::Bool` here instead, and
    // the divergence from `pycc_types`' `Ty::Int` silently mis-sized
    // `x`'s eventual codegen slot (confirmed end to end:
    // `x: int = True; x = 5; return x` printed `11`, not `5`).
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::AnnAssign {
                is_final: false,
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::BoolLiteral(true)),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items,
        vec![
            MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::IntBoundary(Box::new(MirExpr::BoolLiteral(true))),
            }),
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int,
            })),
        ]
    );
}

#[test]
fn an_annotated_assignment_with_a_bool_typed_compare_value_also_widens() {
    // The widening branch above is reachable for *any* `Ty::Bool`-typed
    // initializer under an `int` annotation, not merely a literal
    // `True`/`False` -- `pycc_types::is_assignable(Bool, Int)` accepts
    // a `Compare` result, a bool-typed name, or a bool-returning call
    // identically. This proves the same `IntBoundary` wrapping triggers
    // for a `Compare`-sourced `bool`, not only the literal
    // case the previous test exercises.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            is_final: false,
            target: "x".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            }),
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
            value: MirExpr::IntBoundary(Box::new(MirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(MirExpr::IntLiteral(1)),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Bool,
            })),
        })]
    );
}

#[test]
fn a_value_less_annotated_assignment_lowers_to_a_no_op_and_binds_nothing() {
    // `y: int` alone has no runtime action -- CPython itself does
    // nothing observable for it either. `lower_stmt` must produce
    // `MirStmt::NoOp` and must NOT bind `y` in scope (matching
    // `pycc_types`' own Task 4 choice not to bind a value-less
    // declaration): a later read of `y` with no intervening assignment
    // still panics via `lookup`, proving no phantom binding leaked
    // through.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            is_final: false,
            target: "y".to_string(),
            annotation: Ty::Int,
            value: None,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(mir.items, vec![MirItem::TopLevelStmt(MirStmt::NoOp)]);
}

#[test]
#[should_panic(expected = "has no recorded type")]
fn a_value_less_annotated_assignment_does_not_bind_the_name() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::AnnAssign {
                is_final: false,
                target: "y".to_string(),
                annotation: Ty::Int,
                value: None,
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("y".to_string()))),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

#[test]
fn builds_a_compare_expression_with_bool_type() {
    let hir = HirModule {
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

#[test]
fn a_function_resolves_a_module_global_assigned_after_its_definition() {
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "read_x".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            },
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(5),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::Function {
            name: "read_x".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int,
            }))],
        }
    );
}

#[test]
fn assigning_bool_to_an_existing_int_binding_preserves_its_mir_type() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BoolLiteral(true),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[2],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
            name: "x".to_string(),
            ty: Ty::Int,
        }))
    );
}

#[test]
#[should_panic(expected = "has no recorded type")]
fn a_top_level_read_still_cannot_resolve_a_later_assignment() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

#[test]
#[should_panic(expected = "has no recorded type")]
fn referencing_an_unbound_name_panics_with_an_internal_error() {
    // By construction (see this module's doc comment / D-057 discussion
    // in the task brief), every `Ty` reaching `pycc_mir` is already
    // concrete and every name already resolved by `pycc_types::check`
    // -- this HIR could never come from a real `check_and_resolve`
    // success, but the panic path itself still needs direct coverage.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name(
            "undefined".to_string(),
        )))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

#[test]
fn a_function_local_shadowing_a_module_global_of_a_different_type_resolves_its_own_type() {
    // D-055: a function that assigns a name anywhere in its body
    // classifies that name as local for the *entire* body, independent
    // of any same-named module global -- Python scoping, not a
    // control-flow-sensitive fact. `x` is a module-level `str` here;
    // `f`'s own `x = 5; return x` must resolve to `f`'s own fresh
    // `Ty::Int`, never falling through to the module global's `Ty::Str`.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(5),
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(5)
                },
                MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                })),
            ],
        }
    );
}

#[test]
fn a_sibling_function_after_a_shadowing_function_still_reads_the_unshadowed_global() {
    // `lower_item` pushes and later pops an isolated function scope;
    // lowering one function's shadowing assignment must not mutate the
    // module scope seen by a later sibling function.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "shadows".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(5),
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            },
            HirItem::Function {
                name: "reads_global".to_string(),
                params: vec![],
                return_ty: Ty::Str,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[2],
        MirItem::Function {
            name: "reads_global".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![MirStmt::Return(Some(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Str
            }))],
        }
    );
}

#[test]
fn a_function_parameter_shadowing_a_module_global_resolves_its_own_type() {
    // Parameters are part of D-055's lexical-local list too: a
    // parameter named the same as a module global must resolve to the
    // parameter's own type, never fall through to the global's.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int
            }))],
        }
    );
}

#[test]
fn a_for_range_variable_shadowing_a_module_global_resolves_its_own_type() {
    // `ForRange`'s loop variable is also part of D-055's lexical-local
    // list (it's a binding form, matching Python's own `for`-target
    // classification), so it must shadow a same-named module global too.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "i".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                        body: vec![],
                    },
                    HirStmt::Return(Some(HirExpr::Name("i".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                MirStmt::ForRange {
                    var: "i".to_string(),
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(3),
                    step: MirExpr::IntLiteral(1),
                    body: vec![],
                },
                MirStmt::Return(Some(MirExpr::Name {
                    name: "i".to_string(),
                    ty: Ty::Int
                })),
            ],
        }
    );
}

#[test]
fn a_local_first_assigned_inside_nested_if_and_else_bodies_shadows_a_module_global() {
    // Exercises `lower_stmt` recursing into both `body` and `orelse` --
    // D-055 classifies a name as
    // function-local even when its only assignment is nested inside a
    // conditional, not just when it appears directly in the body.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::If {
                        test: HirExpr::BoolLiteral(true),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                        orelse: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        }],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                MirStmt::If {
                    test: MirExpr::BoolLiteral(true),
                    body: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(1)
                    }],
                    orelse: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(2)
                    }],
                },
                MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                })),
            ],
        }
    );
}

#[test]
fn a_local_first_assigned_inside_a_while_body_shadows_a_module_global() {
    // Exercises `lower_stmt` recursing into a `While` body.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::While {
                        test: HirExpr::BoolLiteral(false),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                MirStmt::While {
                    test: MirExpr::BoolLiteral(false),
                    body: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(1)
                    }],
                },
                MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                })),
            ],
        }
    );
}

#[test]
fn a_local_first_assigned_inside_a_for_range_body_shadows_a_module_global() {
    // Exercises `lower_stmt` recursing into a `ForRange` body (distinct from the loop variable
    // itself, already covered by
    // `a_for_range_variable_shadowing_a_module_global_resolves_its_own_type`).
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("hello".to_string()),
            }),
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::ForRange {
                        var: "loop_i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                        body: vec![HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(1),
                        }],
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                MirStmt::ForRange {
                    var: "loop_i".to_string(),
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(3),
                    step: MirExpr::IntLiteral(1),
                    body: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(1)
                    }],
                },
                MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int
                })),
            ],
        }
    );
}

#[test]
fn mir_expr_ty_covers_every_variant() {
    assert_eq!(MirExpr::IntLiteral(1).ty(), Ty::Int);
    assert_eq!(MirExpr::FloatLiteral(1.0).ty(), Ty::Float);
    assert_eq!(MirExpr::BoolLiteral(true).ty(), Ty::Bool);
    assert_eq!(MirExpr::StringLiteral("s".to_string()).ty(), Ty::Str);
    assert_eq!(MirExpr::FString(vec![]).ty(), Ty::Str);
    assert_eq!(
        MirExpr::Name {
            name: "x".to_string(),
            ty: Ty::Int
        }
        .ty(),
        Ty::Int
    );
    assert_eq!(
        MirExpr::Call {
            callee: "f".to_string(),
            args: vec![],
            ty: Ty::Bool
        }
        .ty(),
        Ty::Bool
    );
    assert_eq!(
        MirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(MirExpr::IntLiteral(1)),
            right: Box::new(MirExpr::IntLiteral(2)),
            ty: Ty::Int,
        }
        .ty(),
        Ty::Int
    );
    assert_eq!(
        MirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(MirExpr::IntLiteral(1)),
            right: Box::new(MirExpr::IntLiteral(2)),
            ty: Ty::Bool,
        }
        .ty(),
        Ty::Bool
    );
    assert_eq!(
        MirExpr::ListLiteral(vec![MirExpr::IntLiteral(1)]).ty(),
        Ty::List(Box::new(Ty::Int))
    );
    assert_eq!(
        MirExpr::Subscript {
            base: Box::new(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::List(Box::new(Ty::Int)),
            }),
            index: Box::new(MirExpr::IntLiteral(0)),
        }
        .ty(),
        Ty::Int
    );
    assert_eq!(
        MirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(MirExpr::IntLiteral(1)),
        }
        .ty(),
        Ty::None
    );
    assert_eq!(
        MirExpr::DictLiteral(vec![(
            MirExpr::StringLiteral("a".to_string()),
            MirExpr::IntLiteral(1)
        )])
        .ty(),
        Ty::Dict(Box::new((Ty::Str, Ty::Int)))
    );
    assert_eq!(
        MirExpr::DictGet {
            dict: Box::new(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
            }),
            key: Box::new(MirExpr::StringLiteral("a".to_string())),
        }
        .ty(),
        Ty::Int
    );
    assert_eq!(
        MirExpr::SetLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)]).ty(),
        Ty::Set(Box::new(Ty::Int))
    );
    assert_eq!(
        MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1), MirExpr::BoolLiteral(true)]).ty(),
        Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool]))
    );
    assert_eq!(
        MirExpr::Slice {
            base: Box::new(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::List(Box::new(Ty::Int)),
            }),
            start: Some(Box::new(MirExpr::IntLiteral(1))),
            stop: None,
            step: None,
        }
        .ty(),
        Ty::List(Box::new(Ty::Int))
    );
    assert_eq!(
        MirExpr::ListPop {
            list: "x".to_string(),
            ty: Ty::Int,
        }
        .ty(),
        Ty::Int
    );
    assert_eq!(
        MirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(MirExpr::StringLiteral("a".to_string())),
            default: Box::new(MirExpr::IntLiteral(0)),
            ty: Ty::Int,
        }
        .ty(),
        Ty::Int
    );
    assert_eq!(
        MirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(MirExpr::IntLiteral(1)),
        }
        .ty(),
        Ty::None
    );
    assert_eq!(
        MirExpr::Instantiate(Box::new(InstantiateExpr {
            ctor: "Point.__init__".to_string(),
            attr_count: 2,
            args: vec![],
            ty: Ty::Instance(Box::new("Point".to_string())),
        }))
        .ty(),
        Ty::Instance(Box::new("Point".to_string()))
    );
    assert_eq!(
        MirExpr::AttrGet {
            base: Box::new(MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Instance(Box::new("Point".to_string())),
            }),
            slot: 0,
            ty: Ty::Int,
        }
        .ty(),
        Ty::Int
    );
}

#[test]
fn lowers_list_literal_to_mir() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    // Not a `let PATTERN = ... else { panic!(...) }` destructure -- this
    // file's own coverage-gate convention (see `pycc_hir`'s equivalent
    // `ListLiteral` test, commit 48f13e6) is that a hand-written panic
    // arm is never taken on the happy path and shows up as a
    // permanently uncovered region under D-014's 100%-regions gate. A
    // direct `assert_eq!` against the whole expected `MirItem` avoids
    // that without weakening the assertion.
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::ListLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)]),
        })
    );
}

#[test]
fn lowers_for_list_to_mir_for_list() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            }),
            HirItem::TopLevelStmt(HirStmt::ForList {
                var: "v".to_string(),
                list: "x".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("v".to_string())],
                })],
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ForList {
            var: "v".to_string(),
            list: "x".to_string(),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            })],
        })
    );
}

#[test]
fn lowers_subscript_to_mir_recursively() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    index: Box::new(HirExpr::IntLiteral(0)),
                },
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
            value: MirExpr::Subscript {
                base: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                index: Box::new(MirExpr::IntLiteral(0)),
            },
        })
    );
}

#[test]
fn lowers_list_append_to_mir_recursively() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListAppend {
                list: "x".to_string(),
                value: Box::new(HirExpr::IntLiteral(2)),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(MirExpr::IntLiteral(2)),
        }))
    );
}

#[test]
fn lowers_list_pop_to_mir_deriving_its_element_type_from_the_list_binding() {
    // PR-12 Task 11 (D-119): `xs.pop()`'s `ty` is derived from `xs`'s
    // own `Ty::List` binding via `lookup`, mirroring
    // `HirExpr::Subscript`'s own base-type lookup.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2),
                    HirExpr::IntLiteral(3),
                ]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::ListPop {
                    list: "xs".to_string(),
                },
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
            value: MirExpr::ListPop {
                list: "xs".to_string(),
                ty: Ty::Int,
            },
        })
    );
}

#[test]
#[should_panic(expected = "`xs` is not list-typed")]
fn list_pop_over_a_non_list_binding_panics_with_an_internal_error() {
    // `pycc_types` already rejects `.pop()` on a non-list base (T0033)
    // before HIR reaches `pycc_mir`, but the defensive panic path in
    // `lower_expr`'s own `HirExpr::ListPop` arm still needs direct
    // coverage, mirroring `a_for_list_loop_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error`
    // above.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::IntLiteral(5),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::ListPop {
                list: "xs".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

#[test]
fn lowers_dict_get_or_default_to_mir_recursively_deriving_its_value_type() {
    // PR-12 Task 11 (D-119): `d.get(key, default)`'s `ty` is derived
    // from `d`'s own `Ty::Dict` binding's value type, and both `key`
    // and `default` are recursively lowered.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(HirExpr::StringLiteral("z".to_string())),
                    default: Box::new(HirExpr::IntLiteral(-1)),
                },
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
            value: MirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(MirExpr::StringLiteral("z".to_string())),
                default: Box::new(MirExpr::IntLiteral(-1)),
                ty: Ty::Int,
            },
        })
    );
}

#[test]
#[should_panic(expected = "`d` is not dict-typed")]
fn dict_get_or_default_over_a_non_dict_binding_panics_with_an_internal_error() {
    // Same reasoning as `list_pop_over_a_non_list_binding_panics_with_an_internal_error`
    // above, for `HirExpr::DictGetOrDefault`'s own defensive panic path.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::IntLiteral(5),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::DictGetOrDefault {
                dict: "d".to_string(),
                key: Box::new(HirExpr::StringLiteral("a".to_string())),
                default: Box::new(HirExpr::IntLiteral(0)),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

#[test]
fn lowers_set_add_to_mir_recursively() {
    // PR-12 Task 11 (D-119): `s.add(value)` mirrors `ListAppend`'s own
    // shape exactly -- `set` is carried as a plain name, `value` is
    // recursively lowered.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "s".to_string(),
                value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::SetAdd {
                set: "s".to_string(),
                value: Box::new(HirExpr::IntLiteral(2)),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(MirExpr::IntLiteral(2)),
        }))
    );
}

#[test]
fn lowers_math_sqrt_call_to_mir_with_float_type_without_panicking() {
    // D-136: without the dedicated `callee == "math.sqrt"` branch, this
    // would panic via `lookup`'s own "has no recorded type" message,
    // exactly like `len` above -- there is no `$fn:math.sqrt` signature
    // to find, even though `pycc_types` already accepts `math.sqrt(x)`
    // as valid, `Ty::Float`-typed.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "n".to_string(),
            value: HirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![HirExpr::FloatLiteral(2.0)],
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "n".to_string(),
            value: MirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![MirExpr::FloatLiteral(2.0)],
                ty: Ty::Float,
            },
        })
    );
}

#[test]
fn lowers_math_pi_name_to_mir_with_float_type_without_panicking() {
    // D-136: without the dedicated `name == "math.pi"` arm, this would
    // panic via `lookup`'s own "has no recorded type" message -- `pi`
    // is never bound in `scopes` the way an ordinary assigned variable
    // is.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "n".to_string(),
            value: HirExpr::Name("math.pi".to_string()),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "n".to_string(),
            value: MirExpr::Name {
                name: "math.pi".to_string(),
                ty: Ty::Float,
            },
        })
    );
}

#[test]
fn lowers_len_call_to_mir_with_int_type_without_panicking() {
    // Required fix (beyond the brief): without a parallel `"len"` branch
    // in the `HirExpr::Call` lowering arm, this would panic via
    // `lookup`'s own "has no recorded type" message, since no `$fn:len`
    // signature is ever registered -- even though `pycc_types` already
    // accepts `len(lst)` as valid, `Ty::Int`-typed (D-105 point 3).
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "n".to_string(),
                value: HirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                },
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
            target: "n".to_string(),
            value: MirExpr::Call {
                callee: "len".to_string(),
                args: vec![MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }],
                ty: Ty::Int,
            },
        })
    );
}

#[test]
fn lowers_float_call_to_mir_with_float_type_without_panicking() {
    // Mirrors `lowers_len_call_to_mir_with_int_type_without_panicking`
    // immediately above, for the same reason (#181): without a parallel
    // `"float"` branch in the `HirExpr::Call` lowering arm, this would
    // panic via `lookup`'s own "has no recorded type" message, since no
    // `$fn:float` signature is ever registered -- even though
    // `pycc_types` already accepts `float(x)` as valid, `Ty::Float`-typed.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(3),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Call {
                    callee: "float".to_string(),
                    args: vec![HirExpr::Name("x".to_string())],
                },
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
            value: MirExpr::Call {
                callee: "float".to_string(),
                args: vec![MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int,
                }],
                ty: Ty::Float,
            },
        })
    );
}

#[test]
fn a_user_defined_float_function_is_lowered_as_a_real_call_not_the_builtin() {
    // Post-merge review finding: unlike `len`/`print`, `float` was
    // undefined until #181, so a program defining its own `float` was
    // valid on `main` immediately before this builtin landed --
    // reproduced directly, printing `6` on a pristine checkout. Without
    // this priority check, the builtin's hardcoded `Ty::Float` would
    // silently override the user function's own registered `Ty::Int`
    // return type.
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "float".to_string(),
                params: vec![("x".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("x".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            },
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Call {
                    callee: "float".to_string(),
                    args: vec![HirExpr::IntLiteral(5)],
                },
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
            value: MirExpr::Call {
                callee: "float".to_string(),
                args: vec![MirExpr::IntLiteral(5)],
                ty: Ty::Int,
            },
        })
    );
}

#[test]
fn list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int() {
    // Mirrors `pycc_types`'s own genericity tests for `ListLiteral`,
    // `Subscript`, and `ForList` (see e.g. its
    // `a_for_list_loop_binds_its_variable_as_str_for_a_list_of_str`):
    // this lowering must derive `ty()`/the loop variable's bound type
    // from the list's *actual* element type, not assume `Ty::Int`.
    // `pycc_types`'s T0034 gate means only `list[int]` ever reaches
    // this crate from a real compiled program, but this crate's own
    // lowering must not bake in that assumption independently of the
    // type it actually observes -- exactly the class of bug the
    // `AnnAssign` widening fix in `stmt.rs`'s `lower_stmt` already
    // guards against (MIR's `ty` silently diverging from what codegen
    // must produce). Uses `str` elements specifically because they are
    // trivially distinguishable from the `Ty::Int` a hardcoded bug
    // would wrongly report.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    index: Box::new(HirExpr::IntLiteral(0)),
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ForList {
                var: "v".to_string(),
                list: "xs".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Name("v".to_string()))],
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("y".to_string()))),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    // `y = xs[0]` binds `y` as `Ty::Str`, derived from `xs`'s own
    // `Ty::List(Box::new(Ty::Str))` binding (itself derived from the
    // `StringLiteral` element), not `Ty::Int`.
    assert_eq!(
        mir.items[3],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
            name: "y".to_string(),
            ty: Ty::Str,
        }))
    );
    // `for v in xs:` binds `v` as `Ty::Str` too, derived from the same
    // list, not `Ty::Int`.
    assert_eq!(
        mir.items[2],
        MirItem::TopLevelStmt(MirStmt::ForList {
            var: "v".to_string(),
            list: "xs".to_string(),
            body: vec![MirStmt::ExprStmt(MirExpr::Name {
                name: "v".to_string(),
                ty: Ty::Str,
            })],
        })
    );
}

#[test]
#[should_panic(expected = "an empty list literal has no element type")]
fn an_empty_list_literals_ty_panics_with_an_internal_error() {
    // By construction (see this module's `lookup` panic doc comment /
    // D-057 discussion), `pycc_types::check` already rejects an empty
    // list literal (T0021) before any HIR reaches `pycc_mir` -- this
    // MIR node could never come from a real `check_and_resolve`
    // success, but the panic path itself still needs direct coverage.
    MirExpr::ListLiteral(vec![]).ty();
}

#[test]
#[should_panic(expected = "subscript base has non-list/tuple type")]
fn a_subscript_over_a_non_list_bases_ty_panics_with_an_internal_error() {
    // A non-list, non-dict, non-tuple subscript base (e.g. `Ty::Int`) is
    // rejected by `pycc_types` (T0033) before HIR reaches `pycc_mir` --
    // unlike a dict base (which `pycc_types` accepts, but `lower_expr`'s
    // own `HirExpr::Subscript` arm routes into `MirExpr::DictGet` instead
    // of ever constructing this node), so this defensive panic path in
    // `MirExpr::Subscript`'s own `ty()` arm still needs direct coverage
    // via a hand-built node that bypasses both guarantees.
    MirExpr::Subscript {
        base: Box::new(MirExpr::IntLiteral(1)),
        index: Box::new(MirExpr::IntLiteral(0)),
    }
    .ty();
}

#[test]
#[should_panic(expected = "dict subscript base has non-dict type")]
fn a_dict_get_over_a_non_dict_bases_ty_panics_with_an_internal_error() {
    // Same reasoning as the subscript panic above, for `MirExpr::DictGet`'s
    // own defensive `ty()` arm: no real lowering ever constructs this
    // node with a non-dict base (`lower_expr`'s own `HirExpr::Subscript`
    // arm only builds `MirExpr::DictGet` when the base's derived type is
    // `Ty::Dict`), but the panic path still needs direct coverage.
    MirExpr::DictGet {
        dict: Box::new(MirExpr::IntLiteral(1)),
        key: Box::new(MirExpr::StringLiteral("a".to_string())),
    }
    .ty();
}

#[test]
#[should_panic(expected = "neither a list, dict, nor set")]
fn a_for_list_loop_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error() {
    // Same reasoning again: `pycc_types` already rejects `for v in x:`
    // when `x` is neither a list, dict, nor set (T0033), but the
    // defensive panic path in `lower_stmt`'s `ForList` arm still needs
    // direct coverage.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(5),
            }),
            HirItem::TopLevelStmt(HirStmt::ForList {
                var: "v".to_string(),
                list: "x".to_string(),
                body: vec![],
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

#[test]
#[should_panic(expected = "an empty dict literal has no key/value type to derive")]
fn an_empty_dict_literals_ty_panics_with_an_internal_error() {
    // By construction, `pycc_types::check` already rejects an empty
    // dict literal (T0021, mirroring the empty-list-literal case above)
    // before any HIR reaches `pycc_mir` -- this MIR node could never
    // come from a real `check_and_resolve` success, but the panic path
    // itself still needs direct coverage.
    MirExpr::DictLiteral(vec![]).ty();
}

#[test]
fn dict_literal_lowers_to_mir_dict_literal_with_correct_ty() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::DictLiteral(vec![(
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            )]),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    let expected_value = MirExpr::DictLiteral(vec![(
        MirExpr::StringLiteral("a".to_string()),
        MirExpr::IntLiteral(1),
    )]);
    assert_eq!(expected_value.ty(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: expected_value,
        })
    );
}

#[test]
fn dict_get_ty_unwraps_the_value_type() {
    // `x["a"]` where `x: dict[str, int]` lowers `HirExpr::Subscript`
    // into `MirExpr::DictGet` (not `MirExpr::Subscript`), whose `ty()`
    // unwraps the dict's value type, mirroring `dict_get_ty_unwraps_the_value_type`
    // in the task brief.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    index: Box::new(HirExpr::StringLiteral("a".to_string())),
                },
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
            value: MirExpr::DictGet {
                dict: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
                }),
                key: Box::new(MirExpr::StringLiteral("a".to_string())),
            },
        })
    );
    assert_eq!(
        MirExpr::DictGet {
            dict: Box::new(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
            }),
            key: Box::new(MirExpr::StringLiteral("a".to_string())),
        }
        .ty(),
        Ty::Int
    );
}

#[test]
fn a_list_subscript_still_lowers_to_mir_subscript_not_dict_get() {
    // Genericity check mirroring `list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int`
    // above: `lower_expr`'s `HirExpr::Subscript` arm must route based on
    // the base's *actual* derived type, not assume every subscript is a
    // dict read now that `MirExpr::DictGet` exists.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Subscript {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    index: Box::new(HirExpr::IntLiteral(0)),
                },
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
            value: MirExpr::Subscript {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                index: Box::new(MirExpr::IntLiteral(0)),
            },
        })
    );
}

#[test]
fn dict_set_lowers_to_mir_dict_set_stmt() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            }),
            HirItem::TopLevelStmt(HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("b".to_string()),
                value: HirExpr::IntLiteral(2),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::DictSet {
            dict: "x".to_string(),
            key: MirExpr::StringLiteral("b".to_string()),
            value: MirExpr::IntLiteral(2),
        })
    );
}

#[test]
fn for_k_in_dict_lowers_to_mir_for_dict() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            }),
            HirItem::TopLevelStmt(HirStmt::ForList {
                var: "k".to_string(),
                list: "x".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("k".to_string())],
                })],
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ForDict {
            var: "k".to_string(),
            dict: "x".to_string(),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "k".to_string(),
                    ty: Ty::Str,
                }],
                ty: Ty::None,
            })],
        })
    );
}

#[test]
#[should_panic(expected = "an empty set literal has no element type to derive")]
fn an_empty_set_literals_ty_panics_with_an_internal_error() {
    // By construction, `pycc_types::check` already rejects an empty set
    // literal (mirroring the empty-list/empty-dict-literal cases above)
    // before any HIR reaches `pycc_mir` -- and, unlike those two, an
    // empty `SetLiteral` cannot even be *written* in real Python source
    // (`{}` always parses as an empty `dict`, never an empty `set`) --
    // but the panic path itself still needs direct coverage.
    MirExpr::SetLiteral(vec![]).ty();
}

#[test]
fn tuple_literal_ty_derives_positionally_from_every_element() {
    let expr = MirExpr::TupleLiteral(vec![
        MirExpr::IntLiteral(1),
        MirExpr::BoolLiteral(true),
        MirExpr::FloatLiteral(2.5),
    ]);
    assert_eq!(
        expr.ty(),
        Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float]))
    );
}

#[test]
#[should_panic(expected = "an empty tuple literal has no element types to derive")]
fn an_empty_tuple_literal_ty_panics_with_an_internal_error() {
    // By construction, `pycc_types::check` already rejects an empty
    // tuple literal (T0039, mirroring the empty-list/empty-dict-literal
    // cases above) before any HIR reaches `pycc_mir` -- but the panic
    // path itself still needs direct coverage.
    MirExpr::TupleLiteral(vec![]).ty();
}

#[test]
fn tuple_subscript_ty_derives_the_positional_element_type() {
    let expr = MirExpr::Subscript {
        base: Box::new(MirExpr::TupleLiteral(vec![
            MirExpr::IntLiteral(1),
            MirExpr::BoolLiteral(true),
        ])),
        index: Box::new(MirExpr::IntLiteral(1)),
    };
    assert_eq!(expr.ty(), Ty::Bool);
}

#[test]
#[should_panic(expected = "tuple subscript index is not a literal int")]
fn a_tuple_subscript_with_a_non_literal_index_ty_panics_with_an_internal_error() {
    // By construction, `pycc_types::check` already rejects a
    // non-literal tuple subscript index (T0040) before any HIR reaches
    // `pycc_mir` -- but the panic path itself still needs direct
    // coverage via a hand-built node that bypasses that guarantee.
    let expr = MirExpr::Subscript {
        base: Box::new(MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1)])),
        index: Box::new(MirExpr::Name {
            name: "i".to_string(),
            ty: Ty::Int,
        }),
    };
    expr.ty();
}

#[test]
#[should_panic(expected = "tuple subscript index is negative")]
fn a_tuple_subscript_with_a_negative_index_ty_panics_with_an_internal_error() {
    // By construction, `pycc_types::check` already rejects a negative
    // literal tuple subscript index (T0040) before any HIR reaches
    // `pycc_mir` -- but the panic path itself still needs direct
    // coverage via a hand-built node that bypasses that guarantee.
    let expr = MirExpr::Subscript {
        base: Box::new(MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1)])),
        index: Box::new(MirExpr::IntLiteral(-1)),
    };
    expr.ty();
}

#[test]
#[should_panic(expected = "tuple subscript index out of range")]
fn a_tuple_subscript_out_of_range_ty_panics_with_an_internal_error() {
    // By construction, `pycc_types::check` already rejects an
    // out-of-range literal tuple subscript index (T0040) before any HIR
    // reaches `pycc_mir` -- but the panic path itself still needs
    // direct coverage via a hand-built node that bypasses that
    // guarantee.
    let expr = MirExpr::Subscript {
        base: Box::new(MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1)])),
        index: Box::new(MirExpr::IntLiteral(5)),
    };
    expr.ty();
}

#[test]
fn tuple_literal_lowers_to_mir_tuple_literal_with_correct_ty() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1), HirExpr::BoolLiteral(true)]),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    let expected_value =
        MirExpr::TupleLiteral(vec![MirExpr::IntLiteral(1), MirExpr::BoolLiteral(true)]);
    assert_eq!(
        expected_value.ty(),
        Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool]))
    );
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: expected_value,
        })
    );
}

#[test]
fn set_literal_lowers_to_mir_set_literal_with_correct_ty() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "x".to_string(),
            value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    let expected_value = MirExpr::SetLiteral(vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)]);
    assert_eq!(expected_value.ty(), Ty::Set(Box::new(Ty::Int)));
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "x".to_string(),
            value: expected_value,
        })
    );
}

#[test]
fn for_x_in_set_lowers_to_mir_for_set() {
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            }),
            HirItem::TopLevelStmt(HirStmt::ForList {
                var: "v".to_string(),
                list: "x".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Name("v".to_string())],
                })],
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ForSet {
            var: "v".to_string(),
            set: "x".to_string(),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            })],
        })
    );
}

// -- PR-12 Task 4 (D-117): comprehension lowering --

#[test]
fn a_range_sourced_list_comprehension_lowers_to_comp_source_range_with_var_ty_int_and_evaluates_its_filter()
 {
    // Exercises `resolve_comp_source`'s `CompIter::Range` branch and the
    // `ListCompAssign` arm's `cond: Some(..)` path (both closures on
    // that arm need at least one executing test for D-014's coverage
    // gate).
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::BoolLiteral(true))),
                elt: Box::new(HirExpr::Name("i".to_string())),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "len".to_string(),
                args: vec![HirExpr::Name("y".to_string())],
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "i".to_string(),
            var_ty: Ty::Int,
            source: CompSource::Range {
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(MirExpr::BoolLiteral(true))),
            elt: Box::new(MirExpr::Name {
                name: "i".to_string(),
                ty: Ty::Int,
            }),
        })
    );
    // `target` is bound as `Ty::List(Ty::Int)`, derived from `elt`'s
    // type -- confirmed via the following statement's own lowered type.
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "len".to_string(),
            args: vec![MirExpr::Name {
                name: "y".to_string(),
                ty: Ty::List(Box::new(Ty::Int)),
            }],
            ty: Ty::Int,
        }))
    );
}

#[test]
fn a_bare_name_list_sourced_list_comprehension_resolves_comp_source_list_with_the_lists_element_type()
 {
    // Exercises `resolve_comp_source`'s `CompIter::Name` branch resolving
    // to `Ty::List` -- uses `str` elements specifically (mirroring this
    // file's own `list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int`)
    // so `var_ty` is trivially distinguishable from the `Ty::Int` a
    // hardcoded bug would wrongly report.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
            }),
            HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "v".to_string(),
                iter: CompIter::Name("xs".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("v".to_string())),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "v".to_string(),
            var_ty: Ty::Str,
            source: CompSource::List("xs".to_string()),
            cond: None,
            elt: Box::new(MirExpr::Name {
                name: "v".to_string(),
                ty: Ty::Str,
            }),
        })
    );
}

#[test]
fn a_range_sourced_set_comprehension_lowers_to_comp_source_range_with_var_ty_int_and_evaluates_its_filter()
 {
    // Exercises the `SetCompAssign` arm's own `cond: Some(..)` path
    // (distinct closures from `ListCompAssign`'s own, needing their own
    // executing test for D-014's coverage gate).
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::BoolLiteral(true))),
            elt: Box::new(HirExpr::Name("i".to_string())),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "i".to_string(),
            var_ty: Ty::Int,
            source: CompSource::Range {
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(MirExpr::BoolLiteral(true))),
            elt: Box::new(MirExpr::Name {
                name: "i".to_string(),
                ty: Ty::Int,
            }),
        })
    );
}

#[test]
fn a_bare_name_set_sourced_set_comprehension_resolves_comp_source_set() {
    // Exercises `resolve_comp_source`'s `CompIter::Name` branch resolving
    // to `Ty::Set`.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "s".to_string(),
                value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            }),
            HirItem::TopLevelStmt(HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "v".to_string(),
                iter: CompIter::Name("s".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("v".to_string())),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "v".to_string(),
            var_ty: Ty::Int,
            source: CompSource::Set("s".to_string()),
            cond: None,
            elt: Box::new(MirExpr::Name {
                name: "v".to_string(),
                ty: Ty::Int,
            }),
        })
    );
}

#[test]
fn a_bare_name_dict_sourced_dict_comprehension_resolves_comp_source_dict_with_var_ty_as_the_key_type_not_the_value_type()
 {
    // Exercises `resolve_comp_source`'s `CompIter::Name` branch resolving
    // to `Ty::Dict`, and the `DictCompAssign` arm's own `cond: Some(..)`
    // path (distinct closures from `ListCompAssign`/`SetCompAssign`'s
    // own). Pins that `var_ty` is the dict's *key* type (`kv.0`), not its
    // value type (`kv.1`) -- mirrors `ForList`'s own identical
    // `Ty::Dict(kv) => kv.0` choice (`for_k_in_dict_lowers_to_mir_for_dict`
    // above binds a `dict[str, int]`'s loop variable as `Ty::Str`, the
    // key type, for the same reason).
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            }),
            HirItem::TopLevelStmt(HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "k".to_string(),
                iter: CompIter::Name("d".to_string()),
                cond: Some(Box::new(HirExpr::BoolLiteral(true))),
                key: Box::new(HirExpr::Name("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(2)),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "k".to_string(),
            var_ty: Ty::Str,
            source: CompSource::Dict("d".to_string()),
            cond: Some(Box::new(MirExpr::BoolLiteral(true))),
            key: Box::new(MirExpr::Name {
                name: "k".to_string(),
                ty: Ty::Str,
            }),
            value: Box::new(MirExpr::IntLiteral(2)),
        })
    );
}

#[test]
#[should_panic(expected = "neither a list, dict, nor set")]
fn a_comprehension_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error() {
    // Same reasoning as `a_for_list_loop_over_a_non_list_non_dict_non_set_binding_panics_with_an_internal_error`
    // above: `pycc_types` already rejects a comprehension whose bare-name
    // iterable is neither a list, dict, nor set (T0033), but
    // `resolve_comp_source`'s own defensive panic path still needs
    // direct coverage via a hand-built HIR that bypasses that guarantee.
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(5),
            }),
            HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "v".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("v".to_string())),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    build(&hir);
}

// -- Task 8 (D-118): `HirExpr::Slice` -> `MirExpr::Slice` lowering ----

/// Builds `xs = [1, 2, 3]` followed by `y = <slice>` for some
/// `HirExpr::Slice` reading `xs`, mirroring the fixture every Task 6
/// (`pycc_hir`) slicing test starts from, so this lowering is exercised
/// against the same shapes those frontend tests already pin.
fn xs_list_then_slice(slice: HirExpr) -> HirModule {
    HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2),
                    HirExpr::IntLiteral(3),
                ]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: slice,
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

#[test]
fn a_slice_expression_with_both_bounds_present_lowers_with_both_bounds_some() {
    // `xs[1:3]` (mirrors `pycc_hir`'s
    // `lowers_a_slice_expression_with_both_bounds_present`).
    let hir = xs_list_then_slice(HirExpr::Slice {
        base: Box::new(HirExpr::Name("xs".to_string())),
        start: Some(Box::new(HirExpr::IntLiteral(1))),
        stop: Some(Box::new(HirExpr::IntLiteral(3))),
        step: None,
    });
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: Some(Box::new(MirExpr::IntLiteral(1))),
                stop: Some(Box::new(MirExpr::IntLiteral(3))),
                step: None,
            },
        })
    );
}

#[test]
fn a_slice_expression_with_only_the_stop_bound_lowers_with_start_and_step_none() {
    // `xs[:3]` (mirrors `pycc_hir`'s
    // `lowers_a_slice_expression_with_only_the_stop_bound`).
    let hir = xs_list_then_slice(HirExpr::Slice {
        base: Box::new(HirExpr::Name("xs".to_string())),
        start: None,
        stop: Some(Box::new(HirExpr::IntLiteral(3))),
        step: None,
    });
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: None,
                stop: Some(Box::new(MirExpr::IntLiteral(3))),
                step: None,
            },
        })
    );
}

#[test]
fn a_slice_expression_with_only_the_start_bound_lowers_with_stop_and_step_none() {
    // `xs[2:]` (mirrors `pycc_hir`'s
    // `lowers_a_slice_expression_with_only_the_start_bound`).
    let hir = xs_list_then_slice(HirExpr::Slice {
        base: Box::new(HirExpr::Name("xs".to_string())),
        start: Some(Box::new(HirExpr::IntLiteral(2))),
        stop: None,
        step: None,
    });
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: Some(Box::new(MirExpr::IntLiteral(2))),
                stop: None,
                step: None,
            },
        })
    );
}

#[test]
fn a_slice_expression_with_all_bounds_omitted_lowers_with_every_bound_none() {
    // `xs[:]` (mirrors `pycc_hir`'s
    // `lowers_a_slice_expression_with_all_bounds_omitted`).
    let hir = xs_list_then_slice(HirExpr::Slice {
        base: Box::new(HirExpr::Name("xs".to_string())),
        start: None,
        stop: None,
        step: None,
    });
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: None,
                stop: None,
                step: None,
            },
        })
    );
}

#[test]
fn a_slice_expression_with_only_a_step_lowers_with_start_and_stop_none() {
    // `xs[::2]` (mirrors `pycc_hir`'s
    // `lowers_a_slice_expression_with_a_step`).
    let hir = xs_list_then_slice(HirExpr::Slice {
        base: Box::new(HirExpr::Name("xs".to_string())),
        start: None,
        stop: None,
        step: Some(Box::new(HirExpr::IntLiteral(2))),
    });
    let mir = build(&hir);
    assert_eq!(
        mir.items[1],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: None,
                stop: None,
                step: Some(Box::new(MirExpr::IntLiteral(2))),
            },
        })
    );
}

#[test]
fn a_slice_expressions_base_and_every_present_bound_are_recursively_lowered() {
    // `f()`/`g()`/`h()` stand in for "some already-lowerable non-literal
    // shape" -- confirms `base`/`start`/`stop`/`step` are each passed
    // through the real `lower_expr` recursively (mirroring
    // `pycc_hir`'s own `a_slice_expressions_base_and_bounds_are_recursively_lowered`),
    // not merely accepted as raw literals or the base's bare `Name`.
    // Registers `f`/`g`/`h` as zero-arg functions returning `int` so
    // `lower_expr`'s `HirExpr::Call` arm resolves their `ty` via the
    // real `$fn:` lookup instead of panicking.
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
            },
            HirItem::Function {
                name: "g".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
            },
            HirItem::Function {
                name: "h".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
            },
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![
                    HirExpr::IntLiteral(1),
                    HirExpr::IntLiteral(2),
                    HirExpr::IntLiteral(3),
                ]),
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Slice {
                    base: Box::new(HirExpr::Name("xs".to_string())),
                    start: Some(Box::new(HirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                    })),
                    stop: Some(Box::new(HirExpr::Call {
                        callee: "g".to_string(),
                        args: vec![],
                    })),
                    step: Some(Box::new(HirExpr::Call {
                        callee: "h".to_string(),
                        args: vec![],
                    })),
                },
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[4],
        MirItem::TopLevelStmt(MirStmt::Assign {
            target: "y".to_string(),
            value: MirExpr::Slice {
                base: Box::new(MirExpr::Name {
                    name: "xs".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }),
                start: Some(Box::new(MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                    ty: Ty::Int,
                })),
                stop: Some(Box::new(MirExpr::Call {
                    callee: "g".to_string(),
                    args: vec![],
                    ty: Ty::Int,
                })),
                step: Some(Box::new(MirExpr::Call {
                    callee: "h".to_string(),
                    args: vec![],
                    ty: Ty::Int,
                })),
            },
        })
    );
}

#[test]
fn a_slices_ty_derives_from_the_actual_base_type_not_hardcoded_list_of_int() {
    // Mirrors this file's own genericity test for `Subscript`/`ForList`
    // (`list_literal_subscript_and_for_list_derive_their_type_from_actual_elements_not_hardcoded_int`):
    // `MirExpr::Slice`'s `ty()` must derive its result from the actual
    // `base.ty()`, not assume `Ty::List(Box::new(Ty::Int))`.
    // `pycc_types`' T0034 gate means only `list[int]` ever reaches this
    // crate from a real compiled program (a `list[str]` slice is
    // rejected before `pycc_mir` ever sees it), but this crate's own
    // `ty()` must not bake in that assumption independently of the type
    // it actually observes -- so this test bypasses that gate with a
    // hand-built `MirExpr::Slice` over a `list[str]` base, exactly like
    // the `Subscript`/`DictGet` panic tests above bypass gates that
    // can't be reached from a real, type-checked program.
    let slice = MirExpr::Slice {
        base: Box::new(MirExpr::ListLiteral(vec![MirExpr::StringLiteral(
            "a".to_string(),
        )])),
        start: Some(Box::new(MirExpr::IntLiteral(0))),
        stop: None,
        step: None,
    };
    assert_eq!(slice.ty(), Ty::List(Box::new(Ty::Str)));

    // Presence/absence of bounds must not affect `ty()` either --
    // Task 8's brief requirement (c). Compare the all-bounds-omitted
    // shape against the same base.
    let slice_no_bounds = MirExpr::Slice {
        base: Box::new(MirExpr::ListLiteral(vec![MirExpr::StringLiteral(
            "a".to_string(),
        )])),
        start: None,
        stop: None,
        step: None,
    };
    assert_eq!(slice.ty(), slice_no_bounds.ty());
}

// -- D-154 (Part 1 of #375): class instantiation, attribute access,
// method calls --------------------------------------------------------

/// A minimal `Point` class module: `__init__(self, x: int, y: int)`
/// sets both attributes from its own parameters; `bump(self) -> None`
/// reads and mutates `self.x`. Mirrors `pycc_types::class::tests`'s own
/// `point_module` fixture (same shape, this crate's own `HirModule`
/// literal convention).
fn point_module(extra_items: Vec<HirItem>) -> HirModule {
    let self_ty = Ty::Instance(Box::new("Point".to_string()));
    let init = HirItem::Function {
        name: "Point.__init__".to_string(),
        params: vec![
            ("self".to_string(), self_ty.clone()),
            ("x".to_string(), Ty::Int),
            ("y".to_string(), Ty::Int),
        ],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::Name("x".to_string()),
            },
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "y".to_string(),
                value: HirExpr::Name("y".to_string()),
            },
            HirStmt::Return(None),
        ],
    };
    let bump = HirItem::Function {
        name: "Point.bump".to_string(),
        params: vec![("self".to_string(), self_ty)],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::AttrGet {
                        base: Box::new(HirExpr::Name("self".to_string())),
                        attr: "x".to_string(),
                    }),
                    right: Box::new(HirExpr::IntLiteral(1)),
                },
            },
            HirStmt::Return(None),
        ],
    };
    let mut items = vec![init, bump];
    items.extend(extra_items);
    HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Point".to_string(),
            HirClassDef {
                name: "Point".to_string(),
                bases: Vec::new(),
                mro: vec!["Point".to_string()],
                attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Point.__init__".to_string()),
                    ("bump".to_string(), "Point.bump".to_string()),
                ],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    }
}

#[test]
fn instantiation_lowers_to_a_dedicated_mir_node() {
    let hir = point_module(vec![HirItem::TopLevelStmt(HirStmt::Assign {
        target: "p".to_string(),
        value: HirExpr::Call {
            callee: "Point".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        },
    })]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::Assign {
            target: "p".to_string(),
            value: MirExpr::Instantiate(Box::new(InstantiateExpr {
                ctor: "Point.__init__".to_string(),
                attr_count: 2,
                args: vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
                ty: Ty::Instance(Box::new("Point".to_string())),
            })),
        }))
    );
}

#[test]
fn a_method_call_lowers_to_an_ordinary_call_with_self_prepended() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("p".to_string())),
            method: "bump".to_string(),
            args: vec![],
        })),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "Point.bump".to_string(),
            args: vec![MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Instance(Box::new("Point".to_string())),
            }],
            ty: Ty::None,
        })))
    );
}

#[test]
fn a_method_call_with_arguments_lowers_each_argument_after_self() {
    // `point_module`'s own `bump` method takes no extra parameters, so
    // `a_method_call_lowers_to_an_ordinary_call_with_self_prepended`
    // above never exercises `MethodCall`'s own per-argument lowering
    // loop -- a standalone minimal `Counter.add(self, n: int) -> None`
    // fixture here does.
    let self_ty = Ty::Instance(Box::new("Counter".to_string()));
    let init = HirItem::Function {
        name: "Counter.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "n".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let add = HirItem::Function {
        name: "Counter.add".to_string(),
        params: vec![
            ("self".to_string(), self_ty.clone()),
            ("n".to_string(), Ty::Int),
        ],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    let hir = HirModule {
        items: vec![
            init,
            add,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "Counter".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("c".to_string())),
                method: "add".to_string(),
                args: vec![HirExpr::IntLiteral(5)],
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Counter".to_string(),
            HirClassDef {
                name: "Counter".to_string(),
                bases: Vec::new(),
                mro: vec!["Counter".to_string()],
                attrs: vec![("n".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Counter.__init__".to_string()),
                    ("add".to_string(), "Counter.add".to_string()),
                ],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "Counter.add".to_string(),
            args: vec![
                MirExpr::Name {
                    name: "c".to_string(),
                    ty: self_ty,
                },
                MirExpr::IntLiteral(5),
            ],
            ty: Ty::None,
        })))
    );
}

#[test]
fn an_attribute_read_lowers_to_a_slot_index() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("p".to_string())),
                attr: "y".to_string(),
            }],
        })),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::AttrGet {
                base: Box::new(MirExpr::Name {
                    name: "p".to_string(),
                    ty: Ty::Instance(Box::new("Point".to_string())),
                }),
                slot: 1,
                ty: Ty::Int,
            }],
            ty: Ty::None,
        })))
    );
}

#[test]
fn an_attribute_write_lowers_to_a_slot_index() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::AttrSet {
            base: HirExpr::Name("p".to_string()),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(9),
        }),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::AttrSet {
            base: MirExpr::Name {
                name: "p".to_string(),
                ty: Ty::Instance(Box::new("Point".to_string())),
            },
            slot: 0,
            value: MirExpr::IntLiteral(9),
        }))
    );
}

#[test]
fn the_init_and_bump_methods_themselves_lower_with_self_typed_as_the_instance() {
    // Exercises `__init__`'s own body -- two `HirStmt::AttrSet`s whose
    // RHS is a bare parameter reference -- and `bump`'s own body -- an
    // `HirExpr::AttrGet` nested inside a `BinOp` -- end to end through
    // `build`, not just the module-scope exercises above. Direct value
    // comparisons against `mir.items[0]`/`[1]`, not a
    // `let PATTERN = .. else { panic!(..) }` destructure -- this file's
    // own established coverage-gate convention (see e.g.
    // `class_instantiation_...` tests elsewhere): a hand-written panic
    // arm never taken on the happy path is a permanently uncovered
    // region under D-014's 100%-region gate.
    let hir = point_module(vec![]);
    let mir = build(&hir);
    let self_ty = Ty::Instance(Box::new("Point".to_string()));
    assert_eq!(
        mir.items[0],
        MirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 1,
                    value: MirExpr::Name {
                        name: "y".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        }
    );
    assert_eq!(
        mir.items[1],
        MirItem::Function {
            name: "Point.bump".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::AttrGet {
                            base: Box::new(MirExpr::Name {
                                name: "self".to_string(),
                                ty: self_ty.clone(),
                            }),
                            slot: 0,
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        }
    );
}

#[test]
#[should_panic(expected = "expected an instance- or protocol-typed expression")]
fn attr_get_on_a_non_instance_base_panics_with_an_internal_error() {
    // Bypasses `pycc_types::check` (which would reject this) with a
    // hand-built `HirExpr::AttrGet` over an `int`-typed base, matching
    // this file's own established internal-error-test convention (see
    // e.g. `referencing_an_unbound_name_panics_with_an_internal_error`
    // above).
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("x".to_string())),
                attr: "y".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "class `Ghost` has no registered HirClassDef")]
fn attr_get_over_an_instance_typed_parameter_from_an_unregistered_class_panics_with_an_internal_error()
 {
    // `class_def_of`'s *second* panic (as opposed to
    // `attr_get_on_a_non_instance_base_panics_with_an_internal_error`
    // above, which exercises its first): `x`'s own declared parameter
    // type names `Ghost`, but `hir.class_defs` never registers it --
    // parameter binding itself never consults `classes` at all, so
    // this reaches `class_def_of`'s own `classes.get(..)` lookup with a
    // genuinely instance-typed expression pointing at a class this
    // module's own side table has no entry for.
    let ghost_ty = Ty::Instance(Box::new("Ghost".to_string()));
    let hir = HirModule {
        items: vec![HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), ghost_ty)],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ExprStmt(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    attr: "y".to_string(),
                }),
                HirStmt::Return(None),
            ],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "attribute `z` not declared on class `Point`")]
fn attr_get_of_an_undeclared_attribute_panics_with_an_internal_error() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("p".to_string())),
            attr: "z".to_string(),
        })),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "method `fly` not declared on class `Point`")]
fn method_call_of_an_undeclared_method_panics_with_an_internal_error() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("p".to_string())),
            method: "fly".to_string(),
            args: vec![],
        })),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "attribute `z` not declared on class `Point`")]
fn attr_set_of_an_undeclared_attribute_panics_with_an_internal_error() {
    let hir = point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::AttrSet {
            base: HirExpr::Name("p".to_string()),
            attr: "z".to_string(),
            value: HirExpr::IntLiteral(1),
        }),
    ]);
    let _ = build(&hir);
}

// #377: a read-only `@property` (no setter) should never reach MIR
// lowering -- `pycc_types::check` rejects the assignment with `T0044`
// before MIR runs. This test bypasses the type checker with a hand-built
// HIR to exercise the panic arm, matching this file's own established
// internal-error-test convention (see e.g.
// `attr_set_of_an_undeclared_attribute_panics_with_an_internal_error`
// above).
#[test]
#[should_panic(expected = "pycc_mir: internal error: property `val` on class `Box` has no setter")]
fn attr_set_on_a_read_only_property_panics_with_an_internal_error() {
    use pycc_hir::{HirClassDef, PropertyDef};
    let self_ty = Ty::Instance(Box::new("Box".to_string()));
    let init = HirItem::Function {
        name: "Box.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "_val".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let getter = HirItem::Function {
        name: "Box.val".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("self".to_string())),
            attr: "_val".to_string(),
        }))],
    };
    let hir = HirModule {
        items: vec![
            init,
            getter,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "b".to_string(),
                value: HirExpr::Call {
                    callee: "Box".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::AttrSet {
                base: HirExpr::Name("b".to_string()),
                attr: "val".to_string(),
                value: HirExpr::IntLiteral(42),
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Box".to_string(),
            HirClassDef {
                name: "Box".to_string(),
                bases: Vec::new(),
                mro: vec!["Box".to_string()],
                attrs: vec![("_val".to_string(), Ty::Int)],
                methods: vec![("__init__".to_string(), "Box.__init__".to_string())],
                properties: vec![PropertyDef {
                    name: "val".to_string(),
                    getter: "Box.val".to_string(),
                    setter: None,
                }],
                type_param: None,
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

// PEP 695 (#387): `GenericClassInstantiate` should never reach MIR —
// `pycc_types::monomorphize` rewrites every such expression to an
// ordinary `HirExpr::Call` before MIR lowering runs. This test bypasses
// the type checker with a hand-built HIR to exercise the panic arm,
// matching this file's own established internal-error-test convention.
#[test]
#[should_panic(
    expected = "pycc_mir: internal error: `GenericClassInstantiate` for class `C` reached MIR lowering"
)]
fn generic_class_instantiate_reaching_mir_panics_with_an_internal_error() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::GenericClassInstantiate {
                class: "C".to_string(),
                type_arg: Ty::Int,
                args: vec![HirExpr::IntLiteral(1)],
            },
        ))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

// #433: a bare `HirExpr::Super` should never reach MIR lowering —
// HIR lowering rejects a standalone `super()` with C0001, and
// `super().method()`/`super().attr` are handled by the special-case
// blocks before recursing into `lower_expr` for the base. This test
// bypasses the type checker with a hand-built HIR to exercise the
// panic arm, matching this file's own established internal-error-test
// convention.
#[test]
#[should_panic(expected = "pycc_mir: internal error: a bare `HirExpr::Super` reached MIR lowering")]
fn bare_super_reaching_mir_panics_with_an_internal_error() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Super))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

// #449: `current_class.expect(...)` panic paths in the Super-base blocks
// of `HirExpr::AttrGet` and `HirExpr::MethodCall`. A `HirExpr::Super` as
// the base of an `AttrGet` or `MethodCall` at top level (outside a method
// body, where `current_class` is `None`) should never reach MIR lowering
// — HIR lowering rejects it with C0001. These tests bypass the type
// checker with a hand-built HIR to exercise the panic arms, matching this
// file's own established internal-error-test convention.

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a method body"
)]
fn super_attr_get_outside_method_panics_with_an_internal_error() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(HirExpr::Super),
            attr: "x".to_string(),
        }))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: `HirExpr::Super` reached lower_expr outside a method body"
)]
fn super_method_call_outside_method_panics_with_an_internal_error() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Super),
                method: "f".to_string(),
                args: vec![],
            },
        ))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let _ = build(&hir);
}

// -- #432: MRO internal-error panic tests --------------------------------
//
// Every test below bypasses `pycc_types::check` (which would reject
// this) with a hand-built HIR whose class's MRO references a class the
// `classes` map has no entry for -- exactly the "MRO and classes side
// table disagree" scenario each MRO-walk's own doc comment names as
// unreachable from any real `check`-validated program, mirroring this
// file's own established convention for internal-consistency panics.

/// Builds a HIR module with a `Derived` class whose MRO lists
/// `["Derived", "Ghost"]`, where `Ghost` is deliberately absent from
/// `class_defs`. The `body_fn` closure receives the `self`-typed
/// parameter name and returns the function body that triggers the
/// specific MRO walk to test -- using a function parameter (not
/// `Instantiate`) avoids the `mro_attr_count` → `mro_attrs` panic
/// firing first, so the `AttrGet`/`AttrSet`/`MethodCall` MRO walk
/// panic is the one actually reached.
fn ghost_mro_param_module(body: Vec<HirStmt>) -> HirModule {
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    // The `__init__` body is deliberately empty (`return` only) -- an
    // `AttrSet` on `self` would itself walk the MRO and panic on the
    // absent `Ghost` entry before the `use_derived` function body (the
    // one that actually tests `AttrGet`/`MethodCall`) is ever lowered.
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    // A function whose parameter is typed `Derived` -- the MRO walk
    // for `AttrGet`/`AttrSet`/`MethodCall` on this parameter triggers
    // the ghost-class panic without going through `Instantiate`.
    let user_fn = HirItem::Function {
        name: "use_derived".to_string(),
        params: vec![("d".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body,
    };
    HirModule {
        items: vec![init, user_fn],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Derived".to_string(),
            HirClassDef {
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: vec![("x".to_string(), Ty::Int)],
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    }
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn attr_set_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    let hir = ghost_mro_param_module(vec![
        HirStmt::AttrSet {
            base: HirExpr::Name("d".to_string()),
            attr: "x".to_string(),
            value: HirExpr::IntLiteral(42),
        },
        HirStmt::Return(None),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn attr_get_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    let hir = ghost_mro_param_module(vec![
        HirStmt::ExprStmt(HirExpr::AttrGet {
            base: Box::new(HirExpr::Name("d".to_string())),
            attr: "x".to_string(),
        }),
        HirStmt::Return(None),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn method_call_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    let hir = ghost_mro_param_module(vec![
        HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("d".to_string())),
            method: "f".to_string(),
            args: vec![],
        }),
        HirStmt::Return(None),
    ]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn mro_attrs_with_a_ghost_class_in_the_mro_panics_with_an_internal_error() {
    // `Instantiate` calls `mro_attr_count` → `mro_attrs`, which walks
    // the full MRO. The `__init__` MRO walk (using `?`, not panicking)
    // finds `Derived.__init__` first and returns early, so
    // `mro_attr_count` is reached -- and its `mro_attrs` call panics on
    // the absent `Ghost` entry.
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            HirStmt::Return(None),
        ],
    };
    let hir = HirModule {
        items: vec![
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![],
                },
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Derived".to_string(),
            HirClassDef {
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: vec![("x".to_string(), Ty::Int)],
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(expected = "pycc_mir: internal error: no `__init__` found in class `C`'s MRO")]
fn instantiate_with_no_init_in_the_mro_panics_with_an_internal_error() {
    // #432: `Instantiate`'s `__init__` MRO walk uses `?` (not
    // panicking) when a class is not in the `classes` map. Including
    // `Ghost` in the MRO (but not in `class_defs`) exercises that `?`
    // arm -- `C` is found but has no `__init__`, then `Ghost` is not
    // found, so `find_map` returns `None` and the `unwrap_or_else`
    // panic fires.
    use pycc_hir::HirClassDef;
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
            target: "c".to_string(),
            value: HirExpr::Call {
                callee: "C".to_string(),
                args: vec![],
            },
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "C".to_string(),
            HirClassDef {
                name: "C".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["C".to_string(), "Ghost".to_string()],
                attrs: vec![],
                methods: vec![("f".to_string(), "C.f".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

#[test]
fn mro_attrs_deduplicates_a_redeclared_attribute_across_the_mro() {
    // #432: a derived class that re-declares an attribute of the same
    // name as a base class "wins" (its declaration appears first in the
    // MRO). `mro_attrs`'s `seen` set skips the base class's duplicate
    // entry, so the flat layout has exactly one slot for `x`, not two.
    // This exercises the `if seen.insert(..)` false branch (the
    // already-seen skip), which is otherwise uncovered when no test
    // re-declares an attribute across the MRO.
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            HirStmt::Return(None),
        ],
    };
    let base_init = HirItem::Function {
        name: "Base.__init__".to_string(),
        params: vec![(
            "self".to_string(),
            Ty::Instance(Box::new("Base".to_string())),
        )],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let hir = HirModule {
        items: vec![
            base_init,
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![],
                },
            }),
            // AttrGet on `d.x` triggers `mro_attrs`, which walks the
            // MRO and deduplicates `x` (declared in both `Derived`
            // and `Base`).
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("d".to_string())),
                attr: "x".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "Base".to_string(),
                HirClassDef {
                    name: "Base".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "Derived".to_string(),
                HirClassDef {
                    name: "Derived".to_string(),
                    bases: vec!["Base".to_string()],
                    mro: vec!["Derived".to_string(), "Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    };
    let mir = build(&hir);
    // The Instantiate node should allocate exactly 1 slot (not 2),
    // because `x` is deduplicated across the MRO.
    let instantiate = mir
        .items
        .iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::Assign {
                value: MirExpr::Instantiate(inst),
                ..
            }) => Some(inst),
            _ => None,
        })
        .expect("expected an Instantiate node");
    assert_eq!(instantiate.attr_count, 1);
}

#[test]
fn mro_attrs_overrides_type_for_a_redeclared_attribute_with_a_different_type() {
    // #432: when a derived class re-declares an attribute with a
    // different type than the base, the most-derived declaration's
    // type wins (pass 2 of `mro_attrs`). This exercises the
    // `result[idx].1 = ty.clone()` line in the second pass.
    use pycc_hir::HirClassDef;
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let init = HirItem::Function {
        name: "Derived.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::FloatLiteral(1.0),
            },
            HirStmt::Return(None),
        ],
    };
    let base_init = HirItem::Function {
        name: "Base.__init__".to_string(),
        params: vec![(
            "self".to_string(),
            Ty::Instance(Box::new("Base".to_string())),
        )],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::IntLiteral(0),
            },
            HirStmt::Return(None),
        ],
    };
    let hir = HirModule {
        items: vec![
            base_init,
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "d".to_string(),
                value: HirExpr::Call {
                    callee: "Derived".to_string(),
                    args: vec![],
                },
            }),
            // AttrGet on `d.x` triggers `mro_attrs`, which walks the
            // MRO and overrides `x`'s type from Int (Base) to Float
            // (Derived) in the second pass.
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("d".to_string())),
                attr: "x".to_string(),
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "Base".to_string(),
                HirClassDef {
                    name: "Base".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "Base.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "Derived".to_string(),
                HirClassDef {
                    name: "Derived".to_string(),
                    bases: vec!["Base".to_string()],
                    mro: vec!["Derived".to_string(), "Base".to_string()],
                    attrs: vec![("x".to_string(), Ty::Float)],
                    methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    };
    let mir = build(&hir);
    // The AttrGet node should have type Float (Derived's declaration
    // wins), not Int (Base's declaration).
    let attr_get = mir
        .items
        .iter()
        .find_map(|item| match item {
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::AttrGet { ty, .. })) => {
                Some(ty.clone())
            }
            _ => None,
        })
        .expect("expected an AttrGet node");
    assert_eq!(
        attr_get,
        Ty::Float,
        "re-declared attribute should use the most-derived type (Float)"
    );
}

// #433: super() MIR lowering tests.

/// Helper: builds a minimal two-class HIR module where `B.__init__`
/// calls `super().__init__()` and `B.greet` calls `super().greet()`.
fn super_module() -> HirModule {
    let self_a = Ty::Instance(Box::new("A".to_string()));
    let self_b = Ty::Instance(Box::new("B".to_string()));
    HirModule {
        items: vec![
            // A.__init__
            HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), self_a.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }],
            },
            // A.greet
            HirItem::Function {
                name: "A.greet".to_string(),
                params: vec![("self".to_string(), self_a.clone())],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: "x".to_string(),
                }))],
            },
            // B.__init__ — calls super().__init__()
            HirItem::Function {
                name: "B.__init__".to_string(),
                params: vec![("self".to_string(), self_b.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Super),
                    method: "__init__".to_string(),
                    args: vec![],
                })],
            },
            // B.greet — calls super().greet()
            HirItem::Function {
                name: "B.greet".to_string(),
                params: vec![("self".to_string(), self_b)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Super),
                    method: "greet".to_string(),
                    args: vec![],
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![
                        ("__init__".to_string(), "A.__init__".to_string()),
                        ("greet".to_string(), "A.greet".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: Vec::new(),
                    methods: vec![
                        ("__init__".to_string(), "B.__init__".to_string()),
                        ("greet".to_string(), "B.greet".to_string()),
                    ],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    }
}

#[test]
fn super_init_lowers_to_direct_call_to_base_init() {
    let hir = super_module();
    let mir = build(&hir);
    let init = mir.items.iter().find_map(|item| match item {
        MirItem::Function { name, body, .. } if name == "B.__init__" => body.first(),
        _ => None,
    });
    assert_eq!(
        init,
        Some(&MirStmt::ExprStmt(MirExpr::Call {
            callee: "A.__init__".to_string(),
            args: vec![MirExpr::Name {
                name: "self".to_string(),
                ty: Ty::Instance(Box::new("B".to_string())),
            }],
            ty: Ty::None,
        }))
    );
}

#[test]
fn super_method_lowers_to_direct_call_to_base_method() {
    let hir = super_module();
    let mir = build(&hir);
    let greet = mir.items.iter().find_map(|item| match item {
        MirItem::Function { name, body, .. } if name == "B.greet" => body.first(),
        _ => None,
    });
    assert_eq!(
        greet,
        Some(&MirStmt::Return(Some(MirExpr::Call {
            callee: "A.greet".to_string(),
            args: vec![MirExpr::Name {
                name: "self".to_string(),
                ty: Ty::Instance(Box::new("B".to_string())),
            }],
            ty: Ty::Int,
        })))
    );
}

#[test]
#[should_panic(expected = "is not a property on any class after `B` in its MRO")]
fn super_attr_get_naming_an_instance_attr_panics_with_an_internal_error() {
    // #587: `super().x` where `x` is an instance attribute is rejected
    // by `pycc_types::class::resolve_super_attr_get` with `T0047`, so
    // this HIR shape cannot reach `pycc_mir` through the real pipeline.
    // Constructing it directly exercises the arm's panic-on-inconsistency
    // guard, which replaced the slot-read lowering this test previously
    // asserted (`super_attr_lowers_to_attr_get_with_self_base`).
    let self_a = Ty::Instance(Box::new("A".to_string()));
    let self_b = Ty::Instance(Box::new("B".to_string()));
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), self_a)],
                return_ty: Ty::None,
                body: vec![HirStmt::AttrSet {
                    base: HirExpr::Name("self".to_string()),
                    attr: "x".to_string(),
                    value: HirExpr::IntLiteral(42),
                }],
            },
            HirItem::Function {
                name: "B.get_x".to_string(),
                params: vec![("self".to_string(), self_b)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Super),
                    attr: "x".to_string(),
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("x".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("get_x".to_string(), "B.get_x".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    };
    build(&hir);
}

#[test]
fn super_property_lowers_to_call_to_base_getter() {
    use pycc_hir::PropertyDef;
    let self_a = Ty::Instance(Box::new("A".to_string()));
    let self_b = Ty::Instance(Box::new("B".to_string()));
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "A.__init__".to_string(),
                params: vec![("self".to_string(), self_a.clone())],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::AttrSet {
                        base: HirExpr::Name("self".to_string()),
                        attr: "_val".to_string(),
                        value: HirExpr::IntLiteral(0),
                    },
                    HirStmt::Return(None),
                ],
            },
            HirItem::Function {
                name: "A.val".to_string(),
                params: vec![("self".to_string(), self_a)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Name("self".to_string())),
                    attr: "_val".to_string(),
                }))],
            },
            HirItem::Function {
                name: "B.get_val".to_string(),
                params: vec![("self".to_string(), self_b)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Super),
                    attr: "val".to_string(),
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "A".to_string(),
                HirClassDef {
                    name: "A".to_string(),
                    bases: vec![],
                    mro: vec!["A".to_string()],
                    attrs: vec![("_val".to_string(), Ty::Int)],
                    methods: vec![("__init__".to_string(), "A.__init__".to_string())],
                    type_param: None,
                    properties: vec![PropertyDef {
                        name: "val".to_string(),
                        getter: "A.val".to_string(),
                        setter: None,
                    }],
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "B".to_string(),
                HirClassDef {
                    name: "B".to_string(),
                    bases: vec!["A".to_string()],
                    mro: vec!["B".to_string(), "A".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("get_val".to_string(), "B.get_val".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    };
    let mir = build(&hir);
    let get_val = mir.items.iter().find_map(|item| match item {
        MirItem::Function { name, body, .. } if name == "B.get_val" => body.first(),
        _ => None,
    });
    assert_eq!(
        get_val,
        Some(&MirStmt::Return(Some(MirExpr::Call {
            callee: "A.val".to_string(),
            args: vec![MirExpr::Name {
                name: "self".to_string(),
                ty: Ty::Instance(Box::new("B".to_string())),
            }],
            ty: Ty::Int,
        })))
    );
}

// -- #436: @staticmethod / @classmethod MIR lowering --------------------

/// Builds a minimal module with a class `C` that has `__init__`, a
/// static method `create(x: int) -> int`, and a class method
/// `greet(cls, x: int) -> int`. Used by the #436 MIR tests below.
fn static_class_hir(extra_items: Vec<HirItem>) -> HirModule {
    let self_ty = Ty::Instance(Box::new("C".to_string()));
    let init = HirItem::Function {
        name: "C.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    let static_fn = HirItem::Function {
        name: "C.create.static".to_string(),
        params: vec![("x".to_string(), Ty::Int)],
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
    };
    let class_fn = HirItem::Function {
        name: "C.greet.classmethod".to_string(),
        params: vec![
            ("cls".to_string(), self_ty.clone()),
            ("x".to_string(), Ty::Int),
        ],
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
    };
    let mut items = vec![init, static_fn, class_fn];
    items.extend(extra_items);
    HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "C".to_string(),
            pycc_hir::HirClassDef {
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("create".to_string(), "C.create.static".to_string())],
                class_methods: vec![("greet".to_string(), "C.greet.classmethod".to_string())],
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    }
}

#[test]
fn a_static_method_call_through_class_lowers_without_a_receiver() {
    let hir = static_class_hir(vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
        HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("C".to_string())),
            method: "create".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        },
    ))]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "C.create.static".to_string(),
            args: vec![MirExpr::IntLiteral(42)],
            ty: Ty::Int,
        })))
    );
}

#[test]
fn a_class_method_call_through_class_lowers_with_null_instance_receiver() {
    let hir = static_class_hir(vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
        HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("C".to_string())),
            method: "greet".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        },
    ))]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "C.greet.classmethod".to_string(),
            args: vec![
                MirExpr::NullInstance {
                    ty: Ty::Instance(Box::new("C".to_string())),
                },
                MirExpr::IntLiteral(42),
            ],
            ty: Ty::Int,
        })))
    );
}

#[test]
fn a_static_method_call_through_instance_lowers_without_a_receiver() {
    let hir = static_class_hir(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "c".to_string(),
            value: HirExpr::Call {
                callee: "C".to_string(),
                args: vec![],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("c".to_string())),
            method: "create".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        })),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "C.create.static".to_string(),
            args: vec![MirExpr::IntLiteral(42)],
            ty: Ty::Int,
        })))
    );
}

#[test]
fn a_class_method_call_through_instance_lowers_with_instance_as_cls() {
    let hir = static_class_hir(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "c".to_string(),
            value: HirExpr::Call {
                callee: "C".to_string(),
                args: vec![],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("c".to_string())),
            method: "greet".to_string(),
            args: vec![HirExpr::IntLiteral(42)],
        })),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "C.greet.classmethod".to_string(),
            args: vec![
                MirExpr::Name {
                    name: "c".to_string(),
                    ty: Ty::Instance(Box::new("C".to_string())),
                },
                MirExpr::IntLiteral(42),
            ],
            ty: Ty::Int,
        })))
    );
}

#[test]
fn null_instance_ty_returns_the_stored_type() {
    let expr = MirExpr::NullInstance {
        ty: Ty::Instance(Box::new("C".to_string())),
    };
    assert_eq!(expr.ty(), Ty::Instance(Box::new("C".to_string())));
}

#[test]
#[should_panic(expected = "pycc_mir: internal error: `C` has no recorded type")]
fn class_name_method_call_with_no_static_or_class_method_falls_through_to_instance_path() {
    // When a `MethodCall` on a class name does not find the method in
    // `static_methods` or `class_methods`, the code falls through to the
    // instance-receiver path, which calls `lower_expr` on the bare class
    // name. Since class names are not in the scope, `lookup` panics.
    // This covers the `None` branch of the class-name `class_mangled`
    // `if let` (the fallthrough past the class-name interception block).
    let hir = static_class_hir(vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
        HirExpr::MethodCall {
            base: Box::new(HirExpr::Name("C".to_string())),
            method: "nonexistent".to_string(),
            args: vec![],
        },
    ))]);
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn static_method_call_through_class_name_with_ghost_mro_panics() {
    // A `MethodCall` on a class name whose MRO contains a ghost class
    // triggers the defensive panic in the static_methods MRO walk. The
    // method name `missing` is not in `C`'s own static_methods, so the
    // `find_map` continues to `Ghost` and panics.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "missing".to_string(),
                args: vec![],
            },
        ))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "C".to_string(),
            pycc_hir::HirClassDef {
                name: "C".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["C".to_string(), "Ghost".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn class_method_call_through_class_name_with_ghost_mro_panics() {
    // Same as above but exercises the class_methods MRO walk. The
    // method name `missing` is not in `C`'s own static_methods or
    // class_methods, so both walks reach `Ghost` and the second one
    // (class_methods) panics.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::MethodCall {
                base: Box::new(HirExpr::Name("C".to_string())),
                method: "missing".to_string(),
                args: vec![],
            },
        ))],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "C".to_string(),
            pycc_hir::HirClassDef {
                name: "C".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["C".to_string(), "Ghost".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("found".to_string(), "C.found.static".to_string())],
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn static_method_call_through_instance_with_ghost_mro_panics() {
    // A `MethodCall` on an instance whose class MRO contains a ghost
    // class triggers the defensive panic in the instance-receiver
    // static_methods MRO walk.
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "Derived.__init__".to_string(),
                params: vec![("self".to_string(), self_ty.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::Return(None)],
            },
            HirItem::Function {
                name: "use_derived".to_string(),
                params: vec![("d".to_string(), self_ty.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    method: "missing".to_string(),
                    args: vec![],
                })],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Derived".to_string(),
            pycc_hir::HirClassDef {
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

#[test]
#[should_panic(
    expected = "pycc_mir: internal error: class `Ghost` in MRO has no registered HirClassDef"
)]
fn class_method_call_through_instance_with_ghost_mro_panics() {
    // Same as above but exercises the instance-receiver class_methods
    // MRO walk. `Derived` has a static method `found` so the
    // static_methods walk does not reach `Ghost`, but the class_methods
    // walk does.
    let self_ty = Ty::Instance(Box::new("Derived".to_string()));
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "Derived.__init__".to_string(),
                params: vec![("self".to_string(), self_ty.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::Return(None)],
            },
            HirItem::Function {
                name: "use_derived".to_string(),
                params: vec![("d".to_string(), self_ty.clone())],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("d".to_string())),
                    method: "missing".to_string(),
                    args: vec![],
                })],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Derived".to_string(),
            pycc_hir::HirClassDef {
                name: "Derived".to_string(),
                bases: vec!["Ghost".to_string()],
                mro: vec!["Derived".to_string(), "Ghost".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "Derived.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: vec![("found".to_string(), "Derived.found.static".to_string())],
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let _ = build(&hir);
}

// -----------------------------------------------------------------------
// #435: isinstance/issubclass MIR lowering unit tests
// -----------------------------------------------------------------------

#[test]
fn isinstance_lowers_to_bool_literal_for_user_class() {
    // `isinstance(D(), D)` — D is in D's MRO, so the result is `true`.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::Call {
                        callee: "D".to_string(),
                        args: vec![],
                    },
                    HirExpr::Name("D".to_string()),
                ],
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![(
            "D".to_string(),
            HirClassDef {
                name: "D".to_string(),
                bases: vec![],
                mro: vec!["D".to_string()],
                methods: vec![("__init__".to_string(), "D.__init__".to_string())],
                attrs: vec![],
                static_methods: vec![],
                class_methods: vec![],
                properties: vec![],
                type_param: None,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::BoolLiteral(true)],
            ty: Ty::None,
        }))
    );
}

#[test]
fn issubclass_lowers_to_bool_literal_for_same_class() {
    // `issubclass(D, D)` — D is in D's MRO, so the result is `true`.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![
                    HirExpr::Name("D".to_string()),
                    HirExpr::Name("D".to_string()),
                ],
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![(
            "D".to_string(),
            HirClassDef {
                name: "D".to_string(),
                bases: vec![],
                mro: vec!["D".to_string()],
                methods: vec![("__init__".to_string(), "D.__init__".to_string())],
                attrs: vec![],
                static_methods: vec![],
                class_methods: vec![],
                properties: vec![],
                type_param: None,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::BoolLiteral(true)],
            ty: Ty::None,
        }))
    );
}

#[test]
fn isinstance_with_float_lowers_to_bool_literal_true() {
    // `isinstance(1.5, float)` — covers the `Ty::Float` arm in
    // `eval_isinstance_single` at the MIR level.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::FloatLiteral(1.5),
                    HirExpr::Name("float".to_string()),
                ],
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::BoolLiteral(true)],
            ty: Ty::None,
        }))
    );
}

#[test]
fn issubclass_with_int_int_lowers_to_bool_literal_true() {
    // `issubclass(int, int)` — covers the `return cls == target_class`
    // line in `eval_issubclass_single` at the MIR level.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![
                    HirExpr::Name("int".to_string()),
                    HirExpr::Name("int".to_string()),
                ],
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::BoolLiteral(true)],
            ty: Ty::None,
        }))
    );
}

#[test]
fn issubclass_with_user_class_vs_builtin_lowers_to_false() {
    // `issubclass(D, int)` — user class vs builtin target, covers the
    // `return false` line in `eval_issubclass_single`.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![
                    HirExpr::Name("D".to_string()),
                    HirExpr::Name("int".to_string()),
                ],
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![(
            "D".to_string(),
            HirClassDef {
                name: "D".to_string(),
                bases: vec![],
                mro: vec!["D".to_string()],
                methods: vec![("__init__".to_string(), "D.__init__".to_string())],
                attrs: vec![],
                static_methods: vec![],
                class_methods: vec![],
                properties: vec![],
                type_param: None,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::BoolLiteral(false)],
            ty: Ty::None,
        }))
    );
}

#[test]
#[should_panic(expected = "internal error: issubclass's first argument")]
fn issubclass_with_non_name_first_arg_panics() {
    // This covers the `unreachable!` branch in `lower_issubclass`.
    // In practice, the type checker rejects this before MIR lowering,
    // but the MIR function must handle the case defensively.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Call {
                callee: "issubclass".to_string(),
                args: vec![HirExpr::IntLiteral(42), HirExpr::Name("int".to_string())],
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![],
    };
    let _ = build(&hir);
}

#[test]
fn enum_member_attr_get_lowers_to_synthetic_global() {
    // #379: `Color.RED` lowers to `MirExpr::Name` reading the
    // synthetic `<Class>.<Member>.enum_member` global.
    let class_def = pycc_hir::HirClassDef {
        name: "Color".to_string(),
        bases: vec![],
        mro: vec!["Color".to_string()],
        attrs: vec![
            ("value".to_string(), Ty::Int),
            ("name".to_string(), Ty::Str),
        ],
        methods: vec![],
        properties: vec![],
        static_methods: vec![],
        class_methods: vec![],
        type_param: None,
        enum_members: vec![("RED".to_string(), 1), ("GREEN".to_string(), 2)],
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::AttrGet {
                base: Box::new(HirExpr::Name("Color".to_string())),
                attr: "RED".to_string(),
            }],
        }))],
        type_aliases: vec![],
        imports: vec![],
        class_defs: vec![("Color".to_string(), class_def)],
    };
    let mir = build(&hir);
    assert_eq!(
        mir.items[0],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "Color.RED.enum_member".to_string(),
                ty: Ty::Instance(Box::new("Color".to_string())),
            }],
            ty: Ty::None,
        }))
    );
}

// -- #378 (PR-18): dataclass __eq__ / __repr__ MIR rewrites -----------

/// A minimal dataclass-like `Point` module with `__init__`, `__eq__`,
/// and `__repr__` methods registered in the class definition. Used by
/// the dataclass MIR-rewrite tests below.
fn dataclass_point_module(extra_items: Vec<HirItem>) -> HirModule {
    let self_ty = Ty::Instance(Box::new("Point".to_string()));
    let init = HirItem::Function {
        name: "Point.__init__".to_string(),
        params: vec![
            ("self".to_string(), self_ty.clone()),
            ("x".to_string(), Ty::Int),
            ("y".to_string(), Ty::Int),
        ],
        return_ty: Ty::None,
        body: vec![
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "x".to_string(),
                value: HirExpr::Name("x".to_string()),
            },
            HirStmt::AttrSet {
                base: HirExpr::Name("self".to_string()),
                attr: "y".to_string(),
                value: HirExpr::Name("y".to_string()),
            },
            HirStmt::Return(None),
        ],
    };
    let eq = HirItem::Function {
        name: "Point.__eq__".to_string(),
        params: vec![
            ("self".to_string(), self_ty.clone()),
            ("other".to_string(), self_ty.clone()),
        ],
        return_ty: Ty::Bool,
        body: vec![HirStmt::Return(Some(HirExpr::BoolLiteral(true)))],
    };
    let repr = HirItem::Function {
        name: "Point.__repr__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::Str,
        body: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
            "Point()".to_string(),
        )))],
    };
    let mut items = vec![init, eq, repr];
    items.extend(extra_items);
    HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Point".to_string(),
            HirClassDef {
                name: "Point".to_string(),
                bases: Vec::new(),
                mro: vec!["Point".to_string()],
                attrs: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                methods: vec![
                    ("__init__".to_string(), "Point.__init__".to_string()),
                    ("__eq__".to_string(), "Point.__eq__".to_string()),
                    ("__repr__".to_string(), "Point.__repr__".to_string()),
                ],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: true,
                dataclass_fields: vec![("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    }
}

#[test]
fn an_eq_comparison_between_same_class_instances_lowers_to_eq_call() {
    let hir = dataclass_point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "q".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "r".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::Eq,
                left: Box::new(HirExpr::Name("p".to_string())),
                right: Box::new(HirExpr::Name("q".to_string())),
            },
        }),
    ]);
    let mir = build(&hir);
    // Items: [__init__, __eq__, __repr__, Assign p, Assign q, Assign r].
    // The comparison is at index 5. Using `matches!` with a guard
    // avoids a hand-written `other => panic!(...)` arm that would be
    // permanently uncovered under D-014's 100%-line-coverage gate
    // (the happy path always matches the expected pattern).
    assert!(matches!(
        &mir.items[5],
        MirItem::TopLevelStmt(MirStmt::Assign {
            value: MirExpr::Call { callee, .. },
            ..
        }) if callee == "Point.__eq__"
    ));
}

#[test]
fn a_neq_comparison_between_same_class_instances_lowers_to_neq_of_eq_call() {
    let hir = dataclass_point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "q".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "r".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::NotEq,
                left: Box::new(HirExpr::Name("p".to_string())),
                right: Box::new(HirExpr::Name("q".to_string())),
            },
        }),
    ]);
    let mir = build(&hir);
    // The NotEq arm wraps the __eq__ call in a Compare with True.
    assert!(matches!(
        &mir.items[5],
        MirItem::TopLevelStmt(MirStmt::Assign {
            value: MirExpr::Compare {
                op: CmpOpKind::NotEq,
                left,
                right,
                ..
            },
            ..
        }) if matches!(left.as_ref(), MirExpr::Call { callee, .. } if callee == "Point.__eq__")
            && matches!(right.as_ref(), MirExpr::BoolLiteral(true))
    ));
}

#[test]
fn a_non_eq_neq_comparison_between_instances_with_eq_falls_through_to_mir_compare() {
    // A non-Eq/NotEq comparison (`<`, `<=`, `>`, `>=`) between same-class
    // instances with `__eq__` falls through to the default
    // `MirExpr::Compare` rather than being rewritten to an `__eq__` call.
    // The `matches!(op, Eq | NotEq)` guard at the top of the rewrite
    // block prevents entry for other operators. The type checker would
    // reject `<` between class instances (T0021), but the MIR lowering
    // is tested directly here without going through the type checker.
    let hir = dataclass_point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "q".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "r".to_string(),
            value: HirExpr::Compare {
                op: CmpOpKind::Lt,
                left: Box::new(HirExpr::Name("p".to_string())),
                right: Box::new(HirExpr::Name("q".to_string())),
            },
        }),
    ]);
    let mir = build(&hir);
    // The `Lt` comparison should NOT be rewritten to an `__eq__` call;
    // it should remain a `MirExpr::Compare` with the original operator.
    assert!(matches!(
        &mir.items[5],
        MirItem::TopLevelStmt(MirStmt::Assign {
            value: MirExpr::Compare { op, .. },
            ..
        }) if *op == CmpOpKind::Lt
    ));
}

#[test]
fn an_eq_comparison_between_instances_without_eq_falls_through_to_mir_compare() {
    // When the class has no `__eq__` method in its MRO, the `if let
    // Some(eq_mangled)` block is not entered, and the comparison falls
    // through to the default `MirExpr::Compare`. This covers the `}`
    // (merge point) of the `if let Some(eq_mangled)` block.
    let self_ty = Ty::Instance(Box::new("Plain".to_string()));
    let init = HirItem::Function {
        name: "Plain.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    let hir = HirModule {
        items: vec![
            init,
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "p".to_string(),
                value: HirExpr::Call {
                    callee: "Plain".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "q".to_string(),
                value: HirExpr::Call {
                    callee: "Plain".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "r".to_string(),
                value: HirExpr::Compare {
                    op: CmpOpKind::Eq,
                    left: Box::new(HirExpr::Name("p".to_string())),
                    right: Box::new(HirExpr::Name("q".to_string())),
                },
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "Plain".to_string(),
            HirClassDef {
                name: "Plain".to_string(),
                bases: Vec::new(),
                mro: vec!["Plain".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "Plain.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    let mir = build(&hir);
    // The last item should be a MirExpr::Compare (not a __eq__ call).
    assert!(matches!(
        &mir.items[3],
        MirItem::TopLevelStmt(MirStmt::Assign {
            value: MirExpr::Compare { op, .. },
            ..
        }) if *op == CmpOpKind::Eq
    ));
}

#[test]
fn print_of_an_instance_with_an_unregistered_class_passes_through() {
    // `rewrite_instance_to_repr` returns `expr.clone()` when the
    // instance's class is not in the `classes` map. This test creates
    // a function with a parameter typed as `Ghost` (an unregistered
    // class), then prints it inside the function body. The MIR should
    // pass the instance through without rewriting (the codegen would
    // panic later, but we only test the MIR lowering here).
    let ghost_ty = Ty::Instance(Box::new("Ghost".to_string()));
    let hir = HirModule {
        items: vec![HirItem::Function {
            name: "test".to_string(),
            params: vec![("g".to_string(), ghost_ty.clone())],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("g".to_string())],
            })],
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    // The function body's print argument should be a MirExpr::Name
    // with the Ghost instance type (not rewritten to a __repr__ call).
    // Using nested `matches!` avoids hand-written `other => panic!(...)`
    // arms that would be permanently uncovered under D-014's coverage gate.
    assert!(matches!(
        &mir.items[0],
        MirItem::Function { body, .. }
            if body.len() == 1
                && matches!(
                    &body[0],
                    MirStmt::ExprStmt(MirExpr::Call { args, .. })
                        if args.len() == 1 && args[0].ty() == ghost_ty
                )
    ));
}

#[test]
fn print_of_an_instance_with_repr_lowers_to_repr_call() {
    let hir = dataclass_point_module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "p".to_string(),
            value: HirExpr::Call {
                callee: "Point".to_string(),
                args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
            },
        }),
        HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Name("p".to_string())],
        })),
    ]);
    let mir = build(&hir);
    // Items: [__init__, __eq__, __repr__, Assign p, ExprStmt(print)].
    // The print is at index 4. The argument should be a MirExpr::Call
    // to Point.__repr__.
    assert!(matches!(
        &mir.items[4],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee,
            args,
            ..
        })) if callee == "print"
              && args.len() == 1
              && matches!(
                  &args[0],
                  MirExpr::Call { callee: repr_callee, .. }
                      if repr_callee == "Point.__repr__"
              )
    ));
}

// -- #380: eval_isinstance_protocol in compile-time isinstance -------

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_true() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Circle".to_string(),
        bases: Vec::new(),
        mro: vec!["Circle".to_string()],
        attrs: Vec::new(),
        methods: vec![("draw".to_string(), "Circle.draw".to_string())],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("Drawable".to_string(), proto_def),
        ("Circle".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Circle".to_string()));
    let proto = classes.get("Drawable").unwrap();
    assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_missing_method() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Circle".to_string(),
        bases: Vec::new(),
        mro: vec!["Circle".to_string()],
        attrs: Vec::new(),
        methods: vec![("foo".to_string(), "Circle.foo".to_string())],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("Drawable".to_string(), proto_def),
        ("Circle".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Circle".to_string()));
    let proto = classes.get("Drawable").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_non_instance() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> =
        [("Drawable".to_string(), proto_def)].into_iter().collect();
    // Non-instance type should return false.
    let obj_ty = Ty::Int;
    let proto = classes.get("Drawable").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_evaluates_to_false_for_unknown_class() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> =
        [("Drawable".to_string(), proto_def)].into_iter().collect();
    // Unknown class should return false.
    let obj_ty = Ty::Instance(Box::new("Unknown".to_string()));
    let proto = classes.get("Drawable").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_member() {
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Point".to_string(),
        bases: Vec::new(),
        mro: vec!["Point".to_string()],
        attrs: vec![("x".to_string(), Ty::Int)],
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("Point".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Point".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_found_via_property() {
    // This exercises the `mro_def.properties.iter().any(...)` path
    // (line 2063) in the attribute presence check.
    use pycc_hir::{HirClassDef, PropertyDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Point".to_string(),
        bases: Vec::new(),
        mro: vec!["Point".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: vec![PropertyDef {
            name: "x".to_string(),
            getter: "get_x".to_string(),
            setter: None,
        }],
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("Point".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("Point".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_missing_returns_false() {
    // This exercises the `return false` path (line 2069) when the
    // attribute is not found in the class's MRO.
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "NoX".to_string(),
        bases: Vec::new(),
        mro: vec!["NoX".to_string()],
        attrs: vec![("y".to_string(), Ty::Int)],
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("NoX".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("NoX".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_attribute_missing_in_mro_returns_false() {
    // This exercises the `false` path (line 2065) when an MRO entry
    // is not found in the `classes` map.
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasX".to_string(),
        bases: Vec::new(),
        mro: vec!["HasX".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Attribute {
            name: "x".to_string(),
            ty: Ty::Int,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    // The class's MRO includes "Ghost" which is NOT in the classes
    // map, exercising the `else { false }` branch.
    let class_def = HirClassDef {
        name: "WithGhost".to_string(),
        bases: Vec::new(),
        mro: vec!["WithGhost".to_string(), "Ghost".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasX".to_string(), proto_def),
        ("WithGhost".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("WithGhost".to_string()));
    let proto = classes.get("HasX").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn isinstance_with_runtime_checkable_protocol_method_missing_in_mro_returns_false() {
    // Exercises the `?`-None path in the method arm of
    // `eval_isinstance_protocol` when an MRO entry is not found in
    // the `classes` map. The attribute arm's equivalent path is
    // covered by `isinstance_with_runtime_checkable_protocol_attribute_missing_in_mro_returns_false`.
    use pycc_hir::{HirClassDef, ProtocolMember};
    let proto_def = HirClassDef {
        name: "HasDraw".to_string(),
        bases: Vec::new(),
        mro: vec!["HasDraw".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    // The class's MRO includes "Ghost" which is NOT in the classes
    // map, exercising the `?`-None path in the method arm's
    // `find_map` closure.
    let class_def = HirClassDef {
        name: "WithGhostMethod".to_string(),
        bases: Vec::new(),
        mro: vec!["WithGhostMethod".to_string(), "Ghost".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let classes: HashMap<String, HirClassDef> = [
        ("HasDraw".to_string(), proto_def),
        ("WithGhostMethod".to_string(), class_def),
    ]
    .into_iter()
    .collect();
    let obj_ty = Ty::Instance(Box::new("WithGhostMethod".to_string()));
    let proto = classes.get("HasDraw").unwrap();
    assert!(!eval_isinstance_protocol(&obj_ty, proto, &classes));
}

#[test]
fn method_call_on_protocol_typed_parameter_resolves_via_class_def_of() {
    // This exercises the `Ty::Protocol(name) => name.as_str()` arm
    // in `class_def_of` (line 1855) by constructing a HIR module
    // with a protocol-typed parameter and a method call on it.
    let proto_ty = Ty::Protocol(Box::new("P".to_string()));
    let hir = HirModule {
        items: vec![
            HirItem::Function {
                name: "P.foo".to_string(),
                params: vec![("self".to_string(), proto_ty.clone())],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
            },
            HirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), proto_ty)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::MethodCall {
                    base: Box::new(HirExpr::Name("x".to_string())),
                    method: "foo".to_string(),
                    args: vec![],
                }))],
            },
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "P".to_string(),
            HirClassDef {
                name: "P".to_string(),
                bases: Vec::new(),
                mro: vec!["P".to_string()],
                attrs: Vec::new(),
                methods: vec![("foo".to_string(), "P.foo".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods: Vec::new(),
                class_methods: Vec::new(),
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: true,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    };
    // Just verify it doesn't panic — the MIR build succeeds when
    // `class_def_of` resolves the protocol-typed expression.
    let _ = build(&hir);
}

#[test]
fn protocol_typed_annassign_binds_concrete_type() {
    // Covers the `else` branch (line 759) and the protocol-annotation
    // `bind_ty` path (line 769) in `lower_stmt`'s `AnnAssign` arm.
    // When the annotation is `Ty::Protocol("P")` and the value is
    // `Ty::Instance("C")`, the types don't match (so the `else`
    // branch is taken), and `bind_ty` uses the concrete type.
    let proto_ty = Ty::Protocol(Box::new("P".to_string()));
    let instance_ty = Ty::Instance(Box::new("C".to_string()));
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
            target: "c".to_string(),
            annotation: proto_ty,
            value: Some(HirExpr::Call {
                callee: "C".to_string(),
                args: vec![],
            }),
            is_final: false,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            (
                "P".to_string(),
                HirClassDef {
                    name: "P".to_string(),
                    bases: Vec::new(),
                    mro: vec!["P".to_string()],
                    attrs: Vec::new(),
                    methods: Vec::new(),
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: true,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
            (
                "C".to_string(),
                HirClassDef {
                    name: "C".to_string(),
                    bases: Vec::new(),
                    mro: vec!["C".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: Vec::new(),
                    is_abstract: false,
                },
            ),
        ],
    };
    let mir = build(&hir);
    // The MIR should bind `c` with the concrete `Instance("C")` type,
    // not the protocol type.
    assert!(mir.items.iter().any(|item| {
        matches!(item, MirItem::TopLevelStmt(MirStmt::Assign { target, value })
                if target == "c" && value.ty() == instance_ty)
    }));
}

#[test]
fn isinstance_with_protocol_target_goes_through_lower_isinstance() {
    // Covers line 1999: the `eval_isinstance_protocol` call inside
    // `lower_isinstance` when the target class is a protocol.  The
    // existing `isinstance_with_runtime_checkable_protocol_*` tests
    // call `eval_isinstance_protocol` directly; this test goes
    // through the full `build` → `lower_expr` → `lower_isinstance`
    // path.
    use pycc_hir::ProtocolMember;
    let proto_def = HirClassDef {
        name: "Drawable".to_string(),
        bases: Vec::new(),
        mro: vec!["Drawable".to_string()],
        attrs: Vec::new(),
        methods: Vec::new(),
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: true,
        runtime_checkable: true,
        protocol_members: vec![ProtocolMember::Method {
            name: "draw".to_string(),
            param_tys: vec![],
            return_ty: Ty::None,
        }],
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let class_def = HirClassDef {
        name: "Circle".to_string(),
        bases: Vec::new(),
        mro: vec!["Circle".to_string()],
        attrs: Vec::new(),
        methods: vec![
            ("__init__".to_string(), "Circle.__init__".to_string()),
            ("draw".to_string(), "Circle.draw".to_string()),
        ],
        type_param: None,
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "c".to_string(),
                value: HirExpr::Call {
                    callee: "Circle".to_string(),
                    args: vec![],
                },
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "isinstance".to_string(),
                args: vec![
                    HirExpr::Name("c".to_string()),
                    HirExpr::Name("Drawable".to_string()),
                ],
            })),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![
            ("Drawable".to_string(), proto_def),
            ("Circle".to_string(), class_def),
        ],
    };
    let mir = build(&hir);
    // The isinstance call should be lowered to a BoolLiteral(true)
    // because Circle conforms to the Drawable protocol.
    assert!(mir.items.iter().any(|item| matches!(
        item,
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::BoolLiteral(true)))
    )));
}

// -- #381: match statement MIR lowering coverage -----------------------

fn match_module(cases: Vec<HirMatchCase>) -> HirModule {
    HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::Match {
                subject: HirExpr::Name("x".to_string()),
                cases,
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

fn match_module_list(cases: Vec<HirMatchCase>) -> HirModule {
    HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            }),
            HirItem::TopLevelStmt(HirStmt::Match {
                subject: HirExpr::Name("x".to_string()),
                cases,
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

fn match_module_dict(cases: Vec<HirMatchCase>) -> HirModule {
    HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("k".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            }),
            HirItem::TopLevelStmt(HirStmt::Match {
                subject: HirExpr::Name("x".to_string()),
                cases,
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

#[test]
fn lowers_match_with_literal_pattern_to_mir() {
    let hir = match_module(vec![
        HirMatchCase {
            pattern: HirPattern::Literal(HirExpr::IntLiteral(1)),
            guard: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(1)],
            })],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(0)],
            })],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_singleton_pattern_to_mir() {
    let hir = match_module(vec![
        HirMatchCase {
            pattern: HirPattern::Singleton(true),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Singleton(false),
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_none_singleton_pattern_to_mir() {
    let hir = match_module(vec![HirMatchCase {
        pattern: HirPattern::NoneSingleton,
        guard: None,
        body: vec![],
    }]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_capture_pattern_to_mir() {
    let hir = match_module(vec![HirMatchCase {
        pattern: HirPattern::Capture("y".to_string()),
        guard: None,
        body: vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::Name("y".to_string())],
        })],
    }]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_sequence_pattern_to_mir() {
    let hir = match_module_list(vec![
        HirMatchCase {
            pattern: HirPattern::Sequence(vec![
                HirPattern::Capture("a".to_string()),
                HirPattern::Capture("b".to_string()),
            ]),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_sequence_star_pattern_to_mir() {
    let hir = match_module_list(vec![
        HirMatchCase {
            pattern: HirPattern::SequenceStar(
                vec![HirPattern::Capture("a".to_string())],
                Some("rest".to_string()),
            ),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_mapping_pattern_to_mir() {
    let hir = match_module_dict(vec![
        HirMatchCase {
            pattern: HirPattern::Mapping(
                vec![(
                    HirExpr::StringLiteral("k".to_string()),
                    HirPattern::Capture("v".to_string()),
                )],
                None,
            ),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_mapping_rest_pattern_to_mir() {
    let hir = match_module_dict(vec![
        HirMatchCase {
            pattern: HirPattern::Mapping(
                vec![(
                    HirExpr::StringLiteral("k".to_string()),
                    HirPattern::Capture("v".to_string()),
                )],
                Some("rest".to_string()),
            ),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_class_pattern_to_mir() {
    let class_def = HirClassDef {
        name: "P".to_string(),
        bases: Vec::new(),
        mro: vec!["P".to_string()],
        attrs: vec![("a".to_string(), Ty::Int)],
        methods: vec![("__init__".to_string(), "P.__init__".to_string())],
        properties: Vec::new(),
        static_methods: Vec::new(),
        class_methods: Vec::new(),
        type_param: None,
        enum_members: Vec::new(),
        is_dataclass: false,
        dataclass_fields: Vec::new(),
        is_protocol: false,
        runtime_checkable: false,
        protocol_members: Vec::new(),
        abstract_methods: Vec::new(),
        is_abstract: false,
    };
    let hir = HirModule {
        items: vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }),
            HirItem::TopLevelStmt(HirStmt::Match {
                subject: HirExpr::Name("x".to_string()),
                cases: vec![
                    HirMatchCase {
                        pattern: HirPattern::Class {
                            class_name: "P".to_string(),
                            positional: vec![HirPattern::Capture("a".to_string())],
                            keyword: vec![("a".to_string(), HirPattern::Capture("a2".to_string()))],
                        },
                        guard: None,
                        body: vec![],
                    },
                    HirMatchCase {
                        pattern: HirPattern::Wildcard,
                        guard: None,
                        body: vec![],
                    },
                ],
            }),
        ],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![("P".to_string(), class_def)],
    };
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_or_pattern_to_mir() {
    let hir = match_module(vec![
        HirMatchCase {
            pattern: HirPattern::Or(vec![
                HirPattern::Literal(HirExpr::IntLiteral(1)),
                HirPattern::Literal(HirExpr::IntLiteral(2)),
                HirPattern::Literal(HirExpr::IntLiteral(3)),
            ]),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_as_pattern_to_mir() {
    let hir = match_module(vec![
        HirMatchCase {
            pattern: HirPattern::As(
                Box::new(HirPattern::Literal(HirExpr::IntLiteral(1))),
                "y".to_string(),
            ),
            guard: None,
            body: vec![],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_guard_to_mir() {
    let hir = match_module(vec![
        HirMatchCase {
            pattern: HirPattern::Capture("y".to_string()),
            guard: Some(HirExpr::Compare {
                op: CmpOpKind::Gt,
                left: Box::new(HirExpr::Name("y".to_string())),
                right: Box::new(HirExpr::IntLiteral(3)),
            }),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("y".to_string())],
            })],
        },
        HirMatchCase {
            pattern: HirPattern::Wildcard,
            guard: None,
            body: vec![],
        },
    ]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_empty_cases_to_mir() {
    let hir = match_module(vec![]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn lowers_match_with_wildcard_only_to_mir() {
    let hir = match_module(vec![HirMatchCase {
        pattern: HirPattern::Wildcard,
        guard: None,
        body: vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::IntLiteral(0)],
        })],
    }]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

#[test]
fn nest_match_alternatives_with_empty_alternatives_returns_seq() {
    let inner_body = vec![MirStmt::ExprStmt(MirExpr::IntLiteral(0))];
    let else_chain = MirStmt::ExprStmt(MirExpr::IntLiteral(1));
    let result = nest_match_alternatives(&[], inner_body, else_chain);
    assert!(matches!(result, MirStmt::Seq(body) if body.len() == 1));
}

#[test]
fn lowers_match_with_or_pattern_with_bindings_to_mir() {
    let hir = match_module(vec![HirMatchCase {
        pattern: HirPattern::Or(vec![
            HirPattern::Capture("a".to_string()),
            HirPattern::Capture("b".to_string()),
        ]),
        guard: None,
        body: vec![],
    }]);
    let mir = build(&hir);
    assert!(!mir.items.is_empty());
}

// #382 (PR-22 Part 1): MIR lowering tests for exception handling.

fn try_module(
    body: Vec<HirStmt>,
    handlers: Vec<pycc_hir::HirExceptHandler>,
    orelse: Vec<HirStmt>,
    finalbody: Vec<HirStmt>,
) -> HirModule {
    HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

/// Test helper: extract a `Try` statement from a top-level MIR item,
/// panicking if the item is not a `Try`.  The panic arm is covered by
/// `expect_top_level_try_panics_on_non_try`.
fn expect_top_level_try(
    item: &MirItem,
) -> (&[MirStmt], &[MirExceptHandler], &[MirStmt], &[MirStmt]) {
    match item {
        MirItem::TopLevelStmt(MirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        }) => (body, handlers, orelse, finalbody),
        _ => panic!("expected Try"),
    }
}

/// Test helper: extract a `Raise` statement from a top-level MIR item,
/// panicking if the item is not a `Raise`.  The panic arm is covered by
/// `expect_top_level_raise_panics_on_non_raise`.
fn expect_top_level_raise(item: &MirItem) -> &MirExceptionValue {
    match item {
        MirItem::TopLevelStmt(MirStmt::Raise { exception }) => exception,
        _ => panic!("expected Raise"),
    }
}

/// Test helper: extract a `RaiseFrom` statement from a top-level MIR item,
/// panicking if the item is not a `RaiseFrom`.  The panic arm is covered
/// by `expect_top_level_raise_from_panics_on_non_raise_from`.
fn expect_top_level_raise_from(item: &MirItem) -> (&MirExceptionValue, &MirExceptionValue) {
    match item {
        MirItem::TopLevelStmt(MirStmt::RaiseFrom { exception, cause }) => (exception, cause),
        _ => panic!("expected RaiseFrom"),
    }
}

fn expect_constructed_exception(value: &MirExceptionValue) -> (&u8, &MirExpr) {
    match value {
        MirExceptionValue::Constructed { type_tag, message } => (type_tag, message),
        MirExceptionValue::Existing(_) => panic!("expected constructed exception"),
    }
}

/// Test helper: assert a top-level MIR item is a `Reraise`, panicking
/// otherwise.  The panic arm is covered by
/// `expect_top_level_reraise_panics_on_non_reraise`.
fn expect_top_level_reraise(item: &MirItem) {
    match item {
        MirItem::TopLevelStmt(MirStmt::Reraise) => {}
        _ => panic!("expected Reraise"),
    }
}

#[test]
#[should_panic(expected = "expected Try")]
fn expect_top_level_try_panics_on_non_try() {
    expect_top_level_try(&MirItem::TopLevelStmt(MirStmt::NoOp));
}

#[test]
#[should_panic(expected = "expected Raise")]
fn expect_top_level_raise_panics_on_non_raise() {
    expect_top_level_raise(&MirItem::TopLevelStmt(MirStmt::NoOp));
}

#[test]
#[should_panic(expected = "expected RaiseFrom")]
fn expect_top_level_raise_from_panics_on_non_raise_from() {
    expect_top_level_raise_from(&MirItem::TopLevelStmt(MirStmt::NoOp));
}

#[test]
#[should_panic(expected = "expected constructed exception")]
fn expect_constructed_exception_panics_on_an_existing_exception() {
    expect_constructed_exception(&MirExceptionValue::Existing(MirExpr::IntLiteral(0)));
}

#[test]
#[should_panic(expected = "expected Reraise")]
fn expect_top_level_reraise_panics_on_non_reraise() {
    expect_top_level_reraise(&MirItem::TopLevelStmt(MirStmt::NoOp));
}

/// Test helper: assert a `MirExpr` is a `StringLiteral`, returning
/// the inner string.  Panicking otherwise.  The panic arm is covered
/// by `expect_string_literal_panics_on_non_string`.
fn expect_string_literal(expr: &MirExpr) -> &str {
    match expr {
        MirExpr::StringLiteral(s) => s,
        _ => panic!("expected StringLiteral"),
    }
}

#[test]
#[should_panic(expected = "expected StringLiteral")]
fn expect_string_literal_panics_on_non_string() {
    expect_string_literal(&MirExpr::IntLiteral(0));
}

#[test]
fn lowers_try_with_value_error_handler_to_mir() {
    let hir = try_module(
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::StringLiteral("hello".to_string())],
        })],
        vec![pycc_hir::HirExceptHandler {
            exc_type: Some("ValueError".to_string()),
            name: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("caught".to_string())],
            })],
        }],
        Vec::new(),
        Vec::new(),
    );
    let mir = build(&hir);
    assert_eq!(mir.items.len(), 1);
    let (body, handlers, orelse, finalbody) = expect_top_level_try(&mir.items[0]);
    assert_eq!(body.len(), 1);
    assert_eq!(handlers.len(), 1);
    assert_eq!(handlers[0].exc_type_tag, Some(1)); // ValueError = 1
    assert!(orelse.is_empty());
    assert!(finalbody.is_empty());
}

#[test]
fn lowers_try_with_bare_except_to_mir() {
    let hir = try_module(
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::StringLiteral("body".to_string())],
        })],
        vec![pycc_hir::HirExceptHandler {
            exc_type: None,
            name: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("caught".to_string())],
            })],
        }],
        Vec::new(),
        Vec::new(),
    );
    let mir = build(&hir);
    let (_, handlers, _, _) = expect_top_level_try(&mir.items[0]);
    assert_eq!(handlers[0].exc_type_tag, None); // bare except
}

#[test]
fn lowers_try_with_else_and_finally_to_mir() {
    let hir = try_module(
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::StringLiteral("body".to_string())],
        })],
        vec![pycc_hir::HirExceptHandler {
            exc_type: Some("Exception".to_string()),
            name: None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::StringLiteral("handler".to_string())],
            })],
        }],
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::StringLiteral("else".to_string())],
        })],
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "print".to_string(),
            args: vec![HirExpr::StringLiteral("finally".to_string())],
        })],
    );
    let mir = build(&hir);
    let (body, handlers, orelse, finalbody) = expect_top_level_try(&mir.items[0]);
    assert_eq!(body.len(), 1);
    assert_eq!(handlers.len(), 1);
    assert_eq!(handlers[0].exc_type_tag, Some(0)); // Exception = 0
    assert_eq!(orelse.len(), 1);
    assert_eq!(finalbody.len(), 1);
}

#[test]
fn lowers_raise_value_error_to_mir() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
            exc: Some(HirExpr::Call {
                callee: "ValueError".to_string(),
                args: vec![HirExpr::StringLiteral("bad value".to_string())],
            }),
            cause: None,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    let (exc_type_tag, message) =
        expect_constructed_exception(expect_top_level_raise(&mir.items[0]));
    assert_eq!(*exc_type_tag, 1); // ValueError = 1
    expect_string_literal(message);
}

#[test]
fn lowers_raise_from_to_mir() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
            exc: Some(HirExpr::Call {
                callee: "ValueError".to_string(),
                args: vec![HirExpr::StringLiteral("bad".to_string())],
            }),
            cause: Some(HirExpr::Call {
                callee: "TypeError".to_string(),
                args: vec![HirExpr::StringLiteral("cause".to_string())],
            }),
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    let (exception, cause) = expect_top_level_raise_from(&mir.items[0]);
    let (exc_type_tag, _) = expect_constructed_exception(exception);
    let (cause_type_tag, _) = expect_constructed_exception(cause);
    assert_eq!(*exc_type_tag, 1); // ValueError = 1
    assert_eq!(*cause_type_tag, 2); // TypeError = 2
}

#[test]
fn lowers_bare_reraise_to_mir() {
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
            exc: None,
            cause: None,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    expect_top_level_reraise(&mir.items[0]);
}

#[test]
fn resolve_exception_tag_maps_all_builtin_types() {
    assert_eq!(resolve_exception_tag("Exception"), Some(0));
    assert_eq!(resolve_exception_tag("ValueError"), Some(1));
    assert_eq!(resolve_exception_tag("TypeError"), Some(2));
    assert_eq!(resolve_exception_tag("KeyError"), Some(3));
    assert_eq!(resolve_exception_tag("IndexError"), Some(4));
    assert_eq!(resolve_exception_tag("ZeroDivisionError"), Some(5));
    assert_eq!(resolve_exception_tag("RuntimeError"), Some(6));
    assert_eq!(resolve_exception_tag("UnknownError"), None);
}

#[test]
fn lowers_raise_with_no_args_uses_fallback_message() {
    // `raise ValueError()` — no args, should use "unknown" fallback.
    let hir = HirModule {
        items: vec![HirItem::TopLevelStmt(HirStmt::Raise {
            exc: Some(HirExpr::Call {
                callee: "ValueError".to_string(),
                args: vec![],
            }),
            cause: None,
        })],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    };
    let mir = build(&hir);
    let (_, message) = expect_constructed_exception(expect_top_level_raise(&mir.items[0]));
    assert_eq!(expect_string_literal(message), "unknown");
}

#[test]
fn lowers_existing_exception_value_without_replacing_it() {
    let mut scopes = vec![HashMap::from([(
        "some_exc".to_string(),
        Ty::Instance(Box::new("ValueError".to_string())),
    )])];
    let value = lower_exception_value(
        &HirExpr::Name("some_exc".to_string()),
        &mut scopes,
        &HashMap::new(),
        None,
    );
    assert!(matches!(
        value,
        MirExceptionValue::Existing(MirExpr::Name {
            name,
            ty: Ty::Instance(class_name),
        }) if name == "some_exc" && class_name.as_str() == "ValueError"
    ));
}

#[test]
#[should_panic(expected = "pycc_types rejects unknown exception handler types before MIR")]
fn an_unknown_exception_handler_cannot_silently_become_a_bare_handler() {
    let hir = try_module(
        vec![HirStmt::ExprStmt(HirExpr::IntLiteral(0))],
        vec![pycc_hir::HirExceptHandler {
            exc_type: Some("TypoError".to_string()),
            name: None,
            body: vec![HirStmt::ExprStmt(HirExpr::IntLiteral(0))],
        }],
        Vec::new(),
        Vec::new(),
    );
    let _ = build(&hir);
}

// -- PEP 560 (#610): value-position `C[x]` lowering ---------------------

/// Builds a module with class `C` whose `__class_getitem__` is spelled
/// as `hook_kind` ("static" or "classmethod"), plus `extra_items`.
fn class_getitem_hir(hook_kind: &str, extra_items: Vec<HirItem>) -> HirModule {
    let self_ty = Ty::Instance(Box::new("C".to_string()));
    let init = HirItem::Function {
        name: "C.__init__".to_string(),
        params: vec![("self".to_string(), self_ty.clone())],
        return_ty: Ty::None,
        body: vec![HirStmt::Return(None)],
    };
    let is_static = hook_kind == "static";
    let hook_symbol = if is_static {
        "C.__class_getitem__.static".to_string()
    } else {
        "C.__class_getitem__.classmethod".to_string()
    };
    let mut params = Vec::new();
    if !is_static {
        params.push(("cls".to_string(), self_ty.clone()));
    }
    params.push(("key".to_string(), Ty::Int));
    let hook = HirItem::Function {
        name: hook_symbol.clone(),
        params,
        return_ty: Ty::Int,
        body: vec![HirStmt::Return(Some(HirExpr::Name("key".to_string())))],
    };
    let mut items = vec![init, hook];
    items.extend(extra_items);
    let (static_methods, class_methods) = if is_static {
        (
            vec![("__class_getitem__".to_string(), hook_symbol)],
            Vec::new(),
        )
    } else {
        (
            Vec::new(),
            vec![("__class_getitem__".to_string(), hook_symbol)],
        )
    };
    HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: vec![(
            "C".to_string(),
            pycc_hir::HirClassDef {
                name: "C".to_string(),
                bases: Vec::new(),
                mro: vec!["C".to_string()],
                attrs: Vec::new(),
                methods: vec![("__init__".to_string(), "C.__init__".to_string())],
                type_param: None,
                properties: Vec::new(),
                static_methods,
                class_methods,
                enum_members: Vec::new(),
                is_dataclass: false,
                dataclass_fields: Vec::new(),
                is_protocol: false,
                runtime_checkable: false,
                protocol_members: Vec::new(),
                abstract_methods: Vec::new(),
                is_abstract: false,
            },
        )],
    }
}

#[test]
fn class_getitem_on_a_bare_class_name_lowers_to_the_static_hook() {
    let hir = class_getitem_hir(
        "static",
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::Subscript {
                base: Box::new(HirExpr::Name("C".to_string())),
                index: Box::new(HirExpr::IntLiteral(3)),
            },
        ))],
    );
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "C.__class_getitem__.static".to_string(),
            args: vec![MirExpr::IntLiteral(3)],
            ty: Ty::Int,
        })))
    );
}

#[test]
fn class_getitem_on_a_bare_class_name_lowers_to_the_classmethod_hook() {
    // The `@classmethod` spelling needs the `NullInstance` receiver the
    // `MethodCall` arm prepends; delegating there is what supplies it.
    let hir = class_getitem_hir(
        "classmethod",
        vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::Subscript {
                base: Box::new(HirExpr::Name("C".to_string())),
                index: Box::new(HirExpr::IntLiteral(3)),
            },
        ))],
    );
    let mir = build(&hir);
    assert_eq!(
        mir.items.last(),
        Some(&MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "C.__class_getitem__.classmethod".to_string(),
            args: vec![
                MirExpr::NullInstance {
                    ty: Ty::Instance(Box::new("C".to_string())),
                },
                MirExpr::IntLiteral(3),
            ],
            ty: Ty::Int,
        })))
    );
}

#[test]
fn a_value_shadowing_a_class_name_subscripts_as_a_value() {
    // `pycc_types` applies the identical guard, so both crates must agree
    // that a name bound as a value indexes that value rather than
    // dispatching the class hook.
    let hir = class_getitem_hir(
        "static",
        vec![
            HirItem::TopLevelStmt(HirStmt::Assign {
                target: "C".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            }),
            HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Subscript {
                base: Box::new(HirExpr::Name("C".to_string())),
                index: Box::new(HirExpr::IntLiteral(0)),
            })),
        ],
    );
    let mir = build(&hir);
    // Both outcomes of the pattern are exercised so the `matches!`
    // fallback arm is a covered region under D-014: the trailing item is
    // the plain subscript, and the leading item (the class's own
    // `__init__`) is not.
    let is_value_subscript = |item: &MirItem| {
        matches!(
            item,
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Subscript { .. }))
        )
    };
    assert!(is_value_subscript(mir.items.last().unwrap()));
    assert!(!is_value_subscript(&mir.items[0]));
}

/// Lowers `-p` / `+p` over a single parameter of type `ty` and asserts the
/// whole lowered function equals `expected` in its return position, so each
/// unary case below states only the arithmetic rewrite it expects.
fn assert_unary_over_param_lowers_to(op: UnaryOpKind, ty: Ty, expected: MirExpr) {
    let hir = HirModule {
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
