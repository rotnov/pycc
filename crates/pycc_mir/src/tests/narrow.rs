//! Issue #769 (Part 2 of #747): flow-sensitive `Optional[T]` narrowing at
//! the MIR layer -- `stmt::lower_stmt`'s `HirStmt::If` arm pushing/popping
//! the `$narrowed:{name}` scope sentinel (`push_narrowing`/`kill_narrowing`
//! in the crate root), and `expr::lower_expr`'s `HirExpr::Name` arm
//! consulting it (`narrowed_ty`) to emit `MirExpr::OptionalUnwrap`.
//!
//! Mirrors `pycc_types::narrow`'s own test suite one layer down: this crate
//! trusts that `pycc_types::check` has already proven every narrowing this
//! module exercises sound, so these tests only prove the *lowering*, not
//! the checker's own eligibility rules (compound tests, non-`Optional`
//! names, etc. -- those never reach `pycc_mir::build` at all, since
//! `pycc_types::check` runs first and would already have rejected an
//! unsound program).

use crate::*;
use pycc_hir::{CmpOpKind, HirExpr, HirItem, HirModule, HirStmt, Ty};

/// `x: int | None = 5` at module scope, ahead of every test below --
/// shared setup for "`x` is already known `Optional[int]` before the `if`".
fn optional_int_decl(target: &str) -> HirStmt {
    HirStmt::AnnAssign {
        is_final: false,
        target: target.to_string(),
        annotation: Ty::Optional(Box::new(Ty::Int)),
        value: Some(HirExpr::IntLiteral(5)),
    }
}

fn is_not_none(name: &str) -> HirExpr {
    HirExpr::Compare {
        op: CmpOpKind::IsNot,
        left: Box::new(HirExpr::Name(name.to_string())),
        right: Box::new(HirExpr::NoneLiteral),
    }
}

fn is_none(name: &str) -> HirExpr {
    HirExpr::Compare {
        op: CmpOpKind::Is,
        left: Box::new(HirExpr::Name(name.to_string())),
        right: Box::new(HirExpr::NoneLiteral),
    }
}

fn print_x(name: &str) -> HirStmt {
    HirStmt::ExprStmt(HirExpr::Call {
        callee: "print".to_string(),
        args: vec![HirExpr::Name(name.to_string())],
    })
}

fn module(items: Vec<HirItem>) -> HirModule {
    HirModule {
        seeded_builtin_exception_classes: false,
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

/// `x: int | None = 5; if x is not None: print(x)` -- the body's read of
/// `x` is narrowed, so it must lower to `MirExpr::OptionalUnwrap` wrapping
/// the still-`Optional`-typed `MirExpr::Name`, not a bare `Name { ty:
/// Optional(Int) }`.
#[test]
fn is_not_none_narrows_the_body_read_to_an_optional_unwrap() {
    let hir = module(vec![
        HirItem::TopLevelStmt(optional_int_decl("x")),
        HirItem::TopLevelStmt(HirStmt::If {
            test: is_not_none("x"),
            body: vec![print_x("x")],
            orelse: vec![],
        }),
    ]);
    let mir = build(&hir);
    let MirItem::TopLevelStmt(MirStmt::If { body, orelse, .. }) = &mir.items[1] else {
        panic!("expected the second item to be the lowered `if`");
    };
    assert!(orelse.is_empty());
    assert_eq!(
        body,
        &vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::OptionalUnwrap(
                Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Optional(Box::new(Ty::Int)),
                }),
                Box::new(Ty::Int),
            )],
            ty: Ty::None,
        })]
    );
}

/// `x: int | None = 5; if x is None: pass else: print(x)` -- the mirror
/// polarity: only `orelse`'s read is narrowed.
#[test]
fn is_none_narrows_the_orelse_read_to_an_optional_unwrap() {
    let hir = module(vec![
        HirItem::TopLevelStmt(optional_int_decl("x")),
        HirItem::TopLevelStmt(HirStmt::If {
            test: is_none("x"),
            body: vec![],
            orelse: vec![print_x("x")],
        }),
    ]);
    let mir = build(&hir);
    let MirItem::TopLevelStmt(MirStmt::If { body, orelse, .. }) = &mir.items[1] else {
        panic!("expected the second item to be the lowered `if`");
    };
    assert!(body.is_empty());
    assert_eq!(
        orelse,
        &vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::OptionalUnwrap(
                Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Optional(Box::new(Ty::Int)),
                }),
                Box::new(Ty::Int),
            )],
            ty: Ty::None,
        })]
    );
}

