//! Emission for `MirExpr::ObjMethodCall` (Part 2 of #1026, PR 2b of #1081),
//! `MirExpr::ObjCall` (#1313, through the same argument marshalling),
//! `MirExpr::ObjSubscript` (Part 3 of #1026, PR 3b of #1082) and
//! `MirStmt::ForObject` (PR 3c of #1082).
//!
//! The two share this module because they share the *packer contract*: each
//! marshals a pycc scalar into a `PyObject *` through a `pycc_ext_obj_pack_*`
//! helper and hands the resulting owned reference to a shim helper that
//! releases it. `emit_subscript` reuses `packer_for` and `shim_fn`
//! unchanged rather than re-deriving them, so that contract cannot fork
//! into two spellings.
//!
//! The call counterpart of `foreign_attr.rs`, which this module reuses for
//! `expect_object_pointer`. Every failure edge is `foreign_fail.rs`'s: the
//! module-exec return inside `pycc_ext_module_exec`, and the bridge plus an
//! immediate branch to the innermost exception target inside any other
//! function (#1316). `emit_iter_loop` alone keeps the
//! `expect_module_exec_entry` assertion, because `pycc_types` still admits
//! `for x in <object>:` only in a module body (#1333).
//! What is new here is *argument marshalling*: each already-evaluated pycc
//! scalar becomes a `PyObject *` through one of the shim's
//! `pycc_ext_obj_pack_*` helpers, the results go into a stack array, and
//! `pycc_ext_obj_call` vectorcalls the already-resolved bound method.
//!
//! **Why the lookup is a separate step.** CPython resolves a call's
//! callable before it evaluates the arguments, so `obj.missing(1 // 0)`
//! raises `AttributeError` rather than `ZeroDivisionError`. `emit_lookup`
//! therefore emits `pycc_ext_obj_getattr` and its NULL check, and
//! `emit_expr`'s own arm runs it before the argument expressions; a fused
//! shim that did the load itself would necessarily run it last. The price
//! is a second NULL check and a second failure edge. The bound method still
//! never becomes a pycc value -- it is an LLVM temporary that
//! `pycc_ext_obj_call` releases -- so it does not join the boundary's
//! leaked set. `src/ext/pycc_ext_module.c`'s own comment on
//! `pycc_ext_obj_call` carries the full rationale.
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
use crate::foreign_fail::{ForeignFailEdge, route_null};
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
/// over what `pycc_types`' `HirExpr::MethodCall` and `HirExpr::Subscript`
/// arms admit for a `Ty::Object` base (`int`/`float`/`bool`/`str` -- an
/// argument in the first case, a key in the second). Any other `Scalar` is
/// a front-end defect: the checker refuses a container, an instance,
/// `None`, and a second `Ty::Object` before lowering ever runs.
fn packer_for<'ctx>(scalar: Scalar<'ctx>) -> (&'static str, BasicValueEnum<'ctx>) {
    match scalar {
        Scalar::Int(value) => (EXT_OBJ_PACK_INT_SYMBOL, value.into()),
        Scalar::Float(value) => (EXT_OBJ_PACK_FLOAT_SYMBOL, value.into()),
        Scalar::Bool(value) => (EXT_OBJ_PACK_BOOL_SYMBOL, value.into()),
        Scalar::Str(value) => (EXT_OBJ_PACK_STR_SYMBOL, value.into()),
        _ => panic!(
            "pycc_codegen: internal error: an operand of a foreign object operation did not \
             evaluate to a marshallable scalar -- pycc_types admits only `int`, `float`, \
             `bool` and `str` there"
        ),
    }
}

/// Allocates `slots` argument pointers in the *entry block* of `entry_fn`,
/// leaving the builder positioned exactly where it was.
///
/// An `alloca` is only reclaimed when its function returns, so emitting one
/// at the call site would make a module body's own loop grow the stack
/// without bound: `for i in range(n): gc.disable()` is an admitted program
/// -- and it segfaulted the hosting interpreter
/// at twenty million iterations before this hoist. The size is a compile-time
/// constant that depends on nothing in scope, so the entry block is always a
/// legal position for it, and LLVM's own convention is that every `alloca`
/// belongs there.
fn alloca_in_entry_block<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    entry_fn: FunctionValue<'ctx>,
    slots: usize,
) -> inkwell::values::PointerValue<'ctx> {
    super::build_at_entry_block(builder, entry_fn, |b| {
        b.build_array_alloca(
            context.ptr_type(inkwell::AddressSpace::default()),
            context.i64_type().const_int(slots as u64, false),
            "foreign_call_args",
        )
        .expect("build_array_alloca should not fail")
    })
}

