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

/// One MIR-level value during codegen. Extended (never replaced) by later
/// tasks: `Str` (Task 7) is a pointer to an opaque `pycc_rt::PyStrObj` --
/// `pycc_codegen` never inspects its layout (D-059's inline/heap
/// representation is entirely `pycc_rt`'s own concern), only ever passing
/// it through to a `pycc_rt_str_*` call. `Ty::None` never needs a variant
/// here -- no v0.1 `MirExpr` can actually construct a `None` *value* (see
/// Task 6's note).
enum Scalar<'ctx> {
    /// Tagged per D-061. Always LLVM `i64`.
    Int(IntValue<'ctx>),
    /// `0`/`1`, LLVM `i8` -- not `i1` (D-061's ABI note: this project has
    /// already hit real cross-platform storage/parameter footguns for
    /// sub-byte types, see D-027/D-028/D-029; `i1` is used only
    /// transiently for a `br` condition or an `icmp`/`fcmp` result,
    /// immediately zero-extended to `i8` before it's stored anywhere).
    Bool(IntValue<'ctx>),
    /// A plain, untagged LLVM `f64` -- unlike `int`, `float` needs no
    /// tagging scheme (D-061's tagged-fixnum representation is specific
    /// to `int`'s own overflow/bigint-promotion story); every `float`
    /// value is exactly one `f64`, always (Task 6).
    Float(FloatValue<'ctx>),
    /// A pointer to a heap-allocated `pycc_rt::PyStrObj` (D-059/D-060,
    /// Task 7) -- always refcounted, never inspected directly by this
    /// crate (see this enum's own doc comment).
    Str(PointerValue<'ctx>),
}

/// Every `pycc_rt` function this crate calls, declared once in
/// `compile_to_object` and threaded through `emit_stmt`/`emit_expr`.
/// Extended (never replaced) by Tasks 8/9/10 as they add more `pycc_rt`
/// declarations.
struct RtFns<'ctx> {
    int_add: FunctionValue<'ctx>,
    int_sub: FunctionValue<'ctx>,
    int_mul: FunctionValue<'ctx>,
    int_floordiv: FunctionValue<'ctx>,
    int_floormod: FunctionValue<'ctx>,
    int_pow: FunctionValue<'ctx>,
    int_cmp: FunctionValue<'ctx>,
    int_truthy: FunctionValue<'ctx>,
    range_continue: FunctionValue<'ctx>,
    int_to_float: FunctionValue<'ctx>,
    float_floordiv: FunctionValue<'ctx>,
    float_floormod: FunctionValue<'ctx>,
    float_pow: FunctionValue<'ctx>,
    str_from_literal: FunctionValue<'ctx>,
    str_concat: FunctionValue<'ctx>,
    str_cmp: FunctionValue<'ctx>,
    str_truthy: FunctionValue<'ctx>,
    str_incref: FunctionValue<'ctx>,
    str_decref: FunctionValue<'ctx>,
    int_to_str: FunctionValue<'ctx>,
    float_to_str: FunctionValue<'ctx>,
    bool_to_str: FunctionValue<'ctx>,
    print_write_str: FunctionValue<'ctx>,
    print_space: FunctionValue<'ctx>,
    print_newline: FunctionValue<'ctx>,
    print_none: FunctionValue<'ctx>,
}

fn declare_rt_functions<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> RtFns<'ctx> {
    let i64_type = context.i64_type();
    let i32_type = context.i32_type();
    let void_type = context.void_type();
    let f64_type = context.f64_type();
    let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
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
        str_from_literal: declare(
            "pycc_rt_str_from_literal",
            ptr_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        str_concat: declare(
            "pycc_rt_str_concat",
            ptr_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
        ),
        str_cmp: declare(
            "pycc_rt_str_cmp",
            i32_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
        ),
        str_truthy: declare(
            "pycc_rt_str_truthy",
            context.i8_type().fn_type(&[ptr_type.into()], false),
        ),
        str_incref: declare("pycc_rt_str_incref", void_type.fn_type(&[ptr_type.into()], false)),
        str_decref: declare("pycc_rt_str_decref", void_type.fn_type(&[ptr_type.into()], false)),
        int_to_str: declare("pycc_rt_int_to_str", ptr_type.fn_type(&[i64_type.into()], false)),
        float_to_str: declare("pycc_rt_float_to_str", ptr_type.fn_type(&[f64_type.into()], false)),
        bool_to_str: declare(
            "pycc_rt_bool_to_str",
            ptr_type.fn_type(&[context.i8_type().into()], false),
        ),
        print_write_str: declare("pycc_rt_print_write_str", void_type.fn_type(&[ptr_type.into()], false)),
        print_space: declare("pycc_rt_print_space", void_type.fn_type(&[], false)),
        print_newline: declare("pycc_rt_print_newline", void_type.fn_type(&[], false)),
        print_none: declare("pycc_rt_print_none", void_type.fn_type(&[], false)),
    }
}

/// Mirrors `pycc_rt::tag_smallint` exactly (compile-time constant folding
/// of the same encoding, see D-061) -- an `int` literal whose magnitude
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
        pycc_mir::Ty::Str => context.ptr_type(inkwell::AddressSpace::default()).into(),
        other => panic!("pycc_codegen: a `{other:?}`-typed parameter/return value is not supported yet"),
    }
}

/// `bool` is an `int` subtype (Python/`pycc_types`'
/// `numeric_or_bool_compatible`) -- widens a `Bool` scalar to a tagged
/// `int` (D-061) via two trivial, unambiguous LLVM instructions (a
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
        Scalar::Str(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got str")
        }
    }
}

/// Promotes any numeric `Scalar` to `f64`: an existing `Float` passes
/// through; `Int` goes through `pycc_rt_int_to_float` (never a raw LLVM
/// cast -- the value is D-061-tagged, so only `pycc_rt` may interpret its
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
        Scalar::Str(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got str")
        }
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

/// Converts any scalar to a fresh, owned `str` object matching CPython's
/// `str(x)` for that value (Task 8) -- reused unchanged by Task 10's
/// `print`. `str` itself passes through (already a `str`, and per this
/// function's own contract already an owned reference by the time it gets
/// here -- see `emit_expr`'s `FString` arm, the only caller); every other
/// type goes through its own `pycc_rt_*_to_str` conversion.
///
/// Two deviations from the task brief, both here:
///
/// 1. The brief's own version of this function took a `context: &'ctx
///    Context` parameter (matching `to_tagged_int`/`to_float`'s own
///    shape) -- but no branch below actually needs it: unlike `to_float`'s
///    `Bool` case (which builds a real `context`-dependent conversion
///    instruction), every branch here only ever picks which already-built
///    `RtFns` field and value to call `build_call` with, and `build_call`
///    itself needs no `Context` at all. An unused parameter would fail
///    this crate's `-D warnings` (`unused_variables` applies to
///    parameters, not just local bindings). Dropped rather than
///    underscore-prefixed, since it's not merely unused in *this* body but
///    structurally unneeded by this function's own job -- matching this
///    file's established convention of removing what's provably
///    unnecessary rather than working around it (see e.g. `emit_expr`'s
///    own `module`-parameter history note).
/// 2. The brief's own version of this function's final `build_call` ended
///    `.try_as_basic_value().left().expect(msg)` -- but (per the doc
///    comment on `emit_expr`'s `BinOp` arm) this inkwell version's
///    `try_as_basic_value()` returns its own `ValueKind` enum, not
///    `either::Either`, so `.left()` doesn't exist on it and the brief's
///    code as written doesn't compile. Fixed to `.expect_basic(msg)`, the
///    same fix already applied throughout this file (e.g.
///    `build_float_rt_binop` above).
fn to_str<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
) -> PointerValue<'ctx> {
    let (rt_fn, arg): (FunctionValue<'ctx>, inkwell::values::BasicMetadataValueEnum) = match scalar
    {
        Scalar::Str(v) => return v,
        Scalar::Int(v) => (rt.int_to_str, v.into()),
        Scalar::Float(v) => (rt.float_to_str, v.into()),
        Scalar::Bool(v) => (rt.bool_to_str, v.into()),
    };
    builder
        .build_call(rt_fn, &[arg], "to_str")
        .expect("build_call should not fail for a well-formed conversion")
        .try_as_basic_value()
        .expect_basic("every pycc_rt_*_to_str function returns a non-void pointer")
        .into_pointer_value()
}

/// Builds a `str` object from a compile-time literal's bytes (Task 7),
/// embedding them as a private constant global and calling
/// `pycc_rt_str_from_literal` -- never null-terminated, since a Python
/// `str` can contain an embedded `\0`, so the byte length is always passed
/// explicitly rather than relying on termination.
///
/// Extracted (Task 8) out of `emit_expr`'s own `MirExpr::StringLiteral` arm
/// below so `MirExpr::FString`'s `Literal`-part case can build the exact
/// same `str` object directly, as a `PointerValue`, without going through
/// `emit_expr`'s general `&MirExpr -> Scalar` dispatch just to immediately
/// pattern-match the `Scalar::Str` back out again. That defensive
/// pattern-match (the task brief's own version used a `let Scalar::Str(ptr)
/// = emit_expr(..., &MirExpr::StringLiteral(s.clone())) else { unreachable!
/// (...) }`) is *provably* dead code given this function's own contract --
/// `MirExpr::StringLiteral` always evaluates to `Scalar::Str`, unconditionally,
/// nothing else ever calls this helper with a different result type -- and
/// `cargo llvm-cov`'s region coverage confirmed it empirically: that
/// `unreachable!()` arm's region never executed under any test, an
/// uncovered region under this project's 100%-line-and-region coverage gate
/// (D-014). Removed by sharing this typed-`PointerValue`-returning helper
/// instead, the same "no redundant impossible-to-cover branch" convention
/// this file already applies elsewhere (see `emit_expr`'s `Name` arm's own
/// doc comment).
fn emit_string_literal<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    s: &str,
) -> PointerValue<'ctx> {
    let bytes = s.as_bytes();
    let array_ty = context.i8_type().array_type(bytes.len() as u32);
    let global = module.add_global(array_ty, None, "str_lit");
    global.set_initializer(&context.const_string(bytes, false));
    global.set_constant(true);
    global.set_linkage(Linkage::Private);
    let ptr = global.as_pointer_value();
    let len = context.i64_type().const_int(bytes.len() as u64, false);
    builder
        .build_call(rt.str_from_literal, &[ptr.into(), len.into()], "str_lit_obj")
        .expect("build_call should not fail for a well-formed string literal construction")
        .try_as_basic_value()
        .expect_basic("pycc_rt_str_from_literal returns a non-void pointer")
        .into_pointer_value()
}

