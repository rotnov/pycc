//! #1490: a call that reaches a function-pointer slot whose `def` has not
//! executed raises a catchable `NameError` instead of aborting.
//!
//! Every test compiles real MIR to an object -- LLVM's verifier runs before
//! any assertion here is believed -- and reads the LLVM text of the user
//! function `f` (mangled `pyfn_f`) or of an export thunk. The behaviour
//! itself is pinned end to end against CPython by
//! `tests/issue_1490_unbound_slot_name_error.rs`; these pin the shape the
//! coverage host can see without a CPython: which raise each build mode
//! calls, which name it reports, that the null block is left through the
//! exception target rather than `unreachable`, and that the slot check
//! precedes argument evaluation and instance allocation, as CPython's own
//! name lookup does.

use crate::{
    CompileOptions, EXT_NAME_ERROR_SYMBOL, EXT_THUNK_PREFIX, compile_to_object_with_observer,
};
use inkwell::values::AnyValue;
use pycc_mir::{InstantiateExpr, MirExpr, MirItem, MirModule, MirStmt, Ty};

/// The LLVM text of every function named in `names`, after compiling
/// `items` under `options`.
fn functions_ir(
    label: &str,
    items: Vec<MirItem>,
    options: &CompileOptions,
    names: &[&str],
) -> Vec<String> {
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
        options,
        Some(&mut observer),
    )
    .expect("codegen should succeed");
    for (ir, name) in irs.iter().zip(names) {
        assert!(!ir.is_empty(), "no {name} was emitted");
    }
    irs
}

fn native() -> CompileOptions {
    CompileOptions::default()
}

fn ext() -> CompileOptions {
    CompileOptions {
        ext: true,
        ..CompileOptions::default()
    }
}

/// The body of the basic block whose label starts with `prefix` (LLVM
/// suffixes a repeated name with a number), up to the next label.
fn block<'ir>(ir: &'ir str, prefix: &str) -> &'ir str {
    let start = ir
        .lines()
        .position(|line| line.starts_with(prefix) && line.contains(':'))
        .unwrap_or_else(|| panic!("no block {prefix} in:\n{ir}"));
    let lines: Vec<&str> = ir.lines().collect();
    let end = lines[start + 1..]
        .iter()
        .position(|line| !line.starts_with(' ') && !line.is_empty())
        .map_or(lines.len(), |offset| start + 1 + offset);
    let first = lines[start].as_ptr() as usize - ir.as_ptr() as usize;
    let last = lines[end - 1].as_ptr() as usize - ir.as_ptr() as usize + lines[end - 1].len();
    &ir[first..last]
}

/// The entry block: everything before the first branch leaves it.
fn entry(ir: &str) -> &str {
    block(ir, "entry")
}

fn int_function(name: &str, params: &[&str], body: MirExpr) -> MirItem {
    MirItem::Function {
        name: name.to_string(),
        params: params
            .iter()
            .map(|param| ((*param).to_string(), Ty::Int))
            .collect(),
        return_ty: Ty::Int,
        body: vec![MirStmt::Return(Some(body))],
    }
}

fn call(callee: &str, args: Vec<MirExpr>) -> MirExpr {
    MirExpr::Call {
        callee: callee.to_string(),
        args,
        ty: Ty::Int,
    }
}

/// `def h(x: int) -> int: return x`, `def side() -> int: return 1` and
/// `def f() -> int: return h(side())`.
fn late_call_items() -> Vec<MirItem> {
    vec![
        int_function("f", &[], call("h", vec![call("side", Vec::new())])),
        int_function(
            "h",
            &["x"],
            MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Int,
            },
        ),
        int_function("side", &[], MirExpr::IntLiteral(1)),
    ]
}