/// The blocks and per-iteration item of a lowered `for x in <object>:`
/// loop, handed back to `emit_stmt` so it can emit the body between them.
pub(super) struct ForeignIterLoop<'ctx> {
    /// The block holding the `pycc_ext_obj_iter_next` call and the
    /// three-way switch; the body's back-edge targets it.
    pub header_bb: inkwell::basic_block::BasicBlock<'ctx>,
    /// Where control resumes after clean exhaustion.
    pub after_bb: inkwell::basic_block::BasicBlock<'ctx>,
    /// A *new* reference to this iteration's item, deliberately never
    /// released -- this is the reference that makes the boundary's leak
    /// trip-count-linear (#1092).
    pub item: PointerValue<'ctx>,
}

/// Emits the preheader and header of `for x in <object>:` (Part 3 of
/// #1026, PR 3c of #1082), leaving the builder positioned at the start of
/// the loop body with the first item already loaded.
///
/// # Shape
///
/// The preheader calls [`EXT_OBJ_GET_ITER_SYMBOL`] once -- Python binds the
/// iterator the `for` statement evaluated, so a body-level rebinding cannot
/// retarget the loop, exactly the reasoning `MirStmt::ForList`'s own
/// `list_ptr` read carries -- and routes a NULL through the module-exec
/// failure edge. The header calls [`EXT_OBJ_ITER_NEXT_SYMBOL`] and
/// **switches** on its three-valued result: `1` enters the body, `0` exits
/// the loop, and anything else -- `-1` and, fail-closed, any value the shim
/// could not produce -- takes a second failure edge of its own.
///
/// Those two are the only new unconditional `EXT_MODULE_EXEC_FAILED`
/// returns this PR adds; **exhaustion is deliberately not one of them**,
/// which is the entire reason the shim helper is three-valued rather than
/// NULL-signalling (see [`EXT_OBJ_ITER_NEXT_SYMBOL`]).
///
/// The out-parameter is a single `alloca` hoisted into the entry block via
/// [`alloca_in_entry_block`], never the header: an `alloca` in a block that
/// executes once per iteration grows the frame without bound, which is the
/// defect that helper exists to prevent.
pub(super) fn emit_iter_loop<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    iterable: Scalar<'ctx>,
) -> ForeignIterLoop<'ctx> {
    let entry_fn = expect_module_exec_entry(builder);
    let iterable_ptr = expect_object_pointer(iterable);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());

    let out_slot = alloca_in_entry_block(context, builder, entry_fn, 1);

    let get_iter = shim_fn(
        module,
        EXT_OBJ_GET_ITER_SYMBOL,
        ptr.fn_type(&[ptr.into()], false),
    );
    let iterator = builder
        .build_call(get_iter, &[iterable_ptr.into()], "foreign_iter_get")
        .expect("build_call should not fail for pycc_ext_obj_get_iter")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_get_iter returns PyObject *")
        .into_pointer_value();
    route_null(
        context,
        builder,
        module,
        rt,
        ForeignFailEdge::ModuleExec(entry_fn),
        iterator,
        "foreign_iter_get",
    );

    let header_bb = context.append_basic_block(entry_fn, "foreign_iter_header");
    let body_bb = context.append_basic_block(entry_fn, "foreign_iter_body");
    let after_bb = context.append_basic_block(entry_fn, "foreign_iter_after");
    let next_fail_bb = context.append_basic_block(entry_fn, "foreign_iter_next_fail");

    builder
        .build_unconditional_branch(header_bb)
        .expect("build_unconditional_branch should not fail entering the loop header");

    builder.position_at_end(header_bb);
    let iter_next = shim_fn(
        module,
        EXT_OBJ_ITER_NEXT_SYMBOL,
        context.i64_type().fn_type(&[ptr.into(), ptr.into()], false),
    );
    let status = builder
        .build_call(
            iter_next,
            &[iterator.into(), out_slot.into()],
            "foreign_iter_status",
        )
        .expect("build_call should not fail for pycc_ext_obj_iter_next")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_iter_next returns long long")
        .into_int_value();
    builder
        .build_switch(
            status,
            next_fail_bb,
            &[
                (context.i64_type().const_int(1, false), body_bb),
                (context.i64_type().const_zero(), after_bb),
            ],
        )
        .expect("build_switch should not fail for an i64 selector");

    builder.position_at_end(next_fail_bb);
    builder
        .build_return(Some(
            &context
                .i64_type()
                .const_int(EXT_MODULE_EXEC_FAILED as u64, true),
        ))
        .expect("build_return should not fail");

    builder.position_at_end(body_bb);
    let item = builder
        .build_load(ptr, out_slot, "foreign_iter_item")
        .expect("build_load should not fail for a slot this function allocated")
        .into_pointer_value();

    ForeignIterLoop {
        header_bb,
        after_bb,
        item,
    }
}

