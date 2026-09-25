//! The failure edge of a foreign-object operation (#1316).
//!
//! Every shim helper behind an `object` operation reports failure the
//! CPython way -- a `NULL` result or a negative status, with CPython's error
//! indicator set -- which pycc's own pending-exception guard (D-173) cannot
//! see. Each emitter therefore routes that failure itself, and where it goes
//! depends only on the function being emitted into:
//!
//! - **The module-exec entry** ([`EXT_MODULE_EXEC_SYMBOL`]) returns
//!   [`EXT_MODULE_EXEC_FAILED`] immediately, leaving CPython's exception set
//!   and unmodified. This is the edge every foreign operation took before
//!   #1316, emitted bit-identically: the host reports the real exception
//!   and the module body's remaining statements never run.
//! - **Any other function** -- an exported function, a private helper, a
//!   method -- has no such edge: its return type is its own. There the
//!   failure calls the shim's total [`EXT_OBJ_ERROR_BRIDGE_SYMBOL`], which
//!   turns CPython's exception into a pending pycc exception (keeping the
//!   original for the host, `docs/RUNTIME.md`), and then branches
//!   **immediately** to the innermost exception target through
//!   [`jump_to_exception_target`], releasing any live bigint temporaries on
//!   the way exactly as `guard_statement_effects` does.
//!
//! The branch is immediate rather than left to `emit_expr`'s post-node
//! guard for two reasons: a method call evaluates its arguments *between*
//! the method lookup and the call, with no guard in between, and the
//! condition-position truth test has no expression guard after it at all.

use super::exception::jump_to_exception_target;
use super::*;
use inkwell::builder::Builder;

/// Where a failed foreign operation goes, chosen once per operation from
/// the function `builder` is emitting into.
///
/// Both arms carry that function: the emitters append their fail and
/// continuation blocks to it, and hoist their out-slots and argument arrays
/// into its entry block, whichever edge they take.
#[derive(Clone, Copy)]
pub(super) enum ForeignFailEdge<'ctx> {
    /// Inside `pycc_ext_module_exec`: return [`EXT_MODULE_EXEC_FAILED`].
    ModuleExec(FunctionValue<'ctx>),
    /// Inside any other function: bridge, then branch to the innermost
    /// exception target.
    Function(FunctionValue<'ctx>),
}

impl<'ctx> ForeignFailEdge<'ctx> {
    /// The edge for the function `builder` is currently positioned in.
    pub(super) fn for_current(builder: &Builder<'ctx>) -> Self {
        let function = builder
            .get_insert_block()
            .expect("the builder is positioned inside a block")
            .get_parent()
            .expect("every basic block belongs to a function");
        if function.get_name().to_bytes() == EXT_MODULE_EXEC_SYMBOL.as_bytes() {
            Self::ModuleExec(function)
        } else {
            Self::Function(function)
        }
    }

    /// The function the edge's blocks and hoisted slots belong to.
    pub(super) fn function(self) -> FunctionValue<'ctx> {
        match self {
            Self::ModuleExec(function) | Self::Function(function) => function,
        }
    }
}

/// Declares `int pycc_ext_obj_error_bridge(void)` once per module.
fn obj_error_bridge_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_ERROR_BRIDGE_SYMBOL) {
        return existing;
    }
    module.add_function(
        EXT_OBJ_ERROR_BRIDGE_SYMBOL,
        context.i32_type().fn_type(&[], false),
        None,
    )
}

/// Emits the failure side of `edge` at the builder's current position,
/// which it terminates.
///
/// The bridge's `int` result is deliberately unread: the bridge is total
/// (`src/ext/pycc_ext_module.c`), so a pycc exception is pending on every
/// return and there is no unbridged fallback to branch to -- unlike the
/// import bridge's `foreign_import_unbridged` block, which a function body
/// could not take anyway.
pub(super) fn emit_failure<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    edge: ForeignFailEdge<'ctx>,
) {
    match edge {
        ForeignFailEdge::ModuleExec(_) => {
            builder
                .build_return(Some(
                    &context
                        .i64_type()
                        .const_int(EXT_MODULE_EXEC_FAILED as u64, true),
                ))
                .expect("build_return should not fail");
        }
        ForeignFailEdge::Function(_) => {
            builder
                .build_call(obj_error_bridge_fn(context, module), &[], "foreign_bridged")
                .expect("build_call should not fail for pycc_ext_obj_error_bridge");
            jump_to_exception_target(context, builder, rt);
        }
    }
}

