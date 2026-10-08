//! Part 1 of #1092: the release of CPython object temporaries.
//!
//! Every IR test compiles real MIR as an `ext` object -- LLVM's verifier
//! runs before any assertion here is believed -- and reads the text of the
//! user function `f` (`pyfn_f`) or of the module-exec entry. The release is
//! a call to the shim's `pycc_ext_obj_release`; these tests pin where it is
//! emitted (after each borrowing consumer, and on every failure edge taken
//! while an operand is held) and where it is not (a borrowed name, a
//! consumed bound method, a value that is bound, returned or never
//! produced).

use super::*;
use crate::{
    CompileOptions, EXT_MODULE_EXEC_SYMBOL, EXT_OBJ_ERROR_BRIDGE_SYMBOL,
    compile_to_object_with_observer,
};
use inkwell::values::AnyValue;

use pycc_mir::{
    CmpOpKind, MirExceptHandler, MirItem, MirModule, MirStmt, ObjBuiltinClass, ObjIsInstanceClass,
    ObjKeywordCall, Ty,
};

const RELEASE: &str = "call void @pycc_ext_obj_release(";

fn import_copy() -> MirItem {
    MirItem::ForeignImport {
        local_name: "copy".to_string(),
        module_path: "copy".to_string(),
        from: None,
    }
}

/// `copy`: a borrowed module global.
fn copy_name() -> MirExpr {
    MirExpr::Name {
        name: "copy".to_string(),
        ty: Ty::Object,
    }
}

/// `copy.<attr>`: a produced new reference.
fn attr(attr: &str) -> MirExpr {
    attr_of(copy_name(), attr)
}

fn attr_of(base: MirExpr, attr: &str) -> MirExpr {
    MirExpr::ObjAttrGet {
        base: Box::new(base),
        attr: attr.to_string(),
        ty: Ty::Object,
    }
}

fn boxed(expr: MirExpr) -> Box<MirExpr> {
    Box::new(expr)
}

/// The LLVM text of the named functions after compiling `import copy` plus
/// `items` as an `ext` object.
fn functions_ir(label: &str, items: Vec<MirItem>, names: &[&str]) -> Vec<String> {
    let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
    let mut all = vec![import_copy()];
    all.extend(items);
    let mut irs = vec![String::new(); names.len()];
    let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
        for (slot, name) in irs.iter_mut().zip(names) {
            if let Some(function) = module.get_function(name) {
                *slot = crate::llvm_string_to_owned(function.print_to_string());
            }
        }
    };
    compile_to_object_with_observer(
        &MirModule {
            items: all,
            ..Default::default()
        },
        &dir.join(format!("{label}.o")),
        &CompileOptions {
            ext: true,
            ..CompileOptions::default()
        },
        Some(&mut observer),
    )
    .expect("ext codegen should succeed");
    for (ir, name) in irs.iter().zip(names) {
        assert!(!ir.is_empty(), "no {name} was emitted");
    }
    irs
}

/// `def f() -> <return_ty>: <body>`.
fn function(return_ty: Ty, body: Vec<MirStmt>) -> MirItem {
    MirItem::Function {
        name: "f".to_string(),
        params: Vec::new(),
        return_ty,
        body,
    }
}

/// The IR of `f` running `body` and then `return`.
fn f_ir(label: &str, mut body: Vec<MirStmt>) -> String {
    body.push(MirStmt::Return(None));
    functions_ir(label, vec![function(Ty::None, body)], &["pyfn_f"]).remove(0)
}

/// The IR of `f` discarding `expr`.
fn discard_ir(label: &str, expr: MirExpr) -> String {
    f_ir(label, vec![MirStmt::ExprStmt(expr)])
}

/// The IR of the module-exec entry running `stmts` at module level.
fn entry_ir(label: &str, stmts: Vec<MirStmt>) -> String {
    let items = stmts.into_iter().map(MirItem::TopLevelStmt).collect();
    functions_ir(label, items, &[EXT_MODULE_EXEC_SYMBOL]).remove(0)
}

/// The text of every block whose label is `prefix` plus LLVM's uniquing
/// digits, in emission order, each up to its terminator.
fn blocks<'a>(ir: &'a str, prefix: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = ir;
    let header = format!("\n{prefix}");
    while let Some(at) = rest.find(&header) {
        let after = &rest[at + header.len()..];
        let label_end = after.find(':').unwrap_or(0);
        let suffix = &after[..label_end];
        let body_end = after.find("\n\n").unwrap_or(after.len());
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            found.push(&after[label_end..body_end]);
        }
        rest = &after[label_end..];
    }
    found
}

