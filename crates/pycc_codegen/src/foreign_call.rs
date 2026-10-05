//! Emission for `MirExpr::ObjMethodCall` (Part 2 of #1026, PR 2b of #1081),
//! `MirExpr::ObjCall` (#1313, through the same argument marshalling; a
//! computed callee since Part 2a of #1371, see [`callee_is_produced`]),
//! `MirExpr::ObjSubscript` (Part 3 of #1026, PR 3b of #1082),
//! `MirExpr::ObjSlice` (Part 2b of #1371, see [`emit_slice`]) and
//! `MirStmt::ForObject` (PR 3c of #1082).
//!
//! They share this module because they share the *packer contract*: each
//! marshals a pycc operand into a `PyObject *` through a `pycc_ext_obj_pack_*`
//! helper and hands the resulting owned reference to a shim helper that
//! releases it. The contract's primitives live in `foreign_pack.rs` (split
//! out in Part 2a of #1371), which `foreign_compare.rs` uses too, so it
//! cannot fork into two spellings.
//!
//! The call counterpart of `foreign_attr.rs`, which this module reuses for
//! `expect_object_pointer`. Every failure edge is `foreign_fail.rs`'s: the
//! module-exec return inside `pycc_ext_module_exec`, and the bridge plus an
//! immediate branch to the innermost exception target inside any other
//! function (#1316). `emit_iter_loop` alone keeps the
//! `expect_module_exec_entry` assertion, because `pycc_types` still admits
//! `for x in <object>:` only in a module body (Part 2 of #1333).
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
use crate::foreign_pack::{emit_pack, shim_fn};
use inkwell::builder::Builder;

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
/// `callee` is an already-evaluated *borrowed* object -- a `MirExpr::Name`
/// read of a reference the caller keeps (a retained module global, or a
/// `for` loop target's slot; `lib.rs`'s `Ty::Object` load arm), or any
/// other callee [`callee_is_produced`] does not list. It therefore goes to
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

/// Whether an `ObjCall` callee evaluates to a *new* reference that nothing
/// else holds, so the call may consume it (Part 2a of #1371).
///
/// An allowlist of the shim's own new-reference producers: a subscript load
/// (`pycc_ext_obj_getitem`), an attribute load (`pycc_ext_obj_getattr`) and
/// a method or direct call's result (`pycc_ext_obj_call`). Each such result
/// is otherwise leaked under the leak-only rule (#1092), so handing it to
/// the consuming `pycc_ext_obj_call` is what releases it. Every other
/// callee is a *borrow* and goes to `pycc_ext_obj_call_borrowed`: a `Name`
/// read (#1313), and also the pycc `__class_getitem__` method call that
/// `C[k]` lowers to for a pycc class base, whose `object` result is the
/// caller's pointer handed back without a new reference
/// (`docs/RUNTIME.md`). Consuming that would underflow the refcount; the
/// borrowed helper is at worst a leak, which is why an unlisted node
/// defaults to it.
pub(super) fn callee_is_produced(callee: &MirExpr) -> bool {
    matches!(
        callee,
        MirExpr::ObjSubscript { .. }
            | MirExpr::ObjAttrGet { .. }
            | MirExpr::ObjMethodCall { .. }
            | MirExpr::ObjCall { .. }
    )
}

/// Emits `callee(args)` for an `ObjCall` once the callee and arguments are
/// evaluated, routing the callee to the consuming or the borrowing shim
/// helper by its MIR shape ([`callee_is_produced`]). A produced callee whose
/// arguments raise before this point is not released: the argument's own
/// failure edge leaves first, on the leak-only rule.
pub(super) fn emit_object_call<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    callee_mir: &MirExpr,
    callee: Scalar<'ctx>,
    args: &[Scalar<'ctx>],
) -> Scalar<'ctx> {
    if callee_is_produced(callee_mir) {
        let callee_ptr = expect_object_pointer(callee);
        emit_call(context, builder, module, rt, callee_ptr, args)
    } else {
        emit_call_borrowed(context, builder, module, rt, callee, args)
    }
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
        let packed = emit_pack(context, builder, module, *arg, "foreign_call_arg");
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
/// Lives here rather than in a module of its own because it reuses the
/// call path's primitives unchanged -- [`emit_pack`] for the key,
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

    let key = emit_pack(context, builder, module, index, "foreign_subscript_key");

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

/// Emits one `o[start:stop:step]` load (Part 2b of #1371), yielding the
/// result as an opaque [`Scalar::Object`]; `None` is an absent bound.
///
/// The same packer contract as [`emit_subscript`]: every present bound is
/// packed and handed to `pycc_ext_obj_getslice`, which consumes it on every
/// path -- a failed packer's `NULL` included -- so the operation keeps
/// exactly one failure edge, on a `NULL` result. An absent bound is passed
/// as a null pointer with its presence bit clear, and the helper hands
/// `PySlice_New` CPython's `None` for it. The result is a new reference,
/// leaked on the same leak-only rule.
pub(super) fn emit_slice<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    base: Scalar<'ctx>,
    bounds: [Option<Scalar<'ctx>>; 3],
) -> Scalar<'ctx> {
    let edge = ForeignFailEdge::for_current(builder);
    let base_ptr = expect_object_pointer(base);
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let i32_type = context.i32_type();
    let mut present = 0u64;
    let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![base_ptr.into()];
    for (bit, bound) in bounds.into_iter().enumerate() {
        let pointer = match bound {
            Some(scalar) => {
                present |= 1 << bit;
                emit_pack(context, builder, module, scalar, "foreign_slice_bound")
            }
            None => ptr.const_null(),
        };
        args.push(pointer.into());
    }
    args.push(i32_type.const_int(present, false).into());
    let getslice = shim_fn(
        module,
        EXT_OBJ_GETSLICE_SYMBOL,
        ptr.fn_type(
            &[
                ptr.into(),
                ptr.into(),
                ptr.into(),
                ptr.into(),
                i32_type.into(),
            ],
            false,
        ),
    );
    let result = builder
        .build_call(getslice, &args, "foreign_slice")
        .expect("build_call should not fail for pycc_ext_obj_getslice")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_getslice returns PyObject *")
        .into_pointer_value();
    route_null(context, builder, module, rt, edge, result, "foreign_slice");
    Scalar::Object(result)
}

#[cfg(test)]
mod tests;