/// Branches on `failed` to a `{label}_fail` block that takes `edge`, and
/// leaves the builder on the `{label}_cont` success continuation.
fn route<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    edge: ForeignFailEdge<'ctx>,
    failed: IntValue<'ctx>,
    label: &str,
) {
    let function = edge.function();
    let fail_bb = context.append_basic_block(function, &format!("{label}_fail"));
    let cont_bb = context.append_basic_block(function, &format!("{label}_cont"));
    builder
        .build_conditional_branch(failed, fail_bb, cont_bb)
        .expect("build_conditional_branch should not fail");
    builder.position_at_end(fail_bb);
    emit_failure(context, builder, module, rt, edge);
    builder.position_at_end(cont_bb);
}

/// Routes a `NULL` `value` -- a shim helper's `PyObject *` failure -- to
/// `edge`, leaving the builder on the success continuation.
pub(super) fn route_null<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    edge: ForeignFailEdge<'ctx>,
    value: PointerValue<'ctx>,
    label: &str,
) {
    let failed = builder
        .build_is_null(value, &format!("{label}_failed"))
        .expect("build_is_null should not fail");
    route(context, builder, module, rt, edge, failed, label);
}

/// Routes a negative `status` -- a shim helper's `int` failure -- to
/// `edge`, leaving the builder on the success continuation.
///
/// A *signed* comparison against zero rather than an equality test against
/// `-1`, so any future negative status is fail-closed; every helper's
/// success values are non-negative.
pub(super) fn route_negative<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    edge: ForeignFailEdge<'ctx>,
    status: IntValue<'ctx>,
    label: &str,
) {
    let failed = builder
        .build_int_compare(
            inkwell::IntPredicate::SLT,
            status,
            context.i32_type().const_zero(),
            &format!("{label}_failed"),
        )
        .expect("build_int_compare should not fail");
    route(context, builder, module, rt, edge, failed, label);
}

/// Declares `void pycc_ext_name_error(const unsigned char *, long long)`
/// once per module.
fn name_error_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_NAME_ERROR_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    module.add_function(
        EXT_NAME_ERROR_SYMBOL,
        context
            .void_type()
            .fn_type(&[ptr.into(), context.i64_type().into()], false),
        None,
    )
}

/// Emits the unbound branch of a module-global read of `name` when it is a
/// foreign `object` read inside a function other than the module-exec
/// entry, and answers whether it did; the caller emits its `llvm.trap`
/// otherwise.
///
/// A function body is checked against the final module environment, so
/// `def f(): return copy.__name__` called above `import copy` type-checks
/// (D-041 cannot prove call order). CPython raises `NameError: name 'copy'
/// is not defined` there, and so does this: the shim's
/// [`EXT_NAME_ERROR_SYMBOL`] sets that exception and bridges it, and the
/// branch to the innermost exception target is the one a failed foreign
/// operation takes. `name` is the *local* binding name, so a
/// `from copy import copy as c` reports `c`, exactly as CPython does.
///
/// A module-body read keeps the trap: D-041 proves every module-level read
/// follows its binding, so that branch is unreachable there.
pub(super) fn emit_unbound_object_read<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    ty: &pycc_mir::Ty,
    name: &str,
) -> bool {
    if *ty != pycc_mir::Ty::Object {
        return false;
    }
    let ForeignFailEdge::Function(_) = ForeignFailEdge::for_current(builder) else {
        return false;
    };
    let bytes = builder
        .build_global_string_ptr(name, &format!("pycc_foreign_name_{name}"))
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let len = context.i64_type().const_int(name.len() as u64, false);
    builder
        .build_call(
            name_error_fn(context, module),
            &[bytes.into(), len.into()],
            "",
        )
        .expect("build_call should not fail for pycc_ext_name_error");
    jump_to_exception_target(context, builder, rt);
    true
}

#[cfg(test)]
#[path = "foreign_fail_tests.rs"]
mod tests;