fn releases(text: &str) -> usize {
    text.matches(RELEASE).count()
}

/// The iterator `iter()` returns, as the IR names it (Part 3 of #1092).
const ITERATOR: &str = "foreign_iter_get";

/// A comprehension's result collection, as the IR names it.
const RESULT: &str = "objcomp_result";

/// The releases of exactly the value the IR names `%{name}`.
fn releases_of(text: &str, name: &str) -> usize {
    text.matches(&format!("{RELEASE}ptr %{name})")).count()
}

#[test]
fn every_shim_producer_is_produced_and_nothing_else_is() {
    let produced = [
        attr("a"),
        MirExpr::ObjMethodCall {
            base: boxed(copy_name()),
            method: "m".to_string(),
            args: Vec::new(),
        },
        MirExpr::ObjCall {
            callee: boxed(copy_name()),
            args: Vec::new(),
        },
        MirExpr::ObjKeywordCall(Box::new(ObjKeywordCall {
            call: MirExpr::ObjCall {
                callee: boxed(copy_name()),
                args: Vec::new(),
            },
            names: Vec::new(),
            values: Vec::new(),
        })),
        MirExpr::ObjSubscript {
            base: boxed(copy_name()),
            index: boxed(MirExpr::IntLiteral(0)),
        },
        MirExpr::ObjType {
            base: boxed(copy_name()),
        },
        MirExpr::ObjSlice {
            base: boxed(copy_name()),
            start: None,
            stop: None,
            step: None,
        },
        MirExpr::ObjList {
            elements: Vec::new(),
        },
        MirExpr::ObjUnpack {
            value: boxed(copy_name()),
            arity: 2,
        },
        MirExpr::ObjCompare {
            op: CmpOpKind::Lt,
            left: boxed(copy_name()),
            right: boxed(copy_name()),
        },
    ];
    for expr in &produced {
        assert!(is_produced(expr), "{expr:?}");
    }
    let borrowed = [
        copy_name(),
        MirExpr::ObjCompare {
            op: CmpOpKind::Is,
            left: boxed(copy_name()),
            right: boxed(MirExpr::NoneLiteral),
        },
        MirExpr::ObjLen {
            base: boxed(copy_name()),
        },
        MirExpr::IntLiteral(1),
    ];
    for expr in &borrowed {
        assert!(!is_produced(expr), "{expr:?}");
    }
}

/// `object_unbox::emit_pack_operand` evaluates the object inside an
/// `ObjectUnbox`, so the hold must classify that object, and a non-object
/// scalar is never held.
#[test]
fn an_unboxed_operand_is_classified_by_the_object_it_unboxes() {
    let context = Context::create();
    let module = context.create_module("object_release_unbox");
    let pointer = context
        .ptr_type(inkwell::AddressSpace::default())
        .const_null();
    let object = Scalar::Object(pointer);
    let unboxed = |inner: MirExpr| MirExpr::ObjectUnbox(Box::new(inner), Box::new(Ty::Float));
    assert!(produced(&context, &module, &unboxed(attr("a")), &object).is_some());
    assert!(produced(&context, &module, &unboxed(copy_name()), &object).is_none());
    let float = Scalar::Float(context.f64_type().const_float(1.0));
    assert!(produced(&context, &module, &attr("a"), &float).is_none());
}

/// `copy.a.b`, discarded in a function: the inner load is released after
/// the outer one and on the outer one's failure edge before the branch; the
/// outer result is released by the discarding statement.
#[test]
fn a_nested_attribute_load_releases_its_base_on_both_paths() {
    let ir = discard_ir("release_nested_attr", attr_of(attr("a"), "b"));
    let fails = blocks(&ir, "foreign_attr_fail");
    assert_eq!(fails.len(), 2, "{ir}");
    assert_eq!(releases(fails[0]), 0, "nothing is held yet: {ir}");
    let outer = fails[1];
    let bridge = outer
        .find(&format!("call i32 @{EXT_OBJ_ERROR_BRIDGE_SYMBOL}()"))
        .unwrap_or_else(|| panic!("{ir}"));
    let release = outer.find(RELEASE).unwrap_or_else(|| panic!("{ir}"));
    let branch = outer
        .rfind("br label %exception_exit")
        .unwrap_or_else(|| panic!("{ir}"));
    assert!(bridge < release && release < branch, "{ir}");
    assert_eq!(releases(outer), 1, "{ir}");
    // The fallthrough: the base after the outer load, then the result.
    assert!(releases(&ir) >= 3, "{ir}");
}

