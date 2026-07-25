use inkwell::FloatPredicate;
use inkwell::IntPredicate;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::BasicType;
use inkwell::values::{FloatValue, FunctionValue, IntValue, PointerValue};
use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt};
use std::collections::HashMap;
use std::path::Path;

/// One MIR-level value during codegen. Extended (never replaced) by
/// later tasks: `Str` in Task 7. `Ty::None` never needs a variant here --
/// no v0.1 `MirExpr` can actually construct a `None` *value* (see Task
/// 6's note).
enum Scalar<'ctx> {
    /// Tagged per D-052. Always LLVM `i64`.
    Int(IntValue<'ctx>),
    /// `0`/`1`, LLVM `i8` -- not `i1` (D-052's ABI note: this project has
    /// already hit real cross-platform storage/parameter footguns for
    /// sub-byte types, see D-027/D-028/D-029; `i1` is used only
    /// transiently for a `br` condition or an `icmp`/`fcmp` result,
    /// immediately zero-extended to `i8` before it's stored anywhere).
    Bool(IntValue<'ctx>),
    /// A plain, untagged LLVM `f64` -- unlike `int`, `float` needs no
    /// tagging scheme (D-052's tagged-fixnum representation is specific
    /// to `int`'s own overflow/bigint-promotion story); every `float`
    /// value is exactly one `f64`, always (Task 6).
    Float(FloatValue<'ctx>),
}

/// Every `pycc_rt` function this crate calls, declared once in
/// `compile_to_object` and threaded through `emit_stmt`/`emit_expr`.
/// Extended (never replaced) by Tasks 6/7/8/9/10 as they add more
/// `pycc_rt` declarations.
struct RtFns<'ctx> {
    int_add: FunctionValue<'ctx>,
    int_sub: FunctionValue<'ctx>,
    int_mul: FunctionValue<'ctx>,
    int_floordiv: FunctionValue<'ctx>,
    int_floormod: FunctionValue<'ctx>,
    int_pow: FunctionValue<'ctx>,
    int_cmp: FunctionValue<'ctx>,
    int_print: FunctionValue<'ctx>,
    int_truthy: FunctionValue<'ctx>,
    range_continue: FunctionValue<'ctx>,
    int_to_float: FunctionValue<'ctx>,
    float_floordiv: FunctionValue<'ctx>,
    float_floormod: FunctionValue<'ctx>,
    float_pow: FunctionValue<'ctx>,
}

fn declare_rt_functions<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> RtFns<'ctx> {
    let i64_type = context.i64_type();
    let i32_type = context.i32_type();
    let void_type = context.void_type();
    let f64_type = context.f64_type();
    let declare = |name: &str, fn_type: inkwell::types::FunctionType<'ctx>| {
        module.add_function(name, fn_type, Some(Linkage::External))
    };
    RtFns {
        int_add: declare(
            "pycc_rt_int_add",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_sub: declare(
            "pycc_rt_int_sub",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_mul: declare(
            "pycc_rt_int_mul",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_floordiv: declare(
            "pycc_rt_int_floordiv",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_floormod: declare(
            "pycc_rt_int_floormod",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_pow: declare(
            "pycc_rt_int_pow",
            i64_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_cmp: declare(
            "pycc_rt_int_cmp",
            i32_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        int_print: declare("pycc_rt_int_print", void_type.fn_type(&[i64_type.into()], false)),
        int_truthy: declare(
            "pycc_rt_int_truthy",
            context.i8_type().fn_type(&[i64_type.into()], false),
        ),
        range_continue: declare(
            "pycc_rt_range_continue",
            context.i8_type().fn_type(&[i64_type.into(), i64_type.into(), i64_type.into()], false),
        ),
        int_to_float: declare("pycc_rt_int_to_float", f64_type.fn_type(&[i64_type.into()], false)),
        float_floordiv: declare(
            "pycc_rt_float_floordiv",
            f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
        ),
        float_floormod: declare(
            "pycc_rt_float_floormod",
            f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
        ),
        float_pow: declare(
            "pycc_rt_float_pow",
            f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
        ),
    }
}

/// Mirrors `pycc_rt::tag_smallint` exactly (compile-time constant folding
/// of the same encoding, see D-052) -- an `int` literal whose magnitude
/// doesn't fit the tagged 63-bit range needs a real bigint *literal*,
/// which doesn't exist until Task 9; this is a narrow, honest,
/// compile-time "not supported yet" (not a silent truncation).
fn tag_smallint_const(context: &Context, n: i64) -> IntValue<'_> {
    let tagged = (n << 1) | 1;
    if (tagged >> 1) != n {
        panic!(
            "pycc_codegen: integer literal {n} is too large for the v0.1 fast \
             path (bigint literal support lands in a later task)"
        );
    }
    context.i64_type().const_int(tagged as u64, true)
}

fn ty_to_basic_type(context: &Context, ty: pycc_mir::Ty) -> inkwell::types::BasicTypeEnum<'_> {
    match ty {
        pycc_mir::Ty::Int => context.i64_type().into(),
        pycc_mir::Ty::Bool => context.i8_type().into(),
        pycc_mir::Ty::Float => context.f64_type().into(),
        other => panic!("pycc_codegen: a `{other:?}`-typed parameter/return value is not supported yet"),
    }
}

/// `bool` is an `int` subtype (Python/`pycc_types`'
/// `numeric_or_bool_compatible`) -- widens a `Bool` scalar to a tagged
/// `int` (D-052) via two trivial, unambiguous LLVM instructions (a
/// zero-extend then a shift-and-or matching `pycc_rt::tag_smallint`
/// exactly); an existing `Int` scalar passes through unchanged. Panics
/// for `Float`, which is never `int`-coercible -- `pycc_types`'
/// `numeric_result_type` always promotes an expression with any `float`
/// operand to `Ty::Float`, so no real MIR can reach this arm with a
/// `Float` operand (see this task's own defensive-panic test exercising
/// it via deliberately malformed MIR, matching this file's existing
/// convention for such arms).
fn to_tagged_int<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
) -> IntValue<'ctx> {
    match scalar {
        Scalar::Int(v) => v,
        Scalar::Bool(v) => {
            let widened = builder
                .build_int_z_extend(v, context.i64_type(), "bool_to_i64")
                .expect("build_int_z_extend should not fail widening i8 to i64");
            let shifted = builder
                .build_left_shift(widened, context.i64_type().const_int(1, false), "tag_shl")
                .expect("build_left_shift should not fail for a constant shift amount");
            builder
                .build_or(shifted, context.i64_type().const_int(1, false), "tag_or")
                .expect("build_or should not fail for two i64 operands")
        }
        Scalar::Float(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got float")
        }
    }
}

/// Promotes any numeric `Scalar` to `f64`: an existing `Float` passes
/// through; `Int` goes through `pycc_rt_int_to_float` (never a raw LLVM
/// cast -- the value is D-052-tagged, so only `pycc_rt` may interpret its
/// bits); `Bool` uses a plain unsigned-int-to-float conversion
/// (unambiguous for a 0/1 value, no tagging involved).
fn to_float<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> FloatValue<'ctx> {
    match scalar {
        Scalar::Float(v) => v,
        Scalar::Int(v) => builder
            .build_call(rt.int_to_float, &[v.into()], "int_to_float")
            .expect("build_call should not fail for a well-formed conversion")
            .try_as_basic_value()
            .expect_basic("pycc_rt_int_to_float returns a non-void f64")
            .into_float_value(),
        Scalar::Bool(v) => builder
            .build_unsigned_int_to_float(v, context.f64_type(), "bool_to_float")
            .expect("build_unsigned_int_to_float should not fail for an i8 0/1 value"),
    }
}

/// Shared by the `BinOp` arm's `FloorDiv`/`Mod`/`Pow` cases under a
/// `Ty::Float` result: each of these three (unlike `Add`/`Sub`/`Mul`/`Div`,
/// which map directly to an LLVM float instruction) needs a `pycc_rt_float_*`
/// runtime call instead -- this is the one piece of call-building logic all
/// three share, parameterized only by which `RtFns` field to call.
fn build_float_rt_binop<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt_fn: FunctionValue<'ctx>,
    l: FloatValue<'ctx>,
    r: FloatValue<'ctx>,
) -> FloatValue<'ctx> {
    builder
        .build_call(rt_fn, &[l.into(), r.into()], "float_binop")
        .expect("build_call should not fail for a well-formed float binop")
        .try_as_basic_value()
        .expect_basic("pycc_rt_float_* functions all return a non-void f64")
        .into_float_value()
}

