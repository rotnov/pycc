//! Unit tests for #1050's `ext` export thunks.
//!
//! These are this crate's first `CompileOptions { ext: true, .. }` tests.
//! They assert the *exact* LLVM signature of each emitted thunk rather than
//! merely that one exists, because the signature is the whole contract: the
//! generated C in `src/ext_build.rs` declares the same symbol with widths it
//! derives independently from `crate::ext`'s convention, and nothing in
//! either toolchain can diagnose a disagreement. The `--ext` link defers
//! undefined symbols (`-undefined dynamic_lookup` on Mach-O, `-Bsymbolic` on
//! ELF), so a wrapper calling a thunk that was never emitted links cleanly
//! and dies on the first call -- which is why the negative assertions here
//! (no thunk for a private, dotted or monomorphized name) are as load-bearing
//! as the positive ones.
//!
//! Deliberately in a `*_tests.rs` file rather than appended to `tests.rs`:
//! that file is already 15k lines, and AGENTS.md's decomposability rule puts
//! a new unit beside the module it tests.

use super::*;
use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

/// One emitted thunk as the tests observe it: symbol, LLVM function type,
/// and the names of its basic blocks in order.
#[derive(Debug, PartialEq, Eq)]
struct Thunk {
    symbol: String,
    signature: String,
    blocks: Vec<String>,
}

/// Compiles `mir` as an `ext` object and reports every `pycc_ext_thunk_*`
/// function the module holds, in module order.
///
/// Goes all the way to an object file rather than stopping at the observer's
/// module, so LLVM's own verifier runs over the emitted thunks: a
/// mistyped `insertvalue` or a block left without a terminator fails here
/// rather than in a CPython process much later.
fn thunks_of(label: &str, mir: &MirModule) -> Vec<Thunk> {
    let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
    let obj_path = dir.join(format!("{label}.o"));
    let mut observed = Vec::new();
    let mut observer = |module: &inkwell::module::Module<'_>, _pipeline: Option<&'static str>| {
        for function in module.get_functions() {
            let symbol = function.get_name().to_string_lossy().into_owned();
            if !symbol.starts_with(EXT_THUNK_PREFIX) {
                continue;
            }
            observed.push(Thunk {
                symbol,
                // D-029: an `LLVMString` must reach an owned `String`
                // through the crate's own wrapper, never through its
                // `Drop`, which faults on Windows against this LLVM
                // release.
                signature: llvm_string_to_owned(function.get_type().print_to_string()),
                blocks: function
                    .get_basic_blocks()
                    .iter()
                    .map(|block| {
                        // LLVM disambiguates a requested name against every
                        // other value name already taken in the function, so
                        // the `fnptr_is_null` block comes back as
                        // `fnptr_is_null1` -- the `icmp` of the same name got
                        // there first. The suffix is an artifact of naming,
                        // not of structure, so it is trimmed here rather than
                        // written into each expectation.
                        let name = block.get_name().to_string_lossy().into_owned();
                        name.trim_end_matches(|c: char| c.is_ascii_digit())
                            .to_string()
                    })
                    .collect(),
            });
        }
    };
    compile_to_object_with_observer(
        mir,
        &obj_path,
        &CompileOptions {
            ext: true,
            ..CompileOptions::default()
        },
        Some(&mut observer),
    )
    .expect("ext codegen should succeed");
    observed
}

/// `def <name>(<params>) -> <return_ty>` with a body that just produces a
/// value of the declared return type. The body is irrelevant to thunk
/// emission -- which runs off the declaration pass -- but has to be
/// emittable for the compile to reach the verifier.
fn func(name: &str, params: &[(&str, Ty)], return_ty: Ty) -> MirItem {
    let body = match &return_ty {
        Ty::None => Vec::new(),
        ty => vec![MirStmt::Return(Some(value_of(ty)))],
    };
    MirItem::Function {
        name: name.to_string(),
        params: params
            .iter()
            .map(|(n, ty)| ((*n).to_string(), ty.clone()))
            .collect(),
        return_ty,
        body,
    }
}