/// The non-narrowed branch of an `is not None` test keeps reading `x` as
/// plain `Optional(Int)` -- no `OptionalUnwrap` on the `orelse` side.
#[test]
fn is_not_none_does_not_narrow_the_orelse_read() {
    let hir = module(vec![
        HirItem::TopLevelStmt(optional_int_decl("x")),
        HirItem::TopLevelStmt(HirStmt::If {
            test: is_not_none("x"),
            body: vec![],
            orelse: vec![print_x("x")],
        }),
    ]);
    let mir = build(&hir);
    let MirItem::TopLevelStmt(MirStmt::If { orelse, .. }) = &mir.items[1] else {
        panic!("expected the second item to be the lowered `if`");
    };
    assert_eq!(
        orelse,
        &vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Optional(Box::new(Ty::Int)),
            }],
            ty: Ty::None,
        })]
    );
}

/// Narrowing never leaks past the `if`: a read of `x` after the statement
/// (a sibling in the same top-level sequence) must still see plain
/// `Optional(Int)`, proving the `$narrowed:{name}` sentinel this arm pushes
/// is popped again once the narrowed branch finishes lowering.
#[test]
fn narrowing_does_not_leak_past_the_if() {
    let hir = module(vec![
        HirItem::TopLevelStmt(optional_int_decl("x")),
        HirItem::TopLevelStmt(HirStmt::If {
            test: is_not_none("x"),
            body: vec![print_x("x")],
            orelse: vec![],
        }),
        HirItem::TopLevelStmt(print_x("x")),
    ]);
    let mir = build(&hir);
    assert_eq!(
        mir.items[2],
        MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Optional(Box::new(Ty::Int)),
            }],
            ty: Ty::None,
        }))
    );
}

/// A reassignment inside the narrowed body kills the narrowing for every
/// read after it, within the same branch: `if x is not None: x = 5;
/// print(x)` -- the `print(x)` here must read plain `Optional(Int)`, not
/// `OptionalUnwrap`, mirroring `pycc_types::narrow`'s own kill-on-assign
/// rule (`crate::kill_narrowing`, called from `lower_stmt`'s `Assign` arm).
#[test]
fn a_reassignment_inside_the_narrowed_body_kills_the_narrowing() {
    let hir = module(vec![
        HirItem::TopLevelStmt(optional_int_decl("x")),
        HirItem::TopLevelStmt(HirStmt::If {
            test: is_not_none("x"),
            body: vec![
                HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                },
                print_x("x"),
            ],
            orelse: vec![],
        }),
    ]);
    let mir = build(&hir);
    let MirItem::TopLevelStmt(MirStmt::If { body, .. }) = &mir.items[1] else {
        panic!("expected the second item to be the lowered `if`");
    };
    assert_eq!(
        body[1],
        MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Optional(Box::new(Ty::Int)),
            }],
            ty: Ty::None,
        })
    );
}

/// A test naming a variable that is not currently `Optional`-scoped is
/// never treated as narrowing-eligible -- `if flag is not None:` where
/// `flag` is a plain `bool` never reaches `pycc_mir::build` in practice
/// (`pycc_types::check`'s own T0021 rejects an `is`/`is not None` operand
/// that is not `Optional` before HIR lowering ever gets here), but this
/// lowering pass's own `narrows_body`/`narrows_orelse` computation must
/// still fail closed (`None`) rather than panicking, matching
/// `pycc_types::narrow::narrowing_target`'s identical fail-closed
/// `_ => None` arm for the same shape.
#[test]
fn a_plain_bool_test_reads_normally_with_no_unwrap() {
    let hir = module(vec![
        HirItem::TopLevelStmt(HirStmt::Assign {
            target: "flag".to_string(),
            value: HirExpr::BoolLiteral(true),
        }),
        HirItem::TopLevelStmt(HirStmt::If {
            test: HirExpr::Name("flag".to_string()),
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::Name("flag".to_string())],
            })],
            orelse: vec![],
        }),
    ]);
    let mir = build(&hir);
    let MirItem::TopLevelStmt(MirStmt::If { body, .. }) = &mir.items[1] else {
        panic!("expected the second item to be the lowered `if`");
    };
    assert_eq!(
        body,
        &vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "flag".to_string(),
                ty: Ty::Bool,
            }],
            ty: Ty::None,
        })]
    );
}

