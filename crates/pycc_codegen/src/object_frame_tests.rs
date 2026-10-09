//! IR tests for `object_frame.rs` (Part 2 of #1499, #1502): a function-frame
//! `object` slot owns its reference.
//!
//! Every test compiles hand-built MIR as an `ext` object -- LLVM's verifier
//! runs before any assertion here is believed -- and reads the text of one
//! user function, or builds IR directly against the helpers.

use super::*;
use crate::{CompileOptions, compile_to_object_with_observer};
use inkwell::values::AnyValue;
use pycc_mir::{MirItem, MirModule, MirStmt, Ty};

const RETAIN: &str = "call void @pycc_ext_obj_retain(";
const RELEASE: &str = "call void @pycc_ext_obj_release(";
const EPILOGUE_LOAD: &str = "%object_epilogue_live";
/// The ext-mode call of a compiled function, through its function pointer.
const CALL_VOID: &str = "call void %load_fnptr(";
const CALL_PTR: &str = "call ptr %load_fnptr(";

fn import_copy() -> MirItem {
    MirItem::ForeignImport {
        local_name: "copy".to_string(),
        module_path: "copy".to_string(),
        from: None,
    }
}

fn name(name: &str) -> MirExpr {
    MirExpr::Name {
        name: name.to_string(),
        ty: Ty::Object,
    }
}

/// `copy.<attr>`: a produced new reference.
fn attr(attr: &str) -> MirExpr {
    MirExpr::ObjAttrGet {
        base: Box::new(name("copy")),
        attr: attr.to_string(),
        ty: Ty::Object,
    }
}

fn bind(target: &str, value: MirExpr) -> MirStmt {
    MirStmt::Assign {
        target: target.to_string(),
        value,
    }
}

fn call(callee: &str, args: Vec<MirExpr>, ty: Ty) -> MirExpr {
    MirExpr::Call {
        callee: callee.to_string(),
        args,
        ty,
    }
}

fn function(name: &str, params: &[(&str, Ty)], return_ty: Ty, body: Vec<MirStmt>) -> MirItem {
    MirItem::Function {
        name: name.to_string(),
        params: params
            .iter()
            .map(|(param, ty)| ((*param).to_string(), ty.clone()))
            .collect(),
        return_ty,
        body,
    }
}

/// `def sink(o: object) -> None: return`, a callee taking one object.
fn sink() -> MirItem {
    function(
        "sink",
        &[("o", Ty::Object)],
        Ty::None,
        vec![MirStmt::Return(None)],
    )
}

/// The LLVM text of each named function after compiling `items` as an `ext`
/// object.
fn compile(label: &str, items: Vec<MirItem>, names: &[&str]) -> Vec<String> {
    compile_as(label, items, names, true)
}

/// [`compile`], as an `ext` object or, when `ext` is false, a native one.
fn compile_as(label: &str, items: Vec<MirItem>, names: &[&str], ext: bool) -> Vec<String> {
    let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
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
            items,
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
    for (ir, name) in irs.iter().zip(names) {
        assert!(!ir.is_empty(), "no {name} was emitted");
    }
    irs
}

/// How many slot loads the owned-slot epilogue emits.
fn epilogue_loads(ir: &str) -> usize {
    defined_by(ir, EPILOGUE_LOAD).len()
}

fn at(ir: &str, needle: &str) -> usize {
    ir.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in:\n{ir}"))
}

/// The SSA name `%name` an instruction line starting with `%name =` and
/// containing `needle` defines.
fn defined_by<'a>(ir: &'a str, needle: &str) -> Vec<&'a str> {
    ir.lines()
        .filter(|line| line.contains(needle))
        .filter_map(|line| line.trim().split_once(" = ").map(|(name, _)| name))
        .collect()
}

