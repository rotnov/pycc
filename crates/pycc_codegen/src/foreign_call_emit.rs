//! The MIR-level drivers for a call on a CPython object: the argument
//! evaluation every object call shares, and the whole emission of
//! `MirExpr::ObjKeywordCall` (Part 8 of #1371).
//!
//! `foreign_call.rs` owns the marshalling primitives and works on
//! already-evaluated [`Scalar`]s; this module is the layer above it that
//! recurses into the MIR through `emit_expr`, kept out of `lib.rs`, which
//! is far past the ~1,000-line threshold.
//!
//! **Evaluation order** is CPython's for every shape: the callee (for a
//! method call, its receiver and then the method lookup), then the
//! positional arguments left to right, then the keyword values left to
//! right. `o.missing(k=1 // 0)` therefore raises `AttributeError`, as the
//! positional `o.missing(1 // 0)` does (`foreign_call.rs`'s module doc).

use super::*;
use crate::foreign_attr::expect_object_pointer;
use pycc_mir::{ObjKeywordCall, Ty};

/// Evaluates each argument of a call on a CPython object, left to right,
/// into the [`Scalar`] `foreign_pack::emit_pack` marshals.
///
/// A `None`-typed argument (Part 8 of #1371) is still evaluated, for its
/// effects, and then replaced by CPython's own `None`
/// (`foreign_pack::none_pointer`), which packs as an `object`: pycc's
/// `None` placeholder has no `PyObject *` of its own.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_object_args<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    args: &[MirExpr],
) -> Vec<Scalar<'ctx>> {
    args.iter()
        .map(|arg| {
            let value = crate::object_unbox::emit_pack_operand(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                arg,
            );
            if arg.ty() == Ty::None {
                Scalar::Object(foreign_pack::none_pointer(context, builder, module))
            } else {
                value
            }
        })
        .collect()
}

/// Emits `o.method(x, key=v)` or `callee(x, key=v)` on a CPython object
/// (Part 8 of #1371), yielding the result as an opaque [`Scalar::Object`].
///
/// `call.call` is the positional half (`pycc_mir::ObjKeywordCall`'s doc):
/// an `ObjMethodCall` is emitted like `lib.rs`'s own arm -- receiver,
/// lookup, arguments -- and its bound method is consumed; an `ObjCall`'s
/// callee is consumed or borrowed by `foreign_call::callee_is_produced`,
/// exactly as for a positional call. The keyword values are evaluated after
/// every positional argument and handed to `foreign_call::emit_call_kw`
/// behind them.
pub(super) fn emit_keyword_call<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    call: &ObjKeywordCall,
) -> Scalar<'ctx> {
    let (consume, callable, positional) = match &call.call {
        MirExpr::ObjMethodCall {
            base, method, args, ..
        } => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            let bound =
                foreign_call::emit_lookup(context, builder, module, rt, base_scalar, method);
            (true, bound, args)
        }
        MirExpr::ObjCall { callee, args } => {
            let callee_scalar =
                emit_expr(context, builder, module, rt, user_functions, locals, callee);
            (
                foreign_call::callee_is_produced(callee),
                expect_object_pointer(callee_scalar),
                args,
            )
        }
        other => panic!(
            "pycc_codegen: internal error: a keyword call's positional half must be an \
             `ObjMethodCall` or an `ObjCall`, got {other:?}"
        ),
    };
    let mut args = emit_object_args(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        positional,
    );
    args.extend(emit_object_args(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        &call.values,
    ));
    foreign_call::emit_call_kw(
        context,
        builder,
        module,
        rt,
        consume,
        callable,
        &args,
        &call.names,
    )
}
