//! Emission for `MirExpr::ObjMethodCall` (Part 2 of #1026, PR 2b of #1081).
//!
//! The call counterpart of `foreign_attr.rs`, which this module reuses for
//! everything the two share -- `expect_object_pointer`, and the
//! `expect_module_exec_entry` assertion that pins the enclosing function.
//! What is new here is *argument marshalling*: each already-evaluated pycc
//! scalar becomes a `PyObject *` through one of the shim's
//! `pycc_ext_obj_pack_*` helpers, the results go into a stack array, and
//! one `pycc_ext_obj_call` performs the attribute load and the vectorcall
//! together.
//!
//! **Why the shim fuses load and call.** An `ObjAttrGet` feeding a separate
//! call node would put the bound method object into a pycc value (so it
//! would join the boundary's leaked set instead of being released the
//! moment the call returns) and would emit two `NULL` checks and two
//! failure edges where one suffices. `src/ext/pycc_ext_module.c`'s own
//! comment on `pycc_ext_obj_call` carries the full rationale.
//!
//! **Ownership** (`docs/RUNTIME.md`). The packers *borrow* their pycc-side
//! arguments -- ownership of every argument stays with the compiled module
//! body, so nothing here has to balance a transfer, and no
//! `pending_int_releases` bookkeeping (D-208, #638) is needed: that machinery
//! exists for a transfer, and this boundary performs none. The `PyObject *`
//! references the packers create are owned by the array and consumed by
//! `pycc_ext_obj_call` on every path, so the only thing that outlives the
//! call is its *result* -- a new reference that is deliberately never
//! released, on exactly the leak-only rule `foreign_attr.rs` documents for
//! an attribute load.

use super::*;
use crate::foreign_attr::{expect_module_exec_entry, expect_object_pointer};
use inkwell::builder::Builder;
use inkwell::values::BasicValueEnum;

/// Declares one of the shim's helpers once per module, returning the
/// existing declaration on every later call.
///
/// `foreign_attr.rs`'s `obj_getattr_fn` generalized over the parameter
/// list, because this module declares five symbols rather than one and a
/// second `add_function` of any one name is an LLVM module-verifier error.
fn shim_fn<'ctx>(
    module: &inkwell::module::Module<'ctx>,
    symbol: &str,
    fn_type: inkwell::types::FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(symbol) {
        return existing;
    }
    module.add_function(symbol, fn_type, None)
}

/// The packer symbol and the LLVM argument value for one marshalled
/// argument.
///
/// Every admitted scalar has exactly one packer, and the mapping is total
/// over what `pycc_types`' `HirExpr::MethodCall` arm admits for a
/// `Ty::Object` base (`int`/`float`/`bool`/`str`). Any other `Scalar` is a
/// front-end defect: the checker refuses a container, an instance, `None`,
/// and a second `Ty::Object` before lowering ever runs.
fn packer_for<'ctx>(scalar: Scalar<'ctx>) -> (&'static str, BasicValueEnum<'ctx>) {
    match scalar {
        Scalar::Int(value) => (EXT_OBJ_PACK_INT_SYMBOL, value.into()),
        Scalar::Float(value) => (EXT_OBJ_PACK_FLOAT_SYMBOL, value.into()),
        Scalar::Bool(value) => (EXT_OBJ_PACK_BOOL_SYMBOL, value.into()),
        Scalar::Str(value) => (EXT_OBJ_PACK_STR_SYMBOL, value.into()),
        _ => panic!(
            "pycc_codegen: internal error: an argument to a foreign method call did not \
             evaluate to a marshallable scalar -- pycc_types admits only `int`, `float`, \
             `bool` and `str` there"
        ),
    }
}

