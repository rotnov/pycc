//! Exception lowering (`exception::lower_raise` and handler lowering).
//!
//! Covers `try`/`except`/`else`/`finally`, bare and typed handlers, `raise`,
//! `raise ... from ...`, bare re-raise, `resolve_exception_tag` over every
//! builtin type, and the test helpers' own panic arms.

use crate::*;
use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt, Ty};

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