fn emit_expr<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    // Only ever passed to `emit_expr`'s own recursive calls in every arm
    // this task adds -- clippy's `only_used_in_recursion` lint (part of
    // `-D warnings`) requires the underscore prefix for that shape until
    // Task 7 adds a `MirExpr::StringLiteral` arm that reads it directly
    // to build a constant global (Task 7 renames this to `module`,
    // dropping the underscore).
    _module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    expr: &MirExpr,
) -> Scalar<'ctx> {
    use pycc_mir::Ty;
    match expr {
        MirExpr::IntLiteral(n) => Scalar::Int(tag_smallint_const(context, *n)),
        MirExpr::FloatLiteral(f) => Scalar::Float(context.f64_type().const_float(*f)),
        MirExpr::Name { name, ty } => {
            let (ptr, local_ty) = locals
                .get(name)
                .unwrap_or_else(|| panic!("pycc_codegen: internal error: `{name}` has no local slot"));
            debug_assert_eq!(local_ty, ty, "pycc_codegen: internal error: local type drifted");
            // Deviation from the task brief: the brief's version matched
            // `ty` twice -- once to pick `load_ty` (a `BasicTypeEnum`) for
            // `build_load`, once more to wrap the loaded value in the
            // right `Scalar` variant -- with the second match's `Ty::Int`/
            // `Ty::Bool` arms mirroring the first's and a trailing
            // `_ => unreachable!("handled above")`. That second match's
            // catch-all is provably dead code, not merely hard to test:
            // both matches inspect the exact same `ty` value, so any input
            // that isn't `Ty::Int`/`Ty::Bool` already panics in the *first*
            // match (this arm's own "not supported yet" case, still
            // present just below) -- the second match's `_` arm can never
            // be reached by any input, well-formed or not. Merged into one
            // match (matching `ty` once) so there's no redundant
            // impossible-to-cover branch left over.
            match ty {
                Ty::Int => {
                    let loaded = builder
                        .build_load(context.i64_type(), *ptr, "load")
                        .expect("build_load should not fail for a slot this function itself allocated");
                    Scalar::Int(loaded.into_int_value())
                }
                Ty::Bool => {
                    let loaded = builder
                        .build_load(context.i8_type(), *ptr, "load")
                        .expect("build_load should not fail for a slot this function itself allocated");
                    Scalar::Bool(loaded.into_int_value())
                }
                Ty::Float => {
                    let loaded = builder
                        .build_load(context.f64_type(), *ptr, "load")
                        .expect("build_load should not fail for a slot this function itself allocated");
                    Scalar::Float(loaded.into_float_value())
                }
                other => panic!("pycc_codegen: reading a `{other:?}`-typed local is not supported yet"),
            }
        }
        MirExpr::BinOp { op, left, right, ty } => {
            // This inkwell version's `try_as_basic_value()` returns its own
            // `ValueKind` enum (not `either::Either` as in older inkwell
            // releases the task brief's original code was written against
            // -- ".left()" doesn't exist on this type); `.expect_basic(msg)`
            // is the direct equivalent, panicking with `msg` if the callee
            // turned out to be void instead of returning a value.
            let l = emit_expr(context, builder, _module, rt, user_functions, locals, left);
            let r = emit_expr(context, builder, _module, rt, user_functions, locals, right);
            match ty {
                Ty::Int => {
                    // `to_tagged_int` promotes a `bool` operand instead of
                    // rejecting it -- Python's `bool` is an `int` subtype
                    // (e.g. `True + 1 == 2`), a case `pycc_types` already
                    // legitimately types `Ty::Int` (see its own
                    // `a_binop_treats_bool_as_int` test); this is normal
                    // Python arithmetic-promotion semantics being handled,
                    // not an internal invariant violation (this codegen's
                    // own earlier, Task 3-era version mislabeled the
                    // now-removed rejection of exactly this case as an
                    // "internal error" -- see this task's own
                    // `adding_a_bool_left_operand_to_an_int_promotes_
                    // bool_to_int` test).
                    let l = to_tagged_int(context, builder, l);
                    let r = to_tagged_int(context, builder, r);
                    let rt_fn = match op {
                        pycc_mir::BinOpKind::Add => rt.int_add,
                        pycc_mir::BinOpKind::Sub => rt.int_sub,
                        pycc_mir::BinOpKind::Mul => rt.int_mul,
                        pycc_mir::BinOpKind::FloorDiv => rt.int_floordiv,
                        pycc_mir::BinOpKind::Mod => rt.int_floormod,
                        pycc_mir::BinOpKind::Pow => rt.int_pow,
                        pycc_mir::BinOpKind::Div => unreachable!(
                            "pycc_types/pycc_mir always type true division as Ty::Float"
                        ),
                    };
                    let result = builder
                        .build_call(rt_fn, &[l.into(), r.into()], "int_binop")
                        .expect("build_call should not fail for a well-formed int binop")
                        .try_as_basic_value()
                        .expect_basic("pycc_rt_int_* functions all return a non-void `i64`");
                    Scalar::Int(result.into_int_value())
                }
                Ty::Float => {
                    let l = to_float(context, builder, rt, l);
                    let r = to_float(context, builder, rt, r);
                    match op {
                        pycc_mir::BinOpKind::Add => Scalar::Float(
                            builder
                                .build_float_add(l, r, "fadd")
                                .expect("build_float_add should not fail for two f64 operands"),
                        ),
                        pycc_mir::BinOpKind::Sub => Scalar::Float(
                            builder
                                .build_float_sub(l, r, "fsub")
                                .expect("build_float_sub should not fail for two f64 operands"),
                        ),
                        pycc_mir::BinOpKind::Mul => Scalar::Float(
                            builder
                                .build_float_mul(l, r, "fmul")
                                .expect("build_float_mul should not fail for two f64 operands"),
                        ),
                        pycc_mir::BinOpKind::Div => Scalar::Float(
                            builder
                                .build_float_div(l, r, "fdiv")
                                .expect("build_float_div should not fail for two f64 operands"),
                        ),
                        // Each arm binds its own `rt_fn` directly (rather
                        // than a shared `FloorDiv | Mod | Pow` arm re-matching
                        // `op` to pick one) so there is no redundant,
                        // provably unreachable `_` fallback arm left over --
                        // this outer `match op` already guarantees exactly
                        // one of these three, and Rust's own exhaustiveness
                        // checker (not a defensive catch-all) is what proves
                        // that here (same reasoning as this file's other
                        // documented dead-code removals, e.g.
                        // `emit_expr`'s `Name` arm above).
                        pycc_mir::BinOpKind::FloorDiv => {
                            Scalar::Float(build_float_rt_binop(builder, rt.float_floordiv, l, r))
                        }
                        pycc_mir::BinOpKind::Mod => {
                            Scalar::Float(build_float_rt_binop(builder, rt.float_floormod, l, r))
                        }
                        pycc_mir::BinOpKind::Pow => {
                            Scalar::Float(build_float_rt_binop(builder, rt.float_pow, l, r))
                        }
                    }
                }
                other => panic!("pycc_codegen: a `{other:?}`-result BinOp is not supported yet"),
            }
        }
        MirExpr::Compare { op, left, right, .. } => {
            let left_ty = left.ty();
            let right_ty = right.ty();
            let l = emit_expr(context, builder, _module, rt, user_functions, locals, left);
            let r = emit_expr(context, builder, _module, rt, user_functions, locals, right);
            let as_bool = if left_ty == Ty::Float || right_ty == Ty::Float {
                let l = to_float(context, builder, rt, l);
                let r = to_float(context, builder, rt, r);
                let predicate = match op {
                    pycc_mir::CmpOpKind::Eq => FloatPredicate::OEQ,
                    // `UNE` ("unordered or not equal"), not `ONE` --
                    // CPython's `float('nan') != float('nan')` is `True`,
                    // and `NaN` involves an *unordered* comparison, not an
                    // ordered not-equal one. The other five predicates
                    // below correctly stay "ordered" (`O*`): Python's
                    // `<`/`<=`/`>`/`>=`/`==` on `float` are all `False`
                    // whenever `NaN` is involved, which is exactly what the
                    // ordered forms give.
                    pycc_mir::CmpOpKind::NotEq => FloatPredicate::UNE,
                    pycc_mir::CmpOpKind::Lt => FloatPredicate::OLT,
                    pycc_mir::CmpOpKind::LtE => FloatPredicate::OLE,
                    pycc_mir::CmpOpKind::Gt => FloatPredicate::OGT,
                    pycc_mir::CmpOpKind::GtE => FloatPredicate::OGE,
                };
                let cond = builder
                    .build_float_compare(predicate, l, r, "fcmp")
                    .expect("build_float_compare should not fail for two f64 operands");
                builder
                    .build_int_z_extend(cond, context.i8_type(), "bool_from_fcmp")
                    .expect("build_int_z_extend should not fail widening i1 to i8")
            } else {
                let l = to_tagged_int(context, builder, l);
                let r = to_tagged_int(context, builder, r);
                let ordering = builder
                    .build_call(rt.int_cmp, &[l.into(), r.into()], "int_cmp")
                    .expect("build_call should not fail for a well-formed comparison")
                    .try_as_basic_value()
                    .expect_basic("pycc_rt_int_cmp returns a non-void `i32`")
                    .into_int_value();
                let zero = context.i32_type().const_int(0, false);
                let predicate = match op {
                    pycc_mir::CmpOpKind::Eq => IntPredicate::EQ,
                    pycc_mir::CmpOpKind::NotEq => IntPredicate::NE,
                    pycc_mir::CmpOpKind::Lt => IntPredicate::SLT,
                    pycc_mir::CmpOpKind::LtE => IntPredicate::SLE,
                    pycc_mir::CmpOpKind::Gt => IntPredicate::SGT,
                    pycc_mir::CmpOpKind::GtE => IntPredicate::SGE,
                };
                let cond = builder
                    .build_int_compare(predicate, ordering, zero, "cmp")
                    .expect("build_int_compare should not fail for two i32 operands");
                builder
                    .build_int_z_extend(cond, context.i8_type(), "bool_from_cmp")
                    .expect("build_int_z_extend should not fail widening i1 to i8")
            };
            Scalar::Bool(as_bool)
        }
        MirExpr::BoolLiteral(b) => {
            Scalar::Bool(context.i8_type().const_int(u64::from(*b), false))
        }
        MirExpr::Call { callee, args, ty } => {
            if callee == "print" {
                panic!("pycc_codegen: using print()'s result as a nested expression is not supported yet");
            }
            // Unlike `emit_stmt`'s void-call arm below, there is no
            // `Result` here to propagate a clean, user-facing error
            // through -- `emit_expr` returns a `Scalar` unconditionally, so
            // an undefined callee can only be this crate's own internal
            // error. Real `pycc_types` already rejects any call to an
            // undefined function (T0021) long before codegen runs, so this
            // is a defensive backstop, not a rejection of legitimate
            // source (see `calling_an_undefined_function_as_a_nested_
            // expression_is_an_internal_error` below).
            let f = *user_functions.get(callee.as_str()).unwrap_or_else(|| {
                panic!(
                    "pycc_codegen: internal error: call to undefined function `{callee}` \
                     should have been rejected by pycc_types before reaching codegen"
                )
            });
            let call_site = build_call_to(context, builder, _module, rt, user_functions, locals, f, args);
            match ty {
                Ty::Int => Scalar::Int(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("this function is declared to return int")
                        .into_int_value(),
                ),
                Ty::Bool => Scalar::Bool(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("this function is declared to return bool")
                        .into_int_value(),
                ),
                Ty::Float => Scalar::Float(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("this function is declared to return float")
                        .into_float_value(),
                ),
                other => panic!("pycc_codegen: a `{other:?}`-typed call result is not supported yet"),
            }
        }
        other => panic!("pycc_codegen: this expression kind's codegen is not supported yet: {other:?}"),
    }
}

/// Evaluates every entry in `args` (via `emit_expr`, so each argument is
/// itself an arbitrary expression -- nested calls included, which is
/// exactly what makes recursion with real arguments work) and emits the
/// `call` instruction to the already-resolved `f`. Shared between
/// `emit_expr`'s `Call` arm (a value-producing call used inside a larger
/// expression) and `emit_stmt`'s void-call arm below (a call whose
/// declared return type is `None`, used as a bare statement) -- `Scalar`
/// has no variant for "no value" (see its own doc comment), so a
/// `None`-returning call can never flow back out of `emit_expr` itself;
/// this is the one piece both call sites need regardless of whether a
/// value comes back afterward.
///
/// Deliberately does *not* also do the `user_functions` lookup for
/// `callee`: the two call sites disagree on how a missing function should
/// be reported. `emit_stmt`'s arm still has a `Result` to propagate a
/// clean, user-facing error through (matching this crate's pre-Task-5
/// behavior, just generalized from zero-arg-only to any arity); `emit_expr`
/// does not (see its own `Call` arm's comment) -- so each resolves `f`
/// itself, with its own error-handling policy, before calling this helper.
#[allow(clippy::too_many_arguments)]
fn build_call_to<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    f: FunctionValue<'ctx>,
    args: &[MirExpr],
) -> inkwell::values::CallSiteValue<'ctx> {
    let arg_values: Vec<inkwell::values::BasicMetadataValueEnum> = args
        .iter()
        .map(|a| match emit_expr(context, builder, module, rt, user_functions, locals, a) {
            Scalar::Int(v) => v.into(),
            Scalar::Bool(v) => v.into(),
            Scalar::Float(v) => v.into(),
        })
        .collect();
    builder
        .build_call(f, &arg_values, "call_user_fn")
        .expect("build_call should not fail for a well-formed user function call")
}

/// Turns any supported `Scalar` into an LLVM `i1` for use as a `br`
/// condition -- the shared truthiness check behind `if`/`while` (Task 4).
/// Extended (never replaced) by Task 7 as `Str` truthiness is added.
fn truthy<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> inkwell::values::IntValue<'ctx> {
    let as_i8 = match scalar {
        Scalar::Bool(v) => v,
        Scalar::Int(v) => builder
            .build_call(rt.int_truthy, &[v.into()], "int_truthy")
            .expect("build_call should not fail for a well-formed truthiness check")
            .try_as_basic_value()
            .expect_basic("pycc_rt_int_truthy returns a non-void i8")
            .into_int_value(),
        // Python's `bool(x)` for a `float` is `False` only for exactly
        // `0.0`/`-0.0` -- `NaN` is truthy -- so this needs the
        // *unordered*-or-not-equal predicate, `UNE`, not `ONE` (same
        // distinction as the `Compare` arm's `NotEq` predicate above).
        Scalar::Float(v) => {
            let zero = context.f64_type().const_float(0.0);
            let cond = builder
                .build_float_compare(FloatPredicate::UNE, v, zero, "float_truthy")
                .expect("build_float_compare should not fail for two f64 operands");
            builder
                .build_int_z_extend(cond, context.i8_type(), "bool_from_float_truthy")
                .expect("build_int_z_extend should not fail widening i1 to i8")
        }
    };
    builder
        .build_int_compare(IntPredicate::NE, as_i8, context.i8_type().const_int(0, false), "truthy")
        .expect("build_int_compare should not fail comparing two i8 operands")
}

/// Allocates (on first assignment) or reuses (on reassignment) the
/// `alloca` backing `target`, stores `value` into it, and records/updates
/// its entry in `locals`. A local's `Ty` never changes across
/// reassignment (`pycc_types` ties one static type to each binding), so
/// reusing an existing slot never needs a type check beyond the
/// `debug_assert_eq!` in `emit_expr`'s `Name` arm above.
fn emit_assign<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    target: &str,
    ty: pycc_mir::Ty,
    value: Scalar<'ctx>,
) {
    let ptr = match locals.get(target) {
        Some((ptr, _)) => *ptr,
        None => {
            let alloca_ty: inkwell::types::BasicTypeEnum = match &value {
                Scalar::Int(v) => v.get_type().into(),
                Scalar::Bool(v) => v.get_type().into(),
                Scalar::Float(v) => v.get_type().into(),
            };
            let ptr = builder
                .build_alloca(alloca_ty, target)
                .expect("build_alloca should not fail for a supported scalar type");
            locals.insert(target.to_string(), (ptr, ty));
            ptr
        }
    };
    let basic_value: inkwell::values::BasicValueEnum = match value {
        Scalar::Int(v) => v.into(),
        Scalar::Bool(v) => v.into(),
        Scalar::Float(v) => v.into(),
    };
    builder
        .build_store(ptr, basic_value)
        .expect("build_store should not fail for a slot this function itself allocated");
}