/// A rebind of a local loads the old value, stores the new one, then
/// releases the old value -- the `Py_XSETREF` order -- unconditionally, with
/// no activation gate. A produced value is stored as it is.
#[test]
fn a_local_rebind_stores_first_then_releases_the_old_value() {
    let f = function(
        "f",
        &[],
        Ty::None,
        vec![
            bind("x", attr("a")),
            bind("x", attr("b")),
            MirStmt::Return(None),
        ],
    );
    let ir = &compile("frame_rebind", vec![import_copy(), f], &["pyfn_f"])[0];
    let olds: Vec<&str> = defined_by(ir, "= load ptr")
        .into_iter()
        .filter(|old| old.starts_with("%frame_old"))
        .collect();
    assert_eq!(olds.len(), 2, "{ir}");
    for old in olds {
        let load = at(ir, &format!("{old} = load ptr"));
        let release = at(ir, &format!("{RELEASE}ptr {old})"));
        let store = ir[load..]
            .find("store ptr")
            .map(|offset| load + offset)
            .expect("a store follows the load");
        assert!(load < store && store < release, "{ir}");
    }
    assert_eq!(ir.matches(RETAIN).count(), 0, "{ir}");
    assert!(!ir.contains("rebind_may_release"), "{ir}");
    // Two rebinds plus one epilogue release of `x`.
    assert_eq!(ir.matches(RELEASE).count(), 3, "{ir}");
    assert_eq!(epilogue_loads(ir), 1, "{ir}");
}

/// A local bound from a borrowed parameter retains it; the epilogue then
/// releases both the parameter and the local, after the last statement.
#[test]
fn a_borrowed_source_is_retained_and_every_slot_is_released_at_exit() {
    let g = function(
        "g",
        &[("y", Ty::Object)],
        Ty::None,
        vec![bind("x", name("y")), MirStmt::Return(None)],
    );
    let ir = &compile("frame_borrowed", vec![g], &["pyfn_g"])[0];
    assert_eq!(ir.matches(RETAIN).count(), 1, "{ir}");
    let retain = at(ir, RETAIN);
    let rebind = at(ir, "%frame_old = load ptr");
    assert!(retain < rebind, "{ir}");
    assert_eq!(epilogue_loads(ir), 2, "{ir}");
    // The local starts null, so a path that never binds it releases nothing.
    assert!(ir.contains("store ptr null"), "{ir}");
}

/// A borrowed argument is retained before the call so the callee's
/// parameter slot owns it; a produced one moves. Neither is released by the
/// caller once the call is made.
#[test]
fn an_object_argument_is_an_owned_reference_the_callee_takes_over() {
    let borrowed = function(
        "borrowed",
        &[("y", Ty::Object)],
        Ty::None,
        vec![
            MirStmt::ExprStmt(call("sink", vec![name("y")], Ty::None)),
            MirStmt::Return(None),
        ],
    );
    let produced = function(
        "produced",
        &[],
        Ty::None,
        vec![
            MirStmt::ExprStmt(call("sink", vec![attr("a")], Ty::None)),
            MirStmt::Return(None),
        ],
    );
    let irs = compile(
        "frame_argument",
        vec![import_copy(), sink(), borrowed, produced],
        &["pyfn_borrowed", "pyfn_produced"],
    );
    let borrowed = &irs[0];
    assert_eq!(borrowed.matches(RETAIN).count(), 1, "{borrowed}");
    assert!(at(borrowed, RETAIN) < at(borrowed, CALL_VOID), "{borrowed}");
    // Only `y`'s own epilogue release.
    assert_eq!(borrowed.matches(RELEASE).count(), 1, "{borrowed}");
    let produced = &irs[1];
    assert_eq!(produced.matches(RETAIN).count(), 0, "{produced}");
    assert_eq!(produced.matches(RELEASE).count(), 0, "{produced}");
}