/// At module level outside every `try`, the foreign failure returns the
/// module-exec failure status directly -- and releases what is held first.
#[test]
fn the_module_exec_failure_return_releases_held_operands() {
    let ir = entry_ir(
        "release_module_return",
        vec![MirStmt::ExprStmt(attr_of(attr("a"), "b"))],
    );
    let fails = blocks(&ir, "foreign_attr_fail");
    assert_eq!(fails.len(), 2, "{ir}");
    let outer = fails[1];
    let release = outer.find(RELEASE).unwrap_or_else(|| panic!("{ir}"));
    let ret = outer.find("ret ").unwrap_or_else(|| panic!("{ir}"));
    assert!(release < ret, "release before the failure return: {ir}");
    assert_eq!(releases(fails[0]), 0, "{ir}");
}

/// Inside a module-level `try`, the failure is bridged to the handler, and
/// the held operand is released on the way.
#[test]
fn a_failure_inside_a_module_level_try_releases_before_the_handler() {
    let ir = entry_ir(
        "release_module_try",
        vec![MirStmt::Try {
            body: vec![MirStmt::ExprStmt(attr_of(attr("a"), "b"))],
            handlers: vec![MirExceptHandler {
                exc_type_tag: None,
                binding_name: None,
                binding_ty: None,
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: Ty::None,
                })],
            }],
            orelse: Vec::new(),
            finalbody: Vec::new(),
        }],
    );
    let outer = blocks(&ir, "foreign_attr_fail")[1];
    let release = outer.find(RELEASE).unwrap_or_else(|| panic!("{ir}"));
    let branch = outer.rfind("br label %").unwrap_or_else(|| panic!("{ir}"));
    assert!(release < branch, "{ir}");
    assert!(!outer.contains("ret "), "{ir}");
}

/// `copy.m(copy.a)`: the bound method is held across the argument (so the
/// argument's failure releases it) and then consumed by the call (so the
/// call's failure does not release it); the produced argument is held
/// across the call and released after it.
#[test]
fn a_method_call_holds_its_bound_method_and_its_produced_argument() {
    let ir = discard_ir(
        "release_method_call",
        MirExpr::ObjMethodCall {
            base: boxed(copy_name()),
            method: "m".to_string(),
            args: vec![attr("a")],
        },
    );
    let lookup = blocks(&ir, "foreign_call_lookup_fail");
    assert_eq!(lookup.len(), 1, "{ir}");
    assert_eq!(releases(lookup[0]), 0, "a borrowed receiver: {ir}");
    let arg = blocks(&ir, "foreign_attr_fail");
    assert_eq!(arg.len(), 1, "{ir}");
    assert_eq!(releases(arg[0]), 1, "the bound method: {ir}");
    let call = blocks(&ir, "foreign_call_fail");
    assert_eq!(call.len(), 1, "{ir}");
    assert_eq!(
        releases(call[0]),
        1,
        "only the argument; the call consumed the bound method: {ir}"
    );
}

/// `copy.a.m()`: a produced receiver is released once the lookup is done
/// and on the lookup's own failure edge.
#[test]
fn a_method_call_releases_a_produced_receiver_after_the_lookup() {
    let ir = discard_ir(
        "release_method_receiver",
        MirExpr::ObjMethodCall {
            base: boxed(attr("a")),
            method: "m".to_string(),
            args: Vec::new(),
        },
    );
    let lookup = blocks(&ir, "foreign_call_lookup_fail");
    assert_eq!(releases(lookup[0]), 1, "{ir}");
    let call = blocks(&ir, "foreign_call_fail");
    assert_eq!(releases(call[0]), 0, "the receiver is gone already: {ir}");
}

