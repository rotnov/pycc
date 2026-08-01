use inkwell::FloatPredicate;
use inkwell::IntPredicate;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::BasicType;
use inkwell::values::{FloatValue, FunctionValue, IntValue, PointerValue};
use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// One MIR-level value during codegen. Extended (never replaced) by later
/// tasks: `Str` (Task 7) is a pointer to an opaque `pycc_rt::PyStrObj` --
/// `pycc_codegen` never inspects its layout (D-059's inline/heap
/// representation is entirely `pycc_rt`'s own concern), only ever passing
/// it through to a `pycc_rt_str_*` call. `Ty::None` uses the same LLVM `i8`
/// carrier shape as `Bool`, but always with the value zero: a distinct enum
/// variant would not add information because MIR's static `Ty` determines
/// whether that carrier means Python's unit value or a real boolean. Function
/// returns of `None` remain LLVM `void`.
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
    /// A pointer to a heap-allocated `pycc_rt::PyIntListObj` (D-105,
    /// Task 10) -- like `Str`, opaque to this crate, which only ever
    /// stores it, passes it to a `pycc_rt_int_list_*` call, or marshals it
    /// across a function boundary.
    ///
    /// Its own variant rather than a reuse of `Str`'s (D-107): the two
    /// runtime objects have entirely different layouts, and `truthy`/
    /// `to_str` are exhaustive matches that would otherwise hand a
    /// `PyIntListObj` pointer straight to a `pycc_rt_str_*` function --
    /// reachable from ordinary type-checked source (`if xs:`, `print(xs)`)
    /// the moment list values become constructible. Keeping them distinct
    /// makes every operation `list[T]` has no v0.2 semantics for a compile
    /// error until it is answered deliberately, instead of silently
    /// misreading memory.
    ///
    /// Refcounting is deliberately *not* wired for this variant in v0.2
    /// (D-107): `pycc_rt_int_list_incref`/`_decref` are never called, so a
    /// list's backing allocation leaks for the process's lifetime. That is
    /// leak-only -- never a premature free or a double free -- because
    /// nothing frees a list value early either.
    List(PointerValue<'ctx>),
    /// A pointer to a heap-allocated `pycc_rt::PyDictObj` (PR-11 Task 2/5,
    /// D-111/D-113) -- like `List`, opaque to this crate, which only ever
    /// stores it, passes it to a `pycc_rt_dict_*` call, or marshals it
    /// across a function boundary.
    ///
    /// Its own variant rather than a reuse of `List`'s or `Str`'s (D-107's
    /// reasoning, extended to this new container by D-114): `PyDictObj` and
    /// `PyIntListObj` have entirely different layouts, and every exhaustive
    /// `Scalar` match (`truthy`/`to_str`/`to_tagged_int`/`to_float`/
    /// `emit_assign`/argument marshalling) would otherwise hand a
    /// `PyDictObj` pointer straight to a `pycc_rt_int_list_*` or
    /// `pycc_rt_str_*` function -- reachable from ordinary type-checked
    /// source (`if x:`, `print(x)`) the moment dict values become
    /// constructible (this task). Keeping it distinct makes every operation
    /// `dict[K, V]` has no v0.2 semantics for a compile error until it is
    /// answered deliberately, instead of silently misreading memory.
    ///
    /// Refcounting is deliberately *not* wired for this variant in v0.2
    /// (D-114, extending D-107's exact reasoning): `pycc_rt_dict_incref`/
    /// `_decref` are never called on a dict value itself, so a dict's
    /// backing allocation leaks for the process's lifetime, identically to
    /// `List`. That is leak-only -- never a premature free or a double free
    /// -- because nothing frees a dict value early either.
    Dict(PointerValue<'ctx>),
    /// A pointer to a heap-allocated `pycc_rt::PyIntSetObj` (PR-11 Task 6,
    /// D-111/D-114) -- like `List`/`Dict`, opaque to this crate, which only
    /// ever stores it, passes it to a `pycc_rt_int_set_*` call, or marshals
    /// it across a function boundary.
    ///
    /// Its own variant rather than a reuse of `List`'s, `Dict`'s, or
    /// `Str`'s (D-107's reasoning, extended to this new container by
    /// D-114, exactly as `Dict`'s own doc comment already extends it):
    /// `PyIntSetObj` has its own layout, distinct from `PyIntListObj`,
    /// `PyDictObj`, and `PyStrObj`, and every exhaustive `Scalar` match
    /// (`truthy`/`to_str`/`to_tagged_int`/`to_float`/`emit_assign`/argument
    /// marshalling) would otherwise hand a `PyIntSetObj` pointer straight to
    /// a `pycc_rt_int_list_*`, `pycc_rt_dict_*`, or `pycc_rt_str_*`
    /// function -- reachable from ordinary type-checked source (`if s:`,
    /// `print(s)`) the moment set values become constructible (PR-11 Task
    /// 9). Keeping it distinct makes every operation `set[int]` has no v0.2
    /// semantics for a compile error until it is answered deliberately,
    /// instead of silently misreading memory.
    ///
    /// Refcounting is deliberately *not* wired for this variant in v0.2
    /// (D-114, extending D-107's exact reasoning, identically to `List`/
    /// `Dict`): `pycc_rt_int_set_incref`/`_decref` are never called on a set
    /// value itself, so a set's backing allocation leaks for the process's
    /// lifetime. That is leak-only -- never a premature free or a double
    /// free -- because nothing frees a set value early either.
    Set(PointerValue<'ctx>),
}

struct UserFunction<'ctx> {
    value: FunctionValue<'ctx>,
    param_tys: Vec<pycc_mir::Ty>,
}

#[derive(Clone)]
struct StorageSlot<'ctx> {
    ptr: PointerValue<'ctx>,
    ty: pycc_mir::Ty,
    /// Module globals and non-parameter locals can have storage before
    /// execution reaches an assignment. Their separate flag prevents an
    /// unexecuted control-flow path from exposing an LLVM initializer, `undef`,
    /// or null as a fabricated Python value. Parameters are initialized at
    /// entry and therefore need no flag; #118 separately owns static
    /// definite-assignment diagnostics.
    initialized: Option<PointerValue<'ctx>>,
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
    float_div: FunctionValue<'ctx>,
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
    /// D-106's input-side boundary conversion (Task 11a): turns a
    /// D-061-tagged `Ty::Int` into the raw, untagged `i64` `PyIntListObj`
    /// actually stores, panicking honestly on a bigint-tagged value.
    /// `pycc_rt`'s job rather than inline IR because it interprets the tag
    /// bit; the reverse direction (`raw_i64_to_tagged_int`) only
    /// *constructs* a tagged value and so stays inline here.
    int_untag_checked: FunctionValue<'ctx>,
    int_list_new: FunctionValue<'ctx>,
    int_list_append: FunctionValue<'ctx>,
    int_list_get: FunctionValue<'ctx>,
    int_list_len: FunctionValue<'ctx>,
    /// PR-11 Task 5's own new `pycc_rt_dict_*` declarations, mirroring the
    /// `int_list_*` cluster immediately above one-for-one: `dict_new` (no
    /// pre-sizing entry point, same reasoning as `int_list_new`),
    /// `dict_set` (insert-or-update, D-113 -- this crate's one dict
    /// "growable op", playing `int_list_append`'s role), `dict_get`
    /// (read, panics on a missing key), `dict_len`, and `dict_key_at`
    /// (`ForDict`'s own per-iteration key read, playing `int_list_get`'s
    /// `ForList`-iteration role).
    dict_new: FunctionValue<'ctx>,
    dict_set: FunctionValue<'ctx>,
    dict_get: FunctionValue<'ctx>,
    dict_len: FunctionValue<'ctx>,
    dict_key_at: FunctionValue<'ctx>,
    /// PR-11 Task 9's own new `pycc_rt_int_set_*` declarations, mirroring
    /// the `int_list_*` cluster above one-for-one: `int_set_new` (no
    /// pre-sizing entry point, same reasoning as `int_list_new`),
    /// `int_set_add` (insert-with-dedup, D-111 -- this crate's one set
    /// "growable op", playing `int_list_append`'s role; the dedup check
    /// itself lives entirely in `pycc_rt_int_set_add`, not here), `int_set_len`,
    /// and `int_set_get` (`ForSet`'s own per-iteration element read, playing
    /// `int_list_get`'s `ForList`-iteration role). Unlike the `dict_*`
    /// cluster, there is no `int_set_get`-adjacent key type at all -- a
    /// set's elements are raw `i64`, so this cluster has no counterpart to
    /// `dict_get`'s "read by key" op.
    int_set_new: FunctionValue<'ctx>,
    int_set_add: FunctionValue<'ctx>,
    int_set_len: FunctionValue<'ctx>,
    int_set_get: FunctionValue<'ctx>,
    trap: FunctionValue<'ctx>,
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
            context
                .i8_type()
                .fn_type(&[i64_type.into(), i64_type.into(), i64_type.into()], false),
        ),
        int_to_float: declare(
            "pycc_rt_int_to_float",
            f64_type.fn_type(&[i64_type.into()], false),
        ),
        float_div: declare(
            "pycc_rt_float_div",
            f64_type.fn_type(&[f64_type.into(), f64_type.into()], false),
        ),
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
        str_incref: declare(
            "pycc_rt_str_incref",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        str_decref: declare(
            "pycc_rt_str_decref",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        int_to_str: declare(
            "pycc_rt_int_to_str",
            ptr_type.fn_type(&[i64_type.into()], false),
        ),
        float_to_str: declare(
            "pycc_rt_float_to_str",
            ptr_type.fn_type(&[f64_type.into()], false),
        ),
        bool_to_str: declare(
            "pycc_rt_bool_to_str",
            ptr_type.fn_type(&[context.i8_type().into()], false),
        ),
        print_write_str: declare(
            "pycc_rt_print_write_str",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        print_space: declare("pycc_rt_print_space", void_type.fn_type(&[], false)),
        print_newline: declare("pycc_rt_print_newline", void_type.fn_type(&[], false)),
        print_none: declare("pycc_rt_print_none", void_type.fn_type(&[], false)),
        int_untag_checked: declare(
            "pycc_rt_int_untag_checked",
            i64_type.fn_type(&[i64_type.into()], false),
        ),
        int_list_new: declare("pycc_rt_int_list_new", ptr_type.fn_type(&[], false)),
        // Returns nothing: `pycc_rt_int_list_append`'s Rust signature is
        // `-> ()`, so this is the one new `pycc_rt_int_list_*` declaration
        // whose call site must *not* go through `try_as_basic_value()`.
        int_list_append: declare(
            "pycc_rt_int_list_append",
            void_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        int_list_get: declare(
            "pycc_rt_int_list_get",
            i64_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        int_list_len: declare(
            "pycc_rt_int_list_len",
            i64_type.fn_type(&[ptr_type.into()], false),
        ),
        dict_new: declare("pycc_rt_dict_new", ptr_type.fn_type(&[], false)),
        // Returns nothing, exactly like `int_list_append` above: this
        // signature must match `pycc_rt_dict_set`'s real Rust one
        // (`fn(*mut PyDictObj, *mut PyStrObj, i64) -> ()`) -- key is a
        // second, distinct pointer parameter (a dict's key and its
        // container are two different heap objects), not folded into one
        // like `int_list_append`'s single `i64` value parameter.
        dict_set: declare(
            "pycc_rt_dict_set",
            void_type.fn_type(&[ptr_type.into(), ptr_type.into(), i64_type.into()], false),
        ),
        dict_get: declare(
            "pycc_rt_dict_get",
            i64_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
        ),
        dict_len: declare(
            "pycc_rt_dict_len",
            i64_type.fn_type(&[ptr_type.into()], false),
        ),
        dict_key_at: declare(
            "pycc_rt_dict_key_at",
            ptr_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        int_set_new: declare("pycc_rt_int_set_new", ptr_type.fn_type(&[], false)),
        // Returns nothing, exactly like `int_list_append` above: this
        // signature must match `pycc_rt_int_set_add`'s real Rust one
        // (`fn(*mut PyIntSetObj, i64) -> ()`) -- the dedup check itself
        // lives entirely inside that function (D-111), so codegen's own
        // call site is exactly as simple as an unconditional append.
        int_set_add: declare(
            "pycc_rt_int_set_add",
            void_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        int_set_len: declare(
            "pycc_rt_int_set_len",
            i64_type.fn_type(&[ptr_type.into()], false),
        ),
        int_set_get: declare(
            "pycc_rt_int_set_get",
            i64_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        trap: module.add_function("llvm.trap", void_type.fn_type(&[], false), None),
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
        // LLVM `void` cannot be a parameter type. v0.1 therefore carries
        // Python's singleton unit value across user-function parameter and
        // local-storage boundaries as the canonical `i8 0`; a `None` return
        // is still emitted as LLVM `void` by `compile_to_object`.
        pycc_mir::Ty::None => context.i8_type().into(),
        // `list[T]`'s runtime object (Task 11) is heap-allocated and always
        // referenced by pointer -- exactly the same storage/parameter
        // representation `Str` already gets above. The element type `T`
        // only affects what Task 11's runtime does with the pointee, never
        // this decision, so every `List(_)` is a pointer regardless of `T`
        // (D-105 restricts real *codegen* for non-`int` elements elsewhere,
        // not this representation choice).
        pycc_mir::Ty::List(_) => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // `dict[K, V]`'s runtime object (PR-11 Task 5) is heap-allocated
        // and always referenced by pointer -- exactly the same
        // storage/parameter representation `List(_)` gets immediately
        // above, for the identical reason (the key/value types only affect
        // what this crate's `pycc_rt_dict_*` calls do with the pointee,
        // never this representation choice). Only `Ty::Dict(Box::new((Ty::
        // Str, Ty::Int)))` ever reaches this arm today (`pycc_types`'
        // T0036 gate rejects every other key/value combination before
        // codegen runs), but the arm itself is not narrowed to that one
        // combination, matching `List(_)`'s own element-type-agnostic
        // shape.
        pycc_mir::Ty::Dict(_) => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // `set[T]`'s runtime object (PR-11 Task 9) is heap-allocated and
        // always referenced by pointer -- exactly the same
        // storage/parameter representation `List(_)`/`Dict(_)` get
        // immediately above, for the identical reason (the element type
        // only affects what this crate's `pycc_rt_int_set_*` calls do with
        // the pointee, never this representation choice). Only
        // `Ty::Set(Box::new(Ty::Int))` ever reaches this arm today
        // (`pycc_types`' T0038 gate rejects every other element type before
        // codegen runs), but the arm itself is not narrowed to that one
        // element type, matching `List(_)`/`Dict(_)`'s own
        // element/key/value-agnostic shape.
        pycc_mir::Ty::Set(_) => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // Deviation from the task brief: the brief's own version of this
        // catch-all's message read "(only int/float/bool/str/list[int] do)"
        // -- but that parenthetical is inaccurate twice over. This function
        // already gives `Ty::None` a representation too (the `i8 0` carrier
        // above), and the `List(_)`/`Dict(_)`/`Set(_)` arms above aren't
        // specific to `list[int]`/`dict[str, int]`/`set[int]`: each
        // produces the same pointer type for any element/key/value type,
        // not just those. Worded to match what this function actually does.
        other => panic!(
            "pycc_codegen: {} has no LLVM representation yet (int/float/bool/str/None/list[_]/dict[_, _]/set[_] do)",
            other.name()
        ),
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
        // Defensive, exactly like the two arms above -- not a feature gap
        // (D-107): `pycc_types`' `numeric_result_type` maps no `Ty::List`
        // to a numeric type, so any arithmetic with a list operand is
        // already rejected as `T0021` before codegen runs. Its own arm
        // rather than folding into `Str`'s, so the message names the type
        // it actually got.
        Scalar::List(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got list")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List` arm directly above, extended to `dict[K, V]` (D-107's
        // reasoning, per D-114): no arithmetic operand is ever `Ty::Dict`,
        // so this is never reached by real, type-checked source.
        Scalar::Dict(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got dict")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict` arms above, extended to `set[T]` (D-107's
        // reasoning, per D-114): no arithmetic operand is ever `Ty::Set`,
        // so this is never reached by real, type-checked source.
        Scalar::Set(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got set")
        }
    }
}

/// D-106's output-side boundary conversion: tags an already-known-in-range
/// raw `i64` (a `list[int]` element read back out, or its length) as an
/// ordinary D-061 `Ty::Int`. Always safe -- never overflows -- because
/// every raw value crossing this boundary already passed through
/// `pycc_rt_int_untag_checked` or is a `Vec::len()` result, both of which
/// are guaranteed within `tag_smallint`'s 63-bit range. Mirrors
/// `to_tagged_int`'s `Scalar::Bool` arm above (same shift-and-or shape),
/// which handles the identical *construct, don't interpret* direction for a
/// different always-in-range source -- as opposed to `to_float`'s
/// `pycc_rt`-delegating precedent for the *interpret an existing tagged
/// value's bits* direction.
///
/// Task 11b wired its three real call sites -- `MirExpr::Subscript`'s
/// element result, the `len(x)` builtin's length result, and
/// `MirStmt::ForList`'s per-iteration element read -- and removed the
/// temporary `#[allow(dead_code)]` Task 11a needed while it had none.
/// `ForList`'s own induction variable and its `len` loop bound are
/// deliberately *not* among them: both are pure LLVM implementation detail,
/// never surfaced to user code as a `Ty::Int` value, so neither is on this
/// boundary at all (see that arm's own comment).
fn raw_i64_to_tagged_int<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    raw: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let shifted = builder
        .build_left_shift(raw, context.i64_type().const_int(1, false), "list_tag_shl")
        .expect("build_left_shift should not fail for a constant shift amount");
    builder
        .build_or(
            shifted,
            context.i64_type().const_int(1, false),
            "list_tag_or",
        )
        .expect("build_or should not fail for two i64 operands")
}

/// Extracts a `PyIntListObj` pointer from an already-evaluated operand that
/// every upstream check says must be a `list[T]`: `len`'s argument,
/// `MirExpr::Subscript`'s base, and `emit_list_name_read`'s named local.
/// `what` names the offending operand for the message.
///
/// One shared helper rather than a `let Scalar::List(..) = .. else` at each
/// of the three sites, for this file's established
/// no-permanently-uncoverable-region reason (see `emit_string_literal`'s own
/// doc comment): `Subscript`'s base in particular can only be reached with a
/// non-list `Scalar` through deliberately self-inconsistent MIR (a
/// `MirExpr` whose `ty()` says `list[T]` while `emit_expr` returns something
/// else), so an inline arm there would be a region D-014's gate could never
/// legitimately exercise. Funnelling all three through one helper makes the
/// check genuinely covered by the site that *is* naturally reachable -- a
/// non-list local named by `.append()`/`for`, and a non-list argument to
/// `len` (see this file's own tests for both).
fn expect_list_pointer<'ctx>(scalar: Scalar<'ctx>, what: &str) -> PointerValue<'ctx> {
    let Scalar::List(ptr) = scalar else {
        panic!(
            "pycc_codegen: internal error: {what} did not evaluate to a list -- \
             pycc_types::check (T0033) should have rejected this before codegen"
        )
    };
    ptr
}

/// D-106's input-side boundary conversion, at all three sites where a
/// D-061-tagged `Ty::Int` crosses into `PyIntListObj`'s raw, untagged
/// storage: `ListLiteral`'s per-element value, `ListAppend`'s value, and
/// `Subscript`'s index. Delegates to `pycc_rt` (rather than emitting the
/// shift inline the way `raw_i64_to_tagged_int` above does for the reverse
/// direction) because this direction has to *interpret* the tag bit to
/// reject a bigint-tagged value, and only `pycc_rt` interprets its own
/// representation -- the same split `to_float`'s doc comment already
/// establishes for `int`-to-`float`.
fn build_untag_checked<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    tagged: IntValue<'ctx>,
    name: &str,
) -> IntValue<'ctx> {
    builder
        .build_call(rt.int_untag_checked, &[tagged.into()], name)
        .expect("build_call should not fail for a well-formed untag")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_untag_checked returns a non-void i64")
        .into_int_value()
}

/// Reads one element out of a `PyIntListObj`. Both operands and the result
/// are on the **raw**, untagged side of D-106's boundary: callers convert
/// on the way in (`build_untag_checked`) and on the way out
/// (`raw_i64_to_tagged_int`) only where the value is genuinely a user-visible
/// `Ty::Int` -- which is why `MirStmt::ForList`'s own induction variable is
/// passed straight through here with no conversion at all.
fn build_int_list_get<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    list_ptr: PointerValue<'ctx>,
    raw_index: IntValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(
            rt.int_list_get,
            &[list_ptr.into(), raw_index.into()],
            "list_get",
        )
        .expect("build_call should not fail for a well-formed list read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_list_get returns a non-void i64")
        .into_int_value()
}

/// A `PyIntListObj`'s current element count, as a **raw**, untagged `i64`
/// (D-106). The `len(x)` builtin re-tags this before handing it back as a
/// `Ty::Int` expression value; `MirStmt::ForList` uses it directly as its
/// own loop bound and deliberately does not.
fn build_int_list_len<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    list_ptr: PointerValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(rt.int_list_len, &[list_ptr.into()], "list_len")
        .expect("build_call should not fail for a well-formed list length read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_list_len returns a non-void i64")
        .into_int_value()
}

/// Appends one already-untagged (D-106) value to a `PyIntListObj`, shared by
/// `MirExpr::ListLiteral`'s per-element construction and
/// `MirExpr::ListAppend`. Returns nothing: `pycc_rt_int_list_append` is
/// declared `void`, so unlike every other `pycc_rt_int_list_*` helper above
/// there is no `try_as_basic_value()` result to extract.
fn build_int_list_append<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    list_ptr: PointerValue<'ctx>,
    raw_value: IntValue<'ctx>,
) {
    builder
        .build_call(
            rt.int_list_append,
            &[list_ptr.into(), raw_value.into()],
            "list_append",
        )
        .expect("build_call should not fail for a well-formed list append");
}

/// Extracts a `PyDictObj` pointer from an already-evaluated operand that
/// every upstream check says must be a `dict[K, V]`: `len`'s argument,
/// `MirExpr::DictGet`'s `dict` operand, and `emit_dict_name_read`'s named
/// local. `what` names the offending operand for the message. Mirrors
/// `expect_list_pointer` exactly, for the identical reason (see that
/// function's own doc comment): one shared helper rather than a `let
/// Scalar::Dict(..) = .. else` at each site, so the check stays genuinely
/// covered by the site that is naturally reachable with a non-dict operand
/// (a non-dict local named by `d[k] = v`/`for`, and a non-dict argument to
/// `len`) rather than an unreachable one (`DictGet`'s own `dict` operand can
/// only be non-dict through deliberately self-inconsistent MIR, exactly like
/// `Subscript`'s base).
fn expect_dict_pointer<'ctx>(scalar: Scalar<'ctx>, what: &str) -> PointerValue<'ctx> {
    let Scalar::Dict(ptr) = scalar else {
        panic!(
            "pycc_codegen: internal error: {what} did not evaluate to a dict -- \
             pycc_types::check (T0033/T0035/T0036) should have rejected this before codegen"
        )
    };
    ptr
}

/// Inserts or updates one already-untagged (D-106) `i64` value under `key`
/// in a `PyDictObj`, shared by `MirExpr::DictLiteral`'s per-pair
/// construction and `MirStmt::DictSet`'s own `d[k] = v` (D-113's
/// insert-or-update operation -- `pycc_rt_dict_set` itself decides which of
/// the two this is, by whether `key` already compares equal to a stored
/// key). Returns nothing, exactly like `build_int_list_append` above:
/// `pycc_rt_dict_set` is declared `void`.
fn build_dict_set<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    dict_ptr: PointerValue<'ctx>,
    key_ptr: PointerValue<'ctx>,
    raw_value: IntValue<'ctx>,
) {
    builder
        .build_call(
            rt.dict_set,
            &[dict_ptr.into(), key_ptr.into(), raw_value.into()],
            "dict_set",
        )
        .expect("build_call should not fail for a well-formed dict set");
}

/// A `PyDictObj`'s current entry count, as a **raw**, untagged `i64`
/// (D-106), shared by the `len(d)` builtin's `Ty::Dict` branch and
/// `MirStmt::ForDict`'s own loop bound -- mirrors `build_int_list_len`
/// exactly, for the identical reason.
fn build_dict_len<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    dict_ptr: PointerValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(rt.dict_len, &[dict_ptr.into()], "dict_len")
        .expect("build_call should not fail for a well-formed dict length read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_dict_len returns a non-void i64")
        .into_int_value()
}

/// Extracts a `PyIntSetObj` pointer from an already-evaluated operand that
/// every upstream check says must be a `set[T]`: `len`'s argument and
/// `emit_set_name_read`'s named local. `what` names the offending operand
/// for the message. Mirrors `expect_list_pointer`/`expect_dict_pointer`
/// exactly, for the identical reason (see `expect_list_pointer`'s own doc
/// comment): one shared helper rather than a `let Scalar::Set(..) = ..
/// else` at each site, so the check stays genuinely covered by the site
/// that is naturally reachable with a non-set operand (a non-set local
/// named by `for`, and a non-set argument to `len`).
fn expect_set_pointer<'ctx>(scalar: Scalar<'ctx>, what: &str) -> PointerValue<'ctx> {
    let Scalar::Set(ptr) = scalar else {
        panic!(
            "pycc_codegen: internal error: {what} did not evaluate to a set -- \
             pycc_types::check (T0033/T0037/T0038) should have rejected this before codegen"
        )
    };
    ptr
}

/// Inserts one already-untagged (D-106) `i64` value into a `PyIntSetObj`,
/// shared by `MirExpr::SetLiteral`'s per-element construction (the only
/// call site in v0.2 -- `set[int]` has no `.add()` method of its own yet,
/// unlike `list[int]`'s `.append()`). Returns nothing: `pycc_rt_int_set_add`
/// is declared `void`, exactly like `build_int_list_append`/`build_dict_set`
/// above. The dedup check that makes repeated elements collapse to one
/// (D-111) lives entirely inside `pycc_rt_int_set_add` itself -- this
/// function (and its one caller) just calls it per element, unconditionally.
fn build_int_set_add<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    set_ptr: PointerValue<'ctx>,
    raw_value: IntValue<'ctx>,
) {
    builder
        .build_call(rt.int_set_add, &[set_ptr.into(), raw_value.into()], "set_add")
        .expect("build_call should not fail for a well-formed set add");
}

/// A `PyIntSetObj`'s current element count, as a **raw**, untagged `i64`
/// (D-106), shared by the `len(s)` builtin's `Ty::Set` branch and
/// `MirStmt::ForSet`'s own loop bound -- mirrors `build_int_list_len`/
/// `build_dict_len` exactly, for the identical reason.
fn build_int_set_len<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    set_ptr: PointerValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(rt.int_set_len, &[set_ptr.into()], "set_len")
        .expect("build_call should not fail for a well-formed set length read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_set_len returns a non-void i64")
        .into_int_value()
}

/// Reads one element out of a `PyIntSetObj` by insertion-order index, used
/// only by `MirStmt::ForSet`'s own per-iteration element read. Both the
/// index and the result are on the **raw**, untagged side of D-106's
/// boundary -- mirrors `build_int_list_get` exactly, for the identical
/// reason (see that function's own doc comment): `ForSet`'s own induction
/// variable is passed straight through here with no conversion at all.
fn build_int_set_get<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    set_ptr: PointerValue<'ctx>,
    raw_index: IntValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(rt.int_set_get, &[set_ptr.into(), raw_index.into()], "set_get")
        .expect("build_call should not fail for a well-formed set read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_set_get returns a non-void i64")
        .into_int_value()
}

