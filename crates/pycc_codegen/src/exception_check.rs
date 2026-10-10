//! The D-173 pending-exception check (#1518).
//!
//! Generated code asks whether a pycc exception is pending after every
//! operation that can raise one. The answer lives in the runtime's
//! per-thread state (`pycc_rt::exception`), which a dynamically loaded
//! artifact can reach only through a `__tls_get_addr` call, so every check
//! used to be a call to `pycc_rt_exception_active` plus that lookup. Each
//! function now asks the runtime for the address of its thread's flag once,
//! in its entry block, and every check in it is a byte load from that
//! address (`docs/RUNTIME.md`'s "The pending-exception check").
//!
//! A function invocation runs on one thread from entry to return, so the
//! address read on entry names the right flag at every check. The load
//! cannot be folded across a runtime or shim call: the address comes from an
//! external function, so LLVM must assume any other external call may write
//! through it.

use super::exception::jump_to_exception_target;
use super::*;

/// Emits a read of the pending-exception flag at the builder's position and
/// returns it as an `i8` that is non-zero exactly when an exception is
/// pending, as `pycc_rt_exception_active()` would.
///
/// The first read in a function also emits the one
/// `pycc_rt_exception_state()` call that every later read in that function
/// reuses, at the top of the function's entry block so that it dominates
/// every block the read can be emitted into.
pub(super) fn load_exception_active<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    name: &str,
) -> IntValue<'ctx> {
    let function = builder
        .get_insert_block()
        .and_then(|block| block.get_parent())
        .expect("the builder is positioned inside a function");
    let flag = *rt
        .exceptions
        .state_flags
        .borrow_mut()
        .entry(function)
        .or_insert_with(|| {
            let entry = function
                .get_first_basic_block()
                .expect("a function being emitted has an entry block");
            let entry_builder = context.create_builder();
            match entry.get_first_instruction() {
                Some(first) => entry_builder.position_before(&first),
                None => entry_builder.position_at_end(entry),
            }
            entry_builder
                .build_call(rt.exception_state, &[], "exc_state")
                .expect("build_call should not fail for exception_state")
                .try_as_basic_value()
                .expect_basic("pycc_rt_exception_state returns a pointer")
                .into_pointer_value()
        });
    builder
        .build_load(context.i8_type(), flag, name)
        .expect("build_load should not fail for the pending-exception flag")
        .into_int_value()
}

