//! `print()` argument rendering and f-string interpolation (moved out of
//! `lib.rs`, AGENTS.md's "Keep source files decomposable"; the tracker for
//! the rest of `lib.rs` is #545).
//!
//! The two places compiled code turns an arbitrary value into the text it
//! writes: `print`'s two-phase evaluate-then-write pipeline (#145) and one
//! `{expr}` part of an f-string.

use std::collections::HashMap;

use inkwell::context::Context;
use inkwell::values::PointerValue;
use pycc_mir::MirExpr;

use crate::{
    RtFns, StorageSlot, UserFunction, emit_expr, emit_string_literal, incref_if_str_duplicate,
    release_scalar_if_int_temporary, to_str,
};

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
pub(super) fn emit_eval_print_arg<'ctx>(
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
/// `int`/`float`/`bool`, or the incref'd duplicate of a borrowed `str` read
/// as classified by `str_value_is_a_duplicate_reference`). `emit_stmt`'s `print`-call arm calls this once per
/// argument in its second loop, after `emit_eval_print_arg` has already
/// evaluated every argument, so that output happens only after all
/// argument side effects complete (see `emit_eval_print_arg`'s own doc
/// comment for the two-phase design and the `cargo llvm-cov`
/// region-attribution artifact that keeps this an extracted helper).
pub(super) fn emit_write_print_arg<'ctx>(
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

/// The body of `emit_stmt`'s `print`-call arm: two-phase
/// evaluate-then-output (fixes #145). Evaluates *all* arguments
/// left-to-right before emitting any output, so a later argument's side
/// effects (e.g. a user function that itself calls `print`) complete before
/// this `print`'s own output begins -- matching CPython's left-to-right
/// argument-evaluation semantics. See [`emit_eval_print_arg`]/
/// [`emit_write_print_arg`] for the phase split and the `cargo llvm-cov`
/// region-attribution artifact that keeps them as extracted helpers.
pub(super) fn emit_print_call<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    args: &[MirExpr],
) {
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
}

/// One `{expr}` part of `emit_expr`'s `MirExpr::FString` arm: evaluates the
/// interpolated expression and yields its text as a fresh `str` pointer the
/// caller concatenates.
pub(super) fn emit_fstring_interpolation<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    user_functions: &HashMap<&str, UserFunction<'ctx>>,
    locals: &HashMap<String, StorageSlot<'ctx>>,
    inner: &MirExpr,
) -> PointerValue<'ctx> {
    if inner.ty() == pycc_mir::Ty::None {
        emit_expr(context, builder, module, rt, user_functions, locals, inner);
        emit_string_literal(context, builder, module, rt, "None")
    } else {
        let scalar = emit_expr(context, builder, module, rt, user_functions, locals, inner);
        let scalar = incref_if_str_duplicate(builder, rt, inner, scalar);
        let as_str = to_str(builder, rt, scalar);
        // #146 Part 2 (D-181): same pure-consumer shape
        // as `print`'s own argument above, and released
        // after `to_str` for the same reason.
        release_scalar_if_int_temporary(context, builder, rt, inner, &scalar);
        as_str
    }
}