/// Emits the *callable lookup* of one `obj.method(args)` call, yielding the
/// bound method as an owned `PyObject *` that [`emit_call`] consumes.
///
/// Split from [`emit_call`] so that `emit_expr`'s own arm can run it
/// *before* it evaluates the argument expressions. CPython resolves a
/// call's callable first and only then evaluates the arguments, so
/// `obj.missing(1 // 0)` raises `AttributeError`; an earlier revision of
/// this module performed the lookup inside the call shim, after every
/// argument, and raised `ZeroDivisionError` instead.
///
/// The bound method still never becomes a pycc value: it is an LLVM
/// temporary that dominates the `pycc_ext_obj_call` consuming it, so it is
/// released rather than joining the boundary's leaked set. Keeping it in an
/// SSA value rather than an `alloca` also matters -- an `alloca` here would
/// grow a module-scope loop's stack per iteration, which is the defect
/// [`alloca_in_entry_block`] exists to prevent.
///
/// # Failure edge
///
/// A missing method returns `NULL` with CPython's `AttributeError` set,
/// which `foreign_fail::route_null` routes: the module-exec return inside
/// `pycc_ext_module_exec`, the bridge and an immediate branch to the
/// innermost exception target in any other function (#1316). The branch is
/// immediate because the arguments are evaluated next, with no guard in
/// between.
pub(super) fn emit_lookup<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    base: Scalar<'ctx>,
    method: &str,
) -> inkwell::values::PointerValue<'ctx> {
    let edge = ForeignFailEdge::for_current(builder);
    let base_ptr = expect_object_pointer(base);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let name = builder
        .build_global_string_ptr(method, &format!("pycc_foreign_method_{method}"))
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let getattr = shim_fn(
        module,
        EXT_OBJ_GETATTR_SYMBOL,
        ptr.fn_type(&[ptr.into(), ptr.into()], false),
    );
    let bound = builder
        .build_call(
            getattr,
            &[base_ptr.into(), name.into()],
            "foreign_call_bound",
        )
        .expect("build_call should not fail for pycc_ext_obj_getattr")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_getattr returns PyObject *")
        .into_pointer_value();
    route_null(
        context,
        builder,
        module,
        rt,
        edge,
        bound,
        "foreign_call_lookup",
    );
    bound
}

/// Marshals `args` and calls `bound`, yielding the call's result as an
/// opaque [`Scalar::Object`].
///
/// `bound` comes from [`emit_lookup`] and `args` are already-evaluated
/// scalars, so this function never recurses into the MIR and the evaluation
/// order visible in the emitted code is exactly CPython's: base, callable,
/// then each argument left to right.
///
/// `pycc_ext_obj_call` consumes `bound` and every packed argument on every
/// path; only the call's *result* outlives it, on the leak-only rule
/// `foreign_attr.rs` documents for an attribute load.
pub(super) fn emit_call<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    bound: inkwell::values::PointerValue<'ctx>,
    args: &[Scalar<'ctx>],
) -> Scalar<'ctx> {
    emit_call_with(
        context,
        builder,
        module,
        rt,
        EXT_OBJ_CALL_SYMBOL,
        bound,
        args,
    )
}

