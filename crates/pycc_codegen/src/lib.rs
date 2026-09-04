use inkwell::FloatPredicate;
use inkwell::IntPredicate;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::FileType;
use inkwell::types::BasicType;
use inkwell::values::{FloatValue, FunctionValue, IntValue, PointerValue};
use pycc_mir::{
    CompSource, EXCEPTION_GROUP_TYPE_TAG, MirExceptionValue, MirExpr, MirItem, MirModule, MirStmt,
};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

mod exception;
use exception::{
    ExceptionCodegenState, emit_exception_set_frame, emit_exception_value,
    expression_can_set_exception, guard_statement_effects,
};
mod bigint_rc;
use bigint_rc::{
    BigIntRefcount, emit_bigint_refcount_call, int_temporary_word, pop_pending_int_release,
    push_pending_int_release_if_scalar_temporary, push_pending_int_release_if_temporary,
    release_if_int_temporary, release_int_slot_before_store,
    release_optional_int_slot_before_store, release_scalar_if_int_temporary,
    retain_if_int_duplicate, retain_if_int_duplicate_and_track_for_exception_edge,
};
mod int_const;
use int_const::{emit_int_constant, tag_smallint_const};
mod exception_render;
use exception_render::emit_exception_message;
mod rt_fns;
use rt_fns::{RtFns, declare_rt_functions};
mod target_machine;
#[cfg(test)]
mod tests;
pub use pycc_artifact_layout as artifact_layout;

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
    /// `T | None` (PEP 604, D-197, #763, Part 1 of #747). An LLVM `struct`
    /// held **by value** -- an SSA aggregate register, exactly like `Tuple`
    /// immediately above and for the identical reason (D-115's reasoning,
    /// extended): `{ inner, i8 }` (payload, present-flag), the explicit
    /// present/absent tag `ty_to_basic_type`'s own `Ty::Optional` arm
    /// documents (not a niche sentinel -- see that arm's doc comment for
    /// why). Only `Optional[int]` is ever constructed by real,
    /// type-checked source in this PR (`pycc_types`' `T0049` gate rejects
    /// every other inner type pre-MIR-lowering, mirroring `list[int]`'s own
    /// `T0034` gate), but this variant's own shape does not assume that --
    /// matching `Tuple`'s own not-narrowed-to-what's-reachable-today
    /// precedent immediately above.
    ///
    /// Every exhaustive `Scalar` match this variant did not already need to
    /// answer for (`truthy`/`to_str`/`to_numeric_encoded_int`/`to_float`) is
    /// a defensive, provably-unreachable-by-real-source panic, for the same
    /// reason `Tuple`'s own arms in those functions are: `pycc_types`
    /// accepts neither printing, truthiness-testing, nor arithmetic on an
    /// `Optional`-typed value in this PR (only construction, `is`/`is not`,
    /// and narrowed-and-then-unwrapped use, which produces a *different*,
    /// non-`Optional` `Ty` once narrowed) -- see this PR's ADR.
    Optional(inkwell::values::StructValue<'ctx>),
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
        // `T | None` (PEP 604, D-197, #763, Part 1 of #747). Only
        // `Optional[int]` reaches this arm from real, codegen-eligible
        // source today (`pycc_types`' `T0049` gate rejects every other
        // inner type before MIR-lowering, mirroring `list[int]`'s own
        // `T0034` gate) -- but this representation function is deliberately
        // element-type-agnostic here too, matching `Tuple`'s own recursive,
        // not-narrowed-to-what's-reachable-today shape immediately above.
        //
        // Representation: `{ inner, i8 }` (payload, present-flag) -- an
        // *explicit* present/absent tag, not a niche sentinel bit pattern.
        // A sentinel scheme was the original hypothesis (see this PR's ADR),
        // but `Ty::Int`'s own representation just above is a plain LLVM
        // `i64` occupying its full 64-bit range (this crate's separate
        // tagged smallint/bool-marker/bigint-pointer encoding in
        // `bigint_rc.rs` is a *different* representation, used only for
        // polymorphic container storage, not for a scalar `int`-typed local
        // or parameter) -- there is no bit pattern of a plain `i64` that is
        // guaranteed to never be a legitimate `int` value, so no sentinel is
        // safe. An explicit flag is therefore correct where `Optional[int]`
        // would otherwise be described as "niche-optimized."
        pycc_mir::Ty::Optional(inner) => {
            let payload_ty = ty_to_basic_type(context, (*inner).clone());
            context
                .struct_type(&[payload_ty, context.i8_type().into()], false)
                .into()
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
        // `Optional[_]` (D-197, #763; widened to `float`/`bool` payloads by
        // #809): an absent (`present == 0`) `{ inner, i8 }` struct, matching
        // every other container arm above for the `present`/tag field --
        // but field 0 (the payload) is NOT unconditionally a raw zero word
        // the way `struct_ty.const_zero()` would give it. This function's
        // two call sites are not both the "never executed" case its
        // neighbours are: the abstract-method `Return(None)` site truly
        // never runs the returned value, but the *other* call site (this
        // function's exceptional-exit default, `emit_function`'s own `if
        // *return_ty == pycc_mir::Ty::None` sibling) IS reached by a real,
        // executing `int | None`-returning function that raises mid-body --
        // the same invariant `coerce_scalar_to_type`'s own
        // placeholder-building arm documents (an `Optional`'s payload field
        // must always be a *valid* payload for its inner type regardless of
        // the present flag, because `truthy`'s branch-free AND reads it
        // unconditionally for `Ty::Int`) applies here too. For `Ty::Int`
        // specifically, a raw `0` word would trip `classify_encoded_int`'s
        // fail-closed panic the moment any caller's own branch-free code
        // touches this exceptional return value's payload field before ever
        // consulting the exception flag, so that case keeps using
        // `tag_smallint_const` to encode `0` exactly as `to_encoded_int`
        // would. `Ty::Float`/`Ty::Bool` carry no analogous tagged encoding
        // (their `truthy` arms below branch on the `Ty::Optional` shape
        // explicitly rather than reading a D-141-encoded int), so a plain
        // per-type zero from `default_value_for_type` itself is a valid
        // placeholder for them.
        pycc_mir::Ty::Optional(ref inner) => {
            let struct_ty = ty_to_basic_type(context, ty.clone()).into_struct_type();
            let payload = match inner.as_ref() {
                pycc_mir::Ty::Int => tag_smallint_const(context, 0).into(),
                other_inner => default_value_for_type(context, other_inner.clone()),
            };
            struct_ty
                .const_named_struct(&[payload, context.i8_type().const_zero().into()])
                .into()
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
        // Defensive for the identical `numeric_result_type` reason as the
        // arms above, extended to `Optional[int]` (D-197, #763, Part 1 of
        // #747, extending D-107/D-124's reasoning): `as_numeric` maps no
        // `Ty::Optional` to a numeric type either, so any arithmetic with
        // an `Optional[int]` operand is already rejected as `T0021` before
        // codegen runs -- unwrapping an `Optional[int]` to use its payload
        // numerically requires narrowing (Part 2+ of #747), which produces
        // a plain `Ty::Int` value, not this type.
        Scalar::Optional(_) => {
            panic!("pycc_codegen: internal error: expected an int-or-bool operand, got optional")
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
/// `len` (see the crate's `tests` module for both).
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
        | Scalar::Instance(_)
        // D-197, #763, Part 1 of #747: an `Optional[int]`-typed instance
        // attribute joins the same defensive arm as every other
        // multi-word/aggregate `Scalar` above -- this raw-`i64`-word slot
        // encoding has no room for the `{ i64, i8 }` struct's extra
        // present/absent byte, and this PR ships no class-attribute use of
        // `Optional[int]` for `slot_ty_from_init_rhs` to have exercised.
        | Scalar::Optional(_) => panic!(
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

/// Applies the representation-changing assignment conversions accepted by
/// this type system: standalone `i8` bool to D-141's identity-preserving
/// int-compatible `i64`, and (D-197, #763, Part 1 of #747) a bare
/// `inner`-typed value or `None` widening into an `Optional[inner]` slot's
/// `{ inner, i8 }` present/absent struct. All other assignable pairs already
/// share a representation.
///
/// This is the *only* place `Optional[inner]`'s representation is built --
/// driven entirely by the target slot's own declared type, not by anything
/// MIR-lowering-time. That single-site design (rather than an `IntBoundary`-
/// style MIR wrapper node) is deliberate: it uniformly covers both the
/// first, `AnnAssign`-introduced binding (`x: int | None = 5`) and every
/// later plain reassignment to the same name (`x = None`), which
/// `pycc_mir::stmt::lower_stmt`'s `Assign` arm never wraps -- see that
/// arm's own doc comment on why a wrapper node is unnecessary.
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
        // A `Scalar::Optional` arriving here is either a *real*
        // `Optional[inner]` value (e.g. `w: int | None = y` where `y` is
        // itself `Optional[int]`, read back out via `MirExpr::Name`'s own
        // `Ty::Optional` arm), or `MirExpr::NoneLiteral`'s own `{ i8, i8 }`
        // all-zero placeholder standing in for the bare `None` literal
        // (`emit_expr_unchecked`'s `MirExpr::NoneLiteral` arm has no target
        // `inner` type to build a real struct against). Distinguished by
        // LLVM struct type, not by any Rust-level tag: an anonymous struct
        // type is uniqued per-`Context` by its field types, so the real
        // `{ i64, i8 }` (or whatever `inner` actually is) and the
        // placeholder's fixed `{ i8, i8 }` compare unequal whenever they
        // are not the exact same shape, and equal (and therefore pass
        // through unchanged) whenever the source already has the target's
        // exact representation.
        //
        // #809 risk, verified: for `inner == Ty::Bool`, `ty_to_basic_type`'s
        // `Ty::Optional` arm produces the real `Optional[bool]` shape `{
        // i8, i8 }` -- structurally, and therefore (LLVM struct types being
        // uniqued per-field-type-list within a `Context`) *literally the
        // same `StructType` value as* the `NoneLiteral` placeholder's own
        // fixed `{ i8, i8 }`. So this arm's `v.get_type() == struct_ty`
        // check cannot distinguish "this is genuinely the untyped None
        // placeholder" from "this is a real, already-correctly-shaped
        // `Optional[bool]` value" when `inner == Ty::Bool` -- both take the
        // `v.get_type() == struct_ty` branch and pass `v` through as-is.
        // This is harmless rather than a bug: taking that branch for the
        // placeholder returns `{ payload: 0, present: 0 }` unchanged, which
        // is *exactly* the correct representation for an absent
        // `Optional[bool]` too -- `Ty::Bool`'s payload carries no tagged
        // encoding analogous to `Ty::Int`'s D-141 word (see `truthy`'s and
        // `MirExpr::OptionalUnwrap`'s own `Ty::Bool` handling below/in
        // `lib.rs`, both of which read the payload only as a plain `0`/`1`
        // and never as anything requiring a specific "valid" bit pattern
        // the way `classify_encoded_int` does for `Ty::Int`), so an
        // arbitrary (here, zero) payload byte is always a safe stand-in
        // when `present == 0`. Every consumer of an `Optional[bool]`
        // payload (`truthy`'s `Scalar::Optional` arm, `OptionalUnwrap`'s
        // codegen arm) is itself only ever reached with `present == 1`
        // guarding real use of the payload's *value* (narrowing proves
        // presence before an unwrap; `truthy` ANDs the payload's
        // truthiness with `present` rather than trusting it alone) -- so no
        // observer can tell the difference between "the None placeholder,
        // never coerced" and "a real, coerced, present `Optional[bool]`"
        // for the one case (`present == 0`) where the collision could ever
        // matter. `optional_bool_none_placeholder_and_real_absent_value_are_the_same_llvm_struct_type`
        // and
        // `optional_bool_absent_value_truthiness_and_narrowed_unwrap_are_both_correct`
        // (`pycc_codegen::tests`) pin both halves of this finding: the
        // literal type-identity collision, and that it produces correct
        // end-to-end behavior for `x: bool | None = None` through both
        // `truthy` and an `if x is not None:` narrowed unwrap.
        (pycc_mir::Ty::Optional(inner), Scalar::Optional(v)) => {
            let struct_ty =
                ty_to_basic_type(context, pycc_mir::Ty::Optional(inner.clone())).into_struct_type();
            if v.get_type() == struct_ty {
                Scalar::Optional(v)
            } else {
                // The placeholder's field 0 must be a value of field 0's
                // *actual* declared type (`struct_ty`'s first field, sized
                // by `inner`), not unconditionally a D-141-encoded int:
                // `struct_ty.get_undef()` is `inner`-shaped (`{ i64, i8 }`
                // for `Ty::Int`, `{ f64, i8 }` for `Ty::Float`, `{ i8, i8 }`
                // for `Ty::Bool`), and `build_insert_value` performs no
                // type-matching validation of its own -- inserting a
                // mismatched-type constant (e.g. an `i64` into a `Ty::Float`
                // struct's `f64` field 0) builds a malformed
                // `ConstantStruct` that both `module.verify()` and LLVM's
                // own IR printer accept without complaint (neither
                // re-validates a `ConstantAggregate`'s element types against
                // its declared struct type), so this was previously a
                // latent miscompile rather than a build-time or
                // verification failure. For `Ty::Int`,
                // `tag_smallint_const(context, 0)` encodes `0` the same way
                // `to_encoded_int` would (`truthy`'s own `Scalar::Optional`
                // arm unconditionally calls `pycc_rt_int_truthy` on field 0
                // and ANDs the result with the present flag rather than
                // branching around it, so an absent-but-invalid payload
                // would trip `classify_encoded_int`'s fail-closed panic even
                // though the AND makes the payload's truth value
                // irrelevant). `Ty::Float` and `Ty::Bool` carry no such
                // tagged encoding, so a plain zero of the field's own type
                // is exact.
                let payload0: inkwell::values::BasicValueEnum = match inner.as_ref() {
                    pycc_mir::Ty::Int => tag_smallint_const(context, 0).into(),
                    pycc_mir::Ty::Float => context.f64_type().const_zero().into(),
                    pycc_mir::Ty::Bool => context.i8_type().const_zero().into(),
                    // `T0049` (`crates/pycc_hir/src/func.rs`) rejects every
                    // `Optional[T]` annotation for `T` outside `{int, float,
                    // bool}` before this value could ever be constructed.
                    other => panic!(
                        "pycc_codegen: internal error: an Optional[_] placeholder targeted an unsupported inner type ({other:?}) -- pycc_types::check (T0049) should have rejected this before codegen"
                    ),
                };
                let with_payload = builder
                    .build_insert_value(struct_ty.get_undef(), payload0, 0, "opt_none_payload")
                    .expect(
                        "build_insert_value should not fail inserting field 0 of a fresh struct",
                    )
                    .into_struct_value();
                let with_flag = builder
                    .build_insert_value(
                        with_payload,
                        context.i8_type().const_zero(),
                        1,
                        "opt_none_flag",
                    )
                    .expect(
                        "build_insert_value should not fail inserting field 1 of a fresh struct",
                    )
                    .into_struct_value();
                Scalar::Optional(with_flag)
            }
        }
        // A bare `inner`-typed (or `inner`-assignable, e.g. `bool` under
        // `Optional[int]`) value widening into an `Optional[inner]` slot:
        // recurse to apply any further representation change the payload
        // itself needs (e.g. `bool -> int`, the arm above), then wrap the
        // result as the struct's present (`i8 = 1`) payload field.
        (pycc_mir::Ty::Optional(inner), bare) => {
            let coerced = coerce_scalar_to_type(context, builder, bare, (*inner).clone());
            // #809 (Part 3 of #747): `T0049`'s widened gate
            // (`crates/pycc_hir/src/func.rs`) now also admits
            // `Optional[float]`/`Optional[bool]`, so this match is keyed on
            // *both* the declared inner type and the coerced `Scalar`
            // variant, not the `Scalar` variant alone -- matching on the
            // variant alone would accept e.g. a stray `Scalar::Float`
            // widening into an `Optional[int]` slot (that combination
            // falls through `coerce_scalar_to_type`'s own recursive call
            // unchanged, since there is no `float -> int` widening arm),
            // silently building a struct whose field 0 both has the wrong
            // LLVM type for `Ty::Optional(Int)`'s `{ i64, i8 }` shape and
            // is never a value any real `Optional[int]`-typed source could
            // produce (`pycc_types` never assigns a bare `float` to an
            // `int`-annotated slot). Pairing the target inner type with the
            // coerced variant keeps each of the three real widenings exact
            // and every mismatched combination falling to the panic below.
            let payload: inkwell::values::BasicValueEnum = match (inner.as_ref(), coerced) {
                (pycc_mir::Ty::Int, Scalar::Int(v)) => v.into(),
                // Each passes through as-is: `ty_to_basic_type`'s own
                // `Ty::Optional` arm already sizes field 0 from the inner
                // type, so no further representation change is needed here.
                (pycc_mir::Ty::Float, Scalar::Float(v)) => v.into(),
                (pycc_mir::Ty::Bool, Scalar::Bool(v)) => v.into(),
                // `T0049` (`crates/pycc_hir/src/func.rs`) rejects every
                // `Optional[T]` annotation for `T` outside `{int, float,
                // bool}` before this value could ever be constructed, and
                // `pycc_types::check`'s own assignability rule only ever
                // widens a bare value into the *matching* inner type (or
                // `bool -> int`, handled by the recursive call above), so
                // an inner/coerced-variant mismatch reaching here means one
                // of those upstream gates accepted something it should
                // have rejected.
                _ => panic!(
                    "pycc_codegen: internal error: an Optional[int|float|bool] assignment's payload did not evaluate to int, float, or bool -- pycc_types::check (T0049) should have rejected this before codegen"
                ),
            };
            let struct_ty =
                ty_to_basic_type(context, pycc_mir::Ty::Optional(inner)).into_struct_type();
            let with_payload = builder
                .build_insert_value(struct_ty.get_undef(), payload, 0, "opt_payload")
                .expect("build_insert_value should not fail inserting field 0 of a fresh struct")
                .into_struct_value();
            let with_flag = builder
                .build_insert_value(
                    with_payload,
                    context.i8_type().const_int(1, false),
                    1,
                    "opt_present",
                )
                .expect("build_insert_value should not fail inserting field 1 of a fresh struct")
                .into_struct_value();
            Scalar::Optional(with_flag)
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
        | Scalar::Instance(_)
        // D-197, #763, Part 1 of #747: `Optional[int]` joins this same
        // or-pattern for the identical `numeric_result_type`/`as_numeric`
        // reason -- `range()` operands are type-checked as plain numeric
        // types before codegen, and an `Optional[int]` is never one.
        | Scalar::Optional(_) => {
            panic!("pycc_codegen: internal error: range() {position} did not evaluate to int")
        }
    }
}

/// Evaluates a `range()`-shaped triple of bounds (`MirStmt::ForRange` and
/// its three comprehension-tail copies all share this exact preheader
/// shape) with #638 (D-208) exception-edge protection: `start`'s word is
/// protected across `stop` and `step`'s own evaluation, and `stop`'s word is
/// protected across `step`'s, mirroring the two-operand push/pop pattern
/// `BinOp`/`Compare` use above but generalized to three operands evaluated
/// in sequence. Per D-179, these bounds are legitimately owned, live
/// `IntValue`s held across each other's evaluation (unlike `Slice`/
/// `Subscript`/container-literal bounds, which abort the process on a
/// bigint before a sibling is ever reached) -- see the #638 decision
/// entry's affected-site inventory for the full boundary.
///
/// Returns the three normalized `IntValue`s; callers keep their own
/// existing retain/release and `owned_range_operands` bookkeeping unchanged
/// -- this helper only factors out the shared evaluation-with-protection
/// step, not the ownership contract built on top of it.
#[allow(clippy::too_many_arguments)]
fn emit_range_operands_with_exception_safety<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    start: &MirExpr,
    stop: &MirExpr,
    step: &MirExpr,
) -> (IntValue<'ctx>, IntValue<'ctx>, IntValue<'ctx>) {
    let start_v = range_operand_to_normalized_int(
        context,
        builder,
        rt,
        emit_expr(context, builder, module, rt, user_functions, locals, start),
        "start",
    );
    let pending_start = push_pending_int_release_if_temporary(rt, start, start_v);
    let stop_v = range_operand_to_normalized_int(
        context,
        builder,
        rt,
        emit_expr(context, builder, module, rt, user_functions, locals, stop),
        "stop",
    );
    let pending_stop = push_pending_int_release_if_temporary(rt, stop, stop_v);
    let step_v = range_operand_to_normalized_int(
        context,
        builder,
        rt,
        emit_expr(context, builder, module, rt, user_functions, locals, step),
        "step",
    );
    // LIFO pop order, matching the LIFO push order above.
    pop_pending_int_release(rt, pending_stop);
    pop_pending_int_release(rt, pending_start);
    (start_v, stop_v, step_v)
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
        // Defensive for the identical `numeric_result_type` reason as the
        // arms above, extended to `Optional[int]` (D-197, #763, Part 1 of
        // #747).
        Scalar::Optional(_) => {
            panic!("pycc_codegen: internal error: expected a numeric operand, got optional")
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
        // A real, reachable feature gap, identical in kind to the `List`/
        // `Dict`/`Set` arms above (D-197, #763, Part 1 of #747):
        // `pycc_types` places no type restriction on `print`'s argument or
        // an f-string interpolation, so `print(x)`/`f"{x}"` for an
        // `Optional[int]` local type-checks today and lands here. This PR
        // ships no `str(Optional[int])`/`Optional`-printing semantics (CPython's
        // own `str(None)` is `"None"` and `str(5)` is `"5"`, which this
        // representation *could* support, but doing so is out of this PR's
        // scope -- see this PR's ADR), and there is no
        // `pycc_rt_optional_int_to_str` to call -- so this panics honestly
        // instead of reinterpreting the struct's first field as an `i64` or
        // `PyStrObj` pointer.
        Scalar::Optional(_) => {
            panic!("pycc_codegen: string conversion of an Optional[int] value is not supported yet")
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
        // The bare literal `None` (D-197, #763, Part 1 of #747), standing
        // alone rather than already known to be flowing into a
        // predeclared `Optional[inner]` slot -- that context-sensitive
        // wrapping happens at the assignment site
        // (`coerce_scalar_to_type`, driven by the target slot's own
        // declared type), not here, so this arm has no target `inner` type
        // to build a real `{ inner, i8 }` struct against. A minimal `{ i8,
        // i8 }` all-zero placeholder is emitted instead: `pycc_types`
        // accepts a bare `None` expression only as an `Optional[_]`
        // initializer or an `is`/`is not` operand in this PR (never
        // printed, compared for equality, or used arithmetically), and
        // both of those consumers rebuild the correctly-typed struct
        // themselves (`coerce_scalar_to_type` for assignment,
        // `MirExpr::Compare`'s `Is`/`IsNot` codegen below reads the
        // struct's `i8` flag field only, which is at the same offset
        // regardless of the payload's own width) -- see this PR's ADR.
        MirExpr::NoneLiteral => {
            let placeholder_ty = context.struct_type(&[context.i8_type().into(); 2], false);
            Scalar::Optional(placeholder_ty.const_zero())
        }
        // `OptionalWrap` (D-197, #763, Part 1 of #747) exists purely to fix
        // `.ty()` for `collect_stmt_bindings`'s slot-type derivation (see
        // its own doc comment in `pycc_mir`) -- the actual `{ inner, i8 }`
        // struct-building work is `coerce_scalar_to_type`'s, called here
        // with the wrapper's own declared `Ty::Optional(inner)` as the
        // target so it applies uniformly whether the wrapped value is a
        // bare `inner`-typed payload or `MirExpr::NoneLiteral`'s
        // placeholder.
        MirExpr::OptionalWrap(value, inner) => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            // #770 review: `value`'s classification must be checked here,
            // against the raw `Scalar` it just produced, *before*
            // `coerce_scalar_to_type` below wraps it into a
            // `Scalar::Optional` struct. `MirStmt::Assign`'s own
            // `retain_if_int_duplicate` call runs on the *outer*
            // `OptionalWrap` expression and its already-wrapped
            // `Scalar::Optional` result, which can never retain anything
            // (`retain_if_int_duplicate`'s `if let Scalar::Int(word) =
            // scalar` guard never matches a struct) and
            // `int_value_is_a_duplicate_reference` classifies
            // `OptionalWrap` itself as owning for exactly that reason --
            // by design, it never looks inside the wrapper. So a borrowed
            // `int` payload (e.g. `x: int | None = n` for a heap-bigint
            // `n`) reached this arm with no retain anywhere in the
            // pipeline before this fix, letting `x` hold a second,
            // unretained reference to `n`'s bigint: `n`'s later
            // reassignment/death still fires `release_int_slot_before_store`
            // unconditionally, freeing the object out from under `x`'s
            // stored payload despite `x` still being live. Reusing the
            // same classification here (rather than duplicating it) keeps
            // both the direct-assignment and the `OptionalWrap` paths
            // agreeing on which shapes are borrowed.
            let scalar = retain_if_int_duplicate(context, builder, rt, value, scalar);
            coerce_scalar_to_type(
                context,
                builder,
                scalar,
                pycc_mir::Ty::Optional(inner.clone()),
            )
        }
        // Issue #769 (Part 2 of #747): the read-side counterpart of
        // `OptionalWrap` immediately above. `pycc_types::check` (via its
        // own `narrow` overlay) has already proven this particular read is
        // reachable only when the value is present, so `value` below
        // always evaluates to a real `Scalar::Optional { payload, present }`
        // struct (never the `MirExpr::NoneLiteral` all-zero placeholder --
        // that placeholder is only ever a `coerce_scalar_to_type` target,
        // never something a narrowing test could recognize as
        // `Optional`). `T0049` (`crates/pycc_hir/src/func.rs`) restricts
        // every `Optional[T]` annotation to `T = int`, so `inner` here is
        // always `Ty::Int` and field 0 is always an already-D-141-encoded
        // int word -- extracting it and returning `Scalar::Int` needs no
        // decode/re-encode step, exactly like `Ty::Optional(_)`'s own
        // `MirExpr::Name` read arm above returns the struct as-is with no
        // transformation of its own fields.
        //
        // Refcount reasoning (bigint case, `n: int | None = <heap bigint>`
        // then `if n is not None: use(n)`): this arm performs a plain
        // *read* of `n`'s slot (via the inner `Name` -> `Ty::Optional`
        // load above), extracting the payload word without incrementing
        // any refcount -- exactly mirroring a bare (non-`Optional`) `int`
        // local's own `MirExpr::Name` read arm, which likewise loads and
        // returns `Scalar::Int` with no retain. Both are borrowed reads:
        // ownership transfer (and therefore the retain that must accompany
        // it) happens only where a value is *stored* into a new owning
        // slot -- `MirStmt::Assign`'s `retain_if_int_duplicate` call, or
        // (for the wrap direction) `OptionalWrap`'s own arm just above.
        // `use(n)` here passing the unwrapped `Scalar::Int` to a function
        // call follows the same already-correct borrowed-argument
        // convention every other bare `int` argument uses; nothing about
        // narrowing changes that convention, so no extra retain belongs in
        // this arm specifically. See `pycc_codegen::bigint_rc`'s
        // `is_owning_producer`/`int_value_is_a_duplicate_reference`
        // classification of this node (mirroring `MirExpr::Name`, not
        // `OptionalWrap`) for the corresponding compile-time classification.
        MirExpr::OptionalUnwrap(value, inner) => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            let Scalar::Optional(v) = scalar else {
                panic!(
                    "pycc_codegen: internal error: OptionalUnwrap's operand did not evaluate to Scalar::Optional -- pycc_mir should only ever wrap a Ty::Optional-scoped Name read"
                );
            };
            let payload = builder
                .build_extract_value(v, 0, "narrowed_payload")
                .expect(
                    "build_extract_value should not fail extracting field 0 of an Optional struct",
                );
            // #809 (Part 3 of #747): `T0049`'s widened gate now admits
            // `Ty::Float`/`Ty::Bool` inner types alongside the pre-existing
            // `Ty::Int`, so the narrowed payload must become the matching
            // `Scalar` variant instead of always `Scalar::Int` -- a plain
            // `f64`/`i8` payload handed to an `int`-typed consumer as
            // `Scalar::Int` would misinterpret it as a D-141-encoded word.
            match inner.as_ref() {
                pycc_mir::Ty::Float => Scalar::Float(payload.into_float_value()),
                pycc_mir::Ty::Bool => Scalar::Bool(payload.into_int_value()),
                // `Ty::Int`, the pre-existing shape -- and every other
                // `Ty` is unreachable here: `T0049`
                // (`crates/pycc_hir/src/func.rs`) restricts every
                // `Ty::Optional` inner type to `{int, float, bool}` before
                // an `OptionalUnwrap` node can ever be constructed.
                _ => Scalar::Int(payload.into_int_value()),
            }
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
                // NOT a pointer read, identical in kind to `Ty::Tuple(_)`
                // above (D-197, #763, Part 1 of #747): an `Optional[int]`
                // slot holds the `{ int, i8 }` struct itself, so this loads
                // the whole aggregate back out as an SSA value. Every real
                // read of an `Optional[int]` local -- `x is None`,
                // `if x:`, passing `x` as an argument, `return x` -- lowers
                // to exactly this `MirExpr::Name`, so without it every one
                // of those would fall through to the catch-all below and
                // panic on a real, type-checked program instead of reading
                // the value.
                Ty::Optional(_) => {
                    let loaded = builder
                        .build_load(
                            ty_to_basic_type(context, ty.clone()).into_struct_type(),
                            slot.ptr,
                            "load",
                        )
                        .expect(
                            "build_load should not fail for a slot this function itself allocated",
                        );
                    Scalar::Optional(loaded.into_struct_value())
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
            // #638 (D-208): `l` may already be an owned, not-yet-released
            // `Ty::Int` birth reference (e.g. `x + x` promoting to a fresh
            // heap word). Protect it across `right`'s evaluation, which may
            // recurse into an exception-settable node and branch away
            // before this arm's own `release_if_int_temporary(left, l)`
            // call below is ever reached. The pop on the fallthrough path
            // immediately precedes that unchanged release call, so at most
            // one of {exception-edge release, fallthrough release} ever
            // executes for this word.
            let pending_l = push_pending_int_release_if_scalar_temporary(rt, left, &l);
            let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
            pop_pending_int_release(rt, pending_l);
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
            // #638 (D-208): same protection as `BinOp`'s `Ty::Int` arm --
            // `l` must survive `right`'s evaluation intact so the int
            // branch's own `release_if_int_temporary(left, l)` call below
            // is guaranteed to see it, even when `right`'s evaluation
            // branches away on an exception.
            let pending_l = push_pending_int_release_if_scalar_temporary(rt, left, &l);
            let r = emit_expr(context, builder, module, rt, user_functions, locals, right);
            pop_pending_int_release(rt, pending_l);
            // `is`/`is not` (D-197, #763, Part 1 of #747). HIR lowering
            // (`crates/pycc_hir/src/expr.rs`'s `Expr::Compare` arm)
            // guarantees one operand is syntactically `Expr::NoneLiteral`
            // whenever `op` is `Is`/`IsNot`, and `pycc_types`' own
            // `Is`/`IsNot` typing arm (`crates/pycc_types/src/expr.rs`)
            // guarantees the *other* operand's type is `Ty::Optional(_)` or
            // `Ty::None` -- never anything else. Handled as its own
            // early-computed branch, before the float/str/numeric branches
            // below (none of which know what to do with a struct-valued
            // `Scalar::Optional`), by testing the *other* operand's
            // present/absent flag directly rather than doing any real
            // comparison: `None`/`Ty::None` is always absent (`is` is
            // always `False`, `is not` always `True`, independent of the
            // operand's own emitted value, so neither `l` nor `r` needs
            // inspecting for that shape).
            // Narrows `op`'s 8 `CmpOpKind` variants down to the 6 ordinary
            // ordering comparators in exactly one place (D-197, #763, Part 1
            // of #747): `Is`/`IsNot` are handled and returned right here,
            // inline, so the three type-dispatched matches below (float/
            // str/int) only ever see `OrderedCmpOp`'s 6 variants and need no
            // `Is`/`IsNot` arm of their own at all. The project's own
            // established convention (see `emit_string_literal`'s doc
            // comment) is to eliminate a provably-dead branch structurally
            // rather than leave an `unreachable!()` arm as a permanently
            // uncovered region under this crate's 100%-region gate (D-014)
            // -- the three-way duplication this replaces (one `unreachable!`
            // per type-dispatched match) was exactly that anti-pattern.
            enum OrderedCmpOp {
                Eq,
                NotEq,
                Lt,
                LtE,
                Gt,
                GtE,
            }
            let op = match op {
                pycc_mir::CmpOpKind::Is | pycc_mir::CmpOpKind::IsNot => {
                    let (other_scalar, other_ty) = if matches!(left.as_ref(), MirExpr::NoneLiteral)
                    {
                        (r, right_ty)
                    } else {
                        (l, left_ty)
                    };
                    // Dispatches on `other_scalar`'s own runtime variant,
                    // not on `other_ty`, so there is no separate "statically
                    // `Optional[_]` but did not evaluate to `Scalar::
                    // Optional`" arm to keep alive: every `Ty::Optional`-
                    // typed `MirExpr` this crate can emit -- `Name` (guarded
                    // by its own `debug_assert_eq!` on `slot.ty`),
                    // `OptionalWrap`, and `Call`'s `Ty::Optional` result
                    // extraction -- always produces a matching `Scalar::
                    // Optional`, so that combination is unreachable by
                    // construction, not merely untested; matching on the
                    // scalar directly removes the branch instead of leaving
                    // it as a dead, permanently-uncoverable region.
                    let present = match other_scalar {
                        Scalar::Optional(v) => builder
                            .build_extract_value(v, 1, "opt_present")
                            .expect("build_extract_value should not fail reading field 1 of a 2-field struct")
                            .into_int_value(),
                        _ if other_ty == Ty::None => context.i8_type().const_zero(),
                        _ => panic!(
                            "pycc_codegen: internal error: an `is`/`is not` operand's non-`None` side must be `Optional[_]` -- pycc_types::check (T0021) should have rejected this before codegen"
                        ),
                    };
                    let is_absent = builder
                        .build_int_compare(
                            IntPredicate::EQ,
                            present,
                            context.i8_type().const_zero(),
                            "is_none",
                        )
                        .expect("build_int_compare should not fail comparing two i8 operands");
                    let as_bool = if matches!(op, pycc_mir::CmpOpKind::Is) {
                        is_absent
                    } else {
                        builder
                            .build_not(is_absent, "is_not_none")
                            .expect("build_not should not fail negating an i1 value")
                    };
                    return Scalar::Bool(
                        builder
                            .build_int_z_extend(as_bool, context.i8_type(), "bool_from_is")
                            .expect("build_int_z_extend should not fail widening i1 to i8"),
                    );
                }
                pycc_mir::CmpOpKind::Eq => OrderedCmpOp::Eq,
                pycc_mir::CmpOpKind::NotEq => OrderedCmpOp::NotEq,
                pycc_mir::CmpOpKind::Lt => OrderedCmpOp::Lt,
                pycc_mir::CmpOpKind::LtE => OrderedCmpOp::LtE,
                pycc_mir::CmpOpKind::Gt => OrderedCmpOp::Gt,
                pycc_mir::CmpOpKind::GtE => OrderedCmpOp::GtE,
            };
            let as_bool = if left_ty == Ty::Float || right_ty == Ty::Float {
                let l = to_float(context, builder, rt, l);
                let r = to_float(context, builder, rt, r);
                let predicate = match op {
                    OrderedCmpOp::Eq => FloatPredicate::OEQ,
                    // `UNE` ("unordered or not equal"), not `ONE` --
                    // CPython's `float('nan') != float('nan')` is `True`,
                    // and `NaN` involves an *unordered* comparison, not an
                    // ordered not-equal one. The other five predicates
                    // below correctly stay "ordered" (`O*`): Python's
                    // `<`/`<=`/`>`/`>=`/`==` on `float` are all `False`
                    // whenever `NaN` is involved, which is exactly what the
                    // ordered forms give.
                    OrderedCmpOp::NotEq => FloatPredicate::UNE,
                    OrderedCmpOp::Lt => FloatPredicate::OLT,
                    OrderedCmpOp::LtE => FloatPredicate::OLE,
                    OrderedCmpOp::Gt => FloatPredicate::OGT,
                    OrderedCmpOp::GtE => FloatPredicate::OGE,
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
                    OrderedCmpOp::Eq => IntPredicate::EQ,
                    OrderedCmpOp::NotEq => IntPredicate::NE,
                    OrderedCmpOp::Lt => IntPredicate::SLT,
                    OrderedCmpOp::LtE => IntPredicate::SLE,
                    OrderedCmpOp::Gt => IntPredicate::SGT,
                    OrderedCmpOp::GtE => IntPredicate::SGE,
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
                    OrderedCmpOp::Eq => IntPredicate::EQ,
                    OrderedCmpOp::NotEq => IntPredicate::NE,
                    OrderedCmpOp::Lt => IntPredicate::SLT,
                    OrderedCmpOp::LtE => IntPredicate::SLE,
                    OrderedCmpOp::Gt => IntPredicate::SGT,
                    OrderedCmpOp::GtE => IntPredicate::SGE,
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
        // #604 (Part 3 of #573): `not x`. Reuses `truthy`, the exact same
        // helper an `if`/`while` condition's own test already calls (see
        // `MirStmt::If` above), then inverts the resulting `i1` -- so a
        // `not` over any operand `truthy` can classify (`bool`/`int`/
        // `float`/`str`/`Optional`; `pycc_types::unop::unary_result_type`
        // rejects every other operand before this ever runs) gets
        // `if`/`while`'s exact truthiness semantics for free, including
        // the `float`'s `UNE`-not-`ONE` NaN handling and `Optional`'s
        // present/payload AND. Every other truthy-call site releases any
        // int temporary the operand produced *after* `truthy` reads it
        // (#146 Part 2, D-181) -- this one follows the identical sequence.
        MirExpr::Not(operand) => {
            let operand_scalar = emit_expr(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                operand,
            );
            let truthy_cond = truthy(context, builder, rt, operand_scalar);
            release_scalar_if_int_temporary(context, builder, rt, operand, &operand_scalar);
            let inverted = builder
                .build_not(truthy_cond, "not_truthy")
                .expect("build_not should not fail inverting a well-formed i1");
            let as_bool = builder
                .build_int_z_extend(inverted, context.i8_type(), "bool_from_not")
                .expect("build_int_z_extend should not fail widening i1 to i8");
            Scalar::Bool(as_bool)
        }
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
                // `Optional[int]` (D-197, #763, Part 1 of #747): unlike
                // `List`/`Dict`/`Set`/`Tuple` below, an `Optional`-returning
                // function IS reachable from real, type-checked source --
                // `pycc_hir::func::annotation_to_ty`'s own `T | None` arm
                // accepts `-> int | None` as a return annotation, so a call
                // to such a function (`y = g(x)`, or `g(x)` used directly
                // as a narrowing condition's operand) must extract its
                // struct-by-value result rather than falling through to the
                // generic panic below. `ty_to_basic_type`'s own
                // `Ty::Optional` arm already gave this function's LLVM
                // signature the matching `{ inner, i8 }` struct return
                // type, mirroring `Tuple`'s own by-value extraction one
                // arm up in kind (D-115).
                Ty::Optional(_) => Scalar::Optional(
                    call_site
                        .try_as_basic_value()
                        .expect_basic("this function is declared to return Optional[int]")
                        .into_struct_value(),
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
        // mutating one in place. No alloca, no pointer, and no allocation
        // of the aggregate itself -- but D-107/D-124's leak-only policy
        // does apply to what the fields hold: under D-182 a borrowed `int`
        // element is retained on the way in (see the retain below) and
        // nothing releases it when the tuple dies.
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
            // #638 (D-208), sixth site: a fresh (non-duplicate) `Ty::Int`
            // element's ownership transfers into the aggregate's own field
            // on the normal path -- there is no release call for it
            // anywhere in this arm, exactly like `build_call_to_with_leading_args`'s
            // argument-marshalling loop. If a *later* sibling element's own
            // evaluation raises before the loop below completes, that
            // transfer never happens and an earlier element's birth
            // reference would otherwise be orphaned. `mark` records the
            // stack depth before this loop so the truncate below is
            // recursion-safe: an element expression can itself contain a
            // nested call or tuple literal whose own loop pushes and pops
            // its own entries while evaluating this loop's own element,
            // and this loop's earlier pending entries must stay untouched
            // by that nested truncation.
            let mark = rt.exceptions.pending_int_releases.borrow().len();
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
                // D-182: a tuple field takes its own reference at ingress.
                // Only a *borrowed* element word is retained here -- that is
                // what `retain_if_int_duplicate_and_track_for_exception_edge`
                // classifies -- because an owning element (a fresh `BinOp`
                // result, an out-of-range literal) already arrives with the
                // single reference this field will hold. Retaining
                // unconditionally would leave rc at 2 with only one owner,
                // which a future `Ty::Tuple` slot-death release under D-124
                // could never balance.
                //
                // `emit_bigint_refcount_call`'s documented postcondition:
                // on the non-constant path it leaves the builder positioned
                // in a fresh continuation block, so `get_insert_block()`
                // must be re-read unconditionally after this call. The
                // `build_insert_value` below is block-agnostic and takes
                // part in no phi, so nothing here depends on the block the
                // element was evaluated in.
                let scalar = retain_if_int_duplicate_and_track_for_exception_edge(
                    context, builder, rt, element, scalar,
                );
                // Push *after* the retain-and-track call, matching
                // `build_call_to_with_leading_args`'s own call order:
                // `int_temporary_word` (via
                // `push_pending_int_release_if_scalar_temporary`) excludes
                // a duplicate reference by construction, so this is a
                // no-op for that case regardless of ordering -- the
                // retain-and-track call above already pushed a *duplicate*
                // element's retained word itself, if any. #834 is closed:
                // a borrowed/duplicate element's extra retain is now
                // protected on this exception edge exactly like an owning
                // element's own word already was, not merely D-180
                // residual 3 (which covers a retain that eventually
                // transfers to a real owner, not one abandoned before
                // transfer completes). See
                // https://github.com/rotnov/pycc/issues/834.
                push_pending_int_release_if_scalar_temporary(rt, element, &scalar);
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
            // #638 (D-208): every element above evaluated successfully --
            // this Rust-level line is reached unconditionally at codegen
            // (emission) time regardless of what the *compiled* program's
            // exception-edge control flow does, since `guard_statement_effects`
            // only emits a runtime branch instruction, never diverts the
            // emitter itself (see `guard_statement_effects`'s own doc
            // comment). Truncate back to `mark` *without* releasing:
            // ownership of every entry pushed above has now transferred to
            // the aggregate's own fields via `build_insert_value`. Getting
            // this backwards (releasing instead of truncating) would
            // double-free every owning element on this normal,
            // non-exception path.
            rt.exceptions
                .pending_int_releases
                .borrow_mut()
                .truncate(mark);
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
        // just by this reasoning: see the crate's `tests` module and its
        // `pop_twice_on_the_same_list_in_one_statement_removes_in_order`
        // end-to-end test.
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
        // unchanged dict. Verified empirically: see the crate's `tests`
        // module and its
        // `dict_get_or_default_nested_in_its_own_default_argument_resolves_correctly`
        // end-to-end test.
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
        // here (see that function's own doc comment). A separate
        // `s.add(x)` statement whose value is already a member needs no
        // special reasoning at all: each `.add()` is its own fully
        // independent `MirStmt`, executed strictly in source order, each
        // re-reading `s`'s current
        // (unchanged-by-`.add()`-itself-except-for-dedup-mutation) pointer
        // -- verified empirically: see the crate's `tests` module and its
        // `set_add_grows_the_set_and_a_repeated_value_still_dedups_codegens_and_runs`
        // end-to-end test.
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
        // Part 3A of #541 (#736): `pycc_mir::class::rewrite_exception_to_message`
        // is this node's sole constructor, applied only to an exception-typed
        // expression -- `base` therefore always evaluates to a
        // `Scalar::Instance` pointer to a live `PyExceptionObj`.
        MirExpr::ExceptionMessage(base) => {
            let base_scalar = emit_expr(context, builder, module, rt, user_functions, locals, base);
            emit_exception_message(builder, rt, base_scalar)
        }
        // PEP 572 (#774): `target := value`. `name`'s storage slot is
        // already predeclared by `collect_expr_bindings` (this node's own
        // slot-scanning counterpart to `collect_stmt_bindings`'s
        // `MirStmt::Assign` handling), exactly the way `MirStmt::Assign`'s
        // own target is predeclared -- so this mirrors that statement's own
        // codegen (`emit_stmt`'s `MirStmt::Assign` arm) almost exactly:
        // evaluate `value`, retain it if it is a borrowed bigint duplicate
        // (T0050 restricts a walrus's value to non-reference-counted scalar
        // types, so `incref_if_str_duplicate` never applies here -- there is
        // no `str`/container/instance case to retain), then store via the
        // same `emit_assign` this statement uses. Unlike a plain assignment
        // statement, this node must also yield a value -- routed through a
        // synthetic `MirExpr::Name` read of the slot it just stored into
        // (mirroring `emit_list_name_read`'s own "round-trip through the
        // `Name` arm" precedent just below), so the result gets the exact
        // same load/definite-assignment/refcount-borrow treatment an
        // ordinary later read of `name` would get -- which is also why
        // `bigint_rc::int_value_is_a_duplicate_reference` classifies
        // `NamedExpr { ty: Ty::Int, .. }` as borrowed exactly like `Name`:
        // the value this arm yields is a second read of the same slot, not
        // a second owned reference.
        MirExpr::NamedExpr { name, value, ty } => {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, value);
            let scalar = retain_if_int_duplicate(context, builder, rt, value, scalar);
            emit_assign(context, builder, rt, locals, name, scalar);
            emit_expr_unchecked(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                &MirExpr::Name {
                    name: name.clone(),
                    ty: ty.clone(),
                },
            )
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
    // #638 (D-208): a fresh (non-duplicate) `Ty::Int` argument's ownership
    // *transfers* to the callee's parameter slot on the normal path --
    // there is no release call for it anywhere in this function, unlike
    // every other affected site in the inventory. If a later sibling
    // argument's own evaluation raises before `build_call`/
    // `build_indirect_call` below is ever reached, that transfer never
    // happens and the reference is orphaned. `mark` records the stack
    // depth before this loop so the truncate below is recursion-safe: an
    // argument expression can itself contain a nested call whose own
    // marshalling loop pushes and pops its own arguments' words while
    // evaluating this loop's own argument, and this loop's earlier
    // pending entries must stay untouched by that nested truncation.
    let mark = rt.exceptions.pending_int_releases.borrow().len();
    let marshalled_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = args
        .iter()
        .zip(&user_function.param_tys[leading_args.len()..])
        .map(|(a, param_ty)| {
            let scalar = emit_expr(context, builder, module, rt, user_functions, locals, a);
            let scalar = incref_if_str_duplicate(builder, rt, a, scalar);
            let scalar = retain_if_int_duplicate_and_track_for_exception_edge(
                context, builder, rt, a, scalar,
            );
            // Push *after* the retain-and-track call: `int_temporary_word`
            // (via `push_pending_int_release_if_scalar_temporary`)
            // excludes a duplicate reference by construction, so this is
            // a no-op for that case regardless of ordering -- the
            // retain-and-track call above already pushed a *duplicate*
            // argument's retained word itself, if any. #834 is closed: a
            // borrowed/duplicate argument's extra retain is now protected
            // on this exception edge exactly like an owning argument's own
            // word already was, not merely D-180 residual 3 (which covers
            // a retain that eventually transfers to a real owner, not one
            // abandoned before transfer completes). See
            // https://github.com/rotnov/pycc/issues/834.
            push_pending_int_release_if_scalar_temporary(rt, a, &scalar);
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
                // Pass-through by VALUE, identical in kind to `Tuple`'s arm
                // above (D-197, #763, Part 1 of #747): an `Optional[int]`
                // argument is an LLVM `{ i64, i8 }` struct at the ABI level
                // -- `coerce_scalar_to_type` immediately above already
                // built it against `param_ty`, so this arm needs no
                // further conversion, only the same `Into` `BasicMetadataValueEnum`
                // has for any `StructValue`.
                Scalar::Optional(v) => v.into(),
            }
        })
        .collect();
    // #638 (D-208): every argument above evaluated successfully -- this
    // Rust-level line is reached unconditionally at codegen (emission)
    // time regardless of what the *compiled* program's exception-edge
    // control flow does, since `guard_statement_effects` only emits a
    // runtime branch instruction, never diverts the emitter itself (see
    // `guard_statement_effects`'s own doc comment). Truncate back to
    // `mark` *without* releasing: ownership of every entry pushed above
    // has now transferred to the callee's parameter slots via the
    // `build_call`/`build_indirect_call` below.
    rt.exceptions
        .pending_int_releases
        .borrow_mut()
        .truncate(mark);
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
        // `Optional[int | float | bool]` (D-197, #763, Part 1 of #747;
        // widened to `float`/`bool` by #809): unlike every
        // container/instance arm above, this one is real rather than a
        // deferred panic -- CPython's `bool(x)` for `x: T | None` is
        // `False` for `None` and otherwise `bool(<the T value>)` (so a
        // present-but-zero/false payload is still falsy), and both halves
        // are directly readable from this representation: field 1 is the
        // present/absent flag, and field 0 (when present) is a payload
        // whose own truthiness this function already knows how to compute
        // for every inner type `T0049` admits -- `Ty::Int`'s D-141-encoded
        // `int` via `pycc_rt_int_truthy` (`Scalar::Int`'s own arm above),
        // `Ty::Float`'s plain `f64` via the *unordered*-not-equal-to-zero
        // compare (`Scalar::Float`'s own arm above, mirrored exactly
        // below), and `Ty::Bool`'s plain `i8` `0`/`1`, which already *is*
        // its own truthiness with no conversion needed (`Scalar::Bool`'s
        // own arm above).
        //
        // This function receives no `Ty` for the inner type -- `Scalar::
        // Optional` carries only the LLVM struct value -- so the inner
        // type is recovered from field 0's own LLVM type instead, which
        // `ty_to_basic_type`'s `Ty::Optional` arm makes unambiguous per
        // inner type: `f64` for `Ty::Float`, `i64` for `Ty::Int`, `i8` for
        // `Ty::Bool`. Every producer of a `Scalar::Optional` reaching this
        // function (`coerce_scalar_to_type`, `OptionalWrap`'s codegen, a
        // `Name` read of an `Optional`-typed local) already builds the
        // struct with the target's own inner-typed payload, so this
        // dispatch is exact, not a heuristic.
        Scalar::Optional(v) => {
            let present = builder
                .build_extract_value(v, 1, "opt_present")
                .expect("build_extract_value should not fail reading field 1 of a 2-field struct")
                .into_int_value();
            let payload = builder
                .build_extract_value(v, 0, "opt_payload")
                .expect("build_extract_value should not fail reading field 0 of a 2-field struct");
            let payload_truthy = match payload {
                inkwell::values::BasicValueEnum::FloatValue(fv) => {
                    // `Ty::Float` inner: mirrors `Scalar::Float`'s own arm
                    // above exactly (same `UNE` predicate, same
                    // zero-extend), rather than inventing a new mechanism.
                    let zero = context.f64_type().const_float(0.0);
                    let cond = builder
                        .build_float_compare(
                            FloatPredicate::UNE,
                            fv,
                            zero,
                            "opt_payload_float_truthy",
                        )
                        .expect("build_float_compare should not fail for two f64 operands");
                    builder
                        .build_int_z_extend(
                            cond,
                            context.i8_type(),
                            "opt_payload_bool_from_float_truthy",
                        )
                        .expect("build_int_z_extend should not fail widening i1 to i8")
                }
                // `i8`-width payload: `Ty::Bool` inner. Already the exact
                // `0`/`1` truthiness value -- `Scalar::Bool`'s own arm
                // above (`Scalar::Bool(v) => v`) treats a bare bool
                // identically, with no runtime call.
                inkwell::values::BasicValueEnum::IntValue(iv)
                    if iv.get_type().get_bit_width() == 8 =>
                {
                    iv
                }
                // Any other int width: `Ty::Int` inner, a D-141-encoded
                // `i64` -- the pre-existing behavior, delegated to
                // `pycc_rt_int_truthy` exactly like `Scalar::Int`'s own arm
                // above.
                inkwell::values::BasicValueEnum::IntValue(iv) => builder
                    .build_call(rt.int_truthy, &[iv.into()], "opt_payload_truthy")
                    .expect("build_call should not fail for a well-formed truthiness check")
                    .try_as_basic_value()
                    .expect_basic("pycc_rt_int_truthy returns a non-void i8")
                    .into_int_value(),
                // Deliberately printing the payload's *type* rather than the
                // payload value itself (e.g. via `{other:?}`, which reaches
                // inkwell's `Value::Debug` impl and calls
                // `LLVMPrintValueToString`): either printer returns an
                // `inkwell::support::LLVMString`, whose `Drop` impl calls
                // `LLVMDisposeMessage` and crashes on Windows for this LLVM
                // release (D-029) unless routed through
                // `llvm_string_to_owned`. Printing the type instead of the
                // value additionally avoids handing the printer a value
                // freshly extracted from a constant struct inside a
                // function whose block is not yet terminated (see PR #812).
                other => panic!(
                    "pycc_codegen: internal error: an Optional[_] payload had an unsupported LLVM representation ({}) -- pycc_types::check (T0049) should have rejected this inner type before codegen",
                    llvm_string_to_owned(other.get_type().print_to_string())
                ),
            };
            builder
                .build_and(present, payload_truthy, "opt_truthy")
                .expect("build_and should not fail for two i8 operands")
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
    } else if matches!(&ty, pycc_mir::Ty::Optional(inner) if **inner == pycc_mir::Ty::Int) {
        // #770 review: mirrors the `Ty::Int` zero-init immediately above,
        // but with the same valid-payload placeholder
        // `default_value_for_type`'s own `Ty::Optional(_)` arm builds
        // (`{ tag_smallint_const(0), 0 }`, never a raw `{0, 0}` struct) --
        // required so `release_optional_int_slot_before_store` can release
        // this slot's payload unconditionally, including at the very first
        // store, without ever reading uninitialized alloca memory or
        // tripping `classify_encoded_int`'s fail-closed panic on a raw
        // zero word.
        let placeholder = default_value_for_type(context, ty.clone());
        builder
            .build_store(ptr, placeholder)
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
// PEP 572 (#774): loosened from `&mut HashMap` to `&HashMap`. The body only
// ever reads the map (`locals.get(target)`), never inserts or removes a key
// -- the actual mutation this function performs is a `build_store` into the
// slot's own already-allocated pointer, not a change to the map of slots
// itself. Every pre-existing call site sits inside `emit_stmt`, which holds
// `locals: &mut HashMap<...>`; passing that through to a `&HashMap`
// parameter is an ordinary reborrow coercion, so this loosening is
// behavior-preserving for all of them. It is required, not merely tidier,
// for `MirExpr::NamedExpr`'s own codegen (`emit_expr_unchecked`'s new arm
// below): that function only ever holds a read-only `locals: &HashMap<...>`
// (expression lowering never needs to declare a *new* slot, only read
// existing ones), yet the walrus's "evaluate value, store to the bound
// local" behavior needs exactly this store logic to run mid-expression.
fn emit_assign<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    rt: &RtFns<'ctx>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    target: &str,
    value: Scalar<'ctx>,
) {
    let slot = locals
        .get(target)
        .cloned()
        .expect("every assignment target must have a predeclared storage slot");
    if slot.ty == pycc_mir::Ty::Int {
        release_int_slot_before_store(context, builder, rt, &slot);
    } else if matches!(&slot.ty, pycc_mir::Ty::Optional(inner) if **inner == pycc_mir::Ty::Int) {
        // #770 review: the release-side half of the retain
        // `MirExpr::OptionalWrap`'s codegen now performs on a borrowed
        // `int` payload -- without this, that retain would be a permanent
        // per-reassignment leak of the slot's previous payload instead of
        // a balanced reference (D-197's only supported `Optional[T]`
        // payload is `int`, so this is the only `Optional` shape that can
        // ever hold a bigint word to release).
        release_optional_int_slot_before_store(context, builder, rt, &slot);
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
        // code at the `build_store` call below. The store itself emits no
        // refcount traffic, but that is no longer because a tuple holds
        // nothing worth counting: under D-182 a tuple field can hold an
        // ingress-retained reference to a borrowed `int` element, so
        // overwriting this slot drops the struct without releasing those
        // fields. That is the accepted, documented leak -- a slot-death
        // release for tuple fields is deferred to D-124's container
        // refcounting, not simply absent because there is nothing to free.
        Scalar::Tuple(v) => v.into(),
        // Pass-through, identical to `List`'s/`Dict`'s/`Set`'s arms above
        // (D-154, Part 1 of #375): storing a class-instance value is
        // storing one opaque pointer into a slot `ty_to_basic_type`
        // already allocated as a pointer. No refcount traffic accompanies
        // it -- `pycc_rt::instance` is leak-only, mirroring `List`/`Dict`/
        // `Set` (see that module's own doc comment).
        Scalar::Instance(v) => v.into(),
        // Pass-through by VALUE, identical in kind to `Tuple`'s arm above
        // (D-197, #763, Part 1 of #747): `coerce_scalar_to_type` above
        // already built the correctly-typed `{ int, i8 }` struct against
        // `slot.ty`, so this arm needs only the store, exactly like
        // `Tuple`'s own struct-by-value store above -- no refcount traffic
        // accompanies it either, for the identical D-182-acknowledged
        // reason `Tuple`'s own comment already gives.
        Scalar::Optional(v) => v.into(),
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

/// PEP 572 (#774): `collect_stmt_bindings`'s own counterpart for a
/// `MirExpr` rather than a `MirStmt` -- a walrus target's storage slot is
/// declared by an expression embedded in a statement (an `if`/`while` test,
/// or a bare expression statement), not by a `MirStmt::Assign` the way
/// every other binding this file predeclares is. Delegates the actual tree
/// walk to `MirExpr::collect_named_expr_bindings` (defined once in
/// `pycc_mir`, shared with that crate's own `pycc_mir::stmt::lower_stmt`
/// hoist-and-bind logic), then applies the exact same storable-type
/// allow-list `MirStmt::Assign`'s own arm below applies -- T0050
/// (`pycc_types::expr::is_walrus_value_ty_supported`) already restricts a
/// walrus's value to a subset of that allow-list (the non-reference-counted
/// scalars), so every `ty` reaching this filter is expected to pass it, but
/// applying the identical check here rather than assuming it keeps this
/// function correct on its own terms if that restriction ever changes.
fn collect_expr_bindings(expr: &MirExpr, bindings: &mut BTreeMap<String, pycc_mir::Ty>) {
    let mut named_bindings = Vec::new();
    expr.collect_named_expr_bindings(&mut named_bindings);
    for (name, ty) in named_bindings {
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
                | pycc_mir::Ty::Instance(_)
                | pycc_mir::Ty::Optional(_)
        ) {
            bindings.entry(name).or_insert(ty);
        }
    }
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
                    // `Optional[int]` (D-197, #763, Part 1 of #747): an
                    // `x: int | None = ...` binding is exactly the case
                    // `OptionalWrap`'s own doc comment describes -- its
                    // lowered `value.ty()` now correctly reports
                    // `Ty::Optional(_)` rather than the bare inner type,
                    // so this allow-list must recognize it or the slot
                    // this function exists to predeclare is silently
                    // skipped, surfacing here as a missing-slot panic
                    // rather than at the type-checking boundary.
                    | pycc_mir::Ty::Optional(_)
            ) {
                bindings.entry(target.clone()).or_insert(ty);
            }
        }
        // PEP 572 (#774): `test` also gets `collect_expr_bindings`'d, not
        // just `body`/`orelse` recursed into -- an `if` test condition is
        // one of the three placements a walrus is permitted in, and its
        // bound name needs a predeclared storage slot exactly like any
        // other local, or `emit_assign`'s own `locals.get(target).expect(..)`
        // panics the first time `emit_expr_unchecked`'s `MirExpr::NamedExpr`
        // arm tries to store into it.
        MirStmt::If {
            test, body, orelse, ..
        } => {
            collect_expr_bindings(test, bindings);
            for stmt in body {
                collect_stmt_bindings(stmt, bindings);
            }
            for stmt in orelse {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // PEP 572 (#774): mirrors `If`'s own `test` handling just above --
        // a `while` test condition is the other permitted placement.
        MirStmt::While { test, body } => {
            collect_expr_bindings(test, bindings);
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
        // PEP 572 (#774): the third permitted walrus placement -- a bare
        // expression statement (`(n := 5)`, or a walrus nested inside a
        // larger expression statement like `f(n := 5)`). `Return`/`NoOp`/
        // `Unreachable` are excluded from this arm on purpose, not merely
        // grouped elsewhere: `pycc_hir::stmt::lower_stmt`'s own
        // `contains_named_expr` restriction rejects a walrus anywhere but
        // the three placements this file's `collect_stmt_bindings` now
        // handles (`ExprStmt` here, `If`/`While`'s own `test` above), so a
        // `MirStmt::Return` can never carry a `NamedExpr` to begin with.
        MirStmt::ExprStmt(expr) => collect_expr_bindings(expr, bindings),
        MirStmt::Return(_) | MirStmt::NoOp | MirStmt::Unreachable => {}
        MirStmt::Seq(stmts) => {
            for stmt in stmts {
                collect_stmt_bindings(stmt, bindings);
            }
        }
        // #382 (PR-22 Part 1): try/except/else/finally — recurse into all
        // nested bodies to collect any bindings introduced within them.
        // Part 3 of #382 (#542, PEP 654, D-202): `except*`/`TryStar` shares
        // this exact recursion -- a handler body's nested bindings are
        // collected identically regardless of whether the handler binds its
        // name to the named exception type (`Try`) or to `ExceptionGroup`
        // (`TryStar`), since that binding-type distinction is resolved by
        // `pycc_mir` lowering, not by this bindings-discovery pass.
        MirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        }
        | MirStmt::TryStar {
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
                // `int | None` (D-197, #763, Part 1 of #747): stored inline
                // like `Tuple`'s own struct arm immediately above, not as a
                // nullable pointer -- an `Optional[inner]` value is an LLVM
                // aggregate, exactly like a tuple, not a heap object. `x:
                // int | None = 5` at module scope is exactly the shape this
                // PR's own conformance fixture uses, so omitting this arm
                // would turn that supported form into an internal compiler
                // panic, the same reasoning `Tuple`'s own arm gives.
                //
                // Unlike `Tuple`'s plain `const_zero()`, the initializer's
                // payload field is built as a valid D-141-encoded smallint
                // `0` (`tag_smallint_const`), not a raw zero word -- for the
                // identical branch-free-masked-read reason
                // `default_value_for_type`'s own `Ty::Optional` arm and
                // `coerce_scalar_to_type`'s placeholder-normalization arm
                // are: `truthy`'s `Scalar::Optional` arm ANDs the payload's
                // truthiness with the present flag unconditionally rather
                // than branching around it, and a global read before its
                // first real assignment (guarded only by the separate
                // `initialized` flag below, not by never reaching this
                // value at all) must not hand that AND a raw zero word that
                // trips `classify_encoded_int`'s fail-closed panic. The
                // present flag itself is `0` either way, matching every
                // "unassigned" global's own zero-flag/null-pointer pattern
                // above.
                pycc_mir::Ty::Optional(inner) => {
                    let struct_ty = ty_to_basic_type(context, ty.clone()).into_struct_type();
                    // #809: `default_value_for_type` (below) already draws
                    // this same `Ty::Int`-vs-other-inner-type distinction
                    // for exactly the same reason -- only `Ty::Int`'s
                    // payload field is the D-141 encoded-word
                    // representation `tag_smallint_const` produces; a
                    // `Float`/`Bool` inner type's payload field is a plain
                    // `f64`/`i8` value, and handing `tag_smallint_const`'s
                    // `i64` constant to `const_named_struct` for a struct
                    // whose first field is `f64`/`i8` is an LLVM constant
                    // type/size mismatch (the `Optional[bool]` case was
                    // reproduced crashing the LLVM backend with "invalid
                    // number of bytes" before this fix). Reuse
                    // `default_value_for_type` itself so this initializer
                    // and the per-local entry-block placeholder in
                    // `storage_slot_at_entry` can never drift apart again.
                    let payload = match inner.as_ref() {
                        pycc_mir::Ty::Int => tag_smallint_const(context, 0).into(),
                        other_inner => default_value_for_type(context, other_inner.clone()),
                    };
                    let zeroed = struct_ty
                        .const_named_struct(&[payload, context.i8_type().const_zero().into()]);
                    (struct_ty.into(), zeroed.into())
                }
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
///
/// # Concurrency
///
/// Safe to call concurrently from several threads of one process. LLVM's
/// process-global target registry is initialized exactly once, behind a
/// `OnceLock`, so no thread can observe the registry mid-write; see
/// `target_machine`'s module documentation for why that guard is needed
/// even though inkwell locks `Target::initialize_all` internally. Each
/// call otherwise owns its own `Context`, `Module` and `TargetMachine`,
/// and callers must still supply distinct `output_path`s.
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
            // Set slot 0 to the member's own value literal, carried in
            // `HirClassDef.enum_members` from HIR through MIR to here.
            //
            // For an `int` member (`RED = 1` → 1), `emit_int_constant` folds
            // the value at compile time into the tagged-pointer
            // representation `pycc_rt` uses for small ints, or materializes
            // a heap bigint through `pycc_rt_int_from_i64` when the
            // discriminant is outside the tagged 63-bit range (D-178) --
            // once, here at module init.
            //
            // For a `str` member (#892: `RED = "red"`, or any member of an
            // `enum.StrEnum` subclass), `emit_string_literal` interns the
            // bytes as a private constant and calls
            // `pycc_rt_str_from_literal` -- the same helper slot 1 (the
            // member's `name`) already uses just below.
            let value_scalar = match member_value {
                pycc_mir::EnumMemberValue::Int(value) => {
                    Scalar::Int(emit_int_constant(context, builder, rt, *value))
                }
                pycc_mir::EnumMemberValue::Str(value) => {
                    Scalar::Str(emit_string_literal(context, builder, module, rt, value))
                }
            };
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

    // See `target_machine::target_machine_for` for the LLVM target-registry
    // initialization contract this call satisfies, and for the D-029 and
    // D-073 reasoning behind the triple and relocation-model choices.
    let target_machine = target_machine::target_machine_for(target_triple, release)?;
    // `--release`'s whole-module optimization pipeline (D-094). This is
    // "maximum whole-module optimization," not literal cross-translation-
    // unit LTO: pycc emits exactly one LLVM module per compilation today
    // (single-file only until v0.4's multi-file support), so there is only
    // ever one module for `"default<O3>"` to optimize.
    //
    // Deliberately `.expect(..)`, not `.map_err(llvm_string_to_owned)?` like
    // `Target::from_triple` (in `target_machine`)/`write_to_file` below:
    // those two are genuine,
    // externally-triggerable failure modes with their own dedicated tests
    // and reachable `Err` paths this crate's 100% region-coverage gate
    // (D-014) can actually exercise. `run_passes` here runs a fixed,
    // always-valid pipeline string against a module `verify_module` has
    // already accepted (skipped only on Windows per D-029's own "no
    // Windows-specific IR-building path exists" reasoning, which applies
    // here too) -- there is no way to make this fail that a test could
    // construct, so this stays infallible given how this function always
    // calls it, the same treatment `create_target_machine` gets in
    // `target_machine` and `module.verify()` gets in `verify_module` below.
    // On the
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
            let (start_v, stop_v, step_v) = emit_range_operands_with_exception_safety(
                context,
                builder,
                module,
                rt,
                user_functions,
                locals,
                start,
                stop,
                step,
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
                        // Pass-through by VALUE, identical in kind to
                        // `Tuple`'s arm above (D-197, #763, Part 1 of
                        // #747): `coerce_scalar_to_type` above already
                        // built the correctly-typed struct against
                        // `expected_return_ty`, and `ty_to_basic_type`
                        // already gave the function's LLVM signature that
                        // same struct return type.
                        Scalar::Optional(v) => v.into(),
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
            // Unlike a local's storage slot, an instance attribute's
            // declared `Ty` is not reachable from `MirStmt::AttrSet` (it
            // carries only a slot *index*), so this release is gated on
            // the *value's* type rather than the slot's.
            //
            // #146 Part 1 recorded one shape that diverged: a `bool` value
            // assigned into an `int`-declared attribute reported
            // `Ty::Bool` here, skipping the release of a bigint the
            // attribute still held -- a leak, never a use-after-free
            // (D-180 Consequences item 6). #627 closed that divergence in
            // MIR instead of here: `pycc_mir`'s `HirStmt::AttrSet` arm now
            // wraps such a value in `MirExpr::IntBoundary`, whose `ty()`
            // is `Ty::Int`, so the release below fires and the store
            // encodes a D-141 word. `tests/issue_146_bigint_release.rs`
            // pins the observable half and `bigint_rc.rs`'s own
            // `an_int_attribute_slot_store_of_a_bool_emits_a_guarded_release`
            // pins this release. D-187 records the correction.
            //
            // The gate stays literally value-typed and only *coincides*
            // with D-180 Decision item 4's slot-typed invariant because
            // `bool` -> `int` is the sole widening `pycc_types` permits at
            // an attribute boundary (`bool` into a `float` attribute is
            // rejected with `error[T0021]`). A future widening would break
            // the coincidence silently; threading the declared type
            // through from `mir.class_defs` remains out of scope.
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
            emit_assign(
                context,
                builder,
                rt,
                locals,
                var,
                Scalar::Int(encoded_element),
            );
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
            //    and its neighboring tests in the crate's `tests`
            //    module). `new_list` itself needs
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
                    let (start_v, stop_v, step_v) = emit_range_operands_with_exception_safety(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        start,
                        stop,
                        step,
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
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        current,
                        BigIntRefcount::Retain,
                    );
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
                    emit_assign(
                        context,
                        builder,
                        rt,
                        locals,
                        var,
                        Scalar::Int(encoded_element),
                    );

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
                    emit_assign(
                        context,
                        builder,
                        rt,
                        locals,
                        var,
                        Scalar::Int(encoded_element),
                    );

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
                    release_scalar_if_int_temporary(context, builder, rt, cond_expr, &cond_scalar);
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
                    let (start_v, stop_v, step_v) = emit_range_operands_with_exception_safety(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        start,
                        stop,
                        step,
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
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        current,
                        BigIntRefcount::Retain,
                    );
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
                    emit_assign(
                        context,
                        builder,
                        rt,
                        locals,
                        var,
                        Scalar::Int(encoded_element),
                    );

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
                    emit_assign(
                        context,
                        builder,
                        rt,
                        locals,
                        var,
                        Scalar::Int(encoded_element),
                    );

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
                    release_scalar_if_int_temporary(context, builder, rt, cond_expr, &cond_scalar);
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
                    let (start_v, stop_v, step_v) = emit_range_operands_with_exception_safety(
                        context,
                        builder,
                        module,
                        rt,
                        user_functions,
                        locals,
                        start,
                        stop,
                        step,
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
                    emit_bigint_refcount_call(
                        context,
                        builder,
                        rt,
                        current,
                        BigIntRefcount::Retain,
                    );
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
                    emit_assign(
                        context,
                        builder,
                        rt,
                        locals,
                        var,
                        Scalar::Int(encoded_element),
                    );

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
                    // test in the crate's `tests` module. Otherwise
                    // identical to `ListCompAssign`'s/
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
                    emit_assign(
                        context,
                        builder,
                        rt,
                        locals,
                        var,
                        Scalar::Int(encoded_element),
                    );

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
                    release_scalar_if_int_temporary(context, builder, rt, cond_expr, &cond_scalar);
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
        MirStmt::Raise {
            exception,
            frame_function,
        } => {
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
            emit_exception_set_frame(context, builder, module, rt, exc_obj, frame_function);
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
        MirStmt::RaiseFrom {
            exception,
            cause,
            frame_function,
        } => {
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
            emit_exception_set_frame(context, builder, module, rt, exc_obj, frame_function);
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
        // Part 3 of #382 (#542, PEP 654, D-202): `try`/`except*`/`else`/
        // `finally` codegen -- see `exception::emit_try_star`'s own doc
        // comment for how its handler dispatch differs from `Try`'s above.
        MirStmt::TryStar {
            body,
            handlers,
            orelse,
            finalbody,
        } => exception::emit_try_star(
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