/// Borrowed operands only: a borrowed name is never released, and the
/// consumed bound method is released only where the call never runs -- the
/// argument read's own `NameError` exit (`global_unbound`). The one other
/// release is the discarded result's.
#[test]
fn borrowed_operands_and_a_consumed_bound_method_are_never_released() {
    let ir = discard_ir(
        "release_borrowed_only",
        MirExpr::ObjMethodCall {
            base: boxed(copy_name()),
            method: "m".to_string(),
            args: vec![copy_name(), MirExpr::IntLiteral(1)],
        },
    );
    assert_eq!(releases(blocks(&ir, "foreign_call_fail")[0]), 0, "{ir}");
    let unbound: usize = blocks(&ir, "global_unbound")
        .iter()
        .map(|block| releases(block))
        .sum();
    assert_eq!(unbound, 1, "the bound method, on the argument's exit: {ir}");
    assert_eq!(releases(&ir), 2, "{ir}");
}

/// A produced value that is bound or returned is not a temporary: Part 1
/// leaves it to the leak-only rule.
#[test]
fn a_bound_or_returned_produced_value_is_not_released() {
    let ir = entry_ir(
        "release_bound",
        vec![MirStmt::Assign {
            target: "x".to_string(),
            value: attr("a"),
        }],
    );
    assert_eq!(releases(&ir), 0, "{ir}");
    let ir = functions_ir(
        "release_returned",
        vec![function(Ty::Object, vec![MirStmt::Return(Some(attr("a")))])],
        &["pyfn_f"],
    )
    .remove(0);
    assert_eq!(releases(&ir), 0, "{ir}");
}

/// A function without an object operation declares no release and keeps
/// its guard shape: the empty pending stack costs nothing.
#[test]
fn a_function_without_object_operations_emits_no_release() {
    let ir = f_ir(
        "release_none",
        vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::IntLiteral(1)],
            ty: Ty::None,
        })],
    );
    assert!(!ir.contains("pycc_ext_obj_release"), "{ir}");
}

