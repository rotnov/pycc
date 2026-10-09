//! IR tests for `object_attr.rs` (Part 4 of #1499, #1504): a compiled
//! instance's `object` attribute owns its reference.
//!
//! Every test compiles hand-built MIR -- LLVM's verifier runs before any
//! assertion here is believed -- and reads the text of one user function.
//! The hosted behaviour, against CPython itself, is pinned by
//! `tests/issue_1504_instance_object_attr_ownership.rs`.

use super::*;
use crate::{CompileOptions, compile_to_object_with_observer};
use inkwell::values::AnyValue;
use pycc_mir::{MirItem, MirModule, MirStmt, Ty};

const RETAIN: &str = "call void @pycc_ext_obj_retain(";
const RELEASE: &str = "call void @pycc_ext_obj_release(";
const OLD_RELEASE: &str = "call void @pycc_ext_obj_release(ptr %object_attr_old_ptr";

fn instance() -> Ty {
    Ty::Instance(Box::new("C".to_string()))
}

fn name(name: &str, ty: Ty) -> MirExpr {
    MirExpr::Name {
        name: name.to_string(),
        ty,
    }
}

/// `o.<slot>`, of type `ty`.
fn get(slot: usize, ty: Ty) -> MirExpr {
    MirExpr::AttrGet {
        base: Box::new(name("o", instance())),
        slot,
        ty,
    }
}

/// `o.<slot> = value`.
fn set(slot: usize, value: MirExpr) -> MirStmt {
    MirStmt::AttrSet {
        base: name("o", instance()),
        slot,
        value,
    }
}

/// `copy.<attr>`: a produced new reference.
fn produced(attr: &str) -> MirExpr {
    MirExpr::ObjAttrGet {
        base: Box::new(name("copy", Ty::Object)),
        attr: attr.to_string(),
        ty: Ty::Object,
    }
}

/// The text of `pyfn_f`, a function of `o: C` and `v: object` whose body is
/// `body`, compiled with `ext` as given.
fn function_ir(label: &str, body: Vec<MirStmt>, ext: bool) -> String {
    let f = MirItem::Function {
        name: "f".to_string(),
        params: vec![("o".to_string(), instance()), ("v".to_string(), Ty::Object)],
        return_ty: Ty::None,
        body: body
            .into_iter()
            .chain(std::iter::once(MirStmt::Return(None)))
            .collect(),
    };
    let import = MirItem::ForeignImport {
        local_name: "copy".to_string(),
        module_path: "copy".to_string(),
        from: None,
    };
    let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
    let mut ir = String::new();
    let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
        let function = module.get_function("pyfn_f").expect("pyfn_f is emitted");
        ir = crate::llvm_string_to_owned(function.print_to_string());
    };
    compile_to_object_with_observer(
        &MirModule {
            items: vec![import, f],
            ..Default::default()
        },
        &dir.join(format!("{label}.o")),
        &CompileOptions {
            ext,
            ..CompileOptions::default()
        },
        Some(&mut observer),
    )
    .expect("codegen should succeed");
    ir
}

fn at(ir: &str, needle: &str) -> usize {
    ir.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in:\n{ir}"))
}

/// A store reads the old word, stores the new one, and only then releases
/// the old one -- the `Py_XSETREF` order -- retaining a borrowed value
/// before it stores.
#[test]
fn a_store_releases_the_replaced_word_after_storing_it() {
    let ir = function_ir(
        "object_attr_store",
        vec![set(0, name("v", Ty::Object))],
        true,
    );
    let retain = at(&ir, RETAIN);
    let old = at(
        &ir,
        "%object_attr_old = call i64 @pycc_rt_instance_get_slot(",
    );
    let store = at(&ir, "@pycc_rt_instance_set_slot(");
    let release = at(&ir, OLD_RELEASE);
    assert!(retain < store, "{ir}");
    assert!(old < store, "{ir}");
    assert!(store < release, "{ir}");
    assert_eq!(ir.matches(OLD_RELEASE).count(), 1, "{ir}");
}

/// A produced value moves into the slot: no retain.
#[test]
fn a_produced_value_moves_into_the_slot() {
    let ir = function_ir("object_attr_move", vec![set(0, produced("copy"))], true);
    assert_eq!(ir.matches(RETAIN).count(), 0, "{ir}");
    assert_eq!(ir.matches(OLD_RELEASE).count(), 1, "{ir}");
}

/// An `object` read retains the slot's word right after the checked read,
/// and the read is a producer: binding it moves it, so `x = o.a` and
/// `o.b = o.a` each take exactly one reference, the read's own.
#[test]
fn an_object_read_is_a_new_reference_a_binding_moves() {
    let ir = function_ir(
        "object_attr_read",
        vec![
            MirStmt::Assign {
                target: "x".to_string(),
                value: get(0, Ty::Object),
            },
            set(1, get(0, Ty::Object)),
        ],
        true,
    );
    assert_eq!(ir.matches(RETAIN).count(), 2, "{ir}");
    let read = at(&ir, "@pycc_rt_instance_get_slot_checked(");
    assert!(read < at(&ir, RETAIN), "{ir}");
    assert!(object_release::is_produced(&get(0, Ty::Object)));
    assert!(!object_release::is_produced(&get(0, Ty::Int)));
}

/// A discarded read is released by its statement, like any produced value.
#[test]
fn a_discarded_object_read_is_released() {
    let ir = function_ir(
        "object_attr_discard",
        vec![MirStmt::ExprStmt(get(0, Ty::Object))],
        true,
    );
    assert_eq!(ir.matches(RETAIN).count(), 1, "{ir}");
    // The discarded read's release, then the epilogue's release of `v`.
    assert_eq!(ir.matches(RELEASE).count(), 2, "{ir}");
    assert!(at(&ir, RETAIN) < at(&ir, RELEASE), "{ir}");
}

/// An `int`, `float`, `str` or `bool` attribute store and read emit no
/// object reference traffic, in a hosted module or a native one.
#[test]
fn a_native_attribute_emits_no_object_traffic() {
    let body = || {
        vec![
            set(0, MirExpr::IntLiteral(1)),
            set(1, MirExpr::FloatLiteral(1.5)),
            set(2, MirExpr::BoolLiteral(true)),
            MirStmt::Assign {
                target: "i".to_string(),
                value: get(0, Ty::Int),
            },
            MirStmt::Assign {
                target: "x".to_string(),
                value: get(1, Ty::Float),
            },
        ]
    };
    for ext in [true, false] {
        let ir = function_ir(&format!("object_attr_native_{ext}"), body(), ext);
        assert!(!ir.contains("@pycc_ext_obj_retain"), "{ir}");
        assert!(!ir.contains("object_attr_old"), "{ir}");
    }
}