#[test]
fn a_native_unbound_call_raises_through_the_runtime_and_reaches_the_exception_target() {
    let [ir] = functions_ir(
        "unbound_native_call",
        late_call_items(),
        &native(),
        &["pyfn_f"],
    )
    .try_into()
    .unwrap();
    let null = block(&ir, "fnptr_is_null");
    // The function's own name global, reported with its length (the
    // terminator excluded), and no CPython shim in a native build.
    assert!(
        null.contains("call void @pycc_rt_name_error(ptr @fnname_h, i64 1)"),
        "{ir}"
    );
    assert!(!ir.contains(EXT_NAME_ERROR_SYMBOL), "{ir}");
    // Before #1490 the null block ended in `unreachable` behind a
    // panicking call; it now leaves through the exception target (no
    // enclosing `try`: the function's own exceptional return).
    assert!(!null.contains("unreachable"), "{ir}");
    assert!(null.contains("br label"), "{ir}");
}

#[test]
fn the_callee_slot_is_checked_before_any_argument_is_evaluated() {
    // CPython looks `h` up before evaluating `side()`, so `h(side())` with
    // `h` unbound never calls `side`. The entry block loads `h`'s slot and
    // branches on it; `side`'s slot is read only on the bound path.
    let [ir] = functions_ir(
        "unbound_call_order",
        late_call_items(),
        &native(),
        &["pyfn_f"],
    )
    .try_into()
    .unwrap();
    let entry = entry(&ir);
    assert!(entry.contains("@fnptr_h"), "{ir}");
    assert!(!entry.contains("@fnptr_side"), "{ir}");
    assert!(!block(&ir, "fnptr_is_null").contains("@fnptr_side"), "{ir}");
}

#[test]
fn an_ext_unbound_call_raises_through_the_cpython_shim() {
    // A host build raises CPython's own `NameError` and bridges it, so the
    // host sees the real class; the panicking-era runtime call is gone.
    let [ir] = functions_ir("unbound_ext_call", late_call_items(), &ext(), &["pyfn_f"])
        .try_into()
        .unwrap();
    let null = block(&ir, "fnptr_is_null");
    assert!(
        null.contains(&format!(
            "call void @{EXT_NAME_ERROR_SYMBOL}(ptr @fnname_h, i64 1)"
        )),
        "{ir}"
    );
    assert!(!ir.contains("@pycc_rt_name_error"), "{ir}");
    assert!(!null.contains("unreachable"), "{ir}");
}

/// `class C` with `def __init__(self) -> None`, and `def f() -> C: return
/// C()`.
fn instantiate_items() -> Vec<MirItem> {
    let self_ty = Ty::Instance(Box::new("C".to_string()));
    vec![
        MirItem::Function {
            name: "f".to_string(),
            params: Vec::new(),
            return_ty: self_ty.clone(),
            body: vec![MirStmt::Return(Some(MirExpr::Instantiate(Box::new(
                InstantiateExpr {
                    ctor: "C.__init__".to_string(),
                    class_name: "C".to_string(),
                    slot_names: vec!["v".to_string()],
                    args: Vec::new(),
                    ty: self_ty.clone(),
                },
            ))))],
        },
        MirItem::Function {
            name: "C.__init__".to_string(),
            params: vec![("self".to_string(), self_ty)],
            return_ty: Ty::None,
            body: vec![MirStmt::Return(None)],
        },
    ]
}

#[test]
fn a_construction_before_its_class_statement_reports_the_class_name() {
    // CPython reports `name 'C' is not defined` for `C()` above `class C`,
    // not the constructor's dotted name.
    let [ir] = functions_ir(
        "unbound_native_ctor",
        instantiate_items(),
        &native(),
        &["pyfn_f"],
    )
    .try_into()
    .unwrap();
    let null = block(&ir, "fnptr_is_null");
    assert!(
        null.contains("call void @pycc_rt_name_error(ptr @fnname_C, i64 1)"),
        "{ir}"
    );
    assert!(!null.contains("unreachable"), "{ir}");
}

