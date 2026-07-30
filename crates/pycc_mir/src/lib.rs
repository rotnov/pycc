use pycc_hir::{FStringPart, HirExpr, HirItem, HirModule, HirStmt};
use std::collections::HashMap;

// Re-exported (not just `use`d) because `pycc_codegen` doesn't depend on
// `pycc_hir` directly (see its Cargo.toml) -- `Ty`, `BinOpKind`, and
// `CmpOpKind` all reach this crate's public API through `MirExpr`'s fields
// (`Name`/`Call` carry a `Ty`; `BinOp` carries a `BinOpKind`; `Compare`
// carries a `CmpOpKind`), so each must be nameable as
// `pycc_mir::{Ty, BinOpKind, CmpOpKind}` from any downstream crate, exactly
// like `pycc_types` already re-exports `Ty` (`pycc_types::Ty`, its own line
// 4) for the same reason.
pub use pycc_hir::{BinOpKind, CmpOpKind, Ty};

#[derive(Debug, Clone, PartialEq)]
pub enum MirExpr {
    IntLiteral(i64),
    FloatLiteral(f64),
    BoolLiteral(bool),
    StringLiteral(String),
    Name {
        name: String,
        ty: Ty,
    },
    Call {
        callee: String,
        args: Vec<MirExpr>,
        ty: Ty,
    },
    BinOp {
        op: BinOpKind,
        left: Box<MirExpr>,
        right: Box<MirExpr>,
        ty: Ty,
    },
    Compare {
        op: CmpOpKind,
        left: Box<MirExpr>,
        right: Box<MirExpr>,
        ty: Ty,
    },
    FString(Vec<MirFStringPart>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirFStringPart {
    Literal(String),
    Interpolation(Box<MirExpr>),
}

impl MirExpr {
    pub fn ty(&self) -> Ty {
        match self {
            MirExpr::IntLiteral(_) => Ty::Int,
            MirExpr::FloatLiteral(_) => Ty::Float,
            MirExpr::BoolLiteral(_) => Ty::Bool,
            MirExpr::StringLiteral(_) | MirExpr::FString(_) => Ty::Str,
            MirExpr::Name { ty, .. }
            | MirExpr::Call { ty, .. }
            | MirExpr::BinOp { ty, .. }
            | MirExpr::Compare { ty, .. } => ty.clone(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum MirStmt {
    ExprStmt(MirExpr),
    Assign {
        target: String,
        value: MirExpr,
    },
    /// A statement with zero runtime effect -- currently only produced by a
    /// value-less PEP 526 annotation (`x: int`), which CPython itself does
    /// nothing observable for either (confirmed empirically during PR-9
    /// planning: no store, no allocation, nothing an oracle diff could see).
    NoOp,
    If {
        test: MirExpr,
        body: Vec<MirStmt>,
        orelse: Vec<MirStmt>,
    },
    While {
        test: MirExpr,
        body: Vec<MirStmt>,
    },
    ForRange {
        var: String,
        start: MirExpr,
        stop: MirExpr,
        step: MirExpr,
        body: Vec<MirStmt>,
    },
    Return(Option<MirExpr>),
}

#[derive(Debug, PartialEq)]
pub enum MirItem {
    Function {
        name: String,
        params: Vec<(String, Ty)>,
        return_ty: Ty,
        body: Vec<MirStmt>,
    },
    TopLevelStmt(MirStmt),
}

pub struct MirModule {
    pub items: Vec<MirItem>,
}

pub fn build(hir: &HirModule) -> MirModule {
    let mut scopes: Vec<HashMap<String, Ty>> = vec![HashMap::new()];
    // First pass: register every function's mangled `$fn:name` signature
    // before lowering any item body -- mirrors `pycc_types::check`'s own
    // two-pass fix (D-038/D-039) so a forward reference, a sibling call, or
    // a recursive self-call all resolve to the right return type regardless
    // of where the callee's `def` appears in the module.
    for item in &hir.items {
        if let HirItem::Function {
            name, return_ty, ..
        } = item
        {
            bind(&mut scopes, format!("$fn:{name}"), return_ty.clone());
        }
    }
    // Lower module statements first, in source order, so the module scope is
    // complete before any function body is lowered. This mirrors
    // `pycc_types::check_with_signatures`'s D-041 three-pass contract:
    // top-level forward reads stay invalid because these statements are still
    // visited sequentially, while a function may read a global assigned after
    // its `def` because function bodies are evaluated only when called.
    let mut lowered: Vec<Option<MirItem>> = hir.items.iter().map(|_| None).collect();
    for (index, item) in hir.items.iter().enumerate() {
        if matches!(item, HirItem::TopLevelStmt(_)) {
            lowered[index] = Some(lower_item(item, &mut scopes));
        }
    }
    for (index, item) in hir.items.iter().enumerate() {
        if matches!(item, HirItem::Function { .. }) {
            lowered[index] = Some(lower_item(item, &mut scopes));
        }
    }
    let items = lowered
        .into_iter()
        .map(|item| item.expect("every HIR item is either a function or a top-level statement"))
        .collect();
    MirModule { items }
}

fn lower_item(item: &HirItem, scopes: &mut Vec<HashMap<String, Ty>>) -> MirItem {
    match item {
        HirItem::Function {
            name,
            params,
            return_ty,
            body,
        } => {
            scopes.push(params.iter().cloned().collect());
            let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
            scopes.pop();
            MirItem::Function {
                name: name.clone(),
                params: params.clone(),
                return_ty: return_ty.clone(),
                body,
            }
        }
        HirItem::TopLevelStmt(stmt) => MirItem::TopLevelStmt(lower_stmt(stmt, scopes)),
    }
}

fn bind(scopes: &mut [HashMap<String, Ty>], name: String, ty: Ty) {
    scopes
        .last_mut()
        .expect("at least one scope is always present")
        .insert(name, ty);
}

fn bind_variable(scopes: &mut [HashMap<String, Ty>], name: String, ty: Ty) {
    scopes
        .last_mut()
        .expect("at least one scope is always present")
        .entry(name)
        .or_insert(ty);
}

fn lookup(scopes: &[HashMap<String, Ty>], name: &str) -> Ty {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
        .unwrap_or_else(|| panic!("pycc_mir: internal error: `{name}` has no recorded type -- pycc_types::check should have rejected this HIR before it reached pycc_mir"))
}

fn lower_stmt(stmt: &HirStmt, scopes: &mut Vec<HashMap<String, Ty>>) -> MirStmt {
    match stmt {
        HirStmt::ExprStmt(expr) => MirStmt::ExprStmt(lower_expr(expr, scopes)),
        HirStmt::Assign { target, value } => {
            let value = lower_expr(value, scopes);
            // The first assignment fixes a binding's representation.
            // In particular, assigning `bool` to an existing `int` is
            // accepted by the type checker but must not silently change the
            // later MIR name type from tagged i64 to i8.
            bind_variable(scopes, target.clone(), value.ty());
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value: Some(value),
        } => {
            let value = lower_expr(value, scopes);
            // `pycc_types::is_assignable` accepts an annotated initializer
            // in exactly two shapes: an exact type match, or a `bool`
            // initializer under an `int` annotation (`bool` is an `int`
            // subtype -- the only widening `is_assignable` allows). Unlike
            // plain `Assign` (whose bound type and lowered value type are
            // always the same, since both come from `value`), `pycc_types`
            // itself binds its checker `env` to the *annotation's* type for
            // `AnnAssign` (`check_assignment(env, target, *annotation)`,
            // not the initializer's inferred type) specifically so a later
            // annotated re-declaration is checked consistently -- see its
            // own comment citing this exact invariant. D-074's "first
            // assignment fixes a binding's representation" rule then
            // requires this lowering to agree, or a later plain
            // reassignment (`x: int = True; x = 5`) would silently widen
            // into a slot still permanently sized for `bool` (confirmed
            // empirically before this fix: the program above printed `11`,
            // the raw tagged-int bit pattern truncated through an `i8`
            // slot, instead of `5`). Widening the lowered value through the
            // existing `BinOp`/`Add`/`0` path reuses codegen's
            // already-tested `bool -> tagged int` promotion with no new MIR
            // node or codegen arm; it is a no-op rebuild when the types
            // already match.
            let value = if value.ty() == *annotation {
                value
            } else {
                MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(value),
                    right: Box::new(MirExpr::IntLiteral(0)),
                    ty: annotation.clone(),
                }
            };
            bind_variable(scopes, target.clone(), annotation.clone());
            MirStmt::Assign {
                target: target.clone(),
                value,
            }
        }
        HirStmt::AnnAssign { value: None, .. } => MirStmt::NoOp,
        HirStmt::If { test, body, orelse } => MirStmt::If {
            test: lower_expr(test, scopes),
            body: body.iter().map(|s| lower_stmt(s, scopes)).collect(),
            orelse: orelse.iter().map(|s| lower_stmt(s, scopes)).collect(),
        },
        HirStmt::While { test, body } => MirStmt::While {
            test: lower_expr(test, scopes),
            body: body.iter().map(|s| lower_stmt(s, scopes)).collect(),
        },
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            let start = lower_expr(start, scopes);
            let stop = lower_expr(stop, scopes);
            let step = lower_expr(step, scopes);
            bind_variable(scopes, var.clone(), Ty::Int);
            let body = body.iter().map(|s| lower_stmt(s, scopes)).collect();
            MirStmt::ForRange {
                var: var.clone(),
                start,
                stop,
                step,
                body,
            }
        }
        HirStmt::Return(value) => MirStmt::Return(value.as_ref().map(|v| lower_expr(v, scopes))),
    }
}

fn lower_expr(expr: &HirExpr, scopes: &[HashMap<String, Ty>]) -> MirExpr {
    match expr {
        HirExpr::IntLiteral(n) => MirExpr::IntLiteral(*n),
        HirExpr::FloatLiteral(f) => MirExpr::FloatLiteral(*f),
        HirExpr::BoolLiteral(b) => MirExpr::BoolLiteral(*b),
        HirExpr::StringLiteral(s) => MirExpr::StringLiteral(s.clone()),
        HirExpr::Name(name) => MirExpr::Name {
            name: name.clone(),
            ty: lookup(scopes, name),
        },
        HirExpr::Call { callee, args } => {
            let args: Vec<MirExpr> = args.iter().map(|a| lower_expr(a, scopes)).collect();
            let ty = if callee == "print" {
                Ty::None
            } else {
                lookup(scopes, &format!("$fn:{callee}"))
            };
            MirExpr::Call {
                callee: callee.clone(),
                args,
                ty,
            }
        }
        HirExpr::BinOp { op, left, right } => {
            let left = lower_expr(left, scopes);
            let right = lower_expr(right, scopes);
            let ty = binop_result_ty(*op, left.ty(), right.ty());
            MirExpr::BinOp {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
                ty,
            }
        }
        HirExpr::Compare { op, left, right } => MirExpr::Compare {
            op: *op,
            left: Box::new(lower_expr(left, scopes)),
            right: Box::new(lower_expr(right, scopes)),
            ty: Ty::Bool,
        },
        HirExpr::FString(parts) => MirExpr::FString(
            parts
                .iter()
                .map(|p| match p {
                    FStringPart::Literal(s) => MirFStringPart::Literal(s.clone()),
                    FStringPart::Interpolation(e) => {
                        MirFStringPart::Interpolation(Box::new(lower_expr(e, scopes)))
                    }
                })
                .collect(),
        ),
    }
}

fn binop_result_ty(op: BinOpKind, left: Ty, right: Ty) -> Ty {
    if left == Ty::Str && right == Ty::Str && op == BinOpKind::Add {
        return Ty::Str;
    }
    // True division always produces `float`, even for two `int`/`bool`
    // operands -- this must match `pycc_types::numeric_result_type`'s own
    // rule (`(Some(_), Some(_)) if op == BinOpKind::Div => Ok(Ty::Float)`)
    // exactly, since `pycc_types` already accepted this program on that
    // promise; a mismatch here would make MIR's `ty` lie about what
    // codegen must produce (self-review correction: an earlier draft of
    // this function returned `Ty::Int` for `int / int`, which is simply
    // wrong -- `5 / 2` is `2.5`, not `2`).
    if op == BinOpKind::Div || left == Ty::Float || right == Ty::Float {
        return Ty::Float;
    }
    Ty::Int
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_hir::{BinOpKind, CmpOpKind, FStringPart, HirExpr, HirItem, HirModule, HirStmt, Ty};

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
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::IntLiteral(1)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
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
        // representation" rule): it wraps the lowered `BoolLiteral` in a
        // `BinOp`/`Add`/`0` node reporting `Ty::Int` (reusing codegen's
        // already-tested `bool -> tagged int` widening, with no new MIR
        // node or codegen arm) and binds `x` to `Ty::Int`, so a later
        // `Name` reference -- and any later plain reassignment -- agrees.
        // Before this fix, `lower_stmt` bound `Ty::Bool` here instead, and
        // the divergence from `pycc_types`' `Ty::Int` silently mis-sized
        // `x`'s eventual codegen slot (confirmed end to end:
        // `x: int = True; x = 5; return x` printed `11`, not `5`).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::AnnAssign {
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::BoolLiteral(true)),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("x".to_string()))),
            ],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::BoolLiteral(true)),
                        right: Box::new(MirExpr::IntLiteral(0)),
                        ty: Ty::Int,
                    },
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
        // identically. This proves the same `BinOp`/`Add`/`0` wrapping
        // triggers for a `Compare`-sourced `bool`, not only the literal
        // case the previous test exercises.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::IntLiteral(2)),
                }),
            })],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::IntLiteral(1)),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Bool,
                    }),
                    right: Box::new(MirExpr::IntLiteral(0)),
                    ty: Ty::Int,
                },
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
                target: "y".to_string(),
                annotation: Ty::Int,
                value: None,
            })],
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
                    target: "y".to_string(),
                    annotation: Ty::Int,
                    value: None,
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name("y".to_string()))),
            ],
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
    }
}