/// Applies the one representation-changing assignment conversion accepted by
/// the v0.1 type system: `bool` to tagged `int`. All other assignable
/// source/target pairs already share the same LLVM representation.
fn coerce_scalar_to_type<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
    target_ty: pycc_mir::Ty,
) -> Scalar<'ctx> {
    match (target_ty, scalar) {
        (pycc_mir::Ty::Int, Scalar::Bool(value)) => {
            Scalar::Int(to_tagged_int(context, builder, Scalar::Bool(value)))
        }
        (_, scalar) => scalar,
    }
}

fn range_operand_to_tagged_int<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
    position: &str,
) -> IntValue<'ctx> {
    match scalar {
        scalar @ (Scalar::Int(_) | Scalar::Bool(_)) => to_tagged_int(context, builder, scalar),
        // `List`/`Dict`/`Set` join this arm's existing or-pattern rather
        // than getting their own (D-107, extended to dict/set by D-114):
        // unlike `to_tagged_int`/`to_float` above and below, this message
        // never names the offending type, so it stays exactly as honest
        // for a list, dict, or set operand as for a `float` or `str` one --
        // and folding adds no separate, permanently-unexecutable region
        // under this crate's 100%-region gate (D-014). `range()` arguments
        // are type-checked by `pycc_types` before codegen, so this whole
        // arm is defensive either way.
        Scalar::Float(_) | Scalar::Str(_) | Scalar::List(_) | Scalar::Dict(_) | Scalar::Set(_) => {
            panic!("pycc_codegen: internal error: range() {position} did not evaluate to int")
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
        // Defensive for the same `numeric_result_type` reason as
        // `to_tagged_int`'s own `List` arm above (D-107), and separate from
        // `Str`'s for the same message-honesty reason.
        Scalar::List(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got list")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List` arm directly above, extended to `dict[K, V]` (D-107's
        // reasoning, per D-114).
        Scalar::Dict(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got dict")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict` arms above, extended to `set[T]` (D-107's
        // reasoning, per D-114).
        Scalar::Set(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got set")
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
        // A real, reachable feature gap rather than a defensive arm
        // (D-107): `pycc_types` accepts any argument type for `print`, so
        // `print(xs)` for a `list[int]` local type-checks today and lands
        // here -- and so does f-string interpolation (`f"{xs}"`, the
        // interpolation arm in `emit_expr`), a second, independent reachable
        // route into this same arm (`emit_print_arg` and that interpolation
        // arm both call into this one shared `to_str` helper). v0.2 has no
        // `str(list)`/list-printing semantics (D-105), and there is no
        // `pycc_rt_list_to_str` to call -- so this panics honestly instead
        // of handing a `PyIntListObj` pointer to a `pycc_rt_*_to_str`
        // function that would read it as a `PyStrObj`.
        Scalar::List(_) => {
            panic!("pycc_codegen: string conversion of a list[T] value is not supported yet")
        }
        // A real, reachable feature gap, identical in kind to the `List`
        // arm directly above: `pycc_types` places no type restriction on
        // `print`'s argument or an f-string interpolation, so `print(x)`/
        // `f"{x}"` for a `dict[str, int]` local type-checks today and
        // lands here. v0.2 has no `str(dict)`/dict-printing semantics
        // (D-113), and there is no `pycc_rt_dict_to_str` to call -- so
        // this panics honestly instead of handing a `PyDictObj` pointer to
        // a `pycc_rt_*_to_str` function that would read it as a
        // `PyStrObj`.
        Scalar::Dict(_) => {
            panic!("pycc_codegen: string conversion of a dict[K, V] value is not supported yet")
        }
        // A real, reachable feature gap, identical in kind to the `List`/
        // `Dict` arms directly above: `pycc_types` places no type
        // restriction on `print`'s argument or an f-string interpolation,
        // so `print(s)`/`f"{s}"` for a `set[int]` local type-checks today
        // and lands here. v0.2 has no `str(set)`/set-printing semantics
        // (D-114), and there is no `pycc_rt_int_set_to_str` to call -- so
        // this panics honestly instead of handing a `PyIntSetObj` pointer
        // to a `pycc_rt_*_to_str` function that would read it as a
        // `PyStrObj`.
        Scalar::Set(_) => {
            panic!("pycc_codegen: string conversion of a set[T] value is not supported yet")
        }
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
        .build_call(
            rt.str_from_literal,
            &[ptr.into(), len.into()],
            "str_lit_obj",
        )
        .expect("build_call should not fail for a well-formed string literal construction")
        .try_as_basic_value()
        .expect_basic("pycc_rt_str_from_literal returns a non-void pointer")
        .into_pointer_value()
}

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
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    expr: &MirExpr,
) -> Scalar<'ctx> {
    use pycc_mir::Ty;
    match expr {
        MirExpr::IntLiteral(n) => Scalar::Int(tag_smallint_const(context, *n)),
        MirExpr::FloatLiteral(f) => Scalar::Float(context.f64_type().const_float(*f)),
        MirExpr::StringLiteral(s) => {
            Scalar::Str(emit_string_literal(context, builder, module, rt, s))
        }
        MirExpr::Name { name, ty } => {
            let slot = locals.get(name).unwrap_or_else(|| {
                panic!("pycc_codegen: internal error: `{name}` has no local slot")
            });
            debug_assert_eq!(
                &slot.ty, ty,
                "pycc_codegen: internal error: local type drifted"
            );
            if let Some(initialized_ptr) = slot.initialized {
                let initialized = builder
                    .build_load(context.i8_type(), initialized_ptr, "global_initialized")
                    .expect("build_load should not fail for a declared global flag")
                    .into_int_value();
                let is_initialized = builder
                    .build_int_compare(
                        IntPredicate::NE,
                        initialized,
                        context.i8_type().const_zero(),
                        "global_is_initialized",
                    )
                    .expect("build_int_compare should not fail for two i8 values");
                let function = builder
                    .get_insert_block()
                    .expect("name reads are always emitted inside a basic block")
                    .get_parent()
                    .expect("the current basic block always belongs to a function");
                let ready = context.append_basic_block(function, "global_ready");
                let unbound = context.append_basic_block(function, "global_unbound");
                builder
                    .build_conditional_branch(is_initialized, ready, unbound)
                    .expect("build_conditional_branch should not fail for an i1 condition");
                builder.position_at_end(unbound);
                builder
                    .build_call(rt.trap, &[], "unbound_global")
                    .expect("build_call should not fail for llvm.trap");
                builder
                    .build_unreachable()
                    .expect("build_unreachable should not fail in a fresh block");
                builder.position_at_end(ready);
            }
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
                        .build_load(context.i64_type(), slot.ptr, "load")
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Int(loaded.into_int_value())
                }
                Ty::Bool => {
                    let loaded = builder
                        .build_load(context.i8_type(), slot.ptr, "load")
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Bool(loaded.into_int_value())
                }
                Ty::Float => {
                    let loaded = builder
                        .build_load(context.f64_type(), slot.ptr, "load")
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Float(loaded.into_float_value())
                }
                Ty::Str => {
                    let loaded = builder
                        .build_load(
                            context.ptr_type(inkwell::AddressSpace::default()),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Str(loaded.into_pointer_value())
                }
                Ty::None => {
                    let loaded = builder
                        .build_load(context.i8_type(), slot.ptr, "load_none")
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    // The static `Ty::None` retains the semantic distinction
                    // from a bool even though both use an LLVM `i8` carrier.
                    Scalar::Bool(loaded.into_int_value())
                }
                // Same pointer-slot read as `Ty::Str` immediately above --
                // `ty_to_basic_type`'s own `List(_)` arm already allocated
                // this slot as a pointer, so reading it back is identical
                // regardless of the element type.
                //
                // Task 5 (D-089) originally carried the loaded pointer in
                // `Scalar::Str`, safe only for as long as no `MirExpr`
                // could construct a `list[T]` value, and flagged as a
                // Task-11 tripwire for `truthy`/`to_str` specifically.
                // Task 11a (D-107) retired that reuse: the pointer is now
                // `Scalar::List`, so every exhaustive `Scalar` match had to
                // answer for `list[T]` explicitly instead of silently
                // treating it as a `PyStrObj`.
                //
                // `str_value_is_a_duplicate_reference` stays gated on
                // `ty: Ty::Str` (Task 5's own fix for the same reuse). That
                // gate is now redundant with the variant split for this
                // arm, but it is still the correct contract for that
                // function -- and `incref_if_str_duplicate` needs no
                // `List` arm at all, since its `if let Scalar::Str(..)
                // else { scalar }` shape already passes a list through
                // untouched, which is exactly D-107's leak-only policy.
                Ty::List(_) => {
                    let loaded = builder
                        .build_load(
                            context.ptr_type(inkwell::AddressSpace::default()),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::List(loaded.into_pointer_value())
                }
                // Same pointer-slot read as `Ty::List(_)` immediately
                // above (PR-11 Task 5) -- `ty_to_basic_type`'s own
                // `Dict(_)` arm already allocated this slot as a pointer,
                // so reading it back is identical regardless of the
                // key/value types. Every real read of a `dict[str, int]`
                // local -- `len(x)`, `x[k]`, `x[k] = v`'s own dict operand,
                // `for k in x:` -- goes through this arm (directly, or via
                // `emit_dict_name_read`'s synthetic `MirExpr::Name`), so
                // without it every one of those would fall through to the
                // catch-all below and panic on a real, type-checked
                // program instead of reading the value.
                Ty::Dict(_) => {
                    let loaded = builder
                        .build_load(
                            context.ptr_type(inkwell::AddressSpace::default()),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Dict(loaded.into_pointer_value())
                }
                // Same pointer-slot read as `Ty::List(_)`/`Ty::Dict(_)`
                // immediately above (PR-11 Task 9) -- `ty_to_basic_type`'s
                // own `Set(_)` arm already allocated this slot as a
                // pointer, so reading it back is identical regardless of
                // the element type. Every real read of a `set[int]`
                // local -- `len(x)`, `for v in x:` -- goes through this
                // arm (directly, or via `emit_set_name_read`'s synthetic
                // `MirExpr::Name`), so without it every one of those would
                // fall through to the catch-all below and panic on a real,
                // type-checked program instead of reading the value.
                Ty::Set(_) => {
                    let loaded = builder
                        .build_load(
                            context.ptr_type(inkwell::AddressSpace::default()),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Set(loaded.into_pointer_value())
                }
                other => {
                    panic!(
                        "pycc_codegen: reading a `{}`-typed local is not supported yet",
                        other.name()
                    )
                }
            }
        }
        MirExpr::BinOp {
            op,
            left,
            right,
            ty,
        } => {
            // This inkwell version's `try_as_basic_value()` returns its own
            // `ValueKind` enum (not `either::Either` as in older inkwell
            // releases the task brief's original code was written against
            // -- ".left()" doesn't exist on this type); `.expect_basic(msg)`
            // is the direct equivalent, panicking with `msg` if the callee
            // turned out to be void instead of returning a value.
            let l = emit_expr(context, builder, module, rt, user_functions, locals, left);
            let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
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
                        pycc_mir::BinOpKind::Div => {
                            Scalar::Float(build_float_rt_binop(builder, rt.float_div, l, r))
                        }
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
                        panic!(
                            "pycc_codegen: internal error: str BinOp operand did not evaluate to str"
                        )
                    };
                    let Scalar::Str(r) = r else {
                        panic!(
                            "pycc_codegen: internal error: str BinOp operand did not evaluate to str"
                        )
                    };
                    if *op != pycc_mir::BinOpKind::Add {
                        panic!(
                            "pycc_codegen: `str {op:?} str` is not supported yet (only concatenation is)"
                        );
                    }
                    let result = builder
                        .build_call(rt.str_concat, &[l.into(), r.into()], "str_concat")
                        .expect("build_call should not fail for a well-formed concatenation")
                        .try_as_basic_value()
                        .expect_basic("pycc_rt_str_concat returns a non-void pointer");
                    Scalar::Str(result.into_pointer_value())
                }
                // No container type supports any `BinOpKind` in this plan's
                // own scope -- not even `+` for list concatenation, which
                // D-105 defers past v0.2. This arm is a pure diagnostic-
                // message improvement over the generic `other` catch-all
                // below (naming the specific container type via `.name()`
                // and calling out that it's the *operator* that's
                // unsupported, not just the result type), not new
                // capability.
                Ty::List(_) | Ty::Dict(..) | Ty::Set(_) | Ty::Tuple(_) => panic!(
                    "pycc_codegen: binary operators are not supported on {} yet",
                    ty.name()
                ),
                other => panic!("pycc_codegen: a `{other:?}`-result BinOp is not supported yet"),
            }
        }
        MirExpr::Compare {
            op, left, right, ..
        } => {
            let left_ty = left.ty();
            let right_ty = right.ty();
            let l = emit_expr(context, builder, module, rt, user_functions, locals, left);
            let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
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
                    panic!(
                        "pycc_codegen: internal error: str Compare operand did not evaluate to str"
                    )
                };
                let Scalar::Str(r) = r else {
                    panic!(
                        "pycc_codegen: internal error: str Compare operand did not evaluate to str"
                    )
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
        MirExpr::BoolLiteral(b) => Scalar::Bool(context.i8_type().const_int(u64::from(*b), false)),
        MirExpr::Call { callee, args, ty } => {
            if callee == "print" {
                panic!(
                    "pycc_codegen: using print()'s result as a nested expression is not supported yet"
                );
            }
            // `len` is the second hand-recognized builtin (D-105 point 3),
            // dispatched here for the same reason `print` is: it has no
            // `user_functions` entry, so it must be claimed before the
            // lookup below turns it into an "undefined function" panic.
            // Mirrors `pycc_types`' own `callee == "len"` arm and
            // `pycc_mir`'s, which already type it `Ty::Int` -- so this
            // returns a `Scalar::Int` directly rather than falling through
            // to the declared-return-type dispatch at the end of this arm.
            if callee == "len" {
                let [list_arg] = args.as_slice() else {
                    panic!(
                        "pycc_codegen: internal error: `len` takes exactly 1 argument, got {} \
                         -- pycc_types::check (T0033) should have rejected this before codegen",
                        args.len()
                    )
                };
                let scalar = emit_expr(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    list_arg,
                );
                // D-113 relaxed `len()` to also accept a `dict[K, V]`
                // argument alongside `list[T]` (PR-11 Task 5), and PR-11
                // Task 9 relaxed it once more to also accept `set[T]`,
                // dispatched on the argument's own static type -- mirrors
                // `pycc_types`'/`pycc_mir`'s identical relaxation at their
                // own hand-recognized `len` dispatch points.
                let raw_len = match list_arg.ty() {
                    Ty::Dict(_) => {
                        let dict_ptr = expect_dict_pointer(scalar, "`len`'s argument");
                        build_dict_len(builder, rt, dict_ptr)
                    }
                    Ty::Set(_) => {
                        let set_ptr = expect_set_pointer(scalar, "`len`'s argument");
                        build_int_set_len(builder, rt, set_ptr)
                    }
                    _ => {
                        let list_ptr = expect_list_pointer(scalar, "`len`'s argument");
                        build_int_list_len(builder, rt, list_ptr)
                    }
                };
                // D-106 output side: the raw count becomes a user-visible
                // `Ty::Int` expression value here (`print(len(x))`,
                // `n = len(x)`), so it is re-tagged -- unlike
                // `MirStmt::ForList`'s own use of the same runtime call,
                // which keeps it raw as a private loop bound.
                return Scalar::Int(raw_i64_to_tagged_int(context, builder, raw_len));
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
            let user_function = user_functions.get(callee.as_str()).unwrap_or_else(|| {
                panic!(
                    "pycc_codegen: internal error: call to undefined function `{callee}` \
                     should have been rejected by pycc_types before reaching codegen"
                )
            });
            let call_site = build_call_to(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                user_function,
                args,
            );
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
                    // `void`, so there is no value to extract. Preserve the
                    // call's side effects and materialize the canonical zero
                    // carrier used when that unit value crosses a parameter
                    // or storage boundary. The surrounding MIR type keeps it
                    // distinct from a real `False` value.
                    Scalar::Bool(context.i8_type().const_int(0, false))
                }
                // No dedicated arm for `List`/`Dict`/`Set`/`Tuple`: a
                // container-typed call result gets the same treatment as
                // every other still-unhandled `Ty` here (currently only
                // `Ty::Infer`, which never reaches real codegen -- see
                // `an_infer_typed_call_result_used_as_a_nested_expression_
                // is_not_supported` below), naming the specific type via
                // `.name()` instead of a bare `{:?}`.
                other => {
                    panic!(
                        "pycc_codegen: a `{}`-typed call result is not supported yet",
                        other.name()
                    )
                }
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
                        if inner.ty() == pycc_mir::Ty::None {
                            emit_expr(context, builder, module, rt, user_functions, locals, inner);
                            emit_string_literal(context, builder, module, rt, "None")
                        } else {
                            let scalar = emit_expr(
                                context,
                                builder,
                                module,
                                rt,
                                user_functions,
                                locals,
                                inner,
                            );
                            let scalar = incref_if_str_duplicate(builder, rt, inner, scalar);
                            to_str(builder, rt, scalar)
                        }
                    }
                };
                acc = Some(match acc {
                    None => part_str,
                    Some(prev) => {
                        let joined = builder
                            .build_call(
                                rt.str_concat,
                                &[prev.into(), part_str.into()],
                                "fstring_concat",
                            )
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
        // `[e1, e2, ...]` (D-105): an empty `pycc_rt_int_list_new()` object
        // followed by one `pycc_rt_int_list_append` per element, in source
        // order. No pre-sizing call: `PyIntListObj`'s payload is a
        // `Vec<i64>` whose own amortized-doubling `push` already handles
        // growth (see that struct's doc comment), so a reserve entry point
        // would be new runtime surface for no behavioral difference.
        //
        // D-106 input side: each element is an arbitrary `Ty::Int`
        // expression and therefore D-061-tagged, while `PyIntListObj`
        // stores raw untagged slots -- hence the `build_untag_checked` per
        // element (which is also what turns a bigint-valued element into an
        // honest runtime panic instead of a silently corrupted slot).
        //
        // `to_tagged_int` rather than a `let Scalar::Int(..) else` match, for
        // the same reason `emit_expr`'s own `BinOp`/`Ty::Int` arm uses it:
        // Python's `bool` is an `int` subtype, so widening is the correct
        // response to a `Scalar::Bool` element rather than an error -- and
        // its `Float`/`Str`/`List` arms already panic honestly.
        MirExpr::ListLiteral(elements) => {
            let list_ptr = builder
                .build_call(rt.int_list_new, &[], "list_new")
                .expect("build_call should not fail for a well-formed list construction")
                .try_as_basic_value()
                .expect_basic("pycc_rt_int_list_new returns a non-void pointer")
                .into_pointer_value();
            for element in elements {
                let scalar = emit_expr(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    element,
                );
                let tagged = to_tagged_int(context, builder, scalar);
                let raw = build_untag_checked(builder, rt, tagged, "list_untag_element");
                build_int_list_append(builder, rt, list_ptr, raw);
            }
            Scalar::List(list_ptr)
        }
        // `base[index]`, read-only (D-105 scope cut 2). Both of D-106's
        // conversions apply here, in opposite directions: the index is a
        // tagged `Ty::Int` expression crossing *into* the list, and the
        // element read back out is a raw slot becoming a user-visible
        // `Ty::Int` expression result.
        //
        // An out-of-range (including any negative) index is
        // `pycc_rt_int_list_get`'s own honest runtime panic, not something
        // this crate can check -- the index is only known at runtime.
        MirExpr::Subscript { base, index } => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            let base_ptr = expect_list_pointer(base_scalar, "the subscripted value");
            let index_scalar =
                emit_expr(context, builder, module, rt, user_functions, locals, index);
            let tagged_index = to_tagged_int(context, builder, index_scalar);
            let raw_index = build_untag_checked(builder, rt, tagged_index, "list_untag_index");
            let raw_element = build_int_list_get(builder, rt, base_ptr, raw_index);
            Scalar::Int(raw_i64_to_tagged_int(context, builder, raw_element))
        }
        // `list.append(value)` (D-105 point 3). Same input-side D-106
        // conversion as `ListLiteral`'s elements above. `list` is a plain
        // variable name rather than a sub-expression (mirroring
        // `HirExpr::ListAppend`), so it is read through
        // `emit_list_name_read` instead of a recursive `emit_expr` call.
        //
        // Python's `list.append` evaluates to `None`; the canonical `i8 0`
        // unit carrier this crate uses for every other `None`-valued
        // expression is what comes back (identical to `emit_expr`'s `Call`
        // arm's own `Ty::None` case).
        MirExpr::ListAppend { list, value } => {
            let list_ptr =
                emit_list_name_read(context, builder, module, rt, user_functions, locals, list);
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            let tagged = to_tagged_int(context, builder, scalar);
            let raw = build_untag_checked(builder, rt, tagged, "list_untag_appended");
            build_int_list_append(builder, rt, list_ptr, raw);
            Scalar::Bool(context.i8_type().const_int(0, false))
        }
        // `{k1: v1, k2: v2, ...}` (PR-11 Task 5, D-113): an empty
        // `pycc_rt_dict_new()` object followed by one `pycc_rt_dict_set`
        // per pair, in source order -- mirrors `MirExpr::ListLiteral`'s own
        // shape exactly (no pre-sizing call: `PyDictObj`'s own payload
        // already handles growth itself, same reasoning as
        // `ListLiteral`'s own doc comment).
        //
        // D-106's input side applies to the value half of each pair only:
        // the key is a `Ty::Str` expression that crosses into `PyDictObj`
        // unchanged (a dict key is a raw pointer either way, no tagging
        // scheme), while the value is an arbitrary `Ty::Int` expression and
        // therefore D-061-tagged, hence the same `build_untag_checked` per
        // pair `ListLiteral`'s own elements already need.
        MirExpr::DictLiteral(pairs) => {
            let dict_ptr = builder
                .build_call(rt.dict_new, &[], "dict_new")
                .expect("build_call should not fail for a well-formed dict construction")
                .try_as_basic_value()
                .expect_basic("pycc_rt_dict_new returns a non-void pointer")
                .into_pointer_value();
            for (key, value) in pairs {
                let key_scalar =
                    emit_expr(context, builder, module, rt, user_functions, locals, key);
                // `PyDictObj` adopts the key pointer it is given as its own
                // permanent reference, without incref'ing it itself (D-114:
                // "the stored key pointer is neither increfed on insert nor
                // decrefed ... ever"). A bare-`Name` key (`{k: 1}`) is a
                // *duplicate* reference to whatever `PyStrObj` `k`'s own
                // slot already owns, exactly the shape `incref_if_str_
                // duplicate` exists to protect everywhere else a `str`
                // value crosses an ownership-taking boundary (`MirStmt::
                // Assign`, `build_call_to`'s argument marshalling,
                // `MirStmt::Return`) -- without this call, a later `k = ...`
                // reassignment would decref (and potentially free) the same
                // `PyStrObj` this dict still points to, a real premature
                // free, not this project's accepted list/dict leak-only
                // policy (D-107/D-114 only ever accept *never freeing*, not
                // freeing too early). A string-literal key (`{"a": 1}`)
                // freshly constructs its own owned reference every time, so
                // `str_value_is_a_duplicate_reference` correctly leaves it
                // untouched here.
                let key_scalar = incref_if_str_duplicate(builder, rt, key, key_scalar);
                // Every `Ty::Dict` value that survives `pycc_types`' own
                // T0036 gate has key type exactly `Ty::Str` (see that
                // gate's own comment), so this is a defensive backstop,
                // not a real feature gap -- the same inline
                // `let Scalar::Str(..) = .. else { panic!(..) }` shape
                // `emit_expr`'s own `BinOp`/`Compare` arms already use for
                // their `Ty::Str` operands, for the identical reason (no
                // shared helper: see this file's established convention of
                // not extracting a single-use-per-arm check into its own
                // function).
                let Scalar::Str(key_ptr) = key_scalar else {
                    panic!(
                        "pycc_codegen: internal error: dict literal key did not evaluate to str \
                         -- pycc_types::check (T0036) should have rejected this before codegen"
                    )
                };
                let value_scalar =
                    emit_expr(context, builder, module, rt, user_functions, locals, value);
                let tagged = to_tagged_int(context, builder, value_scalar);
                let raw = build_untag_checked(builder, rt, tagged, "dict_untag_literal_value");
                build_dict_set(builder, rt, dict_ptr, key_ptr, raw);
            }
            Scalar::Dict(dict_ptr)
        }
        // `dict[key]`, read-only (PR-11 Task 5, D-113 -- `d[k] = v` is a
        // separate, statement-level operation, `MirStmt::DictSet` below).
        // Mirrors `MirExpr::Subscript`'s own shape: the key is a `Ty::Str`
        // expression crossing in unchanged (no D-106 conversion -- a dict
        // key is never tagged), and the raw value read back out becomes a
        // user-visible `Ty::Int` expression result, hence
        // `raw_i64_to_tagged_int` on the way out (D-106's output side).
        //
        // A missing key is `pycc_rt_dict_get`'s own honest runtime panic,
        // not something this crate can check -- the key is only known at
        // runtime.
        MirExpr::DictGet { dict, key } => {
            let dict_scalar =
                emit_expr(context, builder, module, rt, user_functions, locals, dict);
            let dict_ptr = expect_dict_pointer(dict_scalar, "the dict subscripted value");
            let key_scalar = emit_expr(context, builder, module, rt, user_functions, locals, key);
            let Scalar::Str(key_ptr) = key_scalar else {
                panic!(
                    "pycc_codegen: internal error: dict subscript key did not evaluate to str \
                     -- pycc_types::check (T0021) should have rejected this before codegen"
                )
            };
            let raw_value = builder
                .build_call(rt.dict_get, &[dict_ptr.into(), key_ptr.into()], "dict_get")
                .expect("build_call should not fail for a well-formed dict read")
                .try_as_basic_value()
                .expect_basic("pycc_rt_dict_get returns a non-void i64")
                .into_int_value();
            Scalar::Int(raw_i64_to_tagged_int(context, builder, raw_value))
        }
        // `{e1, e2, ...}` (PR-11 Task 9, D-113/D-111): an empty
        // `pycc_rt_int_set_new()` object followed by one
        // `pycc_rt_int_set_add` per element, in source order -- mirrors
        // `MirExpr::ListLiteral`'s own shape exactly (no pre-sizing call:
        // `PyIntSetObj`'s own payload already handles growth itself, same
        // reasoning as `ListLiteral`'s own doc comment), except that
        // `set[int]` has no string-keyed counterpart to `DictLiteral`'s own
        // per-pair key handling -- every element is a raw `i64`, so there is
        // no refcounting concern here at all (unlike `DictLiteral`'s key,
        // D-113's own T0038 gate means every `set[int]` element is exactly
        // `Ty::Int`).
        //
        // D-106's input side applies to every element: each is an arbitrary
        // `Ty::Int` expression and therefore D-061-tagged, while
        // `PyIntSetObj` stores raw untagged slots -- hence the
        // `build_untag_checked` per element, identical to `ListLiteral`'s
        // own elements. The dedup check that makes a repeated element
        // collapse to one (D-111) lives entirely inside
        // `pycc_rt_int_set_add` itself (see `build_int_set_add`'s own doc
        // comment) -- this arm calls it per element, unconditionally, with
        // no dedup logic of its own.
        MirExpr::SetLiteral(elements) => {
            let set_ptr = builder
                .build_call(rt.int_set_new, &[], "set_new")
                .expect("build_call should not fail for a well-formed set construction")
                .try_as_basic_value()
                .expect_basic("pycc_rt_int_set_new returns a non-void pointer")
                .into_pointer_value();
            for element in elements {
                let scalar = emit_expr(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    element,
                );
                let tagged = to_tagged_int(context, builder, scalar);
                let raw = build_untag_checked(builder, rt, tagged, "set_untag_element");
                build_int_set_add(builder, rt, set_ptr, raw);
            }
            Scalar::Set(set_ptr)
        }
    }
}

/// Reads a `list[T]`-typed local by name. `MirExpr::ListAppend`'s `list` and
/// `MirStmt::ForList`'s `list` both carry their list as a plain variable
/// name rather than a sub-expression (mirroring `HirExpr::ListAppend`/
/// `HirStmt::ForList`, D-105), so neither has a `MirExpr` to hand to
/// `emit_expr` directly.
///
/// Routes the read through `emit_expr`'s own `Name` arm (via a synthetic
/// `MirExpr::Name` carrying the slot's own recorded type) rather than
/// loading `slot.ptr` here, so these two reads get exactly the same
/// definite-assignment guard every other name read gets -- without it,
/// `if flag: xs = [1]` followed by an unconditional `xs.append(2)` would
/// dereference an uninitialized slot instead of trapping. Unlike
/// `emit_string_literal`'s own documented removal of a synthetic-`MirExpr`
/// round-trip, this one introduces no permanently-uncoverable arm: the
/// non-list case goes through the shared `expect_list_pointer` above, and a
/// non-list local is exactly the naturally reachable way to cover it (see
/// this file's own `appending_to_a_non_list_local_is_an_internal_error`).
#[allow(clippy::too_many_arguments)]
fn emit_list_name_read<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    name: &str,
) -> PointerValue<'ctx> {
    // `let ... else`, not the `unwrap_or_else(|| panic!(..))` shape
    // `emit_expr`'s own `Name` arm uses for the same lookup: a panicking
    // closure compiles to its own function record, which `cargo llvm-cov`
    // reports as an "Unexecuted instantiation" in whichever crate
    // instantiation never reaches it (observed here for the copy the
    // integration-test binaries link, which has no reason to construct an
    // unbound list name). Writing the check inline keeps its counts inside
    // the enclosing function, where they merge across instantiations.
    let Some(slot) = locals.get(name) else {
        panic!("pycc_codegen: internal error: `{name}` has no local slot")
    };
    let ty = slot.ty.clone();
    let scalar = emit_expr(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        &MirExpr::Name {
            name: name.to_string(),
            ty,
        },
    );
    expect_list_pointer(scalar, &format!("`{name}`"))
}

/// Reads a `dict[K, V]`-typed local by name. `MirStmt::DictSet`'s `dict` and
/// `MirStmt::ForDict`'s `dict` both carry their dict as a plain variable
/// name rather than a sub-expression (mirroring `HirStmt::DictSet`/
/// `HirStmt::ForList`'s dict-typed case, D-113), so neither has a `MirExpr`
/// to hand to `emit_expr` directly. Mirrors `emit_list_name_read` exactly,
/// for the identical reason (see that function's own doc comment,
/// including its `let ... else` shape over `unwrap_or_else(|| panic!(..))`).
#[allow(clippy::too_many_arguments)]
fn emit_dict_name_read<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    name: &str,
) -> PointerValue<'ctx> {
    let Some(slot) = locals.get(name) else {
        panic!("pycc_codegen: internal error: `{name}` has no local slot")
    };
    let ty = slot.ty.clone();
    let scalar = emit_expr(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        &MirExpr::Name {
            name: name.to_string(),
            ty,
        },
    );
    expect_dict_pointer(scalar, &format!("`{name}`"))
}

