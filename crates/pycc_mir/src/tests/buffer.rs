//! Buffer element loads (`MirExpr::BufferGet`, Part 2 of #1027) and buffer
//! length reads (`MirExpr::BufferLen`, #1116).
//!
//! `b[i]` on a `memoryview`-typed name is the one expression #1027 admits
//! over a buffer. Lowering routes it away from `MirExpr::Subscript` -- whose
//! own `ty()` panics on a base that is neither list nor tuple -- into a node
//! of its own whose type is unconditionally `Ty::Float`, matching the
//! `f64`-element view the `--ext` wrapper fills.

use crate::*;
use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt, Ty};

/// A module holding one function whose first parameter `b` is a
/// `memoryview`, which is the only way such a binding can exist: #1027
/// admits `Ty::MemoryView` solely as a parameter of a `pycc build --ext`
/// export, and adds no expression that produces one.
fn module_with_buffer_fn(return_ty: Ty, body: Vec<HirStmt>) -> HirModule {
    HirModule {
        seeded_builtin_exception_classes: false,
        items: vec![HirItem::Function {
            name: "total".to_string(),
            params: vec![
                ("b".to_string(), Ty::MemoryView),
                ("i".to_string(), Ty::Int),
            ],
            return_ty,
            body,
        }],
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
    }
}

/// The lowered statements of the single function `module_with_buffer_fn`
/// builds.
fn function_body(module: &MirModule) -> &[MirStmt] {
    let MirItem::Function { body, .. } = &module.items[0] else {
        panic!("expected the module's only item to be a function");
    };
    body
}

#[test]
fn a_subscript_of_a_memoryview_lowers_to_a_buffer_get_typed_float() {
    let hir = module_with_buffer_fn(
        Ty::Float,
        vec![HirStmt::Return(Some(HirExpr::Subscript {
            base: Box::new(HirExpr::Name("b".to_string())),
            index: Box::new(HirExpr::Name("i".to_string())),
        }))],
    );
    let mir = build(&hir);
    let [MirStmt::Return(Some(expr))] = function_body(&mir) else {
        panic!("expected a single `return`");
    };
    // The node carries no `ty` field: every buffer #1027 admits is an
    // `f64` view, so `ty()` answering `Ty::Float` unconditionally is the
    // contract rather than an inference result.
    assert_eq!(expr.ty(), Ty::Float);
    let MirExpr::BufferGet { base, index } = expr else {
        panic!("expected a `BufferGet`, got {expr:?}");
    };
    assert!(
        matches!(**base, MirExpr::Name { ref name, ty: Ty::MemoryView } if name == "b"),
        "{base:?}"
    );
    assert!(
        matches!(**index, MirExpr::Name { ref name, ty: Ty::Int } if name == "i"),
        "{index:?}"
    );
}

#[test]
fn a_walrus_in_a_buffer_index_binds_for_the_next_statement() {
    // PEP 572 (#774), the same requirement `ObjSubscript` carries:
    // `MirExpr::collect_named_expr_bindings` has to recurse into *both*
    // sides of the new node. `stmt.rs`'s `ExprStmt` arm binds whatever
    // that walk finds, so an empty arm would lower the following
    // `n` against an unbound name and panic.
    let hir = module_with_buffer_fn(
        Ty::None,
        vec![
            HirStmt::ExprStmt(HirExpr::Subscript {
                base: Box::new(HirExpr::Name("b".to_string())),
                index: Box::new(HirExpr::NamedExpr {
                    name: "n".to_string(),
                    value: Box::new(HirExpr::IntLiteral(0)),
                }),
            }),
            HirStmt::ExprStmt(HirExpr::Name("n".to_string())),
        ],
    );
    let mir = build(&hir);
    let [MirStmt::ExprStmt(first), MirStmt::ExprStmt(second)] = function_body(&mir) else {
        panic!("expected two expression statements");
    };
    assert!(matches!(first, MirExpr::BufferGet { .. }), "{first:?}");
    assert!(
        matches!(second, MirExpr::Name { name, ty: Ty::Int } if name == "n"),
        "{second:?}"
    );
}

/// `len(b)` routes away from the scalar `len` lowering -- which stays a
/// `MirExpr::Call` whose codegen reaches `expect_list_pointer` -- into a node
/// of its own whose `ty()` is unconditionally `Ty::Int`.
#[test]
fn a_len_of_a_memoryview_lowers_to_a_buffer_len_typed_int() {
    let hir = module_with_buffer_fn(
        Ty::Int,
        vec![HirStmt::Return(Some(HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("b".to_string())],
        }))],
    );
    let mir = build(&hir);
    let [MirStmt::Return(Some(expr))] = function_body(&mir) else {
        panic!("expected a single `return`");
    };
    // No `ty` field here either: a buffer's element count is an `int`
    // unconditionally, exactly as `ObjLen`'s is.
    assert_eq!(expr.ty(), Ty::Int);
    let MirExpr::BufferLen { base } = expr else {
        panic!("expected a `BufferLen`, got {expr:?}");
    };
    assert!(
        matches!(base.as_ref(), MirExpr::Name { name, ty: Ty::MemoryView } if name == "b"),
        "{base:?}"
    );
}

/// `len` on any other operand is untouched by the new interception: a
/// `list` argument still lowers to the ordinary `MirExpr::Call`, whose
/// codegen re-tags `pycc_rt_int_list_len`'s raw count.
#[test]
fn a_len_of_a_list_still_lowers_to_the_scalar_call() {
    let hir = module_with_buffer_fn(
        Ty::Int,
        vec![
            HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            },
            HirStmt::Return(Some(HirExpr::Call {
                callee: "len".to_string(),
                args: vec![HirExpr::Name("xs".to_string())],
            })),
        ],
    );
    let mir = build(&hir);
    let [_, MirStmt::Return(Some(expr))] = function_body(&mir) else {
        panic!("expected an assignment and a `return`");
    };
    assert!(
        matches!(expr, MirExpr::Call { callee, .. } if callee == "len"),
        "{expr:?}"
    );
}

/// The walrus requirement again, for this node's one child: `len((n := b))`
/// is not expressible -- a walrus cannot rebind a `memoryview` -- so the
/// shape that reaches `BufferLen`'s `collect_named_expr_bindings` arm is a
/// length read whose base carries one. This asserts the arm exists at all by
/// driving the node through the same `ExprStmt` seam the index test uses.
#[test]
fn a_buffer_length_read_walks_its_base_for_walrus_bindings() {
    let hir = module_with_buffer_fn(
        Ty::None,
        vec![HirStmt::ExprStmt(HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("b".to_string())],
        })],
    );
    let mir = build(&hir);
    let [MirStmt::ExprStmt(only)] = function_body(&mir) else {
        panic!("expected one expression statement");
    };
    assert!(matches!(only, MirExpr::BufferLen { .. }), "{only:?}");
}