/// A boxed `None` argument is CPython's borrowed `Py_None`, so it is
/// retained; a boxed `int` is the packer's new reference and moves.
#[test]
fn a_boxed_none_argument_is_retained_and_a_boxed_int_moves() {
    let none = function(
        "none",
        &[],
        Ty::None,
        vec![
            MirStmt::ExprStmt(call("sink", vec![MirExpr::NoneLiteral], Ty::None)),
            MirStmt::Return(None),
        ],
    );
    let int = function(
        "int",
        &[],
        Ty::None,
        vec![
            MirStmt::ExprStmt(call("sink", vec![MirExpr::IntLiteral(7)], Ty::None)),
            MirStmt::Return(None),
        ],
    );
    let irs = compile(
        "frame_boxed_argument",
        vec![sink(), none, int],
        &["pyfn_none", "pyfn_int"],
    );
    assert_eq!(irs[0].matches(RETAIN).count(), 1, "{}", irs[0]);
    assert_eq!(irs[1].matches(RETAIN).count(), 0, "{}", irs[1]);
}

/// A returned parameter is retained (the epilogue releases the slot's own
/// reference); a returned produced value moves.
#[test]
fn a_return_yields_an_owned_reference() {
    let ident = function(
        "ident",
        &[("y", Ty::Object)],
        Ty::Object,
        vec![MirStmt::Return(Some(name("y")))],
    );
    let fresh = function(
        "fresh",
        &[],
        Ty::Object,
        vec![MirStmt::Return(Some(attr("a")))],
    );
    let none = function(
        "none",
        &[],
        Ty::Object,
        vec![MirStmt::Return(Some(MirExpr::NoneLiteral))],
    );
    let irs = compile(
        "frame_return",
        vec![import_copy(), ident, fresh, none],
        &["pyfn_ident", "pyfn_fresh", "pyfn_none"],
    );
    let ident = &irs[0];
    assert_eq!(ident.matches(RETAIN).count(), 1, "{ident}");
    assert!(at(ident, RETAIN) < at(ident, EPILOGUE_LOAD), "{ident}");
    assert_eq!(irs[1].matches(RETAIN).count(), 0, "{}", irs[1]);
    assert_eq!(irs[2].matches(RETAIN).count(), 1, "{}", irs[2]);
}

/// A fully native executable (`ext` false; an embedded one compiles with
/// `ext` true) has no host to provide the retain and release shims,
/// so there frame slots own nothing: an `object` parameter, local, argument,
/// return and discarded call result emit no object traffic at all, and the
/// module links (`tests/issue_1394_generic_base.rs` builds one).
#[test]
fn a_native_executable_keeps_object_frames_borrowed() {
    let ident = function(
        "ident",
        &[("y", Ty::Object)],
        Ty::Object,
        vec![MirStmt::Return(Some(name("y")))],
    );
    let caller = function(
        "caller",
        &[("y", Ty::Object)],
        Ty::Object,
        vec![
            MirStmt::ExprStmt(call("ident", vec![name("y")], Ty::Object)),
            bind("x", call("ident", vec![name("y")], Ty::Object)),
            bind("x", name("y")),
            MirStmt::Return(Some(call("ident", vec![name("x")], Ty::Object))),
        ],
    );
    let irs = compile_as(
        "frame_native_executable",
        vec![ident, caller],
        &["pyfn_ident", "pyfn_caller"],
        false,
    );
    for ir in &irs {
        assert!(!ir.contains("pycc_ext_obj_"), "{ir}");
        assert!(!ir.contains("object_epilogue"), "{ir}");
        assert!(!ir.contains("frame_old"), "{ir}");
    }
}

