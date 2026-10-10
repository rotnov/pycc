//! A branch condition: the `i1` an `if`, `while`, `assert`, conditional
//! expression or comprehension filter branches on, and the truth a `not` or
//! a truth-only `and`/`or` operand tests.
//!
//! Every such site evaluates its expression, tests the value's truth and
//! releases the value. A rich comparison with a CPython object operand
//! (`MirExpr::ObjCompare`) is the one expression this module emits
//! differently (#1518): its truth comes straight from
//! `foreign_compare::emit_compare_truth`, so no `bool` object is built,
//! tested and released (`docs/RUNTIME.md`'s "A comparison that only feeds a
//! branch").

use super::*;

/// Emits `test` and returns its truth as an `i1`.
///
/// A produced CPython object is held across its truth test, which can raise,
/// and released after it (Part 1 of #1092). An `int` temporary is released
/// after the test, which reads a bigint operand's limbs (#146 Part 2,
/// D-181).
pub(super) fn emit_condition<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    test: &MirExpr,
) -> IntValue<'ctx> {
    if let MirExpr::ObjCompare { op, left, right } = test
        && foreign_compare::is_rich_compare(*op)
    {
        return foreign_compare::emit_operands_then(
            context,
            builder,
            module,
            rt,
            user_functions,
            locals,
            left,
            right,
            |l, r| foreign_compare::emit_compare_truth(context, builder, module, rt, *op, l, r),
        );
    }
    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
    let held = object_release::hold(context, module, rt, test, &scalar);
    let cond = truthy(context, builder, module, rt, scalar);
    held.release(builder, rt);
    release_scalar_if_int_temporary(context, builder, rt, test, &scalar);
    cond
}