/// Marshals `args` and calls `callee` *itself* (#1313), yielding the call's
/// result as an opaque [`Scalar::Object`].
///
/// `callee` is the already-evaluated `object`-typed name -- a
/// `MirExpr::Name` read, which is a *borrow* of a reference the caller
/// keeps (a retained module global, or a `for` loop target's slot;
/// `lib.rs`'s `Ty::Object` load arm). It therefore goes to
/// [`EXT_OBJ_CALL_BORROWED_SYMBOL`], which takes its own reference before
/// delegating to the consuming `pycc_ext_obj_call`; passing the callee
/// straight to [`emit_call`]'s consuming helper would release the caller's
/// reference on the first call. The packed
/// arguments are consumed on every path, exactly as for a method call.
pub(super) fn emit_call_borrowed<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    callee: Scalar<'ctx>,
    args: &[Scalar<'ctx>],
) -> Scalar<'ctx> {
    let callee_ptr = expect_object_pointer(callee);
    emit_call_with(
        context,
        builder,
        module,
        rt,
        EXT_OBJ_CALL_BORROWED_SYMBOL,
        callee_ptr,
        args,
    )
}

/// The shared body of [`emit_call`] and [`emit_call_borrowed`]: the two
/// shim helpers take the same `(callable, args, nargs)` parameters and
/// differ only in who owns `callable`, so the packer loop, the hoisted
/// argument array and the failure edge are written once.
fn emit_call_with<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    symbol: &str,
    callable: inkwell::values::PointerValue<'ctx>,
    args: &[Scalar<'ctx>],
) -> Scalar<'ctx> {
    let edge = ForeignFailEdge::for_current(builder);
    let entry_fn = edge.function();
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let i64_type = context.i64_type();

    // `PyObject_Vectorcall` reads `nargs` slots, so a zero-argument call
    // needs no storage at all -- but LLVM's `alloca [0 x ptr]` yields a
    // pointer it is not meaningful to GEP into, and `gc.disable()` is
    // exactly the zero-argument shape this PR's acceptance test exercises.
    // One slot is always allocated and simply left unread.
    let slots = args.len().max(1);
    let arg_array = alloca_in_entry_block(context, builder, entry_fn, slots);
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

    let call = shim_fn(
        module,
        symbol,
        ptr.fn_type(&[ptr.into(), ptr.into(), i64_type.into()], false),
    );
    let result = builder
        .build_call(
            call,
            &[
                callable.into(),
                arg_array.into(),
                i64_type.const_int(args.len() as u64, false).into(),
            ],
            "foreign_call",
        )
        .expect("build_call should not fail for a foreign call helper")
        .try_as_basic_value()
        .expect_basic("a foreign call helper returns PyObject *")
        .into_pointer_value();

    route_null(context, builder, module, rt, edge, result, "foreign_call");
    Scalar::Object(result)
}