/// A conditional expression selecting between a compiled call's `object`
/// result and a borrowed name (Part 3 of #1499): in the CPython-hosted
/// module the call arm is owned, so the name arm is retained and a discarded
/// selection is released; in a fully native executable the same node -- a
/// statement, a bound local and a returned value -- emits no object traffic
/// at all, so the module links without the shims.
#[test]
fn a_mixed_call_selection_retains_only_where_frames_own() {
    let ident = || {
        function(
            "ident",
            &[("y", Ty::Object)],
            Ty::Object,
            vec![MirStmt::Return(Some(name("y")))],
        )
    };
    let select = || MirExpr::IfExp {
        test: Box::new(MirExpr::Name {
            name: "flag".to_string(),
            ty: Ty::Bool,
        }),
        body: Box::new(call("ident", vec![name("y")], Ty::Object)),
        orelse: Box::new(name("y")),
        ty: Ty::Object,
    };
    let pick = || {
        function(
            "pick",
            &[("y", Ty::Object), ("flag", Ty::Bool)],
            Ty::Object,
            vec![
                MirStmt::ExprStmt(select()),
                bind("x", select()),
                MirStmt::Return(Some(select())),
            ],
        )
    };
    let native = &compile_as(
        "frame_native_selection",
        vec![ident(), pick()],
        &["pyfn_pick"],
        false,
    )[0];
    assert!(!native.contains("pycc_ext_obj_"), "{native}");
    let hosted = &compile_as(
        "frame_hosted_selection",
        vec![ident(), pick()],
        &["pyfn_pick"],
        true,
    )[0];
    let orelse = &hosted[hosted
        .find("ifexp_orelse:")
        .expect("the hosted node has an orelse arm")..];
    assert!(
        orelse[..orelse
            .find("br label")
            .expect("the arm branches to the join")]
            .contains(RETAIN),
        "{hosted}"
    );
    assert!(hosted.contains(RELEASE), "{hosted}");
}

/// An `object` call result is a new reference: a discarded one is released,
/// and one bound to a local moves into the slot without a retain.
#[test]
fn an_object_call_result_is_produced() {
    let fresh = function(
        "fresh",
        &[],
        Ty::Object,
        vec![MirStmt::Return(Some(attr("a")))],
    );
    let discard = function(
        "discard",
        &[],
        Ty::None,
        vec![
            MirStmt::ExprStmt(call("fresh", Vec::new(), Ty::Object)),
            MirStmt::Return(None),
        ],
    );
    let keep = function(
        "keep",
        &[],
        Ty::None,
        vec![
            bind("x", call("fresh", Vec::new(), Ty::Object)),
            MirStmt::Return(None),
        ],
    );
    let irs = compile(
        "frame_call_result",
        vec![import_copy(), fresh, discard, keep],
        &["pyfn_discard", "pyfn_keep"],
    );
    let discard = &irs[0];
    assert_eq!(discard.matches(RELEASE).count(), 1, "{discard}");
    assert!(at(discard, CALL_PTR) < at(discard, RELEASE), "{discard}");
    let keep = &irs[1];
    assert_eq!(keep.matches(RETAIN).count(), 0, "{keep}");
    assert!(crate::object_release::is_produced(&call(
        "fresh",
        Vec::new(),
        Ty::Object
    )));
    assert!(!crate::object_release::is_produced(&call(
        "n",
        Vec::new(),
        Ty::Int
    )));
}

/// A function with no `object` slot keeps its epilogue free of any object
/// traffic, so a native function's IR is unchanged.
#[test]
fn a_function_without_an_object_slot_emits_no_object_release() {
    let f = function(
        "f",
        &[("n", Ty::Int)],
        Ty::Int,
        vec![
            bind("m", MirExpr::IntLiteral(1)),
            MirStmt::Return(Some(MirExpr::IntLiteral(2))),
        ],
    );
    let ir = &compile("frame_native", vec![f], &["pyfn_f"])[0];
    assert!(!ir.contains("pycc_ext_obj_"), "{ir}");
    assert!(!ir.contains("object_epilogue"), "{ir}");
}

