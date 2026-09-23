//! `and`/`or` (#1211, Part 3 of #1018): the short circuit, built from basic
//! blocks and a phi join.
//!
//! For `or` (`and` inverts the branch sense):
//!
//! 1. emit `left` and test its truth once;
//! 2. truthy: the `take_left` block converts `left` to the node's type;
//! 3. falsy: the `eval_right` block releases the discarded `left`, emits
//!    `right` and converts it;
//! 4. both arms branch to `join`, whose phi selects the arm's value.
//!
//! Each phi incoming block is read with `get_insert_block()` immediately
//! before that arm's `br join`: an operand's own exception guard and a bigint
//! retain or release each append blocks of their own
//! (`emit_bigint_refcount_call`, `guard_statement_effects`).
//!
//! A truth-only node (`pycc_hir::boolop`) joins `i1` truth bits instead of
//! values, releasing each operand's int temporary right after its truth test
//! as the `MirExpr::Not` arm does.
//!
//! Ownership (the plan's S4): an `int` or `str` result is always owned.
//! Each value arm retains a duplicate (borrowed) operand before converting
//! it, deciding on the operand's own pre-conversion expression, so the
//! duplicate classifiers still see its `Name`/`AttrGet` shape. The `or` arm
//! that extracts an `Optional[int]` payload always retains it: the source
//! is the `Optional` operand, which no classifier calls a duplicate. That
//! leaks one reference when the operand is an owned temporary, and never
//! frees twice. Two further leaks are accepted: a discarded `str` temporary
//! is not decref'd (as in every other truth position today), and a
//! discarded owned `Optional[int]` operand is not released, because
//! `release_scalar_if_int_temporary` handles only a bare `int`.
//!
//! The discarded left operand is released in `eval_right` *before* `right`
//! is emitted, so the node never holds an arm-local word while `right` can
//! raise and pushes nothing onto `pending_int_releases`, whose words would
//! otherwise have to dominate the join.

use super::bigint_rc::{BigIntRefcount, emit_bigint_refcount_call};
use super::{
    RtFns, Scalar, StorageSlot, UserFunction, coerce_scalar_to_type, emit_expr,
    incref_if_str_duplicate, release_scalar_if_int_temporary, retain_if_int_duplicate, truthy,
    ty_to_basic_type,
};
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::values::{BasicValueEnum, IntValue};
use pycc_mir::{BoolOpKind, MirExpr, Ty};
use std::collections::HashMap;

/// The pieces of `emit_expr`'s environment every operand emission needs.
pub(super) struct Emitter<'a, 'ctx> {
    pub(super) context: &'ctx Context,
    pub(super) builder: &'a inkwell::builder::Builder<'ctx>,
    pub(super) module: &'a inkwell::module::Module<'ctx>,
    pub(super) rt: &'a RtFns<'ctx>,
    pub(super) user_functions: &'a HashMap<&'a str, UserFunction<'ctx>>,
    pub(super) locals: &'a HashMap<String, StorageSlot<'ctx>>,
}

impl<'ctx> Emitter<'_, 'ctx> {
    pub(super) fn emit(&self, expr: &MirExpr) -> Scalar<'ctx> {
        emit_expr(
            self.context,
            self.builder,
            self.module,
            self.rt,
            self.user_functions,
            self.locals,
            expr,
        )
    }

    fn truth(&self, scalar: Scalar<'ctx>) -> IntValue<'ctx> {
        truthy(self.context, self.builder, self.module, self.rt, scalar)
    }

    pub(super) fn current_block(&self) -> BasicBlock<'ctx> {
        self.builder
            .get_insert_block()
            .expect("the builder is always positioned inside a block while emitting")
    }

    pub(super) fn new_block(&self, name: &str) -> BasicBlock<'ctx> {
        let function = self
            .current_block()
            .get_parent()
            .expect("the block the builder is positioned in always belongs to a function");
        self.context.append_basic_block(function, name)
    }

    pub(super) fn branch_to(&self, block: BasicBlock<'ctx>) {
        self.builder
            .build_unconditional_branch(block)
            .expect("build_unconditional_branch should not fail for a fresh block");
    }

    /// Branches on `left`'s truth bit: to `decided` when `left` decides the
    /// result (`or` over a truthy `left`, `and` over a falsy one), to
    /// `eval_right` otherwise.
    fn branch_on_left(
        &self,
        op: BoolOpKind,
        left_truth: IntValue<'ctx>,
        decided: BasicBlock<'ctx>,
        eval_right: BasicBlock<'ctx>,
    ) {
        let (then_block, else_block) = match op {
            BoolOpKind::Or => (decided, eval_right),
            BoolOpKind::And => (eval_right, decided),
        };
        self.builder
            .build_conditional_branch(left_truth, then_block, else_block)
            .expect("build_conditional_branch should not fail for a well-formed i1");
    }
}