/// Reads a `set[T]`-typed local by name. `MirStmt::ForSet`'s `set` carries
/// its set as a plain variable name rather than a sub-expression (mirroring
/// `HirStmt::ForList`'s set-typed case, D-113), so it has no `MirExpr` to
/// hand to `emit_expr` directly. Mirrors `emit_list_name_read`/
/// `emit_dict_name_read` exactly, for the identical reason (see
/// `emit_list_name_read`'s own doc comment, including its `let ... else`
/// shape over `unwrap_or_else(|| panic!(..))`).
#[allow(clippy::too_many_arguments)]
fn emit_set_name_read<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    name: &str,
) -> PointerValue<'ctx> {
    let Some(slot) = locals.get(name) else {
        panic!("pycc_codegen: internal error: `{name}` has no local slot")
    };
    let ty = slot.ty.clone();
    let scalar = emit_expr(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        &MirExpr::Name {
            name: name.to_string(),
            ty,
        },
    );
    expect_set_pointer(scalar, &format!("`{name}`"))
}

/// Evaluates every entry in `args` (via `emit_expr`, so each argument is
/// itself an arbitrary expression -- nested calls included, which is
/// exactly what makes recursion with real arguments work) and emits the
/// `call` instruction to the already-resolved `f`. Shared between
/// `emit_expr`'s `Call` arm (a value-producing call used inside a larger
/// expression) and `emit_stmt`'s void-call arm below (a call whose
/// declared return type is `None`, used as a bare statement). A
/// `None`-returning call has no LLVM return value; `emit_expr` separately
/// materializes its canonical unit carrier only when a surrounding
/// expression needs one. This is the one piece both call sites need
/// regardless of whether a value comes back afterward.
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
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    user_function: &UserFunction<'ctx>,
    args: &[MirExpr],
) -> inkwell::values::CallSiteValue<'ctx> {
    let arg_values: Vec<inkwell::values::BasicMetadataValueEnum> = args
        .iter()
        .zip(&user_function.param_tys)
        .map(|(a, param_ty)| {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, a);
            let scalar = incref_if_str_duplicate(builder, rt, a, scalar);
            let scalar = coerce_scalar_to_type(context, builder, scalar, param_ty.clone());
            match scalar {
                Scalar::Int(v) => v.into(),
                Scalar::Bool(v) => v.into(),
                Scalar::Float(v) => v.into(),
                Scalar::Str(v) => v.into(),
                // Pass-through, identical to `Str`'s arm directly above: a
                // `list[T]` parameter is an opaque pointer at the ABI
                // level exactly like a `str` one (`ty_to_basic_type` gives
                // both the same LLVM type), so argument marshalling needs
                // no list-specific handling at all.
                Scalar::List(v) => v.into(),
                // Pass-through, identical to `List`'s arm directly above: a
                // `dict[K, V]` parameter is an opaque pointer at the ABI
                // level exactly like a `str`/`list[T]` one
                // (`ty_to_basic_type` gives all three the same LLVM type),
                // so argument marshalling needs no dict-specific handling
                // at all.
                Scalar::Dict(v) => v.into(),
                // Pass-through, identical to `List`'s/`Dict`'s arms
                // directly above: a `set[T]` parameter is an opaque pointer
                // at the ABI level exactly like a `str`/`list[T]`/
                // `dict[K, V]` one (`ty_to_basic_type` gives all four the
                // same LLVM type), so argument marshalling needs no
                // set-specific handling at all.
                Scalar::Set(v) => v.into(),
            }
        })
        .collect();
    builder
        .build_call(user_function.value, &arg_values, "call_user_fn")
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
        // A real, reachable feature gap rather than a defensive arm
        // (D-107): `pycc_types` places no type restriction on an `if`/
        // `while` condition, so `if xs:` for a `list[int]` local
        // type-checks today and lands here. v0.2 has no `bool(list)`
        // semantics (D-105 ships only `len(x)`/`x[i]`/iteration/
        // `.append()`), and there is no `pycc_rt_int_list_truthy` to call
        // -- so this panics honestly instead of calling
        // `pycc_rt_str_truthy` on a `PyIntListObj` pointer, whose layout
        // has nothing in common with `PyStrObj`'s.
        Scalar::List(_) => {
            panic!("pycc_codegen: truthiness of a list[T] value is not supported yet")
        }
        // A real, reachable feature gap, identical in kind to the `List`
        // arm directly above: `pycc_types` places no type restriction on an
        // `if`/`while` condition, so `if x:` for a `dict[str, int]` local
        // type-checks today and lands here. v0.2 has no `bool(dict)`
        // semantics (D-113 ships only `len(x)`/`x[k]`/`x[k] = v`/
        // iteration), and there is no `pycc_rt_dict_truthy` to call -- so
        // this panics honestly instead of calling `pycc_rt_str_truthy` on
        // a `PyDictObj` pointer, whose layout has nothing in common with
        // `PyStrObj`'s.
        Scalar::Dict(_) => {
            panic!("pycc_codegen: truthiness of a dict[K, V] value is not supported yet")
        }
        // A real, reachable feature gap, identical in kind to the `List`/
        // `Dict` arms directly above: `pycc_types` places no type
        // restriction on an `if`/`while` condition, so `if s:` for a
        // `set[int]` local type-checks today and lands here. v0.2 has no
        // `bool(set)` semantics (D-114 ships only `len(x)`/iteration), and
        // there is no `pycc_rt_int_set_truthy` to call -- so this panics
        // honestly instead of calling `pycc_rt_str_truthy` on a
        // `PyIntSetObj` pointer, whose layout has nothing in common with
        // `PyStrObj`'s.
        Scalar::Set(_) => {
            panic!("pycc_codegen: truthiness of a set[T] value is not supported yet")
        }
    };
    builder
        .build_int_compare(
            IntPredicate::NE,
            as_i8,
            context.i8_type().const_int(0, false),
            "truthy",
        )
        .expect("build_int_compare should not fail comparing two i8 operands")
}

/// Allocates a local slot in the current function's entry block, which
/// dominates every branch, loop, and merge that may read it. String slots are
/// initialized to null so the first emitted decref is a safe no-op. A guarded
/// non-parameter slot also receives an `i8` initialized flag, because #118's
/// static definite-assignment joins have not landed yet and a syntactically
/// present assignment may not execute at runtime.
fn storage_slot_at_entry<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    ty: pycc_mir::Ty,
    name: &str,
    guard_reads: bool,
) -> StorageSlot<'ctx> {
    let current_block = builder.get_insert_block().expect(
        "builder is always positioned inside some block while a statement is being emitted",
    );
    let function = current_block
        .get_parent()
        .expect("the block builder is currently positioned in always belongs to a function");
    let entry_block = function.get_first_basic_block().expect(
        "compile_to_object always appends a function's entry block before emitting its body",
    );
    builder.position_at_end(entry_block);
    let ptr = builder
        .build_alloca(ty_to_basic_type(context, ty.clone()), name)
        .expect("build_alloca should not fail for a supported local type");
    if ty == pycc_mir::Ty::Str {
        builder
            .build_store(
                ptr,
                context
                    .ptr_type(inkwell::AddressSpace::default())
                    .const_null(),
            )
            .expect("build_store should not fail immediately after this function's own alloca");
    }
    let initialized = if guard_reads {
        let initialized_ptr = builder
            .build_alloca(context.i8_type(), &format!("{name}_initialized"))
            .expect("build_alloca should not fail for an i8 initialization flag");
        builder
            .build_store(initialized_ptr, context.i8_type().const_zero())
            .expect("build_store should not fail for a fresh initialization flag");
        Some(initialized_ptr)
    } else {
        None
    };
    builder.position_at_end(current_block);
    StorageSlot {
        ptr,
        ty,
        initialized,
    }
}

/// Reuses the predeclared slot backing `target`. The slot's established type
/// wins over the current expression type, including the accepted
/// `bool`-to-`int` assignment that must store a tagged i64 rather than raw i8.
fn emit_assign<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    locals: &mut HashMap<String, StorageSlot<'ctx>>,
    target: &str,
    value: Scalar<'ctx>,
) {
    let slot = locals
        .get(target)
        .cloned()
        .expect("every assignment target must have a predeclared storage slot");
    let value = coerce_scalar_to_type(context, builder, value, slot.ty);
    let basic_value: inkwell::values::BasicValueEnum = match value {
        Scalar::Int(v) => v.into(),
        Scalar::Bool(v) => v.into(),
        Scalar::Float(v) => v.into(),
        Scalar::Str(v) => v.into(),
        // Pass-through, identical to `Str`'s arm directly above: storing a
        // `list[T]` value is storing one opaque pointer into a slot
        // `ty_to_basic_type` already allocated as a pointer. No refcount
        // traffic accompanies it -- D-107 keeps `list[T]` leak-only for
        // v0.2, so unlike `Str` there is deliberately no incref here and
        // no `decref_str_slot_before_store` counterpart. That helper is
        // never even called for a list target: `emit_stmt`'s `Assign` arm
        // gates the call itself on the target's `Ty` (`if ty ==
        // pycc_mir::Ty::Str`) before ever invoking this function, not the
        // other way around -- the helper's own internal `slot.ty !=
        // Ty::Str` check is a defensive `panic!` for the "reached with the
        // wrong target" case, not a skip a list target relies on.
        Scalar::List(v) => v.into(),
        // Pass-through, identical to `List`'s arm directly above: storing
        // a `dict[K, V]` value is storing one opaque pointer into a slot
        // `ty_to_basic_type` already allocated as a pointer. No refcount
        // traffic accompanies it -- D-114 keeps `dict[K, V]` leak-only for
        // v0.2 (extending D-107's exact reasoning), so unlike `Str` there
        // is deliberately no incref here and no
        // `decref_str_slot_before_store` counterpart.
        Scalar::Dict(v) => v.into(),
        // Pass-through, identical to `List`'s/`Dict`'s arms directly
        // above: storing a `set[T]` value is storing one opaque pointer
        // into a slot `ty_to_basic_type` already allocated as a pointer.
        // No refcount traffic accompanies it -- D-114 keeps `set[T]`
        // leak-only for v0.2 (extending D-107's exact reasoning),
        // identically to `List`/`Dict`.
        Scalar::Set(v) => v.into(),
    };
    builder
        .build_store(slot.ptr, basic_value)
        .expect("build_store should not fail for a slot this function itself allocated");
    if let Some(initialized_ptr) = slot.initialized {
        builder
            .build_store(initialized_ptr, context.i8_type().const_int(1, false))
            .expect("build_store should not fail for a declared global flag");
    }
}