/// Emits one `obj.method(args)` call against a CPython object and yields
/// the call's result as an opaque [`Scalar::Object`].
///
/// `base` and `args` are already-evaluated scalars: the caller
/// (`emit_expr`'s own `MirExpr::ObjMethodCall` arm) walks the MIR, so this
/// function never recurses into it and the evaluation order visible here is
/// exactly source order -- base first, then each argument left to right.
///
/// # Which side owns the "CPython raised" transition
///
/// The same answer `foreign_attr::emit` gives, and for the same reason.
/// `pycc_ext_obj_call` returns `NULL` with *CPython's* error indicator set,
/// which pycc's own pending-exception guard (D-173) cannot see, so this arm
/// emits the `NULL` check itself and routes the failure to the module-exec
/// failure edge: return [`EXT_MODULE_EXEC_FAILED`] immediately, leaving
/// CPython's exception exactly as the shim left it. The host then reports
/// the real `AttributeError` for a missing method, or whatever the method
/// itself raised, and the remaining module-body statements never run.
///
/// This is why PR 2b needs no CPython-to-pycc exception bridge at all.
/// `pycc_rt::exception` carries no `AttributeError` tag, and the plan
/// treated adding one (or accepting a documented semantics downgrade) as an
/// open item; because 2b's calls inherit PR 2a's positional bound -- a
/// foreign object is readable only in a *module body below its own import*
/// -- every admitted call is emitted inside `pycc_ext_module_exec`, where
/// this edge exists. The item dissolves rather than being deferred.
///
/// # Why the enclosing function is always the module-exec entry
///
/// `expect_module_exec_entry` asserts it, before any block is appended, on
/// exactly `foreign_attr::emit`'s reasoning: `pycc_types` refuses reading a
/// foreign object inside a function body (`I0404`), so the failure edge's
/// `ret i64 -1` is always emitted into a function that returns `i64`.
pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    base: Scalar<'ctx>,
    method: &str,
    args: &[Scalar<'ctx>],
) -> Scalar<'ctx> {
    let entry_fn = expect_module_exec_entry(builder);
    let base_ptr = expect_object_pointer(base);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let i64_type = context.i64_type();

    // `PyObject_Vectorcall` reads `nargs` slots, so a zero-argument call
    // needs no storage at all -- but LLVM's `alloca [0 x ptr]` yields a
    // pointer it is not meaningful to GEP into, and `gc.disable()` is
    // exactly the zero-argument shape this PR's acceptance test exercises.
    // One slot is always allocated and simply left unread.
    let slots = args.len().max(1);
    let arg_array = builder
        .build_array_alloca(
            ptr,
            i64_type.const_int(slots as u64, false),
            "foreign_call_args",
        )
        .expect("build_array_alloca should not fail");
    for (index, arg) in args.iter().enumerate() {
        let (symbol, value) = packer_for(*arg);
        let packer = shim_fn(
            module,
            symbol,
            ptr.fn_type(&[value.get_type().into()], false),
        );
        let packed = builder
            .build_call(packer, &[value.into()], "foreign_call_arg")
            .unwrap_or_else(|_| panic!("build_call should not fail for {symbol}"))
            .try_as_basic_value()
            .expect_basic("a pycc_ext_obj_pack_* helper returns PyObject *")
            .into_pointer_value();
        let slot = unsafe {
            builder
                .build_in_bounds_gep(
                    ptr,
                    arg_array,
                    &[i64_type.const_int(index as u64, false)],
                    "foreign_call_arg_slot",
                )
                .expect("build_in_bounds_gep should not fail")
        };
        builder
            .build_store(slot, packed)
            .expect("build_store should not fail");
    }

    let name = builder
        .build_global_string_ptr(method, &format!("pycc_foreign_method_{method}"))
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let call = shim_fn(
        module,
        EXT_OBJ_CALL_SYMBOL,
        ptr.fn_type(
            &[ptr.into(), ptr.into(), ptr.into(), i64_type.into()],
            false,
        ),
    );
    let result = builder
        .build_call(
            call,
            &[
                base_ptr.into(),
                name.into(),
                arg_array.into(),
                i64_type.const_int(args.len() as u64, false).into(),
            ],
            "foreign_call",
        )
        .expect("build_call should not fail for pycc_ext_obj_call")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_call returns PyObject *")
        .into_pointer_value();

    let failed = builder
        .build_is_null(result, "foreign_call_failed")
        .expect("build_is_null should not fail");
    let fail_bb = context.append_basic_block(entry_fn, "foreign_call_fail");
    let cont_bb = context.append_basic_block(entry_fn, "foreign_call_cont");
    builder
        .build_conditional_branch(failed, fail_bb, cont_bb)
        .expect("build_conditional_branch should not fail");
    builder.position_at_end(fail_bb);
    builder
        .build_return(Some(
            &context
                .i64_type()
                .const_int(EXT_MODULE_EXEC_FAILED as u64, true),
        ))
        .expect("build_return should not fail");
    builder.position_at_end(cont_bb);
    Scalar::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileOptions, EXT_MODULE_EXEC_SYMBOL, compile_to_object_with_observer};
    use inkwell::values::AnyValue;
    use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

    /// `import <module>` followed by one discarded
    /// `<module>.<method>(args)` call.
    ///
    /// A discarded `ExprStmt` is the only statement position PR 2b admits
    /// end to end -- `pycc_types` still refuses binding a CPython object to
    /// a name (`I0404`) -- so it is the shape every test here builds.
    fn call(module: &str, method: &str, args: Vec<MirExpr>) -> Vec<MirItem> {
        vec![
            MirItem::ForeignImport {
                local_name: module.to_string(),
                module_path: module.to_string(),
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjMethodCall {
                base: Box::new(MirExpr::Name {
                    name: module.to_string(),
                    ty: Ty::Object,
                }),
                method: method.to_string(),
                args,
                ty: Ty::Object,
            })),
        ]
    }

    /// The LLVM text of the module-exec entry point after compiling `items`
    /// as an `ext` object -- `foreign_attr.rs`'s own `entry_ir`, which is
    /// where the rationale for compiling all the way to an object file
    /// (LLVM's verifier runs before any assertion is believed) lives.
    fn entry_ir(label: &str, items: Vec<MirItem>) -> String {
        let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
        let mut ir = String::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            if let Some(entry) = module.get_function(EXT_MODULE_EXEC_SYMBOL) {
                ir = crate::llvm_string_to_owned(entry.print_to_string());
            }
        };
        compile_to_object_with_observer(
            &MirModule {
                items,
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
        assert!(!ir.is_empty(), "no {EXT_MODULE_EXEC_SYMBOL} was emitted");
        ir
    }

    /// The zero-argument shape this PR's acceptance test builds
    /// (`gc.disable()`): the call reaches the shim by its shared symbol,
    /// the method name reaches it as a global string, and no packer is
    /// emitted.
    ///
    /// The symbol is asserted through [`EXT_OBJ_CALL_SYMBOL`] rather than
    /// against a literal for `foreign_attr.rs`'s reason: the C definition
    /// and this declaration are resolved lazily at load time, so a literal
    /// spelled twice would be a crash at first call rather than a link
    /// error.
    #[test]
    fn a_zero_argument_foreign_method_call_reaches_the_shim_by_its_shared_symbol() {
        let ir = entry_ir("foreign_call_zero_arg", call("gc", "disable", Vec::new()));
        assert!(ir.contains(EXT_OBJ_CALL_SYMBOL), "{ir}");
        assert!(ir.contains("pycc_foreign_method_disable"), "{ir}");
        assert!(!ir.contains(EXT_OBJ_PACK_INT_SYMBOL), "{ir}");
        assert!(
            ir.contains("i64 0"),
            "the arity is passed as a constant: {ir}"
        );
    }

    /// One packer per admitted scalar type, each exercised through its own
    /// argument.
    ///
    /// Written as four separate one-argument calls rather than one
    /// four-argument call so a regression naming the wrong packer for one
    /// type cannot hide behind another's symbol being present.
    #[test]
    fn each_admitted_argument_type_reaches_its_own_packer() {
        for (label, arg, symbol) in [
            ("int", MirExpr::IntLiteral(12345), EXT_OBJ_PACK_INT_SYMBOL),
            (
                "float",
                MirExpr::FloatLiteral(-1.0),
                EXT_OBJ_PACK_FLOAT_SYMBOL,
            ),
            ("bool", MirExpr::BoolLiteral(true), EXT_OBJ_PACK_BOOL_SYMBOL),
            (
                "str",
                MirExpr::StringLiteral("x".to_string()),
                EXT_OBJ_PACK_STR_SYMBOL,
            ),
        ] {
            let ir = entry_ir(
                &format!("foreign_call_arg_{label}"),
                call("gc", "set_threshold", vec![arg]),
            );
            assert!(ir.contains(symbol), "{label}: {ir}");
            assert!(ir.contains("foreign_call_args"), "{label}: {ir}");
            assert!(ir.contains("i64 1"), "the arity is one: {label}: {ir}");
        }
    }

    /// A failed call stops the module body on the module-exec failure edge,
    /// rather than continuing with a `NULL` object.
    ///
    /// This is the edge that makes a missing method surface as CPython's
    /// own `AttributeError` and a raising method as its own exception,
    /// instead of the `SystemError: execution of module n raised unreported
    /// exception` PR 2a's review found when the edge was missing.
    #[test]
    fn a_failed_foreign_method_call_returns_on_the_module_exec_failure_edge() {
        let ir = entry_ir(
            "foreign_call_fail_edge",
            call("gc", "definitely_not_there", Vec::new()),
        );
        assert!(ir.contains("foreign_call_failed"), "{ir}");
        assert!(ir.contains("foreign_call_fail:"), "{ir}");
        assert!(ir.contains("foreign_call_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// Two calls in one module share one extern declaration per symbol.
    ///
    /// `shim_fn` returns the existing `FunctionValue` on every call after
    /// the first; a second `add_function` of one name is an LLVM
    /// module-verifier error, so the second call (which also repeats the
    /// `int` packer) is what proves the early return is taken rather than
    /// merely present.
    #[test]
    fn a_second_foreign_method_call_reuses_the_one_extern_declaration() {
        let mut items = call("gc", "set_threshold", vec![MirExpr::IntLiteral(1)]);
        items.extend(call("gc", "set_debug", vec![MirExpr::IntLiteral(2)]));
        let ir = entry_ir("foreign_call_twice", items);
        let count = |needle: &str| {
            let mut found = 0usize;
            let mut rest = ir.as_str();
            while let Some(at) = rest.find(needle) {
                found += 1;
                rest = &rest[at + needle.len()..];
            }
            found
        };
        assert_eq!(
            count(EXT_OBJ_CALL_SYMBOL),
            2,
            "one call site per call: {ir}"
        );
        assert_eq!(
            count(EXT_OBJ_PACK_INT_SYMBOL),
            2,
            "one packer call site per int argument: {ir}"
        );
    }

    /// The defensive arm in `foreign_attr::expect_object_pointer`, reached
    /// through this node: only `pycc_mir`'s own lowering builds it, and only
    /// over a `Ty::Object` base.
    #[test]
    #[should_panic(expected = "a foreign attribute base did not evaluate to a CPython object")]
    fn a_non_object_base_is_an_internal_error() {
        entry_ir(
            "foreign_call_bad_base",
            vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(
                MirExpr::ObjMethodCall {
                    base: Box::new(MirExpr::IntLiteral(1)),
                    method: "disable".to_string(),
                    args: Vec::new(),
                    ty: Ty::Object,
                },
            ))],
        );
    }

    /// The defensive arm in [`packer_for`]: `pycc_types` refuses every
    /// argument type that has no packer, so reaching it is a front-end
    /// defect. Reached here by handing the node a `None` argument, which no
    /// type-checked program produces.
    #[test]
    #[should_panic(expected = "did not evaluate to a marshallable scalar")]
    fn an_unmarshallable_argument_is_an_internal_error() {
        entry_ir(
            "foreign_call_bad_arg",
            call("gc", "disable", vec![MirExpr::NoneLiteral]),
        );
    }
}
