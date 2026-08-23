//! Exception lowering (`exception::lower_raise` and handler lowering).
//!
//! Covers `try`/`except`/`else`/`finally`, bare and typed handlers, `raise`,
//! `raise ... from ...`, bare re-raise, `resolve_exception_tag` over every
//! builtin type, and the test helpers' own panic arms.

use crate::exception::{handler_type_tags, lower_exception_value, resolve_exception_tag};
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
        seeded_builtin_exception_classes: false,
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
        MirExceptionValue::Constructed {
            type_tag, message, ..
        } => (type_tag, message),
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
    assert_eq!(handlers[0].exc_type_tag, Some(vec![1])); // ValueError = 1
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
    assert_eq!(handlers[0].exc_type_tag, Some(vec![0])); // Exception = 0
    assert_eq!(orelse.len(), 1);
    assert_eq!(finalbody.len(), 1);
}

#[test]
fn lowers_raise_value_error_to_mir() {
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
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
        seeded_builtin_exception_classes: false,
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
        seeded_builtin_exception_classes: false,
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

/// Part 2 of #543 (#739), D-194: `resolve_exception_tag`'s hand-written
/// `match` is documented as implementing exactly the name set
/// `pycc_hir::is_flat_builtin_exception_class` recognizes, "kept in sync by
/// construction, not by a shared constant" (see this function's own doc
/// comment). That comment is a promise with nothing enforcing it: the two
/// are independent artifacts in different crates. This test turns the
/// promise into an assertion, over the full `BUILTIN_EXCEPTION_CLASSES`
/// array rather than a hardcoded name list, so a future edit that widens or
/// reorders the flat-name set in one place without the other fails loudly
/// here instead of silently reopening the `.expect()` panic risk D-194's
/// shadow-gate fix was written to eliminate (see
/// `pycc_types::exception::is_unshadowed_builtin_exception`, which trusts
/// `is_flat_builtin_exception_class` to mean exactly "resolvable by
/// `resolve_exception_tag`").
#[test]
fn resolve_exception_tag_agrees_with_is_flat_builtin_exception_class() {
    for (index, name) in pycc_hir::BUILTIN_EXCEPTION_CLASSES.iter().enumerate() {
        let flat = pycc_hir::is_flat_builtin_exception_class(name);
        let resolved = resolve_exception_tag(name);
        if flat {
            assert_eq!(
                resolved,
                Some(index as u8),
                "`{name}` is flat but resolve_exception_tag disagrees on its tag"
            );
        } else {
            assert_eq!(
                resolved, None,
                "`{name}` is not flat but resolve_exception_tag still resolves it by name"
            );
        }
    }
}

#[test]
fn lowers_raise_with_no_args_uses_fallback_message() {
    // `raise ValueError()` — no args, should use "unknown" fallback.
    let hir = HirModule {
        seeded_builtin_exception_classes: false,
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

// Part 2 of #541 (D-189): user-defined exception classes.

/// A minimal raisable user exception class: `class <name>(<mro[1]>): pass`,
/// carrying the tag HIR lowering would have assigned it.
fn user_exception_class(name: &str, mro: &[&str], tag: Option<u8>) -> (String, HirClassDef) {
    (
        name.to_string(),
        HirClassDef {
            exception_type_tag: tag,
            name: name.to_string(),
            bases: mro
                .get(1)
                .map(|base| vec![base.to_string()])
                .unwrap_or_default(),
            mro: mro.iter().map(|entry| entry.to_string()).collect(),
            attrs: Vec::new(),
            methods: vec![(
                "__init__".to_string(),
                pycc_hir::EXCEPTION_INIT_MANGLED_NAME.to_string(),
            )],
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
        },
    )
}

fn exception_hierarchy() -> HashMap<String, HirClassDef> {
    HashMap::from([
        user_exception_class("AppError", &["AppError", "Exception"], Some(7)),
        user_exception_class(
            "DatabaseError",
            &["DatabaseError", "AppError", "Exception"],
            Some(9),
        ),
        user_exception_class(
            "ConfigError",
            &["ConfigError", "AppError", "Exception"],
            Some(8),
        ),
        // Rooted at a builtin other than `Exception`, so a `ValueError`
        // handler has to widen to it (#702).
        user_exception_class(
            "ParseError",
            &["ParseError", "ValueError", "Exception"],
            Some(10),
        ),
        user_exception_class("Unrelated", &["Unrelated"], None),
    ])
}

#[test]
fn a_user_exception_handler_accepts_its_own_tag_and_every_subclass_tag_sorted() {
    let classes = exception_hierarchy();
    // Sorted ascending even though `ConfigError` (8) was declared after
    // `DatabaseError` (9) and `classes` iterates in hash-map order.
    assert_eq!(handler_type_tags("AppError", &classes), vec![7, 8, 9]);
}

#[test]
fn a_leaf_user_exception_handler_accepts_only_its_own_tag() {
    let classes = exception_hierarchy();
    assert_eq!(handler_type_tags("DatabaseError", &classes), vec![9]);
}

#[test]
fn a_builtin_handler_also_accepts_its_user_defined_subclasses() {
    let classes = exception_hierarchy();
    // `ParseError` derives from `ValueError`, not from `Exception`, so the
    // handler widens from `ValueError`'s own tag 1 to include it.
    assert_eq!(handler_type_tags("ValueError", &classes), vec![1, 10]);
}

#[test]
fn a_builtin_handler_without_user_subclasses_stays_a_single_tag() {
    let classes = exception_hierarchy();
    // No class in the fixture reaches `TypeError`.
    assert_eq!(handler_type_tags("TypeError", &classes), vec![2]);
}

#[test]
fn an_exception_handler_stays_a_single_catch_all_tag() {
    // Tag 0 is `pycc_rt_exception_type_matches`'s own catch-all, so listing
    // every user tag alongside it would emit dead comparisons.
    let classes = exception_hierarchy();
    assert_eq!(handler_type_tags("Exception", &classes), vec![0]);
}

#[test]
fn raising_a_user_exception_class_lowers_to_a_constructed_value_with_its_name() {
    let classes = exception_hierarchy();
    let mut scopes = vec![HashMap::new()];
    let value = lower_exception_value(
        &HirExpr::Call {
            callee: "DatabaseError".to_string(),
            args: vec![HirExpr::StringLiteral("boom".to_string())],
        },
        &mut scopes,
        &classes,
        None,
    );
    assert!(matches!(
        value,
        MirExceptionValue::Constructed {
            type_tag: 9,
            ref class_name,
            message: MirExpr::StringLiteral(ref message),
        } if class_name == "DatabaseError" && message == "boom"
    ));
}

#[test]
fn calling_an_untagged_class_is_not_an_exception_construction() {
    let classes = exception_hierarchy();
    let mut scopes = vec![HashMap::new()];
    let value = lower_exception_value(
        &HirExpr::Call {
            callee: "Unrelated".to_string(),
            args: Vec::new(),
        },
        &mut scopes,
        &classes,
        None,
    );
    assert!(matches!(value, MirExceptionValue::Existing(_)));
}