/// Emits one `MirExpr::BoolOp`.
pub(super) fn emit_bool_op<'ctx>(
    emitter: &Emitter<'_, 'ctx>,
    op: BoolOpKind,
    left: &MirExpr,
    right: &MirExpr,
    ty: &Ty,
    truth_only: bool,
) -> Scalar<'ctx> {
    if truth_only {
        emit_truth_only(emitter, op, left, right)
    } else {
        emit_value(emitter, op, left, right, ty)
    }
}

fn emit_truth_only<'ctx>(
    emitter: &Emitter<'_, 'ctx>,
    op: BoolOpKind,
    left: &MirExpr,
    right: &MirExpr,
) -> Scalar<'ctx> {
    let left_truth = operand_truth(emitter, left);
    let left_end = emitter.current_block();
    let eval_right = emitter.new_block("boolop_eval_right");
    let join = emitter.new_block("boolop_join");
    emitter.branch_on_left(op, left_truth, join, eval_right);

    emitter.builder.position_at_end(eval_right);
    let right_truth = operand_truth(emitter, right);
    let right_end = emitter.current_block();
    emitter.branch_to(join);

    emitter.builder.position_at_end(join);
    let phi = emitter
        .builder
        .build_phi(emitter.context.bool_type(), "boolop_truth")
        .expect("build_phi should not fail for an i1");
    phi.add_incoming(&[(&left_truth, left_end), (&right_truth, right_end)]);
    let as_bool = emitter
        .builder
        .build_int_z_extend(
            phi.as_basic_value().into_int_value(),
            emitter.context.i8_type(),
            "boolop_truth_bool",
        )
        .expect("build_int_z_extend should not fail widening i1 to i8");
    Scalar::Bool(as_bool)
}

/// Emits `operand`, tests its truth and releases its int temporary.
fn operand_truth<'ctx>(emitter: &Emitter<'_, 'ctx>, operand: &MirExpr) -> IntValue<'ctx> {
    let scalar = emitter.emit(operand);
    let truth = emitter.truth(scalar);
    release_scalar_if_int_temporary(
        emitter.context,
        emitter.builder,
        emitter.rt,
        operand,
        &scalar,
    );
    truth
}

fn emit_value<'ctx>(
    emitter: &Emitter<'_, 'ctx>,
    op: BoolOpKind,
    left: &MirExpr,
    right: &MirExpr,
    ty: &Ty,
) -> Scalar<'ctx> {
    let left_scalar = emitter.emit(left);
    let left_truth = emitter.truth(left_scalar);
    let take_left = emitter.new_block("boolop_take_left");
    let eval_right = emitter.new_block("boolop_eval_right");
    let join = emitter.new_block("boolop_join");
    emitter.branch_on_left(op, left_truth, take_left, eval_right);

    emitter.builder.position_at_end(take_left);
    let left_value = arm_value(emitter, op, left, left_scalar, ty);
    let left_end = emitter.current_block();
    emitter.branch_to(join);

    emitter.builder.position_at_end(eval_right);
    release_scalar_if_int_temporary(
        emitter.context,
        emitter.builder,
        emitter.rt,
        left,
        &left_scalar,
    );
    let right_scalar = emitter.emit(right);
    let right_value = owned_value(emitter, right, right_scalar, ty);
    let right_end = emitter.current_block();
    emitter.branch_to(join);

    emitter.builder.position_at_end(join);
    let phi = emitter
        .builder
        .build_phi(
            ty_to_basic_type(emitter.context, ty.clone()),
            "boolop_value",
        )
        .expect("build_phi should not fail for a well-formed value type");
    let (left_basic, right_basic) = (basic_value(left_value), basic_value(right_value));
    phi.add_incoming(&[(&left_basic, left_end), (&right_basic, right_end)]);
    scalar_of(ty, phi.as_basic_value())
}

/// The selected left operand as an owned value of type `ty`. Under `or`, a
/// left `Optional` joined to a bare type is known present here (its truth
/// test just passed), so its payload is extracted and, for `int`, retained.
fn arm_value<'ctx>(
    emitter: &Emitter<'_, 'ctx>,
    op: BoolOpKind,
    left: &MirExpr,
    scalar: Scalar<'ctx>,
    ty: &Ty,
) -> Scalar<'ctx> {
    match (op, left.ty(), scalar) {
        (BoolOpKind::Or, Ty::Optional(inner), Scalar::Optional(value))
            if !matches!(ty, Ty::Optional(_)) =>
        {
            let payload = emitter
                .builder
                .build_extract_value(value, 0, "boolop_payload")
                .expect("build_extract_value should not fail extracting field 0 of an Optional");
            let payload = payload_scalar(&inner, payload);
            if let Scalar::Int(word) = payload {
                emit_bigint_refcount_call(
                    emitter.context,
                    emitter.builder,
                    emitter.rt,
                    word,
                    BigIntRefcount::Retain,
                );
            }
            coerce_scalar_to_type(emitter.context, emitter.builder, payload, ty.clone())
        }
        _ => owned_value(emitter, left, scalar, ty),
    }
}