/// Stops expression/statement evaluation before another effect can be
/// committed when a Python exception is pending.
///
/// #638 (D-208): when `rt.exceptions.pending_int_releases` is non-empty, an
/// enclosing node (never this call site's own operand -- see each push
/// site's own comment) has already evaluated a fresh `Ty::Int` birth
/// reference that a sibling's evaluation, reached from here, might orphan by
/// raising before the enclosing node's own release call is textually
/// reached. In that case the exception edge is routed through an
/// intermediate `effect_exc_unwind` block that releases a *snapshot* of the
/// stack before branching to `exception_target`, instead of branching there
/// directly.
///
/// The `is_empty()` check below is a codegen-time (Rust-side) read of the
/// `RefCell`'s current length while walking the MIR tree -- it costs nothing
/// in the emitted binary either way, and it is what keeps this change from
/// affecting the D-084/D-140 nbody hot loop's own codegen: that loop's
/// arithmetic is entirely smallint, so it never pushes onto this stack, and
/// every guard site it reaches takes the empty-stack branch below, emitting
/// exactly the same two-block shape this function has always emitted. Do
/// not simplify this to unconditionally emit `effect_exc_unwind`: doing so
/// would add an extra empty block and an extra unconditional branch to
/// every guard site in the program, including every one in that hot loop,
/// which is precisely the throughput regression `bigint_rc.rs`'s own guard
/// design exists to avoid.
pub(super) fn guard_statement_effects<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
) {
    let exception_target = rt
        .exceptions
        .targets
        .borrow()
        .last()
        .copied()
        .expect("expression emission always has an installed exception target");
    let active = load_exception_active(context, builder, rt, "effect_exc_active");
    let has_exc = builder
        .build_int_compare(
            inkwell::IntPredicate::NE,
            active,
            context.i8_type().const_zero(),
            "effect_has_exc",
        )
        .expect("build_int_compare should not fail");
    let function = builder.get_insert_block().unwrap().get_parent().unwrap();
    let continuation = context.append_basic_block(function, "effect_exc_cont");
    // Part 1 of #1092: a held object temporary needs the unwind block
    // exactly as a held bigint does; both empty keeps the two-block shape.
    let nothing_pending = rt.exceptions.pending_int_releases.borrow().is_empty()
        && rt.exceptions.pending_object_releases.borrow().is_empty();
    if nothing_pending {
        builder
            .build_conditional_branch(has_exc, exception_target, continuation)
            .expect("build_conditional_branch should guard a statement effect");
        builder.position_at_end(continuation);
        return;
    }
    let unwind_bb = context.append_basic_block(function, "effect_exc_unwind");
    builder
        .build_conditional_branch(has_exc, unwind_bb, continuation)
        .expect("build_conditional_branch should guard a statement effect");
    builder.position_at_end(unwind_bb);
    jump_to_exception_target(context, builder, rt);
    builder.position_at_end(continuation);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `pycc_rt_exception_state()` call per function, at the top of its
    /// entry block whatever block the first check is emitted into, and every
    /// check in the function loads from that one address.
    #[test]
    fn each_function_looks_up_its_flag_address_once_in_its_entry_block() {
        let context = Context::create();
        let module = context.create_module("exception_check");
        let builder = context.create_builder();
        let rt = declare_rt_functions(&context, &module);
        let fn_type = context.void_type().fn_type(&[], false);

        // `f`'s entry block is still empty when its first check is emitted.
        let f = module.add_function("f", fn_type, None);
        let f_entry = context.append_basic_block(f, "entry");
        builder.position_at_end(f_entry);
        let first = load_exception_active(&context, &builder, &rt, "first");
        let later = context.append_basic_block(f, "later");
        builder.build_unconditional_branch(later).unwrap();
        builder.position_at_end(later);
        let second = load_exception_active(&context, &builder, &rt, "second");
        builder.build_return(None).unwrap();

        // `g`'s entry block already holds an instruction, and its first check
        // is emitted into a later block: the lookup goes in front of it.
        let g = module.add_function("g", fn_type, None);
        let g_entry = context.append_basic_block(g, "entry");
        builder.position_at_end(g_entry);
        builder.build_alloca(context.i8_type(), "slot").unwrap();
        let g_later = context.append_basic_block(g, "later");
        builder.build_unconditional_branch(g_later).unwrap();
        builder.position_at_end(g_later);
        load_exception_active(&context, &builder, &rt, "third");
        builder.build_return(None).unwrap();

        assert!(f.verify(true) && g.verify(true));
        let ir = crate::llvm_string_to_owned(module.print_to_string());
        assert_eq!(
            ir.matches("call ptr @pycc_rt_exception_state()").count(),
            2,
            "{ir}"
        );
        for name in ["f", "g"] {
            let body = &ir[ir.find(&format!("@{name}()")).unwrap()..];
            let entry = &body[body.find("entry:").unwrap()..];
            assert!(
                entry
                    .trim_start_matches("entry:")
                    .trim_start()
                    .starts_with("%exc_state"),
                "{ir}"
            );
        }
        assert!(ir.contains("%first = load i8, ptr %exc_state"), "{ir}");
        assert!(ir.contains("%second = load i8, ptr %exc_state"), "{ir}");
        assert!(ir.contains("%third = load i8, ptr %exc_state"), "{ir}");
        assert_eq!(first.get_type(), context.i8_type());
        assert_eq!(second.get_type(), context.i8_type());
    }
}