#[allow(clippy::too_many_arguments)]
fn emit_expr<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    // Tasks 3-6 only ever passed this to `emit_expr`'s own recursive calls
    // (clippy's `only_used_in_recursion` lint, part of `-D warnings`,
    // required an underscore-prefixed `_module` name for that shape). Task
    // 7's `MirExpr::StringLiteral` arm below is the first to read it
    // directly (`module.add_global`, to embed a literal's bytes as a
    // constant), so it's no longer only-used-in-recursion -- dropping the
    // underscore.
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    // Every module-level binding's real (non-stack) LLVM global storage
    // (see `compile_to_object`'s own `collect_module_globals` pass) --
    // consulted by the `Name` arm only as a *fallback* when `name` isn't
    // already in `locals`, never merged into `locals` itself. This is what
    // lets a function read a module global it does not itself assign
    // without conflating that read with a same-named function-local
    // variable: `locals` alone always wins when present (D-055 shadowing,
    // mirrored on the `pycc_mir` side by `collect_function_local_names`),
    // and a name absent from both was already rejected before this HIR
    // reached codegen.
    module_globals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    expr: &MirExpr,
) -> Scalar<'ctx> {
    use pycc_mir::Ty;
    match expr {
        MirExpr::IntLiteral(n) => Scalar::Int(tag_smallint_const(context, *n)),
        MirExpr::FloatLiteral(f) => Scalar::Float(context.f64_type().const_float(*f)),
        MirExpr::StringLiteral(s) => Scalar::Str(emit_string_literal(context, builder, module, rt, s)),
        MirExpr::Name { name, ty } => {
            let (ptr, local_ty) = locals
                .get(name)
                .or_else(|| module_globals.get(name))
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
                Ty::Str => {
                    let loaded = builder
                        .build_load(
                            context.ptr_type(inkwell::AddressSpace::default()),
                            *ptr,
                            "load",
                        )
                        .expect("build_load should not fail for a slot this function itself allocated");
                    Scalar::Str(loaded.into_pointer_value())
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
            let l = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, left);
            let r = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, right);
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
                Ty::Str => {
                    let Scalar::Str(l) = l else {
                        panic!("pycc_codegen: internal error: str BinOp operand did not evaluate to str")
                    };
                    let Scalar::Str(r) = r else {
                        panic!("pycc_codegen: internal error: str BinOp operand did not evaluate to str")
                    };
                    if *op != pycc_mir::BinOpKind::Add {
                        panic!("pycc_codegen: `str {op:?} str` is not supported yet (only concatenation is)");
                    }
                    let result = builder
                        .build_call(rt.str_concat, &[l.into(), r.into()], "str_concat")
                        .expect("build_call should not fail for a well-formed concatenation")
                        .try_as_basic_value()
                        .expect_basic("pycc_rt_str_concat returns a non-void pointer");
                    Scalar::Str(result.into_pointer_value())
                }
                other => panic!("pycc_codegen: a `{other:?}`-result BinOp is not supported yet"),
            }
        }
        MirExpr::Compare { op, left, right, .. } => {
            let left_ty = left.ty();
            let right_ty = right.ty();
            let l = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, left);
            let r = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, right);
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
            } else if left_ty == Ty::Str || right_ty == Ty::Str {
                let Scalar::Str(l) = l else {
                    panic!("pycc_codegen: internal error: str Compare operand did not evaluate to str")
                };
                let Scalar::Str(r) = r else {
                    panic!("pycc_codegen: internal error: str Compare operand did not evaluate to str")
                };
                let ordering = builder
                    .build_call(rt.str_cmp, &[l.into(), r.into()], "str_cmp")
                    .expect("build_call should not fail for a well-formed comparison")
                    .try_as_basic_value()
                    .expect_basic("pycc_rt_str_cmp returns a non-void `i32`")
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
                    .build_int_compare(predicate, ordering, zero, "str_cmp_pred")
                    .expect("build_int_compare should not fail for two i32 operands");
                builder
                    .build_int_z_extend(cond, context.i8_type(), "bool_from_str_cmp")
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
            let call_site = build_call_to(context, builder, module, rt, user_functions, locals, module_globals, f, args);
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
                Ty::Str => Scalar::Str(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("this function is declared to return str")
                        .into_pointer_value(),
                ),
                Ty::None => {
                    // A `None`-returning call's LLVM function returns
                    // `void` -- there is no value to extract, and this
                    // crate has no `Scalar::None` (Task 6's finding: no
                    // v0.1 expression can construct a `None` *value* other
                    // than this exact call-result shape). The only caller
                    // that ever evaluates a `Ty::None`-typed expression is
                    // `emit_stmt`'s `print` dispatch below, which discards
                    // this placeholder and prints the literal `"None"`
                    // instead of using it.
                    Scalar::Bool(context.i8_type().const_int(0, false))
                }
                other => panic!("pycc_codegen: a `{other:?}`-typed call result is not supported yet"),
            }
        }
        MirExpr::FString(parts) => {
            // Two deviations from the task brief, both here:
            //
            // 1. The brief's own version of this arm's `str_concat` call
            //    ended `.try_as_basic_value().left().expect(msg)` -- same
            //    `.left()`-doesn't-exist-on-this-inkwell-version issue
            //    documented on `to_str` and `BinOp` above, fixed the same
            //    way (`.expect_basic(msg)`).
            // 2. The brief's own `Literal`-part case called `emit_expr`
            //    recursively on a synthetic `MirExpr::StringLiteral(s.
            //    clone())` and then defensively pattern-matched the
            //    `Scalar::Str` back out with a `let ... else {
            //    unreachable!(...) }`. That `unreachable!()` arm is
            //    *provably* dead code (confirmed by `cargo llvm-cov`: an
            //    uncovered region under D-014's 100% gate) -- `MirExpr::
            //    StringLiteral` always evaluates to `Scalar::Str`, nothing
            //    else. Fixed by calling `emit_string_literal` (extracted
            //    from `emit_expr`'s own `StringLiteral` arm) directly,
            //    which returns the `PointerValue` itself with no `Scalar`
            //    round-trip and so no impossible arm to cover -- see that
            //    helper's own doc comment.
            let mut acc: Option<PointerValue<'ctx>> = None;
            for part in parts {
                let part_str = match part {
                    pycc_mir::MirFStringPart::Literal(s) => {
                        emit_string_literal(context, builder, module, rt, s)
                    }
                    pycc_mir::MirFStringPart::Interpolation(inner) => {
                        // Mirrors `emit_print_arg`'s own `Ty::None` special
                        // case (see its doc comment): a `None`-typed
                        // interpolated expression is only ever reachable as
                        // a direct `Call` result (Task 6/10's scope note --
                        // there is no `MirExpr::NoneLiteral`), and
                        // `emit_expr`'s `Call` arm returns a placeholder
                        // `Scalar::Bool(0)` for it that is never meant to be
                        // read as a real value. Before this fix, that
                        // placeholder flowed straight into `to_str`, which
                        // has no way to distinguish it from a genuine
                        // `False` -- interpolating a `None`-returning call
                        // rendered `"False"` instead of `"None"`. Embeds the
                        // literal text "None" directly (never dynamic, so
                        // no dedicated `pycc_rt` conversion function is
                        // needed) after still evaluating `inner` for its
                        // side effect (the call itself must still run).
                        if inner.ty() == pycc_mir::Ty::None {
                            if !matches!(inner.as_ref(), MirExpr::Call { .. }) {
                                panic!(
                                    "pycc_codegen: interpolating a `None`-typed value that isn't \
                                     a direct call result is not supported yet"
                                );
                            }
                            emit_expr(context, builder, module, rt, user_functions, locals, module_globals, inner);
                            emit_string_literal(context, builder, module, rt, "None")
                        } else {
                            let scalar =
                                emit_expr(context, builder, module, rt, user_functions, locals, module_globals, inner);
                            let scalar = incref_if_str_duplicate(builder, rt, inner, scalar);
                            to_str(builder, rt, scalar)
                        }
                    }
                };
                acc = Some(match acc {
                    None => part_str,
                    Some(prev) => {
                        let joined = builder
                            .build_call(rt.str_concat, &[prev.into(), part_str.into()], "fstring_concat")
                            .expect("build_call should not fail for a well-formed concatenation")
                            .try_as_basic_value()
                            .expect_basic("pycc_rt_str_concat returns a non-void pointer")
                            .into_pointer_value();
                        builder
                            .build_call(rt.str_decref, &[prev.into()], "fstring_decref_prev")
                            .expect("build_call should not fail for a well-formed decref");
                        builder
                            .build_call(rt.str_decref, &[part_str.into()], "fstring_decref_part")
                            .expect("build_call should not fail for a well-formed decref");
                        joined
                    }
                });
            }
            Scalar::Str(acc.unwrap_or_else(|| {
                // An empty `FString(vec![])` never actually reaches this arm --
                // `pycc_hir`'s own f-string lowering always produces at least
                // one `Literal` part (an empty Python f-string `f""` still
                // lowers to `FString(vec![FStringPart::Literal("")])`, never a
                // truly empty `Vec`) -- but guard it explicitly rather than
                // silently returning a dangling/null pointer if that assumption
                // is ever wrong.
                panic!("pycc_codegen: internal error: an f-string with zero parts should not be reachable")
            }))
        }
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
    module_globals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    f: FunctionValue<'ctx>,
    args: &[MirExpr],
) -> inkwell::values::CallSiteValue<'ctx> {
    let param_types = f.get_type().get_param_types();
    let arg_values: Vec<inkwell::values::BasicMetadataValueEnum> = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, a);
            let scalar = incref_if_str_duplicate(builder, rt, a, scalar);
            // `bool` is an `int` subtype (`pycc_types::is_assignable`) -- a
            // `bool`-typed argument passed where the callee's parameter is
            // declared `int` needs the same D-061 tagging `to_tagged_int`
            // applies elsewhere, or the built call's argument type would
            // not match the callee's own declared signature (`f`'s
            // parameter types, queried directly from its already-built
            // `FunctionType` rather than threading `pycc_mir::Ty` params
            // through every call site).
            let needs_widening = matches!(scalar, Scalar::Bool(_))
                && param_types.get(i).is_some_and(|pt| *pt == context.i64_type().into());
            let scalar =
                if needs_widening { Scalar::Int(to_tagged_int(context, builder, scalar)) } else { scalar };
            match scalar {
                Scalar::Int(v) => v.into(),
                Scalar::Bool(v) => v.into(),
                Scalar::Float(v) => v.into(),
                Scalar::Str(v) => v.into(),
            }
        })
        .collect();
    builder
        .build_call(f, &arg_values, "call_user_fn")
        .expect("build_call should not fail for a well-formed user function call")
}

/// Turns any supported `Scalar` into an LLVM `i1` for use as a `br`
/// condition -- the shared truthiness check behind `if`/`while` (Task 4),
/// now including `str` (Task 7): `False` only for the empty string,
/// delegated to `pycc_rt_str_truthy` (D-059's representation is opaque to
/// this crate).
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
        Scalar::Str(v) => builder
            .build_call(rt.str_truthy, &[v.into()], "str_truthy")
            .expect("build_call should not fail for a well-formed truthiness check")
            .try_as_basic_value()
            .expect_basic("pycc_rt_str_truthy returns a non-void i8")
            .into_int_value(),
    };
    builder
        .build_int_compare(IntPredicate::NE, as_i8, context.i8_type().const_int(0, false), "truthy")
        .expect("build_int_compare should not fail comparing two i8 operands")
}

/// Allocates a pointer-typed slot for a `str` local in the *entry* block of
/// the function currently being emitted -- never wherever `builder` happens
/// to be positioned -- and stores an explicit null pointer into it there,
/// before restoring `builder`'s original position. Fixes a review finding
/// against this file's first `str`-codegen task: `emit_assign`'s general
/// path (below) builds a local's backing `alloca` at the *current* builder
/// position, which is fine for `Int`/`Bool`/`Float` (nothing outside this
/// task ever reads a local's slot from a block the assignment doesn't
/// dominate), but is wrong for `str`: this file also reads a `str` local's
/// slot from *outside* the assignment's own block, at two sites --
/// `decref_old_str_if_reassigning`'s reassignment load, and
/// `compile_to_object`'s top-level-locals completion-decref loop right
/// before `main` returns. If a `str` local's *first* assignment happens
/// inside an `if`/`while`/`for` body, its `alloca` would live in that
/// branch's own block, which does not dominate the merge/after block (or,
/// for a top-level local, the point right before `main`'s `build_return`)
/// -- `module.verify()` correctly rejects the resulting IR, and on Windows
/// (where `verify_module` is a no-op, see D-029) the malformed IR would
/// reach LLVM directly. The entry block dominates every other block in its
/// function by construction, so an `alloca` placed there -- at any position
/// within it, since entry has no predecessor and everything else in the
/// function is only reachable through it -- dominates every later read
/// regardless of which nested block the local's first real assignment
/// happens to execute in. The explicit null store (rather than leaving the
/// slot's initial value as LLVM `undef`) is what lets the null guard
/// already built into `pycc_rt_str_incref`/`pycc_rt_str_decref` safely no-op
/// on a path that never reaches this local's assignment at all -- an
/// `undef` load, unlike a real null, has no defined value a guard could
/// check.
fn alloca_str_at_entry<'ctx>(context: &'ctx Context, builder: &inkwell::builder::Builder<'ctx>) -> PointerValue<'ctx> {
    // Deliberately not built on top of `alloca_at_entry` below: that helper
    // repositions the builder back to the *original* (possibly nested,
    // conditionally-executed) block before returning, so a null store
    // issued after calling it would land in the wrong block -- silently
    // skipping the store on any path that doesn't happen to execute that
    // original block, exactly the bug an earlier draft of this
    // refactor introduced (caught by
    // `a_str_local_never_assigned_on_the_taken_path_decrefs_a_clean_null_
    // at_completion`, which crashed instead of cleanly no-oping once the
    // store silently stopped running at entry). The null store here must
    // execute unconditionally at entry, so this function positions there
    // itself and keeps both the `alloca` and the store inside that window.
    let current_block = builder
        .get_insert_block()
        .expect("builder is always positioned inside some block while a statement is being emitted");
    let function = current_block
        .get_parent()
        .expect("the block builder is currently positioned in always belongs to a function");
    let entry_block = function
        .get_first_basic_block()
        .expect("compile_to_object always appends a function's entry block before emitting its body");
    match entry_block.get_terminator() {
        Some(terminator) => builder.position_before(&terminator),
        None => builder.position_at_end(entry_block),
    }
    let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
    let ptr = builder
        .build_alloca(ptr_type, "str_slot")
        .expect("build_alloca should not fail for a pointer-typed slot");
    builder
        .build_store(ptr, ptr_type.const_null())
        .expect("build_store should not fail immediately after this function's own alloca");
    builder.position_at_end(current_block);
    ptr
}

/// Hoists an `alloca` for a local's *first* assignment to the enclosing
/// function's entry block instead of building it at the current position --
/// shared by `alloca_str_at_entry` (which additionally stores an initial
/// null, see its own doc comment for why that guard is `str`-specific) and
/// every other scalar type's first assignment in `emit_assign` below. This
/// hoist is not an optimization: a first assignment can lexically occur
/// inside an `if`/`while`/`for` body, and this file's `locals` map is shared
/// across sibling branches and any code following the enclosing control-flow
/// statement -- an `alloca` built at that nested position would not
/// dominate a later read or a sibling branch's reuse of the same slot,
/// which `module.verify()` rejects as invalid IR regardless of scalar type
/// (see `an_int_local_first_assigned_inside_an_if_body_is_readable_after_
/// the_if`, which panicked exactly this way before this fix generalized the
/// `str`-only hoist that already existed here).
fn alloca_at_entry<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    ty: inkwell::types::BasicTypeEnum<'ctx>,
    name: &str,
) -> PointerValue<'ctx> {
    let current_block = builder
        .get_insert_block()
        .expect("builder is always positioned inside some block while a statement is being emitted");
    let function = current_block
        .get_parent()
        .expect("the block builder is currently positioned in always belongs to a function");
    let entry_block = function
        .get_first_basic_block()
        .expect("compile_to_object always appends a function's entry block before emitting its body");
    match entry_block.get_terminator() {
        Some(terminator) => builder.position_before(&terminator),
        None => builder.position_at_end(entry_block),
    }
    let ptr = builder
        .build_alloca(ty, name)
        .expect("build_alloca should not fail for a supported scalar type");
    builder.position_at_end(current_block);
    ptr
}