/// `scalar` (the value of `source`) retained or incref'd when `source` is a
/// borrowed read, then converted to `ty`. The retain is decided before the
/// conversion, on `source`'s own shape.
fn owned_value<'ctx>(
    emitter: &Emitter<'_, 'ctx>,
    source: &MirExpr,
    scalar: Scalar<'ctx>,
    ty: &Ty,
) -> Scalar<'ctx> {
    let scalar =
        retain_if_int_duplicate(emitter.context, emitter.builder, emitter.rt, source, scalar);
    let scalar = incref_if_str_duplicate(emitter.builder, emitter.rt, source, scalar);
    coerce_scalar_to_type(emitter.context, emitter.builder, scalar, ty.clone())
}

/// An `Optional` payload as the `Scalar` its inner type carries. `pycc_hir`
/// restricts every `Optional` inner type to `int`, `float` or `bool`.
fn payload_scalar<'ctx>(inner: &Ty, payload: BasicValueEnum<'ctx>) -> Scalar<'ctx> {
    match inner {
        Ty::Float => Scalar::Float(payload.into_float_value()),
        Ty::Bool => Scalar::Bool(payload.into_int_value()),
        _ => Scalar::Int(payload.into_int_value()),
    }
}

/// The LLVM value a `Scalar` carries.
fn basic_value(scalar: Scalar<'_>) -> BasicValueEnum<'_> {
    match scalar {
        Scalar::Int(v) | Scalar::Bool(v) => v.into(),
        Scalar::Float(v) => v.into(),
        Scalar::Str(v)
        | Scalar::List(v)
        | Scalar::Dict(v)
        | Scalar::Set(v)
        | Scalar::Instance(v)
        | Scalar::Object(v)
        | Scalar::MemoryView(v) => v.into(),
        Scalar::Tuple(v) | Scalar::Optional(v) => v.into(),
    }
}

/// The joined value as the `Scalar` for the node's type. A value-context
/// node's type is one of the shapes `pycc_hir::bool_op_result_ty` yields:
/// `int`, `bool`, `float`, `str`, `Optional[_]` or a class instance, the
/// last of which the final arm handles.
fn scalar_of<'ctx>(ty: &Ty, value: BasicValueEnum<'ctx>) -> Scalar<'ctx> {
    match ty {
        Ty::Int => Scalar::Int(value.into_int_value()),
        Ty::Bool => Scalar::Bool(value.into_int_value()),
        Ty::Float => Scalar::Float(value.into_float_value()),
        Ty::Str => Scalar::Str(value.into_pointer_value()),
        Ty::Optional(_) => Scalar::Optional(value.into_struct_value()),
        _ => Scalar::Instance(value.into_pointer_value()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkwell::AddressSpace;

    /// `basic_value` is total over `Scalar`: every variant yields the LLVM
    /// value it wraps, including the container, object and buffer variants
    /// the checker keeps out of an `and`/`or` operand today.
    #[test]
    fn basic_value_returns_the_wrapped_value_for_every_scalar() {
        let context = Context::create();
        let int = context.i64_type().const_int(7, false);
        let bool_bit = context.i8_type().const_int(1, false);
        let float = context.f64_type().const_float(0.5);
        let pointer = context.ptr_type(AddressSpace::default()).const_null();
        let pair = context.const_struct(&[int.into(), bool_bit.into()], false);
        assert_eq!(basic_value(Scalar::Int(int)), BasicValueEnum::from(int));
        assert_eq!(
            basic_value(Scalar::Bool(bool_bit)),
            BasicValueEnum::from(bool_bit)
        );
        assert_eq!(
            basic_value(Scalar::Float(float)),
            BasicValueEnum::from(float)
        );
        for scalar in [
            Scalar::Str(pointer),
            Scalar::List(pointer),
            Scalar::Dict(pointer),
            Scalar::Set(pointer),
            Scalar::Instance(pointer),
            Scalar::Object(pointer),
            Scalar::MemoryView(pointer),
        ] {
            assert_eq!(basic_value(scalar), BasicValueEnum::from(pointer));
        }
        assert_eq!(basic_value(Scalar::Tuple(pair)), BasicValueEnum::from(pair));
        assert_eq!(
            basic_value(Scalar::Optional(pair)),
            BasicValueEnum::from(pair)
        );
    }
}