fn value_of(ty: &Ty) -> MirExpr {
    match ty {
        Ty::Int => MirExpr::IntLiteral(7),
        Ty::Float => MirExpr::FloatLiteral(2.5),
        Ty::Bool => MirExpr::BoolLiteral(true),
        Ty::Tuple(elems) => MirExpr::TupleLiteral(elems.iter().map(value_of).collect()),
        other => panic!("no literal fixture for {other:?}"),
    }
}

fn module(items: Vec<MirItem>) -> MirModule {
    MirModule {
        items,
        class_defs: Vec::new(),
    }
}

fn tuple(elems: Vec<Ty>) -> Ty {
    Ty::Tuple(Box::new(elems))
}

#[test]
fn a_tuple_parameter_flattens_in_place_into_the_thunks_parameter_list() {
    let observed = thunks_of(
        "ext_thunk_tuple_param",
        &module(vec![func(
            "f",
            &[("t", tuple(vec![Ty::Int, Ty::Float]))],
            Ty::Int,
        )]),
    );

    assert_eq!(
        observed,
        vec![Thunk {
            symbol: "pycc_ext_thunk_f".to_string(),
            // `int` is `i64` and `float` is `f64`, matching
            // `ty_to_basic_type`; the aggregate itself never appears.
            signature: "i64 (i64, double)".to_string(),
            blocks: vec![
                "entry".to_string(),
                "fnptr_not_null".to_string(),
                "fnptr_is_null".to_string(),
            ],
        }]
    );
}

#[test]
fn a_tuple_return_leaves_through_one_trailing_out_pointer_per_element() {
    let observed = thunks_of(
        "ext_thunk_tuple_return",
        &module(vec![func("g", &[], tuple(vec![Ty::Int, Ty::Bool]))]),
    );

    // `void`, not the struct: returning the aggregate by value would put
    // pycc's own aggregate convention on the C side of the seam, which is
    // exactly what the thunk exists to prevent.
    assert_eq!(observed[0].symbol, "pycc_ext_thunk_g");
    assert_eq!(observed[0].signature, "void (ptr, ptr)");
    assert_eq!(observed.len(), 1);
}

#[test]
fn out_pointers_are_appended_after_every_flattened_parameter() {
    // The one fixture that pins the *order* of the combined list, and the
    // only one mixing `str` with a tuple: every parameter slot first, in
    // declaration order with tuples spread in place, then one out-pointer
    // per returned element.
    let observed = thunks_of(
        "ext_thunk_both_directions",
        &module(vec![func(
            "h",
            &[
                ("a", Ty::Int),
                ("t", tuple(vec![Ty::Int, Ty::Bool])),
                ("s", Ty::Str),
            ],
            tuple(vec![Ty::Float, Ty::Bool]),
        )]),
    );

    assert_eq!(
        observed[0].signature,
        "void (i64, i64, i8, ptr, ptr, ptr)".to_string()
    );
}

#[test]
fn a_tuple_carrying_export_returning_none_keeps_the_void_return() {
    // `-> None` is already LLVM `void`, so the thunk has no out-pointers
    // *and* no return value -- the arm that would silently read an
    // uninitialized call result if it shared the scalar path.
    let observed = thunks_of(
        "ext_thunk_none_return",
        &module(vec![func(
            "n",
            &[(
                "t",
                tuple(vec![Ty::Int, Ty::Int, Ty::Int, Ty::Int, Ty::Int]),
            )],
            Ty::None,
        )]),
    );

    assert_eq!(
        observed[0].signature,
        "void (i64, i64, i64, i64, i64)".to_string()
    );
}

#[test]
fn every_thunk_guards_the_function_pointer_slot_before_dispatching() {
    // The thunk reaches the compiled function through `fnptr_<name>`, not
    // through its mangled symbol, so a call that arrives before the `def`
    // executed has to raise pycc's `NameError` exactly as a Python-level
    // call would -- never jump through a null slot. `--ext` makes that
    // reachable in a way a native build never is: the host can import a
    // module whose body raised partway through and call an export anyway.
    let observed = thunks_of(
        "ext_thunk_null_guard",
        &module(vec![func("f", &[("t", tuple(vec![Ty::Int]))], Ty::Int)]),
    );

    assert_eq!(
        observed[0].blocks,
        vec![
            "entry".to_string(),
            "fnptr_not_null".to_string(),
            "fnptr_is_null".to_string(),
        ]
    );
}