/// The early-return continuation shape (issue #769's second narrowing
/// mechanism, alongside in-branch narrowing above): `def f(x: int | None):
/// if x is None: return; print(x)` -- the read of `x` in the statement
/// *after* the `if` (a sibling in the same function-body sequence, not a
/// branch of the `if` itself) must still be narrowed, since reaching that
/// point is only possible via the implicit "`x` is not `None`" path. Proves
/// `lower_stmt_sequence`/`apply_post_if_narrowing` (wired into
/// `lower_item`'s `HirItem::Function` arm) actually fires, mirroring
/// `pycc_types::narrow::apply_post_if_narrowing`'s identical checker-layer
/// behavior -- this is the exact shape `tests/fixtures/pep_0604_union.py`'s
/// `describe` function exercises end-to-end.
#[test]
fn an_early_return_guard_narrows_the_read_after_it_in_a_function_body() {
    let hir = module(vec![HirItem::Function {
        name: "describe".to_string(),
        params: vec![("x".to_string(), Ty::Optional(Box::new(Ty::Int)))],
        return_ty: Ty::None,
        body: vec![
            HirStmt::If {
                test: is_none("x"),
                body: vec![HirStmt::Return(None)],
                orelse: vec![],
            },
            print_x("x"),
        ],
    }]);
    let mir = build(&hir);
    let MirItem::Function { body, .. } = &mir.items[0] else {
        panic!("expected the only item to be the lowered function");
    };
    assert_eq!(
        body[1],
        MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::OptionalUnwrap(
                Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Optional(Box::new(Ty::Int)),
                }),
                Box::new(Ty::Int),
            )],
            ty: Ty::None,
        })
    );
}

/// A narrowing fact established by an early-return guard *nested inside* an
/// unrelated `if`'s body must not leak past that `if`'s own close: `def
/// f(cond, x): if cond: (if x is None: return); print(x)` (after the outer
/// `if`) -- the outer `if`'s own test says nothing about `x`, so `x` must
/// still read as plain `Optional(Int)` once the outer `if` finishes, even
/// though the read *inside* the outer `if`'s body (right after the nested
/// early-return guard) is correctly narrowed. This is the isolation fix:
/// without `lower_scoped_body`'s snapshot/restore around every *nested*
/// body (as opposed to `lower_stmt_sequence`'s own top-level, non-isolated
/// use), the nested `apply_post_if_narrowing` hit would otherwise leak its
/// `$narrowed:x` sentinel into `scopes`' single function-level frame and
/// incorrectly narrow the read after the outer `if` too.
#[test]
fn a_nested_early_return_narrowing_does_not_leak_past_the_enclosing_if() {
    let hir = module(vec![HirItem::Function {
        name: "f".to_string(),
        params: vec![
            ("cond".to_string(), Ty::Bool),
            ("x".to_string(), Ty::Optional(Box::new(Ty::Int))),
        ],
        return_ty: Ty::None,
        body: vec![
            HirStmt::If {
                test: HirExpr::Name("cond".to_string()),
                body: vec![
                    HirStmt::If {
                        test: is_none("x"),
                        body: vec![HirStmt::Return(None)],
                        orelse: vec![],
                    },
                    print_x("x"),
                ],
                orelse: vec![],
            },
            print_x("x"),
        ],
    }]);
    let mir = build(&hir);
    let MirItem::Function { body, .. } = &mir.items[0] else {
        panic!("expected the only item to be the lowered function");
    };
    let MirStmt::If {
        body: outer_body, ..
    } = &body[0]
    else {
        panic!("expected the first function statement to be the lowered outer `if`");
    };
    // Inside the outer `if`'s own body, right after the nested guard: still
    // narrowed (proves the nested guard's own narrowing did take effect for
    // its own enclosing sequence).
    assert_eq!(
        outer_body[1],
        MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::OptionalUnwrap(
                Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Optional(Box::new(Ty::Int)),
                }),
                Box::new(Ty::Int),
            )],
            ty: Ty::None,
        })
    );
    // After the outer `if` entirely: back to plain `Optional(Int)`, not
    // leaked.
    assert_eq!(
        body[1],
        MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Optional(Box::new(Ty::Int)),
            }],
            ty: Ty::None,
        })
    );
}

/// `.ty()` of an `OptionalUnwrap` node reports the inner type, not
/// `Optional(inner)` -- the read-side mirror of `OptionalWrap`'s own
/// `.ty()` test.
#[test]
fn optional_unwrap_ty_reports_the_inner_type() {
    let expr = MirExpr::OptionalUnwrap(
        Box::new(MirExpr::Name {
            name: "x".to_string(),
            ty: Ty::Optional(Box::new(Ty::Int)),
        }),
        Box::new(Ty::Int),
    );
    assert_eq!(expr.ty(), Ty::Int);
}