/// Every consumer releases its produced operands: each statement below is
/// compiled on its own and emits exactly `expected` releases, summed over
/// the fallthrough and every failure edge taken while an operand is held.
///
/// The counts are pinned exactly because over-release is the dangerous
/// direction. `subscript`'s 8, read off the IR: `copy` unbound, `b`'s load
/// failing and the effect guard after it each release `a` (3); the
/// subscript's own failure edge and its fallthrough each release `b` then
/// `a` (4); the discarded result is released last (1). Each path from entry
/// to a return releases each produced value at most once; the hosted
/// `tests/issue_1092_object_temp_release.rs` measures that balance.
#[test]
fn every_consumer_releases_its_produced_operands() {
    let call = |callee: &str, ty: Ty| MirExpr::Call {
        callee: callee.to_string(),
        args: vec![attr("a")],
        ty,
    };
    let cases: Vec<(&str, MirStmt, usize)> = vec![
        (
            "len",
            MirStmt::ExprStmt(MirExpr::ObjLen {
                base: boxed(attr("a")),
            }),
            2,
        ),
        (
            "type",
            MirStmt::ExprStmt(MirExpr::ObjType {
                base: boxed(attr("a")),
            }),
            3,
        ),
        (
            "subscript",
            MirStmt::ExprStmt(MirExpr::ObjSubscript {
                base: boxed(attr("a")),
                index: boxed(attr("b")),
            }),
            8,
        ),
        (
            "compare",
            MirStmt::ExprStmt(MirExpr::ObjCompare {
                op: CmpOpKind::Lt,
                left: boxed(attr("a")),
                right: boxed(attr("b")),
            }),
            8,
        ),
        (
            "identity",
            MirStmt::ExprStmt(MirExpr::ObjCompare {
                op: CmpOpKind::Is,
                left: boxed(attr("a")),
                right: boxed(MirExpr::NoneLiteral),
            }),
            1,
        ),
        (
            "contains",
            MirStmt::ExprStmt(MirExpr::ObjContains {
                negate: false,
                item: boxed(attr("a")),
                container: boxed(attr("b")),
            }),
            7,
        ),
        (
            "slice",
            MirStmt::ExprStmt(MirExpr::ObjSlice {
                base: boxed(attr("a")),
                start: Some(boxed(attr("b"))),
                stop: None,
                step: Some(boxed(MirExpr::IntLiteral(2))),
            }),
            8,
        ),
        (
            "list",
            MirStmt::ExprStmt(MirExpr::ObjList {
                elements: vec![attr("a"), MirExpr::IntLiteral(1), attr("b")],
            }),
            8,
        ),
        (
            "isinstance_object",
            MirStmt::ExprStmt(MirExpr::ObjIsInstance {
                value: boxed(attr("a")),
                class: ObjIsInstanceClass::Object(boxed(attr("b"))),
            }),
            7,
        ),
        (
            "isinstance_builtin",
            MirStmt::ExprStmt(MirExpr::ObjIsInstance {
                value: boxed(attr("a")),
                class: ObjIsInstanceClass::Builtin(ObjBuiltinClass::Int),
            }),
            2,
        ),
        (
            "unpack",
            MirStmt::ExprStmt(MirExpr::ObjUnpack {
                value: boxed(attr("a")),
                arity: 2,
            }),
            3,
        ),
        (
            "direct_call",
            MirStmt::ExprStmt(MirExpr::ObjCall {
                callee: boxed(attr("a")),
                args: vec![attr("b")],
            }),
            6,
        ),
        (
            "keyword_method_call",
            MirStmt::ExprStmt(MirExpr::ObjKeywordCall(Box::new(ObjKeywordCall {
                call: MirExpr::ObjMethodCall {
                    base: boxed(attr("a")),
                    method: "m".to_string(),
                    args: vec![attr("b")],
                },
                names: vec!["k".to_string()],
                values: vec![attr("c")],
            }))),
            16,
        ),
        (
            "keyword_direct_call",
            MirStmt::ExprStmt(MirExpr::ObjKeywordCall(Box::new(ObjKeywordCall {
                call: MirExpr::ObjCall {
                    callee: boxed(attr("a")),
                    args: Vec::new(),
                },
                names: vec!["k".to_string()],
                values: vec![attr("c")],
            }))),
            6,
        ),
        ("not", MirStmt::ExprStmt(MirExpr::Not(boxed(attr("a")))), 2),
        ("float", MirStmt::ExprStmt(call("float", Ty::Float)), 2),
        ("bool", MirStmt::ExprStmt(call("bool", Ty::Bool)), 2),
        ("int", MirStmt::ExprStmt(call("int", Ty::Int)), 2),
        ("str", MirStmt::ExprStmt(call("str", Ty::Str)), 2),
        (
            "if",
            MirStmt::If {
                test: attr("a"),
                body: vec![MirStmt::Return(None)],
                orelse: Vec::new(),
            },
            2,
        ),
        (
            "while",
            MirStmt::While {
                test: attr("a"),
                body: vec![MirStmt::Return(None)],
            },
            2,
        ),
        (
            "del_attr",
            MirStmt::ObjDelAttr {
                base: attr("a"),
                attr: "x".to_string(),
            },
            2,
        ),
        (
            "del_slice",
            MirStmt::ObjDelSlice {
                base: attr("a"),
                start: Some(attr("b")),
                stop: None,
                step: None,
            },
            7,
        ),
        (
            "set_attr",
            MirStmt::ObjAttrSet {
                base: attr("a"),
                attr: "x".to_string(),
                value: attr("b"),
            },
            5,
        ),
    ];
    let mut wrong = Vec::new();
    for (label, stmt, expected) in cases {
        let ir = f_ir(&format!("release_consumer_{label}"), vec![stmt]);
        let found = releases(&ir);
        if found != expected {
            wrong.push(format!("{label}: expected {expected}, found {found}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The module-level-only `a, b = o.pair` float-tuple unpack releases its
/// produced base.
#[test]
fn a_module_level_float_tuple_unpack_releases_its_base() {
    let ir = entry_ir(
        "release_float_tuple",
        vec![MirStmt::ExprStmt(MirExpr::ObjUnpackFloatTuple {
            base: boxed(attr("pair")),
            arity: 2,
        })],
    );
    assert!(releases(&ir) >= 1, "{ir}");
}

/// The hold is the fallthrough's to retire: a hold that went missing from
/// the stack is an internal error, never a silent double release.
#[test]
#[should_panic(expected = "missing from pending_object_releases")]
fn retiring_a_hold_twice_is_an_internal_error() {
    let context = Context::create();
    let module = context.create_module("object_release_twice");
    let rt = crate::tests::list_scalar_panic_fixture(&context).1;
    let pointer = context
        .ptr_type(inkwell::AddressSpace::default())
        .const_null();
    let held = hold_new_reference(&context, &module, &rt, pointer);
    let twin = Held(held.0);
    held.consumed(&rt);
    twin.consumed(&rt);
}

/// A comprehension over a CPython object holds a produced filter across its
/// truth test and releases a produced element once the packer has taken its
/// own reference (`[x.v for x in copy.a if x.ok]`). Since Part 3 of #1092
/// the iterator and the result are held for the comprehension's extent:
/// every failure edge after `iter()` releases the iterator, every one
/// after the result exists releases the result too, the normal exit
/// releases the iterator, and the discarded result is released by its
/// statement. Each per-trip item stays unreleased (#1499).
#[test]
fn a_comprehension_releases_its_produced_filter_and_element() {
    let var = || MirExpr::Name {
        name: "x".to_string(),
        ty: Ty::Object,
    };
    let comprehension = MirExpr::Comprehension(Box::new(pycc_mir::MirComprehension {
        var: "x".to_string(),
        var_ty: Ty::Object,
        source: pycc_mir::CompSource::Object(attr("a")),
        cond: Some(attr_of(var(), "ok")),
        elt: pycc_mir::MirCompElt::List(attr_of(var(), "v")),
    }));
    let ir = discard_ir("release_comprehension", comprehension);
    let get_iter_fail = blocks(&ir, "foreign_iter_get_fail");
    assert_eq!(get_iter_fail.len(), 1, "{ir}");
    assert_eq!(releases(get_iter_fail[0]), 1, "{ir}");
    assert_eq!(releases_of(get_iter_fail[0], ITERATOR), 0, "{ir}");
    let result_fail = blocks(&ir, "objcomp_result_fail");
    assert_eq!(result_fail.len(), 1, "{ir}");
    assert_eq!(releases(result_fail[0]), 1, "{ir}");
    assert_eq!(releases_of(result_fail[0], ITERATOR), 1, "{ir}");
    // The source's own attribute load precedes `iter()` and holds nothing.
    let mut later = blocks(&ir, "foreign_attr_fail");
    assert_eq!(later.len(), 3, "{ir}");
    assert_eq!(releases(later.remove(0)), 0, "{ir}");
    // The filter's and the element's attribute loads and their effect
    // checks, the filter's truth test, `next()` and the insertion: each
    // releases both.
    let unwinds = blocks(&ir, "effect_exc_unwind");
    assert_eq!(unwinds.len(), 2, "{ir}");
    later.extend(unwinds);
    for label in [
        "foreign_iter_next_fail",
        "foreign_truthy_fail",
        "objcomp_collect_fail",
    ] {
        let found = blocks(&ir, label);
        assert_eq!(found.len(), 1, "{label}\n{ir}");
        later.extend(found);
    }
    for fail in &later {
        assert_eq!(releases_of(fail, ITERATOR), 1, "{fail}\n{ir}");
        assert_eq!(releases_of(fail, RESULT), 1, "{fail}\n{ir}");
    }
    // The truth test's failure releases the filter as well.
    assert_eq!(releases(blocks(&ir, "foreign_truthy_fail")[0]), 3, "{ir}");
    let packed = ir
        .find("@pycc_ext_obj_pack_object(")
        .expect("the element is packed");
    let collected = ir
        .find("@pycc_ext_obj_collect(")
        .expect("the element is collected");
    assert_eq!(releases(&ir[packed..collected]), 1, "{ir}");
    let after = blocks(&ir, "foreign_iter_after");
    assert_eq!(after.len(), 1, "{ir}");
    assert_eq!(releases(after[0]), 1, "{ir}");
    assert_eq!(releases_of(after[0], ITERATOR), 1, "{ir}");
    // The comprehension hands its result to the statement, which releases
    // it once more on the normal path: nothing else is released.
    assert_eq!(releases_of(&ir, ITERATOR), 9, "{ir}");
    assert_eq!(releases_of(&ir, RESULT), 8, "{ir}");
}

/// A comprehension's result bound to a name is the binding's, not a
/// temporary: only the iterator is released at the exit (#1499 owns the
/// binding).
#[test]
fn a_bound_comprehension_result_is_not_released() {
    let comprehension = MirExpr::Comprehension(Box::new(pycc_mir::MirComprehension {
        var: "x".to_string(),
        var_ty: Ty::Object,
        source: pycc_mir::CompSource::Object(copy_name()),
        cond: None,
        elt: pycc_mir::MirCompElt::List(MirExpr::Name {
            name: "x".to_string(),
            ty: Ty::Object,
        }),
    }));
    let ir = f_ir(
        "release_comprehension_bound",
        vec![MirStmt::Assign {
            target: "c".to_string(),
            value: comprehension,
        }],
    );
    let after = blocks(&ir, "foreign_iter_after");
    assert_eq!(after.len(), 1, "{ir}");
    assert_eq!(releases(after[0]), 1, "{ir}");
    assert_eq!(releases_of(after[0], ITERATOR), 1, "{ir}");
}

/// `for x in copy.a:` releases its produced iterable once `iter()` has
/// returned -- on `iter()`'s failure edge and right after it, before the
/// loop header. Its iterator (Part 3 of #1092) is released by the loop's
/// cleanup target, on `next()`'s direct module-exec return, which looks
/// through that target, and at the loop's normal exit.
#[test]
fn a_for_loop_releases_its_produced_iterable_and_its_iterator() {
    let ir = entry_ir(
        "release_for_iterable",
        vec![MirStmt::ForObject {
            var: "x".to_string(),
            iter: attr("a"),
            body: Vec::new(),
        }],
    );
    assert_eq!(releases(&ir), 5, "{ir}");
    let get_iter_fail = blocks(&ir, "foreign_iter_get_fail");
    assert_eq!(get_iter_fail.len(), 1, "{ir}");
    assert_eq!(releases(get_iter_fail[0]), 1, "{ir}");
    let got = ir
        .find("@pycc_ext_obj_get_iter(")
        .expect("the iterable is iterated");
    let header = ir
        .find("@pycc_ext_obj_iter_next(")
        .expect("the loop advances");
    // `iter()`'s failure edge, its success edge, and the loop's cleanup
    // block, which is laid out ahead of the header.
    assert_eq!(releases(&ir[got..header]), 3, "{ir}");
    let cleanup = blocks(&ir, "foreign_iter_cleanup");
    assert_eq!(cleanup.len(), 1, "{ir}");
    assert_eq!(releases_of(cleanup[0], ITERATOR), 1, "{ir}");
    assert!(
        cleanup[0]
            .trim_end()
            .ends_with("br label %top_exception_exit"),
        "{ir}"
    );
    let next_fail = blocks(&ir, "foreign_iter_next_fail");
    assert_eq!(next_fail.len(), 1, "{ir}");
    assert_eq!(releases_of(next_fail[0], ITERATOR), 1, "{ir}");
    assert!(next_fail[0].trim_end().ends_with("ret i64 -1"), "{ir}");
    let after = blocks(&ir, "foreign_iter_after");
    assert_eq!(after.len(), 1, "{ir}");
    assert_eq!(releases_of(after[0], ITERATOR), 1, "{ir}");
}

/// Inside a module-level `try`, a body failure in a foreign `for` unwinds
/// through the loop's cleanup block -- which releases the iterator -- to
/// the handler dispatch, and a nested loop's cleanup chains to the outer
/// loop's. A failure inside the body's own `try` stays inside the loop and
/// releases no iterator.
#[test]
fn a_for_loop_body_failure_unwinds_through_its_cleanup_blocks() {
    let discard_attr = |name: &str| MirStmt::ExprStmt(attr(name));
    let inner_try = MirStmt::Try {
        body: vec![discard_attr("b")],
        handlers: vec![MirExceptHandler {
            exc_type_tag: None,
            binding_name: None,
            binding_ty: None,
            body: Vec::new(),
        }],
        orelse: Vec::new(),
        finalbody: Vec::new(),
    };
    let ir = entry_ir(
        "release_for_cleanup_chain",
        vec![MirStmt::Try {
            body: vec![MirStmt::ForObject {
                var: "x".to_string(),
                iter: copy_name(),
                body: vec![
                    MirStmt::ForObject {
                        var: "y".to_string(),
                        iter: copy_name(),
                        body: vec![discard_attr("c")],
                    },
                    inner_try,
                ],
            }],
            handlers: vec![MirExceptHandler {
                exc_type_tag: None,
                binding_name: None,
                binding_ty: None,
                body: Vec::new(),
            }],
            orelse: Vec::new(),
            finalbody: Vec::new(),
        }],
    );
    // The label a block's final unconditional branch targets.
    let target = |block: &str| -> String {
        let (_, label) = block
            .trim_end()
            .rsplit_once("br label %")
            .expect("the block ends in a branch");
        label.to_string()
    };
    let cleanups = blocks(&ir, "foreign_iter_cleanup");
    assert_eq!(cleanups.len(), 2, "{ir}");
    let (outer, inner) = (cleanups[0], cleanups[1]);
    assert_eq!(releases(outer), 1, "{ir}");
    assert_eq!(releases_of(outer, ITERATOR), 1, "{ir}");
    assert_eq!(releases(inner), 1, "{ir}");
    assert_eq!(releases_of(inner, ITERATOR), 0, "{ir}");
    assert_eq!(target(outer), "try_handler_dispatch", "{ir}");
    assert_eq!(target(inner), "foreign_iter_cleanup", "{ir}");
    let attr_fails = blocks(&ir, "foreign_attr_fail");
    assert_eq!(attr_fails.len(), 2, "{ir}");
    let inner_label = target(attr_fails[0]);
    assert!(
        inner_label.starts_with("foreign_iter_cleanup") && inner_label != "foreign_iter_cleanup",
        "the inner loop's body unwinds through the inner cleanup\n{ir}"
    );
    assert_eq!(releases(attr_fails[1]), 0, "{ir}");
    assert!(
        target(attr_fails[1]).starts_with("try_handler_dispatch"),
        "the body's own `try` catches before any cleanup\n{ir}"
    );
}

/// A comprehension evaluated while an outer operand is held -- here as the
/// argument of a method call, whose bound method is held across its
/// arguments -- releases that outer operand on every one of its own
/// failure edges, beside its own iterator and result:
/// `copy.m([x for x in copy.a])`.
#[test]
fn a_comprehension_failure_releases_an_outer_held_operand() {
    let comprehension = MirExpr::Comprehension(Box::new(pycc_mir::MirComprehension {
        var: "x".to_string(),
        var_ty: Ty::Object,
        source: pycc_mir::CompSource::Object(attr("a")),
        cond: None,
        elt: pycc_mir::MirCompElt::List(MirExpr::Name {
            name: "x".to_string(),
            ty: Ty::Object,
        }),
    }));
    let ir = discard_ir(
        "release_comprehension_outer",
        MirExpr::ObjMethodCall {
            base: boxed(copy_name()),
            method: "m".to_string(),
            args: vec![comprehension],
        },
    );
    for (label, held) in [
        ("objcomp_result_fail", 2),
        ("foreign_iter_next_fail", 3),
        ("objcomp_collect_fail", 3),
    ] {
        let found = blocks(&ir, label);
        assert_eq!(found.len(), 1, "{label}\n{ir}");
        assert_eq!(
            releases(found[0]),
            held,
            "{label} releases the bound method and what the comprehension holds\n{ir}"
        );
    }
    // `iter()`'s failure releases the produced source and the bound method.
    let get_iter_fail = blocks(&ir, "foreign_iter_get_fail");
    assert_eq!(get_iter_fail.len(), 1, "{ir}");
    assert_eq!(releases(get_iter_fail[0]), 2, "{ir}");
}

/// A native comprehension (`[i for i in range(2) if copy.a]`) holds a
/// produced CPython-object filter across its truth test and releases it on
/// that test's failure edge and on its fallthrough, once per iteration.
#[test]
fn a_native_comprehension_releases_its_produced_object_filter() {
    let int = |value: i64| MirExpr::IntLiteral(value);
    let comprehension = MirExpr::Comprehension(Box::new(pycc_mir::MirComprehension {
        var: "i".to_string(),
        var_ty: Ty::Int,
        source: pycc_mir::CompSource::Range {
            start: int(0),
            stop: int(2),
            step: int(1),
        },
        cond: Some(attr("a")),
        elt: pycc_mir::MirCompElt::List(MirExpr::Name {
            name: "i".to_string(),
            ty: Ty::Int,
        }),
    }));
    let ir = discard_ir("release_native_comprehension_filter", comprehension);
    assert_eq!(releases(&ir), 2, "{ir}");
    let truthy_fail = blocks(&ir, "foreign_truthy_fail");
    assert_eq!(truthy_fail.len(), 1, "{ir}");
    assert_eq!(releases(truthy_fail[0]), 1, "{ir}");
}

#[path = "object_release_operand_tests.rs"]
mod operand;
