//! Declarations of every `pycc_rt` runtime function this crate calls.
//!
//! Cohesion-driven carve out of `lib.rs` under AGENTS.md's decomposability
//! rule (issue #545 Part 2): the `RtFns` handle struct and the single routine
//! that populates it, relocated unchanged. Nothing else is moved.

use super::*;

/// Every `pycc_rt` function this crate calls, declared once in
/// `compile_to_object` and threaded through `emit_stmt`/`emit_expr`.
/// Extended (never replaced) by Tasks 8/9/10 as they add more `pycc_rt`
/// declarations.
pub(super) struct RtFns<'ctx> {
    pub(super) int_from_i64: FunctionValue<'ctx>,
    pub(super) int_add: FunctionValue<'ctx>,
    pub(super) int_sub: FunctionValue<'ctx>,
    pub(super) int_mul: FunctionValue<'ctx>,
    pub(super) int_floordiv: FunctionValue<'ctx>,
    pub(super) int_floormod: FunctionValue<'ctx>,
    pub(super) int_pow: FunctionValue<'ctx>,
    pub(super) int_cmp: FunctionValue<'ctx>,
    pub(super) int_truthy: FunctionValue<'ctx>,
    pub(super) range_continue: FunctionValue<'ctx>,
    /// #147 (D-179): `pycc_rt_range_normalize_operand`, the encoded-word-in,
    /// encoded-word-out normalizer every `range()` operand passes through.
    /// It maps D-141's `False`/`True` markers to the ordinary smallints
    /// `0`/`1` (a range produces integer objects rather than forwarding its
    /// argument object) and passes a smallint or a heap bigint through
    /// unchanged. It replaces the `int_untag_checked` call this position
    /// used to emit, which rejected every bigint.
    pub(super) range_normalize_operand: FunctionValue<'ctx>,
    pub(super) int_to_float: FunctionValue<'ctx>,
    pub(super) float_div: FunctionValue<'ctx>,
    pub(super) float_floordiv: FunctionValue<'ctx>,
    pub(super) float_floormod: FunctionValue<'ctx>,
    pub(super) float_pow: FunctionValue<'ctx>,
    pub(super) str_from_literal: FunctionValue<'ctx>,
    pub(super) str_concat: FunctionValue<'ctx>,
    /// #575 (Part 2 of #123): `pycc_rt_str_repeat`, the `str * int` /
    /// `int * str` primitive. Declared beside `str_concat` because the
    /// two share the same shape apart from the count: one `str` pointer
    /// in, one brand-new `str` pointer out. The count parameter is a
    /// **raw** `i64`, already decoded by `build_untag_checked`, matching
    /// every other raw runtime counter this table declares (list index,
    /// slice bound) rather than pushing D-141's classifier into a second
    /// place. `range` operands left that group in #147 (D-179): they now
    /// stay encoded and go through `range_normalize_operand`.
    pub(super) str_repeat: FunctionValue<'ctx>,
    pub(super) str_cmp: FunctionValue<'ctx>,
    pub(super) str_truthy: FunctionValue<'ctx>,
    pub(super) str_incref: FunctionValue<'ctx>,
    pub(super) str_decref: FunctionValue<'ctx>,
    /// #146 Part 1: `str`'s D-060 refcounting pair, generalized to D-141's
    /// encoded `int` word. Both take the raw `i64` word rather than a
    /// pointer, and both are only ever called under the inline
    /// `(word & 3) == 0 && word != 0` guard `emit_bigint_refcount_call`
    /// wraps them in, so a smallint loop never reaches a runtime call.
    pub(super) bigint_retain: FunctionValue<'ctx>,
    pub(super) bigint_release: FunctionValue<'ctx>,
    pub(super) int_to_str: FunctionValue<'ctx>,
    pub(super) float_to_str: FunctionValue<'ctx>,
    pub(super) bool_to_str: FunctionValue<'ctx>,
    pub(super) print_write_str: FunctionValue<'ctx>,
    pub(super) print_space: FunctionValue<'ctx>,
    pub(super) print_newline: FunctionValue<'ctx>,
    pub(super) print_none: FunctionValue<'ctx>,
    /// D-141's checked numeric decoder. Container ingress calls it for
    /// validation and keeps the original encoded word; index, slice, and
    /// `str`-repeat-count sites use its raw `0`/`1`/smallint result as an
    /// implementation counter. `range` operands no longer call it at all
    /// since #147 (D-179) -- see `range_normalize_operand`. It rejects
    /// bigint and malformed words in `pycc_rt`, which owns the
    /// representation classifier.
    pub(super) int_untag_checked: FunctionValue<'ctx>,
    pub(super) int_list_new: FunctionValue<'ctx>,
    pub(super) int_list_append: FunctionValue<'ctx>,
    pub(super) int_list_get: FunctionValue<'ctx>,
    pub(super) int_list_len: FunctionValue<'ctx>,
    /// PR-12 Task 9's own new `pycc_rt_int_list_slice` declaration
    /// (`base[start:stop:step]`, D-118) -- takes the `list` pointer plus
    /// three already-defaulted, already-untagged raw `i64` bounds and
    /// returns a **new** `PyIntListObj` pointer, exactly the same opaque
    /// pointer type every other `PyIntListObj`-returning function above
    /// already declares (`int_list_new`'s own `ptr_type.fn_type(&[], ..)`
    /// one line below shares that same return type).
    pub(super) int_list_slice: FunctionValue<'ctx>,
    /// PR-12 Task 11's own new `pycc_rt_int_list_pop` declaration
    /// (`list.pop()`, D-119) -- mirrors `int_list_get`'s own shape exactly
    /// (one `ptr_type` parameter, one encoded-element `i64_type` return),
    /// the only difference being no `index` parameter, since
    /// `.pop()` always removes the list's own last element.
    pub(super) int_list_pop: FunctionValue<'ctx>,
    /// PR-11 Task 5's own new `pycc_rt_dict_*` declarations, mirroring the
    /// `int_list_*` cluster immediately above one-for-one: `dict_new` (no
    /// pre-sizing entry point, same reasoning as `int_list_new`),
    /// `dict_set` (insert-or-update, D-123 -- this crate's one dict
    /// "growable op", playing `int_list_append`'s role), `dict_get`
    /// (read, panics on a missing key), `dict_len`, and `dict_key_at`
    /// (`ForDict`'s own per-iteration key read, playing `int_list_get`'s
    /// `ForList`-iteration role).
    pub(super) dict_new: FunctionValue<'ctx>,
    pub(super) dict_set: FunctionValue<'ctx>,
    pub(super) dict_get: FunctionValue<'ctx>,
    /// PR-12 Task 11's own new `pycc_rt_dict_get_or_default` declaration
    /// (`dict.get(key, default)`, D-119) -- mirrors `dict_get`'s own shape
    /// plus one extra encoded-value `i64` `default` parameter, matching
    /// `pycc_rt_dict_get_or_default`'s real Rust signature
    /// (`fn(*mut PyDictObj, *mut PyStrObj, i64) -> i64`) exactly.
    pub(super) dict_get_or_default: FunctionValue<'ctx>,
    pub(super) dict_len: FunctionValue<'ctx>,
    pub(super) dict_key_at: FunctionValue<'ctx>,
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
    pub(super) int_set_new: FunctionValue<'ctx>,
    pub(super) int_set_add: FunctionValue<'ctx>,
    pub(super) int_set_len: FunctionValue<'ctx>,
    pub(super) int_set_get: FunctionValue<'ctx>,
    /// `ForSet`'s own loop-test check (Task 11 review fix, P1): panics if
    /// the set's freshly re-read length no longer matches the length
    /// captured once in the loop's preheader -- see
    /// `pycc_rt_int_set_check_not_resized`'s own doc comment for why
    /// `set.add()` made this reachable.
    pub(super) int_set_check_not_resized: FunctionValue<'ctx>,
    pub(super) trap: FunctionValue<'ctx>,
    /// D-154 (Part 1 of #375): `pycc_rt::instance`'s own three-function
    /// cluster -- `instance_new` (allocates a fresh, zero-initialized
    /// instance with the class's own declared slot count), and
    /// `instance_get_slot`/`instance_set_slot` (the class-instance-layout
    /// ADR's opaque accessor pair; codegen never `GEP`s into a
    /// `PyInstanceObj` directly).
    pub(super) instance_new: FunctionValue<'ctx>,
    pub(super) instance_get_slot: FunctionValue<'ctx>,
    pub(super) instance_set_slot: FunctionValue<'ctx>,
    /// Issue #22: runtime NameError for call-before-`def`. Takes a
    /// null-terminated C string (the function name) and panics -- which
    /// becomes a process abort at the `extern "C"` boundary, matching every
    /// other runtime error in `pycc_rt`.
    pub(super) name_error: FunctionValue<'ctx>,
    /// #382 (PR-22 Part 1): Exception runtime functions.
    /// `exception_active` returns i8 (non-zero if an exception is pending).
    pub(super) exception_active: FunctionValue<'ctx>,
    /// `exception_value` returns a pointer to the current exception object.
    pub(super) exception_value: FunctionValue<'ctx>,
    /// `exception_clear` resets the thread-local pending state (void).
    pub(super) exception_clear: FunctionValue<'ctx>,
    /// `exception_alloc(type_tag: u8, name: *const u8, name_len: usize,
    /// message: *mut PyStrObj) -> *mut PyExceptionObj`. Part 2 of #541
    /// (D-189) added the class name: user-defined exception classes carry
    /// module-assigned tags the runtime cannot map back to a name.
    pub(super) exception_alloc: FunctionValue<'ctx>,
    /// `exception_raise(obj: *mut PyExceptionObj)` — sets pending state (void).
    pub(super) exception_raise: FunctionValue<'ctx>,
    /// `exception_raise_with_cause(obj, cause: *mut PyExceptionObj)` (void).
    pub(super) exception_raise_with_cause: FunctionValue<'ctx>,
    /// `exception_type_matches(obj: *mut PyExceptionObj, type_tag: u8) -> i8`.
    pub(super) exception_type_matches: FunctionValue<'ctx>,
    /// `exception_message(obj: *mut PyExceptionObj) -> *mut PyStrObj` (Part 3A
    /// of #541, #736): the exception's own message alone (`str(e)`
    /// semantics), never `exception_print_and_exit`'s `"{type}: {message}"`
    /// uncaught-exception format.
    pub(super) exception_message: FunctionValue<'ctx>,
    /// `exception_print_and_exit(obj: *mut PyExceptionObj) -> !` (noreturn).
    pub(super) exception_print_and_exit: FunctionValue<'ctx>,
    /// Lexically enclosing `except` handler values used by bare `raise`.
    /// Each handler owns an LLVM local slot. A stack is required because a
    /// nested handler must not overwrite the exception saved by its outer
    /// handler.
    pub(super) exceptions: ExceptionCodegenState<'ctx>,
}

pub(super) fn declare_rt_functions<'ctx>(
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
            ptr_type.fn_type(
                &[
                    context.i8_type().into(),
                    ptr_type.into(),
                    context.i64_type().into(),
                    ptr_type.into(),
                ],
                false,
            ),
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
        exception_message: declare(
            "pycc_rt_exception_message",
            ptr_type.fn_type(&[ptr_type.into()], false),
        ),
        exception_print_and_exit: declare(
            "pycc_rt_exception_print_and_exit",
            void_type.fn_type(&[ptr_type.into()], false),
        ),
        exceptions: ExceptionCodegenState::new(),
    }
}
