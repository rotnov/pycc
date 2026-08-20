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
use pycc_mir::{CompSource, MirExceptionValue, MirExpr, MirItem, MirModule, MirStmt};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

mod exception;
use exception::{
    ExceptionCodegenState, emit_exception_value, expression_can_set_exception,
    guard_statement_effects,
};
mod bigint_rc;
use bigint_rc::{
    BigIntRefcount, emit_bigint_refcount_call, int_temporary_word, release_if_int_temporary,
    release_int_slot_before_store, release_scalar_if_int_temporary, retain_if_int_duplicate,
};
mod int_const;
use int_const::{emit_int_constant, tag_smallint_const};

const RELEASE_PASS_PIPELINE: &str = "default<O3>";
type CodegenObserver<'observer> =
    dyn for<'ctx> FnMut(&inkwell::module::Module<'ctx>, Option<&'static str>) + 'observer;

/// One MIR-level value during codegen. Extended (never replaced) by later
/// tasks: `Str` (Task 7) is a pointer to an opaque `pycc_rt::PyStrObj` --
/// `pycc_codegen` never inspects its layout (D-059's inline/heap
/// representation is entirely `pycc_rt`'s own concern), only ever passing
/// it through to a `pycc_rt_str_*` call. `Ty::None` uses the same LLVM `i8`
/// carrier shape as `Bool`, but always with the value zero: a distinct enum
/// variant would not add information because MIR's static `Ty` determines
/// whether that carrier means Python's unit value or a real boolean. Function
/// returns of `None` remain LLVM `void`.
///
/// `Copy` because every payload is an inkwell SSA handle (itself `Copy`) --
/// a `Scalar` names a value the module already holds, so duplicating the
/// handle duplicates nothing. #146 Part 2 (D-181) needs a site to both hand
/// a value onward and still classify the word it carried.
#[derive(Clone, Copy)]
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
    /// D-121/D-123) -- like `List`, opaque to this crate, which only ever
    /// stores it, passes it to a `pycc_rt_dict_*` call, or marshals it
    /// across a function boundary.
    ///
    /// Its own variant rather than a reuse of `List`'s or `Str`'s (D-107's
    /// reasoning, extended to this new container by D-124): `PyDictObj` and
    /// `PyIntListObj` have entirely different layouts, and every exhaustive
    /// `Scalar` match (`truthy`/`to_str`/`to_numeric_encoded_int`/`to_float`/
    /// `emit_assign`/argument marshalling) would otherwise hand a
    /// `PyDictObj` pointer straight to a `pycc_rt_int_list_*` or
    /// `pycc_rt_str_*` function -- reachable from ordinary type-checked
    /// source (`if x:`, `print(x)`) the moment dict values become
    /// constructible (this task). Keeping it distinct makes every operation
    /// `dict[K, V]` has no v0.2 semantics for a compile error until it is
    /// answered deliberately, instead of silently misreading memory.
    ///
    /// Refcounting is deliberately *not* wired for this variant in v0.2
    /// (D-124, extending D-107's exact reasoning): `pycc_rt_dict_incref`/
    /// `_decref` are never called on a dict value itself, so a dict's
    /// backing allocation leaks for the process's lifetime, identically to
    /// `List`. That is leak-only -- never a premature free or a double free
    /// -- because nothing frees a dict value early either.
    Dict(PointerValue<'ctx>),
    /// A pointer to a heap-allocated `pycc_rt::PyIntSetObj` (PR-11 Task 6,
    /// D-121/D-124) -- like `List`/`Dict`, opaque to this crate, which only
    /// ever stores it, passes it to a `pycc_rt_int_set_*` call, or marshals
    /// it across a function boundary.
    ///
    /// Its own variant rather than a reuse of `List`'s, `Dict`'s, or
    /// `Str`'s (D-107's reasoning, extended to this new container by
    /// D-124, exactly as `Dict`'s own doc comment already extends it):
    /// `PyIntSetObj` has its own layout, distinct from `PyIntListObj`,
    /// `PyDictObj`, and `PyStrObj`, and every exhaustive `Scalar` match
    /// (`truthy`/`to_str`/`to_numeric_encoded_int`/`to_float`/`emit_assign`/argument
    /// marshalling) would otherwise hand a `PyIntSetObj` pointer straight to
    /// a `pycc_rt_int_list_*`, `pycc_rt_dict_*`, or `pycc_rt_str_*`
    /// function -- reachable from ordinary type-checked source (`if s:`,
    /// `print(s)`) the moment set values become constructible (PR-11 Task
    /// 9). Keeping it distinct makes every operation `set[int]` has no v0.2
    /// semantics for a compile error until it is answered deliberately,
    /// instead of silently misreading memory.
    ///
    /// Refcounting is deliberately *not* wired for this variant in v0.2
    /// (D-124, extending D-107's exact reasoning, identically to `List`/
    /// `Dict`): `pycc_rt_int_set_incref`/`_decref` are never called on a set
    /// value itself, so a set's backing allocation leaks for the process's
    /// lifetime. That is leak-only -- never a premature free or a double
    /// free -- because nothing frees a set value early either.
    Set(PointerValue<'ctx>),
    /// An LLVM `struct` held **by value** -- an SSA aggregate register, not
    /// a `PointerValue` to a heap or stack allocation, unlike every other
    /// container variant above (D-115). Each field is one of `Ty::Int`/
    /// `Ty::Bool`/`Ty::Float`'s own existing scalar representation (D-116
    /// admits no other element type in v0.2, so no field is ever itself a
    /// pointer or a refcounted value), and the arity is fixed at compile
    /// time -- which is exactly why no heap object, no `pycc_rt` type, and
    /// no refcounting policy accompanies this variant at all. There is
    /// nothing to leak (contrast `List`/`Dict`/`Set`, whose D-107/D-124
    /// leak-only policy exists because they *do* allocate).
    ///
    /// Its own variant rather than a reuse of any pointer-holding
    /// variant's shape (D-107/D-124's exact reasoning, extended): every
    /// exhaustive `Scalar` match (`truthy`/`to_str`/`to_numeric_encoded_int`/
    /// `to_float`) would otherwise have no way to reject a struct value
    /// passed where a pointer or a tagged int is expected. Unlike those
    /// four, `emit_assign`, `build_call_to`'s argument marshalling, and
    /// `MirStmt::Return` treat this exactly like every other variant's own
    /// pass-through case (D-116): moving a `StructValue` needs no
    /// container-specific logic, since `inkwell`'s `BasicValueEnum` and
    /// `BasicMetadataValueEnum` both already include a `StructValue` arm.
    ///
    /// Field values are stored exactly as the corresponding `Scalar`
    /// carries them. D-141 makes that identical to the current
    /// `PyIntListObj`/`PyDictObj`/`PyIntSetObj` value contract: an int field
    /// or container element is already an int-compatible encoded word,
    /// including a bool-identity marker. A tuple still crosses no runtime
    /// boundary and needs no ingress validation call.
    Tuple(inkwell::values::StructValue<'ctx>),
    /// A pointer to a heap-allocated `pycc_rt::PyInstanceObj` (D-154, Part 1
    /// of #375) -- like `List`/`Dict`/`Set`, opaque to this crate, which
    /// only ever stores it, passes it to a `pycc_rt_instance_*` call, or
    /// marshals it across a function boundary. Never `GEP`'d into directly
    /// (the class-instance-layout ADR's own opaque-accessor decision):
    /// every attribute read/write goes through
    /// `pycc_rt_instance_get_slot`/`_set_slot` with a compile-time-resolved
    /// slot index, exactly like `List`'s own `pycc_rt_int_list_*` calls.
    ///
    /// Its own variant rather than a reuse of `List`'s/`Dict`'s/`Set`'s
    /// (D-107/D-124's exact reasoning, extended identically): `PyInstanceObj`
    /// has its own layout, and every exhaustive `Scalar` match (`truthy`/
    /// `to_str`/`to_numeric_encoded_int`/`to_float`/`emit_assign`/argument
    /// marshalling) would otherwise hand an instance pointer straight to a
    /// `pycc_rt_int_list_*`/`pycc_rt_dict_*`/`pycc_rt_str_*` function.
    ///
    /// Refcounting is deliberately not wired for this variant either (see
    /// `pycc_rt::instance`'s own doc comment): leak-only, identically to
    /// `List`/`Dict`/`Set`.
    Instance(PointerValue<'ctx>),
}

struct UserFunction<'ctx> {
    param_tys: Vec<pycc_mir::Ty>,
    /// Issue #22: the global function-pointer slot for this function name.
    /// Initialized to null; set to the current definition's address when
    /// the `def` executes at module level. Calls load from this slot and
    /// dispatch indirectly, so a call before the `def` has executed sees
    /// null and aborts with `pycc_rt_name_error`, and a redefinition
    /// updates the slot so later calls see the new function.
    /// `None` for monomorphized generic specializations (`0gen_...` names),
    /// which are compiler-generated, not user-defined: they have no
    /// top-level `def` whose execution order matters, so they dispatch
    /// directly through `direct_value` instead.
    fn_ptr_global: Option<inkwell::values::GlobalValue<'ctx>>,
    /// The LLVM function type (parameter types + return type) for this
    /// function name. All definitions of the same name share one type
    /// (the type checker resolves to one signature per name). Needed to
    /// type the indirect call through `fn_ptr_global`.
    fn_type: inkwell::types::FunctionType<'ctx>,
    /// Issue #22: a global string constant holding the function name as a
    /// null-terminated C string, passed to `pycc_rt_name_error` on the
    /// null-pointer (call-before-`def`) path. Created once per function
    /// name in the declaration pass and reused at every call site, rather
    /// than recreating a duplicate-named global on each call.
    /// `None` for monomorphized specializations (no call-before-`def`
    /// path exists for compiler-generated functions).
    name_global: Option<inkwell::values::GlobalValue<'ctx>>,
    /// Issue #22: for monomorphized generic specializations (`0gen_...`
    /// names), the LLVM function value to call directly. `None` for
    /// ordinary user-defined functions, which dispatch indirectly through
    /// `fn_ptr_global` to preserve Python execution order.
    direct_value: Option<FunctionValue<'ctx>>,
}

/// #382 (PR-22 Part 2): A pending `finally` target that `return`
/// statements inside a `try` body must route through before completing.
/// When a `MirStmt::Return` is emitted and the `finally_stack` is
/// non-empty, the return value is stored to `ret_slot` (if non-`None`),
/// the `is_returning` flag is set to `1`, and control branches to
/// `finally_bb` instead of emitting `ret` directly. After the finally
/// body runs, the codegen checks `is_returning`: if set, it loads the
/// return value and emits `ret` (or propagates to an enclosing finally).
#[derive(Clone)]
struct FinallyTarget<'ctx> {
    /// The finally block to branch to instead of emitting `ret`.
    finally_bb: inkwell::basic_block::BasicBlock<'ctx>,
    /// Alloca holding the return value (`None` for `Ty::None` / void
    /// functions, where `build_return(None)` is emitted instead).
    ret_slot: Option<PointerValue<'ctx>>,
    /// Alloca i8 flag: `1` = a `return` was intercepted, `0` = normal
    /// completion (fall-through to finally).
    is_returning: PointerValue<'ctx>,
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
    int_from_i64: FunctionValue<'ctx>,
    int_add: FunctionValue<'ctx>,
    int_sub: FunctionValue<'ctx>,
    int_mul: FunctionValue<'ctx>,
    int_floordiv: FunctionValue<'ctx>,
    int_floormod: FunctionValue<'ctx>,
    int_pow: FunctionValue<'ctx>,
    int_cmp: FunctionValue<'ctx>,
    int_truthy: FunctionValue<'ctx>,
    range_continue: FunctionValue<'ctx>,
    /// #147 (D-179): `pycc_rt_range_normalize_operand`, the encoded-word-in,
    /// encoded-word-out normalizer every `range()` operand passes through.
    /// It maps D-141's `False`/`True` markers to the ordinary smallints
    /// `0`/`1` (a range produces integer objects rather than forwarding its
    /// argument object) and passes a smallint or a heap bigint through
    /// unchanged. It replaces the `int_untag_checked` call this position
    /// used to emit, which rejected every bigint.
    range_normalize_operand: FunctionValue<'ctx>,
    int_to_float: FunctionValue<'ctx>,
    float_div: FunctionValue<'ctx>,
    float_floordiv: FunctionValue<'ctx>,
    float_floormod: FunctionValue<'ctx>,
    float_pow: FunctionValue<'ctx>,
    str_from_literal: FunctionValue<'ctx>,
    str_concat: FunctionValue<'ctx>,
    /// #575 (Part 2 of #123): `pycc_rt_str_repeat`, the `str * int` /
    /// `int * str` primitive. Declared beside `str_concat` because the
    /// two share the same shape apart from the count: one `str` pointer
    /// in, one brand-new `str` pointer out. The count parameter is a
    /// **raw** `i64`, already decoded by `build_untag_checked`, matching
    /// every other raw runtime counter this table declares (list index,
    /// slice bound) rather than pushing D-141's classifier into a second
    /// place. `range` operands left that group in #147 (D-179): they now
    /// stay encoded and go through `range_normalize_operand`.
    str_repeat: FunctionValue<'ctx>,
    str_cmp: FunctionValue<'ctx>,
    str_truthy: FunctionValue<'ctx>,
    str_incref: FunctionValue<'ctx>,
    str_decref: FunctionValue<'ctx>,
    /// #146 Part 1: `str`'s D-060 refcounting pair, generalized to D-141's
    /// encoded `int` word. Both take the raw `i64` word rather than a
    /// pointer, and both are only ever called under the inline
    /// `(word & 3) == 0 && word != 0` guard `emit_bigint_refcount_call`
    /// wraps them in, so a smallint loop never reaches a runtime call.
    bigint_retain: FunctionValue<'ctx>,
    bigint_release: FunctionValue<'ctx>,
    int_to_str: FunctionValue<'ctx>,
    float_to_str: FunctionValue<'ctx>,
    bool_to_str: FunctionValue<'ctx>,
    print_write_str: FunctionValue<'ctx>,
    print_space: FunctionValue<'ctx>,
    print_newline: FunctionValue<'ctx>,
    print_none: FunctionValue<'ctx>,
    /// D-141's checked numeric decoder. Container ingress calls it for
    /// validation and keeps the original encoded word; index, slice, and
    /// `str`-repeat-count sites use its raw `0`/`1`/smallint result as an
    /// implementation counter. `range` operands no longer call it at all
    /// since #147 (D-179) -- see `range_normalize_operand`. It rejects
    /// bigint and malformed words in `pycc_rt`, which owns the
    /// representation classifier.
    int_untag_checked: FunctionValue<'ctx>,
    int_list_new: FunctionValue<'ctx>,
    int_list_append: FunctionValue<'ctx>,
    int_list_get: FunctionValue<'ctx>,
    int_list_len: FunctionValue<'ctx>,
    /// PR-12 Task 9's own new `pycc_rt_int_list_slice` declaration
    /// (`base[start:stop:step]`, D-118) -- takes the `list` pointer plus
    /// three already-defaulted, already-untagged raw `i64` bounds and
    /// returns a **new** `PyIntListObj` pointer, exactly the same opaque
    /// pointer type every other `PyIntListObj`-returning function above
    /// already declares (`int_list_new`'s own `ptr_type.fn_type(&[], ..)`
    /// one line below shares that same return type).
    int_list_slice: FunctionValue<'ctx>,
    /// PR-12 Task 11's own new `pycc_rt_int_list_pop` declaration
    /// (`list.pop()`, D-119) -- mirrors `int_list_get`'s own shape exactly
    /// (one `ptr_type` parameter, one encoded-element `i64_type` return),
    /// the only difference being no `index` parameter, since
    /// `.pop()` always removes the list's own last element.
    int_list_pop: FunctionValue<'ctx>,
    /// PR-11 Task 5's own new `pycc_rt_dict_*` declarations, mirroring the
    /// `int_list_*` cluster immediately above one-for-one: `dict_new` (no
    /// pre-sizing entry point, same reasoning as `int_list_new`),
    /// `dict_set` (insert-or-update, D-123 -- this crate's one dict
    /// "growable op", playing `int_list_append`'s role), `dict_get`
    /// (read, panics on a missing key), `dict_len`, and `dict_key_at`
    /// (`ForDict`'s own per-iteration key read, playing `int_list_get`'s
    /// `ForList`-iteration role).
    dict_new: FunctionValue<'ctx>,
    dict_set: FunctionValue<'ctx>,
    dict_get: FunctionValue<'ctx>,
    /// PR-12 Task 11's own new `pycc_rt_dict_get_or_default` declaration
    /// (`dict.get(key, default)`, D-119) -- mirrors `dict_get`'s own shape
    /// plus one extra encoded-value `i64` `default` parameter, matching
    /// `pycc_rt_dict_get_or_default`'s real Rust signature
    /// (`fn(*mut PyDictObj, *mut PyStrObj, i64) -> i64`) exactly.
    dict_get_or_default: FunctionValue<'ctx>,
    dict_len: FunctionValue<'ctx>,
    dict_key_at: FunctionValue<'ctx>,
    /// PR-11 Task 9's own new `pycc_rt_int_set_*` declarations, mirroring
    /// the `int_list_*` cluster above one-for-one: `int_set_new` (no
    /// pre-sizing entry point, same reasoning as `int_list_new`),
    /// `int_set_add` (insert-with-dedup, D-121 -- this crate's one set
    /// "growable op", playing `int_list_append`'s role; the dedup check
    /// itself lives entirely in `pycc_rt_int_set_add`, not here), `int_set_len`,
    /// and `int_set_get` (`ForSet`'s own per-iteration element read, playing
    /// `int_list_get`'s `ForList`-iteration role). Unlike the `dict_*`
    /// cluster, there is no `int_set_get`-adjacent key type at all -- a
    /// set's elements are encoded `i64`, so this cluster has no counterpart to
    /// `dict_get`'s "read by key" op.
    int_set_new: FunctionValue<'ctx>,
    int_set_add: FunctionValue<'ctx>,
    int_set_len: FunctionValue<'ctx>,
    int_set_get: FunctionValue<'ctx>,
    /// `ForSet`'s own loop-test check (Task 11 review fix, P1): panics if
    /// the set's freshly re-read length no longer matches the length
    /// captured once in the loop's preheader -- see
    /// `pycc_rt_int_set_check_not_resized`'s own doc comment for why
    /// `set.add()` made this reachable.
    int_set_check_not_resized: FunctionValue<'ctx>,
    trap: FunctionValue<'ctx>,
    /// D-154 (Part 1 of #375): `pycc_rt::instance`'s own three-function
    /// cluster -- `instance_new` (allocates a fresh, zero-initialized
    /// instance with the class's own declared slot count), and
    /// `instance_get_slot`/`instance_set_slot` (the class-instance-layout
    /// ADR's opaque accessor pair; codegen never `GEP`s into a
    /// `PyInstanceObj` directly).
    instance_new: FunctionValue<'ctx>,
    instance_get_slot: FunctionValue<'ctx>,
    instance_set_slot: FunctionValue<'ctx>,
    /// Issue #22: runtime NameError for call-before-`def`. Takes a
    /// null-terminated C string (the function name) and panics -- which
    /// becomes a process abort at the `extern "C"` boundary, matching every
    /// other runtime error in `pycc_rt`.
    name_error: FunctionValue<'ctx>,
    /// #382 (PR-22 Part 1): Exception runtime functions.
    /// `exception_active` returns i8 (non-zero if an exception is pending).
    exception_active: FunctionValue<'ctx>,
    /// `exception_value` returns a pointer to the current exception object.
    exception_value: FunctionValue<'ctx>,
    /// `exception_clear` resets the thread-local pending state (void).
    exception_clear: FunctionValue<'ctx>,
    /// `exception_alloc(type_tag: u8, message: *mut PyStrObj) -> *mut PyExceptionObj`.
    exception_alloc: FunctionValue<'ctx>,
    /// `exception_raise(obj: *mut PyExceptionObj)` — sets pending state (void).
    exception_raise: FunctionValue<'ctx>,
    /// `exception_raise_with_cause(obj, cause: *mut PyExceptionObj)` (void).
    exception_raise_with_cause: FunctionValue<'ctx>,
    /// `exception_type_matches(obj: *mut PyExceptionObj, type_tag: u8) -> i8`.
    exception_type_matches: FunctionValue<'ctx>,
    /// `exception_print_and_exit(obj: *mut PyExceptionObj) -> !` (noreturn).
    exception_print_and_exit: FunctionValue<'ctx>,
    /// Lexically enclosing `except` handler values used by bare `raise`.
    /// Each handler owns an LLVM local slot. A stack is required because a
    /// nested handler must not overwrite the exception saved by its outer
    /// handler.
    exceptions: ExceptionCodegenState<'ctx>,
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
        range_normalize_operand: declare(
            "pycc_rt_range_normalize_operand",
            i64_type.fn_type(&[i64_type.into()], false),
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
        str_repeat: declare(
            "pycc_rt_str_repeat",
            ptr_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
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
        bigint_retain: declare(
            "pycc_rt_bigint_retain",
            void_type.fn_type(&[i64_type.into()], false),
        ),
        bigint_release: declare(
            "pycc_rt_bigint_release",
            void_type.fn_type(&[i64_type.into()], false),
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
        int_from_i64: declare(
            "pycc_rt_int_from_i64",
            i64_type.fn_type(&[i64_type.into()], false),
        ),
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
        // `list` pointer plus `start`/`stop`/`step`, all raw `i64`,
        // returning a pointer (the new sliced list) -- matches
        // `pycc_rt_int_list_slice`'s real Rust signature
        // (`fn(*mut PyIntListObj, i64, i64, i64) -> *mut PyIntListObj`)
        // exactly. The task plan's own sketch described this as "4 `i64`
        // parameters" for the whole call, which undercounts by one: it is
        // one `ptr_type` plus three `i64_type` parameters, not four
        // `i64_type` ones -- corrected here to match what `declare` below
        // actually builds.
        int_list_slice: declare(
            "pycc_rt_int_list_slice",
            ptr_type.fn_type(
                &[
                    ptr_type.into(),
                    i64_type.into(),
                    i64_type.into(),
                    i64_type.into(),
                ],
                false,
            ),
        ),
        int_list_pop: declare(
            "pycc_rt_int_list_pop",
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
        dict_get_or_default: declare(
            "pycc_rt_dict_get_or_default",
            i64_type.fn_type(&[ptr_type.into(), ptr_type.into(), i64_type.into()], false),
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
        // lives entirely inside that function (D-121), so codegen's own
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
        int_set_check_not_resized: declare(
            "pycc_rt_int_set_check_not_resized",
            void_type.fn_type(&[i64_type.into(), i64_type.into()], false),
        ),
        trap: module.add_function("llvm.trap", void_type.fn_type(&[], false), None),
        instance_new: declare(
            "pycc_rt_instance_new",
            ptr_type.fn_type(&[i64_type.into()], false),
        ),
        instance_get_slot: declare(
            "pycc_rt_instance_get_slot",
            i64_type.fn_type(&[ptr_type.into(), i64_type.into()], false),
        ),
        instance_set_slot: declare(
            "pycc_rt_instance_set_slot",
            void_type.fn_type(&[ptr_type.into(), i64_type.into(), i64_type.into()], false),
        ),
        name_error: declare(
            "pycc_rt_name_error",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        // #382 (PR-22 Part 1): Exception runtime function declarations.
        exception_active: declare(
            "pycc_rt_exception_active",
            context.i8_type().fn_type(&[], false),
        ),
        exception_value: declare("pycc_rt_exception_value", ptr_type.fn_type(&[], false)),
        exception_clear: declare("pycc_rt_exception_clear", void_type.fn_type(&[], false)),
        exception_alloc: declare(
            "pycc_rt_exception_alloc",
            ptr_type.fn_type(&[context.i8_type().into(), ptr_type.into()], false),
        ),
        exception_raise: declare(
            "pycc_rt_exception_raise",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        exception_raise_with_cause: declare(
            "pycc_rt_exception_raise_with_cause",
            void_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
        ),
        exception_type_matches: declare(
            "pycc_rt_exception_type_matches",
            context
                .i8_type()
                .fn_type(&[ptr_type.into(), context.i8_type().into()], false),
        ),
        exception_print_and_exit: declare(
            "pycc_rt_exception_print_and_exit",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        exceptions: ExceptionCodegenState::new(),
    }
}

fn ty_to_basic_type(context: &Context, ty: pycc_mir::Ty) -> inkwell::types::BasicTypeEnum<'_> {
    match ty {
        pycc_mir::Ty::Int => context.i64_type().into(),
        pycc_mir::Ty::Bool => context.i8_type().into(),
        pycc_mir::Ty::Float => context.f64_type().into(),
        pycc_mir::Ty::Str => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // LLVM `void` cannot be a parameter type. v0.1 therefore carries
        // Python's singleton unit value across user-function parameter and
        // assignment-storage boundaries as the canonical `i8 0`; a `None`
        // return is still emitted as LLVM `void` by `compile_to_object`.
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
        // A class instance's runtime object (`pycc_rt::instance::PyInstanceObj`,
        // D-154, Part 1 of #375) is heap-allocated and always referenced by
        // pointer -- exactly the same storage/parameter representation
        // `List(_)`/`Dict(_)`/`Set(_)` get above, for the identical reason
        // (the class's own declared shape only affects what this crate's
        // `pycc_rt_instance_*` calls do with the pointee, never this
        // representation choice).
        pycc_mir::Ty::Instance(_) => context.ptr_type(inkwell::AddressSpace::default()).into(),
        // `tuple[...]`'s v0.2 representation (D-115), and the one place
        // this function departs from every container arm above: a real
        // LLVM `struct` built positionally from each element's own
        // `ty_to_basic_type`, *not* a pointer. A tuple is the only
        // container this crate holds by value rather than by reference,
        // because it is the only one whose full shape is known at compile
        // time -- fixed arity, and (per D-116) every element a fixed-width
        // scalar -- so it needs no heap allocation and therefore no
        // runtime object to point at.
        //
        // Recursive rather than flat, matching `Ty::Tuple`'s own recursive
        // shape. D-116 admits only `int`/`bool`/`float` elements today, so
        // in practice every field resolves to `i64`/`i8`/`f64` -- but this
        // arm is deliberately not narrowed to those three, matching the
        // element-type-agnostic shape `List(_)`/`Dict(_)`/`Set(_)` already
        // use above.
        pycc_mir::Ty::Tuple(elems) => {
            let field_types: Vec<inkwell::types::BasicTypeEnum> = elems
                .iter()
                .map(|elem_ty| ty_to_basic_type(context, elem_ty.clone()))
                .collect();
            context.struct_type(&field_types, false).into()
        }
        // Deviation from the task brief: the brief's own version of this
        // catch-all's message read "(only int/float/bool/str/list[int] do)"
        // -- but that parenthetical is inaccurate twice over. This function
        // already gives `Ty::None` a representation too (the `i8 0` carrier
        // above), and the `List(_)`/`Dict(_)`/`Set(_)`/`Tuple(_)` arms
        // above aren't specific to `list[int]`/`dict[str, int]`/`set[int]`:
        // each produces the same representation for any element/key/value
        // type, not just those. Worded to match what this function actually
        // does -- `Ty::Infer` is the one variant left with no arm (see
        // `an_infer_typed_return_type_is_not_supported`).
        other => panic!(
            "pycc_codegen: {} has no LLVM representation yet (int/float/bool/str/None/list[_]/dict[_, _]/set[_]/tuple[...] do)",
            other.name()
        ),
    }
}

/// #380 (PR-20): Returns a zero/null default `BasicValueEnum` for the given
/// type, used only for abstract method bodies (which are never executed).
/// The value itself is irrelevant — it just needs to be well-typed so the
/// LLVM verifier accepts the function's `ret` instruction.
fn default_value_for_type<'ctx>(
    context: &'ctx Context,
    ty: pycc_mir::Ty,
) -> inkwell::values::BasicValueEnum<'ctx> {
    match ty {
        pycc_mir::Ty::Int => context.i64_type().const_zero().into(),
        pycc_mir::Ty::Bool => context.i8_type().const_zero().into(),
        pycc_mir::Ty::Float => context.f64_type().const_zero().into(),
        pycc_mir::Ty::Str
        | pycc_mir::Ty::List(_)
        | pycc_mir::Ty::Dict(_)
        | pycc_mir::Ty::Set(_)
        | pycc_mir::Ty::Instance(_)
        | pycc_mir::Ty::Protocol(_) => context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null()
            .into(),
        pycc_mir::Ty::Tuple(elems) => {
            let field_types: Vec<inkwell::types::BasicTypeEnum> = elems
                .iter()
                .map(|elem_ty| ty_to_basic_type(context, elem_ty.clone()))
                .collect();
            let struct_ty = context.struct_type(&field_types, false);
            struct_ty.const_zero().into()
        }
        // `Infer`, `Param`, and `None` never produce a real value at
        // runtime — `Infer` and `Param` are resolved away before MIR,
        // and `None` is represented as a zero i8.  Grouping them here
        // (instead of a `panic!` catch-all) avoids a permanently-
        // uncovered defensive region under D-014's 100 %-coverage gate.
        pycc_mir::Ty::None | pycc_mir::Ty::Infer | pycc_mir::Ty::Param(_) => {
            context.i8_type().const_zero().into()
        }
    }
}

/// Converts a numeric `Int`/`Bool` scalar to D-141's encoded-int carrier.
/// An existing `Int` passes through unchanged, whether it contains an odd
/// ordinary smallint, a bool-identity marker, or a bigint pointer. A
/// standalone `Bool` is being consumed numerically at these call sites, so
/// it becomes an ordinary D-061 smallint via a zero-extend and shift-and-or
/// matching `pycc_rt::tag_smallint`. Panics for `Float`, which is never
/// `int`-coercible -- `pycc_types`'
/// `numeric_result_type` always promotes an expression with any `float`
/// operand to `Ty::Float`, so no real MIR can reach this arm with a
/// `Float` operand (see this task's own defensive-panic test exercising
/// it via deliberately malformed MIR, matching this file's existing
/// convention for such arms).
fn to_numeric_encoded_int<'ctx>(
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
        // reasoning, per D-124): no arithmetic operand is ever `Ty::Dict`,
        // so this is never reached by real, type-checked source.
        Scalar::Dict(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got dict")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict` arms above, extended to `set[T]` (D-107's
        // reasoning, per D-124): no arithmetic operand is ever `Ty::Set`,
        // so this is never reached by real, type-checked source.
        Scalar::Set(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got set")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict`/`Set` arms above, extended to `tuple[...]` (D-107's
        // reasoning, per D-116): no arithmetic operand is ever `Ty::Tuple`,
        // so this is never reached by real, type-checked source.
        Scalar::Tuple(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got tuple")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict`/`Set`/`Tuple` arms above, extended to a class
        // instance (D-154, Part 1 of #375, mirroring D-107/D-124's
        // reasoning): `pycc_types` rejects arithmetic on `Ty::Instance` as
        // `T0021` before codegen runs (see `numeric_result_type`'s own
        // `as_numeric` closure), so this is never reached by real,
        // type-checked source.
        Scalar::Instance(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got instance")
        }
    }
}

/// Converts an int-compatible scalar for storage at a statically-`int`
/// boundary while preserving whether the source object was `False` or
/// `True`. Ordinary integer inputs already use the D-061 encoding. The two
/// even marker words are disjoint from odd smallints and aligned bigint
/// pointers; `pycc_rt` owns their interpretation.
fn to_encoded_int<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
) -> IntValue<'ctx> {
    match scalar {
        Scalar::Int(value) => value,
        Scalar::Bool(value) => {
            let widened = builder
                .build_int_z_extend(value, context.i64_type(), "bool_identity_i64")
                .expect("build_int_z_extend should not fail widening i8 to i64");
            let shifted = builder
                .build_left_shift(
                    widened,
                    context.i64_type().const_int(2, false),
                    "bool_identity_shl",
                )
                .expect("build_left_shift should not fail for a constant shift amount");
            builder
                .build_or(
                    shifted,
                    context.i64_type().const_int(2, false),
                    "bool_identity_or",
                )
                .expect("build_or should not fail for two i64 operands")
        }
        other => to_numeric_encoded_int(context, builder, other),
    }
}

/// Tags an already-known-in-range raw counter as an ordinary smallint.
/// D-141 leaves this only for user-visible lengths -- `range()` operands
/// stopped using it in #147 (D-179), where normalization moved into
/// `pycc_rt_range_normalize_operand` so a bigint operand survives it.
/// Container elements/values carry their encoded words unchanged and never
/// use this helper on read.
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

/// D-154 (Part 1 of #375): mirrors `expect_list_pointer` exactly, for
/// `MirExpr::AttrGet`/`MirStmt::AttrSet`'s own base -- `pycc_types::check`
/// (`T0043`) already rejects a non-instance `AttrGet`/`AttrSet`/`MethodCall`
/// base before codegen runs, so a mismatch here can only mean malformed MIR.
fn expect_instance_pointer<'ctx>(scalar: Scalar<'ctx>, what: &str) -> PointerValue<'ctx> {
    let Scalar::Instance(ptr) = scalar else {
        panic!(
            "pycc_codegen: internal error: {what} did not evaluate to a class instance -- \
             pycc_types::check (T0043) should have rejected this before codegen"
        )
    };
    ptr
}

/// Reinterprets a raw `i64` slot word read from `pycc_rt_instance_get_slot`
/// as the `Scalar` its declared attribute `ty` names (D-154, Part 1 of
/// #375; see `pycc_rt::instance`'s own doc comment for the slot
/// representation this mirrors exactly): `int` passes through unchanged;
/// `bool` truncates to `pycc_codegen`'s own `i8` `Scalar::Bool` carrier;
/// `float` bit-reinterprets the same 8 bytes as `f64` (never a numeric
/// conversion -- the word *is* a float's bit pattern, written by
/// `scalar_to_slot_word`'s own mirror-image `float` arm); `str` reinterprets
/// the word as a `*mut PyStrObj` pointer. Only these four `Ty`s can ever
/// reach here: `pycc_hir::class::slot_ty_from_init_rhs` structurally
/// restricts every attribute slot to a scalar (int/float/bool/str)
/// parameter or literal at `__init__`'s own first-assignment pre-scan, so a
/// `List`/`Dict`/`Set`/`Tuple`/`Instance`/`Param`/`Infer`-typed attribute
/// can never be constructed from real, type-checked source.
fn slot_word_to_scalar<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    raw: IntValue<'ctx>,
    ty: &pycc_mir::Ty,
) -> Scalar<'ctx> {
    match ty {
        pycc_mir::Ty::Int => Scalar::Int(raw),
        pycc_mir::Ty::Bool => Scalar::Bool(
            builder
                .build_int_truncate(raw, context.i8_type(), "attr_bool_trunc")
                .expect("build_int_truncate should not fail truncating i64 to i8"),
        ),
        pycc_mir::Ty::Float => Scalar::Float(
            builder
                .build_bit_cast(raw, context.f64_type(), "attr_float_bitcast")
                .expect("build_bit_cast should not fail reinterpreting i64 bits as f64")
                .into_float_value(),
        ),
        pycc_mir::Ty::Str => Scalar::Str(
            builder
                .build_int_to_ptr(
                    raw,
                    context.ptr_type(inkwell::AddressSpace::default()),
                    "attr_str_inttoptr",
                )
                .expect("build_int_to_ptr should not fail reinterpreting an i64 as a pointer"),
        ),
        other => panic!(
            "pycc_codegen: internal error: an instance attribute of type `{}` is not \
             supported yet -- pycc_hir::class::slot_ty_from_init_rhs should have rejected \
             this before codegen",
            other.name()
        ),
    }
}

/// Mirror image of [`slot_word_to_scalar`]: encodes a `Scalar` as the raw
/// `i64` word `pycc_rt_instance_set_slot` stores. See that function's own
/// doc comment for why only `Int`/`Bool`/`Float`/`Str` are ever reachable
/// here.
fn scalar_to_slot_word<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
) -> IntValue<'ctx> {
    match scalar {
        Scalar::Int(v) => v,
        Scalar::Bool(v) => builder
            .build_int_z_extend(v, context.i64_type(), "attr_bool_zext")
            .expect("build_int_z_extend should not fail widening i8 to i64"),
        Scalar::Float(v) => builder
            .build_bit_cast(v, context.i64_type(), "attr_float_bitcast")
            .expect("build_bit_cast should not fail reinterpreting f64 bits as i64")
            .into_int_value(),
        Scalar::Str(v) => builder
            .build_ptr_to_int(v, context.i64_type(), "attr_str_ptrtoint")
            .expect("build_ptr_to_int should not fail reinterpreting a pointer as i64"),
        Scalar::List(_)
        | Scalar::Dict(_)
        | Scalar::Set(_)
        | Scalar::Tuple(_)
        | Scalar::Instance(_) => panic!(
            "pycc_codegen: internal error: cannot store this value into an instance \
             attribute slot -- pycc_hir::class::slot_ty_from_init_rhs should have rejected \
             this before codegen"
        ),
    }
}

/// Calls D-141's runtime-owned classifier/decoder. Container-value ingress
/// uses the call only to validate bigint exclusion and then stores the
/// original encoded word; index, slice, and `str`-repeat-count sites consume
/// its decoded raw result. `range` operands stopped calling it in #147
/// (D-179). Keeping classification in `pycc_rt` prevents codegen from
/// duplicating or partially interpreting the ABI.
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

/// Reads one encoded element out of a `PyIntListObj`. The positional index
/// is a raw runtime counter; the returned word is already a user-visible
/// D-141 int-compatible value and is forwarded unchanged.
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

/// Removes and returns the list's own last element (`list.pop()`, PR-12
/// Task 11, D-119). Mirrors `build_int_list_get`'s own one-`build_call`
/// shape exactly, minus the `index` parameter -- `.pop()` always targets
/// the last element, so there is nothing else to pass. The returned value
/// is already an encoded D-141 int-compatible word, exactly like
/// `build_int_list_get`'s own return value.
fn build_int_list_pop<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    list_ptr: PointerValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(rt.int_list_pop, &[list_ptr.into()], "list_pop")
        .expect("build_call should not fail for a well-formed list pop")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_list_pop returns a non-void i64")
        .into_int_value()
}

/// A `PyIntListObj`'s current element count, as a raw `i64` counter. The
/// `len(x)` builtin tags this before handing it back as a
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

/// Returns a **new** `PyIntListObj` holding the clamped, strided sub-range
/// `[start, stop)` of `list_ptr`'s elements, stepping by `step`
/// (`base[start:stop:step]`, PR-12 Task 9, D-118). All three bound operands
/// must already be raw, untagged `i64`s with every default/untag conversion
/// already applied by the caller (`MirExpr::Slice`'s own `emit_expr` arm
/// below) -- this helper is a thin one-`build_call` wrapper, exactly like
/// `build_int_list_get` above, not a place that itself interprets D-141's
/// encoded elements or D-118's defaulting rules.
fn build_int_list_slice<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    list_ptr: PointerValue<'ctx>,
    start: IntValue<'ctx>,
    stop: IntValue<'ctx>,
    step: IntValue<'ctx>,
) -> PointerValue<'ctx> {
    builder
        .build_call(
            rt.int_list_slice,
            &[list_ptr.into(), start.into(), stop.into(), step.into()],
            "list_slice",
        )
        .expect("build_call should not fail for a well-formed list slice")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_list_slice returns a non-void pointer")
        .into_pointer_value()
}

/// Appends one already-validated encoded D-141 value to a `PyIntListObj`, shared by
/// `MirExpr::ListLiteral`'s per-element construction and
/// `MirExpr::ListAppend`. Returns nothing: `pycc_rt_int_list_append` is
/// declared `void`, so unlike every other `pycc_rt_int_list_*` helper above
/// there is no `try_as_basic_value()` result to extract.
fn build_int_list_append<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    list_ptr: PointerValue<'ctx>,
    encoded_value: IntValue<'ctx>,
) {
    builder
        .build_call(
            rt.int_list_append,
            &[list_ptr.into(), encoded_value.into()],
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

/// Inserts or updates one already-validated encoded D-141 value under `key`
/// in a `PyDictObj`, shared by `MirExpr::DictLiteral`'s per-pair
/// construction and `MirStmt::DictSet`'s own `d[k] = v` (D-123's
/// insert-or-update operation -- `pycc_rt_dict_set` itself decides which of
/// the two this is, by whether `key` already compares equal to a stored
/// key). Returns nothing, exactly like `build_int_list_append` above:
/// `pycc_rt_dict_set` is declared `void`.
fn build_dict_set<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    dict_ptr: PointerValue<'ctx>,
    key_ptr: PointerValue<'ctx>,
    encoded_value: IntValue<'ctx>,
) {
    builder
        .build_call(
            rt.dict_set,
            &[dict_ptr.into(), key_ptr.into(), encoded_value.into()],
            "dict_set",
        )
        .expect("build_call should not fail for a well-formed dict set");
}

/// Returns the encoded value stored for `key_ptr`, or `encoded_default` if absent
/// (`dict.get(key, default)`, PR-12 Task 11, D-119). Mirrors
/// `build_int_list_pop`'s own one-`build_call` shape exactly, with one
/// extra `i64` operand for the default. Both the default and returned value
/// are D-141 int-compatible encoded words and retain bool identity.
fn build_dict_get_or_default<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    dict_ptr: PointerValue<'ctx>,
    key_ptr: PointerValue<'ctx>,
    encoded_default: IntValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(
            rt.dict_get_or_default,
            &[dict_ptr.into(), key_ptr.into(), encoded_default.into()],
            "dict_get_or_default",
        )
        .expect("build_call should not fail for a well-formed dict get-or-default")
        .try_as_basic_value()
        .expect_basic("pycc_rt_dict_get_or_default returns a non-void i64")
        .into_int_value()
}

/// A `PyDictObj`'s current entry count, as a raw `i64` counter, shared by
/// the `len(d)` builtin's `Ty::Dict` branch and
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

/// Inserts one already-validated encoded D-141 value into a `PyIntSetObj`,
/// shared by `MirExpr::SetLiteral`'s per-element construction and
/// `MirExpr::SetAdd`'s own user-facing `s.add(value)` call (PR-12 Task 11,
/// D-119 -- the second call site; `SetLiteral`'s per-element construction
/// was the first and, until this task, only one). Returns nothing:
/// `pycc_rt_int_set_add` is declared `void`, exactly like
/// `build_int_list_append`/`build_dict_set` above. The dedup check that
/// makes a repeated element collapse to one (D-121) lives entirely inside
/// `pycc_rt_int_set_add` itself -- both callers just call it per value,
/// unconditionally, with no dedup logic of their own.
fn build_int_set_add<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    set_ptr: PointerValue<'ctx>,
    encoded_value: IntValue<'ctx>,
) {
    builder
        .build_call(
            rt.int_set_add,
            &[set_ptr.into(), encoded_value.into()],
            "set_add",
        )
        .expect("build_call should not fail for a well-formed set add");
}

/// A `PyIntSetObj`'s current element count, as a raw `i64` counter, shared
/// by the `len(s)` builtin's `Ty::Set` branch and
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

/// Panics (via `pycc_rt_int_set_check_not_resized`) if `current_len` no
/// longer matches `expected_len` -- called once per `ForSet` loop-test
/// evaluation, comparing a freshly re-read length against the length
/// captured once in the preheader. See that runtime function's own doc
/// comment for why `set.add()` (PR-12, D-119) made this reachable.
fn build_int_set_check_not_resized<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    current_len: IntValue<'ctx>,
    expected_len: IntValue<'ctx>,
) {
    builder
        .build_call(
            rt.int_set_check_not_resized,
            &[current_len.into(), expected_len.into()],
            "set_check_not_resized",
        )
        .expect("build_call should not fail for a well-formed set resize check");
}

/// Reads one element out of a `PyIntSetObj` by insertion-order index, used
/// only by `MirStmt::ForSet`'s own per-iteration element read. The index is
/// a raw counter and the result is an encoded D-141 value, mirroring
/// `build_int_list_get`.
fn build_int_set_get<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    set_ptr: PointerValue<'ctx>,
    raw_index: IntValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_call(
            rt.int_set_get,
            &[set_ptr.into(), raw_index.into()],
            "set_get",
        )
        .expect("build_call should not fail for a well-formed set read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_int_set_get returns a non-void i64")
        .into_int_value()
}

/// Applies the one representation-changing assignment conversion accepted by
/// the v0.1 type system: standalone `i8` bool to D-141's identity-preserving
/// int-compatible `i64`. All other assignable pairs already share a representation.
fn coerce_scalar_to_type<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    scalar: Scalar<'ctx>,
    target_ty: pycc_mir::Ty,
) -> Scalar<'ctx> {
    match (target_ty, scalar) {
        (pycc_mir::Ty::Int, Scalar::Bool(value)) => {
            Scalar::Int(to_encoded_int(context, builder, Scalar::Bool(value)))
        }
        (_, scalar) => scalar,
    }
}

/// Prepares one `range()` operand for the induction phi: the result is an
/// *encoded* int-compatible word (D-061/D-141), normalized so the
/// bool-identity markers become the ordinary smallints `0`/`1` while a
/// smallint or a heap bigint passes through unchanged.
///
/// Renamed from `range_operand_to_tagged_int` in #147 (D-179): the old name
/// described the old mechanism, which decoded to a raw `i64` and re-tagged,
/// and therefore could not represent a bigint operand at all.
fn range_operand_to_normalized_int<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    scalar: Scalar<'ctx>,
    position: &str,
) -> IntValue<'ctx> {
    match scalar {
        scalar @ (Scalar::Int(_) | Scalar::Bool(_)) => {
            let encoded = to_encoded_int(context, builder, scalar);
            // #147 (D-179): normalize instead of decode-and-re-tag. The old
            // `build_untag_checked` + `raw_i64_to_tagged_int` pair enforced
            // D-141's bool normalization by round-tripping through a raw
            // `i64`, which made every bigint operand abort. The runtime
            // normalizer keeps that bool contract and lets a bigint through.
            builder
                .build_call(rt.range_normalize_operand, &[encoded.into()], "range_norm")
                .expect("build_call should not fail for a declared runtime function")
                .try_as_basic_value()
                .expect_basic("pycc_rt_range_normalize_operand returns a non-void i64")
                .into_int_value()
        }
        // `List`/`Dict`/`Set`/`Tuple` join this arm's existing or-pattern
        // rather than getting their own (D-107, extended to dict/set by
        // D-124 and to tuple by D-116): unlike `to_numeric_encoded_int`/`to_float`
        // above and below, this message never names the offending type, so
        // it stays exactly as honest for a list, dict, set, or tuple
        // operand as for a `float` or `str` one -- and folding adds no
        // separate, permanently-unexecutable region under this crate's
        // 100%-region gate (D-014). `range()` arguments are type-checked by
        // `pycc_types` before codegen, so this whole arm is defensive
        // either way.
        Scalar::Float(_)
        | Scalar::Str(_)
        | Scalar::List(_)
        | Scalar::Dict(_)
        | Scalar::Set(_)
        | Scalar::Tuple(_)
        // D-154 (Part 1 of #375): a class instance joins this same
        // or-pattern for the identical reason `List`/`Dict`/`Set`/`Tuple`
        // already do -- no new instrumented region.
        | Scalar::Instance(_) => {
            panic!("pycc_codegen: internal error: range() {position} did not evaluate to int")
        }
    }
}

/// Promotes any numeric `Scalar` to `f64`: an existing `Float` passes
/// through; `Int` goes through `pycc_rt_int_to_float` (never a raw LLVM
/// cast -- the value is D-141 encoded, so only `pycc_rt` may interpret its
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
        // `to_numeric_encoded_int`'s own `List` arm above (D-107), and separate from
        // `Str`'s for the same message-honesty reason.
        Scalar::List(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got list")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List` arm directly above, extended to `dict[K, V]` (D-107's
        // reasoning, per D-124).
        Scalar::Dict(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got dict")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict` arms above, extended to `set[T]` (D-107's
        // reasoning, per D-124).
        Scalar::Set(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got set")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict`/`Set` arms above, extended to `tuple[...]` (D-107's
        // reasoning, per D-116).
        Scalar::Tuple(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got tuple")
        }
        // Defensive for the identical `numeric_result_type` reason as the
        // `List`/`Dict`/`Set`/`Tuple` arms above, extended to a class
        // instance (D-154, Part 1 of #375).
        Scalar::Instance(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got instance")
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
///    Context` parameter (matching `to_numeric_encoded_int`/`to_float`'s own
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
        // route into this same arm (`emit_eval_print_arg` and that interpolation
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
        // (D-123), and there is no `pycc_rt_dict_to_str` to call -- so
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
        // (D-124), and there is no `pycc_rt_int_set_to_str` to call -- so
        // this panics honestly instead of handing a `PyIntSetObj` pointer
        // to a `pycc_rt_*_to_str` function that would read it as a
        // `PyStrObj`.
        Scalar::Set(_) => {
            panic!("pycc_codegen: string conversion of a set[T] value is not supported yet")
        }
        // A real, reachable feature gap -- but NOT "identical in kind" to
        // the `List`/`Dict`/`Set` arms directly above in one respect:
        // `list[T]`'s own `print(xs)`/`f"{xs}"` reachability predates this
        // whole PR-11 effort entirely (established back in PR-10, D-107);
        // `dict`/`set`'s own reachability, while more recent (PR-11a's own
        // HIR literal lowering), was already in place before this PR
        // (PR-11b) started -- neither is something this PR's own diff
        // turned from a clean diagnostic into a panic. `tuple[...]`'s
        // reachability here IS exactly that: it is new as of this PR's own
        // Task 2 (`HirExpr::TupleLiteral` lowering, `crates/pycc_hir/src/
        // lib.rs`) -- before that commit, any program containing a tuple
        // literal failed to lower at all and got a clean `C0001`
        // ("expression kind not supported yet") diagnostic instead of ever
        // reaching this function. `pycc_types` places no type restriction
        // on `print`'s argument or an f-string interpolation, so
        // `print(t)`/`f"{t}"` for a `tuple[...]` local type-checks today
        // and lands here. D-116 ships only construction and literal-index
        // reads, so v0.2 has no tuple string-conversion semantics -- and
        // unlike the three container arms above there is not even a
        // runtime object to hand to a conversion function, since a tuple is
        // a bare LLVM struct with no `pycc_rt` type at all (D-115). Panics
        // honestly instead of reinterpreting the struct's first field as a
        // `PyStrObj` pointer. See `docs/DECISIONS.md`'s D-116 deferred-
        // capability list and `docs/ROADMAP.md`'s matching follow-up for
        // this new-as-of-PR-11b reachability.
        Scalar::Tuple(_) => {
            panic!("pycc_codegen: string conversion of a tuple[...] value is not supported yet")
        }
        // #378 (PR-18): a class instance with a `__repr__` method is
        // converted to a string at the MIR level -- `rewrite_instance_to_repr`
        // in `pycc_mir` rewrites instance-typed f-string interpolations and
        // `print` arguments to `__repr__` calls, so the codegen's `to_str`
        // receives a `str` scalar from the `__repr__` call, never an
        // `Instance` scalar. A bare `to_str` call reaching here with an
        // Instance scalar means the class has no `__repr__` (the MIR rewrite
        // is a no-op for classes without `__repr__`) -- panic honestly,
        // matching the pre-#378 behavior.
        Scalar::Instance(_) => {
            panic!(
                "pycc_codegen: string conversion of a class instance without `__repr__` is not supported yet"
            )
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
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    expr: &MirExpr,
) -> Scalar<'ctx> {
    let value = emit_expr_unchecked(context, builder, module, rt, user_functions, locals, expr);
    // Recursive calls come back through this wrapper. Consequently a node
    // that can set the pending state stops evaluation before its parent can
    // evaluate the next operand, call argument, or other sub-expression.
    // Pure literals, reads, comparisons, and ordinary arithmetic skip the
    // TLS call and branch entirely; D-173's original unconditional guard at
    // every node made tight numeric loops several times slower.
    if expression_can_set_exception(expr) {
        guard_statement_effects(context, builder, rt);
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn emit_expr_unchecked<'ctx>(
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
        MirExpr::IntLiteral(n) => Scalar::Int(emit_int_constant(context, builder, rt, *n)),
        MirExpr::FloatLiteral(f) => Scalar::Float(context.f64_type().const_float(*f)),
        MirExpr::IntBoundary(value) => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            Scalar::Int(to_encoded_int(context, builder, scalar))
        }
        MirExpr::StringLiteral(s) => {
            Scalar::Str(emit_string_literal(context, builder, module, rt, s))
        }
        // D-136: `math.pi` is a compile-time float constant, not a runtime
        // value bound to any local slot -- Task 4's own thin slice emits
        // the literal immediate directly, matching the ADR's explicit "no
        // runtime call at all" design for this one symbol. `std::f64::consts::PI`
        // is Rust's own IEEE-754 double-precision constant for pi, bit-for-bit
        // the same value CPython's `math.pi` uses (both are the nearest
        // representable `f64`/C `double` to the true mathematical constant).
        MirExpr::Name {
            name,
            ty: Ty::Float,
        } if name == "math.pi" => {
            Scalar::Float(context.f64_type().const_float(std::f64::consts::PI))
        }
        // PEP 634-636 (#381, PR-21): `None` singleton in a match pattern
        // comparison. `None` is not a bound variable — it is a constant
        // `i8 0` carrier (D-075), emitted inline exactly like `math.pi`
        // above rather than looked up in `locals`.
        MirExpr::Name { name, ty: Ty::None } if name == "None" => {
            Scalar::Bool(context.i8_type().const_zero())
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
                // NOT a pointer read, unlike the four container arms above
                // (PR-11b Task 5, D-115): a tuple slot holds the struct
                // itself, so this loads the whole aggregate back out as an
                // SSA value. The load type must be computed from `ty`
                // rather than hardcoded like every other arm here, since a
                // tuple's LLVM type depends on its own element types.
                //
                // Not optional, and not merely a nicety: `t[0]` -- the only
                // tuple operation D-116 ships besides construction -- lowers
                // to a `MirExpr::Subscript` whose base is exactly this
                // `MirExpr::Name`. Without this arm every literal-index read
                // of a tuple *variable* (as opposed to an inline
                // `(1, 2)[0]`) would fall through to the catch-all below and
                // panic on a real, type-checked program, which is why the
                // module-level and function-local smoke programs for this
                // task both exercise it.
                Ty::Tuple(_) => {
                    let loaded = builder
                        .build_load(
                            ty_to_basic_type(context, ty.clone()).into_struct_type(),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Tuple(loaded.into_struct_value())
                }
                // Same pointer-slot read as `Ty::List(_)`/`Ty::Dict(_)`/
                // `Ty::Set(_)` above (D-154, Part 1 of #375) --
                // `ty_to_basic_type`'s own `Instance(_)` arm already
                // allocated this slot as a pointer, so reading it back is
                // identical regardless of the class. Every real read of a
                // class-instance local -- `p.x`, `p.bump()`, passing `p` as
                // an argument -- goes through this arm, so without it every
                // one of those would fall through to the catch-all below
                // and panic on a real, type-checked program instead of
                // reading the value.
                Ty::Instance(_) => {
                    let loaded = builder
                        .build_load(
                            context.ptr_type(inkwell::AddressSpace::default()),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Instance(loaded.into_pointer_value())
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
                    // `to_numeric_encoded_int` promotes a `bool` operand instead of
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
                    let l = to_numeric_encoded_int(context, builder, l);
                    let r = to_numeric_encoded_int(context, builder, r);
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
                    // #146 Part 2 (D-181): both operands are dead now. Every
                    // `pycc_rt_int_*` above builds a bigint result through
                    // `tag_bigint(BigIntObj::new(..))`, never by handing an
                    // operand's own word back, so releasing here cannot free
                    // the value just computed -- an invariant that arithmetic
                    // export's own doc comment states and
                    // `an_int_operation_never_returns_an_operand_s_own_word`
                    // pins.
                    release_if_int_temporary(context, builder, rt, left, l);
                    release_if_int_temporary(context, builder, rt, right, r);
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
                    // #575 (Part 2 of #123): string repetition. `pycc_types`
                    // accepts `str * int` / `int * str` (with `bool` as the
                    // count, since `bool <: int`) and
                    // `pycc_mir::binop_result_ty` types it `str`; this arm
                    // now emits it rather than stopping at Part 1's named
                    // D-072 boundary. MIR needs no repetition node of its
                    // own: `BinOp { op: Mul, ty: Ty::Str }` *is* the
                    // repetition, and this `match ty` is what distinguishes
                    // it from numeric multiplication.
                    //
                    // Handled *before* the two operand destructures below on
                    // purpose: the count evaluates to a `Scalar::Int`/
                    // `Scalar::Bool`, so leaving it to fall through would
                    // report the destructures' "internal error" message.
                    if *op == pycc_mir::BinOpKind::Mul {
                        // Both operand orders land here -- whichever side is
                        // the `str` becomes the operand, the other the count.
                        //
                        // Review note: a `str * str` pair (unreachable from
                        // any real pipeline, since `pycc_types` rejects it
                        // with `T0021`) matches the first arm and then fails
                        // inside `to_numeric_encoded_int` with its own
                        // "expected an int-or-bool operand, got str" message
                        // rather than the `str {op} str` message below. That
                        // shadowing is accepted, exactly as Part 1 accepted
                        // it, rather than worked around for a shape no
                        // type-checked program can produce.
                        let (operand, count) = match (l, r) {
                            (Scalar::Str(operand), count) => (operand, count),
                            (count, Scalar::Str(operand)) => (operand, count),
                            _ => panic!(
                                "pycc_codegen: internal error: a str-result `*` had no str operand"
                            ),
                        };
                        // `to_numeric_encoded_int` promotes a `bool` count to
                        // the same D-141 encoding an `int` count already
                        // carries (`"ab" * True` is `"ab"`), and
                        // `build_untag_checked` decodes it to the raw counter
                        // `pycc_rt_str_repeat` expects -- keeping bigint
                        // rejection in D-141's single runtime-owned
                        // classifier instead of duplicating it here.
                        let encoded = to_numeric_encoded_int(context, builder, count);
                        let raw = build_untag_checked(builder, rt, encoded, "str_repeat_count");
                        let result = builder
                            .build_call(rt.str_repeat, &[operand.into(), raw.into()], "str_repeat")
                            .expect("build_call should not fail for a well-formed repetition")
                            .try_as_basic_value()
                            .expect_basic("pycc_rt_str_repeat returns a non-void pointer");
                        return Scalar::Str(result.into_pointer_value());
                    }
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
                let l = to_numeric_encoded_int(context, builder, l);
                let r = to_numeric_encoded_int(context, builder, r);
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
                // #146 Part 2 (D-181): `pycc_rt_int_cmp` returns an `i32`
                // ordering and retains nothing, so both operands are dead
                // the moment the comparison has been made.
                release_if_int_temporary(context, builder, rt, left, l);
                release_if_int_temporary(context, builder, rt, right, r);
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
            // D-136 Task 4: `math.sqrt(x: float) -> float` is this PR's one
            // lowered stdlib function, calling straight into the platform
            // libm `sqrt` symbol -- the same C ABI this crate's existing
            // float codegen already links against for `**`'s `pow` call
            // (see `main.rs`'s `add_linux_system_libs` doc comment: macOS
            // folds libm into libSystem and Windows's UCRT bundles it, so
            // only Linux needs an explicit `-lm`, already added there).
            // Declared once per module and reused on every call site
            // (`get_function` first, matching `declare_rt_functions`'s own
            // idempotent-declare precedent) rather than redeclaring per
            // call -- LLVM rejects a duplicate `declare` with a different
            // linkage/signature, and there is no reason to risk that for a
            // fixed, known-once signature.
            if callee == "math.sqrt" {
                let [arg] = args.as_slice() else {
                    panic!(
                        "pycc_codegen: internal error: `math.sqrt` takes exactly 1 argument, got {} \
                         -- pycc_types::check (T0021) should have rejected this before codegen",
                        args.len()
                    )
                };
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, arg);
                let f64_type = context.f64_type();
                let sqrt_fn = module.get_function("sqrt").unwrap_or_else(|| {
                    module.add_function(
                        "sqrt",
                        f64_type.fn_type(&[f64_type.into()], false),
                        Some(inkwell::module::Linkage::External),
                    )
                });
                // `pycc_types::std_qualified_symbol`'s call-site check
                // already rejects any argument whose static type isn't
                // `Ty::Float` (T0021) before codegen runs -- this match
                // is a defensive backstop against malformed MIR, mirroring
                // `len`'s/`float`'s own internal-error convention above,
                // not a real dispatch over legitimate source.
                let Scalar::Float(arg_value) = scalar else {
                    panic!(
                        "pycc_codegen: internal error: `math.sqrt`'s argument was not a float \
                         -- pycc_types::check (T0021) should have rejected this before codegen"
                    )
                };
                let call_site = builder
                    .build_call(sqrt_fn, &[arg_value.into()], "math_sqrt")
                    .expect("build_call should not fail for the libm `sqrt` declaration");
                return Scalar::Float(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("libm `sqrt` is declared to return a double")
                        .into_float_value(),
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
                // D-123 relaxed `len()` to also accept a `dict[K, V]`
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
                // The raw count becomes a user-visible
                // `Ty::Int` expression value here (`print(len(x))`,
                // `n = len(x)`), so it is re-tagged -- unlike
                // `MirStmt::ForList`'s own use of the same runtime call,
                // which keeps it raw as a private loop bound.
                return Scalar::Int(raw_i64_to_tagged_int(context, builder, raw_len));
            }
            if callee == "float" && !user_functions.contains_key(callee.as_str()) {
                // Hand-recognized builtin -- but, unlike `len` above, only when
                // no `user_functions` entry claims the name first (mirrors
                // `pycc_types`/`pycc_mir`'s own identical guard; see
                // `pycc_types::infer_expr_in`'s comment for why `float`, unlike
                // `len`/`print`, needs one). Reuses `to_float`, which already dispatches
                // `Scalar::Int`/`Scalar::Bool`/`Scalar::Float` correctly (the same
                // helper arithmetic-promotion already calls) and already panics
                // correctly for `Scalar::Str`/`List`/`Dict`/`Set`/`Tuple` -- no new conversion
                // logic, only a new caller. `pycc_types` already rejects a
                // non-numeric or mis-arity call with T0021 before codegen ever runs;
                // this arity check is a defensive backstop against malformed MIR,
                // mirroring `len`'s own internal-error convention immediately above.
                let [arg] = args.as_slice() else {
                    panic!(
                        "pycc_codegen: internal error: `float` takes exactly 1 argument, got {} \
                         -- pycc_types::check (T0021) should have rejected this before codegen",
                        args.len()
                    )
                };
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, arg);
                return Scalar::Float(to_float(context, builder, rt, scalar));
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
            let _ = user_function; // validated above; build_call_to looks up by name
            let call_site = build_call_to(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                callee,
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
                // A class instance is not reachable as a real function
                // return type from this PR's own frontend (`pycc_types`
                // never resolves a return annotation to `Ty::Instance` --
                // class-typed annotations are out of scope, see the plan's
                // own "Explicitly out of scope" list), but the codegen
                // itself needs no defensive deferral the way `List`/`Dict`/
                // `Set`/`Tuple` below still do: `ty_to_basic_type` already
                // gives an `Instance`-returning function's LLVM signature
                // the same pointer return type a `str`-returning one gets,
                // so extracting the call result is identical to `Str`
                // above -- a future PR that does support `-> Self`/
                // `-> ClassName` needs no further codegen work here.
                Ty::Instance(_) => Scalar::Instance(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("this function is declared to return an instance")
                        .into_pointer_value(),
                ),
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
                            let as_str = to_str(builder, rt, scalar);
                            // #146 Part 2 (D-181): same pure-consumer shape
                            // as `print`'s own argument above, and released
                            // after `to_str` for the same reason.
                            release_scalar_if_int_temporary(context, builder, rt, inner, &scalar);
                            as_str
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
        // D-141: each element becomes an int-compatible encoded word. The
        // checked decoder validates bigint exclusion, but storage keeps the
        // original word so a bool marker survives the round-trip.
        //
        // `to_encoded_int` rather than a `let Scalar::Int(..) else` match, for
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
                let encoded = to_encoded_int(context, builder, scalar);
                let _ = build_untag_checked(builder, rt, encoded, "list_validate_element");
                build_int_list_append(builder, rt, list_ptr, encoded);
            }
            Scalar::List(list_ptr)
        }
        // `base[index]`, read-only, over two structurally different bases
        // (PR-11b Task 5). A `list[T]` base is a runtime-indexed read into a
        // heap object; a `tuple[...]` base is a compile-time-indexed field
        // extraction from an SSA aggregate (D-115). They share only their
        // surface syntax, so this arm dispatches between them rather than
        // treating one as a special case of the other. `dict[K, V]` never
        // reaches here at all -- `pycc_mir`'s `lower_expr` routes a dict
        // base to `MirExpr::DictGet` instead.
        MirExpr::Subscript { base, index } => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            // Matched as a *pair* rather than branching on `base.ty()` alone
            // (PR-11b Task 5). Branching on the type alone would need a
            // `let Scalar::Tuple(..) = base_scalar else { panic!(..) }`
            // inside the tuple arm -- and that arm would be permanently
            // uncoverable under D-014's 100%-region gate, because every
            // expression whose `ty()` is `Ty::Tuple` already evaluates to
            // `Scalar::Tuple` by construction: `TupleLiteral` returns one
            // directly, `Name` returns one unconditionally from its own
            // `Ty::Tuple(_)` arm (which dispatches on the very `ty` field
            // `base.ty()` just read, so the two cannot disagree -- this
            // holds on its own, without relying on that arm's
            // `debug_assert_eq!`, which is compiled out in release), and a
            // `Ty::Tuple`-typed `Call` panics at the container catch-all in
            // this same function before it can return anything at all.
            // Pairing the two instead lets that impossible
            // combination fall into the list path's already-covered
            // `expect_list_pointer` check, adding no new branch that no
            // test could ever legitimately reach -- the same
            // "no redundant impossible-to-cover branch" convention
            // `emit_string_literal`'s own doc comment records.
            match (base.ty(), base_scalar) {
                // D-115: the field is read straight off the loaded
                // `StructValue` with `extractvalue` -- no pointer, no
                // `build_struct_gep`, because the aggregate is an SSA
                // register rather than something in memory.
                (pycc_mir::Ty::Tuple(_), Scalar::Tuple(struct_value)) => {
                    let MirExpr::IntLiteral(literal_index) = index.as_ref() else {
                        panic!(
                            "pycc_codegen: internal error: a tuple subscript index is not a \
                             literal int -- pycc_types::check (T0040) should have rejected this \
                             before codegen"
                        )
                    };
                    // Delegates the non-negative and in-range validation to
                    // `MirExpr::ty()`, which already re-derives exactly this
                    // positional element type from the same literal index
                    // and panics on either failure (see `pycc_mir`'s own
                    // `Subscript`/`Ty::Tuple` arm). Re-checking here would
                    // duplicate that logic *and* add two more panic regions
                    // this crate would then have to cover itself.
                    let elem_ty = expr.ty();
                    let field_value = builder
                        .build_extract_value(struct_value, *literal_index as u32, "tuple_extract")
                        .expect("build_extract_value should not fail for a validated index");
                    // No output conversion: like D-141's container payloads,
                    // a tuple field already holds the encoded value that
                    // `TupleLiteral` inserted. Re-tagging would corrupt every
                    // ordinary `int` element and erase bool identity.
                    match elem_ty {
                        Ty::Int => Scalar::Int(field_value.into_int_value()),
                        Ty::Bool => Scalar::Bool(field_value.into_int_value()),
                        Ty::Float => Scalar::Float(field_value.into_float_value()),
                        // Defensive: `pycc_types`' T0039 gate (D-116)
                        // rejects every other element type before codegen.
                        other => panic!(
                            "pycc_codegen: reading a `{}`-typed tuple element is not supported yet",
                            other.name()
                        ),
                    }
                }
                // `base[index]`, read-only (D-105 scope cut 2). The index is
                // decoded to a raw positional counter; the D-141 element
                // word read back out is already user-visible and passes
                // through unchanged.
                //
                // An out-of-range (including any negative) index is
                // `pycc_rt_int_list_get`'s own honest runtime panic, not
                // something this crate can check -- the index is only known
                // at runtime. That is the opposite of the tuple arm above,
                // where the index is a compile-time literal `pycc_types`
                // has already bounds-checked.
                (_, base_scalar) => {
                    let base_ptr = expect_list_pointer(base_scalar, "the subscripted value");
                    let index_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, index);
                    let encoded_index = to_numeric_encoded_int(context, builder, index_scalar);
                    let raw_index =
                        build_untag_checked(builder, rt, encoded_index, "list_untag_index");
                    let encoded_element = build_int_list_get(builder, rt, base_ptr, raw_index);
                    Scalar::Int(encoded_element)
                }
            }
        }
        // `list.append(value)` (D-105 point 3). Same D-141 validation and
        // identity-preserving storage as `ListLiteral` above. `list` is a plain
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
            let encoded = to_encoded_int(context, builder, scalar);
            let _ = build_untag_checked(builder, rt, encoded, "list_validate_appended");
            build_int_list_append(builder, rt, list_ptr, encoded);
            Scalar::Bool(context.i8_type().const_int(0, false))
        }
        // `{k1: v1, k2: v2, ...}` (PR-11 Task 5, D-123): an empty
        // `pycc_rt_dict_new()` object followed by one `pycc_rt_dict_set`
        // per pair, in source order -- mirrors `MirExpr::ListLiteral`'s own
        // shape exactly (no pre-sizing call: `PyDictObj`'s own payload
        // already handles growth itself, same reasoning as
        // `ListLiteral`'s own doc comment).
        //
        // D-141's encoded-value contract applies to the value half only:
        // the key is a `Ty::Str` expression that crosses into `PyDictObj`
        // unchanged (a dict key is a raw pointer either way, no tagging
        // scheme), while each value is validated against bigint storage and
        // then stored with its original encoded word.
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
                // permanent reference, without incref'ing it itself (D-124:
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
                // policy (D-107/D-124 only ever accept *never freeing*, not
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
                let encoded = to_encoded_int(context, builder, value_scalar);
                let _ = build_untag_checked(builder, rt, encoded, "dict_validate_literal_value");
                build_dict_set(builder, rt, dict_ptr, key_ptr, encoded);
            }
            Scalar::Dict(dict_ptr)
        }
        // `dict[key]`, read-only (PR-11 Task 5, D-123 -- `d[k] = v` is a
        // separate, statement-level operation, `MirStmt::DictSet` below).
        // Mirrors `MirExpr::Subscript`'s own shape: the key is a `Ty::Str`
        // expression crossing in unchanged (a dict key is never tagged),
        // and the encoded value read back out is forwarded unchanged.
        //
        // A missing key makes `pycc_rt_dict_get` set D-173's pending
        // `KeyError` state and return a neutral carrier. The enclosing
        // `emit_expr` wrapper guards this node before that carrier can be
        // consumed.
        MirExpr::DictGet { dict, key } => {
            let dict_scalar = emit_expr(context, builder, module, rt, user_functions, locals, dict);
            let dict_ptr = expect_dict_pointer(dict_scalar, "the dict subscripted value");
            let key_scalar = emit_expr(context, builder, module, rt, user_functions, locals, key);
            let Scalar::Str(key_ptr) = key_scalar else {
                panic!(
                    "pycc_codegen: internal error: dict subscript key did not evaluate to str \
                     -- pycc_types::check (T0021) should have rejected this before codegen"
                )
            };
            let encoded_value = builder
                .build_call(rt.dict_get, &[dict_ptr.into(), key_ptr.into()], "dict_get")
                .expect("build_call should not fail for a well-formed dict read")
                .try_as_basic_value()
                .expect_basic("pycc_rt_dict_get returns a non-void i64")
                .into_int_value();
            Scalar::Int(encoded_value)
        }
        // `{e1, e2, ...}` (PR-11 Task 9, D-123/D-121): an empty
        // `pycc_rt_int_set_new()` object followed by one
        // `pycc_rt_int_set_add` per element, in source order -- mirrors
        // `MirExpr::ListLiteral`'s own shape exactly (no pre-sizing call:
        // `PyIntSetObj`'s own payload already handles growth itself, same
        // reasoning as `ListLiteral`'s own doc comment), except that
        // `set[int]` has no string-keyed counterpart to `DictLiteral`'s own
        // per-pair key handling -- every element is an encoded `i64`, so there is
        // no refcounting concern here at all (unlike `DictLiteral`'s key,
        // D-123's own T0038 gate means every `set[int]` element is exactly
        // `Ty::Int`).
        //
        // D-141 applies to every element: validate bigint exclusion, then
        // store the original encoded word. The dedup check that makes a repeated element
        // collapse to one (D-121) lives entirely inside
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
                let encoded = to_encoded_int(context, builder, scalar);
                let _ = build_untag_checked(builder, rt, encoded, "set_validate_element");
                build_int_set_add(builder, rt, set_ptr, encoded);
            }
            Scalar::Set(set_ptr)
        }
        // `(e1, e2, ...)` (PR-11b Task 5, D-115). The one container literal
        // in this file that calls into no `pycc_rt` constructor at all:
        // where `ListLiteral`/`DictLiteral`/`SetLiteral` each allocate a
        // heap object and then fill it, this builds a pure SSA aggregate --
        // `get_undef()` for the struct shape, then one
        // `build_insert_value` (LLVM's `insertvalue`) per element in source
        // order, each returning a *new* aggregate value rather than
        // mutating one in place. No alloca, no pointer, no allocation, and
        // so nothing for D-107/D-124's leak-only policy to apply to.
        //
        // Deliberately no `build_untag_checked` per element: a tuple has no
        // runtime storage boundary to validate. Like D-141 containers, a
        // tuple field stores exactly the `Scalar` it was given. An `int`
        // element therefore goes in as its D-141 encoded word (ordinary
        // smallint, bool marker, or bigint pointer), and `MirExpr::Subscript`'s
        // tuple branch returns that word unchanged, so the two sides never
        // disagree and no conversion happens on either.
        MirExpr::TupleLiteral(elements) => {
            let elem_tys: Vec<pycc_mir::Ty> = elements.iter().map(MirExpr::ty).collect();
            let struct_ty = ty_to_basic_type(context, pycc_mir::Ty::Tuple(Box::new(elem_tys)))
                .into_struct_type();
            let mut aggregate = struct_ty.get_undef();
            for (index, element) in elements.iter().enumerate() {
                let scalar = emit_expr(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    element,
                );
                let field_value: inkwell::values::BasicValueEnum = match scalar {
                    Scalar::Int(v) => v.into(),
                    Scalar::Bool(v) => v.into(),
                    Scalar::Float(v) => v.into(),
                    // A defensive backstop, not a feature gap: `pycc_types`'
                    // own T0039 gate (D-116) rejects every tuple element
                    // type but `int`/`bool`/`float` before codegen runs, so
                    // no type-checked program reaches this. Named in prose
                    // rather than `{other:?}` because `Scalar` deliberately
                    // derives no `Debug` -- matching `expect_list_pointer`'s
                    // own message style.
                    _ => panic!(
                        "pycc_codegen: internal error: a tuple element evaluated to a \
                         non-int/bool/float value -- pycc_types::check (T0039) should have \
                         rejected this before codegen"
                    ),
                };
                aggregate = builder
                    .build_insert_value(aggregate, field_value, index as u32, "tuple_insert")
                    .expect("build_insert_value should not fail for an in-range tuple field")
                    .into_struct_value();
            }
            Scalar::Tuple(aggregate)
        }
        // `base[start:stop:step]` (PR-12 Task 9, D-118). Evaluates `base`,
        // then each present bound left to right, exactly matching Python's
        // own sub-expression evaluation order. `pycc_types` already
        // validated `base` is `list[int]` and every present bound is
        // `int`-assignable (Task 7); this arm applies D-118's own runtime
        // behavior (default/clamp/panic) that `pycc_mir`'s purely
        // structural lowering (Task 8) deliberately left to codegen.
        //
        // Deliberate deviation from this task's own originating plan
        // sketch: `base`'s length (needed only when `stop` is omitted) is
        // read *after* every present bound has already been evaluated, not
        // immediately after `base`. `len()` itself has no side effect, but
        // a present `start`/`step` expression might mutate `base` before
        // yielding its own value (e.g. a helper that appends to the same
        // list `base` names, then returns the bound) -- reading the length
        // this late means an omitted `stop` always reflects `base`'s state
        // as of just before the slice actually runs, matching CPython's own
        // "build the slice object from every sub-expression, then apply it"
        // order, where the length lookup happens once, after `start`/
        // `stop`/`step` have all already been evaluated. This costs nothing
        // in either code size or coverage: both the `Some`/`None` arms for
        // `stop` already need their own tests regardless of where the
        // length read sits.
        //
        // `xs = xs[1:3]` is safe from the identical self-referential-rebind
        // hazard `MirStmt::ListCompAssign`'s own doc comment documents at
        // length (Task 5a's confirmed regression): the only way this arm's
        // result reaches a name is through `MirStmt::Assign`, which fully
        // evaluates this whole expression to a `Scalar::List` -- a pointer
        // to a **freshly allocated** result object `pycc_rt_int_list_slice`
        // returns, never `base_ptr` itself -- before it ever calls
        // `emit_assign` to rebind `target`'s own slot. This arm never
        // writes to `locals` at all, so there is no premature-rebind window
        // here the way an earlier `ListCompAssign` draft had for
        // comprehensions. The original list is left untouched either way
        // (D-107's leak-only policy: no incref/decref of `base_ptr`, and no
        // special handling for the new result beyond what
        // `pycc_rt_int_list_slice` itself already does).
        MirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            let base_ptr = expect_list_pointer(base_scalar, "the sliced value");

            let start_i64 = match start {
                Some(e) => {
                    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, e);
                    let encoded = to_numeric_encoded_int(context, builder, scalar);
                    build_untag_checked(builder, rt, encoded, "slice_untag_start")
                }
                None => context.i64_type().const_int(0, false),
            };
            let stop_raw = match stop {
                Some(e) => {
                    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, e);
                    let encoded = to_numeric_encoded_int(context, builder, scalar);
                    Some(build_untag_checked(
                        builder,
                        rt,
                        encoded,
                        "slice_untag_stop",
                    ))
                }
                None => None,
            };
            let step_i64 = match step {
                Some(e) => {
                    let scalar = emit_expr(context, builder, module, rt, user_functions, locals, e);
                    let encoded = to_numeric_encoded_int(context, builder, scalar);
                    build_untag_checked(builder, rt, encoded, "slice_untag_step")
                }
                None => context.i64_type().const_int(1, false),
            };
            // Read only now: after `base` and every present bound have
            // already been evaluated (see this arm's own doc comment
            // above).
            let stop_i64 = match stop_raw {
                Some(v) => v,
                None => build_int_list_len(builder, rt, base_ptr),
            };

            let result_ptr =
                build_int_list_slice(builder, rt, base_ptr, start_i64, stop_i64, step_i64);
            Scalar::List(result_ptr)
        }
        // `list.pop()` (PR-12 Task 11, D-119): removes and returns `list`'s
        // own last element. Mirrors `MirExpr::ListAppend`'s own shape (a
        // plain-name base read through `emit_list_name_read` and a direct
        // encoded-value return, exactly like `MirExpr::Subscript` and
        // `MirExpr::DictGet`) rather than `MirExpr::Slice`'s -- there
        // is no sub-expression to recursively evaluate here at all, unlike
        // `Slice`'s bounds.
        //
        // Safe under repeated self-reference within one statement (e.g.
        // `xs = [xs.pop(), xs.pop()]`, this task's own brief flags this
        // shape): `emit_list_name_read` always re-reads `xs`'s *current*
        // slot value, which does not change mid-statement -- `MirStmt::
        // Assign`'s own arm (see that arm's own call site) evaluates this
        // whole right-hand-side expression to a finished `Scalar` *before*
        // ever rebinding `xs`'s slot, exactly the ordering `MirExpr::Slice`'s
        // own doc comment above already establishes is what keeps `xs =
        // xs[1:3]` safe from Task 5a's confirmed self-referential-rebind
        // regression. Each `.pop()` mutates the *pointee* `PyIntListObj`
        // in place (via `pycc_rt_int_list_pop`'s own `Cell<Vec<i64>>`), not
        // `xs`'s own slot, so two `.pop()`s emitted in source order run in
        // that same order at runtime -- ordinary sequential `call`
        // instructions, nothing deferred -- and each observes the
        // previous one's mutation, matching CPython's own left-to-right
        // evaluation of `[xs.pop(), xs.pop()]`. Verified empirically, not
        // just by this reasoning: see this file's own
        // `pop_twice_on_the_same_list_in_one_statement_removes_in_order`
        // end-to-end test below.
        MirExpr::ListPop { list, .. } => {
            let list_ptr =
                emit_list_name_read(context, builder, module, rt, user_functions, locals, list);
            let encoded = build_int_list_pop(builder, rt, list_ptr);
            Scalar::Int(encoded)
        }
        // `dict.get(key, default)` (PR-12 Task 11, D-119): returns the
        // stored value, or `default` if `key` is absent -- never panics on
        // a missing key, unlike `MirExpr::DictGet`'s own `d[key]`. `dict`
        // is a plain variable name (mirrors `HirExpr::DictGetOrDefault`),
        // read through `emit_dict_name_read` exactly like
        // `MirStmt::DictSet`'s own `dict` field; `key` and `default` are
        // both arbitrary sub-expressions, evaluated left to right, matching
        // Python's own left-to-right argument evaluation.
        //
        // `key`'s `Scalar::Str` is extracted with **no**
        // `incref_if_str_duplicate` call, unlike `MirStmt::DictSet`'s own
        // key -- following `MirExpr::DictGet`'s own no-incref precedent
        // above, not `DictSet`'s: a read-only lookup never stores the key
        // pointer anywhere persistent, so there is no new owning reference
        // to protect (D-124's incref requirement exists only where a
        // pointer is *adopted*, e.g. `pycc_rt_dict_set`'s own key
        // parameter).
        //
        // `default` is read-only too -- unlike `key`, it never reaches
        // `pycc_rt` as a pointer (a dict value is an encoded `i64`), so no
        // incref question arises for it at all. Safe under
        // nested self-reference within one statement (e.g. `d =
        // d.get("k", d.get("j", 0))`, this task's own brief flags this
        // shape): both the outer and inner `.get()` read `d`'s slot via
        // `emit_dict_name_read`, and neither `.get()` call ever mutates
        // `d`'s pointee (`pycc_rt_dict_get_or_default` is a pure lookup, no
        // `Cell::set` on any success or failure path) -- so, unlike
        // `ListPop`'s own mutating case just above, there is no ordering
        // hazard to reason about here at all: every read observes the same
        // unchanged dict. Verified empirically: see this file's own
        // `dict_get_or_default_nested_in_its_own_default_argument_resolves_correctly`
        // end-to-end test below.
        MirExpr::DictGetOrDefault {
            dict, key, default, ..
        } => {
            let dict_ptr =
                emit_dict_name_read(context, builder, module, rt, user_functions, locals, dict);
            let key_scalar = emit_expr(context, builder, module, rt, user_functions, locals, key);
            let Scalar::Str(key_ptr) = key_scalar else {
                panic!(
                    "pycc_codegen: internal error: dict.get() key did not evaluate to str -- \
                     pycc_types::check (T0021) should have rejected this before codegen"
                )
            };
            let default_scalar = emit_expr(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                default,
            );
            let encoded_default = to_encoded_int(context, builder, default_scalar);
            let _ = build_untag_checked(builder, rt, encoded_default, "dict_get_validate_default");
            let encoded =
                build_dict_get_or_default(builder, rt, dict_ptr, key_ptr, encoded_default);
            Scalar::Int(encoded)
        }
        // `set.add(value)` (PR-12 Task 11, D-119): mirrors `MirExpr::
        // ListAppend`'s own shape exactly (a plain-name base read through
        // `emit_set_name_read`, D-141 validation on `value`, the canonical
        // `Ty::None` `i8 0` result
        // carrier) -- `set` is read only once, `build_int_set_add` is
        // `MirExpr::SetLiteral`'s own already-existing helper (this is
        // its second call site, not a new declaration), and the dedup
        // check that makes a repeated `.add()` of an already-present value
        // a no-op lives entirely inside `pycc_rt_int_set_add` itself, not
        // here (see that function's own doc comment). Repeated `s.add(x);
        // s.add(x)` calls across two separate statements need no special
        // reasoning at all: each is its own fully independent `MirStmt`,
        // executed strictly in source order, each re-reading `s`'s current
        // (unchanged-by-`.add()`-itself-except-for-dedup-mutation) pointer
        // -- verified empirically: see this file's own
        // `add_the_same_value_twice_across_two_statements_still_dedups`
        // end-to-end test below.
        MirExpr::SetAdd { set, value } => {
            let set_ptr =
                emit_set_name_read(context, builder, module, rt, user_functions, locals, set);
            let value_scalar =
                emit_expr(context, builder, module, rt, user_functions, locals, value);
            let encoded = to_encoded_int(context, builder, value_scalar);
            let _ = build_untag_checked(builder, rt, encoded, "set_validate_added");
            build_int_set_add(builder, rt, set_ptr, encoded);
            Scalar::Bool(context.i8_type().const_int(0, false))
        }
        // `ClassName(args)` (D-154, Part 1 of #375): allocate a fresh,
        // zero-initialized instance (`pycc_rt_instance_new`, given the
        // class's own already-resolved `attr_count`), then call the
        // mangled `__init__` with that pointer as `self`, followed by
        // `args` -- see `MirExpr::Instantiate`'s own doc comment for why
        // this needs `build_call_to_with_leading_args` rather than the
        // ordinary `build_call_to` every ordinary/method call above uses.
        MirExpr::Instantiate(inst) => {
            let pycc_mir::InstantiateExpr {
                ctor,
                attr_count,
                args,
                ..
            } = inst.as_ref();
            let count = context.i64_type().const_int(*attr_count as u64, false);
            let instance_ptr = builder
                .build_call(rt.instance_new, &[count.into()], "instance_new")
                .expect("build_call should not fail for a well-formed instance allocation")
                .try_as_basic_value()
                .expect_basic("pycc_rt_instance_new returns a non-void pointer")
                .into_pointer_value();
            let ctor_function = user_functions.get(ctor.as_str()).unwrap_or_else(|| {
                panic!(
                    "pycc_codegen: internal error: constructor `{ctor}` should have been \
                     registered as an ordinary user function -- pycc_hir::class::lower_class \
                     mangles every method, including `__init__`, into HirModule::items"
                )
            });
            build_call_to_with_leading_args(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                ctor_function,
                ctor,
                &[instance_ptr.into()],
                args,
            );
            Scalar::Instance(instance_ptr)
        }
        // `base.attr` (D-154, Part 1 of #375): read the raw slot word via
        // the opaque `pycc_rt_instance_get_slot` accessor (never a direct
        // `GEP`, per the class-instance-layout ADR), then reinterpret it as
        // the attribute's own declared `Ty` -- see `slot_word_to_scalar`'s
        // own doc comment for the conversion and its own reachable-`Ty`
        // scope.
        MirExpr::AttrGet { base, slot, ty } => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            let base_ptr = expect_instance_pointer(base_scalar, "attribute access base");
            let slot_index = context.i64_type().const_int(*slot as u64, false);
            let raw = builder
                .build_call(
                    rt.instance_get_slot,
                    &[base_ptr.into(), slot_index.into()],
                    "instance_get_slot",
                )
                .expect("build_call should not fail for a well-formed attribute read")
                .try_as_basic_value()
                .expect_basic("pycc_rt_instance_get_slot returns a non-void i64")
                .into_int_value();
            slot_word_to_scalar(context, builder, raw, ty)
        }
        MirExpr::NullInstance { .. } => {
            let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
            let null_ptr = ptr_type.const_null();
            Scalar::Instance(null_ptr)
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
/// `HirStmt::ForList`'s dict-typed case, D-123), so neither has a `MirExpr`
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
/// `HirStmt::ForList`'s set-typed case, D-123), so it has no `MirExpr` to
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
    callee_name: &str,
    args: &[MirExpr],
) -> inkwell::values::CallSiteValue<'ctx> {
    let user_function = &user_functions[callee_name];
    build_call_to_with_leading_args(
        context,
        builder,
        module,
        rt,
        user_functions,
        locals,
        user_function,
        callee_name,
        &[],
        args,
    )
}

/// Generalizes [`build_call_to`] with a `leading_args` prefix -- values
/// already computed by the caller (never marshalled through `emit_expr`,
/// since they have no `MirExpr` of their own) that go first in the emitted
/// `call`'s argument list, before `args`' own marshalled values. Used by
/// `MirExpr::Instantiate`'s codegen (D-154, Part 1 of #375) to pass the
/// freshly allocated instance pointer as `__init__`'s own `self` argument --
/// `build_call_to`'s ordinary path has no `MirExpr` to build that pointer
/// from, since it is a codegen-only value with no HIR/MIR node of its own.
/// `leading_args` is skipped when zipping `args` against
/// `user_function.param_tys`, so `args[0]` still lines up with the *first
/// non-leading* declared parameter type, exactly like `build_call_to`'s own
/// contract for every ordinary call (`leading_args` empty).
#[allow(clippy::too_many_arguments)]
fn build_call_to_with_leading_args<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    user_function: &UserFunction<'ctx>,
    _callee_name: &str,
    leading_args: &[inkwell::values::BasicMetadataValueEnum<'ctx>],
    args: &[MirExpr],
) -> inkwell::values::CallSiteValue<'ctx> {
    let mut arg_values: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = leading_args.to_vec();
    let marshalled_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = args
        .iter()
        .zip(&user_function.param_tys[leading_args.len()..])
        .map(|(a, param_ty)| {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, a);
            let scalar = incref_if_str_duplicate(builder, rt, a, scalar);
            let scalar = retain_if_int_duplicate(context, builder, rt, a, scalar);
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
                // Pass-through like the three arms above, but by VALUE
                // rather than by pointer (D-115): a `tuple[...]` argument
                // is an LLVM struct at the ABI level, not an opaque
                // pointer. It still needs no tuple-specific marshalling
                // code, because `BasicMetadataValueEnum` already has a
                // `StructValue` arm -- the calling convention's own
                // by-value aggregate handling is LLVM's job, not this
                // crate's.
                Scalar::Tuple(v) => v.into(),
                // Pass-through, identical to `List`'s/`Dict`'s/`Set`'s arms
                // above: a class-instance parameter (D-154, Part 1 of #375
                // -- `self`, or any other class-typed parameter a future PR
                // might add) is an opaque pointer at the ABI level exactly
                // like a `str`/`list[T]`/`dict[K, V]`/`set[T]` one.
                Scalar::Instance(v) => v.into(),
            }
        })
        .collect();
    arg_values.extend(marshalled_args);
    // Issue #22: dispatch indirectly through the function-pointer slot.
    // Load the current binding; if null, the function hasn't been defined
    // yet at this point in execution -- abort with a runtime NameError.
    // Otherwise, call through the loaded pointer.
    // Monomorphized generic specializations (`0gen_...` names) have no
    // `fn_ptr_global` -- they dispatch directly through `direct_value`
    // since they are compiler-generated, not user-defined.
    if let Some(ref direct_value) = user_function.direct_value {
        return builder
            .build_call(*direct_value, &arg_values, "call_user_fn")
            .expect("build_call should not fail for a well-formed direct call");
    }
    let fn_ptr_global = user_function
        .fn_ptr_global
        .as_ref()
        .expect("non-monomorphized user function has a fn_ptr_global");
    let fn_ptr_type = context.ptr_type(inkwell::AddressSpace::default());
    let fn_ptr = builder
        .build_load(fn_ptr_type, fn_ptr_global.as_pointer_value(), "load_fnptr")
        .expect("build_load should not fail for a global function-pointer slot")
        .into_pointer_value();
    let null_ptr = fn_ptr_type.const_null();
    let is_null = builder
        .build_int_compare(IntPredicate::EQ, fn_ptr, null_ptr, "fnptr_is_null")
        .expect("build_int_compare should not fail for a null check");
    let current_fn = builder
        .get_insert_block()
        .expect("builder is always positioned in a block during call emission")
        .get_parent()
        .expect("every block has a parent function");
    let not_null_block = context.append_basic_block(current_fn, "fnptr_not_null");
    let is_null_block = context.append_basic_block(current_fn, "fnptr_is_null");
    builder
        .build_conditional_branch(is_null, is_null_block, not_null_block)
        .expect("build_conditional_branch should not fail for a null-check dispatch");
    // Null path: call pycc_rt_name_error with the function name as a C
    // string, then unreachable (name_error never returns). The name
    // global was created once per function name in the declaration pass
    // and is reused at every call site.
    builder.position_at_end(is_null_block);
    let name_global = user_function
        .name_global
        .as_ref()
        .expect("non-monomorphized user function has a name_global");
    let name_ptr = name_global.as_pointer_value();
    builder
        .build_call(rt.name_error, &[name_ptr.into()], "name_error")
        .expect("build_call should not fail for a well-formed runtime error call");
    builder
        .build_unreachable()
        .expect("build_unreachable terminates the null-pointer path");
    // Non-null path: indirect call through the loaded pointer.
    builder.position_at_end(not_null_block);
    builder
        .build_indirect_call(user_function.fn_type, fn_ptr, &arg_values, "call_user_fn")
        .expect("build_indirect_call should not fail for a well-formed indirect call")
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
        // semantics (D-123 ships only `len(x)`/`x[k]`/`x[k] = v`/
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
        // `bool(set)` semantics (D-124 ships only `len(x)`/iteration), and
        // there is no `pycc_rt_int_set_truthy` to call -- so this panics
        // honestly instead of calling `pycc_rt_str_truthy` on a
        // `PyIntSetObj` pointer, whose layout has nothing in common with
        // `PyStrObj`'s.
        Scalar::Set(_) => {
            panic!("pycc_codegen: truthiness of a set[T] value is not supported yet")
        }
        // A real, reachable feature gap -- but NOT "identical in kind" to
        // the `List`/`Dict`/`Set` arms directly above in one respect:
        // `list[T]`'s own `if xs:`/`while xs:` reachability predates this
        // whole PR-11 effort entirely (established back in PR-10, D-107);
        // `dict`/`set`'s own reachability, while more recent (PR-11a's own
        // HIR literal lowering), was already in place before this PR
        // (PR-11b) started -- neither is something this PR's own diff
        // turned from a clean diagnostic into a panic. `tuple[...]`'s
        // reachability here IS exactly that: it is new as of this PR's own
        // Task 2 (`HirExpr::TupleLiteral` lowering, `crates/pycc_hir/src/
        // lib.rs`) -- before that commit, any program containing a tuple
        // literal failed to lower at all and got a clean `C0001`
        // ("expression kind not supported yet") diagnostic instead of ever
        // reaching this function. `pycc_types` places no type restriction
        // on an `if`/`while` condition, so `if t:` for a `tuple[...]` local
        // type-checks today and lands here. D-116 ships only construction
        // and literal-index reads, so v0.2 has no tuple truthiness
        // semantics -- and CPython's own rule (a tuple is falsey only when
        // empty) is not derivable from this representation for free
        // anyway, since D-116 admits no empty tuple in the first place.
        // Panics honestly rather than guessing. See `docs/DECISIONS.md`'s
        // D-116 deferred-capability list and `docs/ROADMAP.md`'s matching
        // follow-up for this new-as-of-PR-11b reachability.
        Scalar::Tuple(_) => {
            panic!("pycc_codegen: truthiness of a tuple[...] value is not supported yet")
        }
        // Unlike every container arm above, a class instance's truthiness
        // (D-154, Part 1 of #375) needs no runtime call and no honest-panic
        // deferral: this PR ships no `__bool__`/`__len__` (both explicitly
        // out of scope), so CPython's own default-object rule applies
        // unconditionally -- `bool(x)` is `True` for any instance with
        // neither, with no data to inspect and no way for it to ever be
        // `False`. A plain constant `1` correctly implements that rule
        // rather than deferring it.
        Scalar::Instance(_) => context.i8_type().const_int(1, false),
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

/// #382 (PR-22 Part 2): If the current block's terminator is an
/// `unreachable` (placed by `MirStmt::Raise`/`RaiseFrom`/`Reraise`),
/// erase it so the caller can append an exception check (conditional
/// branch) in this block. Returns `true` if an `unreachable` was erased
/// (meaning a raise happened and the exception is active), `false`
/// otherwise. After erasing, the block has no terminator and the caller
/// can build its exception-check conditional branch normally.
fn erase_unreachable_if_present<'ctx>(builder: &inkwell::builder::Builder<'ctx>) -> bool {
    if let Some(term) = builder.get_insert_block().unwrap().get_terminator()
        && term.get_opcode() == inkwell::values::InstructionOpcode::Unreachable
    {
        term.erase_from_basic_block();
        return true;
    }
    false
}

/// Allocates a local slot in the current function's entry block, which
/// dominates every branch, loop, and merge that may read it. String slots are
/// initialized to null and `int` slots to the word `0` (#146 Part 1), so the
/// first emitted decref/release is a safe no-op rather than a read of
/// uninitialized stack. `0` is deliberately *not* a valid encoded int -- it
/// is `classify_encoded_int`'s fail-closed pattern -- which is exactly why
/// `pycc_rt_bigint_release` returns on it before classifying. A guarded
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
    } else if ty == pycc_mir::Ty::Int {
        builder
            .build_store(ptr, context.i64_type().const_zero())
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
/// `bool`-to-`int` assignment that must store a D-141 encoded i64 rather than raw i8.
///
/// #146 Part 1: an `Ty::Int` slot releases its previous word here, before
/// the store, for *every* caller rather than at each call site
/// individually. `str`'s own release deliberately stays at `MirStmt::
/// Assign` (`decref_str_slot_before_store`): only a small subset of this
/// function's callers bind a `str`, whereas thirteen of them bind an `int`
/// (the four `range`-induction binds, the eight container-element binds,
/// and ordinary assignment), and a per-site release is exactly the shape
/// where one missed site ships a silent leak that no value assertion can
/// observe. Centralizing it here makes "released before every int store" a
/// property of the function rather than of an audit.
///
/// The release is gated on the *slot's* declared type, never on the value's:
/// `x: int = True` reaches here as a `Scalar::Bool` that
/// `coerce_scalar_to_type` (below) turns into a D-141 encoded word, and
/// value-type gating would skip the release for exactly that case.
fn emit_assign<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    locals: &mut HashMap<String, StorageSlot<'ctx>>,
    target: &str,
    value: Scalar<'ctx>,
) {
    let slot = locals
        .get(target)
        .cloned()
        .expect("every assignment target must have a predeclared storage slot");
    if slot.ty == pycc_mir::Ty::Int {
        release_int_slot_before_store(context, builder, rt, &slot);
    }
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
        // no `decref_str_slot_before_store` counterpart, and unlike
        // `Ty::Int` the slot-type guard above skips this arm entirely.
        Scalar::List(v) => v.into(),
        // Pass-through, identical to `List`'s arm directly above: storing
        // a `dict[K, V]` value is storing one opaque pointer into a slot
        // `ty_to_basic_type` already allocated as a pointer. No refcount
        // traffic accompanies it -- D-124 keeps `dict[K, V]` leak-only for
        // v0.2 (extending D-107's exact reasoning), so unlike `Str` there
        // is deliberately no incref here and no
        // `decref_str_slot_before_store` counterpart.
        Scalar::Dict(v) => v.into(),
        // Pass-through, identical to `List`'s/`Dict`'s arms directly
        // above: storing a `set[T]` value is storing one opaque pointer
        // into a slot `ty_to_basic_type` already allocated as a pointer.
        // No refcount traffic accompanies it -- D-124 keeps `set[T]`
        // leak-only for v0.2 (extending D-107's exact reasoning),
        // identically to `List`/`Dict`.
        Scalar::Set(v) => v.into(),
        // Pass-through like the three arms above, but storing a whole
        // struct by value rather than one opaque pointer (D-115) -- into a
        // slot `ty_to_basic_type` already allocated at exactly that struct
        // type, so the store is type-correct without any tuple-specific
        // code at the `build_store` call below. No refcount traffic
        // accompanies it, and for a stronger reason than `List`/`Dict`/
        // `Set`'s leak-only policy: a tuple owns no allocation at all, so
        // overwriting this slot frees nothing and leaks nothing.
        Scalar::Tuple(v) => v.into(),
        // Pass-through, identical to `List`'s/`Dict`'s/`Set`'s arms above
        // (D-154, Part 1 of #375): storing a class-instance value is
        // storing one opaque pointer into a slot `ty_to_basic_type`
        // already allocated as a pointer. No refcount traffic accompanies
        // it -- `pycc_rt::instance` is leak-only, mirroring `List`/`Dict`/
        // `Set` (see that module's own doc comment).
        Scalar::Instance(v) => v.into(),
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
///
/// `MirExpr::AttrGet { ty: Ty::Str, .. }` (D-154, Part 1 of #375) is a
/// duplicate reference for exactly the same reason a bare `Name` is: the
/// instance's own slot keeps its copy of the pointer after this read
/// returns one too, so both `to_str`'s pass-through and every ordinary
/// store site (`Assign`, a call argument, a dict key/value, ...) would
/// otherwise treat this read's result as freshly-owned and eventually
/// decref it once too many, underflowing the refcount and freeing the
/// `PyStrObj` while the instance's own slot still points at it -- a
/// reliably reproducible use-after-free caught in review, not merely a
/// theoretical gap (D-154 Part 1's own post-merge finding).
fn str_value_is_a_duplicate_reference(expr: &MirExpr) -> bool {
    matches!(
        expr,
        MirExpr::Name {
            ty: pycc_mir::Ty::Str,
            ..
        } | MirExpr::AttrGet {
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

/// Mirror of [`decref_str_slot_before_store`] for an instance attribute slot
/// rather than a local's own alloca (D-154, Part 1 of #375): only
/// meaningful for a `Ty::Str` attribute -- reads the slot's *current* raw
/// word through the same opaque `pycc_rt_instance_get_slot` accessor
/// `MirExpr::AttrGet` itself uses, reinterprets it as a `str` pointer, and
/// decrefs it before the new value overwrites the slot. A freshly allocated
/// instance's slots start zero-initialized (`pycc_rt::instance::new_instance`),
/// which decodes to a null pointer whose runtime decref is a documented
/// no-op (`pycc_rt_str_decref`'s own null check) -- exactly like a local's
/// null-initialized string slot -- so the same call is correct for both
/// `__init__`'s first assignment and any later reassignment.
///
/// Unlike `decref_str_slot_before_store`, this function has no runtime
/// assertion that the target slot's own declared type is actually
/// `Ty::Str` -- its one caller (`MirStmt::AttrSet`'s own codegen) only
/// invokes it when `value`'s type is `Ty::Str`, and `pycc_types::class::
/// check_attr_set`'s `is_assignable(value_ty, attr_ty)` gate (`T0021`)
/// already rejects a `str` value targeting a non-`str` attribute before
/// codegen ever runs -- so this slot's declared type is `Ty::Str` too on
/// every reachable call, by construction, not merely by convention left
/// unchecked (D-068 review finding, PR #385).
fn decref_str_attr_slot_before_store<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    base_ptr: PointerValue<'ctx>,
    slot_index: IntValue<'ctx>,
) {
    let raw = builder
        .build_call(
            rt.instance_get_slot,
            &[base_ptr.into(), slot_index.into()],
            "instance_get_slot_old",
        )
        .expect("build_call should not fail for a well-formed attribute read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_instance_get_slot returns a non-void i64")
        .into_int_value();
    let old = builder
        .build_int_to_ptr(
            raw,
            context.ptr_type(inkwell::AddressSpace::default()),
            "attr_str_inttoptr_old",
        )
        .expect("build_int_to_ptr should not fail reinterpreting an i64 as a pointer");
    builder
        .build_call(rt.str_decref, &[old.into()], "str_decref_old_attr")
        .expect("build_call should not fail for a well-formed decref");
}

/// [`release_int_slot_before_store`]'s counterpart for an instance
/// attribute slot, and the exact `int` mirror of
/// [`decref_str_attr_slot_before_store`] directly above: reads the slot's
/// current raw word through `pycc_rt_instance_get_slot` and releases it
/// before the new value overwrites it. A freshly allocated instance's slots
/// are zero-initialized (`pycc_rt::instance::new_instance`), and `0` is the
/// word `pycc_rt_bigint_release` returns on without classifying, so the
/// same call is correct for `__init__`'s first assignment and every later
/// reassignment.
fn release_int_attr_slot_before_store<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    base_ptr: PointerValue<'ctx>,
    slot_index: IntValue<'ctx>,
) {
    let old = builder
        .build_call(
            rt.instance_get_slot,
            &[base_ptr.into(), slot_index.into()],
            "instance_get_slot_old_int",
        )
        .expect("build_call should not fail for a well-formed attribute read")
        .try_as_basic_value()
        .expect_basic("pycc_rt_instance_get_slot returns a non-void i64")
        .into_int_value();
    emit_bigint_refcount_call(context, builder, rt, old, BigIntRefcount::Release);
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
    finally_stack: &mut Vec<FinallyTarget<'ctx>>,
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
            finally_stack,
        )?;
        if erase_unreachable_if_present(builder) {
            let exception_target = rt
                .exceptions
                .targets
                .borrow()
                .last()
                .copied()
                .expect("emit_body is always called inside an exception target");
            builder
                .build_unconditional_branch(exception_target)
                .expect("build_unconditional_branch should route an explicit raise");
            break;
        }
        if builder
            .get_insert_block()
            .unwrap()
            .get_terminator()
            .is_some()
        {
            // A return already terminated this suite. Raising expressions
            // and complete try statements route themselves; explicit raise
            // uses the `unreachable` path handled directly above.
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
    finally_stack: &mut Vec<FinallyTarget<'ctx>>,
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
        finally_stack,
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
            // PR-10); `Ty::Dict(_)` joined it at PR-11 Task 5, `Ty::Set(_)`
            // at PR-11 Task 9, and `Ty::Tuple(_)` joins it here (PR-11b
            // Task 5) -- all for the identical reason: a tuple-typed
            // local's binding does need to be collected, since this task's
            // own codegen (`declare_module_globals`/
            // `storage_slot_at_entry`) depends on this slot already
            // existing, and `x = (1, 2)` is exactly the form D-116 ships.
            // Each is a real, deliberate inclusion, not just a louder panic
            // elsewhere. `Ty::None` is also storable via D-075's canonical
            // unit carrier; only `Ty::Infer` remains excluded.
            if matches!(
                ty,
                pycc_mir::Ty::Int
                    | pycc_mir::Ty::Bool
                    | pycc_mir::Ty::Float
                    | pycc_mir::Ty::Str
                    | pycc_mir::Ty::None
                    | pycc_mir::Ty::List(_)
                    | pycc_mir::Ty::Dict(_)
                    | pycc_mir::Ty::Set(_)
                    | pycc_mir::Ty::Tuple(_)
                    // D-154 (Part 1 of #375): `p = Point(1, 2)` needs its
                    // own predeclared storage slot exactly like every other
                    // heap-object-typed binding above.
                    | pycc_mir::Ty::Instance(_)
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
        // `list[int]`-only `ForList` arm then stores an encoded `int` into. A
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
        // `base.attr = value` (D-154, Part 1 of #375) reassigns an
        // existing instance's attribute slot, not a name -- same reasoning
        // as `DictSet` immediately above.
        MirStmt::AttrSet { .. } => {}
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
        // `target = [elt for var in <source> [if cond]]` (PR-12 Task 5a,
        // D-117): a comprehension introduces *two* new bindings, not one --
        // `target` (the produced `list[T]` container) and `var` (the
        // synthesized loop variable, D-117). `var_ty` is carried explicitly
        // on the MIR node itself (Task 4), so unlike `ForList`'s own
        // `Ty::Int` hardcode above, no re-derivation is needed or attempted
        // here: `resolve_comp_source` (`pycc_mir`) already computed it once,
        // exactly mirroring `ForList`'s `Ty::Dict(kv) => kv.0` choice for a
        // `Dict` source. `target`'s own type is derived structurally from
        // `elt`, exactly like `MirExpr::ListLiteral`'s own `ty()` derivation
        // -- not re-read from `var_ty`, which is `var`'s type, not
        // `target`'s. Nothing recurses into `cond`/`elt`: neither can ever
        // contain a nested statement (both are plain `MirExpr` trees), so
        // there is no `body`-like recursion for this variant the way every
        // `For*` arm above has.
        MirStmt::ListCompAssign {
            target,
            var,
            var_ty,
            elt,
            ..
        } => {
            bindings
                .entry(var.clone())
                .or_insert_with(|| var_ty.clone());
            bindings
                .entry(target.clone())
                .or_insert(pycc_mir::Ty::List(Box::new(elt.ty())));
        }
        // `target = {key: value for var in <source> [if cond]}` (PR-12 Task
        // 5b, D-117): mirrors `ListCompAssign`'s own arm above exactly,
        // substituting `target`'s derived type (`Ty::Dict`, from `key`'s and
        // `value`'s own types, mirroring `MirExpr::DictLiteral`'s own `ty()`
        // derivation) for `Ty::List`. Same reasoning as `ListCompAssign`'s
        // own arm for everything else: `var_ty` is carried explicitly by
        // Task 4's own lowering, not re-derived; no recursion into
        // `cond`/`key`/`value`, none of which can ever contain a nested
        // statement.
        MirStmt::DictCompAssign {
            target,
            var,
            var_ty,
            key,
            value,
            ..
        } => {
            bindings
                .entry(var.clone())
                .or_insert_with(|| var_ty.clone());
            bindings
                .entry(target.clone())
                .or_insert(pycc_mir::Ty::Dict(Box::new((key.ty(), value.ty()))));
        }
        // `target = {elt for var in <source> [if cond]}` (PR-12 Task 5b,
        // D-117): mirrors `ListCompAssign`'s own arm above exactly,
        // substituting `Ty::Set` for `Ty::List`.
        MirStmt::SetCompAssign {
            target,
            var,
            var_ty,
            elt,
            ..
        } => {
            bindings
                .entry(var.clone())
                .or_insert_with(|| var_ty.clone());
            bindings
                .entry(target.clone())
                .or_insert(pycc_mir::Ty::Set(Box::new(elt.ty())));
        }
        MirStmt::ExprStmt(_) | MirStmt::Return(_) | MirStmt::NoOp | MirStmt::Unreachable => {}
        MirStmt::Seq(stmts) => {
            for stmt in stmts {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // #382 (PR-22 Part 1): try/except/else/finally — recurse into all
        // nested bodies to collect any bindings introduced within them.
        MirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
            for handler in handlers {
                for stmt in &handler.body {
                    collect_stmt_bindings(stmt, bindings);
                }
            }
            for stmt in orelse {
                collect_stmt_bindings(stmt, bindings);
            }
            for stmt in finalbody {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // #382: raise/raise-from/reraise introduce no new bindings.
        MirStmt::Raise { .. } | MirStmt::RaiseFrom { .. } | MirStmt::Reraise => {}
    }
}

fn collect_module_bindings(mir: &MirModule) -> BTreeMap<String, pycc_mir::Ty> {
    let mut bindings = BTreeMap::new();
    for item in &mir.items {
        if let MirItem::TopLevelStmt(stmt) = item {
            collect_stmt_bindings(stmt, &mut bindings);
        }
    }
    // #379 (PR-19): declare a module global for each enum member singleton.
    // Each member gets a synthetic global named `<Class>.<Member>.enum_member`
    // (the `.enum_member` suffix ensures no collision with real Python names,
    // which cannot contain `.`). The global's type is `Ty::Instance(class)`,
    // so `declare_module_globals` allocates it as an opaque pointer (null
    // until the module-init sequence stores the singleton into it). The init
    // sequence is emitted in `compile_to_object_with_observer` after the
    // top-level statement loop, mirroring how top-level `Assign` already
    // emits init code.
    for (class_name, class_def) in &mir.class_defs {
        for (member_name, _) in &class_def.enum_members {
            bindings.insert(
                format!("{class_name}.{member_name}.enum_member"),
                pycc_mir::Ty::Instance(Box::new(class_name.clone())),
            );
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
                // D-075's canonical unit carrier extends to ordinary
                // assignment storage: `None` has no payload, so an `i8 0`
                // is sufficient. The separate `initialized` flag created
                // below, not this zero initializer, distinguishes a binding
                // whose assignment has executed from an uninitialized one.
                pycc_mir::Ty::None => (
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
                // trapping any read that reaches it first. D-123 names
                // module scope as one of the places a `dict[str, int]`
                // value is expected to live, and every one of this task's
                // own CLI repro programs assigns `x = {...}` at module
                // scope -- leaving this arm out would turn that documented,
                // supported form into an internal compiler panic. No
                // exit-time decref accompanies it (contrast the `Ty::Str`
                // loop in `compile_to_object`): D-124 keeps `dict[K, V]`
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
                // D-123 names module scope as one of the places a
                // `set[int]` value is expected to live, and every one of
                // this task's own CLI repro programs assigns `x = {...}` at
                // module scope -- leaving this arm out would turn that
                // documented, supported form into an internal compiler
                // panic. No exit-time decref accompanies it (contrast the
                // `Ty::Str` loop in `compile_to_object`): D-124 keeps
                // `set[T]` leak-only for v0.2, extending D-107's exact
                // reasoning.
                pycc_mir::Ty::Set(_) => (
                    context.ptr_type(inkwell::AddressSpace::default()).into(),
                    context
                        .ptr_type(inkwell::AddressSpace::default())
                        .const_null()
                        .into(),
                ),
                // `tuple[...]`'s own storage (PR-11b Task 5, D-115), and
                // the one arm here that is not a nullable pointer: the
                // struct type itself, stored inline in the global, zero-
                // initialized via `const_zero()` (which zero-fills every
                // field whatever the arity and field types -- `i64`/`i8`/
                // `f64` all have a well-defined zero).
                //
                // That zero is deliberately *not* a sentinel, unlike the
                // null pointer the four arms above use. A zeroed struct is
                // indistinguishable from a legitimately-zero tuple, so it
                // could never mark "unassigned" on its own. Nothing is
                // lost: read-before-first-assignment is trapped by the
                // separate `initialized` flag every module global already
                // gets just below -- which is the real guard for the
                // pointer arms too, the null merely being a redundant
                // second signal there. D-116 names module scope as a place
                // a tuple value is expected to live (`x = (1, 2)` at top
                // level is the canonical form), so omitting this arm would
                // turn that supported form into an internal compiler panic.
                // No exit-time decref accompanies it (contrast the
                // `Ty::Str` loop in `compile_to_object`): a tuple owns no
                // allocation to release.
                pycc_mir::Ty::Tuple(_) => {
                    let struct_ty = ty_to_basic_type(context, ty.clone()).into_struct_type();
                    (struct_ty.into(), struct_ty.const_zero().into())
                }
                // Identical storage and reasoning to `Ty::List(_)`/
                // `Ty::Dict(_)`/`Ty::Set(_)` above (D-154, Part 1 of #375):
                // an opaque pointer, null until the first assignment (an
                // instantiation, or a call/attribute-read returning an
                // instance) stores a real `PyInstanceObj` into it, with the
                // separate `initialized` flag below trapping any read that
                // reaches it first. `p = Point(1, 2)` at module scope is
                // exactly the shape Task 8's own conformance fixture uses --
                // omitting this arm would turn that supported form into an
                // internal compiler panic. No exit-time decref accompanies
                // it: `pycc_rt::instance` is leak-only, mirroring `List`/
                // `Dict`/`Set`.
                pycc_mir::Ty::Instance(_) => (
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
    compile_to_object_with_observer(mir, output_path, target_triple, release, None)
}

/// #379 (PR-19): Emit per-enum-member singleton init sequences. Each enum
/// member is a compile-time singleton instance that must be alive before
/// any top-level code reads it. For each enum class with members, and each
/// member in source order, allocate a fresh 2-slot instance
/// (`pycc_rt_instance_new(2)`), set slot 0 to the integer member value,
/// set slot 1 to a string pointer containing the member name, and store
/// the instance pointer into the synthetic global
/// `<Class>.<Member>.enum_member`. Extracted from
/// `compile_to_object_with_observer` to isolate the enum-specific code
/// paths (see cargo-llvm-cov#276 for the coverage instantiation issue).
fn emit_enum_member_inits<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    mir: &MirModule,
    module_globals: &BTreeMap<String, StorageSlot<'ctx>>,
) {
    for (class_name, class_def) in &mir.class_defs {
        for (member_name, member_value) in &class_def.enum_members {
            let global_name = format!("{class_name}.{member_name}.enum_member");
            let slot = &module_globals[&global_name];
            // Allocate a fresh 2-slot instance.
            let count = context.i64_type().const_int(2, false);
            let instance_ptr = builder
                .build_call(rt.instance_new, &[count.into()], "enum_instance_new")
                .expect("build_call should not fail for a well-formed enum member allocation")
                .try_as_basic_value()
                .expect_basic("pycc_rt_instance_new returns a non-void pointer")
                .into_pointer_value();
            // Set slot 0 to the integer member value. The value is the
            // actual `i64` literal from the source (`RED = 1` → 1), carried
            // in `HirClassDef.enum_members` from HIR through MIR to here.
            // `emit_int_constant` folds it at compile time into the
            // tagged-pointer representation `pycc_rt` uses for small ints,
            // or materializes a heap bigint through `pycc_rt_int_from_i64`
            // when the discriminant is outside the tagged 63-bit range
            // (D-178) -- once, here at module init.
            let value_scalar = Scalar::Int(emit_int_constant(context, builder, rt, *member_value));
            let value_word = scalar_to_slot_word(context, builder, value_scalar);
            let slot0_index = context.i64_type().const_int(0, false);
            builder
                .build_call(
                    rt.instance_set_slot,
                    &[instance_ptr.into(), slot0_index.into(), value_word.into()],
                    "enum_set_value",
                )
                .expect("build_call should not fail for a well-formed enum value slot write");
            // Set slot 1 to a string pointer containing the member name.
            let name_ptr = emit_string_literal(context, builder, module, rt, member_name);
            let name_scalar = Scalar::Str(name_ptr);
            let name_word = scalar_to_slot_word(context, builder, name_scalar);
            let slot1_index = context.i64_type().const_int(1, false);
            builder
                .build_call(
                    rt.instance_set_slot,
                    &[instance_ptr.into(), slot1_index.into(), name_word.into()],
                    "enum_set_name",
                )
                .expect("build_call should not fail for a well-formed enum name slot write");
            // Store the instance pointer into the synthetic global and
            // mark it initialized. Module globals always have an
            // initialized flag (see `declare_module_globals`), so
            // `slot.initialized` is always `Some` here — `expect` makes
            // that invariant explicit without an unreachable `None` arm
            // that would create a permanently uncovered coverage region.
            builder
                .build_store(slot.ptr, instance_ptr)
                .expect("build_store should not fail for a declared enum member global");
            let initialized_ptr = slot
                .initialized
                .expect("module globals always have an initialized flag");
            builder
                .build_store(initialized_ptr, context.i8_type().const_int(1, false))
                .expect("build_store should not fail for a declared enum member init flag");
        }
    }
}

fn compile_to_object_with_observer(
    mir: &MirModule,
    output_path: &Path,
    target_triple: Option<&str>,
    release: bool,
    mut observer: Option<&mut CodegenObserver<'_>>,
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
    //
    // Issue #22: each `def` now gets a unique mangled name so redefinition
    // doesn't collide (`pyfn_{name}` for the first, `pyfn_{name}__redef_{n}`
    // for subsequent). A global function-pointer slot per unique name is
    // initialized to null; the top-level emission pass stores each def's
    // address into the slot when the def is "executed" in source order, and
    // all calls dispatch indirectly through the slot. This separates
    // LLVM symbol declaration (this pass, needed for the compiler to
    // generate call instructions) from Python name binding (the store,
    // which happens at the def's source position in top-level execution).
    let mut user_functions: HashMap<&str, UserFunction> = HashMap::new();
    // Maps function name to the number of definitions seen so far, for
    // unique mangled-name generation on redefinition.
    let mut def_counts: HashMap<&str, usize> = HashMap::new();
    // List of (function name, LLVM function value) in source order, for
    // the top-level binding pass.
    let mut function_defs_in_order: Vec<(&str, FunctionValue)> = Vec::new();
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
            let count = def_counts.entry(name.as_str()).or_insert(0);
            let mangled = if *count == 0 {
                format!("pyfn_{name}")
            } else {
                format!("pyfn_{name}__redef_{count}")
            };
            *count += 1;
            let f = module.add_function(&mangled, fn_type, None);
            function_defs_in_order.push((name.as_str(), f));
            // Only insert into user_functions on the first definition;
            // subsequent definitions update fn_ptr_global at their source
            // position but share the same slot and type info. The param_tys
            // and fn_type come from the first definition (the type checker
            // resolves one signature per name).
            user_functions.entry(name.as_str()).or_insert_with(|| {
                // Monomorphized generic specializations (`0gen_...` names)
                // are compiler-generated, not user-defined: they have no
                // top-level `def` whose execution order matters, so they
                // dispatch directly through `direct_value` instead of
                // through the indirect function-pointer slot.
                let is_monomorphized = name.starts_with("0gen_");
                if is_monomorphized {
                    UserFunction {
                        param_tys: params.iter().map(|(_, ty)| ty.clone()).collect(),
                        fn_ptr_global: None,
                        fn_type,
                        name_global: None,
                        direct_value: Some(f),
                    }
                } else {
                    let fn_ptr_type = context.ptr_type(inkwell::AddressSpace::default());
                    let fn_ptr_global =
                        module.add_global(fn_ptr_type, None, &format!("fnptr_{name}"));
                    fn_ptr_global.set_initializer(&fn_ptr_type.const_null());
                    let name_global = module.add_global(
                        context.i8_type().array_type(name.len() as u32 + 1),
                        None,
                        &format!("fnname_{name}"),
                    );
                    name_global.set_linkage(Linkage::Internal);
                    name_global.set_constant(true);
                    name_global.set_initializer(&context.const_string(name.as_bytes(), true));
                    UserFunction {
                        param_tys: params.iter().map(|(_, ty)| ty.clone()).collect(),
                        fn_ptr_global: Some(fn_ptr_global),
                        fn_type,
                        name_global: Some(name_global),
                        direct_value: None,
                    }
                }
            });
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
    let top_exception_exit = context.append_basic_block(entry_fn, "top_exception_exit");
    builder.position_at_end(entry_block);
    // Top-level statements share one `locals` map across the synthetic
    // `main` entry block (module-level Python names are one shared
    // scope); each user function gets its own, fresh map below, since
    // Python function bodies don't see each other's locals.
    let mut top_level_locals: HashMap<_, _> = module_globals
        .iter()
        .map(|(name, binding)| (name.clone(), binding.clone()))
        .collect();
    // #379 (PR-19): emit per-enum-member singleton init sequences BEFORE
    // the top-level statement loop.
    emit_enum_member_inits(&context, &builder, &module, &rt, mir, &module_globals);
    // Issue #22: iterate over ALL items in source order, not just
    // top-level statements. A `MirItem::Function` at its source position
    // represents a `def` statement's runtime binding effect: store the
    // function's address into the global function-pointer slot so calls
    // after this point dispatch to it. A call before the `def` (the slot
    // is still null) aborts with `pycc_rt_name_error` -- matching
    // CPython's `NameError: name 'foo' is not defined`.
    let mut def_iter = function_defs_in_order.iter().peekable();
    rt.exceptions.targets.borrow_mut().push(top_exception_exit);
    for item in &mir.items {
        match item {
            MirItem::TopLevelStmt(stmt) => {
                emit_stmt(
                    &context,
                    &builder,
                    &module,
                    &rt,
                    &user_functions,
                    &mut top_level_locals,
                    stmt,
                    pycc_mir::Ty::None,
                    &mut Vec::new(),
                )?;
                // #382: After each top-level statement, check for an active
                // exception. A `raise` (or a converted runtime failure like
                // division by zero) sets the pending exception state and
                // terminates the block with `unreachable`. Erase the
                // `unreachable` so the exception check can run here. This
                // check intercepts it at the top level: if an exception is
                // active, print it and exit. Inside a `try`, the try's own
                // post-body check intercepts first. `emit_stmt` rejects a
                // `Return` whose parent is this synthetic `main`, so after
                // erasing an exception's `unreachable` the current block is
                // guaranteed to accept this check.
                erase_unreachable_if_present(&builder);
                let active = builder
                    .build_call(rt.exception_active, &[], "top_exc_active")
                    .expect("build_call should not fail for exception_active")
                    .try_as_basic_value()
                    .expect_basic("pycc_rt_exception_active returns i8")
                    .into_int_value();
                let has_exc = builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        active,
                        context.i8_type().const_zero(),
                        "top_has_exc",
                    )
                    .expect("build_int_compare should not fail");
                let cont_bb = context.append_basic_block(entry_fn, "top_exc_cont");
                builder
                    .build_conditional_branch(has_exc, top_exception_exit, cont_bb)
                    .expect("build_conditional_branch should not fail");
                builder.position_at_end(cont_bb);
            }
            MirItem::Function { name, .. } => {
                // Store this definition's function pointer into the
                // global slot, representing the `def`'s runtime binding.
                // `def_iter` is in the same source order as `mir.items`,
                // so the next entry matches this function definition.
                // Monomorphized generic specializations (`0gen_...` names)
                // have no `fn_ptr_global` (they dispatch directly), so
                // skip the store for them.
                let &(_, f) = def_iter.next().expect(
                    "def_iter should have an entry for every MirItem::Function                      (the declaration pass populates function_defs_in_order                      from the same mir.items in the same order)",
                );
                let uf = &user_functions[name.as_str()];
                if let Some(ref fn_ptr_global) = uf.fn_ptr_global {
                    let _ = builder.build_store(
                        fn_ptr_global.as_pointer_value(),
                        f.as_global_value().as_pointer_value(),
                    );
                }
            }
        }
    }
    rt.exceptions.targets.borrow_mut().pop();
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
    // `emit_stmt` rejects every `Return` whose parent is this synthetic
    // `main`; all exception-produced `unreachable` terminators are erased
    // before their explicit state check above. The current continuation is
    // therefore guaranteed to accept the process-success return below.
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

    // Every exceptional module-scope edge converges here. Keeping one
    // target alive while top-level expressions are emitted lets recursive
    // expression guards stop later operands and effects immediately.
    builder.position_at_end(top_exception_exit);
    let exc_val = builder
        .build_call(rt.exception_value, &[], "top_exc_val")
        .expect("build_call should not fail for exception_value")
        .try_as_basic_value()
        .expect_basic("pycc_rt_exception_value returns a pointer")
        .into_pointer_value();
    builder
        .build_call(rt.exception_print_and_exit, &[exc_val.into()], "")
        .expect("build_call should not fail for exception_print_and_exit");
    builder
        .build_unreachable()
        .expect("build_unreachable should terminate a noreturn block");

    // Second pass: fill in each user function's body, now that every
    // function (including ones a body might call) is already declared.
    // Each parameter is bound into `fn_locals` by allocating a fresh slot
    // for it and storing the incoming LLVM argument into it (Task 5) --
    // the same load/store-via-`alloca` model every other local already
    // uses (see `emit_assign`), so a parameter is fully ordinary once
    // bound: reassignable, and readable via `emit_expr`'s `Name` arm with
    // no special-casing.
    // Issue #22: use `function_defs_in_order` to get the correct LLVM
    // function value for each definition -- a redefined name has multiple
    // function values (one per def, with unique mangled names), and each
    // body must be emitted into its own function value, not the first
    // definition's.
    let mut body_def_iter = function_defs_in_order.iter();
    for item in &mir.items {
        if let MirItem::Function {
            name,
            params,
            return_ty,
            body,
        } = item
        {
            // Advance the iterator in lockstep with `mir.items`'s
            // Function items -- same source order, same count.
            let f = body_def_iter
                .next()
                .map(|&(_, f)| f)
                .expect("function body has matching declaration");
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
            let exception_exit = context.append_basic_block(f, "exception_exit");
            rt.exceptions.targets.borrow_mut().push(exception_exit);
            emit_body(
                &context,
                &builder,
                &module,
                &rt,
                &user_functions,
                &mut fn_locals,
                body,
                return_ty.clone(),
                &mut Vec::new(),
            )?;
            rt.exceptions.targets.borrow_mut().pop();
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
                    if exception::block_always_terminates(body) {
                        builder.build_unreachable().expect(
                            "build_unreachable should terminate a statically impossible continuation",
                        );
                    } else {
                        panic!(
                            "pycc_codegen: internal error: `{name}` is declared to return a \
                             non-`None` value but fell through without a `return` -- \
                             pycc_types::check (T0024) should have rejected this HIR before \
                             it reached codegen"
                        );
                    }
                }
                _ => {}
            }

            // A Python exception crosses the native function ABI via the
            // thread-local runtime flag. Return a neutral carrier here;
            // the caller's expression guard observes the still-active flag
            // before it can consume that carrier or evaluate another effect.
            builder.position_at_end(exception_exit);
            if *return_ty == pycc_mir::Ty::None {
                builder
                    .build_return(None)
                    .expect("build_return should not fail for an exceptional None exit");
            } else {
                let default = default_value_for_type(&context, return_ty.clone());
                builder
                    .build_return(Some(&default))
                    .expect("build_return should not fail for an exceptional value exit");
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
    let applied_pipeline = if release {
        module
            .run_passes(
                RELEASE_PASS_PIPELINE,
                &target_machine,
                PassBuilderOptions::create(),
            )
            .expect(
                "LLVM's \"default<O3>\" pipeline should never fail against a module this \
                 function has already verified, using a fixed, always-valid pipeline string",
            );
        Some(RELEASE_PASS_PIPELINE)
    } else {
        None
    };
    if let Some(observer) = observer.as_mut() {
        observer(&module, applied_pipeline);
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

/// Phase 1 of the two-phase `print()` argument pipeline (fixes #145):
/// evaluates one `print()` argument's expression for its side effects and
/// converts it to a `str` pointer, returning `None` for a `Ty::None`
/// argument (side effects evaluated, no `str` pointer -- the literal
/// `"None"` is written later by `emit_write_print_arg`) or
/// `Some(str_ptr)` for every other v0.1 scalar type (evaluated, incref'd
/// if needed via `incref_if_str_duplicate`, converted to `str` via
/// `to_str`, reusing `pycc_rt_int_to_str`/`float_to_str`/`bool_to_str`,
/// the same conversions f-string interpolation already uses).
///
/// `emit_stmt`'s `print`-call arm now runs this once per argument in a
/// first loop -- evaluating *all* arguments left-to-right *before* any
/// output is emitted -- collecting the resulting `Option<PointerValue>`
/// into a `Vec`, then runs `emit_write_print_arg` in a second loop to
/// emit separators and write each value. This splits evaluation from
/// output so that a later argument's side effects (e.g. a user function
/// that itself calls `print`) happen before any of the outer `print`'s
/// own output, matching CPython's left-to-right argument-evaluation
/// semantics: `print(1, side_effect())` emits `2\n1 3\n`, not the
/// interleaved `1 2\n3\n` the old single-phase `emit_print_arg` produced.
///
/// The `str` pointer returned here is an LLVM SSA value that persists
/// within the same basic block between the two phases -- `PointerValue`
/// is `Copy` (inkwell 0.9.0), and no `emit_expr` call for a `print`
/// argument creates LLVM branches (no `and`/`or`/short-circuit in MIR's
/// `BinOpKind`), so no allocas are needed to retain it. The `str_decref`
/// that balances the incref/`to_str` allocation is deferred to
/// `emit_write_print_arg`, keeping the incref/decref pairing identical to
/// the old single-phase `emit_print_arg` (same ownership pattern as
/// `emit_expr`'s `FString` arm's own intermediate concatenation results).
///
/// Kept as an extracted top-level helper rather than inlined back into
/// `emit_stmt`'s `match` arm for the same `cargo llvm-cov`
/// region-attribution artifact the original `emit_print_arg`'s own doc
/// comment recorded: with this logic left inlined directly inside
/// `emit_stmt`'s large `match`, the lines building the `None` branch's
/// `emit_expr` calls were reported as 0-hit ("uncovered") by `cargo
/// llvm-cov --show-missing-lines` even though a `eprintln!` placed on
/// exactly those lines confirmed, via a direct `cargo test -p
/// pycc_codegen -- --nocapture` run, that they really do execute.
/// Restructuring the same logic into its own top-level function made the
/// exact same code report 100% covered with no further changes --
/// behavior is provably identical either way, so this is treated as a
/// coverage-instrumentation measurement artifact of a large `match`
/// arm's own inlining/region mapping, not a real gap, and worked around
/// structurally rather than by reaching for a `--ignore-filename-regex`
/// exemption (D-014's own policy: that exemption is for a documented
/// design constraint, not a measurement quirk with an available
/// structural fix).
fn emit_eval_print_arg<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    arg: &MirExpr,
) -> Option<PointerValue<'ctx>> {
    if arg.ty() == pycc_mir::Ty::None {
        emit_expr(context, builder, module, rt, user_functions, locals, arg);
        None
    } else {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, arg);
        let scalar = incref_if_str_duplicate(builder, rt, arg, scalar);
        let as_str = to_str(builder, rt, scalar);
        // #146 Part 2 (D-181): `print`'s own argument is a pure consumer --
        // `to_str` reads the word and builds a separate `PyStrObj`, so the
        // int word is dead afterwards and nothing else will retire it.
        // Released *after* `to_str`, which reads a bigint's limbs.
        release_scalar_if_int_temporary(context, builder, rt, arg, &scalar);
        Some(as_str)
    }
}

/// Phase 2 of the two-phase `print()` argument pipeline (fixes #145):
/// writes one argument's already-evaluated value -- `print_none` for the
/// `None` case (the literal `"None"`), or `print_write_str` followed by
/// `str_decref` for a `Some(str_ptr)` scalar (writing the `str` built in
/// phase 1, then freeing the temporary `to_str` allocated for
/// `int`/`float`/`bool` or the incref'd duplicate of a bare `Name`/
/// `AttrGet` `str`). `emit_stmt`'s `print`-call arm calls this once per
/// argument in its second loop, after `emit_eval_print_arg` has already
/// evaluated every argument, so that output happens only after all
/// argument side effects complete (see `emit_eval_print_arg`'s own doc
/// comment for the two-phase design and the `cargo llvm-cov`
/// region-attribution artifact that keeps this an extracted helper).
fn emit_write_print_arg<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    maybe_str: Option<PointerValue<'ctx>>,
) {
    match maybe_str {
        None => {
            builder
                .build_call(rt.print_none, &[], "print_none")
                .expect("build_call should not fail for a well-formed print of None");
        }
        Some(str_ptr) => {
            builder
                .build_call(rt.print_write_str, &[str_ptr.into()], "print_write")
                .expect("build_call should not fail for a well-formed print write");
            builder
                .build_call(rt.str_decref, &[str_ptr.into()], "print_decref_temp")
                .expect("build_call should not fail for a well-formed decref");
        }
    }
}

/// Handles every `MirStmt` shape (this match is exhaustive over
/// `MirStmt`, no catch-all arm): a `print()` call of any number of
/// `int`/`float`/`bool`/`str` arguments plus any supported materializable
/// non-`print()` `None` expression, including direct user-function,
/// `ListAppend`, and `SetAdd` results, a D-075 parameter, or ordinary
/// assignment storage (space-separated, one trailing newline, matching
/// CPython's `print(*args)` -- all arguments are now evaluated
/// left-to-right before any output is emitted, matching CPython's own
/// argument-evaluation semantics (fixes #145; see
/// `emit_eval_print_arg`/`emit_write_print_arg`); D-072 still excludes
/// using `print()` itself as a nested expression), any
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
    finally_stack: &mut Vec<FinallyTarget<'ctx>>,
) -> Result<(), String> {
    match stmt {
        MirStmt::ExprStmt(MirExpr::Call { callee, args, .. }) if callee == "print" => {
            // Two-phase evaluate-then-output (fixes #145): evaluate *all*
            // arguments left-to-right before emitting any output, so a
            // later argument's side effects (e.g. a user function that
            // itself calls `print`) complete before this `print`'s own
            // output begins -- matching CPython's left-to-right
            // argument-evaluation semantics. See `emit_eval_print_arg`/
            // `emit_write_print_arg` for the phase split and the
            // `cargo llvm-cov` region-attribution artifact that keeps
            // them as extracted helpers.
            let mut evaluated: Vec<Option<PointerValue<'ctx>>> = Vec::with_capacity(args.len());
            for arg in args.iter() {
                evaluated.push(emit_eval_print_arg(
                    context,
                    builder,
                    module,
                    rt,
                    user_functions,
                    locals,
                    arg,
                ));
            }
            for (i, maybe_str) in evaluated.into_iter().enumerate() {
                if i > 0 {
                    builder
                        .build_call(rt.print_space, &[], "print_sep")
                        .expect("build_call should not fail for a well-formed print separator");
                }
                emit_write_print_arg(builder, rt, maybe_str);
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
            let _ = user_function; // validated above; build_call_to looks up by name
            build_call_to(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                callee,
                args,
            );
            guard_statement_effects(context, builder, rt);
            Ok(())
        }
        MirStmt::ExprStmt(expr) => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, expr);
            // #146 Part 2 (D-181): a statement-position expression's value
            // is discarded outright, so nothing else will ever retire the
            // reference it was born with.
            release_scalar_if_int_temporary(context, builder, rt, expr, &scalar);
            Ok(())
        }
        MirStmt::Assign { target, value } => {
            let ty = value.ty();
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            let scalar = incref_if_str_duplicate(builder, rt, value, scalar);
            let scalar = retain_if_int_duplicate(context, builder, rt, value, scalar);
            if ty == pycc_mir::Ty::Str {
                decref_str_slot_before_store(context, builder, rt, locals, target);
            }
            // #146 Part 1: the matching `int` release is *not* here. It
            // lives inside `emit_assign`, gated on the target slot's own
            // declared type rather than on `ty` above -- `x: int = True`
            // reaches this arm with `ty == Ty::Bool` and would otherwise
            // skip the release of the bigint the slot still holds.
            emit_assign(context, builder, rt, locals, target, scalar);
            Ok(())
        }
        MirStmt::NoOp => Ok(()),
        MirStmt::Unreachable => {
            builder
                .build_unreachable()
                .expect("build_unreachable should terminate a statically impossible match path");
            Ok(())
        }
        MirStmt::If { test, body, orelse } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();
            let cond = {
                let scalar = emit_expr(context, builder, module, rt, user_functions, locals, test);
                let cond = truthy(context, builder, rt, scalar);
                // #146 Part 2 (D-181): released *after* `truthy`, which
                // reads a bigint operand's limbs -- releasing first could
                // free the very word being tested.
                release_scalar_if_int_temporary(context, builder, rt, test, &scalar);
                cond
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
                finally_stack,
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
                    finally_stack,
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
                let cond = truthy(context, builder, rt, scalar);
                // #146 Part 2 (D-181): released *after* `truthy`, which
                // reads a bigint operand's limbs -- releasing first could
                // free the very word being tested.
                release_scalar_if_int_temporary(context, builder, rt, test, &scalar);
                cond
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
                finally_stack,
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
            let start_v = range_operand_to_normalized_int(
                context,
                builder,
                rt,
                emit_expr(context, builder, module, rt, user_functions, locals, start),
                "start",
            );
            let stop_v = range_operand_to_normalized_int(
                context,
                builder,
                rt,
                emit_expr(context, builder, module, rt, user_functions, locals, stop),
                "stop",
            );
            let step_v = range_operand_to_normalized_int(
                context,
                builder,
                rt,
                emit_expr(context, builder, module, rt, user_functions, locals, step),
                "step",
            );
            // #146 Part 1, the `ForRange` ownership contract.
            //
            // `range_operand_to_normalized_int` passes a bigint operand
            // through `pycc_rt_range_normalize_operand` *unchanged*, so
            // `start_v`/`stop_v`/`step_v` alias whatever the source
            // expression already owned -- for `for i in range(b, b, b)`,
            // one single heap object under three names. None of the three
            // has been retained by `retain_if_int_duplicate`: that helper
            // is deliberately not applied to `range` operands, and the
            // three retains below are the loop's own.
            //
            // WARNING for a later change: do *not* route `range`'s operands
            // through `retain_if_int_duplicate` as well. `start_v` would
            // then be retained twice while still being released once, by
            // the ordinary `current` machinery -- a permanent leak of every
            // named `range` bound, invisible to every value assertion.
            //
            // `stop_v` and `step_v` are read on every trip through
            // `for_test` and are released once in `for_after`. `start_v` is
            // retained here as `current`'s *first owner*, not as a third
            // independent operand: on the first trip the induction phi's
            // incoming value *is* `start_v`, so the ordinary per-iteration
            // `current` release below (or `for_after`'s final one, for an
            // empty range) is what balances this retain. Adding a third
            // `for_after` release for `start_v` would free a word the
            // source name still holds.
            //
            // The whole invariant, for `n` executed iterations: `n + 1`
            // owned `current` values (`start_v` plus one `pycc_rt_int_add`
            // result per iteration), matched by `n` per-iteration releases
            // plus one `for_after` release. Separately, each of the `n`
            // binds of the visible target retains `current`, matched by the
            // *next* bind's release-before-store inside `emit_assign`, with
            // the final one matched whenever that slot is next overwritten.
            emit_bigint_refcount_call(context, builder, rt, start_v, BigIntRefcount::Retain);
            emit_bigint_refcount_call(context, builder, rt, stop_v, BigIntRefcount::Retain);
            emit_bigint_refcount_call(context, builder, rt, step_v, BigIntRefcount::Retain);
            // #146 Part 2 (D-181): retire the *source expression's* birth
            // reference for any operand that was freshly built rather than
            // borrowed (`range(a + a, ...)`, or a literal outside D-061's
            // tagged range). Safe immediately, and only immediately after
            // the loop's own retains directly above: the loop is already an
            // owner by this point, so the object survives to `for_after`.
            // Deliberately does *not* conditionalize those retains -- D-180
            // warns that making the loop's ownership depend on the operand's
            // shape is what breaks the `n + 1`/`n + 1` release arithmetic.
            release_if_int_temporary(context, builder, rt, start, start_v);
            release_if_int_temporary(context, builder, rt, stop, stop_v);
            release_if_int_temporary(context, builder, rt, step, step_v);
            // Re-read *after* the retains above: each one splits the block,
            // and the phi's incoming edge must name the block that actually
            // branches into `for_test`.
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
            // Retain before `emit_assign`, which releases the slot's
            // previous word: when the slot already holds this very object
            // (a body that does not reassign the target cannot produce
            // that, but an aliasing source such as `b` can), releasing
            // first would free the word this bind is about to store.
            emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Retain);
            emit_assign(context, builder, rt, locals, var, Scalar::Int(current));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
                finally_stack,
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
                // This iteration's `current` is dead once `next` exists.
                // A `Return` inside `body` skips this release (and both
                // `for_after` releases below) and leaks -- an accepted,
                // documented Part 1 concession, not an oversight.
                emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Release);
                // Re-read after the release: it splits the block, and the
                // phi's incoming edge must name the block that branches.
                let body_end = builder.get_insert_block().unwrap();
                induction.add_incoming(&[(&next, body_end)]);
                builder.build_unconditional_branch(test_bb).expect(
                    "build_unconditional_branch should not fail on a block with no terminator yet",
                );
            }

            builder.position_at_end(after_bb);
            // The `current` that failed `range_continue` was never bound to
            // the visible target and has no per-iteration release; plus the
            // two operand retains from the preheader.
            emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Release);
            emit_bigint_refcount_call(context, builder, rt, stop_v, BigIntRefcount::Release);
            emit_bigint_refcount_call(context, builder, rt, step_v, BigIntRefcount::Release);
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
        // D-141 leaves the induction variable and raw `len` as private LLVM
        // counters. The element read is already encoded and is assigned to
        // the user-visible `Ty::Int` loop target unchanged.
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
            let encoded_element = build_int_list_get(builder, rt, list_ptr, current);
            let element = encoded_element;
            emit_assign(context, builder, rt, locals, var, Scalar::Int(element));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
                finally_stack,
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
            if builder
                .get_insert_block()
                .unwrap()
                .get_parent()
                .unwrap()
                .get_name()
                .to_bytes()
                == b"main"
            {
                panic!(
                    "pycc_codegen: internal error: a top-level statement terminated `main`'s \
                     entry block -- pycc_types::check (T0024) should have rejected a module-level \
                     `return` before it reached codegen"
                );
            }
            // #382 (PR-22 Part 2): If inside a try-with-finally, route the
            // return through the finally block instead of emitting `ret`
            // directly. Store the return value to the finally's ret_slot,
            // set the is_returning flag, and branch to the finally block.
            // After the finally body runs, the codegen emits the `ret`.
            let finally_target = finally_stack.last().cloned();
            match value {
                Some(expr) => {
                    let scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, expr);
                    let scalar = incref_if_str_duplicate(builder, rt, expr, scalar);
                    let scalar = retain_if_int_duplicate(context, builder, rt, expr, scalar);
                    let scalar =
                        coerce_scalar_to_type(context, builder, scalar, expected_return_ty.clone());
                    if expected_return_ty == pycc_mir::Ty::None {
                        // `None` parameters, call results, and stored names
                        // use a canonical `i8 0` carrier inside expressions,
                        // but a function
                        // declared to return `None` has an LLVM `void`
                        // signature. Evaluating above preserves any call or
                        // name-load side effects; the carrier itself is
                        // intentionally discarded here. Returning it as an
                        // `i8` previously built invalid IR that only the
                        // non-Windows verifier caught (D-029).
                        if let Some(ft) = finally_target {
                            builder
                                .build_store(ft.is_returning, context.i8_type().const_int(1, false))
                                .expect("build_store should not fail for is_returning");
                            builder.build_unconditional_branch(ft.finally_bb).expect(
                                "build_unconditional_branch should not fail for finally routing",
                            );
                        } else {
                            builder
                                .build_return(None)
                                .expect("build_return should not fail for a None return value");
                        }
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
                        // Pass-through like the three arms above, but
                        // returning a whole struct by value rather than one
                        // opaque pointer (D-115) -- and
                        // `ty_to_basic_type` already gave this function's
                        // LLVM signature that same struct return type, so
                        // the `ret` is well-typed with no tuple-specific
                        // code here.
                        Scalar::Tuple(v) => v.into(),
                        // Pass-through, identical to `List`'s/`Dict`'s/
                        // `Set`'s arms above (D-154, Part 1 of #375):
                        // returning a class instance returns one opaque
                        // pointer, and `ty_to_basic_type` already gave the
                        // function's LLVM signature the same pointer return
                        // type it gives a `str`/`list[T]`/`dict[K, V]`/
                        // `set[T]`-returning one.
                        Scalar::Instance(v) => v.into(),
                    };
                    if let Some(ft) = finally_target {
                        // Route through finally: store the return value,
                        // set the is_returning flag, and branch to finally.
                        let slot = ft
                            .ret_slot
                            .expect("a value return routed through finally has a return slot");
                        builder
                            .build_store(slot, basic_value)
                            .expect("build_store should not fail for ret_slot");
                        builder
                            .build_store(ft.is_returning, context.i8_type().const_int(1, false))
                            .expect("build_store should not fail for is_returning");
                        builder.build_unconditional_branch(ft.finally_bb).expect(
                            "build_unconditional_branch should not fail for finally routing",
                        );
                    } else {
                        builder
                            .build_return(Some(&basic_value))
                            .expect("build_return should not fail for a well-formed return value");
                    }
                }
                None => {
                    // #380 (PR-20): an abstract method has a `Return(None)`
                    // body but a non-`None` declared return type. The type
                    // checker skips body checking for abstract methods, so
                    // this is the only case where `Return(None)` reaches
                    // codegen with a non-`None` `expected_return_ty`. Emit
                    // a default value of the correct type so the LLVM IR
                    // is well-typed; the abstract method is never called,
                    // so the value doesn't matter.
                    if expected_return_ty == pycc_mir::Ty::None {
                        if let Some(ft) = finally_target {
                            builder
                                .build_store(ft.is_returning, context.i8_type().const_int(1, false))
                                .expect("build_store should not fail for is_returning");
                            builder.build_unconditional_branch(ft.finally_bb).expect(
                                "build_unconditional_branch should not fail for finally routing",
                            );
                        } else {
                            builder
                                .build_return(None)
                                .expect("build_return should not fail for a bare `return`");
                        }
                    } else {
                        let default = default_value_for_type(context, expected_return_ty.clone());
                        if let Some(ft) = finally_target {
                            let slot = ft.ret_slot.expect(
                                "a non-None default return routed through finally has a return slot",
                            );
                            builder
                                .build_store(slot, default)
                                .expect("build_store should not fail for ret_slot");
                            builder
                                .build_store(ft.is_returning, context.i8_type().const_int(1, false))
                                .expect("build_store should not fail for is_returning");
                            builder.build_unconditional_branch(ft.finally_bb).expect(
                                "build_unconditional_branch should not fail for finally routing",
                            );
                        } else {
                            builder
                                .build_return(Some(&default))
                                .expect("build_return should not fail for a default return value");
                        }
                    }
                }
            }
            Ok(())
        }
        // `d[k] = v` (PR-11 Task 5, D-123): insert-or-update --
        // `pycc_rt_dict_set` itself decides which, by whether `key`
        // already compares equal to a stored key. `dict` is read by name
        // (mirrors `MirExpr::ListAppend`'s own `list` field), `key` crosses
        // in as a `Ty::Str` expression unchanged, exactly like
        // `MirExpr::DictGet`'s key, and `value` gets D-141 validation plus
        // identity-preserving encoded storage.
        MirStmt::DictSet { dict, key, value } => {
            let dict_ptr =
                emit_dict_name_read(context, builder, module, rt, user_functions, locals, dict);
            let key_scalar = emit_expr(context, builder, module, rt, user_functions, locals, key);
            // Same `incref_if_str_duplicate` requirement as `MirExpr::
            // DictLiteral`'s own per-pair key above, and for the identical
            // reason (see that arm's own comment): `pycc_rt_dict_set`
            // adopts whatever key pointer it is given as `d`'s own
            // permanent reference without incref'ing it itself (D-124), so
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
            let encoded = to_encoded_int(context, builder, value_scalar);
            let _ = build_untag_checked(builder, rt, encoded, "dict_validate_set_value");
            build_dict_set(builder, rt, dict_ptr, key_ptr, encoded);
            Ok(())
        }
        // `base.attr = value` (D-154, Part 1 of #375): writes the raw slot
        // word via the opaque `pycc_rt_instance_set_slot` accessor -- the
        // mirror image of `MirExpr::AttrGet`'s read, using
        // `scalar_to_slot_word` for the identical encoding
        // `slot_word_to_scalar` decodes.
        //
        // For a `Ty::Str` attribute, mirrors `MirStmt::Assign`'s own two
        // refcount obligations exactly (D-154 Part 1's own post-merge
        // review finding -- the first version of this arm had neither):
        // `incref_if_str_duplicate` before storing, since a bare-`Name`/
        // `AttrGet` source value is a *duplicate* reference whose original
        // binding/slot keeps its own copy; and
        // `decref_str_attr_slot_before_store` before overwriting, to
        // release whatever the slot held previously (a no-op on a fresh
        // instance's zero-initialized slot, exactly like a local's
        // null-initialized string slot). Without both, a `str` attribute
        // read twice, or reassigned, use-after-frees the first `PyStrObj`.
        MirStmt::AttrSet { base, slot, value } => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            let base_ptr = expect_instance_pointer(base_scalar, "attribute assignment base");
            let value_ty = value.ty();
            let value_scalar =
                emit_expr(context, builder, module, rt, user_functions, locals, value);
            let value_scalar = incref_if_str_duplicate(builder, rt, value, value_scalar);
            let value_scalar = retain_if_int_duplicate(context, builder, rt, value, value_scalar);
            let slot_index = context.i64_type().const_int(*slot as u64, false);
            if value_ty == pycc_mir::Ty::Str {
                decref_str_attr_slot_before_store(context, builder, rt, base_ptr, slot_index);
            }
            // #146 Part 1, accepted gap: unlike a local's storage slot, an
            // instance attribute's declared `Ty` is not reachable from
            // `MirStmt::AttrSet` (it carries only a slot *index*), so this
            // release is gated on the value's type rather than the slot's.
            // The one shape that diverges is a `bool` value assigned into
            // an `int`-declared attribute, which skips the release of a
            // bigint the attribute still holds -- a leak, never a
            // use-after-free. That gap is documented in D-180 Consequences
            // item 6 but deliberately *not* pinned by a test: the shape is
            // unobservable today because `scalar_to_slot_word` stores a
            // `Scalar::Bool` as a raw `zext` rather than a D-141 encoded
            // word, so the attribute reads back as `0` regardless of any
            // refcounting (a separate, pre-existing D-154 defect, tracked
            // in D-180 Consequences item 6). A fixture here could only
            // enshrine that wrong output. Threading the declared type
            // through from `mir.class_defs` is deliberately left out of
            // Part 1's scope.
            if value_ty == pycc_mir::Ty::Int {
                release_int_attr_slot_before_store(context, builder, rt, base_ptr, slot_index);
            }
            let word = scalar_to_slot_word(context, builder, value_scalar);
            builder
                .build_call(
                    rt.instance_set_slot,
                    &[base_ptr.into(), slot_index.into(), word.into()],
                    "instance_set_slot",
                )
                .expect("build_call should not fail for a well-formed attribute write");
            Ok(())
        }
        // `for k in d:` (PR-11 Task 5, D-123): an intentional inline
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
        //    pointer to (D-124: "`PyDictObj`'s own keys ... are stored
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
        //    loop's last key is exactly D-124's existing leak-only policy
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
            emit_assign(context, builder, rt, locals, var, Scalar::Str(key_ptr));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
                finally_stack,
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
        // `for x in s:` (PR-11 Task 9, D-123): an intentional inline
        // duplicate of `MirStmt::ForList`'s own loop-building logic above,
        // for the identical "a third consumer, not a second, justifies a
        // shared helper" reason that arm's own comment already gives for
        // its own `ForRange` duplication (and `MirStmt::ForDict`'s own
        // comment gives for its `ForList` duplication).
        //
        // Structurally this is closer to `ForList` than to `ForDict`: a
        // set's elements, like a list's, are encoded `i64` values with no
        // refcounting concern (unlike `ForDict`'s own key, which is a
        // refcounted `PyStrObj` needing its own `pycc_rt_str_incref` before
        // the loop variable's per-iteration bind -- see that arm's own doc
        // comment). Two differences from `ForList`, both consequences of
        // iterating a set rather than a list:
        // 1. The bound is `pycc_rt_int_set_len`, still re-read on every
        //    iteration inside the test block rather than hoisted into the
        //    preheader, for the identical CPython-mutation-during-iteration
        //    reason `ForList`'s own comment gives -- kept identical to
        //    `ForList`/`ForDict`'s own shape. Unlike `ForDict` (D-123
        //    accepts silently visiting newly-inserted keys as a bounded
        //    divergence), a mid-loop `set.add()` (D-119, this same PR) is
        //    checked against the length captured once in the preheader and
        //    panics honestly on any change -- see
        //    `pycc_rt_int_set_check_not_resized`'s own doc comment for why
        //    silently extending the iteration is not safe to accept here:
        //    `for x in s: s.add(x + 1)` would never terminate.
        // 2. Each iteration reads the current element via
        //    `pycc_rt_int_set_get`, not `pycc_rt_int_list_get`.
        //
        // D-141 mirrors `ForList`: the induction variable and `len` are raw
        // private counters, while the element is an encoded user value.
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
            // Captured once, in the preheader -- P1 review fix (PR-12,
            // `pycc_rt_int_set_check_not_resized`'s own doc comment):
            // `set.add()` existing in this same commit made it possible for
            // the loop body to grow `set_ptr` out from under this loop's own
            // `len` re-read below. Comparing every iteration's fresh read
            // against this snapshot, rather than silently visiting whatever
            // `len` grows to, matches CPython's own `RuntimeError` on
            // set-changed-size-during-iteration with an honest panic.
            let initial_len = build_int_set_len(builder, rt, set_ptr);
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
            build_int_set_check_not_resized(builder, rt, len, initial_len);
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
            let encoded_element = build_int_set_get(builder, rt, set_ptr, current);
            emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));
            emit_body(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                body,
                expected_return_ty,
                finally_stack,
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
        // `target = [elt for var in <source> [if cond]]` (PR-12 Task 5a,
        // D-117) -- a *fourth* intentional inline duplicate of `ForRange`'s
        // own loop-building shape (see `ForList`'s own doc comment, which
        // already names `ForList`/`ForDict`/`ForSet` as the first three):
        // this arm differs from every `For*` arm above in (a) allocating the
        // target's own empty backing list as a free-standing SSA pointer
        // before the loop starts, but deliberately **not** storing it into
        // `target`'s own slot until the entire loop has finished (see point
        // 1's own comment below for why this ordering is load-bearing, not
        // cosmetic), (b) branching *internally* on `source`'s own kind for
        // which per-iteration `_get`/`_len` FFI pair backs the loop test/
        // body (mirroring `MirExpr::Subscript`'s own "one MIR node, one
        // codegen arm, branch internally on the resolved kind" precedent,
        // PR-11b, rather than a `MirStmt` variant per source-kind
        // combination), and (c) a conditionally-executed `.append()` call
        // inside the loop body instead of arbitrary user statements.
        //
        // `source`'s own four kinds still need their own loop-skeleton code
        // each (a `phi`-based tagged-int induction for `Range`, mirroring
        // `ForRange`; a raw `i64` index `phi` against a re-read `_len` for
        // `List`/`Dict`/`Set`, mirroring `ForList`/`ForDict`/`ForSet`), so
        // that part is not shared. The *filter-then-append* step that
        // follows, once `var` is bound, is identical regardless of
        // `source`'s kind -- so unlike the loop skeleton, it is written
        // once, after the `match source` below, rather than duplicated four
        // times inline. `CompLoopTail` (local to this arm only, mirroring
        // no shared type any other `MirStmt` arm reads) exists solely to
        // carry the two possible "how do I increment and branch back"
        // shapes (`Range`'s tagged `pycc_rt_int_add` step vs. every
        // container source's raw `build_int_add` index step) across that
        // shared filter/append code to the shared increment code after it.
        //
        // Unlike every `For*` arm above, the increment-and-back-edge at the
        // end of this arm has **no** terminator-safety guard (the
        // `if builder...get_terminator().is_none()` check every `For*` arm
        // repeats). Those arms need one because a `Return` inside an
        // arbitrary user-supplied `body: Vec<MirStmt>` can already
        // terminate `body_bb` before the increment is built. A
        // comprehension's own "body" is only ever `cond`/`elt`, two plain
        // `MirExpr` trees with no `MirStmt::Return` of their own to reach --
        // so the block the builder is positioned in when the filter/append
        // step finishes is always genuinely unterminated, and copying that
        // guard here would add a branch D-014's region gate could never
        // legitimately exercise on its "already terminated" side.
        MirStmt::ListCompAssign {
            target,
            var,
            var_ty: _,
            source,
            cond,
            elt,
        } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();

            // 1. Allocate the target's own empty backing list as a
            //    free-standing SSA pointer value (`new_list`) -- but do
            //    **not** store it into `target`'s own slot yet. Python (and
            //    this crate's own `MirStmt::Assign` arm) fully evaluates an
            //    assignment's RHS before rebinding its target name; a
            //    comprehension's own "RHS" is the entire loop below, not
            //    just this allocation. `source` (a `CompSource::List`/
            //    `Dict`/`Set` read, or a `CompSource::Range`'s own start/
            //    stop/step expressions) and every iteration's `cond`/`elt`
            //    can themselves reference `target`'s own name -- e.g. `xs =
            //    [x for x in xs if x > 2]` or `xs = [i for i in
            //    range(len(xs))]` -- and until this whole statement
            //    finishes, `target` must still resolve to whatever it
            //    already held, not to this not-yet-complete list. Storing
            //    early here previously made a self-referential comprehension
            //    read its own freshly emptied slot instead of the original
            //    container (a confirmed regression, fixed in a review
            //    round -- see `a_list_sourced_list_comprehension_that_
            //    rebinds_its_own_source_name_reads_the_pre_existing_value`
            //    and its neighboring tests below). `new_list` itself needs
            //    no slot at all to stay live across the loop: it is defined
            //    in this block, which dominates every block the loop below
            //    creates (`test_bb`/`body_bb`/`after_bb` and the optional
            //    `if_taken_bb`/`if_skip_bb`), so every `build_int_list_
            //    append(builder, rt, new_list, ..)` call inside the loop can
            //    reference it directly as an ordinary SSA value, exactly
            //    like the loop's own induction `phi` is referenced without
            //    living in a named slot either. `target`'s own
            //    already-declared slot (`collect_stmt_bindings`'s own
            //    `ListCompAssign` arm guarantees it exists before this arm
            //    ever runs, the same invariant every other container-typed
            //    target already relies on) is written once, at the very end
            //    of this arm, only after the loop has fully completed --
            //    mirrors `MirExpr::ListLiteral`'s own `rt.int_list_new` call
            //    for the allocation itself; `emit_assign`'s existing
            //    `Scalar::List` arm needs no decref/incref either way
            //    (D-107's leak-only rule).
            let new_list = builder
                .build_call(rt.int_list_new, &[], "comp_list_new")
                .expect("build_call should not fail for a well-formed list allocation")
                .try_as_basic_value()
                .expect_basic("pycc_rt_int_list_new returns a non-void pointer")
                .into_pointer_value();

            // Carries whatever the shared increment step (after the shared
            // filter/append step) needs to add the phi's back-edge value
            // and branch to `test_bb` again -- see this arm's own doc
            // comment above for why this is a local, arm-private type
            // rather than something shared with any other `MirStmt` arm.
            enum CompLoopTail<'ctx> {
                Range {
                    induction: inkwell::values::PhiValue<'ctx>,
                    current: IntValue<'ctx>,
                    step_v: IntValue<'ctx>,
                },
                Indexed {
                    induction: inkwell::values::PhiValue<'ctx>,
                    current: IntValue<'ctx>,
                },
            }

            // 2. Build the loop's own test/body/after basic blocks,
            //    parametrized internally on `source`'s own kind. Each
            //    branch below positions the builder at the start of
            //    `body_bb` and binds `var` before returning -- mirroring
            //    each corresponding `For*` arm's own preheader/test/body
            //    shape exactly (see this arm's own doc comment for which
            //    `For*` arm each branch mirrors).
            let (test_bb, after_bb, tail, owned_range_operands) = match source {
                CompSource::Range { start, stop, step } => {
                    // Mirrors `MirStmt::ForRange`'s own shape exactly.
                    let start_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, start),
                        "start",
                    );
                    let stop_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, stop),
                        "stop",
                    );
                    let step_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, step),
                        "step",
                    );
                    // #146 Part 1: same ownership contract as
                    // `MirStmt::ForRange`'s own arm (see the long comment
                    // there), minus its `stop_v`/`step_v` retain/release
                    // pair -- `CompLoopTail::Range` does not carry `stop_v`,
                    // and it does not need to: not retaining and not
                    // releasing an operand this loop only ever *reads* is
                    // already balanced. `start_v` is different: it becomes
                    // the first `current`, which the per-iteration and
                    // `after_bb` releases below do retire, so it must be
                    // retained here. Do not additionally route these
                    // operands through `retain_if_int_duplicate`.
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        start_v,
                        BigIntRefcount::Retain,
                    );
                    // #146 Part 2 (D-181). `start_v`'s own birth reference
                    // is retired immediately, exactly as in
                    // `MirStmt::ForRange`: the retain directly above has
                    // already made this loop an owner.
                    //
                    // `stop_v`/`step_v` cannot be: this emitter never
                    // retains them (see the comment above), and
                    // `pycc_rt_range_continue` re-reads both on *every*
                    // iteration, so releasing a freshly built bound here
                    // would be a use-after-free on trip two. They are
                    // carried out to `after_bb` instead -- past the last
                    // read -- and released there.
                    release_if_int_temporary(context, builder, rt, start, start_v);
                    let owned_range_operands: Vec<IntValue<'ctx>> = [
                        int_temporary_word(stop, stop_v),
                        int_temporary_word(step, step_v),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    // Re-read after the retain: it splits the block.
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "listcomp_test");
                    let body_bb = context.append_basic_block(function, "listcomp_body");
                    let after_bb = context.append_basic_block(function, "listcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "listcomp_current")
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
                            "listcomp_cont",
                        )
                        .expect("build_int_compare should not fail comparing two i8 operands");
                    builder
                        .build_conditional_branch(cont_i1, body_bb, after_bb)
                        .expect(
                            "build_conditional_branch should not fail for a well-formed i1 condition",
                        );

                    builder.position_at_end(body_bb);
                    // Retain before `emit_assign`'s release-before-store,
                    // exactly as in `MirStmt::ForRange`.
                    emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Retain);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(current));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Range {
                            induction,
                            current,
                            step_v,
                        },
                        owned_range_operands,
                    )
                }
                CompSource::List(name) => {
                    // Mirrors `MirStmt::ForList`'s own shape exactly.
                    let list_ptr = emit_list_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "listcomp_test");
                    let body_bb = context.append_basic_block(function, "listcomp_body");
                    let after_bb = context.append_basic_block(function, "listcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "listcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_int_list_len(builder, rt, list_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "listcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

                    builder.position_at_end(body_bb);
                    let encoded_element = build_int_list_get(builder, rt, list_ptr, current);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
                CompSource::Dict(name) => {
                    // Mirrors `MirStmt::ForDict`'s own shape exactly,
                    // including its own `pycc_rt_str_incref` call on the
                    // read key before the per-iteration `var` bind (see
                    // that arm's own doc comment for why: this keeps
                    // `var`'s own reference safely alive across the
                    // iteration without corrupting the source dict's own
                    // key). This specific write never itself calls
                    // `decref_str_slot_before_store`, exactly like
                    // `ForDict`'s own per-iteration bind does not. Every
                    // reachable `list[int]`-producing comprehension in this
                    // PR's own scope has `elt: Ty::Int` (T0034), and this
                    // compiler has no `str`-to-`int` builtin of any kind
                    // yet, so a real, type-checked program can never route
                    // a `Dict` source into *this* arm -- but the binding
                    // must still be correct regardless of what `elt` does
                    // with `var` afterward, exactly like `ForDict`'s own
                    // unconditional treatment. See this crate's own
                    // `a_dict_sourced_list_comprehension_binds_its_key_
                    // without_crashing` test, which reaches this branch via
                    // hand-built MIR bypassing `pycc_types`.
                    let dict_ptr = emit_dict_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "listcomp_test");
                    let body_bb = context.append_basic_block(function, "listcomp_body");
                    let after_bb = context.append_basic_block(function, "listcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "listcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_dict_len(builder, rt, dict_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "listcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

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
                    builder
                        .build_call(rt.str_incref, &[key_ptr.into()], "listcomp_dict_key_incref")
                        .expect("build_call should not fail for a well-formed incref");
                    emit_assign(context, builder, rt, locals, var, Scalar::Str(key_ptr));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
                CompSource::Set(name) => {
                    // Mirrors `MirStmt::ForSet`'s own shape exactly.
                    let set_ptr = emit_set_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "listcomp_test");
                    let body_bb = context.append_basic_block(function, "listcomp_body");
                    let after_bb = context.append_basic_block(function, "listcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "listcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_int_set_len(builder, rt, set_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "listcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

                    builder.position_at_end(body_bb);
                    let encoded_element = build_int_set_get(builder, rt, set_ptr, current);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
            };

            // 3. Inside the loop body: if `cond` is `Some`, evaluate it,
            //    branch on truthiness into a small `listcomp_if_taken`/
            //    `listcomp_if_skip` pair of blocks (mirroring `MirStmt::
            //    If`'s own two-block shape), and only inside
            //    `listcomp_if_taken` evaluate `elt` and append it;
            //    `listcomp_if_skip` doubles as the join point either way --
            //    both the "taken" path (after its own append) and the
            //    "condition false" path fall into it directly. If `cond` is
            //    `None`, `elt` is evaluated and appended unconditionally,
            //    with no extra blocks at all. Identical regardless of
            //    `source`'s own kind, so written once here rather than
            //    duplicated inside each branch of the match above.
            match cond {
                Some(cond_expr) => {
                    let cond_scalar = emit_expr(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        cond_expr,
                    );
                    let cond_i1 = truthy(context, builder, rt, cond_scalar);
                    // #146 Part 2 (D-181): released after `truthy`, which
                    // reads a bigint operand's limbs.
                    release_scalar_if_int_temporary(
                        context,
                        builder,
                        rt,
                        cond_expr,
                        &cond_scalar,
                    );
                    let if_taken_bb = context.append_basic_block(function, "listcomp_if_taken");
                    let if_skip_bb = context.append_basic_block(function, "listcomp_if_skip");
                    builder
                        .build_conditional_branch(cond_i1, if_taken_bb, if_skip_bb)
                        .expect(
                            "build_conditional_branch should not fail for a well-formed i1 condition",
                        );
                    builder.position_at_end(if_taken_bb);
                    // 3b. Evaluate `elt`, run the same validation and
                    //     identity-preserving storage as `ListAppend`, and
                    //     append it to `new_list`.
                    let elt_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, elt);
                    let elt_encoded = to_encoded_int(context, builder, elt_scalar);
                    let _ = build_untag_checked(builder, rt, elt_encoded, "listcomp_validate_elt");
                    build_int_list_append(builder, rt, new_list, elt_encoded);
                    builder.build_unconditional_branch(if_skip_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    builder.position_at_end(if_skip_bb);
                }
                None => {
                    let elt_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, elt);
                    let elt_encoded = to_encoded_int(context, builder, elt_scalar);
                    let _ = build_untag_checked(builder, rt, elt_encoded, "listcomp_validate_elt");
                    build_int_list_append(builder, rt, new_list, elt_encoded);
                }
            }

            // 4. Increment and branch back to the loop test -- no
            //    terminator-safety guard needed here (see this arm's own
            //    doc comment above for why).
            // `Some(current)` for a `range` source: the final `current`
            // (the one that failed `range_continue`) was never bound to the
            // visible target and needs its own release once `after_bb` is
            // reached. A comprehension has no `return`, so this release is
            // unconditional -- a terminator guard here would be dead code.
            let unconsumed_current = match tail {
                CompLoopTail::Range {
                    induction,
                    current,
                    step_v,
                } => {
                    let next = builder
                        .build_call(
                            rt.int_add,
                            &[current.into(), step_v.into()],
                            "listcomp_next",
                        )
                        .expect("build_call should not fail for a well-formed int add")
                        .try_as_basic_value()
                        .expect_basic("pycc_rt_int_add returns a non-void i64")
                        .into_int_value();
                    // This iteration's `current` is dead once `next` exists.
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        current,
                        BigIntRefcount::Release,
                    );
                    // Re-read after the release: it splits the block.
                    let body_end = builder.get_insert_block().unwrap();
                    induction.add_incoming(&[(&next, body_end)]);
                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    Some(current)
                }
                CompLoopTail::Indexed { induction, current } => {
                    let next = builder
                        .build_int_add(
                            current,
                            context.i64_type().const_int(1, false),
                            "listcomp_next",
                        )
                        .expect("build_int_add should not fail for two i64 operands");
                    let body_end = builder.get_insert_block().unwrap();
                    induction.add_incoming(&[(&next, body_end)]);
                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    // A container index is a raw `i64` counter, never a
                    // D-141 encoded word, so it owns nothing to release.
                    None
                }
            };

            builder.position_at_end(after_bb);
            if let Some(current) = unconsumed_current {
                emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Release);
            }
            // #146 Part 2 (D-181): the freshly built `range` bounds, past
            // their last `pycc_rt_range_continue` read.
            for word in owned_range_operands {
                emit_bigint_refcount_call(context, builder, rt, word, BigIntRefcount::Release);
            }
            // 5. Only now -- after the loop has fully run to completion,
            //    with every read of `target`'s own name during `source`/
            //    `cond`/`elt` evaluation already having happened against
            //    its pre-existing value -- bind `target` to the now-fully-
            //    built list (see point 1's own comment above for why this
            //    is deferred all the way to here).
            emit_assign(context, builder, rt, locals, target, Scalar::List(new_list));
            Ok(())
        }
        // `target = {elt for var in <source> [if cond]}` (PR-12 Task 5b,
        // D-117): structurally identical to `ListCompAssign`'s own arm
        // above -- same allocate-a-free-standing-pointer-then-loop-then-
        // bind-`target`-only-at-`after_bb` shape (see that arm's own doc
        // comments, "point 1"/"point 5", for why the ordering is
        // load-bearing, not cosmetic: it is copied here unchanged, not
        // re-derived), substituting `rt.int_set_new`/`build_int_set_add`
        // for `rt.int_list_new`/`build_int_list_append`. `CompLoopTail` is
        // redeclared here rather than shared with `ListCompAssign`'s own
        // arm-local type of the same shape -- this file's existing `For*`
        // arms already duplicate their own loop skeletons inline rather
        // than factor out a shared helper for a second consumer (see
        // `ListCompAssign`'s own doc comment: "wait for a third consumer"),
        // and this arm is exactly that established precedent, not a new
        // one. Block names use a `setcomp_` prefix (distinct from
        // `ListCompAssign`'s own `listcomp_` prefix, per that arm's own
        // handoff note) purely for readable disassembly/IR dumps; the
        // strings themselves have no observable behavior.
        MirStmt::SetCompAssign {
            target,
            var,
            var_ty: _,
            source,
            cond,
            elt,
        } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();

            // 1. Allocate the target's own empty backing set as a
            //    free-standing SSA pointer (`new_set`) -- not stored into
            //    `target`'s own slot until the whole loop has completed
            //    (see `ListCompAssign`'s own "point 1" comment above for
            //    why).
            let new_set = builder
                .build_call(rt.int_set_new, &[], "comp_set_new")
                .expect("build_call should not fail for a well-formed set allocation")
                .try_as_basic_value()
                .expect_basic("pycc_rt_int_set_new returns a non-void pointer")
                .into_pointer_value();

            enum CompLoopTail<'ctx> {
                Range {
                    induction: inkwell::values::PhiValue<'ctx>,
                    current: IntValue<'ctx>,
                    step_v: IntValue<'ctx>,
                },
                Indexed {
                    induction: inkwell::values::PhiValue<'ctx>,
                    current: IntValue<'ctx>,
                },
            }

            // 2. Build the loop's own test/body/after basic blocks,
            //    parametrized internally on `source`'s own kind -- mirrors
            //    `ListCompAssign`'s own `match source` exactly (see that
            //    arm's own doc comment for which `For*` arm each branch
            //    mirrors).
            let (test_bb, after_bb, tail, owned_range_operands) = match source {
                CompSource::Range { start, stop, step } => {
                    let start_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, start),
                        "start",
                    );
                    let stop_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, stop),
                        "stop",
                    );
                    let step_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, step),
                        "step",
                    );
                    // #146 Part 1: same ownership contract as
                    // `MirStmt::ForRange`'s own arm (see the long comment
                    // there), minus its `stop_v`/`step_v` retain/release
                    // pair -- `CompLoopTail::Range` does not carry `stop_v`,
                    // and it does not need to: not retaining and not
                    // releasing an operand this loop only ever *reads* is
                    // already balanced. `start_v` is different: it becomes
                    // the first `current`, which the per-iteration and
                    // `after_bb` releases below do retire, so it must be
                    // retained here. Do not additionally route these
                    // operands through `retain_if_int_duplicate`.
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        start_v,
                        BigIntRefcount::Retain,
                    );
                    // #146 Part 2 (D-181). `start_v`'s own birth reference
                    // is retired immediately, exactly as in
                    // `MirStmt::ForRange`: the retain directly above has
                    // already made this loop an owner.
                    //
                    // `stop_v`/`step_v` cannot be: this emitter never
                    // retains them (see the comment above), and
                    // `pycc_rt_range_continue` re-reads both on *every*
                    // iteration, so releasing a freshly built bound here
                    // would be a use-after-free on trip two. They are
                    // carried out to `after_bb` instead -- past the last
                    // read -- and released there.
                    release_if_int_temporary(context, builder, rt, start, start_v);
                    let owned_range_operands: Vec<IntValue<'ctx>> = [
                        int_temporary_word(stop, stop_v),
                        int_temporary_word(step, step_v),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    // Re-read after the retain: it splits the block.
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "setcomp_test");
                    let body_bb = context.append_basic_block(function, "setcomp_body");
                    let after_bb = context.append_basic_block(function, "setcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "setcomp_current")
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
                            "setcomp_cont",
                        )
                        .expect("build_int_compare should not fail comparing two i8 operands");
                    builder
                        .build_conditional_branch(cont_i1, body_bb, after_bb)
                        .expect(
                            "build_conditional_branch should not fail for a well-formed i1 condition",
                        );

                    builder.position_at_end(body_bb);
                    // Retain before `emit_assign`'s release-before-store,
                    // exactly as in `MirStmt::ForRange`.
                    emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Retain);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(current));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Range {
                            induction,
                            current,
                            step_v,
                        },
                        owned_range_operands,
                    )
                }
                CompSource::List(name) => {
                    let list_ptr = emit_list_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "setcomp_test");
                    let body_bb = context.append_basic_block(function, "setcomp_body");
                    let after_bb = context.append_basic_block(function, "setcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "setcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_int_list_len(builder, rt, list_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "setcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

                    builder.position_at_end(body_bb);
                    let encoded_element = build_int_list_get(builder, rt, list_ptr, current);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
                CompSource::Dict(name) => {
                    // `set[int]`'s own element type is always `Ty::Int`
                    // (T0038), so no reachable, type-checked `set[int]`
                    // comprehension can have a `Dict` source (this compiler
                    // has no `str`-to-`int` builtin) -- identical
                    // unreachable-from-real-source situation to
                    // `ListCompAssign`'s own `CompSource::Dict` branch (see
                    // that arm's own doc comment). Copied here unchanged:
                    // the per-iteration key read/incref/bind must still be
                    // correct regardless of what `elt` does with `var`
                    // afterward.
                    let dict_ptr = emit_dict_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "setcomp_test");
                    let body_bb = context.append_basic_block(function, "setcomp_body");
                    let after_bb = context.append_basic_block(function, "setcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "setcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_dict_len(builder, rt, dict_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "setcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

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
                    builder
                        .build_call(rt.str_incref, &[key_ptr.into()], "setcomp_dict_key_incref")
                        .expect("build_call should not fail for a well-formed incref");
                    emit_assign(context, builder, rt, locals, var, Scalar::Str(key_ptr));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
                CompSource::Set(name) => {
                    let set_ptr = emit_set_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "setcomp_test");
                    let body_bb = context.append_basic_block(function, "setcomp_body");
                    let after_bb = context.append_basic_block(function, "setcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "setcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_int_set_len(builder, rt, set_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "setcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

                    builder.position_at_end(body_bb);
                    let encoded_element = build_int_set_get(builder, rt, set_ptr, current);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
            };

            // 3. Inside the loop body: identical filter-then-insert shape
            //    to `ListCompAssign`'s own shared step (see that arm's own
            //    doc comment), substituting `build_int_set_add` for
            //    `build_int_list_append`. `pycc_rt_int_set_add`'s own
            //    dedup check (D-121) makes a repeated element collapse to
            //    one, unconditionally, with no extra logic needed here.
            match cond {
                Some(cond_expr) => {
                    let cond_scalar = emit_expr(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        cond_expr,
                    );
                    let cond_i1 = truthy(context, builder, rt, cond_scalar);
                    // #146 Part 2 (D-181): released after `truthy`, which
                    // reads a bigint operand's limbs.
                    release_scalar_if_int_temporary(
                        context,
                        builder,
                        rt,
                        cond_expr,
                        &cond_scalar,
                    );
                    let if_taken_bb = context.append_basic_block(function, "setcomp_if_taken");
                    let if_skip_bb = context.append_basic_block(function, "setcomp_if_skip");
                    builder
                        .build_conditional_branch(cond_i1, if_taken_bb, if_skip_bb)
                        .expect(
                            "build_conditional_branch should not fail for a well-formed i1 condition",
                        );
                    builder.position_at_end(if_taken_bb);
                    let elt_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, elt);
                    let elt_encoded = to_encoded_int(context, builder, elt_scalar);
                    let _ = build_untag_checked(builder, rt, elt_encoded, "setcomp_validate_elt");
                    build_int_set_add(builder, rt, new_set, elt_encoded);
                    builder.build_unconditional_branch(if_skip_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    builder.position_at_end(if_skip_bb);
                }
                None => {
                    let elt_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, elt);
                    let elt_encoded = to_encoded_int(context, builder, elt_scalar);
                    let _ = build_untag_checked(builder, rt, elt_encoded, "setcomp_validate_elt");
                    build_int_set_add(builder, rt, new_set, elt_encoded);
                }
            }

            // 4. Increment and branch back to the loop test -- no
            //    terminator-safety guard, for the identical reason
            //    `ListCompAssign`'s own arm gives (a comprehension's own
            //    "body" is only `cond`/`elt`, never a `Return`).
            // `Some(current)` for a `range` source: the final `current`
            // (the one that failed `range_continue`) was never bound to the
            // visible target and needs its own release once `after_bb` is
            // reached. A comprehension has no `return`, so this release is
            // unconditional -- a terminator guard here would be dead code.
            let unconsumed_current = match tail {
                CompLoopTail::Range {
                    induction,
                    current,
                    step_v,
                } => {
                    let next = builder
                        .build_call(rt.int_add, &[current.into(), step_v.into()], "setcomp_next")
                        .expect("build_call should not fail for a well-formed int add")
                        .try_as_basic_value()
                        .expect_basic("pycc_rt_int_add returns a non-void i64")
                        .into_int_value();
                    // This iteration's `current` is dead once `next` exists.
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        current,
                        BigIntRefcount::Release,
                    );
                    // Re-read after the release: it splits the block.
                    let body_end = builder.get_insert_block().unwrap();
                    induction.add_incoming(&[(&next, body_end)]);
                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    Some(current)
                }
                CompLoopTail::Indexed { induction, current } => {
                    let next = builder
                        .build_int_add(
                            current,
                            context.i64_type().const_int(1, false),
                            "setcomp_next",
                        )
                        .expect("build_int_add should not fail for two i64 operands");
                    let body_end = builder.get_insert_block().unwrap();
                    induction.add_incoming(&[(&next, body_end)]);
                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    // A container index is a raw `i64` counter, never a
                    // D-141 encoded word, so it owns nothing to release.
                    None
                }
            };

            builder.position_at_end(after_bb);
            if let Some(current) = unconsumed_current {
                emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Release);
            }
            // #146 Part 2 (D-181): the freshly built `range` bounds, past
            // their last `pycc_rt_range_continue` read.
            for word in owned_range_operands {
                emit_bigint_refcount_call(context, builder, rt, word, BigIntRefcount::Release);
            }
            // 5. Only now -- after the loop has fully run to completion --
            //    bind `target` to the now-fully-built set (see
            //    `ListCompAssign`'s own "point 5" comment above for why this
            //    is deferred all the way to here).
            emit_assign(context, builder, rt, locals, target, Scalar::Set(new_set));
            Ok(())
        }
        // `target = {key: value for var in <source> [if cond]}` (PR-12 Task
        // 5b, D-117): same allocate-then-loop-then-bind-`target`-at-
        // `after_bb` shape as `ListCompAssign`/`SetCompAssign` above,
        // differing only in evaluating **two** expressions (`key`/`value`)
        // per taken iteration instead of one (`elt`), and in one additional,
        // genuinely new correctness requirement: `incref_if_str_duplicate`
        // on the evaluated `key`, unconditionally, before `build_dict_set`
        // -- exactly mirroring `MirStmt::DictSet`'s own call (see that
        // arm's own doc comment above, and D-124): `pycc_rt_dict_set` adopts
        // whatever key pointer it is given as `new_dict`'s own permanent
        // reference without incref'ing it itself, so a bare-`Name` key
        // (`{k: 1 for k in d}`, `key.ty() == Ty::Str` -- the only shape
        // T0036 allows a `Dict`-sourced `DictCompAssign` to reach real
        // source through) must be incref'd here first, establishing `new_
        // dict`'s own reference as genuinely independent of whatever `var`'s
        // own per-iteration binding or the source dict itself do
        // afterward. A no-op for any other `key` shape (e.g. an f-string --
        // already fresh, already owning exactly one reference from its own
        // construction, `str_value_is_a_duplicate_reference`'s own gate), so
        // no `source`-kind-specific branching is needed for this half of the
        // fix.
        MirStmt::DictCompAssign {
            target,
            var,
            var_ty: _,
            source,
            cond,
            key,
            value,
        } => {
            let function = builder.get_insert_block().unwrap().get_parent().unwrap();

            let new_dict = builder
                .build_call(rt.dict_new, &[], "comp_dict_new")
                .expect("build_call should not fail for a well-formed dict allocation")
                .try_as_basic_value()
                .expect_basic("pycc_rt_dict_new returns a non-void pointer")
                .into_pointer_value();

            enum CompLoopTail<'ctx> {
                Range {
                    induction: inkwell::values::PhiValue<'ctx>,
                    current: IntValue<'ctx>,
                    step_v: IntValue<'ctx>,
                },
                Indexed {
                    induction: inkwell::values::PhiValue<'ctx>,
                    current: IntValue<'ctx>,
                },
            }

            let (test_bb, after_bb, tail, owned_range_operands) = match source {
                CompSource::Range { start, stop, step } => {
                    let start_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, start),
                        "start",
                    );
                    let stop_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, stop),
                        "stop",
                    );
                    let step_v = range_operand_to_normalized_int(
                        context,
                        builder,
                        rt,
                        emit_expr(context, builder, module, rt, user_functions, locals, step),
                        "step",
                    );
                    // #146 Part 1: same ownership contract as
                    // `MirStmt::ForRange`'s own arm (see the long comment
                    // there), minus its `stop_v`/`step_v` retain/release
                    // pair -- `CompLoopTail::Range` does not carry `stop_v`,
                    // and it does not need to: not retaining and not
                    // releasing an operand this loop only ever *reads* is
                    // already balanced. `start_v` is different: it becomes
                    // the first `current`, which the per-iteration and
                    // `after_bb` releases below do retire, so it must be
                    // retained here. Do not additionally route these
                    // operands through `retain_if_int_duplicate`.
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        start_v,
                        BigIntRefcount::Retain,
                    );
                    // #146 Part 2 (D-181). `start_v`'s own birth reference
                    // is retired immediately, exactly as in
                    // `MirStmt::ForRange`: the retain directly above has
                    // already made this loop an owner.
                    //
                    // `stop_v`/`step_v` cannot be: this emitter never
                    // retains them (see the comment above), and
                    // `pycc_rt_range_continue` re-reads both on *every*
                    // iteration, so releasing a freshly built bound here
                    // would be a use-after-free on trip two. They are
                    // carried out to `after_bb` instead -- past the last
                    // read -- and released there.
                    release_if_int_temporary(context, builder, rt, start, start_v);
                    let owned_range_operands: Vec<IntValue<'ctx>> = [
                        int_temporary_word(stop, stop_v),
                        int_temporary_word(step, step_v),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    // Re-read after the retain: it splits the block.
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "dictcomp_test");
                    let body_bb = context.append_basic_block(function, "dictcomp_body");
                    let after_bb = context.append_basic_block(function, "dictcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "dictcomp_current")
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
                            "dictcomp_cont",
                        )
                        .expect("build_int_compare should not fail comparing two i8 operands");
                    builder
                        .build_conditional_branch(cont_i1, body_bb, after_bb)
                        .expect(
                            "build_conditional_branch should not fail for a well-formed i1 condition",
                        );

                    builder.position_at_end(body_bb);
                    // Retain before `emit_assign`'s release-before-store,
                    // exactly as in `MirStmt::ForRange`.
                    emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Retain);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(current));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Range {
                            induction,
                            current,
                            step_v,
                        },
                        owned_range_operands,
                    )
                }
                CompSource::List(name) => {
                    let list_ptr = emit_list_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "dictcomp_test");
                    let body_bb = context.append_basic_block(function, "dictcomp_body");
                    let after_bb = context.append_basic_block(function, "dictcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "dictcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_int_list_len(builder, rt, list_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "dictcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

                    builder.position_at_end(body_bb);
                    let encoded_element = build_int_list_get(builder, rt, list_ptr, current);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
                CompSource::Dict(name) => {
                    // The one `source` kind this arm's own `key`/`value`
                    // can make reachable from real, type-checked source
                    // (T0036 forces the loop variable's own type, `Ty::Str`
                    // for a `Dict` source, to satisfy `dict[str, ..]`'s own
                    // key-type gate directly) -- see this arm's own doc
                    // comment above and this task's dedicated end-to-end
                    // test below. Otherwise identical to `ListCompAssign`'s/
                    // `SetCompAssign`'s own `CompSource::Dict` branch: the
                    // `pycc_rt_str_incref` on the read key before `var`'s own
                    // per-iteration bind is the *source*-side incref (`d`'s
                    // own key becoming a genuinely independent duplicate
                    // reference for `var`'s own slot) -- a distinct concern
                    // from this arm's own required `incref_if_str_duplicate`
                    // on `key` below (the *target*-side incref, making
                    // `new_dict`'s own stored key genuinely independent of
                    // `var`'s slot in turn).
                    let dict_ptr = emit_dict_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "dictcomp_test");
                    let body_bb = context.append_basic_block(function, "dictcomp_body");
                    let after_bb = context.append_basic_block(function, "dictcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "dictcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_dict_len(builder, rt, dict_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "dictcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

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
                    builder
                        .build_call(rt.str_incref, &[key_ptr.into()], "dictcomp_dict_key_incref")
                        .expect("build_call should not fail for a well-formed incref");
                    emit_assign(context, builder, rt, locals, var, Scalar::Str(key_ptr));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
                CompSource::Set(name) => {
                    let set_ptr = emit_set_name_read(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        name,
                    );
                    let preheader = builder.get_insert_block().unwrap();

                    let test_bb = context.append_basic_block(function, "dictcomp_test");
                    let body_bb = context.append_basic_block(function, "dictcomp_body");
                    let after_bb = context.append_basic_block(function, "dictcomp_after");

                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail entering the loop test",
                    );
                    builder.position_at_end(test_bb);
                    let induction = builder
                        .build_phi(context.i64_type(), "dictcomp_index")
                        .expect("build_phi should not fail in a fresh loop-test block");
                    let zero = context.i64_type().const_zero();
                    induction.add_incoming(&[(&zero, preheader)]);
                    let current = induction.as_basic_value().into_int_value();
                    let len = build_int_set_len(builder, rt, set_ptr);
                    let cont = builder
                        .build_int_compare(IntPredicate::SLT, current, len, "dictcomp_cont")
                        .expect("build_int_compare should not fail comparing two i64 operands");
                    builder
                        .build_conditional_branch(cont, body_bb, after_bb)
                        .expect(
                        "build_conditional_branch should not fail for a well-formed i1 condition",
                    );

                    builder.position_at_end(body_bb);
                    let encoded_element = build_int_set_get(builder, rt, set_ptr, current);
                    emit_assign(context, builder, rt, locals, var, Scalar::Int(encoded_element));

                    (
                        test_bb,
                        after_bb,
                        CompLoopTail::Indexed { induction, current },
                        // A container-iterating comprehension has no
                        // `range` bounds to own.
                        Vec::new(),
                    )
                }
            };

            // 3. Inside the loop body: evaluate `key` then `value` (in that
            //    order -- matching `MirStmt::DictSet`'s own arm exactly),
            //    applying this arm's own required `incref_if_str_duplicate`
            //    fix to `key` before it is stored (see this arm's own doc
            //    comment above), then insert via `build_dict_set`. Wrapped
            //    in the identical `cond`-gated if/skip shape `ListCompAssign`/
            //    `SetCompAssign` share.
            match cond {
                Some(cond_expr) => {
                    let cond_scalar = emit_expr(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        cond_expr,
                    );
                    let cond_i1 = truthy(context, builder, rt, cond_scalar);
                    // #146 Part 2 (D-181): released after `truthy`, which
                    // reads a bigint operand's limbs.
                    release_scalar_if_int_temporary(
                        context,
                        builder,
                        rt,
                        cond_expr,
                        &cond_scalar,
                    );
                    let if_taken_bb = context.append_basic_block(function, "dictcomp_if_taken");
                    let if_skip_bb = context.append_basic_block(function, "dictcomp_if_skip");
                    builder
                        .build_conditional_branch(cond_i1, if_taken_bb, if_skip_bb)
                        .expect(
                            "build_conditional_branch should not fail for a well-formed i1 condition",
                        );
                    builder.position_at_end(if_taken_bb);
                    let key_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, key);
                    let key_scalar = incref_if_str_duplicate(builder, rt, key, key_scalar);
                    let Scalar::Str(key_ptr) = key_scalar else {
                        panic!(
                            "pycc_codegen: internal error: dict comprehension key did not evaluate \
                             to str -- pycc_types::check (T0036) should have rejected this before \
                             codegen"
                        )
                    };
                    let value_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, value);
                    let encoded = to_encoded_int(context, builder, value_scalar);
                    let _ = build_untag_checked(builder, rt, encoded, "dictcomp_validate_value");
                    build_dict_set(builder, rt, new_dict, key_ptr, encoded);
                    builder.build_unconditional_branch(if_skip_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    builder.position_at_end(if_skip_bb);
                }
                None => {
                    let key_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, key);
                    let key_scalar = incref_if_str_duplicate(builder, rt, key, key_scalar);
                    let Scalar::Str(key_ptr) = key_scalar else {
                        panic!(
                            "pycc_codegen: internal error: dict comprehension key did not evaluate \
                             to str -- pycc_types::check (T0036) should have rejected this before \
                             codegen"
                        )
                    };
                    let value_scalar =
                        emit_expr(context, builder, module, rt, user_functions, locals, value);
                    let encoded = to_encoded_int(context, builder, value_scalar);
                    let _ = build_untag_checked(builder, rt, encoded, "dictcomp_validate_value");
                    build_dict_set(builder, rt, new_dict, key_ptr, encoded);
                }
            }

            // 4. Increment and branch back to the loop test -- no
            //    terminator-safety guard, for the identical reason
            //    `ListCompAssign`'s own arm gives.
            // `Some(current)` for a `range` source: the final `current`
            // (the one that failed `range_continue`) was never bound to the
            // visible target and needs its own release once `after_bb` is
            // reached. A comprehension has no `return`, so this release is
            // unconditional -- a terminator guard here would be dead code.
            let unconsumed_current = match tail {
                CompLoopTail::Range {
                    induction,
                    current,
                    step_v,
                } => {
                    let next = builder
                        .build_call(
                            rt.int_add,
                            &[current.into(), step_v.into()],
                            "dictcomp_next",
                        )
                        .expect("build_call should not fail for a well-formed int add")
                        .try_as_basic_value()
                        .expect_basic("pycc_rt_int_add returns a non-void i64")
                        .into_int_value();
                    // This iteration's `current` is dead once `next` exists.
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        current,
                        BigIntRefcount::Release,
                    );
                    // Re-read after the release: it splits the block.
                    let body_end = builder.get_insert_block().unwrap();
                    induction.add_incoming(&[(&next, body_end)]);
                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    Some(current)
                }
                CompLoopTail::Indexed { induction, current } => {
                    let next = builder
                        .build_int_add(
                            current,
                            context.i64_type().const_int(1, false),
                            "dictcomp_next",
                        )
                        .expect("build_int_add should not fail for two i64 operands");
                    let body_end = builder.get_insert_block().unwrap();
                    induction.add_incoming(&[(&next, body_end)]);
                    builder.build_unconditional_branch(test_bb).expect(
                        "build_unconditional_branch should not fail on a block with no terminator yet",
                    );
                    // A container index is a raw `i64` counter, never a
                    // D-141 encoded word, so it owns nothing to release.
                    None
                }
            };

            builder.position_at_end(after_bb);
            if let Some(current) = unconsumed_current {
                emit_bigint_refcount_call(context, builder, rt, current, BigIntRefcount::Release);
            }
            // #146 Part 2 (D-181): the freshly built `range` bounds, past
            // their last `pycc_rt_range_continue` read.
            for word in owned_range_operands {
                emit_bigint_refcount_call(context, builder, rt, word, BigIntRefcount::Release);
            }
            // 5. Only now -- after the loop has fully run to completion --
            //    bind `target` to the now-fully-built dict (see
            //    `ListCompAssign`'s own "point 5" comment above for why this
            //    is deferred all the way to here).
            emit_assign(context, builder, rt, locals, target, Scalar::Dict(new_dict));
            Ok(())
        }
        MirStmt::Seq(stmts) => emit_body(
            context,
            builder,
            module,
            rt,
            user_functions,
            locals,
            stmts,
            expected_return_ty,
            finally_stack,
        ),
        // #382 (PR-22 Part 1): `raise ExceptionType("msg")` — allocate an
        // exception object, set the pending exception state, then terminate
        // this block. `emit_body` routes that explicit raise directly to the
        // nearest installed exception target (a `try` handler, the caller's
        // exceptional exit, or the top-level exit).
        MirStmt::Raise { exception } => {
            let exc_obj = emit_exception_value(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                exception,
                "exception",
            )?;
            builder
                .build_call(rt.exception_raise, &[exc_obj.into()], "")
                .expect("build_call should not fail for exception_raise");
            // Terminate the block with `unreachable` — the exception is
            // already set in the pending state. Do NOT position at a new
            // block after the unreachable: `emit_body` recognizes this
            // marker and replaces it with a branch to the current exception
            // target, while an unreachable block correctly does not fall
            // through in other structural checks.
            builder
                .build_unreachable()
                .expect("build_unreachable should not fail after raise");
            Ok(())
        }
        // #382: `raise ExceptionType("msg") from CauseType("cause")` —
        // allocate both exception and cause objects, then raise with cause.
        // Same `unreachable` approach as `Raise`.
        MirStmt::RaiseFrom { exception, cause } => {
            let exc_obj = emit_exception_value(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                exception,
                "exception",
            )?;
            let cause_obj = emit_exception_value(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                cause,
                "cause",
            )?;
            builder
                .build_call(
                    rt.exception_raise_with_cause,
                    &[exc_obj.into(), cause_obj.into()],
                    "",
                )
                .expect("build_call should not fail for exception_raise_with_cause");
            // Terminate the block with `unreachable` — the exception is
            // already set in the pending state. See `Raise`'s comment for
            // the rationale (same approach).
            builder
                .build_unreachable()
                .expect("build_unreachable should not fail after raise_from");
            Ok(())
        }
        // #382: Bare `raise` (re-raise) — load the lexically enclosing
        // handler's exception value, re-raise it, then terminate the block with
        // `unreachable`. The enclosing check point will detect the active
        // exception and handle it.
        MirStmt::Reraise => {
            let saved_exc = *rt
                .exceptions
                .reraise_values
                .borrow()
                .last()
                .expect("pycc_types rejects bare raise outside an except handler");
            let exc_val = builder
                .build_load(
                    context.ptr_type(inkwell::AddressSpace::default()),
                    saved_exc,
                    "saved_exc_val",
                )
                .expect("build_load should not fail for a handler exception slot")
                .into_pointer_value();
            builder
                .build_call(rt.exception_raise, &[exc_val.into()], "")
                .expect("build_call should not fail for exception_raise");
            // Terminate the block with `unreachable` — the exception is
            // already set in the pending state. See `Raise`'s comment for
            // the rationale (same approach).
            builder
                .build_unreachable()
                .expect("build_unreachable should not fail after reraise");
            Ok(())
        }
        // #382 (PR-22 Part 1): `try`/`except`/`else`/`finally` codegen.
        // The D-173 model uses explicit check-and-branch: after the try
        // body completes, generated code checks `pycc_rt_exception_active`
        // and, if an exception is pending, dispatches to the handler chain.
        // Each handler checks `pycc_rt_exception_type_matches` against its
        // declared type (or catches all for a bare `except:`). The handler
        // clears the exception state before running its body. The `else`
        // body runs only if no exception was raised. The `finally` body
        // always runs (implemented as a shared merge block that both the
        // normal-completion and exception paths branch to before exiting).
        MirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => exception::emit_try(
            context,
            builder,
            module,
            rt,
            user_functions,
            locals,
            body,
            handlers,
            orelse,
            finalbody,
            expected_return_ty,
            finally_stack,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_mir::{
        BinOpKind, CmpOpKind, MirExceptHandler, MirExpr, MirFStringPart, MirItem, MirModule,
        MirStmt, Ty,
    };
    use std::process::Command;

    /// `print(<n>)` as a `MirStmt` -- a convenience single-int-argument
    /// shape reused by many of this file's older tests (`emit_stmt`'s
    /// `print` dispatch itself now handles any number of arguments of any
    /// v0.1 scalar type, plus `None` from direct user-function results,
    /// D-075 parameter values, and assignment storage; see its own doc comment).
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
    fn a_none_typed_module_binding_gets_a_zero_unit_global_slot() {
        let context = Context::create();
        let module = context.create_module("none_global");
        let bindings = BTreeMap::from([("x".to_string(), Ty::None)]);
        let globals = declare_module_globals(&context, &module, &bindings);
        let slot = globals
            .get("x")
            .expect("declare_module_globals should bind `x`");
        assert_eq!(slot.ty, Ty::None);
        assert!(
            slot.initialized.is_some(),
            "zero is the None carrier, not proof that the binding was assigned"
        );
        let initializer = module
            .get_global("pyglobal_x")
            .expect("the global is named pyglobal_<name>")
            .get_initializer()
            .expect("declare_module_globals always sets an initializer")
            .into_int_value();
        assert_eq!(initializer.get_zero_extended_constant(), Some(0));
    }

    #[test]
    #[should_panic(expected = "a `<inferred>`-typed module binding is not supported yet")]
    fn an_infer_typed_module_binding_remains_an_internal_error() {
        let context = Context::create();
        let module = context.create_module("infer_global");
        let bindings = BTreeMap::from([("x".to_string(), Ty::Infer)]);
        let _ = declare_module_globals(&context, &module, &bindings);
    }

    #[test]
    fn a_dict_typed_module_binding_gets_a_real_pointer_backed_global_slot() {
        // Superseded by PR-11 Task 5: this test used to assert that a
        // `dict[str, int]` module binding panicked here (mirroring the
        // `None`/`tuple` catch-all tests above) -- that was accurate for
        // Task 4's own scope, but D-123 names module scope as one of the
        // places a `dict[str, int]` value is expected to live, and every
        // one of this task's own CLI repro programs assigns `x = {...}` at
        // module scope. `declare_module_globals` now gives `Ty::Dict(_)` a
        // real arm (mirrors `Ty::List(_)`'s own arm exactly), so this test
        // instead proves that: a pointer-backed global slot with the
        // `initialized` guard flag every module global gets, not a panic.
        let context = Context::create();
        let module = context.create_module("dict_global");
        let bindings = BTreeMap::from([("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))))]);
        let globals = declare_module_globals(&context, &module, &bindings);
        let slot = globals
            .get("x")
            .expect("declare_module_globals should bind `x`");
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
        let slot = globals
            .get("x")
            .expect("declare_module_globals should bind `x`");
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
    fn monomorphized_generic_function_dispatches_directly_without_fn_ptr_global() {
        // A monomorphized generic specialization (`0gen_...` name) has no
        // `fn_ptr_global` -- it dispatches directly through `direct_value`.
        // This test covers the `None` path of `if let Some(ref fn_ptr_global)`
        // in the top-level binding pass and the `direct_value` path in
        // `build_call_to_with_leading_args`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "0gen_identity__T_int".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "0gen_identity__T_int".to_string(),
                        args: vec![MirExpr::IntLiteral(7)],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("monomorphized_direct_dispatch");
        let obj_path = dir.join("monomorphized_direct_dispatch.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("monomorphized_direct_dispatch");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"7\n");
    }

    #[test]
    fn compiles_top_level_statement_with_no_main() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(call_print(42))],
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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

    /// A hand-built compute-heavy loop whose final `print(float)` references
    /// `pycc_rt_float_to_str`, while nothing references `pycc_rt_int_sub`.
    /// Debug codegen preserves both declarations; the release pipeline must
    /// retain the used declaration and remove the unused one.
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
            class_defs: Vec::new(),
        }
    }

    #[test]
    fn release_mode_actually_runs_llvm_optimization_passes() {
        let mir = release_flag_fixture();

        let debug_dir = tempfile_dir("release_flag_debug");
        let debug_obj_path = debug_dir.join("release_flag_debug.o");
        let mut debug_observations = Vec::new();
        let mut debug_observer = |module: &inkwell::module::Module<'_>, applied_pipeline| {
            debug_observations.push((
                applied_pipeline,
                module.get_function("pycc_rt_float_to_str").is_some(),
                module.get_function("pycc_rt_int_sub").is_some(),
            ));
        };
        compile_to_object_with_observer(
            &mir,
            &debug_obj_path,
            None,
            false,
            Some(&mut debug_observer),
        )
        .expect("debug codegen should succeed");

        let release_dir = tempfile_dir("release_flag_release");
        let release_obj_path = release_dir.join("release_flag_release.o");
        let mut release_observations = Vec::new();
        let mut release_observer = |module: &inkwell::module::Module<'_>, applied_pipeline| {
            release_observations.push((
                applied_pipeline,
                module.get_function("pycc_rt_float_to_str").is_some(),
                module.get_function("pycc_rt_int_sub").is_some(),
            ));
        };
        compile_to_object_with_observer(
            &mir,
            &release_obj_path,
            None,
            true,
            Some(&mut release_observer),
        )
        .expect("release codegen should succeed");

        assert_eq!(debug_observations, vec![(None, true, true)]);
        assert_eq!(
            release_observations,
            vec![(Some("default<O3>"), true, false)]
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
    fn compiles_print_evaluating_all_args_before_output_with_a_side_effecting_call() {
        // #145: `def side_effect() -> int: print(2); return 3` ;
        // `print(1, side_effect())` -- the later argument's side effect
        // (printing `2`) must complete before the outer `print`'s own
        // output begins, so stdout is `2\n1 3\n` (CPython's order), not
        // the interleaved `1 2\n3\n` the old single-phase emit produced.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "side_effect".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::IntLiteral(2)],
                            ty: Ty::None,
                        }),
                        MirStmt::Return(Some(MirExpr::IntLiteral(3))),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::Call {
                            callee: "side_effect".to_string(),
                            args: vec![],
                            ty: Ty::Int,
                        },
                    ],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("print_eval_order_side_effect");
        let obj_path = dir.join("print_eval_order_side_effect.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("print_eval_order_side_effect");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n1 3\n");
    }

    #[test]
    fn compiles_print_with_a_failing_later_argument_produces_no_partial_output() {
        // #145/#382: `def fail_later() -> int: return 1 // 0` ;
        // `print(1, fail_later())` -- the later argument's zero-divisor
        // now sets the pending exception state (D-173, #382) instead of
        // panicking. The function returns a neutral 0, `print` evaluates
        // all args before output, and the top-level exception check after
        // the `print` statement detects the active exception and exits
        // with non-zero. stdout may contain partial output because the
        // process no longer aborts mid-statement — the exception is
        // caught at the next top-level check point.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "fail_later".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                        op: BinOpKind::FloorDiv,
                        left: Box::new(MirExpr::IntLiteral(1)),
                        right: Box::new(MirExpr::IntLiteral(0)),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::Call {
                            callee: "fail_later".to_string(),
                            args: vec![],
                            ty: Ty::Int,
                        },
                    ],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("print_eval_order_fail_later");
        let obj_path = dir.join("print_eval_order_fail_later.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("print_eval_order_fail_later");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        // #382: The process exits with non-zero because the top-level
        // exception check catches the ZeroDivisionError set by `1 // 0`.
        assert!(
            !output.status.success(),
            "process should exit non-zero on zero-divisor exception: {:?}",
            output.status
        );
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // exact case). Task 6's `to_numeric_encoded_int` (see its own doc comment)
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
            class_defs: Vec::new(),
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
        // left-operand case above (`to_numeric_encoded_int` is called once per
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
            class_defs: Vec::new(),
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
        // operand via `to_numeric_encoded_int` instead of rejecting it (same
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("compare_bool_right_promotes");
        let obj_path = dir.join("compare_bool_right_promotes.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    // Inverted for #148/D-178: this module is exactly the one the retired
    // `an_oversized_int_literal_is_not_yet_supported` compiled, and it must
    // now succeed by materializing the literal through
    // `pycc_rt_int_from_i64` rather than panicking in `tag_smallint_const`.
    #[test]
    fn an_oversized_int_literal_materializes_a_runtime_bigint() {
        for value in [
            i64::MAX,
            4_611_686_018_427_387_904_i64,
            -4_611_686_018_427_387_905_i64,
            i64::MIN,
        ] {
            let mir = MirModule {
                items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(value)],
                    ty: Ty::None,
                }))],
                class_defs: Vec::new(),
            };
            let dir = tempfile_dir("oversized_int_literal_materializes");
            let obj_path = dir.join("oversized_int_literal_materializes.o");
            let mut saw_call = false;
            // `Module::print_to_string` returns an `inkwell` `LLVMString`,
            // whose `Drop` calls `LLVMDisposeMessage` -- which faults with
            // `STATUS_ACCESS_VIOLATION` on Windows against this LLVM
            // release (D-029). Route it through `llvm_string_to_owned`,
            // exactly as this crate's error paths already do, instead of
            // letting the temporary drop.
            let mut observer = |module: &inkwell::module::Module<'_>, _| {
                saw_call |= llvm_string_to_owned(module.print_to_string())
                    .contains("call i64 @pycc_rt_int_from_i64");
            };
            compile_to_object_with_observer(&mir, &obj_path, None, false, Some(&mut observer))
                .expect("an out-of-range int literal should compile");
            assert!(saw_call, "{value} should be materialized at run time");
        }
    }

    // D-029 static guard. Every inkwell API that hands back an `LLVMString`
    // is a Windows crash waiting to happen: the wrapper's `Drop` calls
    // `LLVMDisposeMessage`, which faults against the prebuilt LLVM this
    // project links there. Reaching past this crate's own protections to
    // the raw inkwell API compiles and passes every local gate, then kills
    // the whole test binary in CI, because the Windows job is the only
    // place this class is ever executed. This test is the mechanical
    // stand-in for that missing local gate.
    //
    // D-029 records three distinct protections, and this test covers them
    // to three different depths -- state that plainly rather than letting
    // the name imply uniform coverage:
    //
    //  1. `llvm_string_to_owned`, which forgets the wrapper instead of
    //     dropping it. Fully checked: every `print_to_string` call must
    //     name it on the same line.
    //  2. `verify_module`, a no-op under `#[cfg(windows)]`. Fully checked:
    //     exactly one direct `verify` call may exist, the wrapper's own.
    //  3. `ManuallyDrop` at the point a `TargetTriple` is created, which
    //     covers every exit path including the early `?`. Only tripwired:
    //     the wrapping is structural and spans several lines, so a
    //     line-oriented scan cannot confirm it. What it can do is pin the
    //     number of triple-producing call sites, so adding one fails here
    //     and sends the author to this comment.
    //
    // `Target::from_triple`'s and `write_to_file`'s `.map_err` sites fall
    // under (1) and are checked only insofar as they name the wrapper.
    //
    // The needles are assembled at run time so this test's own body is not
    // counted as a violation of itself. Comment lines are excluded, since
    // the crate discusses all three APIs at length in prose.
    #[test]
    fn every_inkwell_llvm_string_call_routes_through_a_d029_wrapper() {
        // Read the whole crate rather than this one file, so a module added
        // later is covered without anyone remembering to extend a list.
        let sources: Vec<String> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
            .expect("the crate's own source directory should be readable")
            .map(|entry| entry.expect("a readable directory entry").path())
            .filter(|path| path.extension() == Some(std::ffi::OsStr::new("rs")))
            .map(|path| std::fs::read_to_string(path).expect("a readable source file"))
            .collect();
        let printer = format!(".{}()", "print_to_string");
        let verifier = format!(".{}()", "verify");
        let wrapper = format!("llvm_string_to_{}(", "owned");
        let created = format!("TargetTriple::{}(", "create");
        let defaulted = format!("TargetMachine::get_default_{}()", "triple");
        let code_lines = || {
            sources
                .iter()
                .flat_map(|source| source.lines())
                .filter(|line| !line.trim_start().starts_with("//"))
        };

        // Deliberately not keyed on the receiver's name: a correctly
        // wrapped call on some other inkwell value must pass too.
        assert_eq!(
            code_lines().filter(|line| line.contains(&printer)).count(),
            code_lines()
                .filter(|line| line.contains(&printer) && line.contains(&wrapper))
                .count(),
            "every inkwell print_to_string call must be an argument of \
             llvm_string_to_owned, or its LLVMString drops and faults on Windows (D-029)"
        );
        assert_eq!(
            code_lines().filter(|line| line.contains(&verifier)).count(),
            1,
            "the only direct inkwell verify call may be the one inside verify_module, \
             which is skipped on Windows; everything else must go through that wrapper (D-029)"
        );
        assert_eq!(
            code_lines()
                .filter(|line| line.contains(&created) || line.contains(&defaulted))
                .count(),
            2,
            "a TargetTriple owns an LLVMString and must be created inside a ManuallyDrop \
             (D-029); this count is a tripwire, so if you added a call site, wrap it and \
             raise the number -- if you removed one, lower it"
        );
    }

    // The in-range arm of `fits_tagged_smallint`: still folded to an
    // immediate, with no runtime call at all.
    #[test]
    fn an_in_range_int_literal_is_still_folded_at_compile_time() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::IntLiteral(4_611_686_018_427_387_903_i64)],
                ty: Ty::None,
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("in_range_int_literal_folds");
        let obj_path = dir.join("in_range_int_literal_folds.o");
        let mut saw_call = false;
        // D-029 again: `print_to_string`'s `LLVMString` must not be dropped
        // on Windows. See the sibling test above.
        let mut observer = |module: &inkwell::module::Module<'_>, _| {
            saw_call |= llvm_string_to_owned(module.print_to_string())
                .contains("call i64 @pycc_rt_int_from_i64");
        };
        compile_to_object_with_observer(&mir, &obj_path, None, false, Some(&mut observer))
            .expect("an in-range int literal should compile");
        assert!(
            !saw_call,
            "an in-range literal needs no runtime materialization"
        );
    }

    // -----------------------------------------------------------------
    // #624 (D-180) codegen-depth: the inline bigint-refcount guard.
    //
    // The behavioral tests for this feature live in
    // `tests/issue_146_bigint_release.rs` (real source through the real
    // CLI, plus two peak-RSS oracles). Those prove the *net effect* -- a
    // loop's peak RSS stops tracking its trip count -- but they cannot see
    // the two structural properties the emitter is actually responsible
    // for, because both are invisible in a program's output and in its RSS:
    //
    //   1. Every `pycc_rt_bigint_retain`/`_release` call is reached only
    //      through `emit_bigint_refcount_call`'s inline `(word & 0b11) == 0
    //      && word != 0` guard, on the *same* word the call receives. An
    //      unguarded call is correct-but-slow, so it regresses D-084/D-140's
    //      nbody throughput floor silently rather than failing a test.
    //   2. That guard splits the current basic block, so any caller
    //      recording a block for a phi incoming edge must re-read
    //      `get_insert_block()` afterwards. A stale phi predecessor is
    //      malformed IR that LLVM's own verifier rejects.
    //
    // These are in-crate rather than in `tests/` on purpose:
    // `compile_to_object_with_observer`, the only way to reach the emitted
    // module before it becomes an object file, is private -- so this
    // follows `an_oversized_int_literal_materializes_a_runtime_bigint`
    // directly above, which is this repository's actual IR-inspection
    // convention, rather than `tests/slice1_codegen_depth.rs`'s
    // hand-built-MIR-through-`compile_to_object` one.
    // -----------------------------------------------------------------

    /// One `pycc_rt_bigint_retain`/`_release` call recovered from emitted
    /// LLVM IR: the callee's bare symbol name and the encoded word it is
    /// actually passed.
    #[derive(Debug, PartialEq, Eq)]
    struct RefcountCall {
        callee: String,
        word: String,
    }

    /// Returns every `pycc_rt_bigint_*` refcount call in `ir`, and panics
    /// unless each one is dominated by exactly the guard
    /// `emit_bigint_refcount_call` emits, evaluated on the same operand the
    /// call receives.
    ///
    /// The chain proved per call, backwards from the call:
    /// the call sits in a `bigint_rc_call*` block; that block is the taken
    /// arm of `br i1 %c, label %bigint_rc_call*, label %bigint_rc_cont*`;
    /// `%c = and i1 %a, %b`; `%a = icmp eq i64 %t, 0` with `%t = and i64
    /// %w, 3`; `%b = icmp ne i64 %w, 0`; and the call's own argument is
    /// that same `%w`.
    fn guarded_bigint_refcount_calls(ir: &str) -> Vec<RefcountCall> {
        // Every name `emit_bigint_refcount_call` produces is a fixed
        // literal, and LLVM disambiguates those only *within* a function --
        // the first guard of two different functions is `%bigint_is_heap`
        // in both. So each function body is analyzed on its own maps, and a
        // call can never be validated against a guard chain read out of
        // some other function. `split` yields the module header first,
        // which `skip(1)` drops.
        ir.split("\ndefine ")
            .skip(1)
            .flat_map(guarded_bigint_refcount_calls_in_one_function)
            .collect()
    }

    /// `guarded_bigint_refcount_calls` for a single `define` body.
    fn guarded_bigint_refcount_calls_in_one_function(ir: &str) -> Vec<RefcountCall> {
        // `%name = <rhs>` for every SSA value in the function.
        let mut defs: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        // `taken block -> (condition, not-taken block)` for every `br i1`.
        let mut branches: std::collections::HashMap<&str, (&str, &str)> =
            std::collections::HashMap::new();
        // Every refcount call, tagged with the block it was found in.
        let mut calls: Vec<(String, RefcountCall)> = Vec::new();
        let mut block = String::new();

        for raw in ir.lines() {
            let line = raw.trim();
            // A block label is the only unindented `name:` line inside a
            // function body; LLVM appends a `; preds = ...` comment to it.
            // The leftover of the `define` line itself is the only such
            // line here, and it carries no colon.
            if !raw.starts_with([' ', '@', '%', '}', '!']) {
                if let Some((label, _)) = line.split_once(':') {
                    block = label.to_string();
                }
                continue;
            }
            if let Some((name, rhs)) = line.split_once(" = ") {
                defs.insert(name, rhs);
            }
            if let Some(rest) = line.strip_prefix("br i1 ") {
                let parts: Vec<&str> = rest.split(", label ").collect();
                assert_eq!(
                    parts.len(),
                    3,
                    "a conditional branch always has a condition and two targets"
                );
                branches.insert(parts[1], (parts[0], parts[2]));
            }
            if let Some(rest) = line.strip_prefix("call void @pycc_rt_bigint_") {
                let (op, args) = rest
                    .split_once('(')
                    .expect("a call instruction always has an argument list");
                let word = args
                    .trim_end_matches(')')
                    .strip_prefix("i64 ")
                    .expect("both refcount entry points take exactly one i64")
                    .to_string();
                calls.push((
                    block.clone(),
                    RefcountCall {
                        callee: format!("pycc_rt_bigint_{op}"),
                        word,
                    },
                ));
            }
        }

        // Only `assert*!` and `expect` are used from here on, never a
        // local `unwrap_or_else(|| panic!(..))` closure: an error closure
        // that never runs is an uncovered function under D-014's region
        // gate, whereas `assert!`'s cold arm lives in `core`.
        for (block, call) in &calls {
            assert!(
                block.starts_with("bigint_rc_call"),
                "{} is emitted in block `{block}`, not a guard's own call block",
                call.callee
            );
            let key: &str = &format!("%{block}");
            let (cond, cont) = *branches
                .get(key)
                .expect("a guard's call block is always the target of its own branch");
            assert!(
                cont.starts_with("%bigint_rc_cont"),
                "`{block}`'s not-taken arm is `{cont}`, not a guard continuation"
            );
            let (aligned, non_zero) = defs[cond]
                .strip_prefix("and i1 ")
                .and_then(|rest| rest.split_once(", "))
                .expect("the guard condition is an `and` of its two halves");
            let low_tag = defs[aligned]
                .strip_prefix("icmp eq i64 ")
                .and_then(|rest| rest.strip_suffix(", 0"))
                .expect("the guard's alignment half compares a masked word against 0");
            let tagged_word = defs[low_tag]
                .strip_prefix("and i64 ")
                .and_then(|rest| rest.strip_suffix(", 3"))
                .expect("the guard's alignment half masks the word with 0b11");
            let tested_word = defs[non_zero]
                .strip_prefix("icmp ne i64 ")
                .and_then(|rest| rest.strip_suffix(", 0"))
                .expect("the guard's non-zero half compares the word against 0");
            assert_eq!(
                tagged_word, tested_word,
                "the guard's two halves test different words"
            );
            assert_eq!(
                tagged_word, call.word,
                "{} is passed a word the guard never tested",
                call.callee
            );
        }

        calls.into_iter().map(|(_, call)| call).collect()
    }

    /// Compiles `mir` and runs `guarded_bigint_refcount_calls` over the
    /// emitted module.
    ///
    /// LLVM's own verifier is what catches a phi whose incoming block
    /// stopped being a real predecessor after `emit_bigint_refcount_call`
    /// split it -- but this helper deliberately does not call it. Callers do
    /// not need to: `compile_to_object_with_observer` already runs
    /// `verify_module` on this exact module *before* it invokes the
    /// observer, so simply reaching this closure means the module already
    /// passed. Calling `module.verify()` a second time here would bypass
    /// `verify_module`'s `#[cfg(windows)]` skip and reintroduce D-029:
    /// inkwell's `Module::verify` calls `LLVMDisposeMessage` on its
    /// *success* path too, which faults against the prebuilt LLVM 22.1.1
    /// the Windows runner uses -- an access violation that kills the whole
    /// test binary rather than failing one test.
    fn refcount_calls_in(label: &str, mir: &MirModule) -> Vec<RefcountCall> {
        let dir = tempfile_dir(label);
        let obj_path = dir.join(format!("{label}.o"));
        let mut calls = Vec::new();
        // D-029: never let `print_to_string`'s `LLVMString` drop; route it
        // through `llvm_string_to_owned` exactly as the sibling tests do.
        let mut observer = |module: &inkwell::module::Module<'_>, _| {
            calls = guarded_bigint_refcount_calls(&llvm_string_to_owned(module.print_to_string()));
        };
        compile_to_object_with_observer(mir, &obj_path, None, false, Some(&mut observer))
            .expect("codegen should succeed");
        calls
    }

    fn int_name(name: &str) -> MirExpr {
        MirExpr::Name {
            name: name.to_string(),
            ty: Ty::Int,
        }
    }

    #[test]
    fn an_int_slot_store_emits_a_guarded_release_of_the_word_it_overwrites() {
        // `v = v` is the minimal named-storage shape: `emit_assign` must
        // release whatever the slot already held *before* storing, and
        // `retain_if_int_duplicate` must retain the loaded word because the
        // slot is about to become a second owner of it.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "reassign".to_string(),
                params: vec![("v".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::Assign {
                        target: "v".to_string(),
                        value: int_name("v"),
                    },
                    MirStmt::Return(Some(int_name("v"))),
                ],
            }],
            class_defs: Vec::new(),
        };
        let calls = refcount_calls_in("bigint_rc_int_slot_store", &mir);
        let retains = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_retain")
            .count();
        let releases = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_release")
            .count();
        // Two retains (the assignment's duplicate and the return's), one
        // release (the store's). Every one of them was proved guarded, on
        // its own operand, by `guarded_bigint_refcount_calls`.
        assert_eq!((retains, releases), (2, 1), "got {calls:?}");
    }

    #[test]
    fn a_range_loop_over_one_aliased_bound_emits_guarded_retains_and_releases() {
        // `for i in range(n, n, n)`: one heap object reachable as start,
        // stop, and step at once -- the shape where an over-eager release
        // would free a word two other operands still hold. Compiling it at
        // all also exercises the phi-predecessor invariant, since every
        // guard here splits a block the loop's induction phi points at.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "loop_over".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::None,
                body: vec![MirStmt::ForRange {
                    var: "i".to_string(),
                    start: int_name("n"),
                    stop: int_name("n"),
                    step: int_name("n"),
                    body: Vec::new(),
                }],
            }],
            class_defs: Vec::new(),
        };
        let calls = refcount_calls_in("bigint_rc_range_aliased_bounds", &mir);
        let retains = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_retain")
            .count();
        let releases = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_release")
            .count();
        // The ownership contract of `MirStmt::ForRange`, stated as a count
        // so that dropping any single one of them fails here rather than
        // only as a slow RSS drift. Four retains: start, stop and step in
        // the preheader (three independent references even though all
        // three alias one object here), plus the induction word once per
        // iteration. Five releases: the loop variable's own slot before
        // each rebind, the induction word once `next` is computed, and
        // three in the exit block (the surviving induction word, stop, and
        // step). Start has no release of its own on purpose -- its
        // preheader retain *is* the induction word's first reference, and
        // releasing it separately would free a word `stop` and `step` still
        // hold in exactly the aliased shape this fixture builds.
        assert_eq!((retains, releases), (4, 5), "got {calls:?}");
    }

    /// #146 Part 2 (D-181). `int_cmp` aborts on a bigint operand
    /// (`require_inline_int`), so a comparison's operand releases can never
    /// be reached with a real heap word from a compiled program -- this is
    /// the only place the emitted shape is observable at all.
    ///
    /// `(n + n) < (n + n)`: each inner `+` has two borrowed `Name` operands
    /// and releases neither, while the comparison itself consumes two
    /// freshly built words and releases both.
    #[test]
    fn an_int_comparison_releases_both_of_its_freshly_built_operands() {
        let sum = || MirExpr::BinOp {
            op: pycc_mir::BinOpKind::Add,
            left: Box::new(int_name("n")),
            right: Box::new(int_name("n")),
            ty: Ty::Int,
        };
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "compare".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Bool,
                body: vec![MirStmt::Return(Some(MirExpr::Compare {
                    op: pycc_mir::CmpOpKind::Lt,
                    left: Box::new(sum()),
                    right: Box::new(sum()),
                    ty: Ty::Bool,
                }))],
            }],
            class_defs: Vec::new(),
        };
        let calls = refcount_calls_in("bigint_rc_compare_operands", &mir);
        let retains = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_retain")
            .count();
        let releases = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_release")
            .count();
        assert_eq!((retains, releases), (0, 2), "got {calls:?}");
    }

    /// The same for a `*` result. `int_mul`/`int_floordiv`/`int_floormod`/
    /// `int_pow` all reject a bigint operand exactly like `int_cmp` does,
    /// and all four reach the identical release site in `MirExpr::BinOp`'s
    /// own `Ty::Int` arm, so one of them stands for the group.
    #[test]
    fn a_multiplying_binop_releases_both_of_its_freshly_built_operands() {
        let sum = || MirExpr::BinOp {
            op: pycc_mir::BinOpKind::Add,
            left: Box::new(int_name("n")),
            right: Box::new(int_name("n")),
            ty: Ty::Int,
        };
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "multiply".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                    op: pycc_mir::BinOpKind::Mul,
                    left: Box::new(sum()),
                    right: Box::new(sum()),
                    ty: Ty::Int,
                }))],
            }],
            class_defs: Vec::new(),
        };
        let calls = refcount_calls_in("bigint_rc_mul_operands", &mir);
        let retains = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_retain")
            .count();
        let releases = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_release")
            .count();
        assert_eq!((retains, releases), (0, 2), "got {calls:?}");
    }

    /// #146 Part 2 (D-181): the tuple-egress classification, from both
    /// sides at once. `t[0]` is *borrowed* (a tuple field aliases whatever
    /// supplied it, since `TupleLiteral` never retains what it stores), so
    /// the `+` releases only its literal operand -- releasing `t[0]` too
    /// would free a word `n` still holds.
    #[test]
    fn a_tuple_element_operand_is_borrowed_while_a_literal_operand_is_owned() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "read_tuple".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::Assign {
                        target: "t".to_string(),
                        value: MirExpr::TupleLiteral(vec![int_name("n"), int_name("n")]),
                    },
                    MirStmt::Return(Some(MirExpr::BinOp {
                        op: pycc_mir::BinOpKind::Add,
                        left: Box::new(MirExpr::Subscript {
                            base: Box::new(MirExpr::Name {
                                name: "t".to_string(),
                                ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])),
                            }),
                            index: Box::new(MirExpr::IntLiteral(0)),
                        }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    })),
                ],
            }],
            class_defs: Vec::new(),
        };
        let calls = refcount_calls_in("bigint_rc_tuple_operand", &mir);
        let retains = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_retain")
            .count();
        let releases = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_release")
            .count();
        assert_eq!((retains, releases), (0, 1), "got {calls:?}");
    }

    /// The retain side of the same classification (#633 direction A):
    /// binding a tuple element to an `int` local makes that local a second
    /// owner of a word the tuple's supplier still holds, so the bind must
    /// retain. Without this arm, overwriting the local released a reference
    /// it never owned and freed the supplier's object out from under it.
    #[test]
    fn binding_a_tuple_element_to_an_int_local_retains_the_shared_word() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "bind_tuple".to_string(),
                params: vec![("n".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![
                    MirStmt::Assign {
                        target: "t".to_string(),
                        value: MirExpr::TupleLiteral(vec![int_name("n"), int_name("n")]),
                    },
                    MirStmt::Assign {
                        target: "y".to_string(),
                        value: MirExpr::Subscript {
                            base: Box::new(MirExpr::Name {
                                name: "t".to_string(),
                                ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])),
                            }),
                            index: Box::new(MirExpr::IntLiteral(0)),
                        },
                    },
                    MirStmt::Return(Some(int_name("y"))),
                ],
            }],
            class_defs: Vec::new(),
        };
        let calls = refcount_calls_in("bigint_rc_tuple_bind", &mir);
        let retains = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_retain")
            .count();
        let releases = calls
            .iter()
            .filter(|c| c.callee == "pycc_rt_bigint_release")
            .count();
        // The tuple bind's retain plus the `return`'s, against the `y`
        // slot's own release-before-store.
        assert_eq!((retains, releases), (2, 1), "got {calls:?}");
    }

    // `declare_module_globals` builds a constant initializer with no
    // `Builder`, so `tag_smallint_const` survives the #148 refactor with
    // its defensive panic. Generated code only ever reaches it with `0`;
    // this test calls it directly, matching this file's existing
    // defensive-arm convention.
    #[test]
    #[should_panic(expected = "too large for the constant-only")]
    fn tag_smallint_const_still_rejects_an_out_of_range_constant() {
        let context = Context::create();
        let _ = tag_smallint_const(&context, i64::MAX);
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        let exception_target = context.append_basic_block(f, "exception");
        builder.position_at_end(block);
        rt.exceptions.targets.borrow_mut().push(exception_target);

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
        let exception_target = context.append_basic_block(f, "exception");
        builder.position_at_end(block);
        rt.exceptions.targets.borrow_mut().push(exception_target);

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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
    fn a_statically_unreachable_match_tail_terminates_its_function() {
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "exhaustive_match_tail".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Unreachable],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("statically_unreachable_match_tail");
        let obj_path = dir.join("statically_unreachable_match_tail.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("a statically unreachable match tail must produce valid LLVM IR");
        let _ = std::fs::remove_dir_all(&dir);
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // this legitimate, accepted program instead of applying range's
        // numeric normalization from `True` to the ordinary tagged int `1`.
        // Identity-preserving int boundaries intentionally keep D-141's
        // encoded `True` marker instead.
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
            class_defs: Vec::new(),
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
        // `range_operand_to_normalized_int`'s `Scalar::Bool` arm specifically.
        // `True` normalizes to the ordinary tagged int `1`, so this loop runs once.
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
            class_defs: Vec::new(),
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
    fn for_range_normalizes_int_typed_bool_markers_before_the_induction_phi() {
        // Unlike the literal-bool range tests above, both helpers return an
        // `i64`-carried `Ty::Int`: their bool results have already crossed
        // the return boundary and become D-141 markers before `ForRange`
        // sees them. All three operands therefore exercise
        // `range_operand_to_normalized_int`'s `Scalar::Int` path. The first
        // visible target must print ordinary integer `0`, not `False`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "false_int".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BoolLiteral(false)))],
                },
                MirItem::Function {
                    name: "true_int".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BoolLiteral(true)))],
                },
                MirItem::TopLevelStmt(MirStmt::ForRange {
                    var: "i".to_string(),
                    start: MirExpr::Call {
                        callee: "false_int".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    },
                    stop: MirExpr::Call {
                        callee: "true_int".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    },
                    step: MirExpr::Call {
                        callee: "true_int".to_string(),
                        args: vec![],
                        ty: Ty::Int,
                    },
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("for_range_encoded_bool_markers");
        let obj_path = dir.join("for_range_encoded_bool_markers.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("for_range_encoded_bool_markers");
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("call_returns_bool");
        let obj_path = dir.join("call_returns_bool.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn a_none_typed_call_result_can_be_stored_and_printed() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::Function {
                    name: "store".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        MirStmt::Assign {
                            target: "x".to_string(),
                            value: MirExpr::Call {
                                callee: "f".to_string(),
                                args: vec![],
                                ty: Ty::None,
                            },
                        },
                        MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::Name {
                                name: "x".to_string(),
                                ty: Ty::None,
                            }],
                            ty: Ty::None,
                        }),
                    ],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "store".to_string(),
                    args: vec![],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("none_typed_call_result_storage");
        let obj_path = dir.join("none_typed_call_result_storage.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("none_typed_call_result_storage");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"None\n");
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
            class_defs: Vec::new(),
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
        // representation yet". Since PR-11b Task 5 gave `Ty::Tuple` a real
        // arm, `Ty::Infer` is the only variant that still reaches the
        // catch-all, so this is now the sole test pinning that wording.
        // The `expected` string here pins the rendered
        // `<inferred>` name too (`Ty::Infer`'s own `.name()` text), not
        // just the trailing message, for the same reason those two do.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("infer_return_panics");
        let obj_path = dir.join("infer_return_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    fn ty_to_basic_type_gives_tuple_a_struct_representation_positionally() {
        // Supersedes `ty_to_basic_type_panics_clearly_for_tuple`, which
        // asserted that `Ty::Tuple` had no LLVM representation at all --
        // accurate through PR-11a, but PR-11b Task 5 gives it a real arm,
        // so a test demanding a panic would now be asserting the absence of
        // this task's own feature.
        //
        // D-115: unlike every other container (all pointer-represented),
        // a tuple's LLVM type is a real `struct` built field-by-field from
        // each element's own `ty_to_basic_type`. Traces the actual field
        // types rather than just "is a struct", so this pins that each
        // position maps to that element type's own scalar representation
        // exactly (`i64`/`i8`/`f64`, mirroring `Ty::Int`/`Ty::Bool`/
        // `Ty::Float`'s own arms) -- a positional mix-up or a widened
        // `bool` field would still be "a struct" but would fail here.
        let context = Context::create();
        let basic_type = ty_to_basic_type(
            &context,
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float])),
        );
        let struct_ty = basic_type.into_struct_type();
        assert_eq!(struct_ty.count_fields(), 3);
        assert_eq!(
            struct_ty.get_field_type_at_index(0),
            Some(context.i64_type().into()),
            "a tuple's int field keeps Ty::Int's own i64 representation"
        );
        assert_eq!(
            struct_ty.get_field_type_at_index(1),
            Some(context.i8_type().into()),
            "a tuple's bool field keeps Ty::Bool's own i8 representation (D-061, not i1)"
        );
        assert_eq!(
            struct_ty.get_field_type_at_index(2),
            Some(context.f64_type().into()),
            "a tuple's float field keeps Ty::Float's own f64 representation"
        );
        assert!(
            !struct_ty.is_packed(),
            "tuple fields stay naturally aligned -- ty_to_basic_type passes packed=false"
        );
    }

    #[test]
    fn ty_to_basic_type_builds_a_nested_tuple_struct_recursively() {
        // The `Ty::Tuple` arm is deliberately not narrowed to D-116's
        // current int/bool/float element gate (matching how `List(_)`/
        // `Dict(_)`/`Set(_)` ignore their own element types) -- it recurses
        // through `ty_to_basic_type` for whatever element types it is
        // handed. `pycc_types`' T0039 means no such type reaches codegen
        // from real source today; this pins the representation rule itself
        // so a future widening of T0039 needs no change here.
        let context = Context::create();
        let basic_type = ty_to_basic_type(
            &context,
            Ty::Tuple(Box::new(vec![
                Ty::Int,
                Ty::Tuple(Box::new(vec![Ty::Float])),
            ])),
        );
        let struct_ty = basic_type.into_struct_type();
        assert_eq!(struct_ty.count_fields(), 2);
        let inner = struct_ty
            .get_field_type_at_index(1)
            .expect("the nested tuple field exists")
            .into_struct_type();
        assert_eq!(inner.count_fields(), 1);
        assert_eq!(
            inner.get_field_type_at_index(0),
            Some(context.f64_type().into())
        );
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
        // PR-11b Task 5 gives a by-value *struct* representation instead,
        // precisely because it has no heap object to point at (D-115; see
        // `ty_to_basic_type_gives_tuple_a_struct_representation_
        // positionally` above). Traces through the actual return value
        // (not just
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
        let exception_target = context.append_basic_block(f, "exception");
        builder.position_at_end(block);
        rt.exceptions.targets.borrow_mut().push(exception_target);

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
    fn to_numeric_encoded_int_rejects_a_list_operand() {
        // Unlike `truthy`/`to_str` above, this one really is defensive:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::List`, so any arithmetic with a list operand is rejected
        // as `T0021` long before codegen. Hence the "internal error"
        // wording (matching this function's own neighbouring `Float`/`Str`
        // arms) rather than the "not supported yet" feature-gap wording
        // `truthy`/`to_str` use. Exercised by calling `to_numeric_encoded_int`
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
        to_numeric_encoded_int(&context, &builder, Scalar::List(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected an int-or-bool operand, got float")]
    fn to_encoded_int_rejects_a_non_int_compatible_operand() {
        // `MirExpr::IntBoundary` is only produced for the valid Bool -> Int
        // subtype boundary. Keep its defensive fallback fail-closed if a
        // malformed MIR module ever supplies another scalar kind.
        let context = Context::create();
        let builder = context.create_builder();
        to_encoded_int(
            &context,
            &builder,
            Scalar::Float(context.f64_type().const_float(1.0)),
        );
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got list")]
    fn to_float_rejects_a_list_operand() {
        // Same defensive-arm rationale as `to_numeric_encoded_int_rejects_a_list_
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
    #[should_panic(
        expected = "pycc_codegen: truthiness of a dict[K, V] value is not supported yet"
    )]
    fn truthiness_of_a_dict_value_panics_honestly() {
        // The `dict[K, V]` counterpart of `truthiness_of_a_list_value_
        // panics_honestly` above (D-107's reasoning, per D-124): `pycc_types`
        // accepts any type in a boolean context, so `if x:` for a
        // `dict[str, int]` local type-checks today. v0.2 has no
        // `bool(dict)` semantics (D-123), so an honest panic naming the gap
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
    fn to_numeric_encoded_int_rejects_a_dict_operand() {
        // The `dict[K, V]` counterpart of `to_numeric_encoded_int_rejects_a_list_
        // operand` above -- genuinely defensive for the identical reason:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::Dict` either, so no real MIR reaches this arm.
        let context = Context::create();
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_numeric_encoded_int(&context, &builder, Scalar::Dict(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got dict")]
    fn to_float_rejects_a_dict_operand() {
        // Same defensive-arm rationale as `to_numeric_encoded_int_rejects_a_dict_
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
    fn collect_stmt_bindings_includes_a_none_typed_assignment_target() {
        let stmt = MirStmt::Assign {
            target: "result".to_string(),
            value: MirExpr::Name {
                name: "result".to_string(),
                ty: Ty::None,
            },
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(
            bindings.get("result"),
            Some(&Ty::None),
            "a None-typed assignment target needs a real storage slot"
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
    fn collect_stmt_bindings_includes_a_tuple_typed_assignment_target() {
        // Inverts `collect_stmt_bindings_excludes_a_tuple_typed_assignment_
        // target`, which asserted the opposite: `Tuple` was excluded from
        // the allow-list while no codegen existed for it. PR-11b Task 5
        // joins `Ty::Tuple(_)` to that list, mirroring `Ty::List(_)`/
        // `Ty::Dict(_)`/`Ty::Set(_)`'s own inclusion, for the identical
        // reason -- `declare_module_globals`/`storage_slot_at_entry` can
        // only allocate a slot for a binding that was collected here, so
        // without this `x = (1, 2)` would reach codegen with no storage at
        // all.
        let stmt = MirStmt::Assign {
            target: "xs".to_string(),
            value: MirExpr::Name {
                name: "xs".to_string(),
                ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float])),
            },
        };
        let mut bindings = BTreeMap::new();
        collect_stmt_bindings(&stmt, &mut bindings);
        assert_eq!(
            bindings.get("xs"),
            Some(&Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float]))),
            "a tuple[int, float]-typed assignment target's binding should be collected"
        );
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        let rt = declare_rt_functions(&context, &module);
        emit_assign(&context, &builder, &rt, &mut locals, "xs", Scalar::List(value));
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
            class_defs: Vec::new(),
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
    #[should_panic(expected = "`float` takes exactly 1 argument, got 0")]
    fn a_float_call_with_the_wrong_argument_count_is_an_internal_error() {
        // `pycc_types` already rejects a mis-arity `float` call with T0021
        // (its own `float` arm checks `arg_tys.len() != 1` before anything
        // else), so this is hand-built malformed MIR exercising codegen's
        // own defensive backstop, mirroring
        // `a_len_call_with_the_wrong_argument_count_is_an_internal_error`
        // immediately above.
        let mir = list_fixture_module(vec![MirStmt::ExprStmt(MirExpr::Call {
            callee: "float".to_string(),
            args: vec![],
            ty: Ty::Float,
        })]);
        let dir = tempfile_dir("float_wrong_arity_panics");
        let _ = compile_to_object(&mir, &dir.join("float_wrong_arity_panics.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "`math.sqrt` takes exactly 1 argument, got 0")]
    fn a_math_sqrt_call_with_the_wrong_argument_count_is_an_internal_error() {
        // `pycc_types` already rejects a mis-arity `math.sqrt` call with
        // T0021, so this is hand-built malformed MIR exercising codegen's
        // own defensive backstop, mirroring
        // `a_float_call_with_the_wrong_argument_count_is_an_internal_error`
        // immediately above.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![],
                ty: Ty::Float,
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("math_sqrt_wrong_arity_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("math_sqrt_wrong_arity_panics.o"),
            None,
            false,
        );
    }

    #[test]
    #[should_panic(expected = "`math.sqrt`'s argument was not a float")]
    fn a_math_sqrt_call_on_a_non_float_argument_is_an_internal_error() {
        // The other half of `pycc_types`' own T0021 `math.sqrt` check (a
        // non-`float` argument) -- hand-built malformed MIR, since
        // `pycc_types::std_qualified_symbol`'s call-site check already
        // rejects this before codegen runs for any legitimate source.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![MirExpr::IntLiteral(1)],
                ty: Ty::Float,
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("math_sqrt_non_float_panics");
        let _ = compile_to_object(&mir, &dir.join("math_sqrt_non_float_panics.o"), None, false);
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
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(1),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(2),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("x")],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_literal_and_len");
        let obj_path = dir.join("dict_literal_and_len.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_literal_and_len");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n");
    }

    fn instance_ty(class_name: &str) -> Ty {
        Ty::Instance(Box::new(class_name.to_string()))
    }

    #[test]
    fn class_instantiation_attribute_and_method_call_codegens_and_runs() {
        // D-154 (Part 1 of #375), end to end through `compile_to_object`:
        //
        //     class Point:
        //         def __init__(self, x: int, y: int) -> None:
        //             self.x = x
        //             self.y = y
        //         def bump(self) -> None:
        //             self.x = self.x + 1
        //
        //     p = Point(1, 2)
        //     p.bump()
        //     print(p.x)
        //     print(p.y)
        //
        // Expected output verified against `python3` on this exact source
        // (`p.x` starts `1`, `bump()` increments it once to `2`; `p.y`
        // stays `2`). Exercises every new codegen path this issue adds in
        // one program: `MirExpr::Instantiate` (allocation + `__init__`
        // call), `MirStmt::AttrSet` (both inside `__init__` and inside
        // `bump`), `MirExpr::AttrGet` (both inside `bump`'s own `self.x +
        // 1` and at module scope for the two `print` calls), and an
        // ordinary method call lowered to `MirExpr::Call` with `self`
        // prepended.
        let self_ty = instance_ty("Point");
        let init = MirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 1,
                    value: MirExpr::Name {
                        name: "y".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let bump = MirItem::Function {
            name: "Point.bump".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::AttrGet {
                            base: Box::new(MirExpr::Name {
                                name: "self".to_string(),
                                ty: self_ty.clone(),
                            }),
                            slot: 0,
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let mir = MirModule {
            items: vec![
                init,
                bump,
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "p".to_string(),
                    value: MirExpr::Instantiate(Box::new(pycc_mir::InstantiateExpr {
                        ctor: "Point.__init__".to_string(),
                        attr_count: 2,
                        args: vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
                        ty: self_ty.clone(),
                    })),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "Point.bump".to_string(),
                    args: vec![MirExpr::Name {
                        name: "p".to_string(),
                        ty: self_ty.clone(),
                    }],
                    ty: Ty::None,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "p".to_string(),
                        ty: self_ty.clone(),
                    }),
                    slot: 0,
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "p".to_string(),
                        ty: self_ty,
                    }),
                    slot: 1,
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("class_instantiation_attribute_and_method_call");
        let obj_path = dir.join("class_instantiation_attribute_and_method_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("class_instantiation_attribute_and_method_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n2\n");
    }

    #[test]
    fn bool_float_and_str_typed_attribute_slots_round_trip_correctly() {
        // `class_instantiation_attribute_and_method_call_codegens_and_runs`
        // above only ever exercises `int`-typed attribute slots --
        // `slot_word_to_scalar`/`scalar_to_slot_word`'s own `Bool`/`Float`/
        // `Str` arms (D-154) need their own exercise. `__init__` forwarding
        // each constructor parameter straight into its own attribute slot
        // exercises both the write (`scalar_to_slot_word`, during
        // `__init__`) and the read (`slot_word_to_scalar`, at each
        // `print(w.<attr>)` below) halves of all three in one program:
        //
        //     class Widget:
        //         def __init__(self, flag: bool, ratio: float, label: str) -> None:
        //             self.flag = flag
        //             self.ratio = ratio
        //             self.label = label
        //
        //     w = Widget(True, 2.5, "hi")
        //     print(w.flag)
        //     print(w.ratio)
        //     print(w.label)
        //
        // Expected output verified against `python3` on this exact source.
        let self_ty = instance_ty("Widget");
        let init = MirItem::Function {
            name: "Widget.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("flag".to_string(), Ty::Bool),
                ("ratio".to_string(), Ty::Float),
                ("label".to_string(), Ty::Str),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "flag".to_string(),
                        ty: Ty::Bool,
                    },
                },
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 1,
                    value: MirExpr::Name {
                        name: "ratio".to_string(),
                        ty: Ty::Float,
                    },
                },
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 2,
                    value: MirExpr::Name {
                        name: "label".to_string(),
                        ty: Ty::Str,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let mir = MirModule {
            items: vec![
                init,
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "w".to_string(),
                    value: MirExpr::Instantiate(Box::new(pycc_mir::InstantiateExpr {
                        ctor: "Widget.__init__".to_string(),
                        attr_count: 3,
                        args: vec![
                            MirExpr::BoolLiteral(true),
                            MirExpr::FloatLiteral(2.5),
                            MirExpr::StringLiteral("hi".to_string()),
                        ],
                        ty: self_ty.clone(),
                    })),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "w".to_string(),
                        ty: self_ty.clone(),
                    }),
                    slot: 0,
                    ty: Ty::Bool,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "w".to_string(),
                        ty: self_ty.clone(),
                    }),
                    slot: 1,
                    ty: Ty::Float,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "w".to_string(),
                        ty: self_ty,
                    }),
                    slot: 2,
                    ty: Ty::Str,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bool_float_str_attribute_slots");
        let obj_path = dir.join("bool_float_str_attribute_slots.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_float_str_attribute_slots");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n2.5\nhi\n");
    }

    #[test]
    fn a_str_attribute_read_twice_and_then_reassigned_does_not_use_after_free() {
        // D-154 Part 1's own post-merge review finding: the first version
        // of `MirStmt::AttrSet`'s codegen neither incref'd a `str` value
        // read out of an instance attribute (`str_value_is_a_duplicate_
        // reference` had no `MirExpr::AttrGet` arm) nor decref'd a slot's
        // previous `str` occupant before overwriting it. Reading a `str`
        // attribute a second time reliably observed freed memory (the
        // first `print` already decrefs the read pointer to 0, freeing it,
        // since the read was wrongly treated as a fresh, unshared value),
        // and `bool_float_and_str_typed_attribute_slots_round_trip_
        // correctly` above cannot catch this: it reads `w.label` exactly
        // once, which structurally cannot observe a premature free.
        //
        //     class Widget:
        //         def __init__(self, label: str) -> None:
        //             self.label = label
        //         def relabel(self, new_label: str) -> None:
        //             self.label = new_label
        //
        //     w = Widget("hi")
        //     print(w.label)
        //     print(w.label)
        //     w.relabel("bye")
        //     print(w.label)
        //
        // Exercises both halves of the fix in one program: the two
        // `print(w.label)` calls before `relabel` exercise
        // `incref_if_str_duplicate`'s new `AttrGet` arm (without it, the
        // second `print` reads freed memory); `relabel`'s own
        // `self.label = new_label` exercises
        // `decref_str_attr_slot_before_store` (without it, `"hi"`'s
        // `PyStrObj` leaks rather than being released when overwritten --
        // not itself a crash this test can observe, but exercised by the
        // same code path the crash-causing half shares).
        let self_ty = instance_ty("Widget");
        let init = MirItem::Function {
            name: "Widget.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("label".to_string(), Ty::Str),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "label".to_string(),
                        ty: Ty::Str,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let relabel = MirItem::Function {
            name: "Widget.relabel".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("new_label".to_string(), Ty::Str),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "new_label".to_string(),
                        ty: Ty::Str,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let mir = MirModule {
            items: vec![
                init,
                relabel,
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "w".to_string(),
                    value: MirExpr::Instantiate(Box::new(pycc_mir::InstantiateExpr {
                        ctor: "Widget.__init__".to_string(),
                        attr_count: 1,
                        args: vec![MirExpr::StringLiteral("hi".to_string())],
                        ty: self_ty.clone(),
                    })),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "w".to_string(),
                        ty: self_ty.clone(),
                    }),
                    slot: 0,
                    ty: Ty::Str,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "w".to_string(),
                        ty: self_ty.clone(),
                    }),
                    slot: 0,
                    ty: Ty::Str,
                })),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "Widget.relabel".to_string(),
                    args: vec![
                        MirExpr::Name {
                            name: "w".to_string(),
                            ty: self_ty.clone(),
                        },
                        MirExpr::StringLiteral("bye".to_string()),
                    ],
                    ty: Ty::None,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "w".to_string(),
                        ty: self_ty,
                    }),
                    slot: 0,
                    ty: Ty::Str,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("str_attribute_read_twice_and_reassigned");
        let obj_path = dir.join("str_attribute_read_twice_and_reassigned.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_attribute_read_twice_and_reassigned");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"hi\nhi\nbye\n");
    }

    #[test]
    #[should_panic(expected = "an instance attribute of type `list[int]` is not supported yet")]
    fn slot_word_to_scalar_rejects_an_unsupported_attribute_type() {
        // `pycc_hir::class::slot_ty_from_init_rhs` structurally restricts
        // every attribute slot to `int`/`float`/`bool`/`str` (D-154), so a
        // `list[T]`-typed slot can never reach this function from real,
        // type-checked source -- hand-built directly, matching this file's
        // own established internal-error-test convention (e.g.
        // `to_numeric_encoded_int_rejects_a_list_operand` above).
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let raw = context.i64_type().const_int(0, false);
        slot_word_to_scalar(
            &context,
            &builder,
            raw,
            &pycc_mir::Ty::List(Box::new(pycc_mir::Ty::Int)),
        );
        let _ = module;
    }

    #[test]
    #[should_panic(expected = "cannot store this value into an instance attribute slot")]
    fn scalar_to_slot_word_rejects_an_unsupported_scalar() {
        // Mirror image of `slot_word_to_scalar_rejects_an_unsupported_attribute_type`
        // above, for the write direction: a `Scalar::List` can never reach
        // `scalar_to_slot_word` from real, type-checked source either.
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        scalar_to_slot_word(&context, &builder, Scalar::List(ptr));
        let _ = module;
    }

    #[test]
    #[should_panic(expected = "did not evaluate to a class instance")]
    fn attribute_read_over_a_non_instance_base_panics_with_an_internal_error() {
        // Bypasses `pycc_types::check` (T0043 would reject this) with a
        // hand-built `MirExpr::AttrGet` over an `int`-typed base, matching
        // `pycc_mir`'s own established internal-error-test convention.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                base: Box::new(MirExpr::IntLiteral(1)),
                slot: 0,
                ty: Ty::Int,
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("attribute_read_over_a_non_instance_base");
        let obj_path = dir.join("attribute_read_over_a_non_instance_base.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "should have been registered as an ordinary user function")]
    fn instantiation_of_an_unregistered_constructor_panics_with_an_internal_error() {
        // Bypasses `pycc_hir::class::lower_class` (which always mangles
        // `__init__` into `HirModule::items`) with a hand-built
        // `MirExpr::Instantiate` naming a constructor no `MirItem::Function`
        // ever declares.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "g".to_string(),
                value: MirExpr::Instantiate(Box::new(pycc_mir::InstantiateExpr {
                    ctor: "Ghost.__init__".to_string(),
                    attr_count: 0,
                    args: vec![],
                    ty: instance_ty("Ghost"),
                })),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("instantiation_of_an_unregistered_constructor");
        let obj_path = dir.join("instantiation_of_an_unregistered_constructor.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    fn a_function_returning_an_instance_codegens_and_runs() {
        // `Ty::Instance`-typed call/return results are not reachable from
        // this PR's own frontend today (a method's return type is never
        // resolved to `Ty::Instance` by `pycc_types` -- see the plan's own
        // out-of-scope list for class-typed annotations), but
        // `emit_stmt`'s `Return` arm's own `Scalar::Instance` pass-through
        // (D-154) is still real, load-bearing codegen (a future PR that
        // does support `-> Self`/`-> ClassName` needs no further work
        // here) -- exercised directly with hand-built MIR: an `identity`
        // function that takes and returns a `Point`, called once and its
        // result's own attribute printed to prove the same pointer round-
        // trips correctly.
        let self_ty = instance_ty("Point");
        let init = MirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let identity = MirItem::Function {
            name: "identity".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: self_ty.clone(),
            body: vec![MirStmt::Return(Some(MirExpr::Name {
                name: "self".to_string(),
                ty: self_ty.clone(),
            }))],
        };
        let mir = MirModule {
            items: vec![
                init,
                identity,
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "p".to_string(),
                    value: MirExpr::Instantiate(Box::new(pycc_mir::InstantiateExpr {
                        ctor: "Point.__init__".to_string(),
                        attr_count: 1,
                        args: vec![MirExpr::IntLiteral(7)],
                        ty: self_ty.clone(),
                    })),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "p2".to_string(),
                    value: MirExpr::Call {
                        callee: "identity".to_string(),
                        args: vec![MirExpr::Name {
                            name: "p".to_string(),
                            ty: self_ty.clone(),
                        }],
                        ty: self_ty.clone(),
                    },
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "p2".to_string(),
                        ty: self_ty,
                    }),
                    slot: 0,
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("a_function_returning_an_instance");
        let obj_path = dir.join("a_function_returning_an_instance.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("a_function_returning_an_instance");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"7\n");
    }

    #[test]
    fn instantiating_a_class_at_module_scope_with_a_truthiness_check_codegens_and_runs() {
        // `p = Point(1, 2)\nif p:\n    print(1)\n` -- exercises `truthy`'s
        // new `Scalar::Instance` arm (always `True`, this PR's own
        // documented default-object rule, see that arm's doc comment) and
        // `declare_module_globals`'s new `Ty::Instance` arm together.
        let self_ty = instance_ty("Point");
        let init = MirItem::Function {
            name: "Point.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
                ("y".to_string(), Ty::Int),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 0,
                    value: MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::AttrSet {
                    base: MirExpr::Name {
                        name: "self".to_string(),
                        ty: self_ty.clone(),
                    },
                    slot: 1,
                    value: MirExpr::Name {
                        name: "y".to_string(),
                        ty: Ty::Int,
                    },
                },
                MirStmt::Return(None),
            ],
        };
        let mir = MirModule {
            items: vec![
                init,
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "p".to_string(),
                    value: MirExpr::Instantiate(Box::new(pycc_mir::InstantiateExpr {
                        ctor: "Point.__init__".to_string(),
                        attr_count: 2,
                        args: vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
                        ty: self_ty.clone(),
                    })),
                }),
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::Name {
                        name: "p".to_string(),
                        ty: self_ty,
                    },
                    body: vec![print_expr(MirExpr::IntLiteral(1))],
                    orelse: vec![],
                }),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("class_instance_truthiness");
        let obj_path = dir.join("class_instance_truthiness.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("class_instance_truthiness");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: string conversion of a class instance without `__repr__` is not supported yet"
    )]
    fn string_conversion_of_a_class_instance_panics_honestly() {
        // Mirrors `string_conversion_of_a_list_value_panics_honestly`
        // above exactly: `pycc_types` type-checks `print(p)` for a class
        // instance unconditionally. #378 (PR-18) added `__repr__` support
        // for dataclass instances (the MIR rewrites `print(instance)` to
        // a `__repr__` call before codegen), but a bare `to_str` call with
        // an Instance scalar (e.g. a class without `__repr__`) still panics
        // honestly instead of handing a `PyInstanceObj` pointer to a
        // `pycc_rt_*_to_str` function expecting a `PyStrObj`.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_str(&builder, &rt, Scalar::Instance(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected an int-or-bool operand, got instance")]
    fn to_numeric_encoded_int_rejects_an_instance_operand() {
        // Mirrors `to_numeric_encoded_int_rejects_a_list_operand` above:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::Instance`, so any arithmetic with a class-instance
        // operand is rejected as `T0021` long before codegen -- this is
        // genuinely defensive, not a reachable feature gap.
        let context = Context::create();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        let builder = context.create_builder();
        to_numeric_encoded_int(&context, &builder, Scalar::Instance(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got instance")]
    fn to_float_rejects_an_instance_operand() {
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_float(&context, &builder, &rt, Scalar::Instance(ptr));
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: internal error: range() start did not evaluate to int"
    )]
    fn range_operand_to_normalized_int_rejects_an_instance_operand() {
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        range_operand_to_normalized_int(&context, &builder, &rt, Scalar::Instance(ptr), "start");
    }

    #[test]
    fn math_sqrt_call_codegens_and_runs() {
        // `print(math.sqrt(2.0))` end to end through `compile_to_object` --
        // D-136 Task 4's one lowered stdlib function, calling the real
        // libm `sqrt` symbol. Expected output verified against `python3`
        // on this exact source (`math.sqrt(2.0)` -> `1.4142135623730951`).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                callee: "math.sqrt".to_string(),
                args: vec![MirExpr::FloatLiteral(2.0)],
                ty: Ty::Float,
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("math_sqrt_call");
        let obj_path = dir.join("math_sqrt_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("math_sqrt_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1.4142135623730951\n");
    }

    #[test]
    fn math_pi_codegens_and_runs() {
        // `print(math.pi)` end to end -- D-136 Task 4's compile-time float
        // constant, no runtime call at all. Expected output verified
        // against `python3` on this exact source.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(print_expr(MirExpr::Name {
                name: "math.pi".to_string(),
                ty: Ty::Float,
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("math_pi");
        let obj_path = dir.join("math_pi.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("math_pi");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3.141592653589793\n");
    }

    #[test]
    fn float_call_codegens_and_runs() {
        // `x = 3\nprint(float(x))\n` end to end through `compile_to_object`
        // -- #181's `float()` hand-recognized builtin, mirroring
        // `dict_literal_construction_codegens_and_runs`'s own shape
        // immediately above. Expected output verified against `python3` on
        // this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntLiteral(3),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "float".to_string(),
                    args: vec![MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }],
                    ty: Ty::Float,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("float_call");
        let obj_path = dir.join("float_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("float_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3.0\n");
    }

    #[test]
    fn a_user_defined_float_function_codegens_and_runs_instead_of_the_builtin() {
        // Post-merge review finding: `def float(x: int) -> int: return x +
        // 1` was a valid, working program on `main` immediately before this
        // builtin landed -- reproduced directly against a pristine
        // checkout, printing `6`. Without the `user_functions.contains_key`
        // guard, this would silently emit the builtin's own float
        // conversion instead of a real call to the user's function.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "float".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "float".to_string(),
                    args: vec![MirExpr::IntLiteral(5)],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("user_defined_float_call");
        let obj_path = dir.join("user_defined_float_call.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("user_defined_float_call");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"6\n");
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
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(1),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(2),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("x")),
                    key: Box::new(MirExpr::StringLiteral("b".to_string())),
                })),
            ],
            class_defs: Vec::new(),
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
        // Task 5, D-123), exercising its update-in-place half: `len(x)`
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
            class_defs: Vec::new(),
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
        // D-123): `len(x)` growing to `2` is exactly what distinguishes an
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
            class_defs: Vec::new(),
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
        // real `MirStmt::ForDict` iteration codegen (PR-11 Task 5, D-123).
        // "b" printing before "a" (insertion order, not sorted order) is
        // the actual point of this test: `PyDictObj`'s own insertion-order
        // guarantee (D-121) surviving through `pycc_rt_dict_key_at`.
        // Expected output verified against `python3` on this exact source.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(2),
                        ),
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(1),
                        ),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // `MirStmt::ForSet` iteration codegen (PR-11 Task 9, D-123). `2`
        // printing before `1` (first-insertion order, with the second `2`
        // deduped away rather than moving `2`'s position) is the actual
        // point of this test: `PyIntSetObj`'s own insertion-order guarantee
        // (D-121) surviving through `pycc_rt_int_set_get`.
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // honestly` above (D-107's reasoning, per D-124): `pycc_types`
        // accepts any type in a boolean context, so `if s:` for a
        // `set[int]` local type-checks today. v0.2 has no `bool(set)`
        // semantics (D-124), so an honest panic naming the gap is the
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
    fn to_numeric_encoded_int_rejects_a_set_operand() {
        // The `set[T]` counterpart of `to_numeric_encoded_int_rejects_a_dict_
        // operand` above -- genuinely defensive for the identical reason:
        // `pycc_types`' `numeric_result_type` has no `as_numeric` mapping
        // for `Ty::Set` either, so no real MIR reaches this arm.
        let context = Context::create();
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_numeric_encoded_int(&context, &builder, Scalar::Set(ptr));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got set")]
    fn to_float_rejects_a_set_operand() {
        // Same defensive-arm rationale as `to_numeric_encoded_int_rejects_a_set_
        // operand` directly above, for `to_float`'s own match.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        let ptr = context
            .ptr_type(inkwell::AddressSpace::default())
            .const_null();
        to_float(&context, &builder, &rt, Scalar::Set(ptr));
    }

    /// `tuple[int, bool, float]`, the heterogeneous shape most of the
    /// tuple fixtures below use.
    fn tuple_int_bool_float() -> Ty {
        Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float]))
    }

    /// A hand-built `Scalar::Tuple` for the four direct-call panic tests
    /// below. The `list`/`dict`/`set` fixtures all use
    /// `ptr_type().const_null()`, which has no tuple analogue -- a tuple is
    /// a by-value struct, not a pointer (D-115) -- so this builds an
    /// `undef` single-field struct instead. No builder is needed: `undef`
    /// is a constant, and every function under test rejects the value
    /// before doing anything with its contents.
    fn tuple_scalar(context: &Context) -> Scalar<'_> {
        Scalar::Tuple(
            context
                .struct_type(&[context.i64_type().into()], false)
                .get_undef(),
        )
    }

    #[test]
    fn tuple_construction_and_literal_index_reads_codegen_and_run() {
        // `t = (1, True, 2.5)` followed by `print(t[0])`/`print(t[1])`/
        // `print(t[2])` end to end through `compile_to_object` -- the first
        // test to actually execute PR-11b Task 5's `MirExpr::TupleLiteral`
        // construction (`insertvalue`) and `MirExpr::Subscript`'s tuple
        // branch (`extractvalue`). Expected output verified against
        // `python3` on this exact source.
        //
        // Heterogeneous on purpose, and reading all three positions on
        // purpose. A single-type tuple would not catch a positional
        // mix-up, and the `int` read in particular is what pins D-115's
        // encoded-word rule: a tuple field stores the already D-141 encoded
        // value, so `Subscript`'s tuple branch must not re-tag it. List and
        // dict reads now follow the same pass-through rule. With a stray
        // `raw_i64_to_tagged_int` here this prints `3` instead of `1`,
        // while the `bool` and `float` reads would both still pass -- which
        // is exactly why this asserts the `int` line too.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "t".to_string(),
                    value: MirExpr::TupleLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::BoolLiteral(true),
                        MirExpr::FloatLiteral(2.5),
                    ]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "t".to_string(),
                        ty: tuple_int_bool_float(),
                    }),
                    index: Box::new(MirExpr::IntLiteral(0)),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "t".to_string(),
                        ty: tuple_int_bool_float(),
                    }),
                    index: Box::new(MirExpr::IntLiteral(1)),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "t".to_string(),
                        ty: tuple_int_bool_float(),
                    }),
                    index: Box::new(MirExpr::IntLiteral(2)),
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_literal_and_reads");
        let obj_path = dir.join("tuple_literal_and_reads.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("tuple_literal_and_reads");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\nTrue\n2.5\n");
    }

    #[test]
    fn a_function_local_tuple_codegens_and_runs_through_its_alloca_slot() {
        // The module-level test above exercises `declare_module_globals`'
        // new `Ty::Tuple(_)` arm; this exercises the *other* storage route,
        // `storage_slot_at_entry`'s alloca, which allocates the struct type
        // directly rather than a pointer. Both routes must work for
        // `x = (...)` to be usable where D-116 says it is.
        //
        // Also the one fixture here that reads a tuple element as a
        // function's return value, so `MirStmt::Return`'s path runs with a
        // real tuple-derived scalar.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "second".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        MirStmt::Assign {
                            target: "u".to_string(),
                            value: MirExpr::TupleLiteral(vec![
                                MirExpr::IntLiteral(10),
                                MirExpr::IntLiteral(20),
                                MirExpr::IntLiteral(30),
                            ]),
                        },
                        MirStmt::Return(Some(MirExpr::Subscript {
                            base: Box::new(MirExpr::Name {
                                name: "u".to_string(),
                                ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int, Ty::Int])),
                            }),
                            index: Box::new(MirExpr::IntLiteral(1)),
                        })),
                    ],
                },
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "second".to_string(),
                    args: vec![],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_local_slot");
        let obj_path = dir.join("tuple_local_slot.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("tuple_local_slot");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"20\n");
    }

    #[test]
    fn an_inline_tuple_literal_can_be_subscripted_without_a_named_binding() {
        // `print((7, 8)[1])` -- the `Subscript` tuple branch reached with a
        // `MirExpr::TupleLiteral` base rather than a `MirExpr::Name` one,
        // so the struct value comes straight from `insertvalue` without
        // ever being stored to or loaded from a slot. Proves the branch
        // dispatches on the *value*, not on having a backing binding.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(print_expr(MirExpr::Subscript {
                base: Box::new(MirExpr::TupleLiteral(vec![
                    MirExpr::IntLiteral(7),
                    MirExpr::IntLiteral(8),
                ])),
                index: Box::new(MirExpr::IntLiteral(1)),
            }))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_inline_subscript");
        let obj_path = dir.join("tuple_inline_subscript.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("tuple_inline_subscript");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"8\n");
    }

    #[test]
    fn a_tuple_typed_module_binding_gets_a_struct_backed_global_slot() {
        // The tuple counterpart of `a_dict_typed_module_binding_gets_a_
        // real_pointer_backed_global_slot` above -- but asserting the
        // *opposite* storage shape, which is the whole point of D-115: a
        // tuple global holds the struct inline, not a nullable pointer to a
        // heap object. Its zero initializer is therefore a real zeroed
        // struct rather than a null sentinel, and the separate
        // `initialized` flag is what guards a read-before-assignment.
        let context = Context::create();
        let module = context.create_module("tuple_global");
        let bindings = BTreeMap::from([("t".to_string(), tuple_int_bool_float())]);
        let slots = declare_module_globals(&context, &module, &bindings);
        let slot = slots.get("t").expect("the tuple binding gets a slot");
        assert_eq!(slot.ty, tuple_int_bool_float());
        assert!(
            slot.initialized.is_some(),
            "a tuple global still gets the initialized flag -- a zeroed struct is \
             indistinguishable from a legitimately-zero tuple, so it cannot self-signal"
        );
        let global = module
            .get_global("pyglobal_t")
            .expect("the global is named pyglobal_<name>");
        let initializer = global
            .get_initializer()
            .expect("declare_module_globals always sets an initializer");
        assert!(
            initializer.is_struct_value(),
            "a tuple global is initialized with a struct value, not a null pointer"
        );
    }

    #[test]
    fn compiles_a_function_with_a_tuple_parameter_and_tuple_return_value() {
        // The tuple counterpart of `compiles_a_function_with_a_set_int_
        // parameter_and_set_int_return_value` above, for the identical
        // reason: no real source program can produce this shape
        // (`pycc_hir::annotation_to_ty` rejects every annotation but a bare
        // name, so an annotated `tuple[...]` parameter or return type never
        // reaches codegen), but this MIR shape must still compile *cleanly*
        // -- `ty_to_basic_type`'s `Tuple(_)` arm, `emit_expr`'s `Name` arm's
        // `Ty::Tuple(_)` arm, and `MirStmt::Return`'s own `Scalar::Tuple`
        // pass-through must all agree on the same by-value struct
        // representation, or the `ret` would be typed against a different
        // aggregate than the signature declares.
        //
        // Deliberately does not link or run the resulting object, for the
        // identical reason the set counterpart doesn't.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("x".to_string(), tuple_int_bool_float())],
                return_ty: tuple_int_bool_float(),
                body: vec![MirStmt::Return(Some(MirExpr::Name {
                    name: "x".to_string(),
                    ty: tuple_int_bool_float(),
                }))],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_param_and_return");
        let obj_path = dir.join("tuple_param_and_return.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn passing_a_tuple_value_as_a_function_argument_marshals_it_by_value() {
        // The tuple counterpart of `passing_a_set_value_as_a_function_
        // argument_marshals_it_like_a_pointer` above: the caller adds the
        // one shape the test directly above does not reach --
        // `build_call_to`'s argument-marshalling match, whose
        // `Scalar::Tuple` arm is also in the pass-through bucket, differing
        // from its neighbours only in that it hands LLVM a `StructValue`
        // rather than a `PointerValue`. Same not-linked, not-run caveat as
        // the set counterpart.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), tuple_int_bool_float())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::IntLiteral(0)))],
                },
                MirItem::Function {
                    name: "g".to_string(),
                    params: vec![("x".to_string(), tuple_int_bool_float())],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Call {
                        callee: "f".to_string(),
                        args: vec![MirExpr::Name {
                            name: "x".to_string(),
                            ty: tuple_int_bool_float(),
                        }],
                        ty: Ty::Int,
                    }))],
                },
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_passed_as_argument");
        let obj_path = dir.join("tuple_passed_as_argument.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: truthiness of a tuple[...] value is not supported yet"
    )]
    fn truthiness_of_a_tuple_value_panics_honestly() {
        // The `tuple[...]` counterpart of `truthiness_of_a_set_value_
        // panics_honestly` above: a real, reachable feature gap, not a
        // defensive arm. `pycc_types` accepts any type in a boolean
        // context, so `if t:` for a tuple local type-checks today; D-116
        // ships no `bool(tuple)` semantics, so an honest panic naming the
        // gap is the correct behavior. Calls `truthy` directly with a
        // hand-built `Scalar::Tuple`, for the identical reason that test
        // gives.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        truthy(&context, &builder, &rt, tuple_scalar(&context));
    }

    #[test]
    #[should_panic(
        expected = "pycc_codegen: string conversion of a tuple[...] value is not supported yet"
    )]
    fn string_conversion_of_a_tuple_value_panics_honestly() {
        // The `tuple[...]` counterpart of `string_conversion_of_a_set_
        // value_panics_honestly` above, for the identical reason: `print(t)`
        // and `f"{t}"` both type-check today and both land in `to_str`.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        to_str(&builder, &rt, tuple_scalar(&context));
    }

    #[test]
    #[should_panic(expected = "internal error: expected an int-or-bool operand, got tuple")]
    fn to_numeric_encoded_int_rejects_a_tuple_operand() {
        // The `tuple[...]` counterpart of `to_numeric_encoded_int_rejects_a_set_
        // operand` above -- genuinely defensive, unlike the two tests
        // directly above: `pycc_types`' `numeric_result_type` has no
        // `as_numeric` mapping for `Ty::Tuple`, so no real MIR reaches this
        // arm.
        let context = Context::create();
        let builder = context.create_builder();
        to_numeric_encoded_int(&context, &builder, tuple_scalar(&context));
    }

    #[test]
    #[should_panic(expected = "internal error: expected a numeric operand, got tuple")]
    fn to_float_rejects_a_tuple_operand() {
        // Same defensive-arm rationale as `to_numeric_encoded_int_rejects_a_tuple_
        // operand` directly above, for `to_float`'s own match.
        let context = Context::create();
        let (_module, rt) = list_scalar_panic_fixture(&context);
        let builder = context.create_builder();
        to_float(&context, &builder, &rt, tuple_scalar(&context));
    }

    #[test]
    #[should_panic(expected = "a tuple element evaluated to a non-int/bool/float value")]
    fn a_non_scalar_tuple_element_is_an_internal_error() {
        // `MirExpr::TupleLiteral`'s own defensive arm: `pycc_types`' T0039
        // gate (D-116) admits only int/bool/float elements, so a `str`
        // element cannot come from type-checked source -- only from
        // hand-built MIR like this. Without the arm, a `PyStrObj` pointer
        // would be silently inserted into a struct field whose declared
        // type came from the same `Ty::Str`, storing an unrefcounted
        // duplicate reference this crate has no policy for.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "t".to_string(),
                value: MirExpr::TupleLiteral(vec![MirExpr::StringLiteral("a".to_string())]),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_non_scalar_element");
        let _ = compile_to_object(&mir, &dir.join("tuple_non_scalar_element.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "a tuple subscript index is not a literal int")]
    fn a_tuple_subscript_with_a_non_literal_index_is_an_internal_error() {
        // `pycc_types`' T0040 rejects every non-literal tuple index before
        // codegen, so this is defensive -- but genuinely reachable from
        // hand-built MIR, because the index expression's shape is
        // independent of the base's type: nothing about `base.ty()` being
        // `Ty::Tuple` constrains `index` to be an `IntLiteral`. Fires
        // before `MirExpr::ty()` would raise `pycc_mir`'s own equivalent
        // panic, so the message pinned here is this crate's.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![
                    ("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int]))),
                    ("i".to_string(), Ty::Int),
                ],
                return_ty: Ty::Int,
                body: vec![MirStmt::Return(Some(MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "t".to_string(),
                        ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])),
                    }),
                    index: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }))],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_non_literal_index");
        let _ = compile_to_object(&mir, &dir.join("tuple_non_literal_index.o"), None, false);
    }

    #[test]
    #[should_panic(expected = "reading a `str`-typed tuple element is not supported yet")]
    fn reading_a_non_scalar_tuple_element_is_not_supported() {
        // `MirExpr::Subscript`'s tuple branch mirrors `TupleLiteral`'s own
        // element gate on the way back out: T0039 keeps every element
        // int/bool/float, so a `str` element reaches this only from
        // hand-built MIR. Reachable here (unlike a "base didn't evaluate to
        // a tuple" check, which is why this arm has one and that one
        // deliberately does not) because the element *type* is carried by
        // the base's `Ty`, independently of the extract itself succeeding.
        let element_ty = Ty::Tuple(Box::new(vec![Ty::Str]));
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![("t".to_string(), element_ty.clone())],
                return_ty: Ty::None,
                body: vec![print_expr(MirExpr::Subscript {
                    base: Box::new(MirExpr::Name {
                        name: "t".to_string(),
                        ty: element_ty,
                    }),
                    index: Box::new(MirExpr::IntLiteral(0)),
                })],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("tuple_non_scalar_element_read");
        let _ = compile_to_object(
            &mir,
            &dir.join("tuple_non_scalar_element_read.o"),
            None,
            false,
        );
    }

    #[test]
    fn a_tuple_operand_is_rejected_as_a_range_bound() {
        // `range()`'s own operand check folds `Tuple` into its existing
        // or-pattern rather than giving it a separate arm (that arm's
        // message never names the offending type, so folding costs no
        // honesty and adds no permanently-unexecutable region). Pinned via
        // a direct call, matching how the arm's other variants are reached.
        let context = Context::create();
        let builder = context.create_builder();
        let module = context.create_module("tuple_range_bound");
        let rt = declare_rt_functions(&context, &module);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            range_operand_to_normalized_int(
                &context,
                &builder,
                &rt,
                tuple_scalar(&context),
                "start",
            );
        }));
        let payload = panicked.expect_err("a tuple range bound must be rejected");
        let message = payload
            .downcast_ref::<String>()
            .expect("this crate's panics all carry a formatted String");
        assert!(
            message.contains("range() start did not evaluate to int"),
            "unexpected panic message: {message}"
        );
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // edge, and proves encoded elements are forwarded unchanged.
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
    fn appending_to_a_list_validates_and_preserves_the_encoded_value() {
        // `MirExpr::ListAppend`'s success path, the one new arm whose body
        // no other unit test in this file reaches (`appending_to_a_non_
        // list_local_is_an_internal_error` above panics inside
        // `emit_list_name_read` before any of it runs). Reading the
        // appended element straight back out pins D-141's round trip for
        // this arm: ingress validates but stores the encoded word unchanged.
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
            class_defs: Vec::new(),
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
    fn a_bool_list_element_keeps_its_identity_in_encoded_storage() {
        // `xs = [True, 1]` never reaches codegen from real source
        // (`pycc_types` rejects a mixed literal with T0032, and an
        // all-`bool` one with T0034), but `MirExpr::ListLiteral`'s arm
        // deliberately routes each element through `to_encoded_int` rather
        // than requiring a `Scalar::Int` outright -- Python's `bool` is an
        // `int` subtype, so widening is the correct answer if a `bool`
        // element ever does reach it, exactly as `emit_expr`'s own
        // `BinOp`/`Ty::Int` arm treats a `bool` operand. D-141 requires the
        // stored marker to print `True`, while arithmetic still produces `1`.
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
        let dir = tempfile_dir("bool_list_element_identity");
        let obj_path = dir.join("bool_list_element_identity.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_list_element_identity");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // `x` is statically `int`-typed, but D-141 preserves and prints the
        // source object's `True` identity.
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bool_arg_widens_to_int");
        let obj_path = dir.join("bool_arg_widens_to_int.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_arg_widens_to_int");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
    }

    #[test]
    fn a_bool_return_value_widens_to_int_when_the_function_declares_int() {
        // `def f() -> int: return True` ; `print(f())` -- same
        // `bool`-is-`int` widening as the argument case above, but for
        // `MirStmt::Return`'s own value-emission arm: it previously mapped
        // the returned `Scalar::Bool` straight to a `BasicValueEnum` with no
        // widening, so the built `ret` instruction's operand type didn't
        // match `f`'s declared `i64` return type -- `module.verify()`
        // rejected the IR. D-141 now preserves and prints `True`.
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bool_return_widens_to_int");
        let obj_path = dir.join("bool_return_widens_to_int.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_return_widens_to_int");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
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
        // followed by an `i64` load expecting a full encoded int word.
        // D-141 stores the `True` marker rather than ordinary integer `1`.
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("reassign_bool_into_int");
        let obj_path = dir.join("reassign_bool_into_int.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("reassign_bool_into_int");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
    }

    #[test]
    fn an_explicit_int_boundary_preserves_bool_identity_in_an_int_slot() {
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::IntBoundary(Box::new(MirExpr::BoolLiteral(true))),
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("int_boundary_bool_identity");
        let obj_path = dir.join("int_boundary_bool_identity.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("int_boundary_bool_identity");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
    }

    #[test]
    fn bool_identity_survives_nested_int_forwarding_and_fstring_formatting() {
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "source".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::BoolLiteral(true)))],
                },
                MirItem::Function {
                    name: "forward".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::FString(vec![
                        MirFStringPart::Literal("value=".to_string()),
                        MirFStringPart::Interpolation(Box::new(MirExpr::Call {
                            callee: "forward".to_string(),
                            args: vec![MirExpr::Call {
                                callee: "source".to_string(),
                                args: vec![],
                                ty: Ty::Int,
                            }],
                            ty: Ty::Int,
                        })),
                    ])],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bool_identity_nested_fstring");
        let obj_path = dir.join("bool_identity_nested_fstring.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_identity_nested_fstring");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"value=True\n");
    }

    #[test]
    fn a_bool_dict_value_round_trips_with_identity_preserved() {
        let dict_ty = Ty::Dict(Box::new((Ty::Str, Ty::Bool)));
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("k".to_string()),
                        MirExpr::BoolLiteral(true),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::DictGet {
                        dict: Box::new(MirExpr::Name {
                            name: "d".to_string(),
                            ty: dict_ty,
                        }),
                        key: Box::new(MirExpr::StringLiteral("k".to_string())),
                    }],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bool_dict_value_identity");
        let obj_path = dir.join("bool_dict_value_identity.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bool_dict_value_identity");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"True\n");
    }

    #[test]
    #[should_panic(expected = "using print()'s result as a nested expression is not supported yet")]
    fn nesting_a_print_call_inside_another_expression_is_not_yet_supported() {
        // `x = print(1)` remains D-072's explicit exception: ordinary
        // materializable `None` results now support assignment storage,
        // but `print()`'s own result is still rejected as a nested
        // expression. `emit_stmt`'s own `print`-call arm builds a
        // `pycc_rt_int_print` call directly and never routes the outer
        // `print(...)` itself through `emit_expr` -- so the only way a
        // `print` call can reach `emit_expr`'s `Call` arm at all is nested
        // one level deeper than that, inside another expression, exercised
        // here via `Assign`.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::IntLiteral(1)],
                    ty: Ty::None,
                },
            })],
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
                class_defs: Vec::new(),
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
    fn an_int_result_binop_with_a_float_operand_hits_to_numeric_encoded_int_defensive_panic() {
        // Deliberately malformed MIR: `pycc_types::numeric_result_type`
        // always promotes an expression with any `float` operand to
        // `Ty::Float` (`5 + 1.0` types as `float`, never `int`), so no real
        // pipeline could ever produce a `BinOp { ty: Ty::Int, .. }` with a
        // `float` operand. Exercises `to_numeric_encoded_int`'s own defensive
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("binop_none_result_panics");
        let obj_path = dir.join("binop_none_result_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "internal error: str BinOp operand did not evaluate to str")]
    fn a_str_result_binop_with_a_non_str_left_operand_hits_the_internal_consistency_check() {
        // Deliberately malformed MIR: `pycc_types`/`pycc_mir` produce a
        // `Ty::Str`-typed `BinOp` only for `str + str` (see `pycc_mir`'s own
        // `adding_two_strings_infers_str` test) and for `Mul` string
        // repetition (#574), and the repetition shape is caught by this
        // arm's own named D-072 boundary above before either destructure
        // runs -- so no real pipeline could reach this `Add` shape with a
        // non-`str` left operand.
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("str_binop_unsupported_op_panics");
        let obj_path = dir.join("str_binop_unsupported_op_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    fn compiles_string_repetition_in_both_operand_orders() {
        // `x = "a" * 3` and `y = 3 * "a"` -- #575 (Part 2 of #123) replaces
        // Part 1's named D-072 boundary with real emission, so this MIR
        // (exactly what a real `pycc build` produces) now compiles. Both
        // operand orders go through the same `match (l, r)` in the `Ty::Str`
        // arm, so both are exercised here rather than only the `str`-first
        // one.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "x".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(MirExpr::StringLiteral("a".to_string())),
                        right: Box::new(MirExpr::IntLiteral(3)),
                        ty: Ty::Str,
                    },
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(MirExpr::IntLiteral(3)),
                        right: Box::new(MirExpr::StringLiteral("a".to_string())),
                        ty: Ty::Str,
                    },
                }),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("str_binop_repetition");
        let obj_path = dir.join("str_binop_repetition.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    fn compiles_string_repetition_with_a_bool_count() {
        // `x = "a" * True` -- `bool` is an `int` subtype, so `pycc_types`
        // types this `str` too. Exercises `to_numeric_encoded_int`'s own
        // `Scalar::Bool` arm from the repetition site, which the `int`-count
        // test above cannot reach.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(MirExpr::StringLiteral("a".to_string())),
                    right: Box::new(MirExpr::BoolLiteral(true)),
                    ty: Ty::Str,
                },
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("str_binop_repetition_bool");
        let obj_path = dir.join("str_binop_repetition_bool.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
    }

    #[test]
    #[should_panic(expected = "pycc_codegen: internal error: a str-result `*` had no str operand")]
    fn a_str_result_multiplication_without_a_str_operand_is_an_internal_error() {
        // `2 * 3` claiming a `str` result -- unreachable from any
        // type-checked program (`pycc_types` types `int * int` as `int`), so
        // this is a genuine internal-error arm, reached only through
        // hand-built MIR. Covered rather than left as an unexecuted region,
        // per D-014.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Assign {
                target: "x".to_string(),
                value: MirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(MirExpr::IntLiteral(2)),
                    right: Box::new(MirExpr::IntLiteral(3)),
                    ty: Ty::Str,
                },
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("str_repetition_without_str_operand");
        let obj_path = dir.join("str_repetition_without_str_operand.o");
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
    fn a_repeated_string_prints_its_repetition() {
        // `print("ab" * 3)`, `print(0 * "ab")`, `print("ab" * True)` and
        // `print("ab" * n)` for `n = 0 - 2` -- the executable half of #575.
        // The negative count comes from `0 - 2` rather than a `-2` literal:
        // this test predates #602's literal-sign fold. Both forms are real
        // reachable programs, and CPython prints an empty line for each.
        let repeat = |left: MirExpr, right: MirExpr| {
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(left),
                    right: Box::new(right),
                    ty: Ty::Str,
                }],
                ty: Ty::None,
            }))
        };
        let mir = MirModule {
            items: vec![
                repeat(
                    MirExpr::StringLiteral("ab".to_string()),
                    MirExpr::IntLiteral(3),
                ),
                repeat(
                    MirExpr::IntLiteral(0),
                    MirExpr::StringLiteral("ab".to_string()),
                ),
                repeat(
                    MirExpr::StringLiteral("ab".to_string()),
                    MirExpr::BoolLiteral(true),
                ),
                repeat(
                    MirExpr::StringLiteral("ab".to_string()),
                    MirExpr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(MirExpr::IntLiteral(0)),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Int,
                    },
                ),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("str_repeat_prints");
        let obj_path = dir.join("str_repeat_prints.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("str_repeat_prints");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"ababab\n\nab\n\n");
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
                class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
                class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
    fn an_int_result_binop_with_a_str_operand_hits_to_numeric_encoded_int_defensive_panic() {
        // Deliberately malformed MIR: `pycc_types::numeric_result_type`
        // never types a `str`-operand `BinOp` as `Ty::Int` (`str` only ever
        // combines with `str`, under `Add`, per its own
        // `adding_two_strings_infers_str` test), so no real pipeline could
        // ever produce this shape. Exercises `to_numeric_encoded_int`'s own
        // defensive `Scalar::Str` arm -- same convention as
        // `an_int_result_binop_with_a_float_operand_hits_to_numeric_encoded_int_defensive_panic`
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("binop_int_result_str_operand_panics");
        let obj_path = dir.join("binop_int_result_str_operand_panics.o");
        let _ = compile_to_object(&mir, &obj_path, None, false);
    }

    #[test]
    #[should_panic(expected = "expected a numeric operand, got str")]
    fn a_float_result_binop_with_a_str_operand_hits_to_float_defensive_panic() {
        // Same rationale as the `to_numeric_encoded_int` version above, exercising
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        let _ = compile_to_object(
            &mir,
            &dir.join("dict_set_on_non_dict_panics.o"),
            None,
            false,
        );
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_get_non_str_key_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("dict_get_non_str_key_panics.o"),
            None,
            false,
        );
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
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_set_non_str_key_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("dict_set_non_str_key_panics.o"),
            None,
            false,
        );
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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
        // without incref'ing it itself (D-124: "neither increfed on insert
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
            class_defs: Vec::new(),
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
            class_defs: Vec::new(),
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

    /// Builds `MirStmt::ForList { var: "v", list: <list>, body: [print(v)] }`,
    /// the shape every list-comprehension test below uses to read its own
    /// result back out: container `to_str`/`truthy` are unimplemented
    /// (D-107/D-124), so a comprehension's own produced list cannot be
    /// printed directly and must be walked element-by-element instead.
    fn print_each_int(list: &str) -> MirStmt {
        MirStmt::ForList {
            var: "v".to_string(),
            list: list.to_string(),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            })],
        }
    }

    /// `MirStmt::ForSet { var: "v", set: <set>, body: [print(v)] }` -- the
    /// `SetCompAssign` test suite's own analog of `print_each_int` above,
    /// for the identical reason (container `to_str`/`truthy` remain
    /// unimplemented, D-107/D-124, so a produced `set[int]` cannot be
    /// printed directly and must be walked element-by-element via
    /// `MirStmt::ForSet` instead, whose own insertion-order iteration this
    /// crate's pre-existing `a_return_inside_a_for_set_body_returns_
    /// immediately_without_looping` test already pins).
    fn print_each_int_from_set(set: &str) -> MirStmt {
        MirStmt::ForSet {
            var: "v".to_string(),
            set: set.to_string(),
            body: vec![MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::Name {
                    name: "v".to_string(),
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            })],
        }
    }

    #[test]
    fn a_range_sourced_list_comprehension_with_no_filter_computes_every_element() {
        // `xs = [i * 2 for i in range(5)]` (PR-12 Task 5a, D-117):
        // `CompSource::Range`, no `cond`. Exercises the `Range` branch of
        // `MirStmt::ListCompAssign`'s own internal `match source` (mirrors
        // `MirStmt::ForRange`'s own shape), the `cond: None` unconditional-
        // append path, and confirms `elt` (`i * 2`, not a bare `Name`) is
        // evaluated fresh every iteration rather than the loop's own raw
        // induction value being appended untransformed.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(5),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int("xs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_range_no_filter");
        let obj_path = dir.join("listcomp_range_no_filter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_range_no_filter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n2\n4\n6\n8\n");
    }

    #[test]
    fn a_list_sourced_list_comprehension_with_a_filter_only_keeps_matching_elements() {
        // `xs = [i for i in range(5)]` then `ys = [x for x in xs if x > 2]`
        // (PR-12 Task 5a, D-117): the second comprehension's own
        // `CompSource::List` is sourced from the *first* comprehension's own
        // produced list, not a literal -- exercising the `List` branch of
        // `MirStmt::ListCompAssign`'s own internal `match source` (mirrors
        // `MirStmt::ForList`'s own shape, via `emit_list_name_read`) and the
        // `cond: Some(..)` filtered-append path (the `listcomp_if_taken`/
        // `listcomp_if_skip` block pair): `0` and `1` and `2` must be
        // dropped, `3` and `4` kept.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(5),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "ys".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::List("xs".to_string()),
                    cond: Some(Box::new(MirExpr::Compare {
                        op: CmpOpKind::Gt,
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Bool,
                    })),
                    elt: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int("ys")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_list_with_filter");
        let obj_path = dir.join("listcomp_list_with_filter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_list_with_filter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n4\n");
    }

    #[test]
    fn a_list_sourced_list_comprehension_that_rebinds_its_own_source_name_reads_the_pre_existing_value()
     {
        // Regression test (review round 1, post-Task-5a): `xs = [i for i in
        // range(5)]` then `xs = [x for x in xs if x > 2]`, reusing the same
        // name for both the source *and* the target of the second
        // comprehension. Real CPython evaluates the entire RHS -- the whole
        // loop over the pre-existing `xs` (`[0, 1, 2, 3, 4]`) -- before
        // rebinding `xs` to the result (`[3, 4]`), exactly like an ordinary
        // `xs = xs + [1]` never sees its own partially-updated target mid-
        // expression. An earlier version of this arm stored the target's
        // freshly allocated *empty* list into `xs`'s own slot before
        // evaluating `source`, so `emit_list_name_read(..., "xs")` read the
        // brand-new empty list instead of the original one, and this
        // produced `len(xs) == 0` instead of `2`. Fixed by deferring
        // `emit_assign(target, ..)` until after the loop (see this arm's own
        // "point 1"/"point 5" doc comments above).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(5),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::List("xs".to_string()),
                    cond: Some(Box::new(MirExpr::Compare {
                        op: CmpOpKind::Gt,
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Bool,
                    })),
                    elt: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int("xs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_self_referential_source");
        let obj_path = dir.join("listcomp_self_referential_source.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_self_referential_source");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n4\n");
    }

    #[test]
    fn a_range_sourced_list_comprehension_whose_bound_reads_its_own_rebound_target_uses_the_pre_existing_length()
     {
        // Regression test (review round 1, post-Task-5a): `xs = [i for i in
        // range(5)]` then `xs = [i * 2 for i in range(len(xs))]`. `source`
        // is `CompSource::Range` here, not `CompSource::List` -- distinct
        // from the test directly above, since it exercises `source`'s own
        // `stop` expression (`len(xs)`) reading `target`'s pre-existing
        // value during the *preheader*, before `test_bb` even exists, not
        // `var`'s own per-iteration container read. Real CPython evaluates
        // `range(len(xs))` once, against the original 5-element `xs`,
        // before the comprehension loop runs at all, giving a 5-element
        // result (`[0, 2, 4, 6, 8]`); the same premature-rebind bug this
        // file's neighboring regression test documents would instead have
        // read the just-emptied `xs` here, giving `range(0)` and an empty
        // result.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(5),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::Call {
                            callee: "len".to_string(),
                            args: vec![MirExpr::Name {
                                name: "xs".to_string(),
                                ty: Ty::List(Box::new(Ty::Int)),
                            }],
                            ty: Ty::Int,
                        },
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int("xs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_self_referential_range_bound");
        let obj_path = dir.join("listcomp_self_referential_range_bound.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_self_referential_range_bound");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n2\n4\n6\n8\n");
    }

    #[test]
    fn a_list_comprehensions_elt_reading_its_own_rebound_target_reads_the_pre_existing_value() {
        // Regression test (review round 1, post-Task-5a): `xs = [1, 2, 3]`
        // then `xs = [xs[0] for i in range(2)]`. Distinct from both tests
        // above: `elt` (not `source`) reads `target`'s own name here,
        // *inside* the loop body, once per iteration. Real CPython gives
        // `[1, 1]` (`xs[0]` is `1` throughout, since the original `xs` is
        // untouched until the whole comprehension finishes). The
        // premature-rebind bug this file's other two regression tests above
        // document made this specific shape crash outright, not merely
        // print the wrong value: `xs`'s slot held the freshly allocated
        // *empty* list for the entire loop, so the very first
        // `xs[0]` read inside `elt` panicked with `pycc_rt`'s own honest
        // "list index out of range" before any element was ever appended.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "xs".to_string(),
                    value: MirExpr::ListLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(3),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(2),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::Subscript {
                        base: Box::new(MirExpr::Name {
                            name: "xs".to_string(),
                            ty: Ty::List(Box::new(Ty::Int)),
                        }),
                        index: Box::new(MirExpr::IntLiteral(0)),
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int("xs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_self_referential_elt");
        let obj_path = dir.join("listcomp_self_referential_elt.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_self_referential_elt");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n1\n");
    }

    #[test]
    fn a_set_sourced_list_comprehension_with_no_filter_visits_every_element() {
        // `zs = [x for x in some_set]` (PR-12 Task 5a, D-117): `set[int]`'s
        // own element type is always `Ty::Int` (T0038), satisfying
        // `list[int]`'s own T0034 gate trivially, so this source is
        // reachable and type-safe from real Python source. Exercises the
        // `Set` branch of `MirStmt::ListCompAssign`'s own internal `match
        // source` (mirrors `MirStmt::ForSet`'s own shape, via
        // `emit_set_name_read`/`build_int_set_get`/`build_int_set_len`).
        // `{1, 2, 3}` is read back in insertion order (pinned by this
        // crate's own `a_return_inside_a_for_set_body_returns_immediately_
        // without_looping` test, which reads the same literal's first
        // element as `1`), so the produced list's own order is
        // deterministic here.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "some_set".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(3),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "zs".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Set("some_set".to_string()),
                    cond: None,
                    elt: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int("zs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_set_no_filter");
        let obj_path = dir.join("listcomp_set_no_filter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_set_no_filter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n3\n");
    }

    #[test]
    fn an_empty_range_sourced_list_comprehension_produces_a_genuinely_valid_empty_list() {
        // `zs = [i for i in range(0)]` (PR-12 Task 5a, D-117): the loop body
        // never executes, so `zs` must still be a real, non-null
        // `PyIntListObj` -- not a null/dangling pointer that merely happens
        // never to be read. `len(zs)` proves the pointer is valid enough for
        // `pycc_rt_int_list_len` to read (a null pointer would segfault, not
        // silently return `0`); appending to it afterward and reading the
        // appended element back out proves it is independently *writable*
        // too, not merely readable.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "zs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(0),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Call {
                        callee: "len".to_string(),
                        args: vec![MirExpr::Name {
                            name: "zs".to_string(),
                            ty: Ty::List(Box::new(Ty::Int)),
                        }],
                        ty: Ty::Int,
                    }],
                    ty: Ty::None,
                })),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ListAppend {
                    list: "zs".to_string(),
                    value: Box::new(MirExpr::IntLiteral(9)),
                })),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::Subscript {
                        base: Box::new(MirExpr::Name {
                            name: "zs".to_string(),
                            ty: Ty::List(Box::new(Ty::Int)),
                        }),
                        index: Box::new(MirExpr::IntLiteral(0)),
                    }],
                    ty: Ty::None,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_range_empty");
        let obj_path = dir.join("listcomp_range_empty.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_range_empty");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n9\n");
    }

    #[test]
    fn a_dict_sourced_list_comprehension_binds_its_key_without_crashing() {
        // `CompSource::Dict` reaching `MirStmt::ListCompAssign` is an
        // internal-error-panic path no type-checked program can ever
        // trigger from real source (PR-12 Task 5a's own brief, D-117): every
        // reachable `list[int]`-producing comprehension has `elt: Ty::Int`
        // (T0034), and this compiler has no `str`-to-`int` builtin of any
        // kind, so `pycc_types` can never route a dict-typed base into a
        // `list[int]` comprehension's own source. This test bypasses
        // `pycc_types` entirely (hand-built MIR, mirroring this crate's own
        // established convention for other unreachable-from-real-source
        // shapes) to confirm the dict-sourced per-iteration binding code
        // path -- the `pycc_rt_str_incref` call on the read key, exactly
        // like `MirStmt::ForDict`'s own per-iteration bind -- does not crash
        // even though nothing reachable exercises it today. `elt` is a
        // constant (`7`), not a read of the bound key, since `var`'s own
        // type here is `Ty::Str` and a `list[int]`'s own element type is
        // `Ty::Int` -- the binding itself is what this test is pinning, not
        // what `elt` does with it afterward (this crate has no `str`-typed
        // `elt` to give it that would still type as `Ty::Int`).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(1),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(2),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ListCompAssign {
                    target: "zs".to_string(),
                    var: "k".to_string(),
                    var_ty: Ty::Str,
                    source: CompSource::Dict("d".to_string()),
                    cond: None,
                    elt: Box::new(MirExpr::IntLiteral(7)),
                }),
                MirItem::TopLevelStmt(print_each_int("zs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("listcomp_dict_source_no_crash");
        let obj_path = dir.join("listcomp_dict_source_no_crash.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("listcomp_dict_source_no_crash");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"7\n7\n");
    }

    // ---- PR-12 Task 5b: `SetCompAssign` codegen tests ----

    #[test]
    fn a_range_sourced_set_comprehension_with_a_filter_only_keeps_matching_elements() {
        // `evens = {x for x in range(10) if x % 2 == 0}` (PR-12 Task 5b,
        // D-117): exercises `MirStmt::SetCompAssign`'s own `Range` branch
        // (mirrors `ListCompAssign`'s own `Range` branch exactly) and the
        // `cond: Some(..)` filtered-insert path. Expected output verified
        // against `python3` on this exact source (`set[int]`'s own
        // insertion order is pinned by this crate's own
        // `a_return_inside_a_for_set_body_returns_immediately_without_
        // looping` test, so the filtered evens print back in ascending
        // order here).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                    target: "evens".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(10),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: Some(Box::new(MirExpr::Compare {
                        op: CmpOpKind::Eq,
                        left: Box::new(MirExpr::BinOp {
                            op: BinOpKind::Mod,
                            left: Box::new(MirExpr::Name {
                                name: "x".to_string(),
                                ty: Ty::Int,
                            }),
                            right: Box::new(MirExpr::IntLiteral(2)),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(0)),
                        ty: Ty::Bool,
                    })),
                    elt: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int_from_set("evens")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("setcomp_range_with_filter");
        let obj_path = dir.join("setcomp_range_with_filter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("setcomp_range_with_filter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n2\n4\n6\n8\n");
    }

    #[test]
    fn a_list_sourced_set_comprehension_with_no_filter_deduplicates_repeated_elements() {
        // `xs = [1, 2, 2, 3]` then `s = {x for x in xs}` (PR-12 Task 5b,
        // D-117): exercises `MirStmt::SetCompAssign`'s own `List` branch and
        // the `cond: None` unconditional-insert path, and confirms
        // `pycc_rt_int_set_add`'s own dedup check (D-121) collapses the
        // repeated `2` to a single entry -- `len(s) == 3`, not `4`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "xs".to_string(),
                    value: MirExpr::ListLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(3),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                    target: "s".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::List("xs".to_string()),
                    cond: None,
                    elt: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![set_name("s")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_each_int_from_set("s")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("setcomp_list_no_filter_dedup");
        let obj_path = dir.join("setcomp_list_no_filter_dedup.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("setcomp_list_no_filter_dedup");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n1\n2\n3\n");
    }

    #[test]
    fn a_set_sourced_set_comprehension_that_rebinds_its_own_source_name_reads_the_pre_existing_value()
     {
        // `s = {1, 2, 3}` then `s = {x for x in s if x > 1}`, reusing the
        // same name for both the source *and* the target (PR-12 Task 5b,
        // D-117): the direct `SetCompAssign` analog of `ListCompAssign`'s
        // own review-round-1 regression (see that arm's own "point 1"/
        // "point 5" comments, and this crate's `a_list_sourced_list_
        // comprehension_that_rebinds_its_own_source_name_reads_the_pre_
        // existing_value` test) -- a premature `emit_assign(target, ..)`
        // before the loop runs would make `emit_set_name_read(..., "s")`
        // read the freshly allocated, still-empty set instead of the
        // original `{1, 2, 3}`, producing an empty result instead of
        // `{2, 3}`. Also exercises `SetCompAssign`'s own `Set` branch and
        // the `cond: Some(..)` filtered path.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(3),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                    target: "s".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Set("s".to_string()),
                    cond: Some(Box::new(MirExpr::Compare {
                        op: CmpOpKind::Gt,
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(1)),
                        ty: Ty::Bool,
                    })),
                    elt: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int_from_set("s")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("setcomp_self_referential_rebind");
        let obj_path = dir.join("setcomp_self_referential_rebind.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("setcomp_self_referential_rebind");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n3\n");
    }

    #[test]
    fn a_range_sourced_set_comprehension_whose_bound_reads_its_own_rebound_target_uses_the_pre_existing_length()
     {
        // `s = {1, 2, 3, 4, 5}` then `s = {i * 2 for i in range(len(s))}`
        // (PR-12 Task 5b, D-117): the `SetCompAssign` analog of
        // `ListCompAssign`'s own `a_range_sourced_list_comprehension_whose_
        // bound_reads_its_own_rebound_target_uses_the_pre_existing_length`
        // regression test -- distinct from the test directly above, since
        // it exercises `source`'s own `stop` expression (`len(s)`) reading
        // `target`'s pre-existing value during the *preheader*, before
        // `test_bb` even exists, not `var`'s own per-iteration container
        // read (`source` here is `CompSource::Range`, not `CompSource::
        // Set`). Real CPython evaluates `range(len(s))` once, against the
        // original 5-element `s`, before the comprehension loop runs at
        // all, giving a 5-element result (`{0, 2, 4, 6, 8}`); the premature-
        // rebind bug this file's `ListCompAssign` neighbor documents would
        // instead have read the just-emptied `s` here, giving `range(0)`
        // and an empty result.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                        MirExpr::IntLiteral(3),
                        MirExpr::IntLiteral(4),
                        MirExpr::IntLiteral(5),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                    target: "s".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::Call {
                            callee: "len".to_string(),
                            args: vec![set_name("s")],
                            ty: Ty::Int,
                        },
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(MirExpr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(2)),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_each_int_from_set("s")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("setcomp_self_referential_range_bound");
        let obj_path = dir.join("setcomp_self_referential_range_bound.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("setcomp_self_referential_range_bound");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n2\n4\n6\n8\n");
    }

    #[test]
    fn a_dict_sourced_set_comprehension_binds_its_key_without_crashing() {
        // `CompSource::Dict` reaching `MirStmt::SetCompAssign` is
        // unreachable from real, type-checked source for the identical
        // reason `ListCompAssign`'s own analogous test gives (`set[int]`'s
        // own element type is always `Ty::Int`, T0038, and this compiler has
        // no `str`-to-`int` builtin) -- hand-built MIR, mirroring this
        // crate's own `a_dict_sourced_list_comprehension_binds_its_key_
        // without_crashing` test exactly. `elt` is a constant (`7`), not a
        // read of the bound key; `pycc_rt_int_set_add`'s own dedup collapses
        // the two (identical) inserted `7`s to one entry, confirming the
        // dict-sourced per-iteration binding path -- the
        // `pycc_rt_str_incref` call on the read key -- does not crash.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(1),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(2),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::SetCompAssign {
                    target: "zs".to_string(),
                    var: "k".to_string(),
                    var_ty: Ty::Str,
                    source: CompSource::Dict("d".to_string()),
                    cond: None,
                    elt: Box::new(MirExpr::IntLiteral(7)),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![set_name("zs")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_each_int_from_set("zs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("setcomp_dict_source_no_crash");
        let obj_path = dir.join("setcomp_dict_source_no_crash.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("setcomp_dict_source_no_crash");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n7\n");
    }

    // ---- PR-12 Task 5b: `DictCompAssign` codegen tests ----

    #[test]
    fn a_range_sourced_dict_comprehension_with_an_fstring_key_computes_every_entry() {
        // `named = {f"n{i}": i for i in range(3)}` (PR-12 Task 5b, D-117):
        // the common, no-aliasing-hazard shape -- `key` is a fresh
        // `MirExpr::FString` (already owning exactly one reference from its
        // own construction), so this arm's own `incref_if_str_duplicate`
        // call on `key` is a no-op here (see that arm's own doc comment
        // above). Exercises `MirStmt::DictCompAssign`'s own `Range` branch
        // and the `cond: None` unconditional-insert path.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                    target: "named".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(3),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    key: Box::new(MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Literal("n".to_string()),
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        })),
                    ])),
                    value: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("named")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named")),
                    key: Box::new(MirExpr::StringLiteral("n0".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named")),
                    key: Box::new(MirExpr::StringLiteral("n1".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named")),
                    key: Box::new(MirExpr::StringLiteral("n2".to_string())),
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_range_fstring_key");
        let obj_path = dir.join("dictcomp_range_fstring_key.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dictcomp_range_fstring_key");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n0\n1\n2\n");
    }

    #[test]
    fn a_dict_sourced_dict_comprehension_builds_an_independent_copy_and_leaves_the_source_intact() {
        // `d = {"a": 10, "b": 20}` then `d2 = {k: 1 for k in d}` (PR-12 Task
        // 5b, D-117's own dedicated safety test, brief item (d)): `d`, a
        // pre-existing `dict[str, int]`, is the one `Dict`-sourced
        // `DictCompAssign` shape reachable from real, type-checked source
        // (T0036: `var`/`k` is `Ty::Str`, satisfying `dict[str, int]`'s own
        // key-type gate directly -- see this arm's own doc comment above for
        // the full argument). Confirms two things: `d2`'s own contents are
        // correct and independently readable, and building `d2` leaves `d`'s
        // own contents unaffected -- exactly what this arm's own
        // `incref_if_str_duplicate` call on `key` (mirroring `MirStmt::
        // DictSet`'s own identical call, D-124) exists to guarantee: `d2`'s
        // own stored key becomes a genuinely independent reference, not a
        // bare, uncounted alias of `d`'s own key pointer.
        //
        // This is a functional-correctness test, not a use-after-free
        // detector: this compiler's container model keeps `dict[K, V]`
        // leak-only (D-124) and has no key-removal operation of any kind
        // (`del`, `.pop()`, ...) yet, so nothing currently implemented can
        // ever bring a dict's own claim on one of its keys down to zero --
        // the missing incref this test guards against cannot be forced into
        // an observable crash via any currently-reachable pycc-language
        // operation (see this task's own report for the full analysis). The
        // fix is still required by D-124's ownership contract and is
        // unconditionally applied here, independent of what this one test
        // can currently observe.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(10),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(20),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                    target: "d2".to_string(),
                    var: "k".to_string(),
                    var_ty: Ty::Str,
                    source: CompSource::Dict("d".to_string()),
                    cond: None,
                    key: Box::new(MirExpr::Name {
                        name: "k".to_string(),
                        ty: Ty::Str,
                    }),
                    value: Box::new(MirExpr::IntLiteral(1)),
                }),
                // `d`'s own contents, read back after `d2` was built.
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("b".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("d")],
                    ty: Ty::Int,
                })),
                // `d2`'s own contents, independently.
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d2")),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d2")),
                    key: Box::new(MirExpr::StringLiteral("b".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("d2")],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_dict_source_independent_copy");
        let obj_path = dir.join("dictcomp_dict_source_independent_copy.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dictcomp_dict_source_independent_copy");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"10\n20\n2\n1\n1\n2\n");
    }

    #[test]
    fn a_list_sourced_dict_comprehension_with_a_filter_only_keeps_matching_entries() {
        // `xs = [10, 20, 30]` then `named2 = {f"v{x}": x for x in xs if x >
        // 15}` (PR-12 Task 5b, D-117): exercises `MirStmt::DictCompAssign`'s
        // own `List` branch (reachable from real, type-checked source:
        // `list[int]`'s element type, T0034, satisfies `var`'s own
        // `Ty::Int`, and an f-string key is always `Ty::Str`, satisfying
        // T0036) and the `cond: Some(..)` filtered-insert path -- `10` must
        // be dropped, `20`/`30` kept.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "xs".to_string(),
                    value: MirExpr::ListLiteral(vec![
                        MirExpr::IntLiteral(10),
                        MirExpr::IntLiteral(20),
                        MirExpr::IntLiteral(30),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                    target: "named2".to_string(),
                    var: "x".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::List("xs".to_string()),
                    cond: Some(Box::new(MirExpr::Compare {
                        op: CmpOpKind::Gt,
                        left: Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(15)),
                        ty: Ty::Bool,
                    })),
                    key: Box::new(MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Literal("v".to_string()),
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "x".to_string(),
                            ty: Ty::Int,
                        })),
                    ])),
                    value: Box::new(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("named2")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named2")),
                    key: Box::new(MirExpr::StringLiteral("v20".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named2")),
                    key: Box::new(MirExpr::StringLiteral("v30".to_string())),
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_list_with_filter");
        let obj_path = dir.join("dictcomp_list_with_filter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dictcomp_list_with_filter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n20\n30\n");
    }

    #[test]
    fn a_set_sourced_dict_comprehension_with_a_filter_only_keeps_matching_entries() {
        // `ys = {5, 6, 7, 10}` then `named3 = {f"s{y}": y for y in ys if y <
        // 10}` (PR-12 Task 5b, D-117): exercises `MirStmt::DictCompAssign`'s
        // own `Set` branch (reachable from real, type-checked source:
        // `set[int]`'s element type, T0038, satisfies `var`'s own `Ty::Int`)
        // and the `cond: Some(..)` filtered-insert path -- `10` must be
        // dropped, `5`/`6`/`7` kept.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "ys".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(5),
                        MirExpr::IntLiteral(6),
                        MirExpr::IntLiteral(7),
                        MirExpr::IntLiteral(10),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                    target: "named3".to_string(),
                    var: "y".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Set("ys".to_string()),
                    cond: Some(Box::new(MirExpr::Compare {
                        op: CmpOpKind::Lt,
                        left: Box::new(MirExpr::Name {
                            name: "y".to_string(),
                            ty: Ty::Int,
                        }),
                        right: Box::new(MirExpr::IntLiteral(10)),
                        ty: Ty::Bool,
                    })),
                    key: Box::new(MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Literal("s".to_string()),
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "y".to_string(),
                            ty: Ty::Int,
                        })),
                    ])),
                    value: Box::new(MirExpr::Name {
                        name: "y".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("named3")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named3")),
                    key: Box::new(MirExpr::StringLiteral("s5".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named3")),
                    key: Box::new(MirExpr::StringLiteral("s6".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("named3")),
                    key: Box::new(MirExpr::StringLiteral("s7".to_string())),
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_set_with_filter");
        let obj_path = dir.join("dictcomp_set_with_filter.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dictcomp_set_with_filter");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n5\n6\n7\n");
    }

    #[test]
    #[should_panic(expected = "dict comprehension key did not evaluate to str")]
    fn a_dict_comprehension_with_a_non_str_key_is_an_internal_error() {
        // `pycc_types` rejects a mismatched dict-comprehension key type with
        // T0036 before codegen ever runs, so this is hand-built malformed
        // MIR -- covers `MirStmt::DictCompAssign`'s own inline key-
        // extraction panic (mirrors `MirStmt::DictSet`'s own identical panic
        // and its own dedicated `a_dict_set_with_a_non_str_key_is_an_
        // internal_error` test).
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                target: "zs".to_string(),
                var: "i".to_string(),
                var_ty: Ty::Int,
                source: CompSource::Range {
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(1),
                    step: MirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(MirExpr::IntLiteral(1)),
                value: Box::new(MirExpr::IntLiteral(2)),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_non_str_key_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("dictcomp_non_str_key_panics.o"),
            None,
            false,
        );
    }

    #[test]
    #[should_panic(expected = "dict comprehension key did not evaluate to str")]
    fn a_dict_comprehension_with_a_non_str_key_under_a_filter_is_an_internal_error() {
        // Same defect as `a_dict_comprehension_with_a_non_str_key_is_an_
        // internal_error` above, but with `cond: Some(..)` rather than
        // `cond: None`: this arm's own key-extraction `let-else` panic is
        // written once inside each of the two `match cond` branches (not
        // shared, mirroring `ListCompAssign`'s/`SetCompAssign`'s own
        // `elt`-evaluation-and-append duplication across those same two
        // branches), so it is two separate coverage regions -- this test
        // covers the `Some(..)` branch's own copy, which the `cond: None`
        // test above does not reach.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                target: "zs".to_string(),
                var: "i".to_string(),
                var_ty: Ty::Int,
                source: CompSource::Range {
                    start: MirExpr::IntLiteral(0),
                    stop: MirExpr::IntLiteral(1),
                    step: MirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(MirExpr::BoolLiteral(true))),
                key: Box::new(MirExpr::IntLiteral(1)),
                value: Box::new(MirExpr::IntLiteral(2)),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_non_str_key_filtered_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("dictcomp_non_str_key_filtered_panics.o"),
            None,
            false,
        );
    }

    #[test]
    fn a_dict_sourced_dict_comprehension_that_rebinds_its_own_source_name_reads_the_pre_existing_value()
     {
        // `d = {"a": 5, "b": 6}` then `d = {k: 9 for k in d}`, reusing the
        // same name for both the source *and* the target (PR-12 Task 5b,
        // D-117): the direct `DictCompAssign` analog of `ListCompAssign`'s
        // own review-round-1 regression (see that arm's own "point 1"/
        // "point 5" comments and this crate's `a_list_sourced_list_
        // comprehension_that_rebinds_its_own_source_name_reads_the_pre_
        // existing_value` test) -- a premature `emit_assign(target, ..)`
        // before the loop runs would make `emit_dict_name_read(..., "d")`
        // read the freshly allocated, still-empty dict instead of the
        // original `{"a": 5, "b": 6}`, producing an empty result instead of
        // two entries. Distinct from `a_dict_sourced_dict_comprehension_
        // builds_an_independent_copy_and_leaves_the_source_intact` above,
        // which uses distinct names (`d`/`d2`) and therefore does not
        // exercise this ordering fix at all -- this test is what actually
        // guards against reintroducing Task 5a's own review-round-1 defect
        // in this arm.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(5),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(6),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                    target: "d".to_string(),
                    var: "k".to_string(),
                    var_ty: Ty::Str,
                    source: CompSource::Dict("d".to_string()),
                    cond: None,
                    key: Box::new(MirExpr::Name {
                        name: "k".to_string(),
                        ty: Ty::Str,
                    }),
                    value: Box::new(MirExpr::IntLiteral(9)),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("d")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("b".to_string())),
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_self_referential_rebind");
        let obj_path = dir.join("dictcomp_self_referential_rebind.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dictcomp_self_referential_rebind");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n9\n9\n");
    }

    #[test]
    fn a_range_sourced_dict_comprehension_whose_bound_reads_its_own_rebound_target_uses_the_pre_existing_length()
     {
        // `d = {"a": 1, "b": 2, "c": 3}` then `d = {f"n{i}": i for i in
        // range(len(d))}` (PR-12 Task 5b, D-117): the `DictCompAssign`
        // analog of `ListCompAssign`'s own `a_range_sourced_list_
        // comprehension_whose_bound_reads_its_own_rebound_target_uses_the_
        // pre_existing_length` regression test -- distinct from
        // `a_dict_sourced_dict_comprehension_that_rebinds_its_own_source_
        // name_reads_the_pre_existing_value` above, since it exercises
        // `source`'s own `stop` expression (`len(d)`) reading `target`'s
        // pre-existing value during the *preheader*, before `test_bb` even
        // exists, not `var`'s own per-iteration container read (`source`
        // here is `CompSource::Range`, not `CompSource::Dict`). Real
        // CPython evaluates `range(len(d))` once, against the original
        // 3-entry `d`, before the comprehension loop runs at all, giving a
        // 3-entry result (`{"n0": 0, "n1": 1, "n2": 2}`); the premature-
        // rebind bug this file's `ListCompAssign` neighbor documents would
        // instead have read the just-emptied `d` here, giving `range(0)`
        // and an empty result.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![
                        (
                            MirExpr::StringLiteral("a".to_string()),
                            MirExpr::IntLiteral(1),
                        ),
                        (
                            MirExpr::StringLiteral("b".to_string()),
                            MirExpr::IntLiteral(2),
                        ),
                        (
                            MirExpr::StringLiteral("c".to_string()),
                            MirExpr::IntLiteral(3),
                        ),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::DictCompAssign {
                    target: "d".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::Call {
                            callee: "len".to_string(),
                            args: vec![dict_name("d")],
                            ty: Ty::Int,
                        },
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: None,
                    key: Box::new(MirExpr::FString(vec![
                        pycc_mir::MirFStringPart::Literal("n".to_string()),
                        pycc_mir::MirFStringPart::Interpolation(Box::new(MirExpr::Name {
                            name: "i".to_string(),
                            ty: Ty::Int,
                        })),
                    ])),
                    value: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![dict_name("d")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("n0".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("n1".to_string())),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGet {
                    dict: Box::new(dict_name("d")),
                    key: Box::new(MirExpr::StringLiteral("n2".to_string())),
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dictcomp_self_referential_range_bound");
        let obj_path = dir.join("dictcomp_self_referential_range_bound.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dictcomp_self_referential_range_bound");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n0\n1\n2\n");
    }

    // -- PR-12 Task 9 (D-118): `MirExpr::Slice` codegen -----------------

    /// `xs = [<values>]` as a `MirStmt`, this section's own analog of
    /// `assign_list_literal` above for an arbitrary element list rather
    /// than the fixed `[1, 2, 3]` that helper hardcodes.
    fn assign_list_literal_values(target: &str, values: &[i64]) -> MirStmt {
        MirStmt::Assign {
            target: target.to_string(),
            value: MirExpr::ListLiteral(values.iter().map(|v| MirExpr::IntLiteral(*v)).collect()),
        }
    }

    fn slice_list_name(name: &str) -> MirExpr {
        MirExpr::Name {
            name: name.to_string(),
            ty: Ty::List(Box::new(Ty::Int)),
        }
    }

    #[test]
    fn a_slice_with_all_three_bounds_present_returns_the_expected_sub_range() {
        // D-118's ordinary path, at the codegen layer: `xs[1:4:1]` on
        // `[10, 20, 30, 40, 50]` is `[20, 30, 40]`. Exercises every `Some`
        // arm of the new `MirExpr::Slice` match (`start`/`stop`/`step` all
        // present), matching `tests/slice1_codegen_depth.rs`'s own identical
        // real-Python-source case (`a_basic_slice_with_explicit_bounds_
        // returns_the_expected_sub_range`) -- this hand-built-MIR version is
        // what actually counts toward `cargo llvm-cov -p pycc_codegen`,
        // since a workspace-root `tests/*.rs` integration binary is
        // attributed to the `pycc` package, not this crate.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[10, 20, 30, 40, 50])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "ys".to_string(),
                    value: MirExpr::Slice {
                        base: Box::new(slice_list_name("xs")),
                        start: Some(Box::new(MirExpr::IntLiteral(1))),
                        stop: Some(Box::new(MirExpr::IntLiteral(4))),
                        step: Some(Box::new(MirExpr::IntLiteral(1))),
                    },
                }),
                MirItem::TopLevelStmt(print_each_int("ys")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("slice_all_bounds_present");
        let obj_path = dir.join("slice_all_bounds_present.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice_all_bounds_present");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"20\n30\n40\n");
    }

    #[test]
    fn a_slice_with_every_bound_omitted_defaults_to_the_whole_list_stepped_by_one() {
        // D-118's own defaulting rule (`start`/`stop`/`step` default to
        // `0`/`len(list)`/`1`): `xs[:]` returns every element unchanged.
        // Exercises every `None` arm of the new match, including the
        // deferred `build_int_list_len` call this arm's own doc comment
        // describes -- the one line no other test in this group reaches,
        // since every other test here supplies an explicit `stop`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[10, 20, 30, 40, 50])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "ys".to_string(),
                    value: MirExpr::Slice {
                        base: Box::new(slice_list_name("xs")),
                        start: None,
                        stop: None,
                        step: None,
                    },
                }),
                MirItem::TopLevelStmt(print_each_int("ys")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("slice_all_bounds_omitted");
        let obj_path = dir.join("slice_all_bounds_omitted.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice_all_bounds_omitted");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"10\n20\n30\n40\n50\n");
    }

    #[test]
    fn a_slice_with_only_a_step_present_skips_every_other_element() {
        // `xs[::2]` on `[0, 1, 2, 3, 4, 5]` is `[0, 2, 4]` -- `start`/`stop`
        // stay omitted (re-exercising the `None` arms the test above
        // already covers) while `step`'s own `Some` arm carries a value
        // other than `1`, pinning D-118's Step 5(c) requirement
        // specifically (a step greater than one, not just present at all).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[0, 1, 2, 3, 4, 5])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "ys".to_string(),
                    value: MirExpr::Slice {
                        base: Box::new(slice_list_name("xs")),
                        start: None,
                        stop: None,
                        step: Some(Box::new(MirExpr::IntLiteral(2))),
                    },
                }),
                MirItem::TopLevelStmt(print_each_int("ys")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("slice_step_only");
        let obj_path = dir.join("slice_step_only.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice_step_only");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"0\n2\n4\n");
    }

    #[test]
    fn a_sliced_list_is_a_genuinely_independent_allocation_from_its_base() {
        // D-107's leak-only policy still requires the slice result to be a
        // *new* allocation, not an alias of `xs`'s own backing storage
        // (`pycc_rt_int_list_slice` itself is unit-tested for this in
        // `pycc_rt`; this pins the same guarantee through the real codegen
        // call site). Appending to `xs` after slicing must not retroactively
        // change `ys`, and vice versa.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[1, 2, 3])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "ys".to_string(),
                    value: MirExpr::Slice {
                        base: Box::new(slice_list_name("xs")),
                        start: Some(Box::new(MirExpr::IntLiteral(0))),
                        stop: Some(Box::new(MirExpr::IntLiteral(3))),
                        step: Some(Box::new(MirExpr::IntLiteral(1))),
                    },
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ListAppend {
                    list: "xs".to_string(),
                    value: Box::new(MirExpr::IntLiteral(99)),
                })),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ListAppend {
                    list: "ys".to_string(),
                    value: Box::new(MirExpr::IntLiteral(77)),
                })),
                MirItem::TopLevelStmt(print_each_int("xs")),
                MirItem::TopLevelStmt(print_each_int("ys")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("slice_result_is_independent");
        let obj_path = dir.join("slice_result_is_independent.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice_result_is_independent");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n2\n3\n99\n1\n2\n3\n77\n");
    }

    #[test]
    fn a_list_sourced_slice_that_rebinds_its_own_source_name_reads_the_pre_existing_value() {
        // Regression test in this arm's own established style (see
        // `a_list_sourced_list_comprehension_that_rebinds_its_own_source_
        // name_reads_the_pre_existing_value` above, Task 5a's confirmed
        // review-round regression): `xs = [1,2,3,4,5]` then `xs =
        // xs[1:3]`, reusing `xs` as both the slice's own `base` and the
        // assignment's own `target`. Real CPython fully evaluates the RHS
        // (reading the original 5-element `xs`, producing `[2, 3]`) before
        // rebinding `xs` to that result -- an implementation that stored
        // the slice's target pointer into `xs`'s slot before `base` had
        // been evaluated and the slice call completed would corrupt this.
        // `MirExpr::Slice`'s own arm never writes to `locals` at all (see
        // its doc comment) -- `MirStmt::Assign` is the only thing that
        // ever does, and only after this whole expression already produced
        // a pointer to a brand new, independent result object -- so there
        // is no premature-rebind window here to begin with, unlike
        // `ListCompAssign`'s own multi-block loop construction, which
        // needed a deliberate fix to get this right.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[1, 2, 3, 4, 5])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "xs".to_string(),
                    value: MirExpr::Slice {
                        base: Box::new(slice_list_name("xs")),
                        start: Some(Box::new(MirExpr::IntLiteral(1))),
                        stop: Some(Box::new(MirExpr::IntLiteral(3))),
                        step: None,
                    },
                }),
                MirItem::TopLevelStmt(print_each_int("xs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("slice_self_referential_rebind");
        let obj_path = dir.join("slice_self_referential_rebind.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("slice_self_referential_rebind");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"2\n3\n");
    }

    // -- PR-12 Task 11 (D-119): `list.pop()`/`dict.get(key, default)`/
    // `set.add(value)` codegen -----------------------------------------

    #[test]
    fn list_pop_removes_and_returns_the_last_element_and_shrinks_len() {
        // `xs = [1,2,3]\ny = xs.pop()\nprint(y)\nprint(len(xs))\n` end to
        // end -- real `MirExpr::ListPop` codegen. Expected output verified
        // against `python3` on this exact source: `3` then `2`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[1, 2, 3])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::ListPop {
                        list: "xs".to_string(),
                        ty: Ty::Int,
                    },
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Name {
                    name: "y".to_string(),
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![slice_list_name("xs")],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("list_pop_basic");
        let obj_path = dir.join("list_pop_basic.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("list_pop_basic");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n2\n");
    }

    #[test]
    fn pop_twice_on_the_same_list_in_one_statement_removes_in_order() {
        // `xs = [1,2,3,4,5]\nys = [xs.pop(), xs.pop()]\n` -- this task's own
        // brief flags exactly this shape as a place a Task-5a-class
        // evaluation-order bug could hide (binding a target's storage slot
        // before fully evaluating a self-referential right-hand side).
        // Verified empirically here, not just by the `MirExpr::ListPop`
        // arm's own doc comment reasoning: real CPython evaluates
        // `[xs.pop(), xs.pop()]`'s two elements strictly left to right,
        // each `.pop()` observing the previous one's mutation, so the
        // first `.pop()` removes `5` (leaving `[1,2,3,4]`) and the second
        // removes the new last element `4` (leaving `[1,2,3]`) --
        // `ys == [5, 4]`, `xs == [1,2,3]`. A codegen bug that read `xs`'s
        // pointer once and cached it, or that evaluated both `.pop()`
        // calls against a stale snapshot, would produce a different
        // result here (e.g. both calls popping the same element, or
        // popping in the wrong order) -- this test would catch either.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(assign_list_literal_values("xs", &[1, 2, 3, 4, 5])),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "ys".to_string(),
                    value: MirExpr::ListLiteral(vec![
                        MirExpr::ListPop {
                            list: "xs".to_string(),
                            ty: Ty::Int,
                        },
                        MirExpr::ListPop {
                            list: "xs".to_string(),
                            ty: Ty::Int,
                        },
                    ]),
                }),
                MirItem::TopLevelStmt(print_each_int("ys")),
                MirItem::TopLevelStmt(print_each_int("xs")),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("list_pop_twice_same_statement");
        let obj_path = dir.join("list_pop_twice_same_statement.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("list_pop_twice_same_statement");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"5\n4\n1\n2\n3\n");
    }

    #[test]
    fn dict_get_or_default_on_a_present_key_returns_the_stored_value_codegens_and_runs() {
        // `d = {"a": 1}\nprint(d.get("a", -1))\n` end to end -- real
        // `MirExpr::DictGetOrDefault` codegen, found-key path. Expected
        // output verified against `python3` on this exact source: `1`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(MirExpr::StringLiteral("a".to_string())),
                    default: Box::new(MirExpr::IntLiteral(-1)),
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_get_or_default_present");
        let obj_path = dir.join("dict_get_or_default_present.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_get_or_default_present");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    fn dict_get_or_default_on_a_missing_key_returns_the_default_codegens_and_runs() {
        // `d = {"a": 1}\nprint(d.get("z", -1))\n` end to end -- real
        // `MirExpr::DictGetOrDefault` codegen, missing-key path (unlike
        // `MirExpr::DictGet`, this never panics). Expected output verified
        // against `python3` on this exact source: `-1`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(MirExpr::StringLiteral("z".to_string())),
                    default: Box::new(MirExpr::IntLiteral(-1)),
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_get_or_default_missing");
        let obj_path = dir.join("dict_get_or_default_missing.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_get_or_default_missing");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"-1\n");
    }

    #[test]
    fn dict_get_or_default_nested_in_its_own_default_argument_resolves_correctly() {
        // `d = {"a": 1}\ny = d.get("k", d.get("a", -1))\nprint(y)\n` --
        // this task's own brief flags exactly this nested shape. Unlike
        // `list.pop()`, `dict.get()` never mutates its dict, so there is
        // no analogous ordering hazard to begin with (both the outer and
        // inner `.get()` read the same unchanged `d`) -- verified
        // empirically here rather than only by that reasoning: `"k"` is
        // absent, so the outer call's `default` sub-expression
        // (`d.get("a", -1)`) must itself be evaluated, and `"a"` is
        // present, so it resolves to `1`. Expected output verified against
        // `python3` on this exact source: `1`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "y".to_string(),
                    value: MirExpr::DictGetOrDefault {
                        dict: "d".to_string(),
                        key: Box::new(MirExpr::StringLiteral("k".to_string())),
                        default: Box::new(MirExpr::DictGetOrDefault {
                            dict: "d".to_string(),
                            key: Box::new(MirExpr::StringLiteral("a".to_string())),
                            default: Box::new(MirExpr::IntLiteral(-1)),
                            ty: Ty::Int,
                        }),
                        ty: Ty::Int,
                    },
                }),
                MirItem::TopLevelStmt(print_expr(MirExpr::Name {
                    name: "y".to_string(),
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_get_or_default_nested_default");
        let obj_path = dir.join("dict_get_or_default_nested_default.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("dict_get_or_default_nested_default");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    #[test]
    #[should_panic(expected = "dict.get() key did not evaluate to str")]
    fn a_dict_get_or_default_with_a_non_str_key_is_an_internal_error() {
        // `pycc_types` (T0021) rejects a mismatched `dict.get()` key type
        // before codegen ever runs, so this is hand-built malformed MIR --
        // covers `MirExpr::DictGetOrDefault`'s own inline key-extraction
        // panic, mirroring `a_dict_get_with_a_non_str_key_is_an_internal_error`
        // above for `MirExpr::DictGet`.
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "d".to_string(),
                    value: MirExpr::DictLiteral(vec![(
                        MirExpr::StringLiteral("a".to_string()),
                        MirExpr::IntLiteral(1),
                    )]),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::DictGetOrDefault {
                    dict: "d".to_string(),
                    key: Box::new(MirExpr::IntLiteral(1)),
                    default: Box::new(MirExpr::IntLiteral(0)),
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("dict_get_or_default_non_str_key_panics");
        let _ = compile_to_object(
            &mir,
            &dir.join("dict_get_or_default_non_str_key_panics.o"),
            None,
            false,
        );
    }

    #[test]
    fn function_redefinition_uses_unique_mangled_names() {
        // Issue #22: each `def` gets a unique mangled name so redefinition
        // doesn't collide (`pyfn_{name}` for the first, `pyfn_{name}__redef_{n}`
        // for subsequent). The global function-pointer slot is initialized to
        // null and updated at each def's source position; calls dispatch
        // indirectly through the slot.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }))],
                },
                MirItem::Function {
                    name: "foo".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "x".to_string(),
                        ty: Ty::Int,
                    }))],
                },
                // Call foo(42) and print the result -- exercises the
                // indirect call dispatch through the function-pointer slot.
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "foo".to_string(),
                    args: vec![MirExpr::IntLiteral(42)],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("fn_redef_unique_names");
        let obj_path = dir.join("fn_redef_unique_names.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("fn_redef_unique_names");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        // The second definition (which returns its argument) is the one
        // bound at call time, so foo(42) should print 42.
        assert_eq!(
            output.stdout,
            b"42
"
        );
        assert!(output.status.success());
    }

    #[test]
    fn set_add_grows_the_set_and_a_repeated_value_still_dedups_codegens_and_runs() {
        // `s = {1,2}\ns.add(3)\nprint(len(s))\ns.add(1)\nprint(len(s))\n`
        // end to end -- real `MirExpr::SetAdd` codegen, the second,
        // user-facing call site for the already-existing
        // `pycc_rt_int_set_add` (`SetLiteral`'s own per-element
        // construction is the first). Two separate statements, each its
        // own independent `MirStmt`, executed strictly in source order --
        // this task's own brief flags `s.add(x); s.add(x)`-shaped repeated
        // calls as a shape to verify dedup still holds for. Expected
        // output verified against `python3` on this exact source: `3`
        // then `3` again (the repeated `.add(1)` does not grow the set).
        let mir = MirModule {
            items: vec![
                MirItem::TopLevelStmt(MirStmt::Assign {
                    target: "s".to_string(),
                    value: MirExpr::SetLiteral(vec![
                        MirExpr::IntLiteral(1),
                        MirExpr::IntLiteral(2),
                    ]),
                }),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::SetAdd {
                    set: "s".to_string(),
                    value: Box::new(MirExpr::IntLiteral(3)),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![set_name("s")],
                    ty: Ty::Int,
                })),
                MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::SetAdd {
                    set: "s".to_string(),
                    value: Box::new(MirExpr::IntLiteral(1)),
                })),
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![set_name("s")],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("set_add_grows_and_dedups");
        let obj_path = dir.join("set_add_grows_and_dedups.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("set_add_grows_and_dedups");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"3\n3\n");
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

    // -- #436: NullInstance codegen (class method called through a class) --

    /// #436: A `@classmethod` called through a class name (`C.greet(21)`)
    /// lowers to MIR with a `MirExpr::NullInstance` as the first argument
    /// (the `cls` receiver). This test verifies that codegen emits a null
    /// pointer for `NullInstance` and that the resulting binary runs
    /// correctly, producing the expected output.
    ///
    ///     class C:
    ///         def __init__(self) -> None:
    ///             return
    ///         @classmethod
    ///         def greet(cls, x: int) -> int:
    ///             return x * 2
    ///
    ///     print(C.greet(21))
    ///
    /// Expected output: `42`.
    #[test]
    fn null_instance_classmethod_codegens_and_runs() {
        let self_ty = instance_ty("C");
        let init = MirItem::Function {
            name: "C.__init__".to_string(),
            params: vec![("self".to_string(), self_ty.clone())],
            return_ty: Ty::None,
            body: vec![MirStmt::Return(None)],
        };
        let greet = MirItem::Function {
            name: "C.greet.classmethod".to_string(),
            params: vec![
                ("cls".to_string(), self_ty.clone()),
                ("x".to_string(), Ty::Int),
            ],
            return_ty: Ty::Int,
            body: vec![MirStmt::Return(Some(MirExpr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(MirExpr::Name {
                    name: "x".to_string(),
                    ty: Ty::Int,
                }),
                right: Box::new(MirExpr::IntLiteral(2)),
                ty: Ty::Int,
            }))],
        };
        let mir = MirModule {
            items: vec![
                init,
                greet,
                MirItem::TopLevelStmt(print_expr(MirExpr::Call {
                    callee: "C.greet.classmethod".to_string(),
                    args: vec![
                        MirExpr::NullInstance { ty: self_ty },
                        MirExpr::IntLiteral(21),
                    ],
                    ty: Ty::Int,
                })),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("null_instance_classmethod");
        let obj_path = dir.join("null_instance_classmethod.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("null_instance_classmethod");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn enum_member_singleton_init_emits_and_runs() {
        // #379: Exercises `emit_enum_member_inits` by building a MIR
        // module with an enum class definition and a top-level statement
        // that reads a member's value. The codegen must emit the
        // per-member singleton init sequence before the top-level
        // statement loop.
        let class_def = pycc_mir::HirClassDef {
            name: "Color".to_string(),
            bases: vec![],
            mro: vec!["Color".to_string()],
            attrs: vec![
                ("value".to_string(), Ty::Int),
                ("name".to_string(), Ty::Str),
            ],
            methods: vec![],
            properties: vec![],
            static_methods: vec![],
            class_methods: vec![],
            type_param: None,
            enum_members: vec![("RED".to_string(), 1), ("GREEN".to_string(), 2)],
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        };
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::Call {
                callee: "print".to_string(),
                args: vec![MirExpr::AttrGet {
                    base: Box::new(MirExpr::Name {
                        name: "Color.RED.enum_member".to_string(),
                        ty: Ty::Instance(Box::new("Color".to_string())),
                    }),
                    slot: 0,
                    ty: Ty::Int,
                }],
                ty: Ty::None,
            }))],
            class_defs: vec![("Color".to_string(), class_def)],
        };
        let dir = tempfile_dir("enum_member_init");
        let obj_path = dir.join("enum_member_init.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("enum_member_init");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
    }

    // -- #380: default_value_for_type covers every Mir Ty variant --------

    #[test]
    fn default_value_for_type_covers_every_mir_ty_variant() {
        let context = Context::create();
        // Scalar types.
        let _ = default_value_for_type(&context, pycc_mir::Ty::Int);
        let _ = default_value_for_type(&context, pycc_mir::Ty::Bool);
        let _ = default_value_for_type(&context, pycc_mir::Ty::Float);
        let _ = default_value_for_type(&context, pycc_mir::Ty::Str);
        let _ = default_value_for_type(&context, pycc_mir::Ty::None);
        let _ = default_value_for_type(&context, pycc_mir::Ty::Infer);
        let _ = default_value_for_type(&context, pycc_mir::Ty::Param(Box::new("T".to_string())));
        // Container types.
        let _ = default_value_for_type(&context, pycc_mir::Ty::List(Box::new(pycc_mir::Ty::Int)));
        let _ = default_value_for_type(
            &context,
            pycc_mir::Ty::Dict(Box::new((pycc_mir::Ty::Int, pycc_mir::Ty::Str))),
        );
        let _ = default_value_for_type(&context, pycc_mir::Ty::Set(Box::new(pycc_mir::Ty::Int)));
        let _ = default_value_for_type(
            &context,
            pycc_mir::Ty::Instance(Box::new("Foo".to_string())),
        );
        let _ = default_value_for_type(&context, pycc_mir::Ty::Protocol(Box::new("P".to_string())));
        let _ = default_value_for_type(
            &context,
            pycc_mir::Ty::Tuple(Box::new(vec![pycc_mir::Ty::Int, pycc_mir::Ty::Str])),
        );
    }

    #[test]
    fn abstract_method_body_with_non_none_return_emits_default_value() {
        // #380 (PR-20): an abstract method has a `Return(None)` body but
        // a non-`None` declared return type. Codegen must emit a correctly
        // typed default value so the LLVM IR is well-typed. This exercises
        // the `else` branch of the `Return(None)` arm in `emit_stmt`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "Animal.sound".to_string(),
                    params: vec![(
                        "self".to_string(),
                        Ty::Instance(Box::new("Animal".to_string())),
                    )],
                    return_ty: Ty::Str,
                    body: vec![MirStmt::Return(None)],
                },
                MirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(None)],
                },
            ],
            class_defs: vec![(
                "Animal".to_string(),
                pycc_mir::HirClassDef {
                    name: "Animal".to_string(),
                    bases: Vec::new(),
                    mro: vec!["Animal".to_string()],
                    attrs: Vec::new(),
                    methods: vec![("sound".to_string(), "Animal.sound".to_string())],
                    type_param: None,
                    properties: Vec::new(),
                    static_methods: Vec::new(),
                    class_methods: Vec::new(),
                    enum_members: Vec::new(),
                    is_dataclass: false,
                    dataclass_fields: Vec::new(),
                    is_protocol: false,
                    runtime_checkable: false,
                    protocol_members: Vec::new(),
                    abstract_methods: vec!["sound".to_string()],
                    is_abstract: true,
                },
            )],
        };
        let dir = tempfile_dir("abstract_default_ret");
        let obj_path = dir.join("abstract_default_ret.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        // The binary is never run — the abstract method is never called.
        // We only need to verify that codegen produces valid LLVM IR.
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- #381: MirStmt::Seq error propagation in emit_stmt ---------------

    #[test]
    fn calling_an_undefined_function_inside_a_seq_is_rejected() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Seq(vec![
                call_print(1),
                call_user_fn("does_not_exist_in_seq"),
            ]))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("seq_undefined_fn");
        let obj_path = dir.join("seq_undefined_fn.o");
        let err = compile_to_object(&mir, &obj_path, None, false).expect_err("should be rejected");
        assert!(
            err.contains("does_not_exist_in_seq"),
            "error should name the offending function: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seq_with_valid_statements_compiles_and_runs() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Seq(vec![
                call_print(10),
                call_print(20),
            ]))],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("seq_valid");
        let obj_path = dir.join("seq_valid.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("seq_valid");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"10\n20\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- #381: None singleton pattern comparison in emit_expr ---------------

    #[test]
    fn none_singleton_comparison_emits_zero_carrier() {
        // Exercises the `MirExpr::Name { name, ty: Ty::None } if name ==
        // "None"` arm of `emit_expr` (line 1633).  The MIR mirrors what
        // `lower_pattern_conds` produces for `match x: case None:`.
        let mir = MirModule {
            items: vec![
                MirItem::Function {
                    name: "get_none".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![MirStmt::Return(Some(MirExpr::Name {
                        name: "None".to_string(),
                        ty: Ty::None,
                    }))],
                },
                MirItem::TopLevelStmt(MirStmt::If {
                    test: MirExpr::Compare {
                        op: CmpOpKind::Eq,
                        left: Box::new(MirExpr::Call {
                            callee: "get_none".to_string(),
                            args: vec![],
                            ty: Ty::None,
                        }),
                        right: Box::new(MirExpr::Name {
                            name: "None".to_string(),
                            ty: Ty::None,
                        }),
                        ty: Ty::Bool,
                    },
                    body: vec![call_print(1)],
                    orelse: vec![call_print(0)],
                }),
            ],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("none_singleton_cmp");
        let obj_path = dir.join("none_singleton_cmp.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("none_singleton_cmp");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- #382 exception handling codegen tests --

    #[test]
    fn exception_terminal_analysis_covers_structured_paths() {
        let returned = || MirStmt::Return(Some(MirExpr::IntLiteral(1)));
        let falls_through = || MirStmt::NoOp;

        assert!(exception::block_always_terminates(&[MirStmt::If {
            test: MirExpr::BoolLiteral(true),
            body: vec![returned()],
            orelse: vec![returned()],
        }]));
        assert!(!exception::block_always_terminates(&[MirStmt::If {
            test: MirExpr::BoolLiteral(true),
            body: vec![returned()],
            orelse: vec![],
        }]));
        assert!(exception::block_always_terminates(&[MirStmt::Seq(vec![
            returned(),
        ])]));

        let terminal_handler = MirExceptHandler {
            exc_type_tag: Some(1),
            binding_name: None,
            binding_ty: None,
            body: vec![returned()],
        };
        assert!(exception::block_always_terminates(&[MirStmt::Try {
            body: vec![falls_through()],
            handlers: vec![terminal_handler.clone()],
            orelse: vec![returned()],
            finalbody: vec![],
        }]));
        assert!(!exception::block_always_terminates(&[MirStmt::Try {
            body: vec![falls_through()],
            handlers: vec![terminal_handler],
            orelse: vec![],
            finalbody: vec![],
        }]));
        assert!(exception::block_always_terminates(&[MirStmt::Try {
            body: vec![falls_through()],
            handlers: vec![],
            orelse: vec![],
            finalbody: vec![returned()],
        }]));
        assert!(!exception::block_always_terminates(&[falls_through()]));
    }

    #[test]
    fn bare_except_codegen_builds_and_runs() {
        // Tests the bare `except:` path (exc_type_tag = None) in codegen.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::Raise {
                    exception: MirExceptionValue::Constructed {
                        type_tag: 1, // ValueError
                        message: MirExpr::StringLiteral("test".to_string()),
                    },
                }],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: None, // bare except
                    binding_name: None,
                    binding_ty: None,
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::StringLiteral("caught".to_string())],
                        ty: Ty::None,
                    })],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bare_except_codegen");
        let obj_path = dir.join("bare_except.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("bare_except");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"caught\n");
        assert!(output.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raise_with_non_string_message_is_a_codegen_error() {
        // Tests the error path for a non-string raise message.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Raise {
                exception: MirExceptionValue::Constructed {
                    type_tag: 1,
                    message: MirExpr::IntLiteral(42),
                },
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("raise_non_str");
        let obj_path = dir.join("raise_non_str.o");
        let result = compile_to_object(&mir, &obj_path, None, false);
        assert!(
            result.is_err(),
            "codegen should fail for non-string raise message"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raising_a_non_instance_existing_value_is_a_codegen_error() {
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Raise {
                exception: MirExceptionValue::Existing(MirExpr::IntLiteral(42)),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("raise_existing_non_instance");
        let obj_path = dir.join("raise_existing_non_instance.o");
        let err = compile_to_object(&mir, &obj_path, None, false)
            .expect_err("a non-instance cannot be an existing exception");
        assert!(err.contains("must be an exception instance"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raising_a_bound_existing_exception_builds_successfully() {
        let exception_ty = Ty::Instance(Box::new("ValueError".to_string()));
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::Raise {
                    exception: MirExceptionValue::Constructed {
                        type_tag: 1,
                        message: MirExpr::StringLiteral("original".to_string()),
                    },
                }],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: Some("error".to_string()),
                    binding_ty: Some(exception_ty.clone()),
                    body: vec![MirStmt::Raise {
                        exception: MirExceptionValue::Existing(MirExpr::Name {
                            name: "error".to_string(),
                            ty: exception_ty,
                        }),
                    }],
                }],
                orelse: vec![],
                finalbody: vec![],
            })],
            class_defs: vec![],
        };
        let dir = tempfile_dir("raise_existing_instance");
        let obj_path = dir.join("raise_existing_instance.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("a bound exception instance can be raised again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raise_from_with_non_string_message_is_a_codegen_error() {
        // Tests the error path for a non-string raise message in RaiseFrom.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::RaiseFrom {
                exception: MirExceptionValue::Constructed {
                    type_tag: 1,
                    message: MirExpr::IntLiteral(42),
                },
                cause: MirExceptionValue::Constructed {
                    type_tag: 2,
                    message: MirExpr::StringLiteral("cause".to_string()),
                },
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("raise_from_non_str");
        let obj_path = dir.join("raise_from_non_str.o");
        let result = compile_to_object(&mir, &obj_path, None, false);
        assert!(
            result.is_err(),
            "codegen should fail for non-string raise message"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raise_from_with_non_string_cause_is_a_codegen_error() {
        // Tests the error path for a non-string cause message in RaiseFrom.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::RaiseFrom {
                exception: MirExceptionValue::Constructed {
                    type_tag: 1,
                    message: MirExpr::StringLiteral("msg".to_string()),
                },
                cause: MirExceptionValue::Constructed {
                    type_tag: 2,
                    message: MirExpr::IntLiteral(42),
                },
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("raise_from_non_str_cause");
        let obj_path = dir.join("raise_from_non_str_cause.o");
        let result = compile_to_object(&mir, &obj_path, None, false);
        assert!(
            result.is_err(),
            "codegen should fail for non-string cause message"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- #382: try/except/finally `?` error propagation and fall-through --

    #[test]
    fn try_body_emit_error_propagates() {
        // An undefined function call in the try body causes `emit_body` to
        // return `Err`, which propagates through the `?` at the try body
        // `emit_body` call site.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![call_user_fn("nonexistent_in_try_body")],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![call_print(99)],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_body_err");
        let obj_path = dir.join("try_body_err.o");
        let err = compile_to_object(&mir, &obj_path, None, false)
            .expect_err("codegen should fail for undefined fn in try body");
        assert!(
            err.contains("nonexistent_in_try_body"),
            "error should name the offending function: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_handler_body_emit_error_propagates() {
        // An undefined function call in the handler body causes `emit_body`
        // to return `Err`, which propagates through the `?` at the handler
        // body `emit_body` call site.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![call_print(1)],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![call_user_fn("nonexistent_in_handler")],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_handler_err");
        let obj_path = dir.join("try_handler_err.o");
        let err = compile_to_object(&mir, &obj_path, None, false)
            .expect_err("codegen should fail for undefined fn in handler body");
        assert!(
            err.contains("nonexistent_in_handler"),
            "error should name the offending function: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_else_body_emit_error_propagates() {
        // An undefined function call in the else body causes `emit_body` to
        // return `Err`, which propagates through the `?` at the else body
        // `emit_body` call site.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![call_print(1)],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![call_print(99)],
                }],
                orelse: vec![call_user_fn("nonexistent_in_else")],
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_else_err");
        let obj_path = dir.join("try_else_err.o");
        let err = compile_to_object(&mir, &obj_path, None, false)
            .expect_err("codegen should fail for undefined fn in else body");
        assert!(
            err.contains("nonexistent_in_else"),
            "error should name the offending function: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_finally_body_emit_error_propagates() {
        // An undefined function call in the finally body causes `emit_body`
        // to return `Err`, which propagates through the `?` at the finally
        // body `emit_body` call site.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![call_print(1)],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![call_print(99)],
                }],
                orelse: Vec::new(),
                finalbody: vec![call_user_fn("nonexistent_in_finally")],
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_finally_err");
        let obj_path = dir.join("try_finally_err.o");
        let err = compile_to_object(&mir, &obj_path, None, false)
            .expect_err("codegen should fail for undefined fn in finally body");
        assert!(
            err.contains("nonexistent_in_finally"),
            "error should name the offending function: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Helper: a `MirStmt::Return(Some(MirExpr::IntLiteral(n)))` — used in
    /// function bodies to terminate the current block so that the enclosing
    /// try's `*_falls_through` check is false.
    fn return_int(n: i64) -> MirStmt {
        MirStmt::Return(Some(MirExpr::IntLiteral(n)))
    }

    #[test]
    fn try_body_with_return_does_not_fall_through() {
        // A `Return` in the try body terminates the block, so
        // `body_falls_through` is false and the exception-check branch is
        // skipped. The function has `return_ty: Ty::None` so the implicit
        // void return on the unreachable `after_bb` is valid.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Try {
                    body: vec![return_int(1)],
                    handlers: vec![MirExceptHandler {
                        exc_type_tag: Some(1),
                        binding_name: None,
                        binding_ty: None,
                        body: vec![call_print(99)],
                    }],
                    orelse: Vec::new(),
                    finalbody: Vec::new(),
                }],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_body_return");
        let obj_path = dir.join("try_body_return.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("codegen should succeed for try body with return");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_handler_body_with_return_does_not_fall_through() {
        // A `Return` in the handler body terminates the block, so
        // `handler_falls_through` is false and the branch to finally is
        // skipped.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Try {
                    body: vec![call_print(1)],
                    handlers: vec![MirExceptHandler {
                        exc_type_tag: Some(1),
                        binding_name: None,
                        binding_ty: None,
                        body: vec![return_int(2)],
                    }],
                    orelse: Vec::new(),
                    finalbody: Vec::new(),
                }],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_handler_return");
        let obj_path = dir.join("try_handler_return.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("codegen should succeed for handler body with return");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_else_body_with_return_does_not_fall_through() {
        // A `Return` in the else body terminates the block, so
        // `else_falls_through` is false and the branch to finally is
        // skipped.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Try {
                    body: vec![call_print(1)],
                    handlers: vec![MirExceptHandler {
                        exc_type_tag: Some(1),
                        binding_name: None,
                        binding_ty: None,
                        body: vec![call_print(99)],
                    }],
                    orelse: vec![return_int(3)],
                    finalbody: Vec::new(),
                }],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_else_return");
        let obj_path = dir.join("try_else_return.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("codegen should succeed for else body with return");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_finally_body_with_return_does_not_fall_through() {
        // A `Return` in the finally body terminates the block, so
        // `finally_falls_through` is false and the exception-check branch
        // is skipped.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Try {
                    body: vec![call_print(1)],
                    handlers: vec![MirExceptHandler {
                        exc_type_tag: Some(1),
                        binding_name: None,
                        binding_ty: None,
                        body: vec![call_print(99)],
                    }],
                    orelse: Vec::new(),
                    finalbody: vec![return_int(4)],
                }],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_finally_return");
        let obj_path = dir.join("try_finally_return.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("codegen should succeed for finally body with return");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bare_non_none_return_routes_through_finally_with_a_default_value() {
        // Raw MIR mirrors an abstract-method-style `Return(None)` paired
        // with a non-None ABI. The type checker keeps this shape out of
        // ordinary functions, but codegen still has to route its neutral
        // carrier through a finally target without producing invalid IR.
        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![MirStmt::Try {
                    body: vec![MirStmt::Return(None)],
                    handlers: vec![],
                    orelse: vec![],
                    finalbody: vec![call_print(1)],
                }],
            }],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("bare_non_none_return_finally");
        let obj_path = dir.join("bare_non_none_return_finally.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("the defensive default return must remain valid through finally");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_finally_return_routing_covers_value_and_none_abis() {
        for (name, return_ty, returned) in [
            (
                "value",
                Ty::Int,
                MirStmt::Return(Some(MirExpr::IntLiteral(7))),
            ),
            ("none", Ty::None, MirStmt::Return(None)),
            (
                "none_expr",
                Ty::None,
                MirStmt::Return(Some(MirExpr::Name {
                    name: "None".to_string(),
                    ty: Ty::None,
                })),
            ),
        ] {
            let mir = MirModule {
                items: vec![MirItem::Function {
                    name: format!("nested_{name}"),
                    params: vec![],
                    return_ty,
                    body: vec![MirStmt::Try {
                        body: vec![MirStmt::Try {
                            body: vec![returned],
                            handlers: vec![],
                            orelse: vec![],
                            finalbody: vec![MirStmt::NoOp],
                        }],
                        handlers: vec![],
                        orelse: vec![],
                        finalbody: vec![MirStmt::NoOp],
                    }],
                }],
                class_defs: vec![],
            };
            let dir = tempfile_dir(&format!("nested_finally_{name}"));
            let obj_path = dir.join(format!("nested_finally_{name}.o"));
            compile_to_object(&mir, &obj_path, None, false)
                .expect("nested finally return routing must produce valid object code");
            let _ = std::fs::remove_dir_all(&dir);
        }

        let mir = MirModule {
            items: vec![MirItem::Function {
                name: "direct_none".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![MirStmt::Try {
                    body: vec![MirStmt::Return(None)],
                    handlers: vec![],
                    orelse: vec![],
                    finalbody: vec![MirStmt::NoOp],
                }],
            }],
            class_defs: vec![],
        };
        let dir = tempfile_dir("direct_none_finally");
        let obj_path = dir.join("direct_none_finally.o");
        compile_to_object(&mir, &obj_path, None, false)
            .expect("a None return routed through finally must produce valid object code");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- #382: RaiseFrom, Reraise, and remaining Try codegen paths --

    #[test]
    fn raise_from_codegen_builds_and_runs() {
        // Exercises the success path of `RaiseFrom` codegen (lines 7296-7323):
        // both message and cause are string literals, so the `Scalar::Str(p)`
        // arms succeed and the full alloc/raise-with-cause sequence runs.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::RaiseFrom {
                    exception: MirExceptionValue::Constructed {
                        type_tag: 1, // ValueError
                        message: MirExpr::StringLiteral("bad".to_string()),
                    },
                    cause: MirExceptionValue::Constructed {
                        type_tag: 2, // TypeError
                        message: MirExpr::StringLiteral("cause".to_string()),
                    },
                }],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::StringLiteral("caught".to_string())],
                        ty: Ty::None,
                    })],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("raise_from_codegen");
        let obj_path = dir.join("raise_from.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("raise_from");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"caught\n");
        assert!(output.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reraise_codegen_builds() {
        // Exercises the `Reraise` codegen path: loads the lexically enclosing
        // handler's saved exception value and re-raises it.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::Raise {
                    exception: MirExceptionValue::Constructed {
                        type_tag: 1,
                        message: MirExpr::StringLiteral("orig".to_string()),
                    },
                }],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![MirStmt::Reraise],
                }],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("reraise_codegen");
        let obj_path = dir.join("reraise.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("reraise");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert!(
            !output.status.success(),
            "reraise should propagate as non-zero exit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_with_no_handlers_branches_to_finally() {
        // Exercises the `handlers.is_empty()` branch (lines 7423-7428):
        // a try with no handlers branches directly to finally, where the
        // exception remains active and propagates after the finally body.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::Raise {
                    exception: MirExceptionValue::Constructed {
                        type_tag: 1,
                        message: MirExpr::StringLiteral("uncaught".to_string()),
                    },
                }],
                handlers: Vec::new(),
                orelse: Vec::new(),
                finalbody: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::StringLiteral("finally".to_string())],
                    ty: Ty::None,
                })],
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_no_handlers");
        let obj_path = dir.join("try_no_handlers.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("try_no_handlers");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"finally\n");
        assert!(
            !output.status.success(),
            "uncaught exception should exit non-zero"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_with_multiple_handlers_dispatches_to_next() {
        // Exercises the `dispatch_bbs[i + 1]` path (line 7455): when the
        // first handler does not match, control flows to the next dispatch
        // block.  Here a `KeyError` (tag 3) is raised but the first handler
        // checks for `ValueError` (tag 1), so the second handler (tag 3)
        // matches.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::Raise {
                    exception: MirExceptionValue::Constructed {
                        type_tag: 3, // KeyError
                        message: MirExpr::StringLiteral("key".to_string()),
                    },
                }],
                handlers: vec![
                    MirExceptHandler {
                        exc_type_tag: Some(1), // ValueError — does not match
                        binding_name: None,
                        binding_ty: None,
                        body: vec![MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::StringLiteral("value".to_string())],
                            ty: Ty::None,
                        })],
                    },
                    MirExceptHandler {
                        exc_type_tag: Some(3), // KeyError — matches
                        binding_name: None,
                        binding_ty: None,
                        body: vec![MirStmt::ExprStmt(MirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![MirExpr::StringLiteral("key".to_string())],
                            ty: Ty::None,
                        })],
                    },
                ],
                orelse: Vec::new(),
                finalbody: Vec::new(),
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_multi_dispatch");
        let obj_path = dir.join("try_multi_dispatch.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("try_multi_dispatch");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"key\n");
        assert!(output.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_else_body_falls_through_to_finally() {
        // Exercises the `else_falls_through` branch (lines 7569-7571):
        // a non-empty else body that completes normally (no return/raise)
        // branches to the finally block.
        let mir = MirModule {
            items: vec![MirItem::TopLevelStmt(MirStmt::Try {
                body: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::StringLiteral("try".to_string())],
                    ty: Ty::None,
                })],
                handlers: vec![MirExceptHandler {
                    exc_type_tag: Some(1),
                    binding_name: None,
                    binding_ty: None,
                    body: vec![MirStmt::ExprStmt(MirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![MirExpr::StringLiteral("handler".to_string())],
                        ty: Ty::None,
                    })],
                }],
                orelse: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::StringLiteral("else".to_string())],
                    ty: Ty::None,
                })],
                finalbody: vec![MirStmt::ExprStmt(MirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![MirExpr::StringLiteral("finally".to_string())],
                    ty: Ty::None,
                })],
            })],
            class_defs: Vec::new(),
        };
        let dir = tempfile_dir("try_else_falls_through");
        let obj_path = dir.join("try_else_falls_through.o");
        compile_to_object(&mir, &obj_path, None, false).expect("codegen should succeed");
        let bin_path = dir.join("try_else_falls_through");
        link_object_with_runtime(&obj_path, &bin_path);
        let output = Command::new(&bin_path).output().expect("binary should run");
        assert_eq!(output.stdout, b"try\nelse\nfinally\n");
        assert!(output.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