/// `assign` declines an unknown target, a non-object value, and an `object`
/// slot that is not a registered frame slot; for a registered slot without
/// an `initialized` flag it emits the load, store and release only.
#[test]
fn assign_handles_exactly_a_registered_frame_object_slot() {
    let context = Context::create();
    let module = context.create_module("frame_assign");
    let builder = context.create_builder();
    let rt = declare_rt_functions(&context, &module);
    let function = module.add_function("frame", context.void_type().fn_type(&[], false), None);
    builder.position_at_end(context.append_basic_block(function, "entry"));
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let flag = module
        .add_global(context.i8_type(), None, "flag")
        .as_pointer_value();
    let mut locals = HashMap::new();
    for (local, initialized) in [("o", None), ("p", None), ("q", Some(flag))] {
        locals.insert(
            local.to_string(),
            StorageSlot {
                ptr: module.add_global(ptr, None, local).as_pointer_value(),
                ty: Ty::Object,
                initialized,
            },
        );
    }
    assert!(!register(&rt, locals["p"].ptr), "disabled until enabled");
    assert!(!is_frame_slot(&rt, &locals["p"]));
    enable(&rt);
    assert!(register(&rt, locals["p"].ptr));
    assert!(register(&rt, locals["q"].ptr));
    assert!(!is_frame_slot(&rt, &locals["o"]));
    assert!(is_frame_slot(&rt, &locals["p"]));
    let object = Scalar::Object(ptr.const_null());
    let int = Scalar::Int(context.i64_type().const_zero());
    let value = attr("a");
    for (target, scalar) in [("missing", object), ("p", int), ("o", object)] {
        assert!(!assign(
            &context, &builder, &module, &rt, &locals, target, &value, scalar
        ));
    }
    assert!(assign(
        &context, &builder, &module, &rt, &locals, "p", &value, object
    ));
    assert!(assign(
        &context, &builder, &module, &rt, &locals, "q", &value, object
    ));
    release_slots(&context, &builder, &module, &[]);
    builder
        .build_return(None)
        .expect("build_return should not fail");
    let ir = crate::llvm_string_to_owned(function.print_to_string());
    assert_eq!(ir.matches("store ptr null").count(), 2, "{ir}");
    assert_eq!(ir.matches("store i8 1, ptr @flag").count(), 1, "{ir}");
    assert_eq!(ir.matches(RELEASE).count(), 2, "{ir}");
    assert!(!ir.contains("object_epilogue"), "{ir}");
}

/// `owned_return` leaves a non-object scalar, and an object returned from a
/// function whose return type is not `object`, unchanged.
#[test]
fn owned_return_touches_only_an_object_return() {
    let context = Context::create();
    let module = context.create_module("frame_owned_return");
    let builder = context.create_builder();
    let rt = declare_rt_functions(&context, &module);
    enable(&rt);
    let function = module.add_function("frame", context.void_type().fn_type(&[], false), None);
    builder.position_at_end(context.append_basic_block(function, "entry"));
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let int = Scalar::Int(context.i64_type().const_zero());
    let object = Scalar::Object(ptr.const_null());
    let borrowed = name("y");
    assert!(matches!(
        owned_return(
            &context,
            &builder,
            &module,
            &rt,
            &Ty::Int,
            &borrowed,
            false,
            int
        ),
        Scalar::Int(_)
    ));
    assert!(matches!(
        owned_return(
            &context,
            &builder,
            &module,
            &rt,
            &Ty::None,
            &borrowed,
            false,
            object
        ),
        Scalar::Object(_)
    ));
    builder
        .build_return(None)
        .expect("build_return should not fail");
    let ir = crate::llvm_string_to_owned(function.print_to_string());
    assert!(!ir.contains(RETAIN), "{ir}");
}

/// A store of an `object` into a registered frame slot that bypasses
/// `assign` would drop the old reference's release, so `emit_assign`
/// refuses it.
#[test]
#[should_panic(expected = "without object_frame::assign")]
fn emit_assign_refuses_a_registered_frame_object_slot() {
    let context = Context::create();
    let module = context.create_module("frame_emit_assign");
    let builder = context.create_builder();
    let rt = declare_rt_functions(&context, &module);
    let function = module.add_function("frame", context.void_type().fn_type(&[], false), None);
    builder.position_at_end(context.append_basic_block(function, "entry"));
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let slot = StorageSlot {
        ptr: module.add_global(ptr, None, "o").as_pointer_value(),
        ty: Ty::Object,
        initialized: None,
    };
    enable(&rt);
    register(&rt, slot.ptr);
    let mut locals = HashMap::new();
    locals.insert("o".to_string(), slot);
    emit_assign(
        &context,
        &builder,
        &rt,
        &locals,
        "o",
        Scalar::Object(ptr.const_null()),
    );
}