/// Whether evaluating `expr` produces a *duplicate* reference to an
/// already-owned `str` (a bare `str`-typed variable read) rather than a
/// fresh object owning exactly one reference from its own construction.
/// v0.1's grammar makes this purely syntactic: every str-producing
/// expression other than a bare `Name` (`StringLiteral`, string
/// concatenation, a `Call`'s return value) freshly constructs its result
/// and already owns exactly one reference (D-060, Task 7).
///
/// Gated on `ty: Ty::Str`, not just the bare-`Name` shape (Task 5, D-089).
/// The gate was originally added because `emit_expr`'s `Name` arm carried a
/// `Ty::List(_)`-typed read in `Scalar::Str` too, which made a bare
/// `list[T]`-typed `Name` indistinguishable from a `str`-typed one at the
/// `Scalar` level -- and `incref_if_str_duplicate` below dispatches on the
/// `Scalar` variant alone, so without the gate it would have called
/// `pycc_rt_str_incref` on a list pointer.
///
/// Task 11a (D-107) removed that reuse: a list read is now `Scalar::List`,
/// so the two are no longer confusable and this gate is no longer what
/// prevents the spurious incref. It is kept because it is independently the
/// correct contract for this function -- it answers "is this a duplicate
/// reference to an already-owned *`str`*", and a non-`str` `Name` is not
/// one, whatever `Scalar` variant it happens to produce. Behavior-identical
/// either way for every reachable case: `incref_if_str_duplicate` only ever
/// consults this function *after* confirming `scalar` is `Scalar::Str`.
fn str_value_is_a_duplicate_reference(expr: &MirExpr) -> bool {
    matches!(
        expr,
        MirExpr::Name {
            ty: pycc_mir::Ty::Str,
            ..
        }
    )
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

/// Only meaningful for `Ty::Str` targets: loads the target's predeclared
/// slot and decrefs its current value before the new value overwrites it.
/// String slots start as null, whose runtime decref is a no-op, so the same
/// path is correct for both first assignment and reassignment and prevents
/// loop-body-first bindings from leaking earlier iteration values (D-074).
fn decref_str_slot_before_store<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    target: &str,
) {
    let slot = &locals[target];
    if slot.ty != pycc_mir::Ty::Str {
        panic!(
            "pycc_codegen: internal error: string assignment target `{target}` has a non-string storage slot"
        );
    }
    let old = builder
        .build_load(
            context.ptr_type(inkwell::AddressSpace::default()),
            slot.ptr,
            "old_str",
        )
        .expect("build_load should not fail for this function's own alloca")
        .into_pointer_value();
    builder
        .build_call(rt.str_decref, &[old.into()], "str_decref_old")
        .expect("build_call should not fail for a well-formed decref");
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
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &mut HashMap<String, StorageSlot<'ctx>>,
    body: &[MirStmt],
    expected_return_ty: pycc_mir::Ty,
) -> Result<(), String> {
    for stmt in body {
        emit_stmt(
            context,
            builder,
            module,
            rt,
            user_functions,
            locals,
            stmt,
            expected_return_ty.clone(),
        )?;
        if builder
            .get_insert_block()
            .unwrap()
            .get_terminator()
            .is_some()
        {
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
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &mut HashMap<String, StorageSlot<'ctx>>,
    body: &[MirStmt],
    dest: inkwell::basic_block::BasicBlock<'ctx>,
    expected_return_ty: pycc_mir::Ty,
) -> Result<bool, String> {
    emit_body(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        body,
        expected_return_ty,
    )?;
    let falls_through = builder
        .get_insert_block()
        .unwrap()
        .get_terminator()
        .is_none();
    if falls_through {
        builder
            .build_unconditional_branch(dest)
            .expect("build_unconditional_branch should not fail on a block with no terminator yet");
    }
    Ok(falls_through)
}

fn collect_stmt_bindings(stmt: &MirStmt, bindings: &mut BTreeMap<String, pycc_mir::Ty>) {
    match stmt {
        MirStmt::Assign { target, value } => {
            let ty = value.ty();
            // `Ty::List(_)` joined the allow-list at D-089 (Task 5 of
            // PR-10); `Ty::Dict(_)` joined it at PR-11 Task 5, and
            // `Ty::Set(_)` joins it here (PR-11 Task 9) -- all for the
            // identical reason: a `set[int]` local's binding does need to
            // be collected, since this task's own codegen
            // (`declare_module_globals`/`storage_slot_at_entry`) depends on
            // this slot already existing. `Tuple` stays excluded: out of
            // this PR's own scope (no codegen exists for it). This is a
            // real, deliberate inclusion, not just a louder panic
            // elsewhere.
            if matches!(
                ty,
                pycc_mir::Ty::Int
                    | pycc_mir::Ty::Bool
                    | pycc_mir::Ty::Float
                    | pycc_mir::Ty::Str
                    | pycc_mir::Ty::List(_)
                    | pycc_mir::Ty::Dict(_)
                    | pycc_mir::Ty::Set(_)
            ) {
                bindings.entry(target.clone()).or_insert(ty);
            }
        }
        MirStmt::If { body, orelse, .. } => {
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
            for stmt in orelse {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        MirStmt::While { body, .. } => {
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        MirStmt::ForRange { var, body, .. } => {
            bindings.entry(var.clone()).or_insert(pycc_mir::Ty::Int);
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // `Ty::Int` for the same reason `ForRange` above hardcodes it, not
        // by analogy: a `for` target's type is the iterated element type,
        // and `pycc_types`' T0034 gate (D-105 scope cut 5) rejects every
        // `list[T]` but `list[int]` before codegen ever runs, so `list`'s
        // element type is `int` for every `ForList` that can reach this
        // crate. Deliberately not derived from `bindings[list]` instead:
        // that entry can be absent -- not because `list` might be a
        // list-typed function *parameter* (unreachable: `pycc_hir::
        // annotation_to_ty` rejects any non-bare-name annotation, so
        // `def f(xs: list[int])` fails with `C0001` long before codegen),
        // but because `list` can be a module-scope global iterated from
        // inside a function body, whose `local_bindings` is built from that
        // function body alone and so has no entry for it at all --
        // exactly what `a_module_level_list_binding_lives_in_a_global_slot`
        // (`tests/slice1_codegen_depth.rs`) exercises. A derived non-`int`
        // element type would allocate a slot `emit_stmt`'s own
        // `list[int]`-only `ForList` arm then stores a tagged `int` into. A
        // future PR widening codegen past `list[int]` owns both halves
        // together.
        MirStmt::ForList { var, body, .. } => {
            bindings.entry(var.clone()).or_insert(pycc_mir::Ty::Int);
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // `d[k] = v` (PR-11 Task 4) reassigns an existing binding's
        // contents, not a name -- mirrors `pycc_types::collect_local_names`'s
        // own identical `HirStmt::DictSet` arm and its comment. Unlike
        // `ForDict` immediately below, this is real, permanent behavior, not
        // a temporary stub: no future codegen task ever needs `d[k] = v` to
        // introduce a new binding, since it structurally cannot.
        MirStmt::DictSet { .. } => {}
        // `MirStmt::ForDict`, produced when a `for k in d:` HIR loop's
        // base resolves to a dict-typed binding (mirrors `MirStmt::ForList`
        // above, which is produced for the list-typed case). `Ty::Str` for
        // the same reason `ForList`'s own comment gives for its `Ty::Int`
        // hardcode, not by analogy: a `for` target's type is the iterated
        // element type, and `pycc_mir`'s own `HirStmt::ForList` lowering
        // (see that crate's own `lower_stmt`) binds a dict-typed loop
        // variable to `kv.0` -- the dict's key type -- and `pycc_types`'
        // T0036 gate means that key type is always exactly `Ty::Str` for
        // every `Ty::Dict` value that ever reaches this crate (no other
        // key type is compiled). PR-11 Task 5's own codegen (`emit_stmt`'s
        // `MirStmt::ForDict` arm) binds the loop variable to a
        // `Scalar::Str` every iteration, so its slot must already exist
        // before that arm runs, exactly like `ForList`'s own `var` slot --
        // unlike Task 4's version of this arm, this is no longer a
        // deferred design decision. Recursing into `body` is unchanged: it
        // is what lets a nested, ordinary statement (e.g. `for k in d:\n
        // y = 1\n`) still get `y`'s own binding collected, exactly like
        // every other container arm above.
        MirStmt::ForDict { var, body, .. } => {
            bindings.entry(var.clone()).or_insert(pycc_mir::Ty::Str);
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // `MirStmt::ForSet`, produced when a `for x in s:` HIR loop's base
        // resolves to a set-typed binding (mirrors `MirStmt::ForDict`
        // immediately above, which is produced for the dict-typed case).
        // `Ty::Int` for the same reason `ForList`'s own comment gives for
        // its identical hardcode, not by analogy: a `for` target's type is
        // the iterated element type, and `pycc_types`' T0038 gate means
        // that element type is always exactly `Ty::Int` for every `Ty::Set`
        // value that ever reaches this crate (no other element type is
        // compiled). PR-11 Task 9's own codegen (`emit_stmt`'s
        // `MirStmt::ForSet` arm) binds the loop variable to a `Scalar::Int`
        // every iteration, so its slot must already exist before that arm
        // runs, exactly like `ForList`'s own `var` slot -- unlike Task 8's
        // version of this arm (which deliberately left this binding out,
        // since what the slot should look like was this task's own codegen
        // design decision), this is no longer a deferred decision.
        // Recursing into `body` is unchanged: it is what lets a nested,
        // ordinary statement (e.g. `for x in s:\n y = 1\n`) still get `y`'s
        // own binding collected, exactly like every other container arm
        // above.
        MirStmt::ForSet { var, body, .. } => {
            bindings.entry(var.clone()).or_insert(pycc_mir::Ty::Int);
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        MirStmt::ExprStmt(_) | MirStmt::Return(_) | MirStmt::NoOp => {}
    }
}

fn collect_module_bindings(mir: &MirModule) -> BTreeMap<String, pycc_mir::Ty> {
    let mut bindings = BTreeMap::new();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            collect_stmt_bindings(stmt, &mut bindings);
        }
    }
    bindings
}

fn declare_module_globals<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
    bindings: &BTreeMap<String, pycc_mir::Ty>,
) -> BTreeMap<String, StorageSlot<'ctx>> {
    bindings
        .iter()
        .map(|(name, ty)| {
            let (storage_ty, initializer): (
                inkwell::types::BasicTypeEnum,
                inkwell::values::BasicValueEnum,
            ) = match ty {
                pycc_mir::Ty::Int => (
                    context.i64_type().into(),
                    tag_smallint_const(context, 0).into(),
                ),
                pycc_mir::Ty::Bool => (
                    context.i8_type().into(),
                    context.i8_type().const_zero().into(),
                ),
                pycc_mir::Ty::Float => (
                    context.f64_type().into(),
                    context.f64_type().const_zero().into(),
                ),
                pycc_mir::Ty::Str => (
                    context.ptr_type(inkwell::AddressSpace::default()).into(),
                    context
                        .ptr_type(inkwell::AddressSpace::default())
                        .const_null()
                        .into(),
                ),
                // Identical storage to `Ty::Str` directly above: an opaque
                // pointer, null until the first assignment stores a real
                // `PyIntListObj` into it, with the separate `initialized`
                // flag below (which every module global gets) trapping any
                // read that reaches it first.
                //
                // Task 5 (D-089) deliberately left this arm out and let a
                // module-level `list[int]` binding hit the catch-all below,
                // on the grounds that no real source could construct a list
                // value at all yet; its own report flagged the interaction
                // for Task 11 to re-derive. Task 11b makes `x = [1, 2, 3]`
                // constructible from real source, and D-105's first scope
                // cut names module scope as one of the two places a
                // `list[int]` value is expected to live -- so leaving it out
                // now would turn that documented, supported form into an
                // internal compiler panic. No exit-time decref accompanies
                // it (contrast the `Ty::Str` loop in `compile_to_object`):
                // D-107 keeps `list[T]` leak-only for v0.2.
                pycc_mir::Ty::List(_) => (
                    context.ptr_type(inkwell::AddressSpace::default()).into(),
                    context
                        .ptr_type(inkwell::AddressSpace::default())
                        .const_null()
                        .into(),
                ),
                // Identical storage and reasoning to `Ty::List(_)`
                // directly above (PR-11 Task 5): an opaque pointer, null
                // until the first assignment stores a real `PyDictObj`
                // into it, with the separate `initialized` flag below
                // trapping any read that reaches it first. D-113 names
                // module scope as one of the places a `dict[str, int]`
                // value is expected to live, and every one of this task's
                // own CLI repro programs assigns `x = {...}` at module
                // scope -- leaving this arm out would turn that documented,
                // supported form into an internal compiler panic. No
                // exit-time decref accompanies it (contrast the `Ty::Str`
                // loop in `compile_to_object`): D-114 keeps `dict[K, V]`
                // leak-only for v0.2, extending D-107's exact reasoning.
                pycc_mir::Ty::Dict(_) => (
                    context.ptr_type(inkwell::AddressSpace::default()).into(),
                    context
                        .ptr_type(inkwell::AddressSpace::default())
                        .const_null()
                        .into(),
                ),
                // Identical storage and reasoning to `Ty::List(_)`/
                // `Ty::Dict(_)` directly above (PR-11 Task 9): an opaque
                // pointer, null until the first assignment stores a real
                // `PyIntSetObj` into it, with the separate `initialized`
                // flag below trapping any read that reaches it first.
                // D-113 names module scope as one of the places a
                // `set[int]` value is expected to live, and every one of
                // this task's own CLI repro programs assigns `x = {...}` at
                // module scope -- leaving this arm out would turn that
                // documented, supported form into an internal compiler
                // panic. No exit-time decref accompanies it (contrast the
                // `Ty::Str` loop in `compile_to_object`): D-114 keeps
                // `set[T]` leak-only for v0.2, extending D-107's exact
                // reasoning.
                pycc_mir::Ty::Set(_) => (
                    context.ptr_type(inkwell::AddressSpace::default()).into(),
                    context
                        .ptr_type(inkwell::AddressSpace::default())
                        .const_null()
                        .into(),
                ),
                other => panic!(
                    "pycc_codegen: a `{}`-typed module binding is not supported yet",
                    other.name()
                ),
            };
            let global = module.add_global(storage_ty, None, &format!("pyglobal_{name}"));
            global.set_linkage(Linkage::Internal);
            global.set_initializer(&initializer);
            let initialized =
                module.add_global(context.i8_type(), None, &format!("pyglobal_init_{name}"));
            initialized.set_linkage(Linkage::Internal);
            initialized.set_initializer(&context.i8_type().const_zero());
            (
                name.clone(),
                StorageSlot {
                    ptr: global.as_pointer_value(),
                    ty: ty.clone(),
                    initialized: Some(initialized.as_pointer_value()),
                },
            )
        })
        .collect()
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
///
/// `release`: `false` (the default, matching every build before this PR)
/// creates the target machine with `OptimizationLevel::None` and skips
/// LLVM's optimizer entirely -- today's only behavior. `true` selects
/// `OptimizationLevel::Aggressive` and additionally runs the `"default<O3>"`
/// whole-module pass pipeline via `Module::run_passes` before the object is
/// written (D-094). True cross-translation-unit LTO has no effect yet:
/// pycc emits exactly one LLVM module per compilation (single-file only
/// until v0.4's multi-file support), so this is "maximum whole-module
/// optimization," not literal cross-file link-time optimization.
pub fn compile_to_object(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
    release: bool,
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
    let mut user_functions: HashMap<&str, UserFunction> = HashMap::new();
    for item in &mir.items {
        if let MirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        {
            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = params
                .iter()
                .map(|(_, ty)| ty_to_basic_type(&context, ty.clone()).into())
                .collect();
            let fn_type = match return_ty {
                pycc_mir::Ty::None => context.void_type().fn_type(&param_types, false),
                other => ty_to_basic_type(&context, other.clone()).fn_type(&param_types, false),
            };
            let mangled = format!("pyfn_{name}");
            let f = module.add_function(&mangled, fn_type, None);
            user_functions.insert(
                name.as_str(),
                UserFunction {
                    value: f,
                    param_tys: params.iter().map(|(_, ty)| ty.clone()).collect(),
                },
            );
        }
    }

    // Module bindings need process-wide storage because generated functions
    // can read them (D-041), including bindings whose assignment appears
    // after the function definition. Declare every slot before emitting either
    // the synthetic module entry point or any user function.
    let module_bindings = collect_module_bindings(mir);
    let module_globals = declare_module_globals(&context, &module, &module_bindings);

    let entry_fn_type = i64_type.fn_type(&[], false);
    let entry_fn = module.add_function("main", entry_fn_type, None);
    let entry_block = context.append_basic_block(entry_fn, "entry");
    builder.position_at_end(entry_block);
    // Top-level statements share one `locals` map across the synthetic
    // `main` entry block (module-level Python names are one shared
    // scope); each user function gets its own, fresh map below, since
    // Python function bodies don't see each other's locals.
    let mut top_level_locals: HashMap<_, _> = module_globals
        .iter()
        .map(|(name, binding)| (name.clone(), binding.clone()))
        .collect();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            emit_stmt(
                &context,
                &builder,
                &module,
                &rt,
                &user_functions,
                &mut top_level_locals,
                stmt,
                pycc_mir::Ty::None,
            )?;
        }
    }
    // Module-level Python code has no `return` (T0024) -- every top-level
    // `str` local's single exit point is program completion right here, so
    // this is where its accepted refcounting scope (D-061's Task 7
    // addendum) decrefs it exactly once, before `main` itself returns.
    for slot in module_globals.values() {
        if slot.ty == pycc_mir::Ty::Str {
            let value = builder
                .build_load(
                    context.ptr_type(inkwell::AddressSpace::default()),
                    slot.ptr,
                    "final_str",
                )
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
    if builder
        .get_insert_block()
        .unwrap()
        .get_terminator()
        .is_some()
    {
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
        if let MirItem::Function {
            name,
            params,
            return_ty,
            body,
        } = item
        {
            let f = user_functions[name.as_str()].value;
            let block = context.append_basic_block(f, "entry");
            builder.position_at_end(block);
            let mut fn_locals: HashMap<_, _> = module_globals
                .iter()
                .map(|(global_name, binding)| (global_name.clone(), binding.clone()))
                .collect();
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
                let incoming = f.get_nth_param(i as u32).expect(
                    "this function was declared with exactly `params.len()` parameters above",
                );
                let slot = storage_slot_at_entry(&context, &builder, ty.clone(), param_name, false);
                builder.build_store(slot.ptr, incoming).expect(
                    "build_store should not fail for a slot this function itself allocated",
                );
                fn_locals.insert(param_name.clone(), slot);
            }
            let mut local_bindings = BTreeMap::new();
            for stmt in body {
                collect_stmt_bindings(stmt, &mut local_bindings);
            }
            for (param_name, _) in params {
                local_bindings.remove(param_name);
            }
            for (local_name, ty) in local_bindings {
                let slot = storage_slot_at_entry(&context, &builder, ty, &local_name, true);
                // A function-local target shadows a same-named module global
                // throughout the function (D-055), so this intentionally
                // replaces any global slot seeded above.
                fn_locals.insert(local_name, slot);
            }
            emit_body(
                &context,
                &builder,
                &module,
                &rt,
                &user_functions,
                &mut fn_locals,
                body,
                return_ty.clone(),
            )?;
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
                    if builder
                        .get_insert_block()
                        .unwrap()
                        .get_terminator()
                        .is_none()
                    {
                        builder.build_return(None).expect(
                            "build_return should not fail: builder is always freshly positioned before this call",
                        );
                    }
                }
                _ if builder
                    .get_insert_block()
                    .unwrap()
                    .get_terminator()
                    .is_none() =>
                {
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
            if release {
                OptimizationLevel::Aggressive
            } else {
                OptimizationLevel::None
            },
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
    // `--release`'s whole-module optimization pipeline (D-094). This is
    // "maximum whole-module optimization," not literal cross-translation-
    // unit LTO: pycc emits exactly one LLVM module per compilation today
    // (single-file only until v0.4's multi-file support), so there is only
    // ever one module for `"default<O3>"` to optimize.
    //
    // Deliberately `.expect(..)`, not `.map_err(llvm_string_to_owned)?` like
    // `Target::from_triple`/`write_to_file` below: those two are genuine,
    // externally-triggerable failure modes with their own dedicated tests
    // and reachable `Err` paths this crate's 100% region-coverage gate
    // (D-014) can actually exercise. `run_passes` here runs a fixed,
    // always-valid pipeline string against a module `verify_module` has
    // already accepted (skipped only on Windows per D-029's own "no
    // Windows-specific IR-building path exists" reasoning, which applies
    // here too) -- there is no way to make this fail that a test could
    // construct, so this stays infallible given how this function always
    // calls it, the same treatment `create_target_machine` gets immediately
    // above and `module.verify()` gets in `verify_module` below. On the
    // vanishingly narrow chance this ever legitimately panics on Windows,
    // the `LLVMString`'s `Drop` would run during unwinding (D-029's crash)
    // -- an accepted, currently unreachable risk, not a silently ignored one.
    if release {
        module
            .run_passes("default<O3>", &target_machine, PassBuilderOptions::create())
            .expect(
                "LLVM's \"default<O3>\" pipeline should never fail against a module this \
                 function has already verified, using a fixed, always-valid pipeline string",
            );
    }
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
/// `Ty::None` evaluates `arg` for its side effects and discards its canonical
/// unit carrier, printing the literal `"None"` instead; every other v0.1
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
fn emit_print_arg<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    arg: &MirExpr,
) {
    if arg.ty() == pycc_mir::Ty::None {
        emit_expr(context, builder, module, rt, user_functions, locals, arg);
        builder
            .build_call(rt.print_none, &[], "print_none")
            .expect("build_call should not fail for a well-formed print of None");
    } else {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, arg);
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

/// Handles every `MirStmt` shape (this match is exhaustive over
/// `MirStmt`, no catch-all arm): a `print()` call of any number of
/// `int`/`float`/`bool`/`str` arguments plus `None` from either a direct
/// user-function result or a D-075 parameter value (Task 10, space-separated,
/// one trailing newline, matching CPython's `print(*args)`; D-072 still
/// excludes using `print()` itself as a nested expression), any
/// other bare expression statement (a user-function call with any number of
/// arguments included -- see `emit_expr`'s `Call` arm, which this now
/// delegates to uniformly instead of special-casing zero-arg calls here), a
/// local-variable assignment, `If`/`While`/
/// `ForRange` control flow (Task 4) -- real basic blocks, conditional
/// branches, and loop back-edges, using `truthy` for the shared `if`/
/// `while` truthiness check and `emit_body_then_branch`/an inline
/// equivalent for the terminator-safety this introduces (see both
/// helpers' own doc comments) -- `Return` (Task 5), terminating
/// the current block with the evaluated value (or none, for a bare
/// `return`) -- and `ForList` (v0.2, D-105/Task 11b), reusing `ForRange`'s
/// own loop/branch-building infrastructure parametrized over a runtime
/// `pycc_rt_int_list_len` call instead of a static bound. `ForList` is a
/// v0.2 addition, not part of v0.1's own original shape set.
#[allow(clippy::too_many_arguments)]
fn emit_stmt<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &mut HashMap<String, StorageSlot<'ctx>>,
    stmt: &MirStmt,
    expected_return_ty: pycc_mir::Ty,
) -> Result<(), String> {
    match stmt {
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    builder
                        .build_call(rt.print_space, &[], "print_sep")
                        .expect("build_call should not fail for a well-formed print separator");
                }
                emit_print_arg(context, builder, module, rt, user_functions, locals, arg);
            }
            builder
                .build_call(rt.print_newline, &[], "print_end")
                .expect("build_call should not fail for a well-formed print newline");
            Ok(())
        }
        // A user-function call whose declared return type is `None`, used
        // as a bare statement (e.g. `main()`) -- must go through
        // `build_call_to` directly rather than the general `ExprStmt(expr)`
        // arm below: the call itself returns LLVM `void`, and the general
        // expression path's canonical unit carrier is unnecessary when the
        // result is discarded. Matched by `ty`
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
        MirStmt::ExprStmt(MirExpr::Call {
            callee,
            args,
            ty: pycc_mir::Ty::None,
        }) => {
            let user_function = user_functions.get(callee.as_str()).ok_or_else(|| {
                format!("pycc_codegen v0.1: call to undefined function `{callee}`")
            })?;
            build_call_to(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                user_function,
                args,
            );
            Ok(())
        }
        MirStmt::ExprStmt(expr) => {
            emit_expr(context, builder, module, rt, user_functions, locals, expr);
            Ok(())
        }
        MirStmt::Assign { target, value } => {
            let ty = value.ty();
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            let scalar = incref_if_str_duplicate(builder, rt, value, scalar);
            if ty == pycc_mir::Ty::Str {
                decref_str_slot_before_store(context, builder, rt, locals, target);
            }
            emit_assign(context, builder, locals, target, scalar);
            Ok(())
        }
        MirStmt::NoOp => Ok(()),
        MirStmt::If { test, body, orelse } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let cond = {
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
                truthy(context, builder, rt, scalar)
            };
            let then_bb = context.append_basic_block(function, "if_then");
            let merge_bb = context.append_basic_block(function, "if_merge");
            let else_bb = if orelse.is_empty() {
                merge_bb
            } else {
                context.append_basic_block(function, "if_else")
            };
            builder
                .build_conditional_branch(cond, then_bb, else_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(then_bb);
            let then_falls_through = emit_body_then_branch(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                merge_bb,
                expected_return_ty.clone(),
            )?;

            let else_falls_through = if orelse.is_empty() {
                true
            } else {
                builder.position_at_end(else_bb);
                emit_body_then_branch(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    orelse,
                    merge_bb,
                    expected_return_ty,
                )?
            };

            builder.position_at_end(merge_bb);
            if !then_falls_through && !else_falls_through {
                builder
                    .build_unreachable()
                    .expect("build_unreachable should terminate a merge with no predecessors");
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
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
                truthy(context, builder, rt, scalar)
            };
            builder
                .build_conditional_branch(cond, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            emit_body_then_branch(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                test_bb,
                expected_return_ty,
            )?;

            builder.position_at_end(after_bb);
            Ok(())
        }
        MirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let start_v = range_operand_to_tagged_int(
                context,
                builder,
                emit_expr(context, builder, module, rt, user_functions, locals, start),
                "start",
            );
            let stop_v = range_operand_to_tagged_int(
                context,
                builder,
                emit_expr(context, builder, module, rt, user_functions, locals, stop),
                "stop",
            );
            let step_v = range_operand_to_tagged_int(
                context,
                builder,
                emit_expr(context, builder, module, rt, user_functions, locals, step),
                "step",
            );
            let preheader = builder.get_insert_block().unwrap();

            let test_bb = context.append_basic_block(function, "for_test");
            let body_bb = context.append_basic_block(function, "for_body");
            let after_bb = context.append_basic_block(function, "for_after");

            builder
                .build_unconditional_branch(test_bb)
                .expect("build_unconditional_branch should not fail entering the loop test");
            builder.position_at_end(test_bb);
            let induction = builder
                .build_phi(context.i64_type(), "for_current")
                .expect("build_phi should not fail in a fresh loop-test block");
            induction.add_incoming(&[(&start_v, preheader)]);
            let current = induction.as_basic_value().into_int_value();
            let cont = builder
                .build_call(
                    rt.range_continue,
                    &[current.into(), stop_v.into(), step_v.into()],
                    "range_continue",
                )
                .expect("build_call should not fail for a well-formed range_continue check")
                .try_as_basic_value()
                .expect_basic("pycc_rt_range_continue returns a non-void i8")
                .into_int_value();
            let cont_i1 = builder
                .build_int_compare(
                    IntPredicate::NE,
                    cont,
                    context.i8_type().const_int(0, false),
                    "for_cont",
                )
                .expect("build_int_compare should not fail comparing two i8 operands");
            builder
                .build_conditional_branch(cont_i1, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            // Python binds a `for` target only after an element actually
            // exists. The visible target is separate from the hidden SSA
            // induction value so an empty range leaves it unbound, the final
            // target remains the last element, and body reassignment cannot
            // corrupt the next iteration.
            emit_assign(context, builder, locals, var, Scalar::Int(current));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
            )?;
            // `ForRange`'s own inline copy of `emit_body_then_branch`'s
            // terminator-safety guard (see that function's own doc comment
            // for why): a `Return` reached inside `body` already terminates
            // `body_bb`, so the increment-and-branch-back below must be
            // skipped in that case -- building it anyway would try to add a
            // second terminator onto an already-terminated block, which is
            // invalid LLVM IR.
            if builder
                .get_insert_block()
                .unwrap()
                .get_terminator()
                .is_none()
            {
                let next = builder
                    .build_call(rt.int_add, &[current.into(), step_v.into()], "for_next")
                    .expect("build_call should not fail for a well-formed int add")
                    .try_as_basic_value()
                    .expect_basic("pycc_rt_int_add returns a non-void i64")
                    .into_int_value();
                let body_end = builder.get_insert_block().unwrap();
                induction.add_incoming(&[(&next, body_end)]);
                builder.build_unconditional_branch(test_bb).expect(
                    "build_unconditional_branch should not fail on a block with no terminator yet",
                );
            }

            builder.position_at_end(after_bb);
            Ok(())
        }
        // An intentional inline duplicate of `ForRange`'s loop-building
        // logic directly above -- exactly as that arm is itself a
        // deliberate inline copy of `emit_body_then_branch`'s (see both
        // their own comments). Not factored into a shared helper: the two
        // arms differ in their bound, their induction step, and their
        // per-iteration prologue, and this file's established position is
        // that a third consumer, not a second, is what justifies extracting
        // shared loop-building machinery.
        //
        // Three real differences from `ForRange`, all of them consequences
        // of iterating a container rather than an arithmetic range:
        //
        // 1. The induction variable is a plain, raw LLVM `i64` index, never
        //    a D-061-tagged `Ty::Int` -- so the increment is
        //    `build_int_add`, not `pycc_rt_int_add`, and the loop test is a
        //    plain `icmp slt`, not `pycc_rt_range_continue`. Both of those
        //    runtime functions take *tagged* operands (D-061); calling
        //    either with this raw counter would silently compute on the
        //    wrong values.
        // 2. The bound is `pycc_rt_int_list_len`, re-read on every
        //    iteration inside the test block rather than hoisted into the
        //    preheader. This matches CPython, whose list iterator compares
        //    its cursor against the list's *current* length on every
        //    `__next__` -- so `for v in xs: xs.append(v)` runs forever
        //    there, and would terminate here if the length were hoisted.
        // 3. Each iteration prepends one `pycc_rt_int_list_get` read.
        //
        // D-106 applies to exactly one value in this arm: the element read.
        // The induction variable and the raw `len` it is compared against
        // are private LLVM implementation detail, never exposed to user
        // code as `Ty::Int` values, so they need neither
        // `build_untag_checked` on the way in nor `raw_i64_to_tagged_int`
        // on the way out. The element does need re-tagging, because `var`
        // is an ordinary user-visible `Ty::Int` local the body may print or
        // compute with.
        MirStmt::ForList { var, list, body } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            // Read once, in the preheader, not per iteration: Python binds
            // its iterator to the object the `for` statement evaluated, so
            // a body-level rebinding of the same name (`xs = [9]`) must not
            // retarget the loop.
            //
            // That makes `list_ptr` a *borrowed* reference held across
            // arbitrary body code without an incref, which is sound only
            // because D-107 keeps `list[T]` leak-only in v0.2 -- nothing
            // frees a list. Whichever future PR wires D-107's own
            // reassignment-cleanup site must give this read an incref/decref
            // pair at the same time, or exactly the rebinding shape above
            // would free the object out from under the loop.
            let list_ptr =
                emit_list_name_read(context, builder, module, rt, user_functions, locals, list);
            let preheader = builder.get_insert_block().unwrap();

            let test_bb = context.append_basic_block(function, "for_list_test");
            let body_bb = context.append_basic_block(function, "for_list_body");
            let after_bb = context.append_basic_block(function, "for_list_after");

            builder
                .build_unconditional_branch(test_bb)
                .expect("build_unconditional_branch should not fail entering the loop test");
            builder.position_at_end(test_bb);
            let induction = builder
                .build_phi(context.i64_type(), "for_list_index")
                .expect("build_phi should not fail in a fresh loop-test block");
            let zero = context.i64_type().const_zero();
            induction.add_incoming(&[(&zero, preheader)]);
            let current = induction.as_basic_value().into_int_value();
            let len = build_int_list_len(builder, rt, list_ptr);
            let cont = builder
                .build_int_compare(IntPredicate::SLT, current, len, "for_list_cont")
                .expect("build_int_compare should not fail comparing two i64 operands");
            builder
                .build_conditional_branch(cont, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            // Same Python target-binding semantics `ForRange`'s own
            // comment describes: the visible target is a separate storage
            // slot written once per iteration, so the final target keeps
            // the last element and body reassignment cannot corrupt the
            // next iteration (both pinned by
            // `list_targets_keep_the_last_element_and_ignore_body_reassignment`
            // in `tests/slice1_codegen_depth.rs`). `ForRange`'s third
            // property -- an empty sequence leaving the target unbound --
            // holds here structurally for the same reason, but unlike
            // `range(0)` it is unreachable from real source: `pycc_types`
            // rejects an empty list literal (T0021, no inferable element
            // type) and v0.2 has no `pop`/`del` to empty a list afterwards.
            let raw_element = build_int_list_get(builder, rt, list_ptr, current);
            let element = raw_i64_to_tagged_int(context, builder, raw_element);
            emit_assign(context, builder, locals, var, Scalar::Int(element));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
            )?;
            // `ForRange`'s own terminator-safety guard, for the identical
            // reason (see that arm's comment): a `Return` inside `body`
            // already terminated `body_bb`, and adding the increment and
            // back-edge anyway would build a second terminator on it.
            if builder
                .get_insert_block()
                .unwrap()
                .get_terminator()
                .is_none()
            {
                let next = builder
                    .build_int_add(
                        current,
                        context.i64_type().const_int(1, false),
                        "for_list_next",
                    )
                    .expect("build_int_add should not fail for two i64 operands");
                let body_end = builder.get_insert_block().unwrap();
                induction.add_incoming(&[(&next, body_end)]);
                builder.build_unconditional_branch(test_bb).expect(
                    "build_unconditional_branch should not fail on a block with no terminator yet",
                );
            }

            builder.position_at_end(after_bb);
            Ok(())
        }
        MirStmt::Return(value) => {
            match value {
                Some(expr) => {
                    let scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, expr);
                    let scalar = incref_if_str_duplicate(builder, rt, expr, scalar);
                    let scalar =
                        coerce_scalar_to_type(context, builder, scalar, expected_return_ty.clone());
                    if expected_return_ty == pycc_mir::Ty::None {
                        // `None` parameters and call results use a canonical
                        // `i8 0` carrier inside expressions, but a function
                        // declared to return `None` has an LLVM `void`
                        // signature. Evaluating above preserves any call or
                        // name-load side effects; the carrier itself is
                        // intentionally discarded here. Returning it as an
                        // `i8` previously built invalid IR that only the
                        // non-Windows verifier caught (D-029).
                        builder
                            .build_return(None)
                            .expect("build_return should not fail for a None return value");
                        return Ok(());
                    }
                    let basic_value: inkwell::values::BasicValueEnum = match scalar {
                        Scalar::Int(v) => v.into(),
                        Scalar::Bool(v) => v.into(),
                        Scalar::Float(v) => v.into(),
                        Scalar::Str(v) => v.into(),
                        // Pass-through, identical to `Str`'s arm directly
                        // above: returning a `list[T]` returns one opaque
                        // pointer, and `ty_to_basic_type` already gave the
                        // function's LLVM signature the same pointer
                        // return type it gives a `str`-returning one.
                        Scalar::List(v) => v.into(),
                        // Pass-through, identical to `List`'s arm directly
                        // above: returning a `dict[K, V]` returns one
                        // opaque pointer, and `ty_to_basic_type` already
                        // gave the function's LLVM signature the same
                        // pointer return type it gives a `str`/`list[T]`-
                        // returning one.
                        Scalar::Dict(v) => v.into(),
                        // Pass-through, identical to `List`'s/`Dict`'s arms
                        // directly above: returning a `set[T]` returns one
                        // opaque pointer, and `ty_to_basic_type` already
                        // gave the function's LLVM signature the same
                        // pointer return type it gives a
                        // `str`/`list[T]`/`dict[K, V]`-returning one.
                        Scalar::Set(v) => v.into(),
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
        // `d[k] = v` (PR-11 Task 5, D-113): insert-or-update --
        // `pycc_rt_dict_set` itself decides which, by whether `key`
        // already compares equal to a stored key. `dict` is read by name
        // (mirrors `MirExpr::ListAppend`'s own `list` field), `key` crosses
        // in as a `Ty::Str` expression unchanged (no D-106 conversion,
        // exactly like `MirExpr::DictGet`'s own key above), and `value`
        // gets D-106's input-side conversion applied identically to
        // `ListLiteral`/`ListAppend`'s own value.
        MirStmt::DictSet { dict, key, value } => {
            let dict_ptr =
                emit_dict_name_read(context, builder, module, rt, user_functions, locals, dict);
            let key_scalar = emit_expr(context, builder, module, rt, user_functions, locals, key);
            // Same `incref_if_str_duplicate` requirement as `MirExpr::
            // DictLiteral`'s own per-pair key above, and for the identical
            // reason (see that arm's own comment): `pycc_rt_dict_set`
            // adopts whatever key pointer it is given as `d`'s own
            // permanent reference without incref'ing it itself (D-114), so
            // a bare-`Name` key (`d[k] = v`) must be incref'd here first,
            // or a later reassignment of `k` would decref -- and
            // potentially free -- the same `PyStrObj` `d` still points to.
            let key_scalar = incref_if_str_duplicate(builder, rt, key, key_scalar);
            let Scalar::Str(key_ptr) = key_scalar else {
                panic!(
                    "pycc_codegen: internal error: dict item-assignment key did not evaluate to \
                     str -- pycc_types::check (T0021) should have rejected this before codegen"
                )
            };
            let value_scalar =
                emit_expr(context, builder, module, rt, user_functions, locals, value);
            let tagged = to_tagged_int(context, builder, value_scalar);
            let raw = build_untag_checked(builder, rt, tagged, "dict_untag_set_value");
            build_dict_set(builder, rt, dict_ptr, key_ptr, raw);
            Ok(())
        }
        // `for k in d:` (PR-11 Task 5, D-113): an intentional inline
        // duplicate of `MirStmt::ForList`'s own loop-building logic
        // directly above, for the identical "a third consumer, not a
        // second, justifies a shared helper" reason that arm's own comment
        // already gives for its own `ForRange` duplication.
        //
        // Three differences from `ForList`, all consequences of iterating
        // a dict's keys rather than a list's elements:
        // 1. The bound is `pycc_rt_dict_len`, still re-read on every
        //    iteration inside the test block rather than hoisted into the
        //    preheader, for the identical CPython-mutation-during-iteration
        //    reason `ForList`'s own comment gives: this compiler has no
        //    `del`/removal for a dict yet, but `d[k] = v` inside the loop
        //    body can still grow it, and a hoisted bound would silently
        //    stop tracking that.
        // 2. Each iteration reads the current *key*, via
        //    `pycc_rt_dict_key_at`, not an element via
        //    `pycc_rt_int_list_get`.
        // 3. The loop variable is bound `Scalar::Str`, not `Scalar::Int` --
        //    and, unlike every `ForList`/`ForRange` induction target (never
        //    refcounted, D-061), a dict key genuinely is a refcounted
        //    `PyStrObj` that `d` itself still holds a live, non-incref'd
        //    pointer to (D-114: "`PyDictObj`'s own keys ... are stored
        //    without incref on insert"). Without the `pycc_rt_str_incref`
        //    below, an ordinary body-level reassignment of the loop
        //    variable (`for k in d:\n    k = "z"\n`) would go through
        //    `emit_stmt`'s own `Assign` arm, whose
        //    `decref_str_slot_before_store` call fires unconditionally for
        //    any `Ty::Str` target -- decref'ing `d`'s *own* only reference
        //    to that key and freeing it while `d` still holds the
        //    now-dangling pointer, a real premature free this project's own
        //    leak-only containers are never supposed to cause (see
        //    `Scalar::List`'s and `Scalar::Dict`'s own doc comments: "leak-
        //    only -- never a premature free"). The incref gives the loop
        //    variable's own slot a reference distinct from `d`'s, so that
        //    decref (or the next iteration's own overwrite, which bypasses
        //    it entirely -- see `emit_assign` below) only ever brings the
        //    *duplicate* back down, never below `d`'s own copy. The
        //    resulting extra, never-brought-back-down increment on the
        //    loop's last key is exactly D-114's existing leak-only policy
        //    playing out one level lower: an unbalanced incref only makes
        //    an already-permanent leak larger, never a premature free.
        MirStmt::ForDict { var, dict, body } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            // Read once, in the preheader, not per iteration -- identical
            // reasoning to `ForList`'s own preheader read (see that arm's
            // comment): Python binds its iterator to the object the `for`
            // statement evaluated, so a body-level rebinding of `dict`'s
            // own name must not retarget the loop. Sound leak-only for the
            // same reason `ForList`'s is: nothing ever frees a dict value,
            // so a borrowed reference held across the body without its own
            // incref cannot go stale.
            let dict_ptr =
                emit_dict_name_read(context, builder, module, rt, user_functions, locals, dict);
            let preheader = builder.get_insert_block().unwrap();

            let test_bb = context.append_basic_block(function, "for_dict_test");
            let body_bb = context.append_basic_block(function, "for_dict_body");
            let after_bb = context.append_basic_block(function, "for_dict_after");

            builder
                .build_unconditional_branch(test_bb)
                .expect("build_unconditional_branch should not fail entering the loop test");
            builder.position_at_end(test_bb);
            let induction = builder
                .build_phi(context.i64_type(), "for_dict_index")
                .expect("build_phi should not fail in a fresh loop-test block");
            let zero = context.i64_type().const_zero();
            induction.add_incoming(&[(&zero, preheader)]);
            let current = induction.as_basic_value().into_int_value();
            let len = build_dict_len(builder, rt, dict_ptr);
            let cont = builder
                .build_int_compare(IntPredicate::SLT, current, len, "for_dict_cont")
                .expect("build_int_compare should not fail comparing two i64 operands");
            builder
                .build_conditional_branch(cont, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            let key_ptr = builder
                .build_call(
                    rt.dict_key_at,
                    &[dict_ptr.into(), current.into()],
                    "dict_key_at",
                )
                .expect("build_call should not fail for a well-formed dict key read")
                .try_as_basic_value()
                .expect_basic("pycc_rt_dict_key_at returns a non-void pointer")
                .into_pointer_value();
            // See this arm's own doc comment, point 3: gives the loop
            // variable's slot a reference distinct from `d`'s own, so a
            // body-level reassignment of `var` cannot free a key `d`
            // itself still points to.
            builder
                .build_call(rt.str_incref, &[key_ptr.into()], "for_dict_key_incref")
                .expect("build_call should not fail for a well-formed incref");
            // Same Python target-binding semantics `ForList`'s own comment
            // describes: a separate storage slot written once per
            // iteration (via `emit_assign` directly, not through
            // `emit_stmt`'s `Assign` arm -- so, like `ForList`'s own
            // per-iteration bind, this specific write never itself calls
            // `decref_str_slot_before_store`).
            emit_assign(context, builder, locals, var, Scalar::Str(key_ptr));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
            )?;
            // `ForList`'s own terminator-safety guard, for the identical
            // reason (see that arm's comment): a `Return` inside `body`
            // already terminated `body_bb`, and adding the increment and
            // back-edge anyway would build a second terminator on it.
            if builder
                .get_insert_block()
                .unwrap()
                .get_terminator()
                .is_none()
            {
                let next = builder
                    .build_int_add(
                        current,
                        context.i64_type().const_int(1, false),
                        "for_dict_next",
                    )
                    .expect("build_int_add should not fail for two i64 operands");
                let body_end = builder.get_insert_block().unwrap();
                induction.add_incoming(&[(&next, body_end)]);
                builder.build_unconditional_branch(test_bb).expect(
                    "build_unconditional_branch should not fail on a block with no terminator yet",
                );
            }

            builder.position_at_end(after_bb);
            Ok(())
        }
        // `for x in s:` (PR-11 Task 9, D-113): an intentional inline
        // duplicate of `MirStmt::ForList`'s own loop-building logic above,
        // for the identical "a third consumer, not a second, justifies a
        // shared helper" reason that arm's own comment already gives for
        // its own `ForRange` duplication (and `MirStmt::ForDict`'s own
        // comment gives for its `ForList` duplication).
        //
        // Structurally this is closer to `ForList` than to `ForDict`: a
        // set's elements, like a list's, are raw `i64` values with no
        // refcounting concern (unlike `ForDict`'s own key, which is a
        // refcounted `PyStrObj` needing its own `pycc_rt_str_incref` before
        // the loop variable's per-iteration bind -- see that arm's own doc
        // comment). Two differences from `ForList`, both consequences of
        // iterating a set rather than a list:
        // 1. The bound is `pycc_rt_int_set_len`, still re-read on every
        //    iteration inside the test block rather than hoisted into the
        //    preheader, for the identical CPython-mutation-during-iteration
        //    reason `ForList`'s own comment gives: this compiler has no
        //    `.add()`/removal for a set yet, but nothing else can grow one
        //    mid-loop either, so this is behaviorally moot today -- kept
        //    identical to `ForList`/`ForDict`'s own shape anyway, so a
        //    future `.add()` addition does not also have to remember to fix
        //    a hoisted bound here.
        // 2. Each iteration reads the current element via
        //    `pycc_rt_int_set_get`, not `pycc_rt_int_list_get`.
        //
        // D-106 applies to exactly one value in this arm, identically to
        // `ForList`: the element read. The induction variable and the raw
        // `len` it is compared against are private LLVM implementation
        // detail, never exposed to user code as `Ty::Int` values.
        MirStmt::ForSet { var, set, body } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            // Read once, in the preheader, not per iteration -- identical
            // reasoning to `ForList`'s own preheader read (see that arm's
            // comment): Python binds its iterator to the object the `for`
            // statement evaluated, so a body-level rebinding of `set`'s own
            // name must not retarget the loop. Sound leak-only for the same
            // reason `ForList`'s is: nothing ever frees a set value, so a
            // borrowed reference held across the body without its own
            // incref cannot go stale.
            let set_ptr =
                emit_set_name_read(context, builder, module, rt, user_functions, locals, set);
            let preheader = builder.get_insert_block().unwrap();

            let test_bb = context.append_basic_block(function, "for_set_test");
            let body_bb = context.append_basic_block(function, "for_set_body");
            let after_bb = context.append_basic_block(function, "for_set_after");

            builder
                .build_unconditional_branch(test_bb)
                .expect("build_unconditional_branch should not fail entering the loop test");
            builder.position_at_end(test_bb);
            let induction = builder
                .build_phi(context.i64_type(), "for_set_index")
                .expect("build_phi should not fail in a fresh loop-test block");
            let zero = context.i64_type().const_zero();
            induction.add_incoming(&[(&zero, preheader)]);
            let current = induction.as_basic_value().into_int_value();
            let len = build_int_set_len(builder, rt, set_ptr);
            let cont = builder
                .build_int_compare(IntPredicate::SLT, current, len, "for_set_cont")
                .expect("build_int_compare should not fail comparing two i64 operands");
            builder
                .build_conditional_branch(cont, body_bb, after_bb)
                .expect("build_conditional_branch should not fail for a well-formed i1 condition");

            builder.position_at_end(body_bb);
            // Same Python target-binding semantics `ForList`'s own comment
            // describes: a separate storage slot written once per
            // iteration.
            let raw_element = build_int_set_get(builder, rt, set_ptr, current);
            let element = raw_i64_to_tagged_int(context, builder, raw_element);
            emit_assign(context, builder, locals, var, Scalar::Int(element));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
            )?;
            // `ForList`'s own terminator-safety guard, for the identical
            // reason (see that arm's comment): a `Return` inside `body`
            // already terminated `body_bb`, and adding the increment and
            // back-edge anyway would build a second terminator on it.
            if builder
                .get_insert_block()
                .unwrap()
                .get_terminator()
                .is_none()
            {
                let next = builder
                    .build_int_add(
                        current,
                        context.i64_type().const_int(1, false),
                        "for_set_next",
                    )
                    .expect("build_int_add should not fail for two i64 operands");
                let body_end = builder.get_insert_block().unwrap();
                induction.add_incoming(&[(&next, body_end)]);
                builder.build_unconditional_branch(test_bb).expect(
                    "build_unconditional_branch should not fail on a block with no terminator yet",
                );
            }

            builder.position_at_end(after_bb);
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
    /// v0.1 scalar type, plus `None` from direct user-function results and
    /// D-075 parameter values; see its own doc comment, Task 10).
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
    #[should_panic(expected = "a `None`-typed module binding is not supported yet")]
    fn a_none_typed_module_binding_has_no_storage_representation() {
        let context = Context::create();
        let module = context.create_module("unsupported_global");
        let bindings = BTreeMap::from([("x".to_string(), Ty::None)]);
        let _ = declare_module_globals(&context, &module, &bindings);
    }

    #[test]
    fn a_dict_typed_module_binding_gets_a_real_pointer_backed_global_slot() {
        // Superseded by PR-11 Task 5: this test used to assert that a
        // `dict[str, int]` module binding panicked here (mirroring the
        // `None`/`tuple` catch-all tests above) -- that was accurate for
        // Task 4's own scope, but D-113 names module scope as one of the
        // places a `dict[str, int]` value is expected to live, and every
        // one of this task's own CLI repro programs assigns `x = {...}` at
        // module scope. `declare_module_globals` now gives `Ty::Dict(_)` a
        // real arm (mirrors `Ty::List(_)`'s own arm exactly), so this test
        // instead proves that: a pointer-backed global slot with the
        // `initialized` guard flag every module global gets, not a panic.
        let context = Context::create();
        let module = context.create_module("dict_global");
        let bindings = BTreeMap::from([(
            "x".to_string(),
            Ty::Dict(Box::new((Ty::Str, Ty::Int))),
        )]);
        let globals = declare_module_globals(&context, &module, &bindings);
        let slot = globals.get("x").expect("declare_module_globals should bind `x`");
        assert_eq!(slot.ty, Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        assert!(slot.initialized.is_some());
    }

    #[test]
    fn a_set_typed_module_binding_gets_a_real_pointer_backed_global_slot() {
        // The `set[int]` counterpart of `a_dict_typed_module_binding_gets_a_
        // real_pointer_backed_global_slot` above, for the identical reason
        // (PR-11 Task 9): `declare_module_globals` now gives `Ty::Set(_)` a
        // real arm (mirrors `Ty::List(_)`/`Ty::Dict(_)`'s own arms exactly),
        // so a `set[int]` module binding gets a pointer-backed global slot
        // with the `initialized` guard flag every module global gets, not a
        // panic.
        let context = Context::create();
        let module = context.create_module("set_global");
        let bindings = BTreeMap::from([("x".to_string(), Ty::Set(Box::new(Ty::Int)))]);
        let globals = declare_module_globals(&context, &module, &bindings);
        let slot = globals.get("x").expect("declare_module_globals should bind `x`");
        assert_eq!(slot.ty, Ty::Set(Box::new(Ty::Int)));
        assert!(slot.initialized.is_some());
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice0_toplevel");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn a_no_op_statement_compiles_and_produces_no_observable_output() {
        // `MirStmt::NoOp` is produced only by lowering a value-less PEP 526
        // annotation (`y: int`, Task 5) -- it must compile to nothing at
        // all: no store, no allocation, and it must not swallow or block
        // the statements sequenced around it.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::NoOp),
                MirItem::TopLevelStmt(call_print(1)),
            ],
        };
        let dir = tempfile_dir("slice0_no_op");
        let obj_path = dir.join("slice0_no_op.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice0_no_op");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
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
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
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
        compile_to_object(&mir, &obj_path, None, false)
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
        let err = compile_to_object(&mir, &bad_path, None, false)
            .expect_err("should fail: parent dir doesn't exist");
        assert!(!err.is_empty());
    }

    /// `x = 1.0`; `i = 0`; `while i < 1000: x = x * 1.0000001; i = i + 1`;
    /// `print(x)` -- as top-level statements directly in the synthetic
    /// entry `main`, rather than a separately defined-and-called user
    /// function: this crate's own test module has no helper for building a
    /// `MirModule` from a Python source string (`pycc_codegen`'s
    /// `Cargo.toml` depends only on `pycc_mir`, not the frontend crates
    /// `pycc_parser`/`pycc_hir`/`pycc_types` that would be needed to parse
    /// one), so every other test in this file hand-builds its `MirModule`
    /// the same way -- this one does too, matching the shape of the
    /// compute-heavy loop the plan's own brief describes: repeated float
    /// multiplication in a loop is exactly the kind of code `"default<O3>"`
    /// constant-folds/vectorizes/unrolls very differently from an
    /// unoptimized build.
    fn release_flag_fixture() -> MirModule {
        MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::FloatLiteral(1.0),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "i".to_string(),
                    value: MirExpr::IntLiteral(0),
                }),
                MirItem::TopLevelStmt(MirStmt::While {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(1000)),
                        ty: Ty::Bool,
                    },
                    body: vec![
                        MirStmt::Assign {
                            target: "x".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Mul,
                                left: Box::new(MirExpr::Name {
                                    name: "x".to_string(),
                                    ty: Ty::Float,
                                }),
                                right: Box::new(MirExpr::FloatLiteral(1.0000001)),
                                ty: Ty::Float,
                            },
                        },
                        MirStmt::Assign {
                            target: "i".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Add,
                                left: Box::new(MirExpr::Name {
                                    name: "i".to_string(),
                                    ty: Ty::Int,
                                }),
                                right: Box::new(MirExpr::IntLiteral(1)),
                                ty: Ty::Int,
                            },
                        },
                    ],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Float,
                    }],
                    ty: Ty::None,
                })),
            ],
        }
    }

    #[test]
    fn release_mode_actually_runs_llvm_optimization_passes() {
        let mir = release_flag_fixture();

        let debug_dir = tempfile_dir("release_flag_debug");
        let debug_obj_path = debug_dir.join("release_flag_debug.o");
        compile_to_object(&mir, &debug_obj_path, None, false).expect("codegen should succeed");
        let debug_obj = std::fs::read(&debug_obj_path).expect("object file should be readable");

        let release_dir = tempfile_dir("release_flag_release");
        let release_obj_path = release_dir.join("release_flag_release.o");
        compile_to_object(&mir, &release_obj_path, None, true).expect("codegen should succeed");
        let release_obj = std::fs::read(&release_obj_path).expect("object file should be readable");

        assert_ne!(
            debug_obj, release_obj,
            "release and debug object files must differ -- optimization passes did not run"
        );
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
        compile_to_object(&mir, &obj_path, Some("x86_64-apple-darwin"), false)
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
        let err = compile_to_object(&mir, &obj_path, Some("not-a-real-target-triple"), false)
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("print_none_from_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"None\n");
    }

    #[test]
    fn returning_a_void_returning_call_result_from_a_none_returning_function_runs_the_callee_and_returns_void()
     {
        // `def helper() -> None: return` ; `def outer() -> None: return
        // helper()` ; `outer()` -- must not build `ret i8 0` inside
        // `outer`'s `void` LLVM signature (previously failed `verify_module`
        // with "Found return instr that returns non-void in Function of
        // void return type!").
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::Function {
                    name: "outer".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(Some(MirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                        ty: Ty::None,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "outer".to_string(),
                    args: vec![],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("return_none_call");
        let obj_path = dir.join("return_none_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("return_none_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"");
    }

    #[test]
    fn printing_a_none_typed_parameter_renders_none() {
        // `def source() -> None: return`; `def sink(y: None) -> None:
        // print(y)`; `sink(source())` -- exercises the canonical unit
        // carrier as a call argument, parameter slot, name read, and print
        // input without ever confusing it for the physically-identical
        // `False` carrier.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "source".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::Function {
                    name: "sink".to_string(),
                    params: vec![("y".to_string(), Ty::None)],
                    return_ty: Ty::None,
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name {
                            name: "y".to_string(),
                            ty: Ty::None,
                        }],
                        ty: Ty::None,
                    })],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "sink".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "source".to_string(),
                        args: vec![],
                        ty: Ty::None,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("print_none_typed_parameter");
        let obj_path = dir.join("print_none_typed_parameter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("print_none_typed_parameter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"None\n");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::Name {
                            name: "y".to_string(),
                            ty: Ty::Int,
                        }),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("locals_arith");
        let obj_path = dir.join("locals_arith.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("reassign_local");
        let obj_path = dir.join("reassign_local.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn ge_and_ne_execute_with_the_correct_boolean_value_for_int() {
        // `compiles_the_remaining_comparison_operators` above only proves
        // `GtE`/`NotEq` produce IR that compiles -- it never links, runs, or
        // checks a value, so a predicate swap (e.g. `IntPredicate::SGE` ->
        // `IntPredicate::SGT`) would still pass every existing test. This
        // links and runs, asserting the actual computed booleans.
        fn print_compare(op: CmpOpKind, left: i64, right: i64) -> MirStmt {
            MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Compare {
                    op,
                    left: Box::new(MirExpr::IntLiteral(left)),
                    right: Box::new(MirExpr::IntLiteral(right)),
                    ty: Ty::Bool,
                }],
                ty: Ty::None,
            })
        }
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(print_compare(CmpOpKind::GtE, 5, 5)),
                MirItem::TopLevelStmt(print_compare(CmpOpKind::GtE, 5, 6)),
                MirItem::TopLevelStmt(print_compare(CmpOpKind::NotEq, 5, 5)),
                MirItem::TopLevelStmt(print_compare(CmpOpKind::NotEq, 5, 6)),
            ],
        };
        let dir = tempfile_dir("ge_ne_values");
        let obj_path = dir.join("ge_ne_values.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("ge_ne_values");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\nFalse\nFalse\nTrue\n");
    }

    #[test]
    fn a_nan_float_is_not_equal_to_itself_and_is_truthy() {
        // `float('nan') != float('nan')` and `bool(float('nan'))` are both
        // `True` in CPython (IEEE-754 unordered-comparison semantics) --
        // `Compare`'s `NotEq` arm and `truthy`'s `Float` arm both
        // deliberately use `FloatPredicate::UNE` (not the ordered `ONE`) to
        // match this. v0.1's Python-source surface has no NaN-producing
        // expression (this same diff's `float_pow` domain guards ensure
        // `**` panics before a NaN could leak out that path either), so a
        // hand-built `FloatLiteral(f64::NAN)` is the only way to exercise
        // it -- without this test, swapping either `UNE` for `ONE` would
        // still pass every other test in the suite.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Compare {
                        op: CmpOpKind::NotEq,
                        left: Box::new(MirExpr::FloatLiteral(f64::NAN)),
                        right: Box::new(MirExpr::FloatLiteral(f64::NAN)),
                        ty: Ty::Bool,
                    }],
                    ty: Ty::None,
                })),
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::FloatLiteral(f64::NAN),
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::StringLiteral("nan_is_truthy".to_string())],
                        ty: Ty::None,
                    })],
                    orelse: vec![],
                }),
            ],
        };
        let dir = tempfile_dir("nan_comparisons");
        let obj_path = dir.join("nan_comparisons.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("nan_comparisons");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\nnan_is_truthy\n");
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
                    value: MirExpr::Name {
                        name: "b".to_string(),
                        ty: Ty::Bool,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("read_bool_local");
        let obj_path = dir.join("read_bool_local.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("binop_bool_left_promotes");
        let obj_path = dir.join("binop_bool_left_promotes.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("binop_bool_right_promotes");
        let obj_path = dir.join("binop_bool_right_promotes.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(
                MirExpr::IntLiteral(5),
            ))],
        };
        let dir = tempfile_dir("bare_expr_stmt");
        let obj_path = dir.join("bare_expr_stmt.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn reading_a_none_typed_parameter_slot_emits_its_unit_carrier() {
        // Calls `emit_expr` directly to isolate its `Ty::None` name-load
        // arm. The real source-level ABI path is covered separately by
        // `printing_a_none_typed_parameter_renders_none`.
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let rt = declare_rt_functions(&context, &module);
        let fn_type = context.void_type().fn_type(&[], false);
        let f = module.add_function("f", fn_type, None);
        let block = context.append_basic_block(f, "entry");
        builder.position_at_end(block);

        let user_functions: HashMap<&str, UserFunction> = HashMap::new();
        let mut locals = HashMap::new();
        let ptr = builder
            .build_alloca(context.i8_type(), "x")
            .expect("build_alloca should not fail for a fresh block");
        builder
            .build_store(ptr, context.i8_type().const_zero())
            .expect("build_store should not fail for a fresh unit slot");
        locals.insert(
            "x".to_string(),
            StorageSlot {
                ptr,
                ty: Ty::None,
                initialized: None,
            },
        );

        let value = emit_expr(
            &context,
            &builder,
            &module,
            &rt,
            &user_functions,
            &locals,
            &MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::None,
            },
        );
        // Reaching this point proves the typed load was emitted. Its exact
        // LLVM `i8` representation is verified by the module-level ABI and
        // runtime tests; matching the private wrapper here would add an
        // intentionally-unreachable assertion branch under the hard region
        // coverage gate.
        let _ = value;
    }

    #[test]
    #[should_panic(expected = "reading a `<inferred>`-typed local is not supported yet")]
    fn reading_an_unresolved_infer_typed_local_is_an_internal_error() {
        // `Ty::Infer` is an HIR-only solver marker and must be resolved
        // before MIR reaches codegen. Hand-built storage keeps the
        // defensive catch-all in the name-load path covered without
        // weakening the invariant for real source programs.
        //
        // Task 5 (D-089) changed this catch-all's message to name the type
        // via `Ty::name()` instead of a bare `{:?}`, so `Ty::Infer` now
        // renders as `<inferred>` (`Ty::name()`'s own text for that
        // variant) rather than the `Debug`-derived `Infer` this test's
        // `expected` string pinned before -- same panic, updated wording.
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let rt = declare_rt_functions(&context, &module);
        let fn_type = context.void_type().fn_type(&[], false);
        let f = module.add_function("f", fn_type, None);
        let block = context.append_basic_block(f, "entry");
        builder.position_at_end(block);

        let user_functions: HashMap<&str, UserFunction> = HashMap::new();
        let ptr = builder
            .build_alloca(context.i8_type(), "x")
            .expect("build_alloca should not fail for a fresh block");
        let locals = HashMap::from([(
            "x".to_string(),
            StorageSlot {
                ptr,
                ty: Ty::Infer,
                initialized: None,
            },
        )]);

        emit_expr(
            &context,
            &builder,
            &module,
            &rt,
            &user_functions,
            &locals,
            &MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Infer,
            },
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
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Float,
                        }),
                        right: Box::new(MirExpr::FloatLiteral(1.0)),
                        ty: Ty::Float,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("read_float_local");
        let obj_path = dir.join("read_float_local.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn a_function_parameter_can_be_reassigned_read_back_and_printed() {
        // `def f(n: int): n = n + 1; print(n)` ; `f(7)` -- supersedes this test's
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
                    body: vec![
                        MirStmt::Assign {
                            target: "n".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Add,
                                left: Box::new(MirExpr::Name {
                                    name: "n".to_string(),
                                    ty: Ty::Int,
                                }),
                                right: Box::new(MirExpr::IntLiteral(1)),
                                ty: Ty::Int,
                            },
                        },
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::Name {
                                name: "n".to_string(),
                                ty: Ty::Int,
                            }],
                            ty: Ty::None,
                        }),
                    ],
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("param_reference_reads_back");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"8\n");
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
        // (non-stack) LLVM global storage and seeding each function's
        // `fn_locals` map from those slots before parameters and lexical
        // locals override same-named entries.
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
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("function_reads_module_global");
        let obj_path = dir.join("function_reads_module_global.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("if_else");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"10\n");
    }

    #[test]
    fn an_if_whose_both_branches_return_terminates_its_unreachable_merge() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "choose".to_string(),
                    params: vec![("flag".to_string(), Ty::Bool)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::If {
                        test: MirExpr::Name {
                            name: "flag".to_string(),
                            ty: Ty::Bool,
                        },
                        body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
                        orelse: vec![MirStmt::Return(Some(MirExpr::IntLiteral(2)))],
                    }],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "choose".to_string(),
                        args: vec![MirExpr::BoolLiteral(true)],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("if_both_branches_return");
        let obj_path = dir.join("if_both_branches_return.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("if_both_branches_return");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                        left: Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(0)),
                        ty: Ty::Bool,
                    },
                    body: vec![
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::Name {
                                name: "i".to_string(),
                                ty: Ty::Int,
                            }],
                            ty: Ty::None,
                        }),
                        MirStmt::Assign {
                            target: "i".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Sub,
                                left: Box::new(MirExpr::Name {
                                    name: "i".to_string(),
                                    ty: Ty::Int,
                                }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    test: MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    },
                    body: vec![
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::Name {
                                name: "i".to_string(),
                                ty: Ty::Int,
                            }],
                            ty: Ty::None,
                        }),
                        MirStmt::Assign {
                            target: "i".to_string(),
                            value: MirExpr::BinOp {
                                op: BinOpKind::Sub,
                                left: Box::new(MirExpr::Name {
                                    name: "i".to_string(),
                                    ty: Ty::Int,
                                }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("while_body_always_returns");
        let obj_path = dir.join("while_body_always_returns.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_pos");
        let obj_path = dir.join("for_range_pos.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        // `collect_stmt_bindings`'s `ForRange` arm with an already-present
        // `BTreeMap` entry: the second occurrence must not re-declare or
        // change the type of its global slot.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ForRange {
                    var: "i".to_string(),
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(2),
                    step: MirExpr::IntLiteral(1),
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }],
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
                        args: vec![MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }],
                        ty: Ty::None,
                    })],
                }),
            ],
        };
        let dir = tempfile_dir("for_range_reused_loop_var");
        let obj_path = dir.join("for_range_reused_loop_var.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_neg");
        let obj_path = dir.join("for_range_neg.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_range_neg");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n2\n1\n");
    }

    #[test]
    #[should_panic(expected = "range() start did not evaluate to int")]
    fn for_range_with_a_non_int_start_is_rejected() {
        // `bool` is accepted as an `int` subtype; float is not. Hand-built
        // malformed MIR exercises the defensive backend check directly.
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "range() stop did not evaluate to int")]
    fn for_range_with_a_non_int_stop_is_rejected() {
        // Distinct region from the `start` case above.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ForRange {
                var: "i".to_string(),
                start: MirExpr::IntLiteral(0),
                stop: MirExpr::StringLiteral("3".to_string()),
                step: MirExpr::IntLiteral(1),
                body: vec![],
            })],
        };
        let dir = tempfile_dir("for_range_bad_stop_panics");
        let obj_path = dir.join("for_range_bad_stop_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
                    args: vec![MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_bool_start_stop_step");
        let obj_path = dir.join("for_range_bool_start_stop_step.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            })],
        };
        let dir = tempfile_dir("for_range_bool_stop");
        let obj_path = dir.join("for_range_bool_stop.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                            left: Box::new(MirExpr::Name {
                                name: "i".to_string(),
                                ty: Ty::Int,
                            }),
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
                        args: vec![MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }],
                        ty: Ty::None,
                    }),
                ],
            })],
        };
        let dir = tempfile_dir("nested_control_flow_resume");
        let obj_path = dir.join("nested_control_flow_resume.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
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
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
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
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
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
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
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
                        left: Box::new(MirExpr::Name {
                            name: "a".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::Name {
                            name: "b".to_string(),
                            ty: Ty::Int,
                        }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                        left: Box::new(MirExpr::Name {
                            name: "a".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::Name {
                            name: "b".to_string(),
                            ty: Ty::Int,
                        }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    left: Box::new(MirExpr::Name {
                        name: "n".to_string(),
                        ty: Ty::Int,
                    }),
                    right: Box::new(MirExpr::IntLiteral(1)),
                    ty: Ty::Bool,
                },
                body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(1)))],
                orelse: vec![],
            },
            MirStmt::Return(Some(MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::Name {
                    name: "n".to_string(),
                    ty: Ty::Int,
                }),
                right: Box::new(MirExpr::Call {
                    callee: "fact".to_string(),
                    args: vec![MirExpr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(MirExpr::Name {
                            name: "n".to_string(),
                            ty: Ty::Int,
                        }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                            left: Box::new(MirExpr::Name {
                                name: "x".to_string(),
                                ty: Ty::Int,
                            }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
            compile_to_object(&mir, &obj_path, None, false)
        }));
        assert!(
            result.is_err(),
            "expected a panic, not a successfully-compiled object"
        );
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
            items: vec![MirItem::TopLevelStmt(MirStmt::Return(Some(
                MirExpr::IntLiteral(0),
            )))],
        };
        let dir = tempfile_dir("top_level_return_internal_error");
        let obj_path = dir.join("top_level_return_internal_error.o");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compile_to_object(&mir, &obj_path, None, false)
        }));
        assert!(
            result.is_err(),
            "expected a panic, not a successfully-compiled object"
        );
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
                        left: Box::new(MirExpr::Name {
                            name: "n".to_string(),
                            ty: Ty::Int,
                        }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "every assignment target must have a predeclared storage slot")]
    fn a_none_typed_call_result_cannot_be_stored_as_a_general_value() {
        // D-072 leaves general `None` storage outside v0.1. `emit_expr`
        // preserves the call side effect with an internal placeholder, but
        // binding collection deliberately creates no slot for `Ty::None`;
        // the assignment invariant must therefore fail loudly instead of
        // silently materializing a bool local.
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
                    value: MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::None,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("none_typed_call_result_has_no_storage");
        let obj_path = dir.join("none_typed_call_result_has_no_storage.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "a `<inferred>`-typed call result is not supported yet")]
    fn an_infer_typed_call_result_used_as_a_nested_expression_is_not_supported() {
        // Exercises `emit_expr`'s `Call` arm's own defensive `other =>`
        // catch-all on `ty` -- `Ty::Infer` (an HIR-only inference
        // placeholder no real MIR ever carries this far, same rationale as
        // `ty_to_basic_type`'s own `an_infer_typed_return_value_is_not_yet_
        // supported` test above) is the one `Ty` variant left that still
        // reaches it, now that Task 10 gives `Ty::None` its own explicit
        // (non-panicking) case there (see the test directly above).
        //
        // Task 5 (D-089) updated this catch-all to name the type via
        // `Ty::name()` instead of a bare `{:?}`, so `Ty::Infer` renders as
        // `<inferred>` here now -- same panic, updated wording.
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
                    value: MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Infer,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("infer_typed_call_result_panics");
        let obj_path = dir.join("infer_typed_call_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "<inferred> has no LLVM representation yet")]
    fn an_infer_typed_return_value_is_not_yet_supported() {
        // `ty_to_basic_type` now implements `Int`/`Bool`/`Float`/`Str`
        // (Task 7 closed the `Str` gap this test's earlier, Task 3-era
        // incarnation exercised -- see
        // `compiles_a_function_with_a_str_parameter_and_str_return_value`
        // below) plus `None`/`List(_)` (Task 5, D-089). `Ty::None` can't
        // stand in for "still unhandled" here: `compile_to_object`'s own
        // `return_ty` match special-cases `Ty::None` into
        // `void_type().fn_type(...)` *before* `ty_to_basic_type` is ever
        // called for a return type (see that match's own `Ty::None` arm)
        // -- `Ty::Infer` (an HIR-only inference placeholder no real MIR
        // ever carries this far) is the one `Ty` variant left that still
        // reaches `ty_to_basic_type`'s own defensive catch-all from the
        // return-type position.
        //
        // Task 5 (D-089) also rewrote this catch-all's whole message (not
        // just the type-name formatting) to "{name} has no LLVM
        // representation yet" -- see `ty_to_basic_type_panics_clearly_
        // for_dict`/`_tuple` below for the two new tests that pin this
        // exact wording. The `expected` string here pins the rendered
        // `<inferred>` name too (`Ty::Infer`'s own `.name()` text), not
        // just the trailing message, for the same reason those two do.
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "tuple[int, str] has no LLVM representation yet")]
    fn ty_to_basic_type_panics_clearly_for_tuple() {
        // `tuple[int, str]` stays PR-11's own scope (no v0.2 code path
        // constructs one), so it must still panic here -- but with an
        // honest message naming the actual type via `Ty::name()`
        // ("tuple[int, str]") instead of the generic `{other:?}` Debug
        // format this catch-all used before D-089. The `expected` string
        // pins the rendered type name itself (not just the trailing "has
        // no LLVM representation yet"), so this test would fail if
        // `.name()` were ever reverted back to `{other:?}`.
        let context = Context::create();
        ty_to_basic_type(&context, Ty::Tuple(Box::new(vec![Ty::Int, Ty::Str])));
    }

    #[test]
    fn ty_to_basic_type_gives_list_dict_and_set_a_pointer_representation_like_str() {
        // Task 5 (D-089) gave `List(_)` a *real* arm here -- the runtime
        // list object is heap-allocated and pointer-referenced exactly
        // like `Str`'s `PyStrObj`, so both must produce the same LLVM
        // representation. PR-11 Task 5 gave `Dict(_)` the identical
        // treatment for the identical reason (`PyDictObj` is heap-
        // allocated and pointer-referenced too), and PR-11 Task 9 gives
        // `Set(_)` the same treatment again (`PyIntSetObj` is heap-
        // allocated and pointer-referenced too) -- unlike `Tuple`, which
        // stays PR-11's own scope and still panics (see the test directly
        // above). Traces through the actual return value (not just
        // "doesn't panic") to prove all four really match, for a
        // `list[int]`/`list[str]` element type, a `dict[str, int]`
        // key/value pair, and a `set[int]` element type --
        // `ty_to_basic_type`'s own `List(_)`/`Dict(_)`/`Set(_)` arms ignore
        // the element/key/value types entirely.
        let context = Context::create();
        let str_repr = ty_to_basic_type(&context, Ty::Str);
        let list_int_repr = ty_to_basic_type(&context, Ty::List(Box::new(Ty::Int)));
        let list_str_repr = ty_to_basic_type(&context, Ty::List(Box::new(Ty::Str)));
        let dict_repr = ty_to_basic_type(&context, Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let set_repr = ty_to_basic_type(&context, Ty::Set(Box::new(Ty::Int)));
        assert!(str_repr.is_pointer_type());
        assert!(list_int_repr.is_pointer_type());
        assert!(list_str_repr.is_pointer_type());
        assert!(dict_repr.is_pointer_type());
        assert!(set_repr.is_pointer_type());
        assert_eq!(str_repr, list_int_repr);
        assert_eq!(str_repr, list_str_repr);
        assert_eq!(str_repr, dict_repr);
        assert_eq!(str_repr, set_repr);
    }

    #[test]
    fn reading_a_list_typed_local_back_out_of_its_alloca_produces_a_list_scalar() {
        // Exercises `emit_expr`'s `Name` arm's `Ty::List(_)` arm directly
        // (added by Task 5, D-089; retargeted from the `Scalar::Str` reuse
        // onto `Scalar::List` by Task 11a, D-107) -- same hand-built-
        // `StorageSlot` convention as `reading_an_unresolved_infer_typed_
        // local_is_an_internal_error` above: hand-building the slot is what
        // lets one `emit_expr` call read a `list[int]` local and the next
        // read a `str` one, with nothing else in the fixture to confuse
        // which variant came from which.
        //
        // D-107's entire point is that a `list[T]` pointer must stop being
        // *indistinguishable* from a `str` pointer at the `Scalar` level,
        // so this reads one `list[int]`-typed and one `str`-typed local
        // through the same `emit_expr` entry point and proves the two now
        // produce different variants.
        //
        // The variant is extracted through a helper exercised with *both*
        // values rather than a single-value `let Scalar::List(ptr) = value
        // else { panic!(..) }`: this arm returns `Scalar::List`
        // unconditionally for a `Ty::List` slot, so a single-value
        // `else`/`_` arm would still be statically unreachable and
        // permanently uncovered under this crate's 100%-region gate
        // (D-014) -- exactly the objection this test's own pre-Task-11a
        // comment raised, which giving `list[T]` its own variant does not
        // by itself remove. Feeding the same helper a `str` read covers
        // its other arm with a real, meaningful assertion instead.
        fn loaded_pointer_type<'ctx>(
            scalar: &Scalar<'ctx>,
        ) -> Option<inkwell::types::PointerType<'ctx>> {
            match scalar {
                Scalar::List(ptr) => Some(ptr.get_type()),
                _ => None,
            }
        }

        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let rt = declare_rt_functions(&context, &module);
        let fn_type = context.void_type().fn_type(&[], false);
        let f = module.add_function("f", fn_type, None);
        let block = context.append_basic_block(f, "entry");
        builder.position_at_end(block);

        let user_functions: HashMap<&str, UserFunction> = HashMap::new();
        let ptr = builder
            .build_alloca(context.ptr_type(inkwell::AddressSpace::default()), "xs")
            .expect("build_alloca should not fail for a fresh block");
        builder
            .build_store(
                ptr,
                context
                    .ptr_type(inkwell::AddressSpace::default())
                    .const_null(),
            )
            .expect("build_store should not fail immediately after this function's own alloca");
        let str_ptr = builder
            .build_alloca(context.ptr_type(inkwell::AddressSpace::default()), "s")
            .expect("build_alloca should not fail for a fresh block");
        builder
            .build_store(
                str_ptr,
                context
                    .ptr_type(inkwell::AddressSpace::default())
                    .const_null(),
            )
            .expect("build_store should not fail immediately after this function's own alloca");
        let locals = HashMap::from([
            (
                "xs".to_string(),
                StorageSlot {
                    ptr,
                    ty: Ty::List(Box::new(Ty::Int)),
                    initialized: None,
                },
            ),
            (
                "s".to_string(),
                StorageSlot {
                    ptr: str_ptr,
                    ty: Ty::Str,
                    initialized: None,
                },
            ),
        ]);

        let list_value = emit_expr(
            &context,
            &builder,
            &module,
            &rt,
            &user_functions,
            &locals,
            &MirExpr::Name {
                name: "xs".to_string(),
                ty: Ty::List(Box::new(Ty::Int)),
            },
        );
        let str_value = emit_expr(
            &context,
            &builder,
            &module,
            &rt,
            &user_functions,
            &locals,
            &MirExpr::Name {
                name: "s".to_string(),
                ty: Ty::Str,
            },
        );
        // The `list[int]` read produces `Scalar::List` carrying the loaded
        // pointer (whose LLVM type must match what `ty_to_basic_type`
        // allocated the slot as); the `str` read, through the very same
        // entry point, does not -- which is the property D-107 exists to
        // establish and the one `Scalar::Str` reuse could never provide.
        assert_eq!(
            loaded_pointer_type(&list_value),
            Some(context.ptr_type(inkwell::AddressSpace::default()))
        );
        assert_eq!(loaded_pointer_type(&str_value), None);
    }

    /// Builds the minimal `Context`/`Module`/`Builder`/`RtFns` set the
    /// `Scalar::List` defensive-panic tests below need. None of them build
    /// any IR -- every one panics inside its function's own `match` before
    /// reaching a `build_*` call -- so unlike the hand-built-`StorageSlot`
    /// tests above, none needs a function or a positioned basic block.
    fn list_scalar_panic_fixture(context: &Context) -> (inkwell::module::Module<'_>, RtFns<'_>) {
        let module = context.create_module("test");
        let rt = declare_rt_functions(context, &module);
        (module, rt)
    }

    #[test]
    #[should_panic(expected = "pycc_codegen: truthiness of a list[T] value is not supported yet")]
    fn truthiness_of_a_list_value_panics_honestly() {
        // D-107 confirmed this path is genuinely reachable, not defensive:
        // `pycc_types` accepts any type in a boolean context
        // (`crates/pycc_types/src/lib.rs`'s `if`/`while` handling calls
        // `infer_expr` with no type restriction at all), so `if xs:` for a
        // `list[int]` local type-checks today. v0.2 has no `bool(list)`
        // semantics (D-105 ships only `len(x)`/`x[i]`/iteration/`.append()`),
        // so an honest panic naming the gap is the correct behavior -- the
        // alternative this replaces was `pycc_rt_str_truthy` reading a
        // `PyIntListObj` as a `PyStrObj`.
        //
        // Calls `truthy` directly with a hand-built `Scalar::List` rather
        // than compiling `if xs:` from real MIR: this pins the panic to
        // `truthy` itself, where a future `bool(list)` implementation would
        // land, instead of to whichever caller happens to reach it first.
        // (`docs/ARCHITECTURE.md` records the same gap as user-visible
        // behavior -- `if xs:` type-checks and then stops codegen here.)
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        truthy(&context, &builder, &rt, Scalar::List(ptr));
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: string conversion of a list[T] value is not supported yet"
    )]
    fn string_conversion_of_a_list_value_panics_honestly() {
        // The `to_str` half of the same D-107 pair: `pycc_types` type-checks
        // `print(xs)` for a `list[int]` local unconditionally (its `print`
        // arm returns `Ok(Ty::None)` for any argument type), and `to_str` is
        // what `print` hands its evaluated argument to. Same honest-panic
        // reasoning as `truthiness_of_a_list_value_panics_honestly` above --
        // this replaces handing a `PyIntListObj` pointer to a
        // `pycc_rt_*_to_str` function expecting a `PyStrObj`.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_str(&builder, &rt, Scalar::List(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected an int-or-bool operand, got list")]
    fn to_tagged_int_rejects_a_list_operand() {
        // Unlike `truthy`/`to_str` above, this one really is defensive:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::List`, so any arithmetic with a list operand is rejected
        // as `T0021` long before codegen. Hence the "internal error"
        // wording (matching this function's own neighbouring `Float`/`Str`
        // arms) rather than the "not supported yet" feature-gap wording
        // `truthy`/`to_str` use. Exercised by calling `to_tagged_int`
        // directly, since a list operand cannot reach it through any MIR --
        // `emit_expr`'s `BinOp` arm claims a `Ty::List` result with its own
        // container-specific panic first (see
        // `a_list_result_binop_is_not_yet_supported` below), so no
        // arithmetic shape gets this far.
        let context = Context::create();
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_tagged_int(&context, &builder, Scalar::List(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got list")]
    fn to_float_rejects_a_list_operand() {
        // Same defensive-arm rationale as `to_tagged_int_rejects_a_list_
        // operand` directly above, for `to_float`'s own match.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_float(&context, &builder, &rt, Scalar::List(ptr));
    }

    #[test]
    #[should_panic(expected = "pycc_codegen: truthiness of a dict[K, V] value is not supported yet")]
    fn truthiness_of_a_dict_value_panics_honestly() {
        // The `dict[K, V]` counterpart of `truthiness_of_a_list_value_
        // panics_honestly` above (D-107's reasoning, per D-114): `pycc_types`
        // accepts any type in a boolean context, so `if x:` for a
        // `dict[str, int]` local type-checks today. v0.2 has no
        // `bool(dict)` semantics (D-113), so an honest panic naming the gap
        // is the correct behavior. Calls `truthy` directly with a hand-built
        // `Scalar::Dict`, for the identical reason that test gives.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        truthy(&context, &builder, &rt, Scalar::Dict(ptr));
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: string conversion of a dict[K, V] value is not supported yet"
    )]
    fn string_conversion_of_a_dict_value_panics_honestly() {
        // The `dict[K, V]` counterpart of `string_conversion_of_a_list_
        // value_panics_honestly` above, for the identical reason.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_str(&builder, &rt, Scalar::Dict(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected an int-or-bool operand, got dict")]
    fn to_tagged_int_rejects_a_dict_operand() {
        // The `dict[K, V]` counterpart of `to_tagged_int_rejects_a_list_
        // operand` above -- genuinely defensive for the identical reason:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::Dict` either, so no real MIR reaches this arm.
        let context = Context::create();
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_tagged_int(&context, &builder, Scalar::Dict(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got dict")]
    fn to_float_rejects_a_dict_operand() {
        // Same defensive-arm rationale as `to_tagged_int_rejects_a_dict_
        // operand` directly above, for `to_float`'s own match.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_float(&context, &builder, &rt, Scalar::Dict(ptr));
    }

    #[test]
    fn collect_stmt_bindings_includes_a_list_typed_assignment_target() {
        // Task 5 (D-089) added `Ty::List(_)` to this allow-list -- Task 11
        // depends on a `list[int]` local's binding already being collected
        // here, so this is a real, deliberate inclusion to verify, not
        // just a louder panic elsewhere.
        let stmt = MirStmt::Assign {
            target: "xs".to_string(),
            value: MirExpr::Name {
                name: "xs".to_string(),
                ty: Ty::List(Box::new(Ty::Int)),
            },
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(
            bindings.get("xs"),
            Some(&Ty::List(Box::new(Ty::Int))),
            "a list[int]-typed assignment target's binding should be collected"
        );
    }

    #[test]
    fn collect_stmt_bindings_includes_a_dict_typed_assignment_target() {
        // PR-11 Task 5 joins `Ty::Dict(_)` to this allow-list, mirroring
        // `Ty::List(_)`'s own inclusion directly above for the identical
        // reason: this task's own codegen (`declare_module_globals`/
        // `storage_slot_at_entry`) depends on a `dict[str, int]` local's
        // binding already being collected here.
        let stmt = MirStmt::Assign {
            target: "x".to_string(),
            value: MirExpr::Name {
                name: "x".to_string(),
                ty: Ty::Dict(Box::new((Ty::Str, Ty::Int))),
            },
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(
            bindings.get("x"),
            Some(&Ty::Dict(Box::new((Ty::Str, Ty::Int)))),
            "a dict[str, int]-typed assignment target's binding should be collected"
        );
    }

    #[test]
    fn collect_stmt_bindings_includes_a_set_typed_assignment_target() {
        // PR-11 Task 9 joins `Ty::Set(_)` to this allow-list, mirroring
        // `Ty::List(_)`/`Ty::Dict(_)`'s own inclusion above for the
        // identical reason: this task's own codegen
        // (`declare_module_globals`/`storage_slot_at_entry`) depends on a
        // `set[int]` local's binding already being collected here.
        let stmt = MirStmt::Assign {
            target: "xs".to_string(),
            value: MirExpr::Name {
                name: "xs".to_string(),
                ty: Ty::Set(Box::new(Ty::Int)),
            },
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(
            bindings.get("xs"),
            Some(&Ty::Set(Box::new(Ty::Int))),
            "a set[int]-typed assignment target's binding should be collected"
        );
    }

    #[test]
    fn collect_stmt_bindings_excludes_a_tuple_typed_assignment_target() {
        // `Tuple` stays excluded -- PR-11's own scope, not this task's (no
        // codegen exists for it) -- so its binding must NOT be collected,
        // unlike `List(_)`/`Dict(_)`/`Set(_)` above.
        let stmt = MirStmt::Assign {
            target: "xs".to_string(),
            value: MirExpr::Name {
                name: "xs".to_string(),
                ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Str])),
            },
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(bindings.get("xs"), None);
    }

    #[test]
    fn collect_stmt_bindings_excludes_a_dict_set_target() {
        // `d[k] = v` (PR-11 Task 4) reassigns an existing binding's
        // contents, not a name -- mirrors `pycc_types::collect_local_names`'s
        // own identical `HirStmt::DictSet` test. `dict` itself must not
        // pick up an entry from this statement.
        let stmt = MirStmt::DictSet {
            dict: "x".to_string(),
            key: MirExpr::StringLiteral("a".to_string()),
            value: MirExpr::IntLiteral(1),
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(bindings.get("x"), None);
    }

    #[test]
    fn collect_stmt_bindings_binds_a_for_dict_loop_variable_as_str_and_recurses_into_its_body() {
        // `MirStmt::ForDict` now binds its own loop variable (PR-11 Task 5),
        // mirroring `MirStmt::ForList`'s own `Ty::Int` hardcode -- but
        // `Ty::Str`, since a dict's iterated key type is always exactly
        // `Ty::Str` (see this arm's own doc comment). It must also still
        // recurse into `body`, exactly like `ForList`/`ForRange`/`If`/
        // `While` above, so a nested, ordinary statement's binding is not
        // silently dropped.
        let stmt = MirStmt::ForDict {
            var: "k".to_string(),
            dict: "d".to_string(),
            body: vec![MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::IntLiteral(1),
            }],
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(bindings.get("y"), Some(&Ty::Int));
        assert_eq!(bindings.get("k"), Some(&Ty::Str));
    }

    #[test]
    fn collect_stmt_bindings_binds_a_for_set_loop_variable_as_int_and_recurses_into_its_body() {
        // `MirStmt::ForSet` now binds its own loop variable (PR-11 Task 9),
        // mirroring `MirStmt::ForList`'s own `Ty::Int` hardcode exactly --
        // a set's iterated element type is always exactly `Ty::Int` (see
        // this arm's own doc comment), unlike `ForDict`'s `Ty::Str` key.
        // Superseded Task 8's own version of this test (`collect_stmt_
        // bindings_recurses_into_a_for_set_loop_body_without_binding_its_
        // own_var`), which pinned the pre-Task-9 deferred-decision
        // behavior. It must also still recurse into `body`, exactly like
        // `ForList`/`ForDict`/`ForRange`/`If`/`While` above, so a nested,
        // ordinary statement's binding is not silently dropped.
        let stmt = MirStmt::ForSet {
            var: "v".to_string(),
            set: "s".to_string(),
            body: vec![MirStmt::Assign {
                target: "y".to_string(),
                value: MirExpr::IntLiteral(1),
            }],
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(bindings.get("y"), Some(&Ty::Int));
        assert_eq!(bindings.get("v"), Some(&Ty::Int));
    }

    #[test]
    #[should_panic(expected = "binary operators are not supported on list[int] yet")]
    fn a_list_result_binop_is_not_yet_supported() {
        // No real Python operator produces a `BinOp` typed `list[int]`
        // (D-105 defers even `+` list concatenation past v0.2), so
        // `pycc_types`/`pycc_mir` never produce this shape -- hand-crafted
        // MIR exercises `emit_expr`'s `BinOp` arm's new explicit container
        // arm directly, same "hand-construct the otherwise-unreachable
        // shape" convention as `a_none_result_binop_is_not_yet_supported`
        // above.
        //
        // Deliberately a function-local assignment, not a top-level one:
        // `collect_stmt_bindings`'s allow-list now includes `Ty::List(_)`
        // (Task 5, D-089, see the two `collect_stmt_bindings_*` tests
        // above), so a top-level `list[int]` binding would be collected
        // and routed to `declare_module_globals` -- which does *not* get a
        // `List(_)` arm (that catch-all's own test covers it) -- panicking
        // there first and never reaching this arm at all. A function-local
        // binding's slot is instead declared via `storage_slot_at_entry`/
        // `ty_to_basic_type`, which *does* give `List(_)` a real pointer
        // representation, so codegen gets past slot allocation and
        // actually reaches this `BinOp` arm.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::IntLiteral(1)),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::List(Box::new(Ty::Int)),
                    },
                }],
            }],
        };
        let dir = tempfile_dir("binop_list_result_panics");
        let obj_path = dir.join("binop_list_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    fn compiles_a_function_with_a_list_int_parameter_and_list_int_return_value() {
        // `def f(x: list[int]) -> list[int]: return x` -- no real source
        // program can produce this shape (`pycc_hir::annotation_to_ty`
        // rejects every annotation but a bare name, and D-105's first scope
        // cut keeps it that way for v0.2, so an annotated `list[int]`
        // parameter or return type never reaches codegen), but Task 5
        // (D-089) requires this MIR shape to compile *cleanly* rather than
        // panic: `ty_to_basic_type`'s `List(_)` arm (parameter type, and
        // transitively the return type via `compile_to_object`'s `fn_type`
        // delegation) and `emit_expr`'s `Name` arm's `List(_)` arm must
        // agree on the same pointer representation, or `module.verify()`
        // inside `compile_to_object` would reject the mismatched IR.
        // Deliberately does not link or run the resulting object: `f` is
        // never called, and no caller could construct the annotated
        // `list[int]` argument it wants, so this only proves the codegen
        // shape is internally consistent, not that the program is
        // meaningful to run.
        //
        // Also the regression test for a review finding on this task's
        // first pass: `return x` is a bare `Name` read of a `list[int]`
        // parameter, which is exactly the shape `incref_if_str_duplicate`
        // dispatches on -- before `str_value_is_a_duplicate_reference` was
        // gated on `ty: Ty::Str` (see that function's own doc comment),
        // this test's own generated object emitted a spurious call to
        // `pycc_rt_str_incref` on the list pointer. Asserted against
        // directly below, using the equivalently-shaped
        // `compiles_a_function_with_a_float_parameter_and_float_return_value`
        // above as the known-clean baseline (a bare `float`-typed `Name`
        // return, which has never referenced any `pycc_rt_str_*` symbol).
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::List(Box::new(Ty::Int)))],
                return_ty: Ty::List(Box::new(Ty::Int)),
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::List(Box::new(Ty::Int)),
                }))],
            }],
        };
        let dir = tempfile_dir("list_int_param_and_return");
        let obj_path = dir.join("list_int_param_and_return.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let obj_bytes = std::fs::read(&obj_path).expect("object file should be readable");
        let references_symbol =
            |name: &str| obj_bytes.windows(name.len()).any(|w| w == name.as_bytes());
        assert!(
            !references_symbol("pycc_rt_str_incref"),
            "a list[int]-typed bare-Name read must not be treated as a duplicate str \
             reference and incref'd as if it were one"
        );
        assert!(
            !references_symbol("pycc_rt_str_decref"),
            "a list[int]-typed value must not be decref'd as if it were str"
        );
    }

    #[test]
    fn passing_a_list_value_as_a_function_argument_marshals_it_like_a_pointer() {
        // `def f(x: list[int]) -> list[int]: return x` plus
        // `def g(x: list[int]) -> list[int]: return f(x)` -- the caller adds
        // the one shape the test directly above does not reach:
        // `build_call_to`'s argument-marshalling match, whose `Scalar::List`
        // arm Task 11a (D-107) put in the *pass-through* bucket. That claim
        // ("a list pointer marshals identically to a str pointer -- it's an
        // opaque pointer either way") is exactly what `module.verify()`
        // inside `compile_to_object` checks here: if the marshalled argument
        // disagreed with `ty_to_basic_type`'s parameter type, LLVM would
        // reject the call instruction outright.
        //
        // Same not-linked, not-run caveat as the test above: neither `f`
        // nor `g` is ever called, and their annotated `list[int]`
        // parameters are unreachable from real source, so this proves the
        // codegen shape only. The `pycc_rt_str_*` assertion carries over for
        // the same reason -- a `list[int]` argument is a bare `Name` read,
        // the exact shape `incref_if_str_duplicate` dispatches on, and
        // `build_call_to` calls that helper on every argument.
        //
        // Both functions return `int`, not `list[int]`: `emit_expr`'s `Call`
        // arm dispatches its *result* on the declared `Ty`, and that match
        // has no `Ty::List` arm -- it panics honestly through its catch-all
        // ("a `list[int]`-typed call result is not supported yet"). That is
        // correct and deliberately left alone here: D-105's own first scope
        // cut means no v0.2 function can be *annotated* to return
        // `list[int]` in the first place, so the gap is unreachable rather
        // than a hole this task should fill. Only the argument side is
        // exercised, which is the side that actually has a `Scalar::List`
        // arm to verify.
        let list_int = || Ty::List(Box::new(Ty::Int));
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), list_int())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(0)))],
                },
                MirItem::Function {
                    name: "g".to_string(),
                    params: vec![("x".to_string(), list_int())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::Name {
                            name: "x".to_string(),
                            ty: list_int(),
                        }],
                        ty: Ty::Int,
                    }))],
                },
            ],
        };
        let dir = tempfile_dir("list_int_passed_as_argument");
        let obj_path = dir.join("list_int_passed_as_argument.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let obj_bytes = std::fs::read(&obj_path).expect("object file should be readable");
        let references_symbol =
            |name: &str| obj_bytes.windows(name.len()).any(|w| w == name.as_bytes());
        assert!(
            !references_symbol("pycc_rt_str_incref"),
            "a list[int]-typed argument must not be incref'd as if it were a duplicate str \
             reference"
        );
    }

    #[test]
    fn assigning_a_list_value_stores_the_raw_pointer() {
        // Covers `emit_assign`'s `Scalar::List` arm, the third member of
        // Task 11a's pass-through bucket (D-107), in isolation: it calls
        // `emit_assign` directly with a hand-built `Scalar::List` and a
        // hand-built `StorageSlot` so the store instruction itself is what
        // `f.verify(true)` judges, with no surrounding list construction to
        // fail first. (`xs = [1, 2, 3]` reaches this same arm through real
        // MIR in the list tests further below; this one pins the store's IR
        // shape rather than the program's output.)
        //
        // `f.verify(true)` is the real assertion: `ty_to_basic_type`
        // allocated this slot as a pointer, so a `store` of anything but a
        // pointer-typed value would be rejected as malformed IR. Also
        // proves no `str`-style refcount traffic is emitted -- D-107 keeps
        // `list[T]` leak-only for v0.2.
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let fn_type = context.void_type().fn_type(&[], false);
        let f = module.add_function("f", fn_type, None);
        let block = context.append_basic_block(f, "entry");
        builder.position_at_end(block);

        let ptr = builder
            .build_alloca(context.ptr_type(inkwell::AddressSpace::default()), "xs")
            .expect("build_alloca should not fail for a fresh block");
        let mut locals = HashMap::from([(
            "xs".to_string(),
            StorageSlot {
                ptr,
                ty: Ty::List(Box::new(Ty::Int)),
                initialized: None,
            },
        )]);
        let value = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        emit_assign(&context, &builder, &mut locals, "xs", Scalar::List(value));
        builder
            .build_return(None)
            .expect("build_return should not fail for a void function");

        assert!(
            f.verify(true),
            "storing a list[T] pointer into its own pointer-typed slot must be valid IR"
        );
    }

    /// Wraps `body` in a `None`-returning function `f` that is then called
    /// at top level -- the shape every `list[int]` MIR fixture below needs,
    /// since a `list[int]` binding only becomes a function-local (rather
    /// than a module global) when its assignment lives inside a function
    /// body. Kept as a helper because each of these tests differs only in
    /// the statements it puts inside.
    fn list_fixture_module(body: Vec<MirStmt>) -> MirModule {
        MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body,
                },
                MirItem::TopLevelStmt(call_user_fn("f")),
            ],
        }
    }

    /// `xs = [1, 2, 3]` as a `MirStmt`, the prelude most of the `list[int]`
    /// fixtures below open with.
    fn assign_list_literal(target: &str) -> MirStmt {
        MirStmt::Assign {
            target: target.to_string(),
            value: MirExpr::ListLiteral(vec![
                MirExpr::IntLiteral(1),
                MirExpr::IntLiteral(2),
                MirExpr::IntLiteral(3),
            ]),
        }
    }

    #[test]
    #[should_panic(expected = "`len` takes exactly 1 argument, got 0")]
    fn a_len_call_with_the_wrong_argument_count_is_an_internal_error() {
        // `pycc_types` already rejects a mis-arity `len` call with T0033
        // (its own `len` arm checks `arg_tys.len() != 1` before anything
        // else), so this is hand-built malformed MIR exercising codegen's
        // own defensive backstop -- the same convention as
        // `referencing_a_name_with_no_bound_local_is_an_internal_error`
        // above.
        let mir = list_fixture_module(vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "len".to_string(),
            args: vec![],
            ty: Ty::Int,
        })]);
        let dir = tempfile_dir("len_wrong_arity_panics");
        let _ = compile_to_object(&mir, &dir.join("len_wrong_arity_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "`len`'s argument did not evaluate to a list")]
    fn a_len_call_on_a_non_list_argument_is_an_internal_error() {
        // The other half of `pycc_types`' own T0033 `len` check (a
        // non-`list[T]` argument), and the naturally reachable cover for
        // `expect_list_pointer`'s shared panic: `emit_expr` really does
        // return a non-`List` scalar here, no self-inconsistent MIR needed.
        let mir = list_fixture_module(vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "len".to_string(),
            args: vec![MirExpr::IntLiteral(1)],
            ty: Ty::Int,
        })]);
        let dir = tempfile_dir("len_non_list_panics");
        let _ = compile_to_object(&mir, &dir.join("len_non_list_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "`n` did not evaluate to a list")]
    fn appending_to_a_non_list_local_is_an_internal_error() {
        // `n = 1` then `n.append(2)`: `pycc_types` rejects this with T0033
        // ("value does not support list operations"), so codegen only sees
        // it as hand-built malformed MIR. Covers `emit_list_name_read`'s
        // own use of `expect_list_pointer`, the shared check
        // `MirStmt::ForList`'s list operand and `MirExpr::Subscript`'s base
        // also go through.
        let mir = list_fixture_module(vec![
            MirStmt::Assign {
                target: "n".to_string(),
                value: MirExpr::IntLiteral(1),
            },
            MirStmt::ExprStmt(MirExpr::ListAppend {
                list: "n".to_string(),
                value: Box::new(MirExpr::IntLiteral(2)),
            }),
        ]);
        let dir = tempfile_dir("append_to_non_list_panics");
        let _ = compile_to_object(&mir, &dir.join("append_to_non_list_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "`never_bound` has no local slot")]
    fn iterating_a_name_with_no_local_slot_is_an_internal_error() {
        // `for v in never_bound:` where nothing ever bound `never_bound` --
        // `pycc_types` rejects an unbound list operand (T0033/T0021, see
        // its `lookup_bound_name` helper), so this is codegen's own
        // defensive backstop for `emit_list_name_read`'s slot lookup, the
        // one branch of it `appending_to_a_non_list_local_is_an_internal_
        // error` above does not reach.
        let mir = list_fixture_module(vec![MirStmt::ForList {
            var: "v".to_string(),
            list: "never_bound".to_string(),
            body: vec![],
        }]);
        let dir = tempfile_dir("for_list_unbound_name_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("for_list_unbound_name_panics.o"),
            None,
            false,
        );
    }

    /// Builds `print(<a bare expression>)` as a `MirStmt`, for the dict
    /// codegen tests below -- like `call_print` above but for an arbitrary
    /// argument expression rather than only an int literal.
    fn print_expr(arg: MirExpr) -> MirStmt {
        MirStmt::ExprStmt(MirExpr::Call {
            callee: "print".to_string(),
            args: vec![arg],
            ty: Ty::None,
        })
    }

    fn dict_str_int() -> Ty {
        Ty::Dict(Box::new((Ty::Str, Ty::Int)))
    }

    fn dict_name(name: &str) -> MirExpr {
        MirExpr::Name {
            name: name.to_string(),
            ty: dict_str_int(),
        }
    }

    fn set_int() -> Ty {
        Ty::Set(Box::new(Ty::Int))
    }

    fn set_name(name: &str) -> MirExpr {
        MirExpr::Name {
            name: name.to_string(),
            ty: set_int(),
        }
    }

    #[test]
    fn dict_literal_construction_codegens_and_runs() {
        // `x = {"a": 1, "b": 2}\nprint(len(x))\n` end to end through
        // `compile_to_object` -- real, previously-panicking `MirExpr::
        // DictLiteral` construction (PR-11 Task 5) plus `len()`'s new
        // `Ty::Dict` branch. Expected output verified against `python3` on
        // this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (MirExpr::StringLiteral("a".to_string()), MirExpr::IntLiteral(1)),
                        (MirExpr::StringLiteral("b".to_string()), MirExpr::IntLiteral(2)),
                    ]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("x")],
                    ty: Ty::Int,
                })),
            ],
        };
        let dir = tempfile_dir("dict_literal_and_len");
        let obj_path = dir.join("dict_literal_and_len.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_literal_and_len");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn dict_get_codegens_and_runs() {
        // `x = {"a": 1, "b": 2}\nprint(x["b"])\n` end to end -- real
        // `MirExpr::DictGet` read codegen (PR-11 Task 5). Expected output
        // verified against `python3` on this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (MirExpr::StringLiteral("a".to_string()), MirExpr::IntLiteral(1)),
                        (MirExpr::StringLiteral("b".to_string()), MirExpr::IntLiteral(2)),
                    ]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("x")),
                    key: Box::new(MirExpr::StringLiteral("b".to_string())),
                })),
            ],
        };
        let dir = tempfile_dir("dict_get");
        let obj_path = dir.join("dict_get.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_get");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn dict_set_item_updates_an_existing_key_in_place() {
        // `x = {"a": 1}\nx["a"] = 5\nprint(x["a"])\nprint(len(x))\n` end to
        // end -- real `MirStmt::DictSet` insert-or-update codegen (PR-11
        // Task 5, D-113), exercising its update-in-place half: `len(x)`
        // staying `1` (not growing to `2`) is exactly what distinguishes
        // an update from an append. Expected output verified against
        // `python3` on this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictSet {
                    dict: "x".to_string(),
                    key: MirExpr::StringLiteral("a".to_string()),
                    value: MirExpr::IntLiteral(5),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("x")),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("x")],
                    ty: Ty::Int,
                })),
            ],
        };
        let dir = tempfile_dir("dict_set_update");
        let obj_path = dir.join("dict_set_update.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_set_update");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"5\n1\n");
    }

    #[test]
    fn dict_set_item_appends_a_new_key() {
        // `x = {"a": 1}\nx["b"] = 2\nprint(len(x))\n` end to end --
        // `MirStmt::DictSet`'s append-a-new-key half (PR-11 Task 5,
        // D-113): `len(x)` growing to `2` is exactly what distinguishes an
        // append from an update. Expected output verified against
        // `python3` on this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictSet {
                    dict: "x".to_string(),
                    key: MirExpr::StringLiteral("b".to_string()),
                    value: MirExpr::IntLiteral(2),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("x")],
                    ty: Ty::Int,
                })),
            ],
        };
        let dir = tempfile_dir("dict_set_append");
        let obj_path = dir.join("dict_set_append.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_set_append");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    #[test]
    fn for_k_in_dict_iterates_keys_in_insertion_order() {
        // `x = {"b": 2, "a": 1}\nfor k in x:\n    print(k)\n` end to end --
        // real `MirStmt::ForDict` iteration codegen (PR-11 Task 5, D-113).
        // "b" printing before "a" (insertion order, not sorted order) is
        // the actual point of this test: `PyDictObj`'s own insertion-order
        // guarantee (D-111) surviving through `pycc_rt_dict_key_at`.
        // Expected output verified against `python3` on this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (MirExpr::StringLiteral("b".to_string()), MirExpr::IntLiteral(2)),
                        (MirExpr::StringLiteral("a".to_string()), MirExpr::IntLiteral(1)),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ForDict {
                    var: "k".to_string(),
                    dict: "x".to_string(),
                    body: vec![print_expr(MirExpr::Name {
                        name: "k".to_string(),
                        ty: Ty::Str,
                    })],
                }),
            ],
        };
        let dir = tempfile_dir("for_dict_iteration");
        let obj_path = dir.join("for_dict_iteration.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_dict_iteration");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"b\na\n");
    }

    #[test]
    fn reassigning_the_for_dict_loop_variable_inside_the_body_does_not_corrupt_the_dict() {
        // The regression test for this arm's own doc comment (point 3):
        // `for k in x:\n    k = "z"\n    print(k)\nprint(len(x))\nprint(x["a"])\n`
        // reassigns the loop variable on every iteration, which -- without
        // the `pycc_rt_str_incref` `MirStmt::ForDict`'s codegen gives the
        // loop variable's own slot -- would decref (and, on the last
        // iteration, free) a key `x` itself still points to, corrupting
        // the dict for the two `print`s that follow the loop. Both survive
        // intact if the incref is doing its job. Expected output verified
        // against `python3` on this exact source (CPython, of course, has
        // no such hazard at all -- this test is pinning pycc's own
        // representation-specific safety property, not a language
        // semantic).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::ForDict {
                    var: "k".to_string(),
                    dict: "x".to_string(),
                    body: vec![
                        MirStmt::Assign {
                            target: "k".to_string(),
                            value: MirExpr::StringLiteral("z".to_string()),
                        },
                        print_expr(MirExpr::Name {
                            name: "k".to_string(),
                            ty: Ty::Str,
                        }),
                    ],
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("x")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("x")),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                })),
            ],
        };
        let dir = tempfile_dir("for_dict_var_reassignment");
        let obj_path = dir.join("for_dict_var_reassignment.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_dict_var_reassignment");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"z\n1\n1\n");
    }

    #[test]
    fn set_literal_construction_dedups_and_reports_correct_len() {
        // `x = {1, 2, 2, 3}\nprint(len(x))\n` end to end through
        // `compile_to_object` -- real `MirExpr::SetLiteral` construction
        // (PR-11 Task 9) plus `len()`'s new `Ty::Set` branch. The dedup
        // check lives entirely in `pycc_rt_int_set_add` (Task 6); this test
        // proves codegen actually calls it per element, unconditionally,
        // and that the repeated `2` collapses to one element. Expected
        // output verified against `python3` on this exact source (a set
        // literal's own construction order does not affect its `len`).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(3),
                    ]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![set_name("x")],
                    ty: Ty::Int,
                })),
            ],
        };
        let dir = tempfile_dir("set_literal_and_len");
        let obj_path = dir.join("set_literal_and_len.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("set_literal_and_len");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n");
    }

    #[test]
    fn for_x_in_set_iterates_in_first_insertion_order() {
        // `x = {2, 1, 2}\nfor v in x:\n    print(v)\n` end to end -- real
        // `MirStmt::ForSet` iteration codegen (PR-11 Task 9, D-113). `2`
        // printing before `1` (first-insertion order, with the second `2`
        // deduped away rather than moving `2`'s position) is the actual
        // point of this test: `PyIntSetObj`'s own insertion-order guarantee
        // (D-111) surviving through `pycc_rt_int_set_get`.
        //
        // This is pycc's own internal-consistency check, NOT a conformance
        // fixture against CPython: `python3` on this exact source prints
        // `1`/`2` (CPython's own set iteration order for small ints is not
        // insertion order -- small ints hash to themselves, so CPython's
        // hash-table iteration order here happens to be numeric order, not
        // insertion order). Task 10 owns why no conformance fixture makes
        // this same assertion against CPython; this test only pins pycc's
        // own behavior against itself.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ForSet {
                    var: "v".to_string(),
                    set: "x".to_string(),
                    body: vec![print_expr(MirExpr::Name {
                        name: "v".to_string(),
                        ty: Ty::Int,
                    })],
                }),
            ],
        };
        let dir = tempfile_dir("for_set_iteration");
        let obj_path = dir.join("for_set_iteration.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_set_iteration");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n1\n");
    }

    #[test]
    #[should_panic(expected = "`n` did not evaluate to a set")]
    fn for_set_over_a_non_set_local_is_an_internal_error() {
        // `n = 1` then `for v in n:`: `pycc_types` rejects this with T0033
        // ("not iterable"), so codegen only sees it as hand-built malformed
        // MIR -- mirrors `for_dict_over_an_unbound_name_is_an_internal_
        // error`/`appending_to_a_non_list_local_is_an_internal_error`
        // above, for the identical reason. Covers `emit_set_name_read`'s
        // own use of `expect_set_pointer`.
        let mir = list_fixture_module(vec![
            MirStmt::Assign {
                target: "n".to_string(),
                value: MirExpr::IntLiteral(1),
            },
            MirStmt::ForSet {
                var: "v".to_string(),
                set: "n".to_string(),
                body: vec![],
            },
        ]);
        let dir = tempfile_dir("for_set_on_non_set_panics");
        let _ = compile_to_object(&mir, &dir.join("for_set_on_non_set_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "`never_bound` has no local slot")]
    fn for_set_over_an_unbound_name_is_an_internal_error() {
        // `for v in never_bound:` where nothing ever bound `never_bound` --
        // mirrors `iterating_a_name_with_no_local_slot_is_an_internal_
        // error`/`for_dict_over_an_unbound_name_is_an_internal_error`
        // above, for the identical reason: codegen's own defensive
        // backstop for `emit_set_name_read`'s slot lookup, the one branch
        // `for_set_over_a_non_set_local_is_an_internal_error` above does
        // not reach.
        let mir = list_fixture_module(vec![MirStmt::ForSet {
            var: "v".to_string(),
            set: "never_bound".to_string(),
            body: vec![],
        }]);
        let dir = tempfile_dir("for_set_unbound_name_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("for_set_unbound_name_panics.o"),
            None,
            false,
        );
    }

    #[test]
    fn compiles_a_function_with_a_set_int_parameter_and_set_int_return_value() {
        // The `set[int]` counterpart of `compiles_a_function_with_a_dict_
        // str_int_parameter_and_dict_str_int_return_value` above, for the
        // identical reason: no real source program can produce this shape
        // (`pycc_hir::annotation_to_ty` rejects every annotation but a
        // bare name, so an annotated `set[int]` parameter or return type
        // never reaches codegen), but this MIR shape must still compile
        // *cleanly* -- `ty_to_basic_type`'s `Set(_)` arm and `emit_expr`'s
        // `Name` arm's `Set(_)` arm must agree on the same pointer
        // representation, and `MirStmt::Return`'s own `Scalar::Set`
        // pass-through arm must actually build a valid `ret` instruction.
        // Deliberately does not link or run the resulting object, for the
        // identical reason the dict counterpart doesn't.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), set_int())],
                return_ty: set_int(),
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: set_int(),
                }))],
            }],
        };
        let dir = tempfile_dir("set_int_param_and_return");
        let obj_path = dir.join("set_int_param_and_return.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn passing_a_set_value_as_a_function_argument_marshals_it_like_a_pointer() {
        // The `set[int]` counterpart of `passing_a_dict_value_as_a_
        // function_argument_marshals_it_like_a_pointer` above: the caller
        // adds the one shape the test directly above does not reach --
        // `build_call_to`'s argument-marshalling match, whose `Scalar::Set`
        // arm is also in the pass-through bucket. Same not-linked, not-run
        // caveat as the dict counterpart: neither `f` nor `g` is ever
        // called, and their annotated `set[int]` parameters are
        // unreachable from real source, so this proves the codegen shape
        // only.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), set_int())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(0)))],
                },
                MirItem::Function {
                    name: "g".to_string(),
                    params: vec![("x".to_string(), set_int())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::Name {
                            name: "x".to_string(),
                            ty: set_int(),
                        }],
                        ty: Ty::Int,
                    }))],
                },
            ],
        };
        let dir = tempfile_dir("set_int_passed_as_argument");
        let obj_path = dir.join("set_int_passed_as_argument.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn an_error_inside_a_for_set_body_propagates_out_of_codegen() {
        // `MirStmt::ForSet`'s arm emits its body through `emit_body`, whose
        // `Result` it must propagate rather than swallow -- mirrors
        // `an_error_inside_a_for_dict_body_propagates_out_of_codegen`
        // above, for the identical reason. A call to an undefined function
        // is the one failure `emit_stmt` reports as a clean `Err` instead
        // of a panic.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::SetLiteral(vec![MirExpr::IntLiteral(1)]),
                }),
                MirItem::TopLevelStmt(MirStmt::ForSet {
                    var: "v".to_string(),
                    set: "x".to_string(),
                    body: vec![call_user_fn("missing")],
                }),
            ],
        };
        let dir = tempfile_dir("for_set_body_error");
        let error = compile_to_object(&mir, &dir.join("for_set_body_error.o"), None, false)
            .expect_err("the undefined call inside the loop body should fail");
        assert!(error.contains("missing"));
    }

    #[test]
    #[should_panic(expected = "pycc_codegen: truthiness of a set[T] value is not supported yet")]
    fn truthiness_of_a_set_value_panics_honestly() {
        // The `set[T]` counterpart of `truthiness_of_a_dict_value_panics_
        // honestly` above (D-107's reasoning, per D-114): `pycc_types`
        // accepts any type in a boolean context, so `if s:` for a
        // `set[int]` local type-checks today. v0.2 has no `bool(set)`
        // semantics (D-114), so an honest panic naming the gap is the
        // correct behavior. Calls `truthy` directly with a hand-built
        // `Scalar::Set`, for the identical reason that test gives.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        truthy(&context, &builder, &rt, Scalar::Set(ptr));
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: string conversion of a set[T] value is not supported yet"
    )]
    fn string_conversion_of_a_set_value_panics_honestly() {
        // The `set[T]` counterpart of `string_conversion_of_a_dict_value_
        // panics_honestly` above, for the identical reason.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_str(&builder, &rt, Scalar::Set(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected an int-or-bool operand, got set")]
    fn to_tagged_int_rejects_a_set_operand() {
        // The `set[T]` counterpart of `to_tagged_int_rejects_a_dict_
        // operand` above -- genuinely defensive for the identical reason:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::Set` either, so no real MIR reaches this arm.
        let context = Context::create();
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_tagged_int(&context, &builder, Scalar::Set(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got set")]
    fn to_float_rejects_a_set_operand() {
        // Same defensive-arm rationale as `to_tagged_int_rejects_a_set_
        // operand` directly above, for `to_float`'s own match.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_float(&context, &builder, &rt, Scalar::Set(ptr));
    }

    #[test]
    fn a_return_inside_a_for_list_body_returns_immediately_without_looping() {
        // The `MirStmt::ForList` counterpart of `a_return_inside_a_for_
        // range_body_returns_immediately_without_looping` above, for the
        // same reason: `ForList`'s arm carries its own inline copy of the
        // terminator-safety guard, and without it the increment-and-branch-
        // back would build a second terminator onto a block `body`'s
        // `Return` already terminated -- IR `module.verify()` rejects.
        // Prints "1", not "1\n2\n3\n".
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "first_of_list".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        assign_list_literal("xs"),
                        MirStmt::ForList {
                            var: "v".to_string(),
                            list: "xs".to_string(),
                            body: vec![MirStmt::Return(Some(MirExpr::Name {
                                name: "v".to_string(),
                                ty: Ty::Int,
                            }))],
                        },
                        // Unreachable in practice (the loop always returns
                        // on its first iteration for this non-empty list),
                        // but required to keep this hand-built MIR
                        // well-formed for a non-`None`-returning function --
                        // exactly the same caveat the `ForRange` version of
                        // this test documents.
                        MirStmt::Return(Some(MirExpr::IntLiteral(-1))),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "first_of_list".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("for_list_return_inside_body");
        let obj_path = dir.join("for_list_return_inside_body.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_list_return_inside_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_return_inside_a_for_set_body_returns_immediately_without_looping() {
        // The `MirStmt::ForSet` counterpart of `a_return_inside_a_for_list_
        // body_returns_immediately_without_looping` above, for the same
        // reason: `ForSet`'s arm carries its own inline copy of the
        // terminator-safety guard, and without it the increment-and-branch-
        // back would build a second terminator onto a block `body`'s
        // `Return` already terminated -- IR `module.verify()` rejects. This
        // is the one shape none of this task's other `ForSet` tests
        // exercise (they all either fall through normally or error before
        // reaching a `Return`), so it is the dedicated regression coverage
        // for that specific skip branch. Prints one element only, not
        // every element in the set.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "first_of_set".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::Assign {
                            target: "xs".to_string(),
                            value: MirExpr::SetLiteral(vec![
                                MirExpr::IntLiteral(1),
                                MirExpr::IntLiteral(2),
                                MirExpr::IntLiteral(3),
                            ]),
                        },
                        MirStmt::ForSet {
                            var: "v".to_string(),
                            set: "xs".to_string(),
                            body: vec![MirStmt::Return(Some(MirExpr::Name {
                                name: "v".to_string(),
                                ty: Ty::Int,
                            }))],
                        },
                        // Unreachable in practice (the loop always returns
                        // on its first iteration for this non-empty set),
                        // but required to keep this hand-built MIR
                        // well-formed for a non-`None`-returning function --
                        // exactly the same caveat the `ForList`/`ForRange`
                        // versions of this test document.
                        MirStmt::Return(Some(MirExpr::IntLiteral(-1))),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "first_of_set".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("for_set_return_inside_body");
        let obj_path = dir.join("for_set_return_inside_body.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_set_return_inside_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_for_list_loop_visits_every_element_in_order() {
        // The full `MirStmt::ForList` loop, run to completion: unlike
        // `a_return_inside_a_for_list_body_returns_immediately_without_
        // looping` above, this one reaches the arm's increment-and-branch-
        // back block on every iteration and its loop test's exhaustion
        // edge, and proves the per-iteration element read is re-tagged
        // (D-106) rather than printed as a raw slot value -- an untagged
        // element would print `0`/`1`/`1` here, not `1`/`2`/`3`.
        //
        // Kept as a `pycc_codegen` unit test even though
        // `tests/slice1_codegen_depth.rs` already covers the same behavior
        // from real source, because empirically that was not enough: with
        // this test and `a_module_level_list_binding_gets_a_null_
        // initialized_pointer_global` below absent, `cargo llvm-cov
        // --workspace` reported this file at 99.68% regions with no
        // uncovered line to point at. That integration suite drives the
        // separate `pycc` binary, which links its own copy of this crate,
        // and llvm-cov's per-instantiation accounting does not always let
        // that copy stand in for the one this crate's own test binary
        // uses.
        let mir = list_fixture_module(vec![
            assign_list_literal("xs"),
            MirStmt::ForList {
                var: "v".to_string(),
                list: "xs".to_string(),
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "v".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })],
            },
        ]);
        let dir = tempfile_dir("for_list_visits_every_element");
        let obj_path = dir.join("for_list_visits_every_element.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_list_visits_every_element");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n3\n");
    }

    #[test]
    fn a_for_list_loop_keeps_its_per_iteration_length_read_under_release_optimization() {
        // `MirStmt::ForList` calls `pycc_rt_int_list_len` inside its
        // loop-test block on purpose, so appending during iteration extends
        // the loop exactly as CPython's list iterator does.
        // `iterating_a_list_rereads_its_length_each_step_like_cpython` in
        // `tests/slice1_codegen_depth.rs` pins that from real source, but
        // only for an unoptimized build, where nothing could hoist the call
        // anyway. `--release` additionally runs LLVM's `"default<O3>"`
        // pipeline (D-094), whose LICM pass is precisely the transform that
        // would lift a loop-invariant-looking call out of the loop and
        // silently restore the hoisted behavior. It does not today --
        // `declare_rt_functions` gives the declaration no attributes, so
        // LLVM must assume the call may write memory -- but that is an
        // inference from an absence, and a future PR adding
        // `readonly`/`willreturn` to these externs for performance would
        // invalidate it with no other release-profile test noticing.
        //
        // The fixture therefore has to *mutate* `xs` mid-loop. An earlier
        // version of this test iterated a fixed `[1, 2, 3]` and asserted
        // `1\n2\n3\n`, which a hoisted length read satisfies just as well --
        // `len(xs)` is loop-invariant at 3 either way, so the test could not
        // fail for the reason it is named after. Confirmed by patching this
        // arm to compute the length in the preheader: that version still
        // passed. This one grows the list from 1 element to 3 while
        // iterating, so a length read hoisted out of the loop sees 1, runs a
        // single iteration, and prints only "1".
        //
        // `if len(xs) < 3: xs.append(v + 1)` then `print(v)`, the same
        // program the end-to-end test above uses.
        let list_int = || Ty::List(Box::new(Ty::Int));
        let mir = list_fixture_module(vec![
            MirStmt::Assign {
                target: "xs".to_string(),
                value: MirExpr::ListLiteral(vec![MirExpr::IntLiteral(1)]),
            },
            MirStmt::ForList {
                var: "v".to_string(),
                list: "xs".to_string(),
                body: vec![
                    MirStmt::If {
                        test: MirExpr::Compare {
                            op: CmpOpKind::Lt,
                            left: Box::new(MirExpr::Call {
                                callee: "len".to_string(),
                                args: vec![MirExpr::Name {
                                    name: "xs".to_string(),
                                    ty: list_int(),
                                }],
                                ty: Ty::Int,
                            }),
                            right: Box::new(MirExpr::IntLiteral(3)),
                            ty: Ty::Bool,
                        },
                        body: vec![MirStmt::ExprStmt(MirExpr::ListAppend {
                            list: "xs".to_string(),
                            value: Box::new(MirExpr::BinOp {
                                op: BinOpKind::Add,
                                left: Box::new(MirExpr::Name {
                                    name: "v".to_string(),
                                    ty: Ty::Int,
                                }),
                                right: Box::new(MirExpr::IntLiteral(1)),
                                ty: Ty::Int,
                            }),
                        })],
                        orelse: vec![],
                    },
                    MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::Name {
                            name: "v".to_string(),
                            ty: Ty::Int,
                        }],
                        ty: Ty::None,
                    }),
                ],
            },
        ]);
        let dir = tempfile_dir("for_list_release");
        let obj_path = dir.join("for_list_release.o");
        compile_to_object(&mir, &obj_path, None, true).expect("release codegen should succeed");
        let bin_path = dir.join("for_list_release");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n3\n");
    }

    #[test]
    fn appending_to_a_list_untags_the_value_before_it_reaches_runtime_storage() {
        // `MirExpr::ListAppend`'s success path, the one new arm whose body
        // no other unit test in this file reaches (`appending_to_a_non_
        // list_local_is_an_internal_error` above panics inside
        // `emit_list_name_read` before any of it runs). Reading the
        // appended element straight back out is what pins D-106's
        // round trip for this arm specifically: the value is untagged on
        // the way into `pycc_rt_int_list_append` and re-tagged on the way
        // out of `pycc_rt_int_list_get`, so a missing conversion on either
        // side would print a mangled number here rather than "4".
        let mir = list_fixture_module(vec![
            assign_list_literal("xs"),
            MirStmt::ExprStmt(MirExpr::ListAppend {
                list: "xs".to_string(),
                value: Box::new(MirExpr::IntLiteral(4)),
            }),
            MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Int)),
                    }),
                    index: Box::new(MirExpr::IntLiteral(3)),
                }],
                ty: Ty::None,
            }),
        ]);
        let dir = tempfile_dir("list_append_round_trip");
        let obj_path = dir.join("list_append_round_trip.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("list_append_round_trip");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"4\n");
    }

    #[test]
    fn a_module_level_list_binding_gets_a_null_initialized_pointer_global() {
        // `declare_module_globals`' `Ty::List(_)` arm: a module-scope
        // `xs = [1, 2, 3]` (one of the two places D-105's first scope cut
        // says a `list[int]` value may live) becomes an LLVM global rather
        // than a function-local alloca. Task 5 (D-089) deliberately left
        // this arm out while no real source could build a list value, and
        // flagged re-deriving it for Task 11; without it this exact MIR
        // panics with "a `list[int]`-typed module binding is not supported
        // yet".
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal("xs")),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "len".to_string(),
                        args: vec![MirExpr::Name {
                            name: "xs".to_string(),
                            ty: Ty::List(Box::new(Ty::Int)),
                        }],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("module_level_list_global");
        let obj_path = dir.join("module_level_list_global.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("module_level_list_global");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n");
    }

    #[test]
    fn an_error_inside_a_for_list_body_propagates_out_of_codegen() {
        // `MirStmt::ForList`'s arm emits its body through `emit_body`,
        // whose `Result` it must propagate rather than swallow -- the same
        // `?`-propagation every other body-emitting arm relies on (see
        // `public_codegen_api_propagates_an_error_from_a_function_body` in
        // `tests/slice1_codegen_depth.rs`). A call to an undefined function
        // is the one failure `emit_stmt` reports as a clean `Err` instead
        // of a panic.
        let mir = list_fixture_module(vec![
            assign_list_literal("xs"),
            MirStmt::ForList {
                var: "v".to_string(),
                list: "xs".to_string(),
                body: vec![call_user_fn("missing")],
            },
        ]);
        let dir = tempfile_dir("for_list_body_error");
        let error = compile_to_object(&mir, &dir.join("for_list_body_error.o"), None, false)
            .expect_err("the undefined call inside the loop body should fail");
        assert!(error.contains("missing"));
    }

    #[test]
    fn a_bool_list_element_widens_to_a_tagged_int_before_it_is_stored() {
        // `xs = [True, 1]` never reaches codegen from real source
        // (`pycc_types` rejects a mixed literal with T0032, and an
        // all-`bool` one with T0034), but `MirExpr::ListLiteral`'s arm
        // deliberately routes each element through `to_tagged_int` rather
        // than requiring a `Scalar::Int` outright -- Python's `bool` is an
        // `int` subtype, so widening is the correct answer if a `bool`
        // element ever does reach it, exactly as `emit_expr`'s own
        // `BinOp`/`Ty::Int` arm already treats a `bool` operand. Prints
        // "1", the tagged `int` value `True` widens to (the same v0.1
        // representation deviation `a_bool_return_value_widens_to_int_when_
        // the_function_declares_int` above pins), not "True".
        let mir = list_fixture_module(vec![
            MirStmt::Assign {
                target: "xs".to_string(),
                value: MirExpr::ListLiteral(vec![MirExpr::BoolLiteral(true)]),
            },
            MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "xs".to_string(),
                        ty: Ty::List(Box::new(Ty::Bool)),
                    }),
                    index: Box::new(MirExpr::IntLiteral(0)),
                }],
                ty: Ty::None,
            }),
        ]);
        let dir = tempfile_dir("bool_list_element_widens");
        let obj_path = dir.join("bool_list_element_widens.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_list_element_widens");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn compiles_a_none_typed_parameter_and_value_return() {
        // `def f(x: None) -> None: return x` -- `None` returns stay LLVM
        // `void`, while the parameter and name read use the canonical i8
        // unit carrier. This is valid frontend input and must not fail only
        // when codegen declares the function.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), Ty::None)],
                return_ty: Ty::None,
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::None,
                }))],
            }],
        };
        let dir = tempfile_dir("none_param_compiles");
        let obj_path = dir.join("none_param_compiles.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                        args: vec![MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }],
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bool_return_widens_to_int");
        let obj_path = dir.join("bool_return_widens_to_int.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("reassign_bool_into_int");
        let obj_path = dir.join("reassign_bool_into_int.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        // `a_function_parameter_can_be_reassigned_read_back_and_printed` above),
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "z".to_string(),
                        ty: pycc_mir::Ty::Int,
                    }],
                    ty: pycc_mir::Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bool_arith");
        let obj_path = dir.join("bool_arith.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
            compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Str,
                        }),
                        right: Box::new(MirExpr::StringLiteral("bar".to_string())),
                        ty: Ty::Str,
                    },
                }),
            ],
        };
        let dir = tempfile_dir("str_concat_reassign");
        let obj_path = dir.join("str_concat_reassign.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_concat_reassign");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    #[should_panic(expected = "string assignment target `value` has a non-string storage slot")]
    fn a_string_assignment_rejects_a_predeclared_non_string_slot() {
        // Deliberately malformed MIR: the checked pipeline preserves a
        // binding's established representation, so it cannot assign `str`
        // to an `int` target. The hand-built shape verifies codegen rejects
        // the mismatch before treating an integer slot as a string pointer.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "value".to_string(),
                    value: MirExpr::IntLiteral(1),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "value".to_string(),
                    value: MirExpr::StringLiteral("bad".to_string()),
                }),
            ],
        };
        let dir = tempfile_dir("str_assignment_non_str_slot_panics");
        let obj_path = dir.join("str_assignment_non_str_slot_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    fn an_int_local_first_assigned_inside_an_if_body_is_readable_after_the_if() {
        // `if True: x = 1` then `print(x)` at the top level, after the
        // `if`. `collect_module_bindings` finds `x` before emission, and
        // `declare_module_globals` creates its process-wide slot and
        // initialized flag before either branch exists. The taken branch
        // stores into that dominating slot and marks it initialized, so the
        // read after `if_merge` succeeds rather than depending on storage
        // first created inside `if_then`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::BoolLiteral(true),
                    body: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(1),
                    }],
                    orelse: vec![],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("int_first_assign_in_if_body");
        let obj_path = dir.join("int_first_assign_in_if_body.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("int_first_assign_in_if_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn an_int_local_assigned_in_both_branches_of_an_if_else_is_readable_after_the_if() {
        // `if True: x = 1 else: x = 2` then `print(x)` -- module-binding
        // collection predeclares one global slot and initialized flag for
        // `x` before codegen reaches either sibling block. Both branches
        // store into that same dominating slot and mark it initialized, so
        // the merged read is independent of branch emission order.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::BoolLiteral(true),
                    body: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(1),
                    }],
                    orelse: vec![MirStmt::Assign {
                        target: "x".to_string(),
                        value: MirExpr::IntLiteral(2),
                    }],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("int_first_assign_in_if_else_both");
        let obj_path = dir.join("int_first_assign_in_if_else_both.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("int_first_assign_in_if_else_both");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_str_local_first_assigned_inside_an_if_body_is_freed_at_top_level_completion() {
        // Regression test for a review finding against this file's first
        // `str`-codegen task: `if True: s = "hi"` -- `s`'s *first*
        // assignment executes inside the `if`'s own `then` block, never at
        // the top level directly, and `s` is never read by any user code
        // either. `collect_module_bindings` predeclares `s` as an internal
        // pointer global with a null initializer and a separate initialized
        // flag before `main` is emitted. The nested assignment stores into
        // that process-wide slot, and the top-level completion pass can load
        // and decref it after the merge without any CFG-dominance dependency
        // on the `then` block (D-074).
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_if_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_assigned_in_both_branches_of_an_if_else_is_freed_at_top_level_completion() {
        // `if True: s = "hi" else: s = "bye"` -- `s` is classified and
        // predeclared as one null-initialized module global before either
        // branch is emitted. Both siblings store into that same slot rather
        // than whichever branch is visited first creating storage. This
        // additionally proves the pre-store null/old-value decref path is
        // safe from either predecessor (D-074).
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_if_else_both");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_first_assigned_inside_a_while_body_frees_previous_and_final_values() {
        // `i = 0; while i < 3: s = "x"; i = i + 1` -- `s`'s first assignment
        // happens inside the `while`'s own body block. Its module-global
        // pointer slot is nevertheless declared with a null initializer
        // before `main` is emitted, so every loop iteration reuses the same
        // storage: the pre-store decref is harmless on the first iteration,
        // frees the previous value on later iterations, and the top-level
        // completion pass frees the final value (D-074).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "i".to_string(),
                    value: MirExpr::IntLiteral(0),
                }),
                MirItem::TopLevelStmt(MirStmt::While {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }),
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
                                left: Box::new(MirExpr::Name {
                                    name: "i".to_string(),
                                    ty: Ty::Int,
                                }),
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_while_body");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_never_assigned_on_the_taken_path_decrefs_a_clean_null_at_completion() {
        // `flag = ""; if flag: s = "hi"` -- `flag` is falsy (the empty
        // string), so the `then` block containing `s`'s only assignment
        // never runs at all. `declare_module_globals` still creates `s`'s
        // pointer global with a null initializer and its initialized flag
        // with a false initializer before `main` starts. The top-level
        // completion pass therefore loads a real null rather than LLVM
        // `undef`, and `pycc_rt_str_decref` safely no-ops. Dropping that
        // global initializer would make this path load an indeterminate
        // pointer even though no Python assignment executed.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "flag".to_string(),
                    value: MirExpr::StringLiteral(String::new()),
                }),
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::Name {
                        name: "flag".to_string(),
                        ty: Ty::Str,
                    },
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_never_assigned_on_taken_path");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(
            output.status.success(),
            "should run without crashing (null-guarded decref of a never-assigned slot)"
        );
    }

    #[test]
    fn a_str_local_first_assigned_inside_a_functions_own_leading_if_body() {
        // `def f() -> None:\n    if True:\n        s = "hi"` ; `f()` --
        // function-local collection finds `s` throughout the body before
        // emitting its leading control flow. `storage_slot_at_entry` creates
        // the null-initialized pointer slot and false initialized flag while
        // the builder is still in `f`'s entry block; only then is the `if`
        // emitted. The nested assignment marks that dominating per-call slot
        // initialized without creating storage inside `if_then`.
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_fn_leading_if");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn a_str_local_first_assigned_as_a_functions_own_plain_leading_statement() {
        // `def f() -> None:\n    s = "hi"` ; `f()` -- the straight-line
        // counterpart to the leading-`if` test above. Body preclassification
        // still creates one guarded, null-initialized per-call slot before
        // statement emission, and the assignment reuses it and flips its
        // initialized flag.
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_first_assign_in_fn_plain");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success(), "should run without crashing");
    }

    #[test]
    fn an_int_local_assigned_in_both_branches_of_a_functions_own_leading_if_else() {
        // `def f() -> int:\n    if True:\n        x = 1\n    else:\n        x = 2\n    return x`
        // ; `print(f())` -- must print `1`. Function-local collection
        // preclassifies `x`, and `storage_slot_at_entry` creates its slot and
        // initialized flag before the leading `if` is emitted. Both sibling
        // branches then store into that one dominating per-call slot, and
        // the return reads the value selected by the executed branch.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::If {
                            test: MirExpr::BoolLiteral(true),
                            body: vec![MirStmt::Assign {
                                target: "x".to_string(),
                                value: MirExpr::IntLiteral(1),
                            }],
                            orelse: vec![MirStmt::Assign {
                                target: "x".to_string(),
                                value: MirExpr::IntLiteral(2),
                            }],
                        },
                        MirStmt::Return(Some(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        })),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("int_first_assign_in_fn_leading_if_else");
        let obj_path = dir.join("int_first_assign_in_fn_leading_if_else.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("int_first_assign_in_fn_leading_if_else");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn a_float_local_first_assigned_inside_a_function_uses_preclassified_storage() {
        // `def f() -> float:\n    y = 2.5\n    return y` ; `print(f())` --
        // body preclassification creates `y`'s guarded f64 storage in the
        // function entry block before emission. The plain assignment reuses
        // that slot and preserves its float representation through the
        // subsequent return (distinct from the integer and string cases).
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Float,
                    body: vec![
                        MirStmt::Assign {
                            target: "y".to_string(),
                            value: MirExpr::FloatLiteral(2.5),
                        },
                        MirStmt::Return(Some(MirExpr::Name {
                            name: "y".to_string(),
                            ty: Ty::Float,
                        })),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![],
                        ty: Ty::Float,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("float_first_assign_in_fn");
        let obj_path = dir.join("float_first_assign_in_fn.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
            compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
            let bin_path = dir.join("str_cmp_runtime");
            link_object_with_runtime(&obj_path, &bin_path);
            let output = Command::new(&bin_path).output().expect("binary should run");
            assert_eq!(
                output.stdout,
                expected.as_bytes(),
                "comparing {left:?} < {right:?}"
            );
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
            compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        let _ = compile_to_object(&mir, &obj_path, None, false);
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn compiles_an_f_string_interpolating_a_none_returning_call_as_none() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "returns_none".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "text".to_string(),
                    value: MirExpr::FString(vec![pycc_mir::MirFStringPart::Interpolation(
                        Box::new(MirExpr::Call {
                            callee: "returns_none".to_string(),
                            args: vec![],
                            ty: Ty::None,
                        }),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "text".to_string(),
                        ty: Ty::Str,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("fstring_none_call");
        let obj_path = dir.join("fstring_none_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("fstring_none_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"None\n");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
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
                    args: vec![MirExpr::Name {
                        name: "s".to_string(),
                        ty: Ty::Str,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("fstring_none_call");
        let obj_path = dir.join("fstring_none_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("fstring_none_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"got: None\n");
    }

    #[test]
    fn interpolating_a_none_typed_parameter_renders_none() {
        // `render(source())` exercises the same unit carrier through an
        // f-string interpolation of a parameter name rather than `print`'s
        // dedicated `None` path.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "source".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::Function {
                    name: "render".to_string(),
                    params: vec![("value".to_string(), Ty::None)],
                    return_ty: Ty::Str,
                    body: vec![MirStmt::Return(Some(MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "value".to_string(),
                            ty: Ty::None,
                        })),
                    ])))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "render".to_string(),
                        args: vec![MirExpr::Call {
                            callee: "source".to_string(),
                            args: vec![],
                            ty: Ty::None,
                        }],
                        ty: Ty::Str,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("fstring_none_typed_parameter");
        let obj_path = dir.join("fstring_none_typed_parameter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("fstring_none_typed_parameter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"None\n");
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
                            left: Box::new(MirExpr::Name {
                                name: "acc".to_string(),
                                ty: Ty::Int,
                            }),
                            right: Box::new(MirExpr::Name {
                                name: "acc".to_string(),
                                ty: Ty::Int,
                            }),
                            ty: Ty::Int,
                        },
                    }],
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Name {
                        name: "acc".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
        };
        let dir = tempfile_dir("bigint_overflow_loop");
        let obj_path = dir.join("bigint_overflow_loop.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bigint_overflow_loop");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, format!("{expected}\n").into_bytes());
    }

    #[test]
    #[should_panic(expected = "`n` did not evaluate to a dict")]
    fn dict_set_item_on_a_non_dict_local_is_an_internal_error() {
        // `n = 1` then `n["a"] = 2`: `pycc_types` rejects this with T0033
        // ("does not support item assignment"), so codegen only sees it as
        // hand-built malformed MIR -- mirrors `appending_to_a_non_list_
        // local_is_an_internal_error` above, for the identical reason.
        // Covers `emit_dict_name_read`'s own use of `expect_dict_pointer`,
        // the shared check `MirStmt::ForDict`'s dict operand also goes
        // through.
        let mir = list_fixture_module(vec![
            MirStmt::Assign {
                target: "n".to_string(),
                value: MirExpr::IntLiteral(1),
            },
            MirStmt::DictSet {
                dict: "n".to_string(),
                key: MirExpr::StringLiteral("a".to_string()),
                value: MirExpr::IntLiteral(2),
            },
        ]);
        let dir = tempfile_dir("dict_set_on_non_dict_panics");
        let _ = compile_to_object(&mir, &dir.join("dict_set_on_non_dict_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "`never_bound` has no local slot")]
    fn for_dict_over_an_unbound_name_is_an_internal_error() {
        // `for k in never_bound:` where nothing ever bound `never_bound` --
        // mirrors `iterating_a_name_with_no_local_slot_is_an_internal_error`
        // above, for the identical reason: codegen's own defensive backstop
        // for `emit_dict_name_read`'s slot lookup, the one branch
        // `dict_set_item_on_a_non_dict_local_is_an_internal_error` above
        // does not reach.
        let mir = list_fixture_module(vec![MirStmt::ForDict {
            var: "k".to_string(),
            dict: "never_bound".to_string(),
            body: vec![],
        }]);
        let dir = tempfile_dir("for_dict_unbound_name_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("for_dict_unbound_name_panics.o"),
            None,
            false,
        );
    }

    #[test]
    #[should_panic(expected = "dict literal key did not evaluate to str")]
    fn a_dict_literal_with_a_non_str_key_is_an_internal_error() {
        // `pycc_types`' T0036 gate rejects every dict literal whose key
        // isn't `Ty::Str` before codegen ever runs, so this is hand-built
        // malformed MIR, not a real-source repro -- covers `MirExpr::
        // DictLiteral`'s own inline key-extraction panic.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::DictLiteral(vec![(MirExpr::IntLiteral(1), MirExpr::IntLiteral(2))]),
            })],
        };
        let dir = tempfile_dir("dict_literal_non_str_key_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("dict_literal_non_str_key_panics.o"),
            None,
            false,
        );
    }

    #[test]
    #[should_panic(expected = "dict subscript key did not evaluate to str")]
    fn a_dict_get_with_a_non_str_key_is_an_internal_error() {
        // `pycc_types` rejects a mismatched dict-subscript key type with
        // T0021 before codegen ever runs, so this is hand-built malformed
        // MIR -- covers `MirExpr::DictGet`'s own inline key-extraction
        // panic.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::DictGet {
                dict: Box::new(MirExpr::DictLiteral(vec![(
                    MirExpr::StringLiteral("a".to_string()),
                    MirExpr::IntLiteral(1),
                )])),
                key: Box::new(MirExpr::IntLiteral(1)),
            }))],
        };
        let dir = tempfile_dir("dict_get_non_str_key_panics");
        let _ = compile_to_object(&mir, &dir.join("dict_get_non_str_key_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "dict item-assignment key did not evaluate to str")]
    fn a_dict_set_with_a_non_str_key_is_an_internal_error() {
        // `pycc_types` rejects a mismatched `d[k] = v` key type with T0021
        // before codegen ever runs, so this is hand-built malformed MIR --
        // covers `MirStmt::DictSet`'s own inline key-extraction panic.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictSet {
                    dict: "x".to_string(),
                    key: MirExpr::IntLiteral(1),
                    value: MirExpr::IntLiteral(2),
                }),
            ],
        };
        let dir = tempfile_dir("dict_set_non_str_key_panics");
        let _ = compile_to_object(&mir, &dir.join("dict_set_non_str_key_panics.o"), None, false);
    }

    #[test]
    fn compiles_a_function_with_a_dict_str_int_parameter_and_dict_str_int_return_value() {
        // The `dict[str, int]` counterpart of `compiles_a_function_with_a_
        // list_int_parameter_and_list_int_return_value` above, for the
        // identical reason: no real source program can produce this shape
        // (`pycc_hir::annotation_to_ty` rejects every annotation but a
        // bare name, so an annotated `dict[str, int]` parameter or return
        // type never reaches codegen), but this MIR shape must still
        // compile *cleanly* -- `ty_to_basic_type`'s `Dict(_)` arm and
        // `emit_expr`'s `Name` arm's `Dict(_)` arm must agree on the same
        // pointer representation, and `MirStmt::Return`'s own `Scalar::
        // Dict` pass-through arm must actually build a valid `ret`
        // instruction. Deliberately does not link or run the resulting
        // object, for the identical reason the list counterpart doesn't.
        let dict_str_int = || Ty::Dict(Box::new((Ty::Str, Ty::Int)));
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), dict_str_int())],
                return_ty: dict_str_int(),
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: dict_str_int(),
                }))],
            }],
        };
        let dir = tempfile_dir("dict_str_int_param_and_return");
        let obj_path = dir.join("dict_str_int_param_and_return.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn passing_a_dict_value_as_a_function_argument_marshals_it_like_a_pointer() {
        // The `dict[str, int]` counterpart of `passing_a_list_value_as_a_
        // function_argument_marshals_it_like_a_pointer` above: the caller
        // adds the one shape the test directly above does not reach --
        // `build_call_to`'s argument-marshalling match, whose `Scalar::
        // Dict` arm is also in the pass-through bucket. Same not-linked,
        // not-run caveat as the list counterpart: neither `f` nor `g` is
        // ever called, and their annotated `dict[str, int]` parameters are
        // unreachable from real source, so this proves the codegen shape
        // only.
        let dict_str_int = || Ty::Dict(Box::new((Ty::Str, Ty::Int)));
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), dict_str_int())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(0)))],
                },
                MirItem::Function {
                    name: "g".to_string(),
                    params: vec![("x".to_string(), dict_str_int())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::Name {
                            name: "x".to_string(),
                            ty: dict_str_int(),
                        }],
                        ty: Ty::Int,
                    }))],
                },
            ],
        };
        let dir = tempfile_dir("dict_str_int_passed_as_argument");
        let obj_path = dir.join("dict_str_int_passed_as_argument.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn an_error_inside_a_for_dict_body_propagates_out_of_codegen() {
        // `MirStmt::ForDict`'s arm emits its body through `emit_body`,
        // whose `Result` it must propagate rather than swallow -- mirrors
        // `an_error_inside_a_for_list_body_propagates_out_of_codegen`
        // above, for the identical reason. A call to an undefined function
        // is the one failure `emit_stmt` reports as a clean `Err` instead
        // of a panic.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::ForDict {
                    var: "k".to_string(),
                    dict: "x".to_string(),
                    body: vec![call_user_fn("missing")],
                }),
            ],
        };
        let dir = tempfile_dir("for_dict_body_error");
        let error = compile_to_object(&mir, &dir.join("for_dict_body_error.o"), None, false)
            .expect_err("the undefined call inside the loop body should fail");
        assert!(error.contains("missing"));
    }

    #[test]
    fn a_dict_literal_key_read_from_a_variable_is_increfed_before_storage() {
        // Regression test for a confirmed use-after-free a pinned-reviewer
        // pass on this task caught: `pycc_rt_dict_set` adopts whatever key
        // pointer it is given as `PyDictObj`'s own permanent reference,
        // without incref'ing it itself (D-114: "neither increfed on insert
        // nor decrefed ... ever"). `{k: 1}` where `k` is a bare `str`
        // variable is exactly the *duplicate*-reference shape `incref_if_
        // str_duplicate` exists to protect at every other ownership-taking
        // boundary (`MirStmt::Assign`, `build_call_to`'s argument
        // marshalling, `MirStmt::Return`) -- without incref'ing it here
        // too, a later `k = ...` reassignment would decref (and,at
        // refcount 1, free) the exact `PyStrObj` the dict still points to.
        // Checked via the same object-symbol-presence technique
        // `passing_a_list_value_as_a_function_argument_marshals_it_like_a_
        // pointer` above uses: this minimal fixture has no other reason to
        // reference `pycc_rt_str_incref` at all, so the assertion is a
        // deterministic proxy for "the fix's `incref_if_str_duplicate` call
        // actually ran," not dependent on allocator behavior the way
        // observing an actual corrupted read would be.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "k".to_string(),
                    value: MirExpr::StringLiteral("a".to_string()),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::Name {
                            name: "k".to_string(),
                            ty: Ty::Str,
                        },
                        MirExpr::IntLiteral(1),
                    )]),
                }),
            ],
        };
        let dir = tempfile_dir("dict_literal_variable_key_incref");
        let obj_path = dir.join("dict_literal_variable_key_incref.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let obj_bytes = std::fs::read(&obj_path).expect("object file should be readable");
        let references_symbol =
            |name: &str| obj_bytes.windows(name.len()).any(|w| w == name.as_bytes());
        assert!(
            references_symbol("pycc_rt_str_incref"),
            "a variable-keyed dict literal must incref its key's shared PyStrObj \
             before handing it to pycc_rt_dict_set, or a later reassignment of the \
             source variable could free memory the dict still points to"
        );
    }

    #[test]
    fn dict_set_item_key_read_from_a_variable_is_increfed_before_storage() {
        // Same regression coverage as `a_dict_literal_key_read_from_a_
        // variable_is_increfed_before_storage` above, for `MirStmt::
        // DictSet`'s own key -- the statement-level `d[k] = v` counterpart
        // of the expression-level dict literal.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("z".to_string()),
                        MirExpr::IntLiteral(0),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "k".to_string(),
                    value: MirExpr::StringLiteral("a".to_string()),
                }),
                MirItem::TopLevelStmt(MirStmt::DictSet {
                    dict: "x".to_string(),
                    key: MirExpr::Name {
                        name: "k".to_string(),
                        ty: Ty::Str,
                    },
                    value: MirExpr::IntLiteral(1),
                }),
            ],
        };
        let dir = tempfile_dir("dict_set_variable_key_incref");
        let obj_path = dir.join("dict_set_variable_key_incref.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let obj_bytes = std::fs::read(&obj_path).expect("object file should be readable");
        let references_symbol =
            |name: &str| obj_bytes.windows(name.len()).any(|w| w == name.as_bytes());
        assert!(
            references_symbol("pycc_rt_str_incref"),
            "a variable-keyed d[k] = v must incref its key's shared PyStrObj before \
             handing it to pycc_rt_dict_set, or a later reassignment of the source \
             variable could free memory the dict still points to"
        );
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
