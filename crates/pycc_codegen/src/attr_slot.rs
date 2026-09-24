//! Instance attribute slot words (D-154): the encoding between a `Scalar`
//! and the raw `i64` word `pycc_rt_instance_get_slot`/`set_slot` carry, and
//! the release of a slot's previous `str`/`int` value before a store.
//!
//! Extracted from `lib.rs` per AGENTS.md's decomposition rule when #1262
//! (Part 1 of #1218) added the `list[int]`/`dict[str, int]` pointer word.

use crate::Scalar;
use crate::bigint_rc::{BigIntRefcount, emit_bigint_refcount_call};
use crate::rt_fns::RtFns;
use inkwell::context::Context;
use inkwell::values::{IntValue, PointerValue};

/// Reinterprets a raw `i64` slot word read from `pycc_rt_instance_get_slot`
/// as the `Scalar` its declared attribute `ty` names (D-154, Part 1 of
/// #375; see `pycc_rt::instance`'s own doc comment for the slot
/// representation this mirrors exactly): `int` passes through unchanged;
/// `bool` truncates to `pycc_codegen`'s own `i8` `Scalar::Bool` carrier;
/// `float` bit-reinterprets the same 8 bytes as `f64` (never a numeric
/// conversion -- the word *is* a float's bit pattern, written by
/// `scalar_to_slot_word`'s own mirror-image `float` arm); `str` reinterprets
/// the word as a `*mut PyStrObj` pointer; and -- since #1262 -- a
/// `list[int]` or `dict[str, int]` reinterprets the word as the container's
/// pointer, the same `inttoptr` `str` uses. Containers are leak-only (D-107,
/// D-124), so the read hands back the very object the slot holds, which is
/// CPython's aliasing. Only these six `Ty`s can ever reach here:
/// `pycc_hir::class::init_slot::slot_ty_from_init_rhs` admits a scalar
/// (int/float/bool/str) parameter or literal, or a `list[int]`/`dict[str,
/// int]` parameter, at `__init__`'s own first-assignment pre-scan, so a
/// `Set`/`Tuple`/`Instance`/`Param`/`Infer`-typed attribute can never be
/// constructed from real, type-checked source.
pub(crate) fn slot_word_to_scalar<'ctx>(
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
        // #1262: one `inttoptr` for both containers, then an asymmetric
        // wrapper choice -- two sibling arms would be the structurally
        // identical regions that have lost coverage counts in this code
        // before (see `init_slot::slot_ty_from_init_rhs`'s own comment).
        pycc_mir::Ty::List(_) | pycc_mir::Ty::Dict(..) => {
            let ptr = builder
                .build_int_to_ptr(
                    raw,
                    context.ptr_type(inkwell::AddressSpace::default()),
                    "attr_container_inttoptr",
                )
                .expect("build_int_to_ptr should not fail reinterpreting an i64 as a pointer");
            if matches!(ty, pycc_mir::Ty::List(_)) {
                Scalar::List(ptr)
            } else {
                Scalar::Dict(ptr)
            }
        }
        other => panic!(
            "pycc_codegen: internal error: an instance attribute of type `{}` is not \
             supported yet -- pycc_hir::class::init_slot::slot_ty_from_init_rhs should have \
             rejected \
             this before codegen",
            other.name()
        ),
    }
}

/// Mirror image of [`slot_word_to_scalar`]: encodes a `Scalar` as the raw
/// `i64` word `pycc_rt_instance_set_slot` stores. See that function's own
/// doc comment for why only `Int`/`Bool`/`Float`/`Str`/`List`/`Dict` are
/// ever reachable here.
pub(crate) fn scalar_to_slot_word<'ctx>(
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
        // #1262: a `list[int]`/`dict[str, int]` slot holds the container's
        // pointer. No refcount traffic: containers are leak-only (D-107,
        // D-124), and `MirStmt::AttrSet`'s release calls are gated on a
        // `str`/`int` value type.
        Scalar::List(v) | Scalar::Dict(v) => builder
            .build_ptr_to_int(v, context.i64_type(), "attr_container_ptrtoint")
            .expect("build_ptr_to_int should not fail reinterpreting a pointer as i64"),
        Scalar::Set(_)
        | Scalar::Tuple(_)
        | Scalar::Instance(_)
        // D-197, #763, Part 1 of #747: an `Optional[int]`-typed instance
        // attribute joins the same defensive arm as every other
        // multi-word/aggregate `Scalar` above -- this raw-`i64`-word slot
        // encoding has no room for the `{ i64, i8 }` struct's extra
        // present/absent byte, and this PR ships no class-attribute use of
        // `Optional[int]` for `slot_ty_from_init_rhs` to have exercised.
        | Scalar::Optional(_)
        // D-244, Part 2 of #1026: a foreign CPython object joins the same
        // or-pattern for the identical reason the aggregate variants above
        // do -- `slot_ty_from_init_rhs` admits only a scalar or a
        // `list[int]`/`dict[str, int]` slot, so a `Ty::Object` attribute is
        // never built. Folded
        // into the existing group rather than given its own arm so it adds
        // no separate, permanently-unexecutable region.
        // Part 2 of #1027: a `memoryview` joins the same or-pattern for
        // the identical reason -- `slot_ty_from_init_rhs` admits no
        // `memoryview` slot. Part 2a of #1142 (#1165) gave
        // the type its one storable position, a *local* slot bound by
        // `a = ndarray(n)`; an instance attribute is not that position and
        // stays refused, so no such attribute is ever built.
        | Scalar::MemoryView(_)
        | Scalar::Object(_) => panic!(
            "pycc_codegen: internal error: cannot store this value into an instance \
             attribute slot -- pycc_hir::class::init_slot::slot_ty_from_init_rhs should \
             have rejected this before codegen"
        ),
    }
}