/// Emits one `o[k]` load, yielding the result as an opaque
/// [`Scalar::Object`].
///
/// Lives here rather than in a module of its own because it reuses this
/// one's three primitives unchanged -- [`packer_for`] for the key,
/// [`shim_fn`] for the declaration, [`route_null`] for the failure edge --
/// and because the *packer contract* is the thing that must not fork:
/// `pycc_ext_obj_getitem` consumes the packed key exactly as
/// `pycc_ext_obj_call` consumes a packed argument, so whatever a packer
/// creates is always released by the shim helper it is handed to.
///
/// That is also why no NULL check is emitted on the packed key. A failed
/// packer stores `NULL`, the shim tests for it and propagates the
/// already-set exception, and the operation therefore has exactly *one*
/// failure edge rather than two. The result is a new reference
/// that is deliberately never released, on the leak-only rule
/// `foreign_attr.rs` documents for an attribute load.
pub(super) fn emit_subscript<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    base: Scalar<'ctx>,
    index: Scalar<'ctx>,
) -> Scalar<'ctx> {
    let edge = ForeignFailEdge::for_current(builder);
    let base_ptr = expect_object_pointer(base);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());

    let (symbol, value) = packer_for(index);
    let packer = shim_fn(
        module,
        symbol,
        ptr.fn_type(&[value.get_type().into()], false),
    );
    let key = builder
        .build_call(packer, &[value.into()], "foreign_subscript_key")
        .unwrap_or_else(|_| panic!("build_call should not fail for {symbol}"))
        .try_as_basic_value()
        .expect_basic("a pycc_ext_obj_pack_* helper returns PyObject *")
        .into_pointer_value();

    let getitem = shim_fn(
        module,
        EXT_OBJ_GETITEM_SYMBOL,
        ptr.fn_type(&[ptr.into(), ptr.into()], false),
    );
    let result = builder
        .build_call(getitem, &[base_ptr.into(), key.into()], "foreign_subscript")
        .expect("build_call should not fail for pycc_ext_obj_getitem")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_getitem returns PyObject *")
        .into_pointer_value();

    route_null(
        context,
        builder,
        module,
        rt,
        edge,
        result,
        "foreign_subscript",
    );
    Scalar::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod call_tests;
    use crate::{CompileOptions, EXT_MODULE_EXEC_SYMBOL, compile_to_object_with_observer};
    use inkwell::values::AnyValue;
    use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

    /// `import <module>` followed by one discarded
    /// `<module>.<method>(args)` call.
    ///
    /// A discarded `ExprStmt` was the only statement position PR 2b admitted
    /// end to end -- `pycc_types` then refused binding a CPython object to
    /// a name (`I0404`; admitted at module scope since #1325) -- so it is
    /// the shape every test here builds.
    fn call(module: &str, method: &str, args: Vec<MirExpr>) -> Vec<MirItem> {
        vec![
            MirItem::ForeignImport {
                local_name: module.to_string(),
                module_path: module.to_string(),
                from: None,
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjMethodCall {
                base: Box::new(MirExpr::Name {
                    name: module.to_string(),
                    ty: Ty::Object,
                }),
                method: method.to_string(),
                args,
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

    /// The *lookup*'s own failure edge, which the call edge above does not
    /// cover: a missing method makes `pycc_ext_obj_getattr` return NULL
    /// before any argument is packed, and that NULL must reach the same
    /// `ret i64 -1`.
    #[test]
    fn a_failed_method_lookup_returns_on_the_module_exec_failure_edge() {
        let ir = entry_ir(
            "foreign_call_lookup_fail_edge",
            call("gc", "definitely_not_there", vec![MirExpr::IntLiteral(1)]),
        );
        assert!(ir.contains("foreign_call_lookup_failed"), "{ir}");
        assert!(ir.contains("foreign_call_lookup_fail:"), "{ir}");
        assert!(ir.contains("foreign_call_lookup_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// CPython resolves a call's callable before it evaluates the
    /// arguments, so `obj.missing(1 // 0)` raises `AttributeError` rather
    /// than `ZeroDivisionError`. This pins that order where it is decided:
    /// the `pycc_ext_obj_getattr` call site must precede every packer call
    /// site in the emitted entry function.
    #[test]
    fn the_callable_is_resolved_before_any_argument_is_packed() {
        let ir = entry_ir(
            "foreign_call_lookup_order",
            call("gc", "set_threshold", vec![MirExpr::IntLiteral(1)]),
        );
        let lookup_at = ir
            .find(EXT_OBJ_GETATTR_SYMBOL)
            .unwrap_or_else(|| panic!("no method lookup was emitted: {ir}"));
        let pack_at = ir
            .find(EXT_OBJ_PACK_INT_SYMBOL)
            .unwrap_or_else(|| panic!("no argument packer was emitted: {ir}"));
        assert!(lookup_at < pack_at, "{ir}");
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
    #[should_panic(expected = "did not evaluate to a CPython object")]
    fn a_non_object_base_is_an_internal_error() {
        entry_ir(
            "foreign_call_bad_base",
            vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(
                MirExpr::ObjMethodCall {
                    base: Box::new(MirExpr::IntLiteral(1)),
                    method: "disable".to_string(),
                    args: Vec::new(),
                },
            ))],
        );
    }

    /// `import <module>` followed by one discarded `<module>[index]`
    /// load -- the subscript counterpart of [`call`] above, and for the
    /// same reason: a discarded `ExprStmt` is the only statement position
    /// PR 3b admits end to end.
    fn subscript(module: &str, index: MirExpr) -> Vec<MirItem> {
        vec![
            MirItem::ForeignImport {
                local_name: module.to_string(),
                module_path: module.to_string(),
                from: None,
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjSubscript {
                base: Box::new(MirExpr::Name {
                    name: module.to_string(),
                    ty: Ty::Object,
                }),
                index: Box::new(index),
            })),
        ]
    }

    /// The subscript load reaches the shim by its shared symbol, and does
    /// so without emitting the attribute lookup or the call helper: `o[k]`
    /// is one `PyObject_GetItem`, not a `__getitem__` lookup followed by a
    /// call.
    ///
    /// The symbol is asserted through [`EXT_OBJ_GETITEM_SYMBOL`] rather
    /// than against a literal for the same reason the call test gives: the
    /// C definition and this declaration are resolved lazily at load time,
    /// so a literal spelled twice would be a crash at first call rather
    /// than a link error.
    #[test]
    fn a_foreign_subscript_reaches_the_shim_by_its_shared_symbol() {
        let ir = entry_ir(
            "foreign_subscript_symbol",
            subscript("gc", MirExpr::IntLiteral(0)),
        );
        assert!(ir.contains(EXT_OBJ_GETITEM_SYMBOL), "{ir}");
        assert!(ir.contains(EXT_OBJ_PACK_INT_SYMBOL), "{ir}");
        assert!(!ir.contains(EXT_OBJ_GETATTR_SYMBOL), "{ir}");
        assert!(!ir.contains(EXT_OBJ_CALL_SYMBOL), "{ir}");
    }

    /// One packer per admitted key type, each exercised through its own
    /// key, so a regression naming the wrong packer for one type cannot
    /// hide behind another's symbol being present.
    ///
    /// `packer_for` is shared with the method-call path, but the *key*
    /// slot is a second caller of it, and it is the slot whose ownership
    /// rule the shim implements (`pycc_ext_obj_getitem` consumes the
    /// reference the packer returns, on every path).
    #[test]
    fn each_admitted_key_type_reaches_its_own_packer() {
        for (label, key, symbol) in [
            ("int", MirExpr::IntLiteral(7), EXT_OBJ_PACK_INT_SYMBOL),
            (
                "float",
                MirExpr::FloatLiteral(1.5),
                EXT_OBJ_PACK_FLOAT_SYMBOL,
            ),
            ("bool", MirExpr::BoolLiteral(true), EXT_OBJ_PACK_BOOL_SYMBOL),
            (
                "str",
                MirExpr::StringLiteral("k".to_string()),
                EXT_OBJ_PACK_STR_SYMBOL,
            ),
        ] {
            let ir = entry_ir(
                &format!("foreign_subscript_key_{label}"),
                subscript("gc", key),
            );
            assert!(ir.contains(symbol), "{label}: {ir}");
            assert!(ir.contains("foreign_subscript_key"), "{label}: {ir}");
            assert!(ir.contains(EXT_OBJ_GETITEM_SYMBOL), "{label}: {ir}");
        }
    }

    /// A failed load stops the module body on the module-exec failure
    /// edge, rather than continuing with a `NULL` object.
    ///
    /// This is the one new unconditional `EXT_MODULE_EXEC_FAILED` edge PR
    /// 3b adds (#1096): a failed *key packer* does not get its own, because
    /// `pycc_ext_obj_getitem` tolerates a `NULL` key and returns `NULL`
    /// itself, folding that case into this same branch.
    #[test]
    fn a_failed_foreign_subscript_returns_on_the_module_exec_failure_edge() {
        let ir = entry_ir(
            "foreign_subscript_fail_edge",
            subscript("gc", MirExpr::IntLiteral(0)),
        );
        assert!(ir.contains("foreign_subscript_failed"), "{ir}");
        assert!(ir.contains("foreign_subscript_fail:"), "{ir}");
        assert!(ir.contains("foreign_subscript_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// `import <module>` followed by `for x in <module>.attr:` with a
    /// body that consumes the loop variable.
    ///
    /// `ObjLen` is the body statement because it is the one operation on
    /// the loop variable that produces a value a discarded `ExprStmt` can
    /// hold, so the target store is provably read rather than dead.
    fn iter_loop(module: &str, body: Vec<MirStmt>) -> Vec<MirItem> {
        vec![
            MirItem::ForeignImport {
                local_name: module.to_string(),
                module_path: module.to_string(),
                from: None,
            },
            MirItem::TopLevelStmt(MirStmt::ForObject {
                var: "x".to_string(),
                iter: MirExpr::ObjAttrGet {
                    base: Box::new(MirExpr::Name {
                        name: module.to_string(),
                        ty: Ty::Object,
                    }),
                    attr: "garbage".to_string(),
                    ty: Ty::Object,
                },
                body,
            }),
        ]
    }

    /// The body used by every loop test here: `len(x)`, discarded.
    fn iter_body() -> Vec<MirStmt> {
        vec![MirStmt::ExprStmt(MirExpr::ObjLen {
            base: Box::new(MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Object,
            }),
        })]
    }

    /// **The block structure of a `for x in <object>:` loop**, asserted as
    /// a shape rather than as a set of symbols.
    ///
    /// Six appended blocks, listed in the order emission creates them: the
    /// `get_iter` NULL test's own fail/continuation pair (appended by the
    /// shared `foreign_fail::route_null` helper), then the header, body, after and
    /// next-failure blocks. The `get_iter` call itself is emitted into the
    /// current block and appends none. The array below is the list; do not
    /// restate its length in prose here or in `docs/RUNTIME.md`. Naming each one and asserting on the label is
    /// what makes a regression that collapses two of them -- most
    /// dangerously, routing exhaustion into the failure block -- fail here
    /// instead of only in the `#[ignore]`d hosted test.
    #[test]
    fn a_foreign_for_loop_emits_its_full_block_structure() {
        let ir = entry_ir("foreign_iter_blocks", iter_loop("gc", iter_body()));
        for label in [
            "foreign_iter_get_fail:",
            "foreign_iter_get_cont:",
            "foreign_iter_header:",
            "foreign_iter_body:",
            "foreign_iter_after:",
            "foreign_iter_next_fail:",
        ] {
            assert!(ir.contains(label), "missing {label}: {ir}");
        }
        assert!(ir.contains(EXT_OBJ_GET_ITER_SYMBOL), "{ir}");
        assert!(ir.contains(EXT_OBJ_ITER_NEXT_SYMBOL), "{ir}");
    }

    /// **The three-way switch.** `1` enters the body, `0` leaves the loop,
    /// and every other value -- `-1` and anything the shim could not
    /// produce -- takes the failure block, because it is the `switch`'s
    /// *default*.
    ///
    /// Asserted against the emitted `switch` instruction itself, not
    /// against a pair of comparisons: a regression that replaced the
    /// switch with two `icmp`s could keep all six block labels above and
    /// still send an unexpected status somewhere harmless.
    #[test]
    fn a_foreign_for_loop_switches_three_ways_on_the_iterator_status() {
        let ir = entry_ir("foreign_iter_switch", iter_loop("gc", iter_body()));
        let switch = ir
            .lines()
            .find(|line| line.trim_start().starts_with("switch i64 "))
            .unwrap_or_else(|| panic!("no switch instruction was emitted: {ir}"));
        assert!(
            switch.contains("label %foreign_iter_next_fail"),
            "the default destination is the failure block: {switch}"
        );
        let cases: String = ir
            .lines()
            .skip_while(|line| !line.trim_start().starts_with("switch i64 "))
            .take(4)
            .collect();
        assert!(
            cases.contains("i64 1, label %foreign_iter_body"),
            "a written item enters the body: {cases}"
        );
        assert!(
            cases.contains("i64 0, label %foreign_iter_after"),
            "clean exhaustion leaves the loop: {cases}"
        );
    }

    /// **Two** new unconditional `EXT_MODULE_EXEC_FAILED` edges, and
    /// exactly two (#1096): a `NULL` from `pycc_ext_obj_get_iter` and a
    /// `-1` from `pycc_ext_obj_iter_next`. Clean exhaustion is
    /// deliberately not one of them, which is why the shim helper is
    /// three-valued rather than NULL-signalling.
    #[test]
    fn a_foreign_for_loop_adds_exactly_two_module_exec_failure_edges() {
        let ir = entry_ir("foreign_iter_fail_edges", iter_loop("gc", iter_body()));
        assert!(ir.contains("foreign_iter_get_failed"), "{ir}");
        // Each of the two named failure blocks carries exactly one
        // `ret i64 EXT_MODULE_EXEC_FAILED`, and they are the only blocks
        // this statement adds that do. Counted per named block rather than
        // over the whole entry point, because the iterable expression and
        // the loop body carry pre-existing edges of their own.
        for label in ["foreign_iter_get_fail:", "foreign_iter_next_fail:"] {
            let block: String = ir
                .lines()
                .skip_while(|line| !line.starts_with(label))
                .skip(1)
                .take_while(|line| !line.trim().is_empty())
                .collect();
            assert_eq!(
                block
                    .matches(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}"))
                    .count(),
                1,
                "{label} returns the module-exec failure value exactly once: {ir}"
            );
        }
    }

    /// The out-parameter is allocated **once**, in the entry block, not
    /// once per iteration: an `alloca` inside the header would grow the
    /// frame without bound on a long iteration, which is exactly the
    /// defect [`alloca_in_entry_block`] exists to prevent.
    #[test]
    fn a_foreign_for_loop_allocates_its_out_parameter_in_the_entry_block() {
        let ir = entry_ir("foreign_iter_alloca", iter_loop("gc", iter_body()));
        let header_start = ir
            .find("foreign_iter_header:")
            .unwrap_or_else(|| panic!("no header block: {ir}"));
        assert!(
            !ir[header_start..].contains("alloca"),
            "no alloca may appear at or after the loop header: {ir}"
        );
        assert_eq!(
            ir.matches("= alloca ptr, i64 1").count(),
            1,
            "exactly one out-parameter slot is allocated: {ir}"
        );
    }

    /// A `Return` inside the loop body already terminates its block, so
    /// the back-edge must not be built on top of it -- the same
    /// terminator-safety guard `MirStmt::ForList` carries. LLVM's verifier
    /// runs inside `entry_ir`, so a second terminator would fail the
    /// compile rather than this assertion.
    ///
    /// A module body cannot contain a `return`, so the terminating
    /// statement here is a `raise`, which reaches the same guard.
    #[test]
    fn a_foreign_for_loop_body_that_already_terminates_gets_no_back_edge() {
        let ir = entry_ir(
            "foreign_iter_terminated_body",
            iter_loop("gc", vec![MirStmt::Unreachable]),
        );
        assert!(ir.contains("foreign_iter_after:"), "{ir}");
    }

    /// A codegen error raised *inside* the loop body propagates out of the
    /// `ForObject` arm instead of being swallowed, exactly as the `ForList`
    /// and `try*` arms propagate theirs.
    ///
    /// `MirExceptionValue::Constructed` with a non-string message is the
    /// standard way to make `emit_body` fail (`crates/pycc_codegen/src/
    /// tests.rs`'s `a_codegen_error_in_a_try_star_body_propagates_...`),
    /// and it is the only error `emit_body` can return at all.
    #[test]
    fn a_codegen_error_in_a_foreign_for_loop_body_propagates() {
        let dir = pycc_scratch::ScratchDir::new("foreign_iter_body_codegen_error")
            .expect("failed to create scratch dir");
        let err = compile_to_object_with_observer(
            &MirModule {
                items: iter_loop(
                    "gc",
                    vec![MirStmt::Raise {
                        exception: pycc_mir::MirExceptionValue::Constructed {
                            type_tag: 1,
                            class_name: "ValueError".to_string(),
                            message: MirExpr::IntLiteral(42),
                        },
                        frame_function: "test_fn".to_string(),
                    }],
                ),
                ..Default::default()
            },
            &dir.join("foreign_iter_body_codegen_error.o"),
            &CompileOptions {
                ext: true,
                ..CompileOptions::default()
            },
            None,
        )
        .expect_err("a codegen error in the loop body must propagate");
        assert!(err.contains("message must be a string"), "{err}");
    }

    /// The defensive arm in `foreign_attr::expect_object_pointer` reached
    /// through the *loop* node, which has its own call to it.
    #[test]
    #[should_panic(expected = "did not evaluate to a CPython object")]
    fn a_non_object_for_loop_iterable_is_an_internal_error() {
        entry_ir(
            "foreign_iter_bad_iterable",
            vec![MirItem::TopLevelStmt(MirStmt::ForObject {
                var: "x".to_string(),
                iter: MirExpr::IntLiteral(1),
                body: Vec::new(),
            })],
        );
    }

    /// The defensive arm in `foreign_attr::expect_object_pointer` reached
    /// through the *subscript* node, which has its own call to it.
    #[test]
    #[should_panic(expected = "did not evaluate to a CPython object")]
    fn a_non_object_subscript_base_is_an_internal_error() {
        entry_ir(
            "foreign_subscript_bad_base",
            vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(
                MirExpr::ObjSubscript {
                    base: Box::new(MirExpr::IntLiteral(1)),
                    index: Box::new(MirExpr::IntLiteral(0)),
                },
            ))],
        );
    }

    /// The defensive arm in [`packer_for`] reached through the key slot:
    /// `pycc_types` refuses every key type that has no packer, so reaching
    /// it is a front-end defect.
    #[test]
    #[should_panic(expected = "did not evaluate to a marshallable scalar")]
    fn an_unmarshallable_subscript_key_is_an_internal_error() {
        entry_ir(
            "foreign_subscript_bad_key",
            subscript("gc", MirExpr::NoneLiteral),
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