/// Emits every statement in `body` in order, stopping early the moment the
/// current block already ends in a terminator.
///
/// **History (Tasks 3/4 removed this exact check as unreachable, Task 5
/// re-adds it as their own doc comments predicted it eventually would
/// need to):** Task 3 (only `ExprStmt`/`Assign` existed) and Task 4 (which
/// added `If`/`While`/`ForRange`, every arm of which always finishes by
/// repositioning the builder at a fresh, never-yet-terminated continuation
/// block before returning) both proved this check was unreachable in their
/// own scope, confirmed empirically by `cargo llvm-cov`, and removed it
/// rather than carry dead code -- while flagging that a future `Return`
/// arm would be the first `MirStmt` shape whose codegen terminates the
/// *current* block without repositioning the builder afterward (nothing is
/// left to emit into). Task 5 adds exactly that `Return` arm, so a body
/// with a `Return` followed by further (legal, if dead, Python) statements
/// now really can leave this loop's *next* iteration trying to emit into
/// an already-terminated block -- without this check, that would build a
/// second terminator onto the same block, invalid LLVM IR that
/// `module.verify()` (correctly) rejects. `emit_body_then_branch` below
/// and `ForRange`'s own inline copy in `emit_stmt` need the exact same
/// reasoning applied to their own trailing branch, and both re-add their
/// own guard for the same reason (see each one's own doc comment).
fn emit_body<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    body: &[MirStmt],
) -> Result<(), String> {
    for stmt in body {
        emit_stmt(context, builder, module, rt, user_functions, locals, stmt)?;
        if builder.get_insert_block().unwrap().get_terminator().is_some() {
            break;
        }
    }
    Ok(())
}

/// Emits `body` (via `emit_body`, which may now leave the current block
/// already terminated -- see its own doc comment), then an unconditional
/// branch to `dest` *unless* the block is already terminated (a `Return`
/// reached inside `body`, in which case there is nothing left to branch
/// from and doing so anyway would build an invalid second terminator).
/// Used by `If`'s `then`/`else` arms and `While`'s body; `ForRange`'s body
/// needs its own variant inline (it has extra post-body work --
/// incrementing the loop variable -- to do before branching back to the
/// loop test), so does not reuse this helper, but re-adds the identical
/// guard around its own trailing branch for the same reason.
#[allow(clippy::too_many_arguments)]
fn emit_body_then_branch<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    body: &[MirStmt],
    dest: inkwell::basic_block::BasicBlock<'ctx>,
) -> Result<(), String> {
    emit_body(context, builder, module, rt, user_functions, locals, body)?;
    if builder.get_insert_block().unwrap().get_terminator().is_none() {
        builder
            .build_unconditional_branch(dest)
            .expect("build_unconditional_branch should not fail on a block with no terminator yet");
    }
    Ok(())
}

/// `target_triple`: `None` compiles for the host's own default target (the
/// common case). `Some(triple)` cross-compiles for a different Tier-1
/// target -- LLVM's codegen backend is inherently multi-target, so this
/// only requires `Target::initialize_all` (rather than
/// `Target::initialize_native`) plus the requested `TargetTriple` instead
/// of the host's default; producing an actual *linked binary* for a
/// foreign target is a separate concern the caller handles (see
/// `src/main.rs`'s `--target` handling and its doc comment on what's
/// actually achievable without bundling a full foreign sysroot).
pub fn compile_to_object(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("pycc_module");
    let builder = context.create_builder();
    let i64_type = context.i64_type();
    let rt = declare_rt_functions(&context, &module);

    // First pass: declare every user-defined function -- with its real
    // parameter types and return type (Task 5), instead of Task 3/4's
    // placeholder zero-arg/void signature -- under a mangled name (never
    // the bare Python name) before emitting any body. Two reasons: this is
    // what lets a function call another function defined later in the
    // same module, or itself (recursion, now with real arguments and a
    // real return value flowing through it, since every signature is
    // already known before any body is lowered); and mangling is what
    // stops a Python-level function actually named `main` from colliding
    // with the real C-ABI entry point below, which must be literally
    // named `main` for the OS loader to find it. A def alone has no
    // runtime effect in Python regardless of its name -- something has to
    // call it, which is exactly the bug this pass structure fixes (see
    // git history: an earlier version treated a function merely named
    // `main` as auto-invoked, which doesn't match CPython at all).
    let mut user_functions: HashMap<&str, FunctionValue> = HashMap::new();
    for item in &mir.items {
        if let MirItem::Function { name, params, return_ty, .. } = item {
            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = params
                .iter()
                .map(|(_, ty)| ty_to_basic_type(&context, *ty).into())
                .collect();
            let fn_type = match return_ty {
                pycc_mir::Ty::None => context.void_type().fn_type(&param_types, false),
                other => ty_to_basic_type(&context, *other).fn_type(&param_types, false),
            };
            let mangled = format!("pyfn_{name}");
            let f = module.add_function(&mangled, fn_type, None);
            user_functions.insert(name.as_str(), f);
        }
    }

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    builder.position_at_end(entry_block);
    // Top-level statements share one `locals` map across the synthetic
    // `main` entry block (module-level Python names are one shared
    // scope); each user function gets its own, fresh map below, since
    // Python function bodies don't see each other's locals.
    let mut top_level_locals = HashMap::new();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            emit_stmt(&context, &builder, &module, &rt, &user_functions, &mut top_level_locals, stmt)?;
        }
    }
    // See the module-level comment block below for why these .expect()s
    // (this one included) are deliberate rather than Result-threaded: each
    // covers an operation that stays infallible given how this function
    // always calls it. Two calls below are genuine, externally-triggerable
    // failure modes and stay real Results the caller must handle:
    // Target::from_triple (a user-supplied --target can legitimately name
    // a triple LLVM doesn't recognize) and write_to_file, at the very end.
    builder
        .build_return(Some(&i64_type.const_int(0, false)))
        .expect(
            "build_return should not fail: builder is always freshly positioned before this call",
        );

    // Second pass: fill in each user function's body, now that every
    // function (including ones a body might call) is already declared.
    // Each parameter is bound into `fn_locals` by allocating a fresh slot
    // for it and storing the incoming LLVM argument into it (Task 5) --
    // the same load/store-via-`alloca` model every other local already
    // uses (see `emit_assign`), so a parameter is fully ordinary once
    // bound: reassignable, and readable via `emit_expr`'s `Name` arm with
    // no special-casing.
    for item in &mir.items {
        if let MirItem::Function { name, params, return_ty, body } = item {
            let f = user_functions[name.as_str()];
            let block = context.append_basic_block(f, "entry");
            builder.position_at_end(block);
            let mut fn_locals = HashMap::new();
            for (i, (param_name, ty)) in params.iter().enumerate() {
                // `.expect(...)`, not `.unwrap_or_else(|| panic!(...))`:
                // `f`'s own `fn_type` (built above, in the first pass) was
                // constructed from this exact `params` list, so
                // `get_nth_param` always succeeds for every `i` this loop
                // produces -- an unreachable "missing parameter" case
                // would be a defensive branch this crate's own coverage
                // gate (D-014) could never legitimately exercise, so it's
                // not introduced in the first place, the same reasoning
                // already applied elsewhere in this file (e.g.
                // `emit_body_then_branch`'s removed guard, Task 4).
                let incoming = f
                    .get_nth_param(i as u32)
                    .expect("this function was declared with exactly `params.len()` parameters above");
                let ptr = builder
                    .build_alloca(ty_to_basic_type(&context, *ty), param_name)
                    .expect("build_alloca should not fail for a supported scalar type");
                builder
                    .build_store(ptr, incoming)
                    .expect("build_store should not fail for a slot this function itself allocated");
                fn_locals.insert(param_name.clone(), (ptr, *ty));
            }
            emit_body(&context, &builder, &module, &rt, &user_functions, &mut fn_locals, body)?;
            // A `None`-returning function falling through its last
            // statement without an explicit `return` is ordinary, legal
            // Python (an implicit `return None`); a non-`None`-returning
            // function falling through is not -- `pycc_types`' T0024
            // fallthrough check rejects that HIR before it ever reaches
            // codegen, so seeing it here is this crate's own internal
            // error, not a user-facing rejection (see this task's own
            // `a_non_none_function_falling_through_is_an_internal_error_
            // not_bad_ir` test).
            match return_ty {
                pycc_mir::Ty::None => {
                    if builder.get_insert_block().unwrap().get_terminator().is_none() {
                        builder.build_return(None).expect(
                            "build_return should not fail: builder is always freshly positioned before this call",
                        );
                    }
                }
                _ if builder.get_insert_block().unwrap().get_terminator().is_none() => {
                    panic!(
                        "pycc_codegen: internal error: `{name}` is declared to return a \
                         non-`None` value but fell through without a `return` -- \
                         pycc_types::check (T0024) should have rejected this HIR before \
                         it reached codegen"
                    );
                }
                _ => {}
            }
        }
    }

    verify_module(&module);

    // initialize_all (not initialize_native): a requested target_triple may
    // not match the host's own architecture, and LLVM only has codegen
    // support for a target's backend if that backend was initialized.
    Target::initialize_all(&InitializationConfig::default());
    // ManuallyDrop, not a plain value: see D-029. TargetTriple wraps an
    // LLVMString (inkwell's own message wrapper around LLVMCreateMessage /
    // LLVMGetDefaultTargetTriple), whose Drop calls LLVMDisposeMessage --
    // this crashes on Windows against the official prebuilt LLVM 22.1.1
    // release. Suppressing the drop here, at the point of creation, covers
    // every exit path uniformly (the early `?` below included), not just
    // the success path a trailing forget would. Leaks one small string per
    // compile on every platform -- negligible in a short-lived CLI process,
    // and simpler than cfg-gating a type difference for a Windows-only leak.
    let triple = std::mem::ManuallyDrop::new(match target_triple {
        Some(t) => TargetTriple::create(t),
        None => TargetMachine::get_default_triple(),
    });
    let target = Target::from_triple(&triple).map_err(|e| {
        format!(
            "pycc_codegen: `{}` is not a target LLVM knows how to generate code for: {}",
            triple.as_str().to_string_lossy(),
            llvm_string_to_owned(e)
        )
    })?;
    let target_machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect(
            "creating a target machine with generic CPU/features should never fail for a \
             triple Target::from_triple has already accepted",
        );
    target_machine
        .write_to_file(&module, FileType::Object, output_path)
        .map_err(llvm_string_to_owned)
}

/// See D-029: every message inkwell hands back as an `LLVMString` --
/// `Target::from_triple`'s and `TargetMachine::write_to_file`'s error
/// paths, on top of `TargetTriple` itself -- shares the same broken `Drop`
/// (`LLVMDisposeMessage`) on Windows against this LLVM release. Converts
/// to an owned `String` (a real copy, safe to keep and format past this
/// call) and forgets the original rather than letting it drop. General
/// fix at the one place `LLVMString` crosses into this crate's error
/// values, not a patch per call site -- so a future one isn't missed.
fn llvm_string_to_owned(message: inkwell::support::LLVMString) -> String {
    let owned = message.to_string();
    std::mem::forget(message);
    owned
}