#[test]
fn a_top_level_statement_is_stepped_over_rather_than_treated_as_an_export() {
    // `mir.items` interleaves the module body with the `def`s, so the
    // thunk loop walks statements as well as functions. A module whose
    // body runs before (and between) its definitions is the ordinary
    // shape, not an edge case -- and the statement carries no name,
    // parameters or return type to ask `ext_thunk_required` about.
    let observed = thunks_of(
        "ext_thunk_top_level_stmt",
        &module(vec![
            MirItem::TopLevelStmt(MirStmt::NoOp),
            func("f", &[("t", tuple(vec![Ty::Int, Ty::Float]))], Ty::Int),
            MirItem::TopLevelStmt(MirStmt::NoOp),
        ]),
    );

    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].symbol, "pycc_ext_thunk_f");
}

#[test]
fn a_rebound_public_name_gets_exactly_one_thunk() {
    // Two `def`s of one name share one `fnptr_` slot and one signature
    // (`T0021` refuses a redefinition that changes it), so a second thunk
    // would be a duplicate symbol -- the C compiler would reject the
    // artifact outright.
    let observed = thunks_of(
        "ext_thunk_rebound",
        &module(vec![
            func("f", &[("t", tuple(vec![Ty::Int, Ty::Int]))], Ty::Int),
            func("f", &[("t", tuple(vec![Ty::Int, Ty::Int]))], Ty::Int),
        ]),
    );

    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].symbol, "pycc_ext_thunk_f");
}

#[test]
fn no_thunk_is_emitted_for_a_name_the_export_set_excludes() {
    // Each of these carries a tuple and would otherwise qualify. A thunk
    // for one of them is not merely dead weight: `0gen_` names have no
    // `fnptr_` global at all, so emitting one would panic, and a `_private`
    // or `Class.method` thunk would advertise a symbol `collect_exports`
    // never generates a wrapper for.
    let observed = thunks_of(
        "ext_thunk_excluded_names",
        &module(vec![
            func("_private", &[("t", tuple(vec![Ty::Int]))], Ty::Int),
            func("Shape.area", &[("t", tuple(vec![Ty::Int]))], Ty::Int),
            func("0gen_pair", &[("t", tuple(vec![Ty::Int]))], Ty::Int),
        ]),
    );

    assert_eq!(observed, Vec::new());
}

#[test]
fn a_scalar_only_signature_gets_no_thunk() {
    // The wrapper for a scalar-only export still calls through
    // `fnptr_<name>` directly, so a thunk here would be emitted into every
    // artifact and called by nothing.
    let observed = thunks_of(
        "ext_thunk_scalar_only",
        &module(vec![
            func("f", &[("a", Ty::Int), ("s", Ty::Str)], Ty::Bool),
            func("g", &[], Ty::None),
        ]),
    );

    assert_eq!(observed, Vec::new());
}

#[test]
fn a_native_build_emits_no_thunks_at_all() {
    let mir = module(vec![func(
        "f",
        &[("t", tuple(vec![Ty::Int, Ty::Int]))],
        tuple(vec![Ty::Int, Ty::Int]),
    )]);
    let dir = pycc_scratch::ScratchDir::new("ext_thunk_native_build")
        .expect("failed to create scratch dir");
    let mut observed = Vec::new();
    let mut observer = |module: &inkwell::module::Module<'_>, _pipeline: Option<&'static str>| {
        observed.push(module.get_function("pycc_ext_thunk_f").is_some());
    };
    compile_to_object_with_observer(
        &mir,
        &dir.join("ext_thunk_native_build.o"),
        &CompileOptions::default(),
        Some(&mut observer),
    )
    .expect("native codegen should succeed");

    assert_eq!(observed, vec![false]);
}