#[test]
fn a_construction_checks_the_ctor_slot_before_allocating_the_instance() {
    // Allocating first would leak the instance on the `NameError` path; the
    // allocation lives on the bound path only.
    let [ir] = functions_ir(
        "unbound_ctor_order",
        instantiate_items(),
        &ext(),
        &["pyfn_f"],
    )
    .try_into()
    .unwrap();
    let entry = entry(&ir);
    assert!(entry.contains("@fnptr_0m1_C8___init__"), "{ir}");
    assert!(!entry.contains("pycc_rt_instance_new"), "{ir}");
    let null = block(&ir, "fnptr_is_null");
    assert!(!null.contains("pycc_rt_instance_new"), "{ir}");
    assert!(
        null.contains(&format!(
            "call void @{EXT_NAME_ERROR_SYMBOL}(ptr @fnname_C, i64 1)"
        )),
        "{ir}"
    );
    assert!(
        block(&ir, "fnptr_not_null").contains("pycc_rt_instance_new"),
        "{ir}"
    );
}

#[test]
fn a_second_construction_reuses_the_class_name_constant() {
    // Two `C()` sites share one `fnname_C` constant rather than minting
    // `fnname_C.1`.
    let self_ty = Ty::Instance(Box::new("C".to_string()));
    let construct = || {
        MirExpr::Instantiate(Box::new(InstantiateExpr {
            ctor: "C.__init__".to_string(),
            class_name: "C".to_string(),
            slot_names: vec!["v".to_string()],
            args: Vec::new(),
            ty: self_ty.clone(),
        }))
    };
    let mut items = instantiate_items();
    items.push(MirItem::Function {
        name: "g".to_string(),
        params: Vec::new(),
        return_ty: self_ty.clone(),
        body: vec![
            MirStmt::ExprStmt(construct()),
            MirStmt::Return(Some(construct())),
        ],
    });
    let [ir] = functions_ir("unbound_ctor_reuse", items, &native(), &["pyfn_g"])
        .try_into()
        .unwrap();
    assert_eq!(ir.matches("(ptr @fnname_C, i64 1)").count(), 2, "{ir}");
    assert!(!ir.contains("@fnname_C."), "{ir}");
}

#[test]
fn an_export_thunks_unbound_path_raises_through_the_shim_and_returns() {
    // The C wrapper checks the slot before calling a thunk (#1199), so this
    // path is defensive; should it run, the thunk returns a zero carrier
    // with the `NameError` pending for the wrapper to translate.
    let tuple = Ty::Tuple(Box::new(vec![Ty::Int]));
    let items = vec![
        MirItem::Function {
            name: "t".to_string(),
            params: vec![("p".to_string(), tuple)],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(7)))],
        },
        MirItem::Function {
            name: "u".to_string(),
            params: vec![("p".to_string(), Ty::Tuple(Box::new(vec![Ty::Int])))],
            return_ty: Ty::None,
            body: vec![MirStmt::Return(None)],
        },
    ];
    let (t, u) = (
        format!("{EXT_THUNK_PREFIX}t"),
        format!("{EXT_THUNK_PREFIX}u"),
    );
    let [int_thunk, void_thunk] = functions_ir("unbound_thunk", items, &ext(), &[&t, &u])
        .try_into()
        .unwrap();
    let null = block(&int_thunk, "fnptr_is_null");
    assert!(
        null.contains(&format!(
            "call void @{EXT_NAME_ERROR_SYMBOL}(ptr @fnname_t, i64 1)"
        )),
        "{int_thunk}"
    );
    assert!(null.contains("ret i64 0"), "{int_thunk}");
    assert!(!null.contains("unreachable"), "{int_thunk}");
    let null = block(&void_thunk, "fnptr_is_null");
    assert!(null.contains("@fnname_u, i64 1)"), "{void_thunk}");
    assert!(null.contains("ret void"), "{void_thunk}");
}