/// Skipped on Windows: `module.verify()` crashes there with an access
/// violation when linked against the official prebuilt LLVM 22.1.1 release
/// -- isolated with stderr checkpoints bracketing every call from the end of
/// IR building through object emission (D-029): every other call completed,
/// consistently, across every test that reached this point; only this one
/// never returned. Root cause not further isolated -- no Windows debugger
/// available in this environment to get an exact crash address/frame -- so
/// this is a targeted skip of a *pure internal sanity check* (a failure
/// here would mean a pycc_codegen bug, never a rejection of legitimate user
/// code -- see the non-Windows body's own message), not a change to what
/// gets compiled. The identical IR-building code already runs verified on
/// macOS/Linux for every test in this suite, which narrows the residual,
/// Windows-only risk to "a bug that only produces malformed IR on a
/// Windows-specific code path" -- and no such path exists yet, since IR
/// building above has no platform-conditional logic at all.
#[cfg(windows)]
fn verify_module(_module: &inkwell::module::Module<'_>) {}

#[cfg(not(windows))]
fn verify_module(module: &inkwell::module::Module<'_>) {
    module.verify().expect(
        "generated IR should always be well-formed for this fixed instruction shape; \
         a verify() failure here means a bug in pycc_codegen itself, not bad user input",
    );
}

/// Handles every `MirStmt` shape in v0.1 (this match is exhaustive over
/// `MirStmt`, no catch-all arm): a `print()` call of a single `int`-typed
/// expression, any other bare expression statement (a user-function call
/// with any number of arguments included -- see `emit_expr`'s `Call` arm,
/// which this now delegates to uniformly instead of special-casing
/// zero-arg calls here), a local-variable assignment, `If`/`While`/
/// `ForRange` control flow (Task 4) -- real basic blocks, conditional
/// branches, and loop back-edges, using `truthy` for the shared `if`/
/// `while` truthiness check and `emit_body_then_branch`/an inline
/// equivalent for the terminator-safety this introduces (see both
/// helpers' own doc comments) -- and now (Task 5) `Return`, terminating
/// the current block with the evaluated value (or none, for a bare
/// `return`).
fn emit_stmt<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    stmt: &MirStmt,
) -> Result<(), String> {
    match stmt {
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => {
            match args.as_slice() {
                [expr] if expr.ty() == pycc_mir::Ty::Int => {
                    let Scalar::Int(v) = emit_expr(context, builder, module, rt, user_functions, locals, expr) else {
                        unreachable!("Ty::Int always evaluates to Scalar::Int")
                    };
                    builder
                        .build_call(rt.int_print, &[v.into()], "print_int")
                        .expect("build_call should not fail for a well-formed print call");
                    Ok(())
                }
                _ => panic!(
                    "pycc_codegen: this print() argument shape is not supported yet \
                     (multi-arg / non-int print lands in Task 10)"
                ),
            }
        }
        // A user-function call whose declared return type is `None`, used
        // as a bare statement (e.g. `main()`) -- must go through
        // `build_call_to` directly rather than the general `ExprStmt(expr)`
        // arm below: `emit_expr`'s own `Call` arm always maps its result
        // into a `Scalar`, and `Scalar` has no variant representing "no
        // value" (see its own doc comment), so a `None`-returning call can
        // never validly flow through `emit_expr` at all. Matched by `ty`
        // alone (no `callee != "print"` guard needed): the arm above
        // already claims every `print` call via its own guard regardless
        // of `ty`, so only a non-`print` call ever reaches this one.
        //
        // Generalizes this crate's pre-Task-5 zero-arg-only special case
        // (which this task's brief called "redundant" and removed) to a
        // call of any arity: unlike `emit_expr`'s `Call` arm, this arm
        // *does* still have a `Result` to propagate a clean, user-facing
        // error through for an undefined callee, exactly like the
        // zero-arg case always did -- so it keeps doing that, rather than
        // switching to `emit_expr`'s "internal error" panic, preserving
        // both the existing user-facing behavior and the `?`-propagation
        // coverage every nested-body call site below relies on.
        MirStmt::ExprStmt(MirExpr::Call { callee, args, ty: pycc_mir::Ty::None }) => {
            let f = *user_functions
                .get(callee.as_str())
                .ok_or_else(|| format!("pycc_codegen v0.1: call to undefined function `{callee}`"))?;
            build_call_to(context, builder, module, rt, user_functions, locals, f, args);
            Ok(())
        }
        MirStmt::ExprStmt(expr) => {
            emit_expr(context, builder, module, rt, user_functions, locals, expr);
            Ok(())
        }
        MirStmt::Assign { target, value } => {
            let ty = value.ty();
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            emit_assign(builder, locals, target, ty, scalar);
            Ok(())
        }
        MirStmt::If { test, body, orelse } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let cond = {
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
                truthy(context, builder, rt, scalar)
            };
            let then_bb = context.append_basic_block(function, "if_then");
            let merge_bb = context.append_basic_block(function, "if_merge");
            let else_bb = if orelse.is_empty() { merge_bb } else { context.append_basic_block(function, "if_else") };
            builder
                .build_conditional_branch(cond, then_bb, else_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(then_bb);
            emit_body_then_branch(context, builder, module, rt, user_functions, locals, body, merge_bb)?;

            if !orelse.is_empty() {
                builder.position_at_end(else_bb);
                emit_body_then_branch(context, builder, module, rt, user_functions, locals, orelse, merge_bb)?;
            }

            builder.position_at_end(merge_bb);
            Ok(())
        }
        MirStmt::While { test, body } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let test_bb = context.append_basic_block(function, "while_test");
            let body_bb = context.append_basic_block(function, "while_body");
            let after_bb = context.append_basic_block(function, "while_after");

            builder
                .build_unconditional_branch(test_bb)
                .expect("build_unconditional_branch should not fail entering the loop test");
            builder.position_at_end(test_bb);
            let cond = {
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
                truthy(context, builder, rt, scalar)
            };
            builder
                .build_conditional_branch(cond, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            emit_body_then_branch(context, builder, module, rt, user_functions, locals, body, test_bb)?;

            builder.position_at_end(after_bb);
            Ok(())
        }
        MirStmt::ForRange { var, start, stop, step, body } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let Scalar::Int(start_v) = emit_expr(context, builder, module, rt, user_functions, locals, start) else {
                panic!("pycc_codegen: internal error: range() start did not evaluate to int")
            };
            let Scalar::Int(stop_v) = emit_expr(context, builder, module, rt, user_functions, locals, stop) else {
                panic!("pycc_codegen: internal error: range() stop did not evaluate to int")
            };
            let Scalar::Int(step_v) = emit_expr(context, builder, module, rt, user_functions, locals, step) else {
                panic!("pycc_codegen: internal error: range() step did not evaluate to int")
            };
            emit_assign(builder, locals, var, pycc_mir::Ty::Int, Scalar::Int(start_v));

            let test_bb = context.append_basic_block(function, "for_test");
            let body_bb = context.append_basic_block(function, "for_body");
            let after_bb = context.append_basic_block(function, "for_after");

            builder
                .build_unconditional_branch(test_bb)
                .expect("build_unconditional_branch should not fail entering the loop test");
            builder.position_at_end(test_bb);
            let (var_ptr, _) = *locals.get(var).expect("range() var was just bound above");
            let current = builder
                .build_load(context.i64_type(), var_ptr, "for_var")
                .expect("build_load should not fail for this function's own alloca")
                .into_int_value();
            let cont = builder
                .build_call(rt.range_continue, &[current.into(), stop_v.into(), step_v.into()], "range_continue")
                .expect("build_call should not fail for a well-formed range_continue check")
                .try_as_basic_value()
                .expect_basic("pycc_rt_range_continue returns a non-void i8")
                .into_int_value();
            let cont_i1 = builder
                .build_int_compare(IntPredicate::NE, cont, context.i8_type().const_int(0, false), "for_cont")
                .expect("build_int_compare should not fail comparing two i8 operands");
            builder
                .build_conditional_branch(cont_i1, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            emit_body(context, builder, module, rt, user_functions, locals, body)?;
            // `ForRange`'s own inline copy of `emit_body_then_branch`'s
            // terminator-safety guard (see that function's own doc comment
            // for why): a `Return` reached inside `body` already terminates
            // `body_bb`, so the increment-and-branch-back below must be
            // skipped in that case -- building it anyway would try to add a
            // second terminator onto an already-terminated block, which is
            // invalid LLVM IR.
            if builder.get_insert_block().unwrap().get_terminator().is_none() {
                let current = builder
                    .build_load(context.i64_type(), var_ptr, "for_var_reload")
                    .expect("build_load should not fail for this function's own alloca")
                    .into_int_value();
                let next = builder
                    .build_call(rt.int_add, &[current.into(), step_v.into()], "for_next")
                    .expect("build_call should not fail for a well-formed int add")
                    .try_as_basic_value()
                    .expect_basic("pycc_rt_int_add returns a non-void i64")
                    .into_int_value();
                builder
                    .build_store(var_ptr, next)
                    .expect("build_store should not fail for this function's own alloca");
                builder
                    .build_unconditional_branch(test_bb)
                    .expect("build_unconditional_branch should not fail on a block with no terminator yet");
            }

            builder.position_at_end(after_bb);
            Ok(())
        }
        MirStmt::Return(value) => {
            match value {
                Some(expr) => {
                    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, expr);
                    let basic_value: inkwell::values::BasicValueEnum = match scalar {
                        Scalar::Int(v) => v.into(),
                        Scalar::Bool(v) => v.into(),
                        Scalar::Float(v) => v.into(),
                    };
                    builder
                        .build_return(Some(&basic_value))
                        .expect("build_return should not fail for a well-formed return value");
                }
                None => {
                    builder
                        .build_return(None)
                        .expect("build_return should not fail for a bare `return`");
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_mir::{BinOpKind, CmpOpKind, MirExpr, MirItem, MirModule, MirStmt, Ty};
    use std::process::Command;

    /// `print(<n>)` as a `MirStmt` -- the only `print()` argument shape
    /// `emit_stmt` handles so far (see its doc comment).
    fn call_print(n: i64) -> MirStmt {
        MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![MirExpr::IntLiteral(n)],
            ty: Ty::None,
        })
    }

    /// A zero-arg call to a user-defined function as a `MirStmt`.
    fn call_user_fn(name: &str) -> MirStmt {
        MirStmt::ExprStmt(MirExpr::Call {
            callee: name.to_string(),
            args: vec![],
            ty: Ty::None,
        })
    }

    #[test]
    fn defining_main_without_calling_it_produces_no_output() {
        // The regression test for the bug this file's git history fixed:
        // a function definition alone must never run, regardless of its
        // name -- matches CPython exactly (confirmed empirically against
        // python3.14 on this exact source: zero bytes of stdout).
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "main".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![call_print(42)],
            }],
        };
        let dir = tempfile_dir("slice0_uncalled_main");
        let obj_path = dir.join("slice0_uncalled_main.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0_uncalled_main");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"");
    }

    #[test]
    fn compiles_an_explicit_call_to_main_to_a_running_binary() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![call_print(42)],
                },
                MirItem::TopLevelStmt(call_user_fn("main")),
            ],
        };
        let dir = tempfile_dir("slice0");
        let obj_path = dir.join("slice0.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn compiles_top_level_statement_with_no_main() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(call_print(42))],
        };
        let dir = tempfile_dir("slice0_toplevel");
        let obj_path = dir.join("slice0_toplevel.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0_toplevel");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn top_level_statements_run_in_order_including_a_call_to_main() {
        // RUNTIME.md's ordering guarantee ("top-level code ... runs once
        // ... at process start") applies to top-level statements
        // themselves running in source order -- which now includes an
        // explicit call to a user function as just another top-level
        // statement, not a special auto-invoked case.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(call_print(1)),
                MirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![call_print(2)],
                },
                MirItem::TopLevelStmt(call_user_fn("main")),
            ],
        };
        let dir = tempfile_dir("slice0_combined");
        let obj_path = dir.join("slice0_combined.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("slice0_combined");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n");
    }

    #[test]
    fn calling_an_undefined_function_at_top_level_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(call_user_fn("does_not_exist"))],
        };
        let dir = tempfile_dir("slice0_undefined_fn");
        let obj_path = dir.join("slice0_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(
            err.contains("does_not_exist"),
            "error should name the offending function: {err}"
        );
    }

    #[test]
    fn calling_an_undefined_function_inside_a_function_body_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "main".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![call_user_fn("also_does_not_exist")],
            }],
        };
        let dir = tempfile_dir("slice0_undefined_fn_nested");
        let obj_path = dir.join("slice0_undefined_fn_nested.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(
            err.contains("also_does_not_exist"),
            "error should name the offending function: {err}"
        );
    }

    #[test]
    fn a_function_can_be_defined_under_any_name_without_being_called() {
        // There is no longer a "must be named main" restriction: any
        // function name is legal to *define*; only calling one runs it.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "helper".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![],
            }],
        };
        let dir = tempfile_dir("slice0_any_fn_name");
        let obj_path = dir.join("slice0_any_fn_name.o");
        compile_to_object(&mir, &obj_path, None)
            .expect("defining a function under any name should succeed");
    }

    #[test]
    fn write_to_file_failure_is_reported_as_an_error() {
        // A real, reachable failure mode (unlike the internal invariants
        // asserted via .expect() in compile_to_object): the output path's
        // parent directory doesn't exist. an_unknown_target_triple_is_a_
        // clean_error below covers this function's other genuine failure
        // mode, Target::from_triple.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(call_print(42))],
        };
        let bad_path = std::env::temp_dir()
            .join(format!(
                "pycc_codegen_test_nonexistent_dir_{}",
                std::process::id()
            ))
            .join("does_not_exist")
            .join("out.o");
        let err = compile_to_object(&mir, &bad_path, None)
            .expect_err("should fail: parent dir doesn't exist");
        assert!(!err.is_empty());
    }

    #[test]
    fn cross_compiles_object_code_for_a_different_target_triple() {
        // This host is aarch64-apple-darwin; request the other macOS Tier-1
        // architecture. LLVM's codegen backend is inherently multi-target,
        // so this only needs Target::initialize_all (see compile_to_object)
        // plus the requested triple -- verified by checking the emitted
        // object file's actual architecture, not just that codegen didn't
        // error.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(call_print(42))],
        };
        let dir = tempfile_dir("cross_x64");
        let obj_path = dir.join("cross_x64.o");
        compile_to_object(&mir, &obj_path, Some("x86_64-apple-darwin"))
            .expect("cross-compiling to a different Tier-1 target should succeed");

        assert!(
            object_file_cpu_type_is_x86_64(&obj_path),
            "expected a Mach-O object file with cputype x86_64"
        );
    }

    /// Reads the Mach-O header directly instead of shelling out to the
    /// `file` utility, which this test used to do: fragile on Windows,
    /// where `file` isn't a standard tool and only worked because Git's
    /// bundled `usr/bin/file.exe` happened to be on `PATH` there -- an
    /// environment coincidence, not a guarantee. This test only ever
    /// emits Mach-O (`--target x86_64-apple-darwin`), so a full
    /// multi-format parser isn't needed -- just enough of
    /// `mach_header_64`'s fixed layout (magic, then cputype, both
    /// little-endian on every Tier-1 target this project builds for) to
    /// assert the emitted object's architecture is genuinely x86_64, not
    /// a copy-paste no-op.
    fn object_file_cpu_type_is_x86_64(path: &std::path::Path) -> bool {
        const MH_MAGIC_64: [u8; 4] = 0xfeed_facf_u32.to_le_bytes();
        const CPU_TYPE_X86_64: [u8; 4] = 0x0100_0007_u32.to_le_bytes();
        let bytes = std::fs::read(path).expect("object file should be readable");
        bytes.len() >= 8 && bytes[0..4] == MH_MAGIC_64 && bytes[4..8] == CPU_TYPE_X86_64
    }

    #[test]
    fn an_unknown_target_triple_is_a_clean_error() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(call_print(42))],
        };
        let dir = tempfile_dir("bad_triple");
        let obj_path = dir.join("bad_triple.o");
        let err = compile_to_object(&mir, &obj_path, Some("not-a-real-target-triple"))
            .expect_err("an unrecognized target triple should be rejected");
        assert!(!err.is_empty());
    }

    #[test]
    #[should_panic(expected = "this print() argument shape is not supported yet")]
    fn printing_more_than_one_argument_is_not_yet_supported_by_codegen() {
        // `emit_stmt`'s `print`-call arm only handles a single int-literal
        // argument so far (see its doc comment) -- everything else HIR/MIR
        // can now represent for `print` (multiple args, a float, a name
        // reference, ...) hits this explicit panic until a later task in
        // this plan replaces it.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("print_multi_arg_panics");
        let obj_path = dir.join("print_multi_arg_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn a_bare_return_with_no_value_exits_a_none_returning_function_early() {
        // `def f() -> None:\n    return\n    print(999)` ; `f(); print(1)`
        // -- supersedes this test's earlier (Task 3/4) incarnation,
        // `a_return_statement_is_not_yet_supported_by_codegen`, which
        // proved `Return` had no codegen at all yet (via `emit_stmt`'s
        // then-catch-all, since removed -- the match is now exhaustive
        // over `MirStmt`, see Task 5's own doc comment on `emit_stmt`).
        // Now that `Return` is fully implemented, this instead exercises
        // its `None` arm (a bare `return`, as opposed to `return <expr>`,
        // which the two dedicated function-call tests above already
        // cover) and proves `emit_body`'s terminator-safety early-stop
        // (re-added by this task, see its own doc comment) really does
        // skip the unreachable `print(999)` after the `return`, rather
        // than trying to emit into an already-terminated block. Only "1"
        // should print.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        MirStmt::Return(None),
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::IntLiteral(999)],
                            ty: Ty::None,
                        }),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                    ty: Ty::None,
                })),
                MirItem::TopLevelStmt(call_print(1)),
            ],
        };
        let dir = tempfile_dir("bare_return_none");
        let obj_path = dir.join("bare_return_none.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("bare_return_none");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn compiles_local_variable_arithmetic_comparisons_and_floor_division() {
        // `x = 7; y = 2; print(x // y)` at the MIR level, exercising: a fresh
        // `alloca` per local, `BinOp::FloorDiv` codegen, and reading a `Name`
        // back out of its local for a later statement -- everything Task 2's
        // temporary `emit_stmt` explicitly could not do yet.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(7),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::IntLiteral(2),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::BinOp {
                        op: BinOpKind::FloorDiv,
                        left: Box::new(MirExpr::Name { name: "x".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::Name { name: "y".to_string(), ty: Ty::Int }),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("locals_arith");
        let obj_path = dir.join("locals_arith.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("locals_arith");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n");
    }

    #[test]
    fn compiles_a_comparison_result_stored_in_a_bool_local() {
        // `b = 1 < 2` -- exercises `Compare` codegen and a `bool`-typed
        // (`i8`) local's own `alloca`, distinct from `int`'s tagged `i64`.
        // Nothing yet reads `b` back out (print(bool) is Task 10's job), so
        // this only proves the assignment itself doesn't crash/miscompile;
        // `verify_module`'s `module.verify()` call (non-Windows) is the
        // actual proof the generated IR is well-formed.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "b".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("bool_local");
        let obj_path = dir.join("bool_local.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn reassigning_a_local_reuses_its_existing_alloca() {
        // `x = 1; x = 2; print(x)` -- the second `Assign` must reuse `x`'s
        // existing slot (not allocate a second, shadowing one), matching
        // ordinary Python rebinding semantics.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(1),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(2),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("reassign_local");
        let obj_path = dir.join("reassign_local.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("reassign_local");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn compiles_and_runs_add_sub_mul_mod_and_pow_binops() {
        // Exercises every remaining `BinOpKind` arm `emit_expr`'s `BinOp`
        // codegen selects a `pycc_rt` function for (`FloorDiv` already has
        // its own dedicated test above) end to end: compiled, linked, run,
        // with real stdout checked -- not just that codegen didn't panic.
        fn print_binop(op: BinOpKind, left: i64, right: i64) -> MirStmt {
            MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BinOp {
                    op,
                    left: Box::new(MirExpr::IntLiteral(left)),
                    right: Box::new(MirExpr::IntLiteral(right)),
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            })
        }
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(print_binop(BinOpKind::Add, 3, 4)),
                MirItem::TopLevelStmt(print_binop(BinOpKind::Sub, 10, 3)),
                MirItem::TopLevelStmt(print_binop(BinOpKind::Mul, 6, 7)),
                MirItem::TopLevelStmt(print_binop(BinOpKind::Mod, 7, 2)),
                MirItem::TopLevelStmt(print_binop(BinOpKind::Pow, 2, 5)),
            ],
        };
        let dir = tempfile_dir("int_binops");
        let obj_path = dir.join("int_binops.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("int_binops");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"7\n7\n42\n1\n32\n");
    }

    #[test]
    #[should_panic(expected = "true division")]
    fn true_division_binop_codegen_panics_via_its_dedicated_arm() {
        // `pycc_mir::binop_result_ty` always types `BinOpKind::Div` as
        // `Ty::Float` (`5 / 2 == 2.5`, never `Ty::Int`), so no real
        // `pycc_types`-produced MIR can construct `BinOp { op: Div, ty:
        // Ty::Int, .. }` -- a real float-division `BinOp` (`ty: Ty::Float`)
        // is now correctly handled by `to_float`/`build_float_div` (Task 6;
        // see `compiles_true_division_of_two_ints_as_float_arithmetic`
        // above), not a catch-all. The *only* way to reach this dedicated
        // (now `unreachable!`) `Div` arm inside the `BinOp { ty: Ty::Int,
        // .. }` match is to hand-construct this deliberately mislabeled
        // shape, matching this crate's existing convention (see
        // `printing_a_mistyped_compare_expression_hits_the_internal_consistency_check`
        // below) for testing defensive arms real MIR can't reach.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Div,
                    left: Box::new(MirExpr::IntLiteral(4)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Int,
                },
            })],
        };
        let dir = tempfile_dir("true_div_panics");
        let obj_path = dir.join("true_div_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_the_remaining_comparison_operators() {
        // `Lt` already has its own dedicated test above; this exercises the
        // rest of `IntPredicate`'s match arms (`Eq`/`NotEq`/`LtE`/`Gt`/`GtE`).
        fn assign_compare(target: &str, op: CmpOpKind) -> MirStmt {
            MirStmt::Assign {
                target: target.to_string(),
                value: MirExpr::Compare {
                    op,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Bool,
                },
            }
        }
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_compare("a", CmpOpKind::Eq)),
                MirItem::TopLevelStmt(assign_compare("b", CmpOpKind::NotEq)),
                MirItem::TopLevelStmt(assign_compare("c", CmpOpKind::LtE)),
                MirItem::TopLevelStmt(assign_compare("d", CmpOpKind::Gt)),
                MirItem::TopLevelStmt(assign_compare("e", CmpOpKind::GtE)),
            ],
        };
        let dir = tempfile_dir("remaining_cmp_ops");
        let obj_path = dir.join("remaining_cmp_ops.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn reading_a_bool_local_back_out_of_its_alloca() {
        // `b = 1 < 2; c = b` -- exercises `emit_expr`'s `Name` arm on a
        // `Ty::Bool` local (the existing bool-local test above only ever
        // assigns one, never reads it back).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "b".to_string(),
                    value: MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::IntLiteral(1)),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Bool,
                    },
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "c".to_string(),
                    value: MirExpr::Name { name: "b".to_string(), ty: Ty::Bool },
                }),
            ],
        };
        let dir = tempfile_dir("read_bool_local");
        let obj_path = dir.join("read_bool_local.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn adding_a_bool_left_operand_to_an_int_promotes_bool_to_int() {
        // `x = True + 1; print(x)` -- `pycc_types` accepts this (`bool` is
        // numeric-like, see its own `a_binop_treats_bool_as_int` test) and
        // infers `Ty::Int`. This file's earlier (Task 3) version of this
        // test proved that `emit_expr` did not yet implement the
        // bool-to-int promotion this needs, and hit a defensive check
        // mislabeled "internal error" for what a prior review correctly
        // flagged is actually reachable from real, legitimate source (this
        // exact case). Task 6's `to_tagged_int` (see its own doc comment)
        // now implements that promotion for real, so this is rewritten
        // into what it always should have been: a positive test proving
        // `True + 1` correctly computes `2`, not a panic.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::BoolLiteral(true)),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    },
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("binop_bool_left_promotes");
        let obj_path = dir.join("binop_bool_left_promotes.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("binop_bool_left_promotes");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn adding_an_int_and_a_bool_right_operand_promotes_bool_to_int() {
        // `x = 1 + True; print(x)` -- distinct region from the
        // left-operand case above (`to_tagged_int` is called once per
        // operand).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::IntLiteral(1)),
                        right: Box::new(MirExpr::BoolLiteral(true)),
                        ty: Ty::Int,
                    },
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("binop_bool_right_promotes");
        let obj_path = dir.join("binop_bool_right_promotes.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("binop_bool_right_promotes");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn comparing_a_bool_left_operand_to_an_int_promotes_bool_to_int() {
        // `True < 2` -- `pycc_types` accepts comparing `bool` and `int`
        // (`bool` is a subtype of `int`, see its own
        // `comparing_a_bool_and_an_int_succeeds_since_bool_is_a_subtype_of_int`
        // test); Task 6's `Compare` codegen now promotes the `bool`
        // operand via `to_tagged_int` instead of rejecting it (same
        // rewrite rationale as the `BinOp` tests above). Nothing reads
        // the result back (print(bool) is Task 10's job), so this only
        // proves the comparison itself doesn't crash/miscompile, same as
        // `compiles_a_comparison_result_stored_in_a_bool_local`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::BoolLiteral(true)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("compare_bool_left_promotes");
        let obj_path = dir.join("compare_bool_left_promotes.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn comparing_an_int_and_a_bool_right_operand_promotes_bool_to_int() {
        // Distinct region from the left-operand case above (`1 < True`).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::BoolLiteral(true)),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("compare_bool_right_promotes");
        let obj_path = dir.join("compare_bool_right_promotes.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "too large for the v0.1 fast path")]
    fn an_oversized_int_literal_is_not_yet_supported() {
        // `tag_smallint_const`'s own round-trip check: `i64::MAX` doesn't
        // fit the 63-bit tagged range (D-052).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(i64::MAX)],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("oversized_int_literal_panics");
        let obj_path = dir.join("oversized_int_literal_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "this expression kind's codegen is not supported yet")]
    fn assigning_a_string_literal_is_not_yet_supported_by_codegen() {
        // `pycc_types` fully accepts `x = "hello"` (see its own
        // `infers_a_string_literal_as_str` test); Task 3's `emit_expr`
        // simply doesn't implement `StringLiteral` yet (Task 7's job).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::StringLiteral("hello".to_string()),
            })],
        };
        let dir = tempfile_dir("string_literal_panics");
        let obj_path = dir.join("string_literal_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "Ty::Int always evaluates to Scalar::Int")]
    fn printing_a_mistyped_compare_expression_hits_the_internal_consistency_check() {
        // Deliberately malformed MIR: `pycc_mir::build` always lowers
        // `Compare` with `ty: Ty::Bool` (see `pycc_mir`'s own
        // `builds_a_compare_expression_with_bool_type` test) -- no real
        // pipeline could ever produce `ty: Ty::Int` here. This directly
        // exercises `emit_stmt`'s defensive `unreachable!()`, genuinely
        // unlike the bool/int-mixing cases above (which real source *can*
        // reach): `emit_expr`'s `Compare` arm always returns `Scalar::Bool`
        // regardless of what the (lied-about) `ty` field claims, so the
        // print branch's `Ty::Int`-guarded call still gets a `Scalar::Bool`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("print_mistyped_compare_panics");
        let obj_path = dir.join("print_mistyped_compare_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn a_bare_expression_statement_evaluates_and_discards_its_value() {
        // `5` as its own top-level statement (Python allows a bare
        // expression statement); nothing currently has a side effect from
        // it, but the shape is legal MIR and must not panic or miscompile.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::IntLiteral(5)))],
        };
        let dir = tempfile_dir("bare_expr_stmt");
        let obj_path = dir.join("bare_expr_stmt.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "reading a `Str`-typed local is not supported yet")]
    fn reading_a_str_typed_local_is_not_yet_supported() {
        // Not reachable through `compile_to_object`/real MIR at all yet:
        // every path that could insert a non-`Int`/`Bool`/`Float`-typed
        // entry into `locals` needs `emit_expr` to first evaluate a value
        // of that type successfully via `emit_assign`, and nothing in this
        // crate's own `emit_expr` can do that yet for `Str`
        // (`StringLiteral` itself hits the generic catch-all before an
        // `Assign` could ever complete) -- so this calls `emit_expr`
        // directly with a hand-built `locals` map instead. This test's
        // earlier (Task 3) incarnation proved the identical gap for
        // `Ty::Float`, which Task 6 closed (see
        // `reading_a_float_local_back_out_of_its_alloca` below) -- this
        // becomes reachable via `compile_to_object` for real, legitimate
        // source the moment Task 7 adds real local support for `str`;
        // this same arm keeps guarding whatever remains unimplemented at
        // that point.
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let rt = declare_rt_functions(&context, &module);
        let fn_type = context.void_type().fn_type(&[], false);
        let f = module.add_function("f", fn_type, None);
        let block = context.append_basic_block(f, "entry");
        builder.position_at_end(block);

        let user_functions: HashMap<&str, FunctionValue> = HashMap::new();
        let mut locals = HashMap::new();
        // The alloca's own LLVM type is arbitrary here (`str` has no
        // codegen representation yet to allocate correctly) -- it's never
        // actually loaded from, since `emit_expr`'s `Name` arm panics on
        // `Ty::Str` before reaching any `build_load` call.
        let ptr = builder
            .build_alloca(context.f64_type(), "x")
            .expect("build_alloca should not fail for a fresh block");
        locals.insert("x".to_string(), (ptr, Ty::Str));

        emit_expr(
            &context,
            &builder,
            &module,
            &rt,
            &user_functions,
            &locals,
            &MirExpr::Name { name: "x".to_string(), ty: Ty::Str },
        );
    }

    #[test]
    fn reading_a_float_local_back_out_of_its_alloca() {
        // `x = 1.5; y = x + 1.0` -- exercises `emit_expr`'s `Name` arm on
        // a `Ty::Float` local (mirrors the existing bool-local read-back
        // test, `reading_a_bool_local_back_out_of_its_alloca` above).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::FloatLiteral(1.5),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::Name { name: "x".to_string(), ty: Ty::Float }),
                        right: Box::new(MirExpr::FloatLiteral(1.0)),
                        ty: Ty::Float,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("read_float_local");
        let obj_path = dir.join("read_float_local.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn a_function_parameter_can_be_read_back_and_printed() {
        // `def f(n: int): print(n)` ; `f(7)` -- supersedes this test's
        // earlier (Task 3) incarnation, `referencing_a_function_parameter_
        // is_not_yet_supported`, which proved the opposite: that
        // `compile_to_object` started each function's `fn_locals` map
        // empty, so reading a parameter back by name hit an internal-error
        // panic. Task 5 fixes exactly that gap (see `compile_to_object`'s
        // second pass: each parameter gets its own `alloca`, with the
        // incoming LLVM argument stored into it before the body runs), so
        // this now proves a parameter is fully ordinary -- readable via
        // `emit_expr`'s `Name` arm exactly like any other local -- and
        // this call site also exercises `emit_stmt`'s void-call arm with a
        // *non-empty* argument list (every other void-call test in this
        // file uses a zero-arg call).
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("n".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name { name: "n".to_string(), ty: Ty::Int }],
                        ty: Ty::None,
                    })],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![MirExpr::IntLiteral(7)],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("param_reference_reads_back");
        let obj_path = dir.join("param_reference_reads_back.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("param_reference_reads_back");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"7\n");
    }

    #[test]
    fn compiles_an_if_else_choosing_the_correct_branch_at_runtime() {
        // `x = 1; if x < 2: print(10) else: print(20)` -- must print 10.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(1),
                }),
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::Name { name: "x".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Bool,
                    },
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::IntLiteral(10)],
                        ty: Ty::None,
                    })],
                    orelse: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::IntLiteral(20)],
                        ty: Ty::None,
                    })],
                }),
            ],
        };
        let dir = tempfile_dir("if_else");
        let obj_path = dir.join("if_else.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("if_else");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"10\n");
    }

    #[test]
    fn compiles_an_if_with_no_else_and_a_false_test_prints_nothing() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::BoolLiteral(false),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: Ty::None,
                })],
                orelse: vec![],
            })],
        };
        let dir = tempfile_dir("if_no_else");
        let obj_path = dir.join("if_no_else.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("if_no_else");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"");
    }

    #[test]
    fn compiles_a_while_loop_that_counts_down() {
        // `i = 3; while i > 0: print(i); i = i - 1` -- prints 3, 2, 1.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "i".to_string(),
                    value: MirExpr::IntLiteral(3),
                }),
                MirItem::TopLevelStmt(MirStmt::While {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Gt,
                        left: Box::new(MirExpr::Name { name: "i".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::IntLiteral(0)),
                        ty: Ty::Bool,
                    },
                    body: vec![
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                            ty: Ty::None,
                        }),
                        MirStmt::Assign {
                            target: "i".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Sub,
                                left: Box::new(MirExpr::Name { name: "i".to_string(), ty: Ty::Int }),
                                right: Box::new(MirExpr::IntLiteral(1)),
                                ty: Ty::Int,
                            },
                        },
                    ],
                }),
            ],
        };
        let dir = tempfile_dir("while_countdown");
        let obj_path = dir.join("while_countdown.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("while_countdown");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n2\n1\n");
    }

    #[test]
    fn compiles_a_while_loop_using_a_bare_int_condition_via_truthy() {
        // `i = 3; while i: print(i); i = i - 1` -- prints 3, 2, 1, same
        // countdown as the test above, but the loop test is a plain
        // `int`-typed `Name` (not a `Compare`), so this is the only test
        // in this file exercising `truthy`'s `Scalar::Int` arm (every
        // other `If`/`While` test's condition is a `Compare`, which always
        // evaluates to `Scalar::Bool`) -- `pycc_rt_int_truthy` genuinely
        // gets called from generated code here, not just unit-tested
        // directly in `pycc_rt`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "i".to_string(),
                    value: MirExpr::IntLiteral(3),
                }),
                MirItem::TopLevelStmt(MirStmt::While {
                    test: MirExpr::Name { name: "i".to_string(), ty: Ty::Int },
                    body: vec![
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                            ty: Ty::None,
                        }),
                        MirStmt::Assign {
                            target: "i".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Sub,
                                left: Box::new(MirExpr::Name { name: "i".to_string(), ty: Ty::Int }),
                                right: Box::new(MirExpr::IntLiteral(1)),
                                ty: Ty::Int,
                            },
                        },
                    ],
                }),
            ],
        };
        let dir = tempfile_dir("while_int_truthy");
        let obj_path = dir.join("while_int_truthy.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("while_int_truthy");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n2\n1\n");
    }

    #[test]
    fn compiles_a_for_range_loop_with_a_positive_step() {
        // `for i in range(0, 6, 2): print(i)` -- prints 0, 2, 4.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(6),
                step: MirExpr::IntLiteral(2),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_pos");
        let obj_path = dir.join("for_range_pos.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("for_range_pos");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n2\n4\n");
    }

    #[test]
    fn compiles_a_for_range_loop_with_a_negative_step() {
        // `for i in range(3, 0, -1): print(i)` -- prints 3, 2, 1.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(3),
                stop: MirExpr::IntLiteral(0),
                step: MirExpr::IntLiteral(-1),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_neg");
        let obj_path = dir.join("for_range_neg.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("for_range_neg");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n2\n1\n");
    }

    #[test]
    #[should_panic(expected = "range() start did not evaluate to int")]
    fn for_range_with_a_non_int_start_is_rejected() {
        // `pycc_types`/`pycc_mir` only ever build `ForRange` with `int`
        // `start`/`stop`/`step` (matching CPython's own `range()` argument
        // rule); this defensive check is unreachable via any real pipeline
        // output, same convention as the `BinOp`/`Compare` operand-type
        // checks above -- hand-built malformed MIR exercises it directly.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::BoolLiteral(true),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
                body: vec![],
            })],
        };
        let dir = tempfile_dir("for_range_bad_start_panics");
        let obj_path = dir.join("for_range_bad_start_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "range() stop did not evaluate to int")]
    fn for_range_with_a_non_int_stop_is_rejected() {
        // Distinct region from the `start` case above.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::BoolLiteral(true),
                step: MirExpr::IntLiteral(1),
                body: vec![],
            })],
        };
        let dir = tempfile_dir("for_range_bad_stop_panics");
        let obj_path = dir.join("for_range_bad_stop_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "range() step did not evaluate to int")]
    fn for_range_with_a_non_int_step_is_rejected() {
        // Distinct region from the `start`/`stop` cases above.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::BoolLiteral(true),
                body: vec![],
            })],
        };
        let dir = tempfile_dir("for_range_bad_step_panics");
        let obj_path = dir.join("for_range_bad_step_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_nested_control_flow_with_a_statement_after_it_in_the_same_body() {
        // `for i in range(0, 3, 1): (if i == 1: print(100)); print(i)` --
        // exercises two things no other test in this file does: control
        // flow (`If`) nested inside other control flow (`ForRange`), and a
        // statement following a control-flow statement in the *same*
        // `body` list. Every other test's `If`/`While`/`ForRange` is the
        // last statement of its enclosing body -- so nothing else proves
        // that `emit_stmt`'s `If` arm correctly leaves the builder
        // positioned at `merge_bb` in a state where a *subsequent*
        // statement resumes into it correctly (right `locals`, right
        // block, no invalid IR from double-terminating or orphaning a
        // block) -- exactly the invariant `emit_body`'s own doc comment
        // relies on to justify never needing an early-terminator-stop
        // check in Task 4's scope. Expected: i=0 -> "0"; i=1 -> "100" then
        // "1"; i=2 -> "2".
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
                body: vec![
                    MirStmt::If {
                        test: MirExpr::Compare {
                            op: CmpOpKind::Eq,
                            left: Box::new(MirExpr::Name { name: "i".to_string(), ty: Ty::Int }),
                            right: Box::new(MirExpr::IntLiteral(1)),
                            ty: Ty::Bool,
                        },
                        body: vec![MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::IntLiteral(100)],
                            ty: Ty::None,
                        })],
                        orelse: vec![],
                    },
                    MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                        ty: Ty::None,
                    }),
                ],
            })],
        };
        let dir = tempfile_dir("nested_control_flow_resume");
        let obj_path = dir.join("nested_control_flow_resume.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("nested_control_flow_resume");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n100\n1\n2\n");
    }

    // The four tests below are not in the brief's own Step 3 list -- added
    // because `cargo llvm-cov`'s region coverage showed each of `If`'s
    // `then`/`orelse` arms, `While`'s body, and `ForRange`'s body has its
    // own distinct `?`-propagation region (one per `emit_body`/`emit_body_
    // then_branch` call site inside `emit_stmt`, not shared across arms):
    // every prior test's nested body only ever contains statements that
    // succeed, so none of these four `?` operators had ever actually
    // propagated an `Err`. Mirrors the existing top-level/function-body
    // `calling_an_undefined_function_..._is_rejected` tests above, just
    // with the undefined call nested one level deeper.
    #[test]
    fn calling_an_undefined_function_inside_an_if_then_body_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::BoolLiteral(true),
                body: vec![call_user_fn("does_not_exist_in_if_then")],
                orelse: vec![],
            })],
        };
        let dir = tempfile_dir("if_then_undefined_fn");
        let obj_path = dir.join("if_then_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(
            err.contains("does_not_exist_in_if_then"),
            "error should name the offending function: {err}"
        );
    }

    #[test]
    fn calling_an_undefined_function_inside_an_if_orelse_body_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::BoolLiteral(false),
                body: vec![],
                orelse: vec![call_user_fn("does_not_exist_in_if_orelse")],
            })],
        };
        let dir = tempfile_dir("if_orelse_undefined_fn");
        let obj_path = dir.join("if_orelse_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(
            err.contains("does_not_exist_in_if_orelse"),
            "error should name the offending function: {err}"
        );
    }

    #[test]
    fn calling_an_undefined_function_inside_a_while_body_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::While {
                test: MirExpr::BoolLiteral(true),
                body: vec![call_user_fn("does_not_exist_in_while")],
            })],
        };
        let dir = tempfile_dir("while_undefined_fn");
        let obj_path = dir.join("while_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(
            err.contains("does_not_exist_in_while"),
            "error should name the offending function: {err}"
        );
    }

    #[test]
    fn calling_an_undefined_function_inside_a_for_range_body_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::IntLiteral(3),
                step: MirExpr::IntLiteral(1),
                body: vec![call_user_fn("does_not_exist_in_for_range")],
            })],
        };
        let dir = tempfile_dir("for_range_undefined_fn");
        let obj_path = dir.join("for_range_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None).expect_err("should be rejected");
        assert!(
            err.contains("does_not_exist_in_for_range"),
            "error should name the offending function: {err}"
        );
    }

    #[test]
    fn compiles_a_function_call_with_real_arguments_and_a_return_value() {
        // `def add(a: int, b: int) -> int: return a + b` ; `print(add(2, 3))`
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "add".to_string(),
                    params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::Name { name: "a".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::Name { name: "b".to_string(), ty: Ty::Int }),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "add".to_string(),
                        args: vec![MirExpr::IntLiteral(2), MirExpr::IntLiteral(3)],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("call_with_args");
        let obj_path = dir.join("call_with_args.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("call_with_args");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"5\n");
    }

    #[test]
    fn a_multi_argument_call_binds_each_parameter_in_the_right_order() {
        // `def sub(a: int, b: int) -> int: return a - b` ; `print(sub(10, 3))`
        // -- `add`'s own test above is commutative (`2 + 3 == 3 + 2`), so it
        // can't tell a correct argument-to-parameter binding apart from a
        // transposed one (`get_nth_param(i)` bound to the wrong
        // `param_name`, or `build_call_to` marshaling `args` out of order).
        // `sub` isn't commutative: `10 - 3 == 7`, but the transposed
        // binding would compute `3 - 10 == -7` instead. Prints "7", not
        // "-7".
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "sub".to_string(),
                    params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(MirExpr::Name { name: "a".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::Name { name: "b".to_string(), ty: Ty::Int }),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "sub".to_string(),
                        args: vec![MirExpr::IntLiteral(10), MirExpr::IntLiteral(3)],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("call_arg_order");
        let obj_path = dir.join("call_arg_order.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("call_arg_order");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"7\n");
    }

    #[test]
    fn compiles_a_recursive_function_with_an_early_return() {
        // `def fact(n: int) -> int:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)`
        // `print(fact(5))` -- exercises recursion (calling `fact` from inside
        // its own not-yet-fully-emitted body works because the two-pass
        // declare-then-define structure already declares every function
        // before any body is compiled), a return nested inside an `if` with
        // no `else`, and a second `return` reached only via that `if`'s false
        // edge (Task 4's `merge_bb` handling).
        let fact_body = vec![
            MirStmt::If {
                test: MirExpr::Compare {
                    op: CmpOpKind::LtE,
                    left: Box::new(MirExpr::Name { name: "n".to_string(), ty: Ty::Int }),
                    right: Box::new(MirExpr::IntLiteral(1)),
                    ty: Ty::Bool,
                },
                body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
                orelse: vec![],
            },
            MirStmt::Return(Some(MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::Name { name: "n".to_string(), ty: Ty::Int }),
                right: Box::new(MirExpr::Call {
                    callee: "fact".to_string(),
                    args: vec![MirExpr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(MirExpr::Name { name: "n".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    }],
                    ty: Ty::Int,
                }),
                ty: Ty::Int,
            })),
        ];
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "fact".to_string(),
                    params: vec![("n".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: fact_body,
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "fact".to_string(),
                        args: vec![MirExpr::IntLiteral(5)],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("recursive_fact");
        let obj_path = dir.join("recursive_fact.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("recursive_fact");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"120\n");
    }

    #[test]
    fn a_non_none_function_falling_through_is_an_internal_error_not_bad_ir() {
        // `pycc_types`' T0024 fallthrough check should have rejected this
        // HIR already -- this proves codegen fails loudly (a clear panic)
        // rather than emitting an invalid `ret` from a function declared to
        // return `int`, if that check is ever somehow bypassed.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "broken".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![],
            }],
        };
        let dir = tempfile_dir("fallthrough_internal_error");
        let obj_path = dir.join("fallthrough_internal_error.o");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compile_to_object(&mir, &obj_path, None)
        }));
        assert!(result.is_err(), "expected a panic, not a successfully-compiled object");
    }

    #[test]
    #[should_panic(expected = "internal error: call to undefined function")]
    fn calling_an_undefined_function_as_a_nested_expression_is_an_internal_error() {
        // Unlike a bare statement-level call (see `calling_an_undefined_
        // function_at_top_level_is_rejected` and its siblings above, which
        // still return a clean `Result::Err` -- `emit_stmt`'s void-call
        // arm generalizes this crate's pre-Task-5 zero-arg-only behavior
        // rather than switching to a panic), a call used *inside* another
        // expression flows through `emit_expr`'s `Call` arm, which returns
        // a `Scalar`, not a `Result` -- there is no way to propagate a
        // graceful error from there. Real `pycc_types` already rejects any
        // call to an undefined function (T0021) long before codegen runs,
        // so this is this crate's own defensive "should never happen"
        // backstop, not a rejection of legitimate source.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::Call {
                    callee: "does_not_exist_as_expr".to_string(),
                    args: vec![],
                    ty: Ty::Int,
                },
            })],
        };
        let dir = tempfile_dir("undefined_fn_nested_expr_panics");
        let obj_path = dir.join("undefined_fn_nested_expr_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_a_function_call_returning_bool_used_as_an_expression() {
        // `def is_positive(n: int) -> bool: return n > 0` ;
        // `x = is_positive(5)` -- the brief's own Step 1 tests only ever
        // exercise `emit_expr`'s `Call` arm's `Ty::Int` branch; this
        // exercises its `Ty::Bool` branch instead.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "is_positive".to_string(),
                    params: vec![("n".to_string(), Ty::Int)],
                    return_ty: Ty::Bool,
                    body: vec![MirStmt::Return(Some(MirExpr::Compare {
                        op: CmpOpKind::Gt,
                        left: Box::new(MirExpr::Name { name: "n".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::IntLiteral(0)),
                        ty: Ty::Bool,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::Call {
                        callee: "is_positive".to_string(),
                        args: vec![MirExpr::IntLiteral(5)],
                        ty: Ty::Bool,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("call_returns_bool");
        let obj_path = dir.join("call_returns_bool.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "a `None`-typed call result is not supported yet")]
    fn a_none_typed_call_result_used_as_a_nested_expression_is_not_supported() {
        // Deliberately malformed MIR: a `None`-returning function's result
        // can only legitimately appear as a bare statement (see
        // `emit_stmt`'s own void-call arm) -- real `pycc_types` would
        // never type an `Assign`'s value as `Ty::None` this way (there is
        // no `x = None`-shaped source this could come from in v0.1).
        // Exercises `emit_expr`'s `Call` arm's own defensive `other =>`
        // catch-all on `ty`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::None },
                }),
            ],
        };
        let dir = tempfile_dir("none_typed_call_result_panics");
        let obj_path = dir.join("none_typed_call_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "a `Str`-typed parameter/return value is not supported yet")]
    fn a_str_typed_return_value_is_not_yet_supported() {
        // `def f() -> str: ...` -- `ty_to_basic_type` implements
        // `Int`/`Bool`/`Float` (Task 6 added `Float`, see
        // `compiles_a_function_with_a_float_parameter_and_float_return_value`
        // below); `Str` is Task 7's job. This test's earlier (Task 3)
        // incarnation used `Ty::Float` here, before Task 6 closed that gap.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Str,
                body: vec![],
            }],
        };
        let dir = tempfile_dir("str_return_panics");
        let obj_path = dir.join("str_return_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "a `Str`-typed parameter/return value is not supported yet")]
    fn a_str_typed_parameter_is_not_yet_supported() {
        // `def f(x: str): ...` -- a distinct `ty_to_basic_type` call site
        // from the return-type test above (a function's parameter list,
        // inside `compile_to_object`'s first pass), same underlying panic.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::Str)],
                return_ty: Ty::None,
                body: vec![],
            }],
        };
        let dir = tempfile_dir("str_param_panics");
        let obj_path = dir.join("str_param_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_a_function_with_a_float_parameter_and_float_return_value() {
        // `def f(x: float) -> float: return x` ; `y = f(1.5)` -- exercises
        // `ty_to_basic_type`'s new `Ty::Float` arm (both the parameter and
        // return-type positions), `build_call_to`'s argument-marshaling
        // match's `Scalar::Float` arm, `emit_expr`'s `Call` arm's
        // `Ty::Float`-result match arm, and `emit_stmt`'s `Return` arm's
        // `Scalar::Float` match arm -- every `float`-typed position Task 5
        // could not support yet.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Float)],
                    return_ty: Ty::Float,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Float,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::FloatLiteral(1.5)],
                        ty: Ty::Float,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("float_param_and_return");
        let obj_path = dir.join("float_param_and_return.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn a_return_inside_a_for_range_body_returns_immediately_without_looping() {
        // `def first_of_range() -> int:\n    for i in range(0, 5, 1):\n
        // return i\n    return -1` ; `print(first_of_range())` -- the
        // trailing `return -1` is unreachable in practice (every
        // legitimate call actually returns from inside the loop on its
        // first iteration) but keeps this hand-built MIR shape well-formed
        // for a non-`None`-returning function (real `pycc_types` would
        // likely reject a bare `for` loop as satisfying T0024's
        // definite-return check on its own, since a `for` loop is never
        // assumed to execute at least once). Proves `ForRange`'s own
        // inline terminator-safety guard (this task's re-add, see the
        // `ForRange` arm's own comment) correctly skips the
        // increment-and-branch-back the moment `body`'s `Return` already
        // terminates `body_bb` -- without it, this would try to build a
        // second terminator onto an already-terminated block, which
        // `module.verify()` would (correctly) reject. Prints "0", not
        // "0\n1\n2\n3\n4\n" (which would mean the loop kept running) or a
        // crash.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "first_of_range".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::ForRange {
                            var: "i".to_string(),
                            start: MirExpr::IntLiteral(0),
                            stop: MirExpr::IntLiteral(5),
                            step: MirExpr::IntLiteral(1),
                            body: vec![MirStmt::Return(Some(MirExpr::Name {
                                name: "i".to_string(),
                                ty: Ty::Int,
                            }))],
                        },
                        MirStmt::Return(Some(MirExpr::IntLiteral(-1))),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "first_of_range".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("for_range_return_inside_body");
        let obj_path = dir.join("for_range_return_inside_body.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("for_range_return_inside_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n");
    }

    #[test]
    fn compiles_a_function_call_with_a_bool_argument() {
        // `def identity_bool(b: bool) -> bool: return b` ;
        // `x = identity_bool(True)` -- exercises `build_call_to`'s
        // `Scalar::Bool` argument-marshalling arm (every other
        // function-call test in this file passes only `int` arguments).
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "identity_bool".to_string(),
                    params: vec![("b".to_string(), Ty::Bool)],
                    return_ty: Ty::Bool,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "b".to_string(),
                        ty: Ty::Bool,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::Call {
                        callee: "identity_bool".to_string(),
                        args: vec![MirExpr::BoolLiteral(true)],
                        ty: Ty::Bool,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("call_with_bool_arg");
        let obj_path = dir.join("call_with_bool_arg.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "using print()'s result as a nested expression is not supported yet")]
    fn nesting_a_print_call_inside_another_expression_is_not_yet_supported() {
        // `x = print(1)` -- v0.1's `print()` always returns `None`, and
        // nothing implements using a `None` value as an operand yet.
        // `emit_stmt`'s own `print`-call arm builds a `pycc_rt_int_print`
        // call directly and never routes the outer `print(...)` itself
        // through `emit_expr` -- so the only way a `print` call can reach
        // `emit_expr`'s `Call` arm at all is nested one level deeper than
        // that, inside another expression, exercised here via `Assign`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: Ty::None,
                },
            })],
        };
        let dir = tempfile_dir("print_result_nested_panics");
        let obj_path = dir.join("print_result_nested_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "has no local slot")]
    fn referencing_a_name_with_no_bound_local_is_an_internal_error() {
        // Real `pycc_types` already rejects any reference to an undefined
        // name (T0021) long before codegen runs, so this is hand-built
        // malformed MIR exercising `emit_expr`'s `Name` arm's own
        // defensive backstop directly. This panic's coverage used to come
        // from this file's earlier (Task 3) `referencing_a_function_
        // parameter_is_not_yet_supported` test, which happened to hit it
        // via an *unbound* parameter; now that Task 5 binds parameters for
        // real, that test was rewritten into a positive one (see
        // `a_function_parameter_can_be_read_back_and_printed` above),
        // leaving this exact check's own coverage to this more direct,
        // dedicated test instead.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Name {
                name: "never_bound".to_string(),
                ty: Ty::Int,
            }))],
        };
        let dir = tempfile_dir("unbound_name_panics");
        let obj_path = dir.join("unbound_name_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_true_division_of_two_ints_as_float_arithmetic() {
        // `x = 7 / 2` -- must promote both operands to float and use
        // `fdiv`, not integer division (`pycc_types` already types this
        // `Ty::Float`; this proves codegen honors that, not `int`'s own
        // `//`).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: pycc_mir::BinOpKind::Div,
                    left: Box::new(MirExpr::IntLiteral(7)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: pycc_mir::Ty::Float,
                },
            })],
        };
        let dir = tempfile_dir("true_div");
        let obj_path = dir.join("true_div.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_mixed_int_and_float_addition() {
        // `y = 1 + 1.5` -- promotes the `int` operand to `float`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::BinOp {
                    op: pycc_mir::BinOpKind::Add,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::FloatLiteral(1.5)),
                    ty: pycc_mir::Ty::Float,
                },
            })],
        };
        let dir = tempfile_dir("mixed_add");
        let obj_path = dir.join("mixed_add.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_bool_arithmetic_promoted_to_int() {
        // `z = True + True` -- Python's `bool` is an `int` subtype; the
        // result is `2` (`int`), not a `bool`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "z".to_string(),
                    value: MirExpr::BinOp {
                        op: pycc_mir::BinOpKind::Add,
                        left: Box::new(MirExpr::BoolLiteral(true)),
                        right: Box::new(MirExpr::BoolLiteral(true)),
                        ty: pycc_mir::Ty::Int,
                    },
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "z".to_string(), ty: pycc_mir::Ty::Int }],
                    ty: pycc_mir::Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bool_arith");
        let obj_path = dir.join("bool_arith.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("bool_arith");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn compiles_a_float_comparison() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "b".to_string(),
                value: MirExpr::Compare {
                    op: pycc_mir::CmpOpKind::Lt,
                    left: Box::new(MirExpr::FloatLiteral(1.5)),
                    right: Box::new(MirExpr::FloatLiteral(2.5)),
                    ty: pycc_mir::Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("float_cmp");
        let obj_path = dir.join("float_cmp.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_an_if_test_on_a_float_expression() {
        // `if 0.0: print(1)` -- must print nothing (`0.0` is falsy).
        // `if 1.5: print(1)` -- must print `1`.
        for (test, expected) in [(0.0, ""), (1.5, "1\n")] {
            let mir = MirModule {
                items: vec![MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::FloatLiteral(test),
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::IntLiteral(1)],
                        ty: pycc_mir::Ty::None,
                    })],
                    orelse: vec![],
                })],
            };
            let dir = tempfile_dir(&format!("float_truthy_{test}"));
            let obj_path = dir.join("float_truthy.o");
            compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
            let bin_path = dir.join("float_truthy");
            link_object_with_runtime(&obj_path, &bin_path);
            let output = Command::new(&bin_path).output().expect("binary should run");
            assert_eq!(output.stdout, expected.as_bytes(), "test value {test}");
        }
    }

    #[test]
    #[should_panic(expected = "expected an int-or-bool operand, got float")]
    fn an_int_result_binop_with_a_float_operand_hits_to_tagged_int_defensive_panic() {
        // Deliberately malformed MIR: `pycc_types::numeric_result_type`
        // always promotes an expression with any `float` operand to
        // `Ty::Float` (`5 + 1.0` types as `float`, never `int`), so no real
        // pipeline could ever produce a `BinOp { ty: Ty::Int, .. }` with a
        // `float` operand. Exercises `to_tagged_int`'s own defensive
        // `Scalar::Float` arm -- same "hand-construct the otherwise
        // unreachable shape" convention as
        // `printing_a_mistyped_compare_expression_hits_the_internal_consistency_check`
        // and `true_division_binop_codegen_panics_via_its_dedicated_arm`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::FloatLiteral(1.5)),
                    right: Box::new(MirExpr::IntLiteral(1)),
                    ty: Ty::Int,
                },
            })],
        };
        let dir = tempfile_dir("binop_int_result_float_operand_panics");
        let obj_path = dir.join("binop_int_result_float_operand_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "a `Str`-result BinOp is not supported yet")]
    fn a_str_result_binop_is_not_yet_supported() {
        // `pycc_types` can legitimately produce a `Ty::Str`-typed `BinOp`
        // (`"a" + "b"`, string concatenation -- see its own
        // `adding_two_strings_infers_str` test), but reaching *this* arm
        // with that real shape isn't actually possible yet: a real `str`
        // operand (`StringLiteral`/a `str`-typed `Name`) already panics in
        // `emit_expr` before ever reaching this `BinOp` arm's own `ty`
        // dispatch (`Str` codegen is Task 7's job -- see
        // `assigning_a_string_literal_is_not_yet_supported_by_codegen` and
        // `reading_a_str_typed_local_is_not_yet_supported` above). This
        // exercises the `BinOp` arm's own defensive catch-all directly
        // instead, using `int` operands under a mislabeled `ty` -- same
        // "hand-construct the otherwise-unreachable shape" convention as
        // `true_division_binop_codegen_panics_via_its_dedicated_arm` above.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Str,
                },
            })],
        };
        let dir = tempfile_dir("binop_str_result_panics");
        let obj_path = dir.join("binop_str_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_bool_promoted_to_float_in_mixed_arithmetic() {
        // `y = True + 0.5` -- `bool` is `int`-compatible, and any `float`
        // operand promotes the whole expression to `float`
        // (`pycc_types`' `numeric_or_bool_compatible`); exercises
        // `to_float`'s own `Scalar::Bool` arm, not otherwise reached by
        // this task's other fixtures (`compiles_mixed_int_and_float_
        // addition` above only ever passes `to_float` an `Int` or `Float`
        // operand).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::BoolLiteral(true)),
                    right: Box::new(MirExpr::FloatLiteral(0.5)),
                    ty: Ty::Float,
                },
            })],
        };
        let dir = tempfile_dir("bool_float_mixed");
        let obj_path = dir.join("bool_float_mixed.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_the_remaining_float_binop_kinds() {
        // `compiles_true_division_of_two_ints_as_float_arithmetic` already
        // covers `Div`; this exercises every other `BinOpKind` arm under a
        // `Ty::Float` result -- `Add`/`Sub`/`Mul` go through `build_float_*`
        // directly, `FloorDiv`/`Mod`/`Pow` through the `pycc_rt_float_*`
        // runtime calls -- mirroring `compiles_and_runs_add_sub_mul_mod_
        // and_pow_binops`'s `int` coverage. `print(float)` doesn't exist
        // until Task 10 (same limitation as `compiles_true_division_of_
        // two_ints_as_float_arithmetic`/`compiles_mixed_int_and_float_
        // addition` above), so this only proves each arm compiles and
        // verifies, not a runtime stdout value.
        fn float_binop(op: BinOpKind, left: f64, right: f64) -> MirStmt {
            MirStmt::Assign {
                target: format!("{op:?}").to_lowercase(),
                value: MirExpr::BinOp {
                    op,
                    left: Box::new(MirExpr::FloatLiteral(left)),
                    right: Box::new(MirExpr::FloatLiteral(right)),
                    ty: Ty::Float,
                },
            }
        }
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(float_binop(BinOpKind::Add, 3.0, 4.0)),
                MirItem::TopLevelStmt(float_binop(BinOpKind::Sub, 10.0, 3.0)),
                MirItem::TopLevelStmt(float_binop(BinOpKind::Mul, 6.0, 7.0)),
                MirItem::TopLevelStmt(float_binop(BinOpKind::FloorDiv, 7.0, 2.0)),
                MirItem::TopLevelStmt(float_binop(BinOpKind::Mod, 7.0, 2.0)),
                MirItem::TopLevelStmt(float_binop(BinOpKind::Pow, 2.0, 5.0)),
            ],
        };
        let dir = tempfile_dir("float_binops");
        let obj_path = dir.join("float_binops.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_a_mixed_int_and_float_comparison() {
        // `1 < 1.5` -- exercises the `Compare` arm's `left_ty == Ty::Float
        // || right_ty == Ty::Float` promotion check's right-hand disjunct:
        // `left_ty` alone is `Ty::Int` here, so only evaluating `right_ty`
        // decides this comparison promotes to `float` (distinct from
        // `compiles_a_float_comparison`, where `left_ty == Ty::Float`
        // alone already decides it, short-circuiting before `right_ty` is
        // even considered).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "b".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::FloatLiteral(1.5)),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("mixed_cmp");
        let obj_path = dir.join("mixed_cmp.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_the_remaining_float_comparison_operators() {
        // `Lt` already has its own dedicated test above
        // (`compiles_a_float_comparison`); this exercises the rest of
        // `FloatPredicate`'s match arms (`Eq`/`NotEq`/`LtE`/`Gt`/`GtE`),
        // mirroring `compiles_the_remaining_comparison_operators`'s `int`
        // coverage.
        fn assign_compare(target: &str, op: CmpOpKind) -> MirStmt {
            MirStmt::Assign {
                target: target.to_string(),
                value: MirExpr::Compare {
                    op,
                    left: Box::new(MirExpr::FloatLiteral(1.0)),
                    right: Box::new(MirExpr::FloatLiteral(2.0)),
                    ty: Ty::Bool,
                },
            }
        }
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_compare("a", CmpOpKind::Eq)),
                MirItem::TopLevelStmt(assign_compare("b", CmpOpKind::NotEq)),
                MirItem::TopLevelStmt(assign_compare("c", CmpOpKind::LtE)),
                MirItem::TopLevelStmt(assign_compare("d", CmpOpKind::Gt)),
                MirItem::TopLevelStmt(assign_compare("e", CmpOpKind::GtE)),
            ],
        };
        let dir = tempfile_dir("remaining_float_cmp_ops");
        let obj_path = dir.join("remaining_float_cmp_ops.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    fn tempfile_dir(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pycc_codegen_test_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Test-only linking helper. `pycc`'s real CLI (Task 8) does this via
    /// `cc`/clang (see `src/main.rs`'s `linker_command`/`effective_link_target`/
    /// `add_windows_system_libs`); duplicated minimally here so
    /// pycc_codegen's own tests can prove the object file it produces
    /// actually links and runs, without depending on the `pycc` binary
    /// crate (that would be a dependency cycle: pycc depends on
    /// pycc_codegen, not the other way around). Needs the same Windows
    /// handling as `main.rs`, and for the same reasons: there's no default
    /// `cc` there (D-028) -- on this runner it silently resolved to
    /// MinGW's `gcc`, which cannot link the MSVC-ABI `pycc_rt.lib` (the
    /// exact "undefined reference to `__imp_...`"/`collect2` wall D-028
    /// already diagnosed for `main.rs`, reproduced here because this
    /// helper wasn't covered by that fix); clang's bare-invocation default
    /// target also proved unreliable (D-028), so `-target` must be
    /// explicit too.
    fn link_object_with_runtime(obj_path: &std::path::Path, bin_path: &std::path::Path) {
        let rt_lib_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");

        #[cfg(windows)]
        let mut cmd = {
            let clang = std::path::Path::new(env!("LLVM_SYS_221_PREFIX"))
                .join("bin")
                .join("clang.exe");
            let mut cmd = Command::new(clang);
            cmd.arg("-target").arg("x86_64-pc-windows-msvc");
            cmd
        };
        #[cfg(not(windows))]
        let mut cmd = Command::new("cc");

        cmd.arg(obj_path)
            .arg("-L")
            .arg(&rt_lib_dir)
            .arg("-lpycc_rt")
            .arg("-o")
            .arg(bin_path);

        #[cfg(windows)]
        for lib in [
            "ws2_32",
            "ntdll",
            "userenv",
            "advapi32",
            "shell32",
            "ole32",
            "uuid",
            "psapi",
            "dbghelp",
            "kernel32",
            "legacy_stdio_definitions",
        ] {
            cmd.arg(format!("-l{lib}"));
        }

        let status = cmd.status().expect("the linker driver should run");
        assert!(status.success(), "linking failed");
    }
}