/// Allocates (on first assignment) or reuses (on reassignment) the
/// `alloca` backing `target`, stores `value` into it, and records/updates
/// its entry in `locals`. A local's `Ty` never changes across
/// reassignment (`pycc_types`' sticky-first-type rule, T0023, ties one
/// static type to each binding forever) -- but the *value* being stored on
/// a reassignment can still be a legally narrower `Scalar` than that sticky
/// type (`bool` is `int`-assignable), since `pycc_mir` reports every read
/// of the local using its original type, never the latest assignment's own
/// type. Reusing an existing slot therefore widens a `Scalar::Bool` value
/// into a tagged `int` (D-061) when the local's own recorded type is
/// `Int`, matching the widening `emit_expr`'s `Name` arm will use to read
/// it back -- the only other combination possible here, since
/// `pycc_types::check_assignment` already rejected anything else before
/// this HIR could reach codegen.
///
/// A `str` local's *first* assignment hoists its `alloca` to the function's
/// entry block instead of building it at the current position -- see
/// `alloca_str_at_entry`'s own doc comment for why this one `Scalar`
/// variant needs different treatment here.
fn emit_assign<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    target: &str,
    ty: pycc_mir::Ty,
    value: Scalar<'ctx>,
) {
    let (ptr, value) = match locals.get(target) {
        Some((ptr, local_ty)) => {
            let value = match (local_ty, &value) {
                (pycc_mir::Ty::Int, Scalar::Bool(_)) => Scalar::Int(to_tagged_int(context, builder, value)),
                _ => value,
            };
            (*ptr, value)
        }
        None => {
            let ptr = match &value {
                Scalar::Str(_) => alloca_str_at_entry(context, builder),
                Scalar::Int(v) => alloca_at_entry(builder, v.get_type().into(), target),
                Scalar::Bool(v) => alloca_at_entry(builder, v.get_type().into(), target),
                Scalar::Float(v) => alloca_at_entry(builder, v.get_type().into(), target),
            };
            locals.insert(target.to_string(), (ptr, ty));
            (ptr, value)
        }
    };
    let basic_value: inkwell::values::BasicValueEnum = match value {
        Scalar::Int(v) => v.into(),
        Scalar::Bool(v) => v.into(),
        Scalar::Float(v) => v.into(),
        Scalar::Str(v) => v.into(),
    };
    builder
        .build_store(ptr, basic_value)
        .expect("build_store should not fail for a slot this function itself allocated");
}

/// Whether evaluating `expr` produces a *duplicate* reference to an
/// already-owned `str` (a bare variable read) rather than a fresh object
/// owning exactly one reference from its own construction. v0.1's grammar
/// makes this purely syntactic: every str-producing expression other than a
/// bare `Name` (`StringLiteral`, string concatenation, a `Call`'s return
/// value) freshly constructs its result and already owns exactly one
/// reference (D-060, Task 7).
fn str_value_is_a_duplicate_reference(expr: &MirExpr) -> bool {
    matches!(expr, MirExpr::Name { .. })
}

/// Increments a `str` scalar's refcount when `source_expr` is a bare
/// variable read (see `str_value_is_a_duplicate_reference`) -- binding a
/// second owning reference to the same `PyStrObj` without this would leave
/// the original binding's own eventual decref underflowing the refcount
/// (D-060, Task 7). A no-op for every non-`Str` scalar.
fn incref_if_str_duplicate<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    source_expr: &MirExpr,
    scalar: Scalar<'ctx>,
) -> Scalar<'ctx> {
    if let Scalar::Str(ptr) = scalar {
        if str_value_is_a_duplicate_reference(source_expr) {
            builder
                .build_call(rt.str_incref, &[ptr.into()], "str_incref")
                .expect("build_call should not fail for a well-formed incref");
        }
        Scalar::Str(ptr)
    } else {
        scalar
    }
}

/// Only meaningful for `Ty::Str` targets: if `target` already has a slot in
/// `locals` (this `Assign` is a reassignment, not a first binding), loads
/// its current value and decrefs it before the new value overwrites it --
/// otherwise reassigning a `str` local in a loop would leak its previous
/// value every iteration (D-060/D-061, Task 7).
fn decref_old_str_if_reassigning<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    target: &str,
) {
    if let Some((slot_ptr, pycc_mir::Ty::Str)) = locals.get(target) {
        let old = builder
            .build_load(context.ptr_type(inkwell::AddressSpace::default()), *slot_ptr, "old_str")
            .expect("build_load should not fail for this function's own alloca")
            .into_pointer_value();
        builder
            .build_call(rt.str_decref, &[old.into()], "str_decref_old")
            .expect("build_call should not fail for a well-formed decref");
    }
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
#[allow(clippy::too_many_arguments)]
fn emit_body<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    module_globals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    body: &[MirStmt],
) -> Result<(), String> {
    for stmt in body {
        emit_stmt(context, builder, module, rt, user_functions, locals, module_globals, stmt)?;
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
    module_globals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    body: &[MirStmt],
    dest: inkwell::basic_block::BasicBlock<'ctx>,
) -> Result<(), String> {
    emit_body(context, builder, module, rt, user_functions, locals, module_globals, body)?;
    if builder.get_insert_block().unwrap().get_terminator().is_none() {
        builder
            .build_unconditional_branch(dest)
            .expect("build_unconditional_branch should not fail on a block with no terminator yet");
    }
    Ok(())
}

/// Every module-level name bound in file order, paired with its
/// sticky-first type: `value.ty()` (for an `Assign`) or `Ty::Int` (for a
/// `ForRange` loop variable) at the point of its *first* occurrence only --
/// `pycc_mir`'s own sticky-first-type binding (T0023) guarantees a later
/// reassignment's `value.ty()` is only ever a narrower, `is_assignable` type
/// into this same original one (e.g. `bool` into `int`), never a genuinely
/// different one, so the first occurrence alone determines each global's
/// real storage type. Recurses into `If`/`While`/`ForRange` bodies: Python
/// has no separate block scope, so a name first bound inside a top-level
/// `if`/`while`/`for` is still a module-level global exactly like one bound
/// directly at the top level.
fn collect_module_globals(items: &[MirItem]) -> Vec<(String, pycc_mir::Ty)> {
    fn scan(
        stmts: &[MirStmt],
        order: &mut Vec<(String, pycc_mir::Ty)>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        for stmt in stmts {
            match stmt {
                MirStmt::Assign { target, value } => {
                    if seen.insert(target.clone()) {
                        order.push((target.clone(), value.ty()));
                    }
                }
                MirStmt::ForRange { var, body, .. } => {
                    if seen.insert(var.clone()) {
                        order.push((var.clone(), pycc_mir::Ty::Int));
                    }
                    scan(body, order, seen);
                }
                MirStmt::If { body, orelse, .. } => {
                    scan(body, order, seen);
                    scan(orelse, order, seen);
                }
                MirStmt::While { body, .. } => scan(body, order, seen),
                MirStmt::ExprStmt(_) | MirStmt::Return(_) => {}
            }
        }
    }
    let mut order = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if let MirItem::TopLevelStmt(stmt) = item {
            scan(std::slice::from_ref(stmt), &mut order, &mut seen);
        }
    }
    order
}

/// A module-level global's placeholder initial value, before any top-level
/// statement has actually run -- never legitimately observed by a
/// well-formed program (module-level code always executes, in file order,
/// before `main` calls any function that might read one of these globals),
/// but chosen to be a genuinely valid value for its type regardless, rather
/// than relying on that guarantee: a properly tagged fixnum zero for `int`
/// (D-061 -- an untagged `0` is not a valid tagged-int bit pattern), a real
/// null for `str` (matching `alloca_str_at_entry`'s own null-guard
/// convention, safe for `pycc_rt_str_decref`'s null guard to no-op on).
/// `None` for any other `Ty` (`None`/`Infer`, matching this file's other
/// "not a real runtime representation" catch-alls) -- deliberately not a
/// panic: several of this file's own hand-crafted, deliberately malformed
/// "unreachable via any real pipeline" tests build a top-level `Assign`
/// with exactly such a `ty` (see e.g. `a_none_result_binop_is_not_yet_
/// supported`) specifically to reach a *different*, deeper defensive panic
/// elsewhere in `emit_expr`/`emit_stmt`; this function's caller (the global
/// declaration pass in `compile_to_object`) must let those cases fall
/// through with no pre-declared global at all -- exactly `HashMap::new()`'s
/// original starting behavior for `top_level_locals`, before this task
/// introduced module globals at all -- rather than front-run them with a
/// premature panic of its own.
fn zero_initializer<'ctx>(context: &'ctx Context, ty: pycc_mir::Ty) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
    match ty {
        pycc_mir::Ty::Int => Some(tag_smallint_const(context, 0).into()),
        pycc_mir::Ty::Bool => Some(context.i8_type().const_int(0, false).into()),
        pycc_mir::Ty::Float => Some(context.f64_type().const_float(0.0).into()),
        pycc_mir::Ty::Str => Some(context.ptr_type(inkwell::AddressSpace::default()).const_null().into()),
        _ => None,
    }
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

    // Every module-level binding gets real (non-stack) LLVM global storage,
    // declared before any statement is emitted -- unlike a plain local's
    // `alloca`, a global is valid to read from *any* function, which is
    // what lets a function body read a module-level global it does not
    // itself assign (see `emit_expr`'s `Name` arm and its own doc comment on
    // the `module_globals` fallback parameter this enables). A stack
    // `alloca` in `main`'s own frame could never serve this role: it would
    // be a cross-frame reference the moment any other function tried to use
    // it. Each global is zero-initialized (`zero_initializer`) and given
    // `Private` linkage (module-internal only, never referenced from
    // outside this translation unit).
    let module_globals: HashMap<String, (PointerValue, pycc_mir::Ty)> = collect_module_globals(&mir.items)
        .into_iter()
        .filter_map(|(name, ty)| {
            let initial = zero_initializer(&context, ty)?;
            let global = module.add_global(ty_to_basic_type(&context, ty), None, &name);
            global.set_initializer(&initial);
            global.set_linkage(Linkage::Private);
            Some((name, (global.as_pointer_value(), ty)))
        })
        .collect();

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    builder.position_at_end(entry_block);
    // Top-level statements share one `locals` map across the synthetic
    // `main` entry block (module-level Python names are one shared
    // scope); each user function gets its own, fresh map below, since
    // Python function bodies don't see each other's locals. Seeded with a
    // clone of `module_globals` (not `module_globals` itself, which stays
    // borrowed immutably below as every function body's own read-only
    // fallback) -- every top-level name IS one of these globals, so its
    // "first" assignment is just an ordinary store through the
    // already-declared pointer, never a fresh `alloca`.
    let mut top_level_locals = module_globals.clone();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            emit_stmt(&context, &builder, &module, &rt, &user_functions, &mut top_level_locals, &module_globals, stmt)?;
        }
    }
    // Module-level Python code has no `return` (T0024) -- every top-level
    // `str` local's single exit point is program completion right here, so
    // this is where its accepted refcounting scope (D-061's Task 7
    // addendum) decrefs it exactly once, before `main` itself returns.
    for (ptr, ty) in top_level_locals.values() {
        if *ty == pycc_mir::Ty::Str {
            let value = builder
                .build_load(context.ptr_type(inkwell::AddressSpace::default()), *ptr, "final_str")
                .expect("build_load should not fail for this function's own alloca")
                .into_pointer_value();
            builder
                .build_call(rt.str_decref, &[value.into()], "str_decref_final")
                .expect("build_call should not fail for a well-formed decref");
        }
    }
    // Module-level Python code cannot contain a `return`: `pycc_types`'
    // T0024 rejects one anywhere at module scope, including nested inside a
    // top-level `if`/`while`/`for` (its `check_stmt` recurses into itself,
    // not into a function-context variant). The only `emit_stmt` path that
    // builds a terminator is `MirStmt::Return`, so no top-level statement
    // can have terminated this block. If one somehow did, appending the
    // `build_return` below would put a second terminator on an
    // already-terminated block -- invalid IR that `module.verify()` catches
    // everywhere except Windows, where D-029 makes `verify_module` a no-op.
    // Fail loudly on every platform instead of silently emitting bad IR
    // there (see `a_top_level_return_is_an_internal_error_not_bad_ir`), the
    // same guard-then-explicit-panic shape the per-function completion loop
    // below already uses for its own T0024-guaranteed-unreachable case.
    if builder.get_insert_block().unwrap().get_terminator().is_some() {
        panic!(
            "pycc_codegen: internal error: a top-level statement terminated `main`'s \
             entry block -- pycc_types::check (T0024) should have rejected a module-level \
             `return` before it reached codegen"
        );
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
            emit_body(&context, &builder, &module, &rt, &user_functions, &mut fn_locals, &module_globals, body)?;
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
            // `RelocMode::Default` resolves to absolute (non-PIC)
            // addressing for this LLVM/target pairing on Linux, but
            // Ubuntu's `cc`/`gcc` links as a PIE by default (D-073):
            // large-`.rodata` programs (confirmed with the
            // `mandelbrot_ascii` fixture -- its ASCII palette/float
            // constants push a relocation past what a 32-bit absolute
            // reloc can express in a PIE) fail with "relocation
            // R_X86_64_32 against `.rodata' can not be used when making
            // a PIE object". `RelocMode::PIC` matches every Tier-1
            // linker's actual default (mandatory on macOS, standard on
            // Windows/MSVC, and Linux's own PIE default) uniformly.
            RelocMode::PIC,
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

/// Emits one `print()` argument (Task 10), called once per element of
/// `emit_stmt`'s `print`-call arm's `args`, in order, with the separator
/// space between arguments already built by that arm itself (not here) --
/// `Ty::None` (only ever reachable as a direct `Call` result, per Task
/// 6/10's own scope note: there is no `MirExpr::NoneLiteral`, and a `Name`
/// bound to a `None`-typed variable stays an explicit, narrow "not
/// supported yet" here, since `emit_expr`'s `Name` arm has no `Ty::None`
/// case at all) evaluates `arg` for its side effects (the call itself must
/// still run) and discards the placeholder `Scalar` `emit_expr` returns for
/// it (see that arm's own doc comment on `emit_expr`'s `Call` arm),
/// printing the literal `"None"` instead of using it; every other v0.1
/// scalar type converts to `str` via `to_str` (reusing `pycc_rt_int_to_str`/
/// `float_to_str`/`bool_to_str`, the same conversions f-string
/// interpolation already uses) and writes it with `pycc_rt_print_write_str`,
/// then immediately decrefs the fresh `str` `to_str` built -- it's never
/// retained beyond this one print call (same ownership pattern as
/// `emit_expr`'s `FString` arm's own intermediate concatenation results).
///
/// Pulled out of `emit_stmt`'s own `print`-call arm into its own named
/// function, rather than left inlined in that arm's `for` loop body as the
/// task brief's own version had it, for two reasons: it matches this file's
/// established style of extracting each self-contained unit of IR-building
/// logic into its own helper (see `to_str`/`incref_if_str_duplicate`/
/// `truthy` above, all extracted the same way); and, empirically, it fixes
/// a `cargo llvm-cov` region-attribution quirk this task's own development
/// hit -- with this logic left inlined directly inside `emit_stmt`'s large
/// `match`, the lines building the `None` branch's `emit_expr`/
/// `rt.print_none` calls were reported as 0-hit ("uncovered") by `cargo
/// llvm-cov --show-missing-lines` even though a `eprintln!` placed on
/// exactly those lines confirmed, via a direct `cargo test -p pycc_codegen
/// -- --nocapture` run, that they really do execute for `compiles_print_
/// of_a_void_returning_call_as_none`. Restructuring the same logic (first
/// as a plain `if`, ruling out `let-else` specifically as the cause, then)
/// into its own top-level function made the exact same code report 100%
/// covered with no further changes -- behavior is provably identical
/// either way (every test in this file, including the runtime-stdout ones,
/// still passes), so this is treated as a coverage-instrumentation
/// measurement artifact of a large `match` arm's own inlining/region
/// mapping, not a real gap, and worked around structurally rather than by
/// reaching for a `--ignore-filename-regex` exemption (D-014's own policy:
/// that exemption is for a documented design constraint, not a
/// measurement quirk with an available structural fix).
#[allow(clippy::too_many_arguments)]
fn emit_print_arg<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    module_globals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    arg: &MirExpr,
) {
    if arg.ty() == pycc_mir::Ty::None {
        if !matches!(arg, MirExpr::Call { .. }) {
            panic!(
                "pycc_codegen: printing a `None`-typed value that isn't a direct \
                 call result is not supported yet"
            );
        }
        emit_expr(context, builder, module, rt, user_functions, locals, module_globals, arg);
        builder
            .build_call(rt.print_none, &[], "print_none")
            .expect("build_call should not fail for a well-formed print of None");
    } else {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, arg);
        let scalar = incref_if_str_duplicate(builder, rt, arg, scalar);
        let str_ptr = to_str(builder, rt, scalar);
        builder
            .build_call(rt.print_write_str, &[str_ptr.into()], "print_write")
            .expect("build_call should not fail for a well-formed print write");
        builder
            .build_call(rt.str_decref, &[str_ptr.into()], "print_decref_temp")
            .expect("build_call should not fail for a well-formed decref");
    }
}

