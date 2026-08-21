//! Variant coverage for `MirExpr::ty`.
//!
//! This test exercises the crate root's `impl MirExpr { fn ty }` rather than
//! `expr::lower_expr`, so it lives in its own module instead of `expr.rs`.

use crate::*;
use pycc_hir::{BinOpKind, CmpOpKind, Ty};

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