/// Mirror of [`crate::decref_str_slot_before_store`] for an instance attribute slot
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
pub(crate) fn decref_str_attr_slot_before_store<'ctx>(
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

/// [`crate::bigint_rc::release_int_slot_before_store`]'s counterpart for an instance
/// attribute slot, and the exact `int` mirror of
/// [`decref_str_attr_slot_before_store`] directly above: reads the slot's
/// current raw word through `pycc_rt_instance_get_slot` and releases it
/// before the new value overwrites it. A freshly allocated instance's slots
/// are zero-initialized (`pycc_rt::instance::new_instance`), and `0` is the
/// word `pycc_rt_bigint_release` returns on without classifying, so the
/// same call is correct for `__init__`'s first assignment and every later
/// reassignment.
pub(crate) fn release_int_attr_slot_before_store<'ctx>(
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

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile_to_object_with_observer, llvm_string_to_owned};
    use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

    fn name(name: &str, ty: Ty) -> MirExpr {
        MirExpr::Name {
            name: name.to_string(),
            ty,
        }
    }

    /// The IR of the one function that stores and reads the container
    /// slots, compiled through the real pipeline.
    fn container_slot_function_ir() -> String {
        let self_ty = Ty::Instance(Box::new("Holder".to_string()));
        let list_ty = Ty::List(Box::new(Ty::Int));
        let dict_ty = Ty::Dict(Box::new((Ty::Str, Ty::Int)));
        let get = |slot: usize, ty: &Ty| MirExpr::AttrGet {
            base: Box::new(name("self", self_ty.clone())),
            slot,
            ty: ty.clone(),
        };
        let init = MirItem::Function {
            name: "Holder.__init__".to_string(),
            params: vec![
                ("self".to_string(), self_ty.clone()),
                ("xs".to_string(), list_ty.clone()),
                ("d".to_string(), dict_ty.clone()),
            ],
            return_ty: Ty::None,
            body: vec![
                MirStmt::AttrSet {
                    base: name("self", self_ty.clone()),
                    slot: 0,
                    value: name("xs", list_ty.clone()),
                },
                MirStmt::AttrSet {
                    base: name("self", self_ty.clone()),
                    slot: 1,
                    value: name("d", dict_ty.clone()),
                },
                MirStmt::Assign {
                    target: "ys".to_string(),
                    value: get(0, &list_ty),
                },
                MirStmt::Assign {
                    target: "e".to_string(),
                    value: get(1, &dict_ty),
                },
                MirStmt::Return(None),
            ],
        };
        let mir = MirModule {
            items: vec![init],
            class_defs: Vec::new(),
        };
        let dir = pycc_scratch::ScratchDir::new("attr_slot_container").expect("scratch dir");
        let obj_path = dir.join("attr_slot_container.o");
        let mut ir = String::new();
        // D-029: route `print_to_string`'s `LLVMString` through
        // `llvm_string_to_owned`, as every sibling IR test does.
        let mut observer = |module: &inkwell::module::Module<'_>, _| {
            ir = llvm_string_to_owned(module.print_to_string());
        };
        compile_to_object_with_observer(
            &mir,
            &obj_path,
            &CompileOptions::default(),
            Some(&mut observer),
        )
        .expect("codegen should succeed");
        ir.split("\ndefine ")
            .find(|function| function.contains("attr_container_ptrtoint"))
            .expect("the container slot store should be emitted")
            .to_string()
    }

    #[test]
    fn a_container_slot_is_stored_and_read_as_a_pointer_word_with_no_refcount_traffic() {
        // #1262: `list[int]`/`dict[str, int]` slots hold the container's
        // pointer word (one `ptrtoint` per store, one `inttoptr` per read).
        // Containers are leak-only (D-107, D-124), so neither store may
        // release the slot's previous value: `MirStmt::AttrSet` gates its
        // `str` decref and `int` release on the value's type.
        let ir = container_slot_function_ir();
        let defined = |prefix: &str| {
            ir.lines()
                .filter(|line| line.trim_start().starts_with(prefix))
                .count()
        };
        assert_eq!(defined("%attr_container_ptrtoint"), 2, "{ir}");
        assert_eq!(defined("%attr_container_inttoptr"), 2, "{ir}");
        assert!(!ir.contains("pycc_rt_str_decref"), "{ir}");
        assert!(!ir.contains("pycc_rt_bigint_release"), "{ir}");
    }
}