/// Handles every `MirStmt` shape in v0.1 (this match is exhaustive over
/// `MirStmt`, no catch-all arm): a `print()` call of any number of
/// `int`/`float`/`bool`/`str` arguments plus the narrow `print(f(...))`
/// `None`-result shape (Task 10, space-separated, one trailing newline,
/// matching CPython's `print(*args)`; see that arm's own doc comment), any
/// other bare expression statement (a user-function call with any number of
/// arguments included -- see `emit_expr`'s `Call` arm, which this now
/// delegates to uniformly instead of special-casing zero-arg calls here), a
/// local-variable assignment, `If`/`While`/
/// `ForRange` control flow (Task 4) -- real basic blocks, conditional
/// branches, and loop back-edges, using `truthy` for the shared `if`/
/// `while` truthiness check and `emit_body_then_branch`/an inline
/// equivalent for the terminator-safety this introduces (see both
/// helpers' own doc comments) -- and now (Task 5) `Return`, terminating
/// the current block with the evaluated value (or none, for a bare
/// `return`).
#[allow(clippy::too_many_arguments)]
fn emit_stmt<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, FunctionValue<'ctx>>,
    locals: &mut HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    module_globals: &HashMap<String, (PointerValue<'ctx>, pycc_mir::Ty)>,
    stmt: &MirStmt,
) -> Result<(), String> {
    match stmt {
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    builder
                        .build_call(rt.print_space, &[], "print_sep")
                        .expect("build_call should not fail for a well-formed print separator");
                }
                emit_print_arg(context, builder, module, rt, user_functions, locals, module_globals, arg);
            }
            builder
                .build_call(rt.print_newline, &[], "print_end")
                .expect("build_call should not fail for a well-formed print newline");
            Ok(())
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
            build_call_to(context, builder, module, rt, user_functions, locals, module_globals, f, args);
            Ok(())
        }
        MirStmt::ExprStmt(expr) => {
            emit_expr(context, builder, module, rt, user_functions, locals, module_globals, expr);
            Ok(())
        }
        MirStmt::Assign { target, value } => {
            let ty = value.ty();
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, value);
            let scalar = incref_if_str_duplicate(builder, rt, value, scalar);
            if ty == pycc_mir::Ty::Str {
                decref_old_str_if_reassigning(context, builder, rt, locals, target);
            }
            emit_assign(context, builder, locals, target, ty, scalar);
            Ok(())
        }
        MirStmt::If { test, body, orelse } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let cond = {
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, test);
                truthy(context, builder, rt, scalar)
            };
            let then_bb = context.append_basic_block(function, "if_then");
            let merge_bb = context.append_basic_block(function, "if_merge");
            let else_bb = if orelse.is_empty() { merge_bb } else { context.append_basic_block(function, "if_else") };
            builder
                .build_conditional_branch(cond, then_bb, else_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(then_bb);
            emit_body(context, builder, module, rt, user_functions, locals, module_globals, body)?;
            let then_reaches_merge = builder.get_insert_block().unwrap().get_terminator().is_none();
            if then_reaches_merge {
                builder
                    .build_unconditional_branch(merge_bb)
                    .expect("build_unconditional_branch should not fail on a block with no terminator yet");
            }

            // An empty `orelse` never itself reaches `merge_bb` through this
            // branch -- the conditional branch above already wires its
            // `else_bb == merge_bb` false edge directly, independently of
            // whether the `then` branch returned.
            let else_reaches_merge = if orelse.is_empty() {
                true
            } else {
                builder.position_at_end(else_bb);
                emit_body(context, builder, module, rt, user_functions, locals, module_globals, orelse)?;
                let reaches = builder.get_insert_block().unwrap().get_terminator().is_none();
                if reaches {
                    builder
                        .build_unconditional_branch(merge_bb)
                        .expect("build_unconditional_branch should not fail on a block with no terminator yet");
                }
                reaches
            };

            builder.position_at_end(merge_bb);
            // Both branches returned (or otherwise terminated) before
            // reaching `merge_bb`: it has zero predecessors and, left
            // alone, no terminator of its own -- invalid IR, and (if this
            // `If` is a function body's last statement) a false "fell
            // through without a `return`" positive from
            // `compile_to_object`'s own end-of-function check, even though
            // every real path through this function already returned. An
            // explicit `unreachable` terminator makes this block valid IR
            // and marks it as already-terminated to every caller that
            // checks `get_terminator()` (`emit_body`'s own loop,
            // `compile_to_object`'s fallthrough check), exactly like a real
            // `return` would.
            if !then_reaches_merge && !else_reaches_merge {
                builder
                    .build_unreachable()
                    .expect("build_unreachable should not fail on a fresh block with no terminator");
            }
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
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, test);
                truthy(context, builder, rt, scalar)
            };
            builder
                .build_conditional_branch(cond, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            emit_body_then_branch(context, builder, module, rt, user_functions, locals, module_globals, body, test_bb)?;

            builder.position_at_end(after_bb);
            Ok(())
        }
        MirStmt::ForRange { var, start, stop, step, body } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            // `bool` is an `int` subtype (`pycc_types::is_assignable`) --
            // `check_range_operand_in` accepts a bool-typed `start`/`stop`/
            // `step` for exactly that reason (see
            // `a_for_range_loop_accepts_bool_as_an_int_subtype`), so real,
            // accepted source like `range(True, 5)` legitimately reaches
            // this arm with a `Scalar::Bool` operand -- widened via
            // `to_tagged_int` (D-061), the same promotion `build_call_to`
            // and `Return` already apply. `Float`/`Str` genuinely can never
            // reach here from real `pycc_types` output (range operands must
            // be int-assignable), so those keep their own distinct,
            // position-specific internal-error panics.
            let start_v = match emit_expr(context, builder, module, rt, user_functions, locals, module_globals, start) {
                Scalar::Int(v) => v,
                scalar @ Scalar::Bool(_) => to_tagged_int(context, builder, scalar),
                _ => panic!("pycc_codegen: internal error: range() start did not evaluate to int"),
            };
            let stop_v = match emit_expr(context, builder, module, rt, user_functions, locals, module_globals, stop) {
                Scalar::Int(v) => v,
                scalar @ Scalar::Bool(_) => to_tagged_int(context, builder, scalar),
                _ => panic!("pycc_codegen: internal error: range() stop did not evaluate to int"),
            };
            let step_v = match emit_expr(context, builder, module, rt, user_functions, locals, module_globals, step) {
                Scalar::Int(v) => v,
                scalar @ Scalar::Bool(_) => to_tagged_int(context, builder, scalar),
                _ => panic!("pycc_codegen: internal error: range() step did not evaluate to int"),
            };
            emit_assign(context, builder, locals, var, pycc_mir::Ty::Int, Scalar::Int(start_v));

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
            emit_body(context, builder, module, rt, user_functions, locals, module_globals, body)?;
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
                    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, module_globals, expr);
                    let scalar = incref_if_str_duplicate(builder, rt, expr, scalar);
                    // `bool` is an `int` subtype (`pycc_types::is_assignable`)
                    // -- returning a `bool` value from a function declared
                    // to return `int` needs the same D-061 tagging
                    // `build_call_to`'s argument-marshalling applies, or the
                    // built `ret` instruction's operand type would not
                    // match this function's own declared return type
                    // (queried directly from its `FunctionType`, the same
                    // approach `build_call_to` uses for parameters).
                    let return_ty = builder
                        .get_insert_block()
                        .expect("builder is always positioned inside some block while a statement is being emitted")
                        .get_parent()
                        .expect("the block builder is currently positioned in always belongs to a function")
                        .get_type()
                        .get_return_type();
                    let needs_widening = matches!(scalar, Scalar::Bool(_))
                        && return_ty.is_some_and(|rt_ty| rt_ty == context.i64_type().into());
                    let scalar =
                        if needs_widening { Scalar::Int(to_tagged_int(context, builder, scalar)) } else { scalar };
                    let basic_value: inkwell::values::BasicValueEnum = match scalar {
                        Scalar::Int(v) => v.into(),
                        Scalar::Bool(v) => v.into(),
                        Scalar::Float(v) => v.into(),
                        Scalar::Str(v) => v.into(),
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

    /// `print(<n>)` as a `MirStmt` -- a convenience single-int-argument
    /// shape reused by many of this file's older tests (`emit_stmt`'s
    /// `print` dispatch itself now handles any number of arguments of any
    /// v0.1 scalar type, plus the narrow `None`-result shape; see its own
    /// doc comment, Task 10).
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
    fn compiles_a_zero_argument_print_producing_just_a_newline() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("print_zero_args");
        let obj_path = dir.join("print_zero_args.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("print_zero_args");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"\n");
    }

    #[test]
    fn compiles_a_multi_argument_print_with_mixed_types_space_separated() {
        // `print(1, 2.5, True, "hi")` -- prints `1 2.5 True hi\n`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![
                    MirExpr::IntLiteral(1),
                    MirExpr::FloatLiteral(2.5),
                    MirExpr::BoolLiteral(true),
                    MirExpr::StringLiteral("hi".to_string()),
                ],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("print_mixed_multi");
        let obj_path = dir.join("print_mixed_multi.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("print_mixed_multi");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1 2.5 True hi\n");
    }

    #[test]
    fn compiles_print_of_a_bool_false() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BoolLiteral(false)],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("print_false");
        let obj_path = dir.join("print_false.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("print_false");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"False\n");
    }

    #[test]
    fn compiles_print_of_a_void_returning_call_as_none() {
        // `def f() -> None: return` ; `print(f())` -- prints `None`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::None,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("print_none_from_call");
        let obj_path = dir.join("print_none_from_call.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("print_none_from_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"None\n");
    }

    #[test]
    #[should_panic(expected = "printing a `None`-typed value that isn't a direct call result is not supported yet")]
    fn printing_a_none_typed_name_that_isnt_a_direct_call_result_panics() {
        // `print(y)` where `y` is (hypothetically) `None`-typed but not
        // itself a `Call` -- the narrow gap this task's own scope note
        // documents (Task 6's finding: a `Name` bound to a `None`-typed
        // variable is legal Python but stays unsupported here, since
        // `emit_expr`'s `Name` arm has no `Ty::None` case at all). Real
        // `pycc_types` has no way to produce a `None`-typed `Name` in v0.1
        // (there is no `x = None`-shaped source, and even `x =
        // some_void_function()` would need `emit_expr`'s own `Name` arm to
        // support reading it back, which it deliberately doesn't) -- this
        // is deliberately malformed MIR exercising `emit_stmt`'s own
        // defensive guard for that shape directly, matching this file's
        // established convention for internal-error tests. The panic
        // fires purely from `arg`'s own shape (`matches!(arg, MirExpr::
        // Call { .. })`), before any name lookup, so `y` is never actually
        // bound in `locals`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name { name: "y".to_string(), ty: Ty::None }],
                ty: Ty::None,
            }))],
        };
        let dir = tempfile_dir("print_none_typed_name_panics");
        let obj_path = dir.join("print_none_typed_name_panics.o");
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
        // Nothing here reads `b` back out (a dedicated runtime `print(bool)`
        // test exists separately -- `compiles_print_of_a_bool_false`), so
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
        // the result back here (see `compiles_print_of_a_bool_false` for a
        // dedicated runtime `print(bool)` test), so this only proves the
        // comparison itself doesn't crash/miscompile, same as
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
        // fit the 63-bit tagged range (D-061).
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
    #[should_panic(expected = "an f-string with zero parts should not be reachable")]
    fn assigning_a_zero_part_fstring_hits_the_defensive_internal_panic() {
        // Renamed and re-targeted for Task 8: this test's previous
        // (Task 3/7-era) incarnation exercised `emit_expr`'s final
        // catch-all arm (`other => panic!("...this expression kind's
        // codegen is not supported yet...")`), back when `MirExpr::FString`
        // had no arm of its own at all. Task 8 gives `FString` a real arm,
        // and with every other `MirExpr` variant already handled by its own
        // named arm, that catch-all became dead code (unreachable for any
        // input) and was removed rather than kept as untestable dead
        // weight -- the same "remove a provably dead arm" convention this
        // file already applies elsewhere (see `emit_expr`'s `Name` arm's
        // own doc comment).
        //
        // `MirExpr::FString(vec![])` (zero parts) is still not a real,
        // reachable program shape -- `pycc_hir`'s own f-string lowering
        // always produces at least one `Literal` part, even for a literal
        // empty f-string `f""` -- but `emit_expr`'s new `FString` arm
        // guards that assumption defensively instead of silently returning
        // a dangling/null pointer if it's ever wrong (see that arm's own
        // doc comment). This test is what exercises that guard: deliberately
        // malformed MIR no real pipeline produces, same convention as this
        // file's other "internal error" tests (e.g.
        // `referencing_a_name_with_no_bound_local_is_an_internal_error`).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::FString(vec![]),
            })],
        };
        let dir = tempfile_dir("fstring_zero_parts_panics");
        let obj_path = dir.join("fstring_zero_parts_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn printing_a_mistyped_compare_expression_prints_the_actual_runtime_value() {
        // Deliberately malformed MIR: `pycc_mir::build` always lowers
        // `Compare` with `ty: Ty::Bool` (see `pycc_mir`'s own
        // `builds_a_compare_expression_with_bool_type` test) -- no real
        // pipeline could ever produce `ty: Ty::Int` here.
        //
        // Before Task 10, `emit_stmt`'s `print` arm dispatched on this
        // (lied-about) declared `ty` field -- a `Ty::Int`-guarded arm then
        // pattern-matched the actual `Scalar` back out with a `let
        // Scalar::Int(v) = ... else { unreachable!(...) }`, which this test
        // used to prove panics for a mismatched `ty`. Task 10's fully
        // general dispatch removed that per-argument `ty`-based branch
        // entirely: it only ever inspects `arg.ty()` to tell a `None`-typed
        // argument apart from every other one (see that arm's own doc
        // comment), and then hands whatever `Scalar` `emit_expr` actually
        // produced straight to `to_str`, which matches on the real `Scalar`
        // variant, never the caller-declared `ty`. So this exact
        // mismatched-`ty` shape can no longer desync from reality -- it
        // just prints the real `Scalar::Bool` value `Compare` always
        // produces (`1 < 2` is `True`), regardless of what `ty` claims.
        // Kept (renamed, no longer `#[should_panic]`) as a regression test
        // documenting this behavior change rather than being deleted
        // outright, same rationale as `a_none_typed_call_result_used_as_a_
        // nested_expression_no_longer_panics` above.
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
        let dir = tempfile_dir("print_mistyped_compare_prints_actual_value");
        let obj_path = dir.join("print_mistyped_compare_prints_actual_value.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("print_mistyped_compare_prints_actual_value");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
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
    #[should_panic(expected = "reading a `None`-typed local is not supported yet")]
    fn reading_a_none_typed_local_is_not_yet_supported() {
        // Not reachable through `compile_to_object`/real MIR at all: no
        // real pipeline can produce a `None`-typed *local* (only a call's
        // or function's return type is ever legitimately `None`) -- this
        // calls `emit_expr` directly with a hand-built `locals` map
        // instead, exercising the `Name` arm's defensive catch-all. This
        // test's earlier (Task 3-era) incarnation used `Ty::Str` here,
        // before Task 7 added real `str`-local support (see
        // `reading_a_float_local_back_out_of_its_alloca` below for the
        // precedent this mirrors, and
        // `compiles_string_concatenation_and_a_reassignment_that_frees_the_old_value`
        // for the real `str`-local read-back test that replaced this one's
        // old role).
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
        // The alloca's own LLVM type is arbitrary here (`None` has no
        // codegen representation to allocate correctly) -- it's never
        // actually loaded from, since `emit_expr`'s `Name` arm panics on
        // `Ty::None` before reaching any `build_load` call.
        let ptr = builder
            .build_alloca(context.f64_type(), "x")
            .expect("build_alloca should not fail for a fresh block");
        locals.insert("x".to_string(), (ptr, Ty::None));

        let module_globals = HashMap::new();
        emit_expr(
            &context,
            &builder,
            &module,
            &rt,
            &user_functions,
            &locals,
            &module_globals,
            &MirExpr::Name { name: "x".to_string(), ty: Ty::None },
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
    fn a_function_reads_a_module_level_global_it_does_not_itself_assign() {
        // `x = 5` ; `def f() -> int:\n    return x` ; `print(f())` -- must
        // print `5`. Before this fix, every function's `fn_locals` map
        // started empty (seeded only with its own parameters), and every
        // top-level name lived only in `main`'s own separate
        // `top_level_locals` map -- entirely discarded once `main`'s body
        // finished emitting, never visible to any function's own codegen.
        // `emit_expr`'s `Name` arm panicked ("no local slot") the moment a
        // function body read a module-level global it did not itself
        // assign, even though `pycc_types` (D-055) and `pycc_mir` (this
        // file's own sibling fix) both correctly accept and type this
        // program. Fixed by giving every module-level binding real
        // (non-stack) LLVM global storage, reachable from any function via
        // a `module_globals` fallback consulted only when a name isn't
        // already bound in that function's own `locals`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(5),
                }),
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Name { name: "x".to_string(), ty: Ty::Int }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("function_reads_module_global");
        let obj_path = dir.join("function_reads_module_global.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("function_reads_module_global");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"5\n");
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
    fn a_while_loop_body_that_always_returns_skips_its_own_trailing_branch() {
        // `def f() -> int:\n    while True:\n        return 1\n    return 2`
        // ; `print(f())` -- must print `1`. The trailing `return 2` is
        // unreachable dead code, present only because `pycc_types`' T0022
        // fallthrough check (`block_always_returns`) always treats a
        // `while`/`for` loop as *not* provably exhaustive on its own
        // (deferred to issue #118, per D-055), so a bare `while True: return
        // 1` with nothing after it would never actually be accepted source
        // -- this shape is what real accepted source produces instead.
        // Distinct region from every other `while` test in this file, all
        // of whose *loop bodies* fall through normally and so always take
        // `emit_body_then_branch`'s own trailing
        // `build_unconditional_branch(test_bb)` back to the loop test: here
        // the loop body's own `return` already terminates it, so that
        // helper's terminator check must skip building a second (invalid)
        // terminator on top of it.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::While {
                            test: MirExpr::BoolLiteral(true),
                            body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
                        },
                        MirStmt::Return(Some(MirExpr::IntLiteral(2))),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("while_body_always_returns");
        let obj_path = dir.join("while_body_always_returns.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("while_body_always_returns");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
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
    fn a_second_top_level_for_range_loop_reusing_a_loop_variable_name_is_not_redeclared() {
        // `for i in range(0, 2, 1): print(i)` followed by a second, separate
        // `for i in range(0, 3, 1): print(i)` -- both loops share the same
        // module-level loop variable name `i`. Exercises
        // `collect_module_globals`'s `ForRange` arm's own `if
        // seen.insert(var.clone())` check on its *false* path (a second
        // occurrence of an already-registered name must not re-declare or
        // re-order its global), distinct from every other `ForRange` test in
        // this file, which only ever registers each loop variable once.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ForRange {
                    var: "i".to_string(),
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(2),
                    step: MirExpr::IntLiteral(1),
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                        ty: Ty::None,
                    })],
                }),
                MirItem::TopLevelStmt(MirStmt::ForRange {
                    var: "i".to_string(),
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(3),
                    step: MirExpr::IntLiteral(1),
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                        ty: Ty::None,
                    })],
                }),
            ],
        };
        let dir = tempfile_dir("for_range_reused_loop_var");
        let obj_path = dir.join("for_range_reused_loop_var.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("for_range_reused_loop_var");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n1\n0\n1\n2\n");
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
        // `pycc_types` only accepts an int-*assignable* `start`/`stop`/
        // `step` (`check_range_operand_in`'s own `is_assignable(actual,
        // Ty::Int)` check) -- `bool` qualifies (see
        // `for_range_with_a_bool_start_widens_to_int` below) but `float`
        // never does, so this stays genuinely unreachable via any real
        // pipeline output, same convention as the `BinOp`/`Compare`
        // operand-type checks above -- hand-built malformed MIR exercises
        // it directly.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::FloatLiteral(1.0),
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
                stop: MirExpr::FloatLiteral(3.0),
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
                step: MirExpr::FloatLiteral(1.0),
                body: vec![],
            })],
        };
        let dir = tempfile_dir("for_range_bad_step_panics");
        let obj_path = dir.join("for_range_bad_step_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn for_range_with_a_bool_start_stop_and_step_all_widen_to_int() {
        // `for i in range(True, 4, True): print(i)` -- `bool` is an `int`
        // subtype (`pycc_types::is_assignable`), and
        // `a_for_range_loop_accepts_bool_as_an_int_subtype` proves
        // `pycc_types` genuinely accepts a bool-typed `range()` argument for
        // any of its three positions, so this reaches codegen with
        // `Scalar::Bool` `start`/`stop`/`step` operands (`stop` is `4`, a
        // plain int literal, to keep this a short, checkable loop; `start`
        // and `step` are both `True` to exercise both of that arm's other
        // two match sites). Before this fix, each position's
        // `let Scalar::Int(..) = ... else { panic!(...) }` destructure
        // rejected any non-`Int` scalar outright, crashing the compiler on
        // this legitimate, accepted program instead of widening `True` to
        // the tagged int `1` like every other bool-into-int site in this
        // file already does.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::BoolLiteral(true),
                stop: MirExpr::IntLiteral(4),
                step: MirExpr::BoolLiteral(true),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_bool_start_stop_step");
        let obj_path = dir.join("for_range_bool_start_stop_step.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("for_range_bool_start_stop_step");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n3\n");
    }

    #[test]
    fn for_range_with_a_bool_stop_widens_to_int() {
        // `for i in range(0, True, 1): print(i)` -- distinct region from
        // the `start`/`step` coverage above: exercises `stop`'s own
        // `scalar @ Scalar::Bool(_) => to_tagged_int(...)` arm specifically.
        // `True` widens to the tagged int `1`, so this loop runs once.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::BoolLiteral(true),
                step: MirExpr::IntLiteral(1),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "i".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_bool_stop");
        let obj_path = dir.join("for_range_bool_stop.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("for_range_bool_stop");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n");
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
    fn a_function_returning_from_both_if_and_else_branches_compiles_and_runs() {
        // `def f(x: int) -> int:\n    if x > 0:\n        return 1\n    else:\n        return 2`
        // Every real path through `f` returns, so this is legal, ordinary
        // Python -- but before this fix, `MirStmt::If`'s codegen
        // unconditionally positioned the builder at `if_merge` after
        // emitting both branches, even when both had already terminated via
        // `return` (leaving `if_merge` an unreachable block with zero
        // predecessors and no terminator of its own). `emit_body`'s caller
        // (here, `compile_to_object`'s own end-of-function fallthrough
        // check) then saw a terminator-less current block and raised its
        // own "fell through without a `return`" internal-error panic --
        // a false positive for a function that provably always returns.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::If {
                        test: MirExpr::Compare {
                            op: CmpOpKind::Gt,
                            left: Box::new(MirExpr::Name { name: "x".to_string(), ty: Ty::Int }),
                            right: Box::new(MirExpr::IntLiteral(0)),
                            ty: Ty::Bool,
                        },
                        body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
                        orelse: vec![MirStmt::Return(Some(MirExpr::IntLiteral(2)))],
                    }],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::IntLiteral(5)],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("if_else_both_return");
        let obj_path = dir.join("if_else_both_return.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("if_else_both_return");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
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
    fn a_top_level_return_is_an_internal_error_not_bad_ir() {
        // `pycc_types`' T0024 rejects any module-level `return` already (even
        // nested in a top-level `if`/`while`/`for`) -- this proves codegen
        // fails loudly (a clear panic) rather than emitting a second
        // terminator into `main`'s entry block, which is invalid IR that
        // `module.verify()` cannot catch on Windows (D-029's no-op), if that
        // check is ever somehow bypassed. Mirrors
        // `a_non_none_function_falling_through_is_an_internal_error_not_bad_ir`
        // above for the per-function analogue of the same guard.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Return(Some(MirExpr::IntLiteral(0))))],
        };
        let dir = tempfile_dir("top_level_return_internal_error");
        let obj_path = dir.join("top_level_return_internal_error.o");
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
    fn a_none_typed_call_result_used_as_a_nested_expression_no_longer_panics() {
        // Deliberately malformed MIR: a `None`-returning function's result
        // can only legitimately appear as a bare statement (see
        // `emit_stmt`'s own void-call arm) or as one of `print`'s own
        // arguments (see `emit_stmt`'s `print` dispatch) -- real
        // `pycc_types` would never type an `Assign`'s value as `Ty::None`
        // this way (there is no `x = None`-shaped source this could come
        // from in v0.1). Before Task 10, this hit `emit_expr`'s `Call`
        // arm's defensive `other =>` catch-all and panicked with "a
        // `None`-typed call result is not supported yet"; Task 10 gives
        // that arm its own explicit `Ty::None` case instead (a placeholder
        // `Scalar`, needed so `emit_stmt`'s `print` dispatch can call
        // `emit_expr` on a `None`-typed argument at all -- see that arm's
        // own doc comment), so this exact malformed shape now silently
        // produces a placeholder `bool` local instead of panicking. Kept
        // (renamed, no longer `#[should_panic]`) as a regression test
        // documenting that specific, intentional behavior change rather
        // than being deleted outright -- `an_infer_typed_call_result_used_
        // as_a_nested_expression_is_not_supported` below takes over this
        // test's original job of exercising the `other =>` catch-all
        // itself, via the one `Ty` variant that still reaches it.
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
        let dir = tempfile_dir("none_typed_call_result_no_longer_panics");
        let obj_path = dir.join("none_typed_call_result_no_longer_panics.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "a `Infer`-typed call result is not supported yet")]
    fn an_infer_typed_call_result_used_as_a_nested_expression_is_not_supported() {
        // Exercises `emit_expr`'s `Call` arm's own defensive `other =>`
        // catch-all on `ty` -- `Ty::Infer` (an HIR-only inference
        // placeholder no real MIR ever carries this far, same rationale as
        // `ty_to_basic_type`'s own `an_infer_typed_return_value_is_not_yet_
        // supported` test above) is the one `Ty` variant left that still
        // reaches it, now that Task 10 gives `Ty::None` its own explicit
        // (non-panicking) case there (see the test directly above).
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::Infer },
                }),
            ],
        };
        let dir = tempfile_dir("infer_typed_call_result_panics");
        let obj_path = dir.join("infer_typed_call_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "a `Infer`-typed parameter/return value is not supported yet")]
    fn an_infer_typed_return_value_is_not_yet_supported() {
        // `ty_to_basic_type` now implements `Int`/`Bool`/`Float`/`Str`
        // (Task 7 closed the `Str` gap this test's earlier, Task 3-era
        // incarnation exercised -- see
        // `compiles_a_function_with_a_str_parameter_and_str_return_value`
        // below). `Ty::None` can't stand in for "still unhandled" here:
        // `compile_to_object`'s own `return_ty` match special-cases
        // `Ty::None` into `void_type().fn_type(...)` *before*
        // `ty_to_basic_type` is ever called for a return type (see that
        // match's own `Ty::None` arm) -- `Ty::Infer` (an HIR-only inference
        // placeholder no real MIR ever carries this far) is the one `Ty`
        // variant left that still reaches `ty_to_basic_type`'s own
        // defensive catch-all from the return-type position.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![],
            }],
        };
        let dir = tempfile_dir("infer_return_panics");
        let obj_path = dir.join("infer_return_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "a `None`-typed parameter/return value is not supported yet")]
    fn a_none_typed_parameter_is_not_yet_supported() {
        // `def f(x: None): ...` -- a distinct `ty_to_basic_type` call site
        // from the return-type test above (a function's parameter list,
        // inside `compile_to_object`'s first pass, which has no `Ty::None`
        // bypass of its own), same underlying panic. This test's earlier
        // (Task 3-era) incarnation used `Ty::Str` here, before Task 7
        // closed that gap.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::None)],
                return_ty: Ty::None,
                body: vec![],
            }],
        };
        let dir = tempfile_dir("none_param_panics");
        let obj_path = dir.join("none_param_panics.o");
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
    fn a_bool_argument_widens_to_int_when_the_parameter_is_declared_int() {
        // `def f(x: int) -> None: print(x)` ; `f(True)` -- `bool` is an
        // `int` subtype (`pycc_types::is_assignable`), so this is valid,
        // type-checked v0.1 Python. `build_call_to` previously passed the
        // evaluated `Scalar::Bool` (an `i8`) straight through with no
        // widening, so the built call's argument type didn't match `f`'s
        // declared `i64` parameter -- `module.verify()` rejected the IR.
        // `x` is `int`-typed, so `True` widens to the tagged fixnum `1`;
        // prints "1", not "True".
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                        ty: Ty::None,
                    })],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![MirExpr::BoolLiteral(true)],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bool_arg_widens_to_int");
        let obj_path = dir.join("bool_arg_widens_to_int.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("bool_arg_widens_to_int");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_bool_return_value_widens_to_int_when_the_function_declares_int() {
        // `def f() -> int: return True` ; `print(f())` -- same
        // `bool`-is-`int` widening as the argument case above, but for
        // `MirStmt::Return`'s own value-emission arm: it previously mapped
        // the returned `Scalar::Bool` straight to a `BasicValueEnum` with no
        // widening, so the built `ret` instruction's operand type didn't
        // match `f`'s declared `i64` return type -- `module.verify()`
        // rejected the IR. Prints "1", not "True".
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BoolLiteral(true)))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bool_return_widens_to_int");
        let obj_path = dir.join("bool_return_widens_to_int.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("bool_return_widens_to_int");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn reassigning_an_int_local_with_a_bool_value_widens_it_to_int() {
        // `x = 5; x = True; print(x)` -- `pycc_types::check_assignment`'s
        // sticky-first-type rule (T0023) keeps `x` typed `int` throughout
        // (a later `bool` value is `is_assignable` into it, never rebinding
        // it), so `pycc_mir` reports every `Name("x")` read as `Ty::Int`.
        // `emit_assign` previously reused the first assignment's `i64`
        // alloca but stored the second assignment's raw `Scalar::Bool` (an
        // `i8`) into it verbatim -- an `i8` store into an `i64`-sized slot,
        // followed by an `i64` load expecting a full tagged fixnum. Prints
        // "1" (the tagged fixnum for the `int` value `1`), not "True".
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(5),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BoolLiteral(true),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("reassign_bool_into_int");
        let obj_path = dir.join("reassign_bool_into_int.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("reassign_bool_into_int");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
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
    #[should_panic(expected = "a `None`-result BinOp is not supported yet")]
    fn a_none_result_binop_is_not_yet_supported() {
        // No real Python operator returns `None` from a `BinOp`, so
        // `pycc_types`/`pycc_mir` never produce this shape -- hand-crafted
        // MIR exercises the `BinOp` arm's own defensive catch-all directly
        // instead, using `int` operands under a mislabeled `ty`, same
        // "hand-construct the otherwise-unreachable shape" convention as
        // `true_division_binop_codegen_panics_via_its_dedicated_arm` above.
        // This test's earlier (Task 3-era) incarnation used `Ty::Str` here
        // (real string concatenation was Task 7's job then); Task 7 now
        // implements `Ty::Str` for real, so `Ty::None` is the placeholder
        // that keeps this catch-all covered.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::None,
                },
            })],
        };
        let dir = tempfile_dir("binop_none_result_panics");
        let obj_path = dir.join("binop_none_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "internal error: str BinOp operand did not evaluate to str")]
    fn a_str_result_binop_with_a_non_str_left_operand_hits_the_internal_consistency_check() {
        // Deliberately malformed MIR: `pycc_types`/`pycc_mir` only ever
        // produce a `Ty::Str`-typed `BinOp` for `str + str` (see
        // `pycc_mir`'s own `adding_two_strings_infers_str` test), so no real
        // pipeline could reach this arm with a non-`str` left operand.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::StringLiteral("b".to_string())),
                    ty: Ty::Str,
                },
            })],
        };
        let dir = tempfile_dir("str_binop_left_mismatch_panics");
        let obj_path = dir.join("str_binop_left_mismatch_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "internal error: str BinOp operand did not evaluate to str")]
    fn a_str_result_binop_with_a_non_str_right_operand_hits_the_internal_consistency_check() {
        // Same rationale as the left-operand version above, isolating the
        // `BinOp` arm's *second* `let Scalar::Str(r) = r else { .. }` check
        // -- the left operand must genuinely be `str` (so the first check
        // passes) for this one to be reached at all.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::StringLiteral("a".to_string())),
                    right: Box::new(MirExpr::IntLiteral(2)),
                    ty: Ty::Str,
                },
            })],
        };
        let dir = tempfile_dir("str_binop_right_mismatch_panics");
        let obj_path = dir.join("str_binop_right_mismatch_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "`str Sub str` is not supported yet (only concatenation is)")]
    fn a_str_binop_other_than_concatenation_is_not_yet_supported() {
        // `"a" - "b"` -- real `str` operands on both sides, but only `Add`
        // (concatenation) is implemented; Python doesn't define `str - str`
        // either, so `pycc_types` would reject this long before codegen --
        // this exercises the `Str` arm's own `op != Add` guard directly.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Sub,
                    left: Box::new(MirExpr::StringLiteral("a".to_string())),
                    right: Box::new(MirExpr::StringLiteral("b".to_string())),
                    ty: Ty::Str,
                },
            })],
        };
        let dir = tempfile_dir("str_binop_unsupported_op_panics");
        let obj_path = dir.join("str_binop_unsupported_op_panics.o");
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
        // and_pow_binops`'s `int` coverage. This test itself doesn't print
        // any of these results (same limitation as `compiles_true_division_
        // of_two_ints_as_float_arithmetic`/`compiles_mixed_int_and_float_
        // addition` above -- `print(float)` runtime output is exercised
        // separately, e.g. via `compiles_a_multi_argument_print_with_mixed_
        // types_space_separated`'s `2.5` argument), so this only proves
        // each arm compiles and verifies, not a runtime stdout value.
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

    #[test]
    fn compiles_string_concatenation_and_a_reassignment_that_frees_the_old_value() {
        // `x = "foo"; x = x + "bar"` -- the second `Assign` reads the
        // existing `x` (needs an incref before rebinding) and overwrites
        // `x`'s slot (must decref the *original* `"foo"` first). Nothing
        // observes the refcounting directly; this proves it doesn't crash
        // and that codegen for the whole sequence succeeds.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::StringLiteral("foo".to_string()),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::Name { name: "x".to_string(), ty: Ty::Str }),
                        right: Box::new(MirExpr::StringLiteral("bar".to_string())),
                        ty: Ty::Str,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("str_concat_reassign");
        let obj_path = dir.join("str_concat_reassign.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_concat_reassign");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn an_int_local_first_assigned_inside_an_if_body_is_readable_after_the_if() {
        // `if True: x = 1` then `print(x)` at the top level, after the
        // `if`. Before this fix, `emit_assign`'s `None` branch built a
        // non-`str` local's `alloca` at whatever position `builder` was
        // already at (inside the `if_then` block for `x`'s first
        // assignment) instead of hoisting it to the function's entry block
        // (the same fix `alloca_str_at_entry` already applies to `str`
        // locals only) -- `print(x)`'s read, positioned in `if_merge` after
        // the `if`, is not dominated by `if_then`, so `module.verify()`
        // rejected the resulting IR.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::BoolLiteral(true),
                    body: vec![MirStmt::Assign { target: "x".to_string(), value: MirExpr::IntLiteral(1) }],
                    orelse: vec![],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("int_first_assign_in_if_body");
        let obj_path = dir.join("int_first_assign_in_if_body.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("int_first_assign_in_if_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn an_int_local_assigned_in_both_branches_of_an_if_else_is_readable_after_the_if() {
        // `if True: x = 1 else: x = 2` then `print(x)` -- `x`'s first
        // assignment (the `then` branch, codegen'd first) creates its slot;
        // the `else` branch's own `Assign` reuses that same slot
        // (`emit_assign`'s `locals.get(target) => Some` path) from a
        // sibling block that neither dominates nor is dominated by the
        // `then` block -- broken before this fix for the same reason as the
        // single-branch case above, and additionally proving the
        // reused-slot path is safe once the slot lives in the entry block.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::BoolLiteral(true),
                    body: vec![MirStmt::Assign { target: "x".to_string(), value: MirExpr::IntLiteral(1) }],
                    orelse: vec![MirStmt::Assign { target: "x".to_string(), value: MirExpr::IntLiteral(2) }],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "x".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("int_first_assign_in_if_else_both");
        let obj_path = dir.join("int_first_assign_in_if_else_both.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("int_first_assign_in_if_else_both");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_str_local_first_assigned_inside_an_if_body_is_freed_at_top_level_completion() {
        // Regression test for a review finding against this file's first
        // `str`-codegen task: `if True: s = "hi"` -- `s`'s *first*
        // assignment happens inside the `if`'s own `then` block, never at
        // the top level directly, and `s` is never read by any user code
        // either. The only read of `s`'s slot is
        // `compile_to_object`'s own top-level-locals completion-decref loop,
        // positioned right before `main`'s `build_return` -- outside the
        // `if`'s `then` block entirely. Before this fix, `s`'s `alloca`
        // lived wherever `builder` happened to be positioned when
        // `emit_assign` first ran for `s` (inside the `then` block), which
        // does not dominate that later completion-loop read: `module.
        // verify()` rejected the resulting IR and `compile_to_object`
        // panicked ("Instruction does not dominate all uses! %final_str =
        // load ptr, ptr %s"). Hoisting `s`'s `alloca` to the function's
        // entry block (`alloca_str_at_entry`) fixes this regardless of
        // which nested block `s`'s first assignment executes in.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::BoolLiteral(true),
                body: vec![MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::StringLiteral("hi".to_string()),
                }],
                orelse: vec![],
            })],
        };
        let dir = tempfile_dir("str_first_assign_in_if_body");
        let obj_path = dir.join("str_first_assign_in_if_body.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_if_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_assigned_in_both_branches_of_an_if_else_is_freed_at_top_level_completion() {
        // `if True: s = "hi" else: s = "bye"` -- `s`'s *first* assignment
        // (the `then` branch, processed first by this file's `If` codegen)
        // creates its slot; the `else` branch's own `Assign` then reuses
        // that same slot (`emit_assign`'s `locals.get(target) => Some`
        // path) from a sibling block that neither dominates nor is
        // dominated by the `then` block. Same underlying entry-block-
        // hoisting fix as the single-branch test above, additionally
        // proving the reused-slot path is safe once the slot itself lives
        // in the entry block rather than in whichever branch first created
        // it.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::If {
                test: MirExpr::BoolLiteral(true),
                body: vec![MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::StringLiteral("hi".to_string()),
                }],
                orelse: vec![MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::StringLiteral("bye".to_string()),
                }],
            })],
        };
        let dir = tempfile_dir("str_first_assign_in_if_else_both");
        let obj_path = dir.join("str_first_assign_in_if_else_both.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_if_else_both");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_first_assigned_inside_a_while_body_is_freed_at_top_level_completion() {
        // `i = 0; while i < 3: s = "x"; i = i + 1` -- `s`'s first assignment
        // happens inside the `while`'s own body block, exercising the same
        // entry-block-hoisting fix through a loop back-edge instead of an
        // `if`/`else` merge. This is the only test in this file whose first
        // binding of a `str` local is itself inside a loop body, so it also
        // pins D-061's accepted leak class (b) (see D-070): codegen visits
        // this `Assign` exactly once, when `s` is not yet in `locals`, so
        // `decref_old_str_if_reassigning` never fires for it and no decref is
        // emitted into the loop body at all. At runtime every iteration but
        // the last therefore overwrites `s`'s slot without freeing its
        // predecessor's `"x"` -- a bounded, memory-safe per-iteration leak;
        // only the final iteration's value is later freed, by the top-level
        // completion pass, which is exactly what this test's name describes.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "i".to_string(),
                    value: MirExpr::IntLiteral(0),
                }),
                MirItem::TopLevelStmt(MirStmt::While {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::Name { name: "i".to_string(), ty: Ty::Int }),
                        right: Box::new(MirExpr::IntLiteral(3)),
                        ty: Ty::Bool,
                    },
                    body: vec![
                        MirStmt::Assign {
                            target: "s".to_string(),
                            value: MirExpr::StringLiteral("x".to_string()),
                        },
                        MirStmt::Assign {
                            target: "i".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Add,
                                left: Box::new(MirExpr::Name { name: "i".to_string(), ty: Ty::Int }),
                                right: Box::new(MirExpr::IntLiteral(1)),
                                ty: Ty::Int,
                            },
                        },
                    ],
                }),
            ],
        };
        let dir = tempfile_dir("str_first_assign_in_while_body");
        let obj_path = dir.join("str_first_assign_in_while_body.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_while_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_never_assigned_on_the_taken_path_decrefs_a_clean_null_at_completion() {
        // `flag = ""; if flag: s = "hi"` -- `flag` is falsy (the empty
        // string), so the `then` block containing `s`'s only assignment
        // never runs at all. This exercises the *other* half of the same
        // review finding the three tests above cover: hoisting `s`'s
        // `alloca` to the entry block only fixes *dominance*; it's the
        // explicit `store null` `alloca_str_at_entry` also does at entry
        // that keeps this path safe, since `s`'s slot genuinely holds that
        // null (never LLVM `undef`) when the top-level completion loop
        // loads and decrefs it -- `pycc_rt_str_decref`'s own null guard
        // then safely no-ops. Without the null store (hoisting the
        // `alloca` alone), this exact path would load `undef` and hand it
        // to `pycc_rt_str_decref`, which is undefined behavior no null
        // guard can catch -- so this is the one test that would actually
        // fail (or crash/UB, not just panic cleanly) if the null store were
        // ever dropped while the hoist itself stayed in place.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "flag".to_string(),
                    value: MirExpr::StringLiteral(String::new()),
                }),
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::Name { name: "flag".to_string(), ty: Ty::Str },
                    body: vec![MirStmt::Assign {
                        target: "s".to_string(),
                        value: MirExpr::StringLiteral("hi".to_string()),
                    }],
                    orelse: vec![],
                }),
            ],
        };
        let dir = tempfile_dir("str_never_assigned_on_taken_path");
        let obj_path = dir.join("str_never_assigned_on_taken_path.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_never_assigned_on_taken_path");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing (null-guarded decref of a never-assigned slot)");
    }

    #[test]
    fn a_str_local_first_assigned_inside_a_functions_own_leading_if_body() {
        // `def f() -> None:\n    if True:\n        s = "hi"` ; `f()` --
        // regression test for a gap left over from making module-level
        // bindings real LLVM globals (this task's own fix): every one of
        // this file's *top-level* `str`-hoisting tests above now pre-seeds
        // its target as a module global before any statement runs, so
        // `emit_assign`'s `None` branch (and therefore
        // `alloca_str_at_entry`) never fires for them anymore -- a
        // function-local `str`, which still goes through a real per-call
        // `alloca`, is the only remaining way to reach it. The `if` is
        // deliberately `f`'s *first* statement: emitting its conditional
        // branch terminates `f`'s own entry block before `s`'s `alloca` is
        // hoisted there, exercising `alloca_str_at_entry`'s `Some(terminator)`
        // branch (insert *before* the existing terminator) rather than its
        // `None` branch (`compiles_a_function_with_a_str_parameter_and_str_
        // return_value`'s own entry block, by contrast, is never terminated
        // before such a hoist since its body has no leading control flow).
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::If {
                        test: MirExpr::BoolLiteral(true),
                        body: vec![MirStmt::Assign {
                            target: "s".to_string(),
                            value: MirExpr::StringLiteral("hi".to_string()),
                        }],
                        orelse: vec![],
                    }],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("str_first_assign_in_fn_leading_if");
        let obj_path = dir.join("str_first_assign_in_fn_leading_if.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_fn_leading_if");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_first_assigned_as_a_functions_own_plain_leading_statement() {
        // `def f() -> None:\n    s = "hi"` ; `f()` -- distinct region from
        // the leading-`if` test above: exercises `alloca_str_at_entry`'s
        // `None` branch (`entry_block.get_terminator()` is still `None` at
        // the point of the hoist, since a plain `Assign` -- unlike an `if`
        // -- builds no terminator of its own) rather than its
        // `Some(terminator)` branch.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Assign {
                        target: "s".to_string(),
                        value: MirExpr::StringLiteral("hi".to_string()),
                    }],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("str_first_assign_in_fn_plain");
        let obj_path = dir.join("str_first_assign_in_fn_plain.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_fn_plain");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn an_int_local_assigned_in_both_branches_of_a_functions_own_leading_if_else() {
        // `def f() -> int:\n    if True:\n        x = 1\n    else:\n        x = 2\n    return x`
        // ; `print(f())` -- must print `1`. The `int` counterpart of the
        // `str` test directly above, for the same reason: every top-level
        // `int`-hoisting regression test in this file is now pre-seeded as
        // a module global too, so a function-local `int` is the only
        // remaining way to reach `alloca_at_entry`'s `Some(terminator)`
        // branch, and this additionally proves the reused-slot path (both
        // branches assign the same new local) still works once the slot
        // itself is hoisted to a nested function's own entry block, not
        // just `main`'s.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::If {
                            test: MirExpr::BoolLiteral(true),
                            body: vec![MirStmt::Assign { target: "x".to_string(), value: MirExpr::IntLiteral(1) }],
                            orelse: vec![MirStmt::Assign { target: "x".to_string(), value: MirExpr::IntLiteral(2) }],
                        },
                        MirStmt::Return(Some(MirExpr::Name { name: "x".to_string(), ty: Ty::Int })),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("int_first_assign_in_fn_leading_if_else");
        let obj_path = dir.join("int_first_assign_in_fn_leading_if_else.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("int_first_assign_in_fn_leading_if_else");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_float_local_first_assigned_inside_a_functions_own_body_widens_via_alloca_at_entry() {
        // `def f() -> float:\n    y = 2.5\n    return y` ; `print(f())` --
        // exercises `alloca_at_entry`'s `Scalar::Float` arm specifically
        // (distinct region from the `Int`/`Bool` arms already covered by
        // the tests above): a function-local `float`, first assigned as a
        // plain (non-nested) statement, still goes through
        // `emit_assign`'s `None` branch and `alloca_at_entry`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Float,
                    body: vec![
                        MirStmt::Assign { target: "y".to_string(), value: MirExpr::FloatLiteral(2.5) },
                        MirStmt::Return(Some(MirExpr::Name { name: "y".to_string(), ty: Ty::Float })),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call { callee: "f".to_string(), args: vec![], ty: Ty::Float }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("float_first_assign_in_fn");
        let obj_path = dir.join("float_first_assign_in_fn.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("float_first_assign_in_fn");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2.5\n");
    }

    #[test]
    fn compiles_a_string_comparison() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "b".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::StringLiteral("apple".to_string())),
                    right: Box::new(MirExpr::StringLiteral("banana".to_string())),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("str_cmp");
        let obj_path = dir.join("str_cmp.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn a_string_comparison_result_is_correct_at_runtime() {
        // `if "a" < "b": print(1)` -- unlike `compiles_a_string_comparison`
        // above (which only proves codegen for a `str` `Compare` succeeds),
        // this proves `pycc_rt_str_cmp`'s lexicographic ordering actually
        // drives a real `if` branch decision correctly, in both directions.
        for (left, right, expected) in [("a", "b", "1\n"), ("b", "a", "")] {
            let mir = MirModule {
                items: vec![MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::StringLiteral(left.to_string())),
                        right: Box::new(MirExpr::StringLiteral(right.to_string())),
                        ty: Ty::Bool,
                    },
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::IntLiteral(1)],
                        ty: Ty::None,
                    })],
                    orelse: vec![],
                })],
            };
            let dir = tempfile_dir(&format!("str_cmp_runtime_{left}_{right}"));
            let obj_path = dir.join("str_cmp_runtime.o");
            compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
            let bin_path = dir.join("str_cmp_runtime");
            link_object_with_runtime(&obj_path, &bin_path);
            let output = Command::new(&bin_path).output().expect("binary should run");
            assert_eq!(output.stdout, expected.as_bytes(), "comparing {left:?} < {right:?}");
        }
    }

    #[test]
    fn compiles_the_remaining_string_comparison_operators() {
        // `Lt` already has its own dedicated test above
        // (`compiles_a_string_comparison`); this exercises the rest of the
        // `str` branch's `IntPredicate` match arms
        // (`Eq`/`NotEq`/`LtE`/`Gt`/`GtE`), mirroring
        // `compiles_the_remaining_comparison_operators`'s `int` coverage and
        // `compiles_the_remaining_float_comparison_operators`'s `float` one.
        fn assign_compare(target: &str, op: CmpOpKind) -> MirStmt {
            MirStmt::Assign {
                target: target.to_string(),
                value: MirExpr::Compare {
                    op,
                    left: Box::new(MirExpr::StringLiteral("a".to_string())),
                    right: Box::new(MirExpr::StringLiteral("b".to_string())),
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
        let dir = tempfile_dir("remaining_str_cmp_ops");
        let obj_path = dir.join("remaining_str_cmp_ops.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "internal error: str Compare operand did not evaluate to str")]
    fn a_mixed_int_and_string_comparison_hits_the_internal_consistency_check() {
        // `1 < "x"` -- deliberately malformed MIR (`pycc_types` never mixes
        // `int`/`str` operands in one comparison): `left_ty` alone is
        // `Ty::Int` here, so only evaluating `right_ty` decides this enters
        // the `Compare` arm's `str` branch (exercising that `||`'s
        // right-hand disjunct, mirroring
        // `compiles_a_mixed_int_and_float_comparison`'s identical
        // left-Int/right-Float construction) -- and since the left operand
        // genuinely evaluates to `Scalar::Int`, this also isolates the
        // `str` branch's *left*-operand internal-consistency check.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "b".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Lt,
                    left: Box::new(MirExpr::IntLiteral(1)),
                    right: Box::new(MirExpr::StringLiteral("x".to_string())),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("mixed_int_str_cmp_panics");
        let obj_path = dir.join("mixed_int_str_cmp_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "internal error: str Compare operand did not evaluate to str")]
    fn a_string_comparison_with_a_lying_right_operand_hits_the_internal_consistency_check() {
        // Deliberately malformed MIR, isolating the `Compare` arm's str
        // branch's *right*-operand check specifically (the test above only
        // ever reaches the *left*-operand one, since that check runs
        // first): the right operand is a nested `Compare` node that claims
        // `ty: Ty::Str` but -- like every `Compare` node, regardless of its
        // own `ty` field -- always evaluates to `Scalar::Bool` (`emit_expr`'s
        // `Compare` arm never reads its own `ty` when constructing its
        // result; only a *parent* expression's `left.ty()`/`right.ty()`
        // call ever inspects it). This makes `right_ty == Ty::Str` true
        // (entering the branch) while `r` itself evaluates to `Scalar::Bool`
        // -- with the left operand a real `str`, so the left-operand check
        // passes and only the right-operand one fires. Same "nested lying
        // node" convention as this file's other internal-consistency tests
        // (e.g. `printing_a_mistyped_compare_expression_hits_the_internal_consistency_check`).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "b".to_string(),
                value: MirExpr::Compare {
                    op: CmpOpKind::Eq,
                    left: Box::new(MirExpr::StringLiteral("x".to_string())),
                    right: Box::new(MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::IntLiteral(1)),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Str,
                    }),
                    ty: Ty::Bool,
                },
            })],
        };
        let dir = tempfile_dir("lying_str_cmp_panics");
        let obj_path = dir.join("lying_str_cmp_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_an_if_test_on_a_string_expression() {
        // `if "": print(1)` prints nothing; `if "x": print(1)` prints `1`.
        for (test, expected) in [("", ""), ("x", "1\n")] {
            let mir = MirModule {
                items: vec![MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::StringLiteral(test.to_string()),
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::IntLiteral(1)],
                        ty: Ty::None,
                    })],
                    orelse: vec![],
                })],
            };
            let dir = tempfile_dir(&format!("str_truthy_{}", test.len()));
            let obj_path = dir.join("str_truthy.o");
            compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
            let bin_path = dir.join("str_truthy");
            link_object_with_runtime(&obj_path, &bin_path);
            let output = Command::new(&bin_path).output().expect("binary should run");
            assert_eq!(output.stdout, expected.as_bytes(), "test value {test:?}");
        }
    }

    #[test]
    fn compiles_a_string_literal_longer_than_the_inline_cap() {
        let long = "y".repeat(30); // exceeds D-059's 22-byte inline threshold
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "s".to_string(),
                value: MirExpr::StringLiteral(long),
            })],
        };
        let dir = tempfile_dir("str_long_literal");
        let obj_path = dir.join("str_long_literal.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_a_function_with_a_str_parameter_and_str_return_value() {
        // `def f(x: str) -> str: return x` ; `y = f("hi")` -- exercises
        // `ty_to_basic_type`'s `Ty::Str` arm (both the parameter and
        // return-type positions), `build_call_to`'s argument-marshaling
        // match's `Scalar::Str` arm (plus `incref_if_str_duplicate`'s
        // duplicate-reference branch on the `"hi"` literal argument, which
        // is *not* a duplicate reference, exercising its `else` half),
        // `emit_expr`'s `Call` arm's `Ty::Str`-result match arm, and
        // `emit_stmt`'s `Return` arm's `Scalar::Str` arm together with
        // `incref_if_str_duplicate`'s duplicate-reference branch on `return
        // x` (a bare `Name`, which *is* a duplicate reference, exercising
        // its `if` half) -- every `str`-typed position Task 5/6's
        // float-parameter precedent
        // (`compiles_a_function_with_a_float_parameter_and_float_return_value`)
        // established for `float`, now closed for `str`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Str)],
                    return_ty: Ty::Str,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Str,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::StringLiteral("hi".to_string())],
                        ty: Ty::Str,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("str_param_and_return");
        let obj_path = dir.join("str_param_and_return.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("str_param_and_return");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    #[should_panic(expected = "expected an int-or-bool operand, got str")]
    fn an_int_result_binop_with_a_str_operand_hits_to_tagged_int_defensive_panic() {
        // Deliberately malformed MIR: `pycc_types::numeric_result_type`
        // never types a `str`-operand `BinOp` as `Ty::Int` (`str` only ever
        // combines with `str`, under `Add`, per its own
        // `adding_two_strings_infers_str` test), so no real pipeline could
        // ever produce this shape. Exercises `to_tagged_int`'s own
        // defensive `Scalar::Str` arm -- same convention as
        // `an_int_result_binop_with_a_float_operand_hits_to_tagged_int_defensive_panic`
        // above, now for `str` instead of `float`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::StringLiteral("x".to_string())),
                    right: Box::new(MirExpr::IntLiteral(1)),
                    ty: Ty::Int,
                },
            })],
        };
        let dir = tempfile_dir("binop_int_result_str_operand_panics");
        let obj_path = dir.join("binop_int_result_str_operand_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    #[should_panic(expected = "expected a numeric operand, got str")]
    fn a_float_result_binop_with_a_str_operand_hits_to_float_defensive_panic() {
        // Same rationale as the `to_tagged_int` version above, exercising
        // `to_float`'s own defensive `Scalar::Str` arm instead (a brand-new
        // arm this task adds -- `to_float`'s match was previously exhaustive
        // over `Int`/`Bool`/`Float` alone, with no catch-all to fill in).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(MirExpr::StringLiteral("x".to_string())),
                    right: Box::new(MirExpr::FloatLiteral(1.0)),
                    ty: Ty::Float,
                },
            })],
        };
        let dir = tempfile_dir("binop_float_result_str_operand_panics");
        let obj_path = dir.join("binop_float_result_str_operand_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_an_f_string_interpolating_an_int_between_literal_parts() {
        // `x = 5; s = f"n={x}!"` -- `s` would hold `"n=5!"`.
        //
        // Deviations from the task brief, both in this test:
        //
        // 1. The brief's own version wrote `pycc_hir::Ty::Int`/`pycc_hir::
        //    Ty::None` -- but `pycc_hir` is not a dependency of this crate
        //    (only `pycc_mir` is, per `Cargo.toml`), and Rust doesn't
        //    resolve an indirect crate's name from a `pub use` re-export
        //    alone (`pycc_mir::Ty` is the exact same type as `pycc_hir::
        //    Ty`, but the bare path `pycc_hir::` itself isn't in scope
        //    here). Fixed to use the plain `Ty` already imported from
        //    `pycc_mir` at this module's own top (`use pycc_mir::{BinOpKind,
        //    CmpOpKind, MirExpr, MirItem, MirModule, MirStmt, Ty};`),
        //    matching every other test in this file.
        //
        // 2. The brief's own version wrapped the f-string in `print(...)`
        //    instead of a plain `Assign`. `emit_stmt`'s own `print()` arm
        //    (a few hundred lines above) only accepts a *single, `Ty::Int`-
        //    typed* argument today -- any other shape, including a single
        //    `Ty::Str`-typed argument, falls through to its own documented
        //    "this print() argument shape is not supported yet (multi-arg /
        //    non-int print lands in Task 10)" panic, confirmed empirically:
        //    with the brief's own `print(f"n={x}!")` shape, this test
        //    failed with exactly that panic instead of proving f-string
        //    codegen itself works. Wiring a `str`-typed argument into
        //    `print` is explicitly Task 10's job (this file's own doc
        //    comment on `emit_stmt`, and the plan's own Task 10 scope) --
        //    implementing it here would reach into a later task's scope.
        //    Changed to a plain `Assign` (`s = f"..."`), the same shape
        //    already used by this brief's own next two tests below, so this
        //    test actually exercises `MirExpr::FString`'s own codegen
        //    without depending on unfinished `print` dispatch.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(5),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Literal("n=".to_string()),
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        })),
                        pycc_mir::MirFStringPart::Literal("!".to_string()),
                    ]),
                }),
            ],
        };
        let dir = tempfile_dir("fstring_int");
        let obj_path = dir.join("fstring_int.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_an_f_string_interpolating_a_float_and_a_bool() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "s".to_string(),
                value: MirExpr::FString(vec![
                    pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::FloatLiteral(2.5))),
                    pycc_mir::MirFStringPart::Literal(" ".to_string()),
                    pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::BoolLiteral(true))),
                ]),
            })],
        };
        let dir = tempfile_dir("fstring_float_bool");
        let obj_path = dir.join("fstring_float_bool.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_an_f_string_with_only_literal_parts() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "s".to_string(),
                value: MirExpr::FString(vec![pycc_mir::MirFStringPart::Literal(
                    "no interpolation".to_string(),
                )]),
            })],
        };
        let dir = tempfile_dir("fstring_literal_only");
        let obj_path = dir.join("fstring_literal_only.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn compiles_an_f_string_interpolating_an_existing_str_value() {
        // `s = "hi"; t = f"{s} there"` -- added beyond the task brief's own
        // three tests above: none of those ever interpolates an
        // already-`str`-typed value, so `to_str`'s `Scalar::Str` passthrough
        // arm (`return v` -- no `pycc_rt_*_to_str` conversion call at all)
        // would otherwise never execute, an uncovered region under this
        // project's 100%-line-and-region coverage gate (D-014). Also
        // exercises `incref_if_str_duplicate`'s true branch for a bare
        // `Name` read inside an interpolation (needed so the f-string's own
        // final decref of every non-literal part doesn't underflow `s`'s
        // refcount below what its own binding still owns).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::StringLiteral("hi".to_string()),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "t".to_string(),
                    value: MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "s".to_string(),
                            ty: Ty::Str,
                        })),
                        pycc_mir::MirFStringPart::Literal(" there".to_string()),
                    ]),
                }),
            ],
        };
        let dir = tempfile_dir("fstring_str_passthrough");
        let obj_path = dir.join("fstring_str_passthrough.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
    }

    #[test]
    fn interpolating_a_none_returning_call_in_an_f_string_renders_none_not_false() {
        // `def f() -> None:\n    return` ; `s = f"got: {f()}"` ; `print(s)`
        // -- must print `"got: None"`. Before this fix, a `None`-typed
        // interpolation's placeholder `Scalar::Bool(0)` (see `emit_expr`'s
        // `Call` arm doc comment) flowed straight into `to_str`, which has
        // no way to tell it apart from a genuine `False` -- rendering
        // `"got: False"` instead.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Literal("got: ".to_string()),
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Call {
                            callee: "f".to_string(),
                            args: vec![],
                            ty: Ty::None,
                        })),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "s".to_string(), ty: Ty::Str }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("fstring_none_call");
        let obj_path = dir.join("fstring_none_call.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("fstring_none_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"got: None\n");
    }

    #[test]
    #[should_panic(
        expected = "interpolating a `None`-typed value that isn't a direct call result is not supported yet"
    )]
    fn interpolating_a_none_typed_name_that_isnt_a_direct_call_result_panics() {
        // Mirrors `printing_a_none_typed_name_that_isnt_a_direct_call_result_
        // panics`'s own deliberately malformed MIR (a `Name` bound to a
        // `None`-typed variable is not real `pycc_types` output -- see that
        // test's own doc comment), exercising the same defensive guard on
        // the f-string interpolation path instead of `print`'s.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "s".to_string(),
                value: MirExpr::FString(vec![pycc_mir::MirFStringPart::Interpolation(Box::new(
                    MirExpr::Name { name: "y".to_string(), ty: Ty::None },
                ))]),
            })],
        };
        let dir = tempfile_dir("fstring_none_typed_name_panics");
        let obj_path = dir.join("fstring_none_typed_name_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None);
    }

    #[test]
    fn compiles_a_loop_whose_accumulator_overflows_into_a_bigint() {
        // `i = 0; acc = 4611686018427387903; while i < 3: acc = acc + acc; i = i + 1`
        // `print(acc)` -- starts at `i64::MAX >> 1` and doubles 3 times,
        // overflowing well past `i64::MAX` partway through; must print the
        // exact mathematical result via real bigint arithmetic, not a
        // wrapped/truncated one.
        let start: i64 = i64::MAX >> 1;
        let expected = (start as i128) * 8; // doubled 3 times
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "acc".to_string(),
                    value: MirExpr::IntLiteral(start),
                }),
                MirItem::TopLevelStmt(MirStmt::ForRange {
                    var: "i".to_string(),
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(3),
                    step: MirExpr::IntLiteral(1),
                    body: vec![MirStmt::Assign {
                        target: "acc".to_string(),
                        value: MirExpr::BinOp {
                            op: BinOpKind::Add,
                            left: Box::new(MirExpr::Name { name: "acc".to_string(), ty: Ty::Int }),
                            right: Box::new(MirExpr::Name { name: "acc".to_string(), ty: Ty::Int }),
                            ty: Ty::Int,
                        },
                    }],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name { name: "acc".to_string(), ty: Ty::Int }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bigint_overflow_loop");
        let obj_path = dir.join("bigint_overflow_loop.o");
        compile_to_object(&mir, &obj_path, None).expect("codegen should succeed");
        let bin_path = dir.join("bigint_overflow_loop");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, format!("{expected}\n").into_bytes());
    }

    fn tempfile_dir(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pycc_codegen_test_{label}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Test-only linking helper. `pycc`'s real CLI (Task 8) does this via
    /// `cc`/clang (see `src/main.rs`'s `linker_command`/`effective_link_target`/
    /// `add_windows_system_libs`/`add_linux_system_libs`); duplicated
    /// minimally here so pycc_codegen's own tests can prove the object file
    /// it produces actually links and runs, without depending on the `pycc`
    /// binary crate (that would be a dependency cycle: pycc depends on
    /// pycc_codegen, not the other way around). Needs the same Windows
    /// handling as `main.rs`, and for the same reasons: there's no default
    /// `cc` there (D-028) -- on this runner it silently resolved to
    /// MinGW's `gcc`, which cannot link the MSVC-ABI `pycc_rt.lib` (the
    /// exact "undefined reference to `__imp_...`"/`collect2` wall D-028
    /// already diagnosed for `main.rs`, reproduced here because this
    /// helper wasn't covered by that fix); clang's bare-invocation default
    /// target also proved unreliable (D-028), so `-target` must be
    /// explicit too. Needs the same Linux handling too, for the same
    /// reason `main.rs` does (`f64::powf` -> libm's `pow`, not linked by
    /// GCC's/clang's default driver invocation): this helper's own
    /// `undefined reference to 'pow'` failure on both Linux architectures
    /// wasn't covered by that fix either, since it's a separate linker
    /// invocation from `main.rs`'s.
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

        #[cfg(target_os = "linux")]
        cmd.arg("-lm");

        let status = cmd.status().expect("the linker driver should run");
        assert!(status.success(), "linking failed");
    }
}
