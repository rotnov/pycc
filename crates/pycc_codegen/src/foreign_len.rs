//! Emission for `len(o)`, truth testing and `float(o)` on a CPython object
//! value (Part 3 of #1026, PR 3a of #1082; Part 4 of #1026, PR 4a of #1083).
//!
//! The third sibling of `foreign_attr.rs` and `foreign_call.rs`, carved out
//! of `lib.rs` for the same reason (AGENTS.md's "Keep source files
//! decomposable"; the tracker for the rest of `lib.rs` is #545). The two
//! operations share a module because they share everything that matters at
//! this layer: each is one call to a fixed shim helper that answers a
//! *scalar* rather than a `PyObject *`, and each reports failure as `-1`
//! rather than as `NULL`, so none can reuse `foreign_call.rs`'s
//! `fail_on_null`. PR 4a's `float(o)` is the third member of that family and
//! lives here for the same reason; `bool(o)` needs no emitter of its own at
//! all, because it is exactly [`emit_truthy`] widened to a `Scalar::Bool`.
//!
//! **Ownership** (`docs/RUNTIME.md`). None of these helpers lets a reference
//! escape -- `pycc_ext_obj_len` answers a D-141 encoded `int` word,
//! `pycc_ext_obj_truthy` answers a C `int`, and `pycc_ext_obj_to_float`
//! answers a `double` after releasing the temporary `PyNumber_Float` handed
//! it, on *every* exit rather than only the successful one -- and none
//! touches its operand's refcount. So unlike an attribute load or a method
//! call, none of these adds anything to the #1092 leak-only set: there is
//! nothing to leak in compiled code and nothing left for it to release.
//!
//! **Failure** (`docs/RUNTIME.md`). Every helper here leaves *CPython's*
//! error indicator set, which pycc's own pending-exception guard (D-173) cannot
//! see, so each arm below emits its own check and returns
//! [`EXT_MODULE_EXEC_FAILED`] from the module-exec entry point -- exactly
//! `foreign_attr::emit`'s answer to the same question, and for exactly its
//! reasons. #1096 tracks the fact that such an edge bypasses pycc's MIR
//! exception target and so cannot be caught by a module-scope `try`.

use super::*;
use crate::foreign_attr::{expect_module_exec_entry, expect_object_pointer};
use inkwell::builder::Builder;
use inkwell::values::{IntValue, PointerValue};

/// Declares the shim's `int pycc_ext_obj_len(PyObject *, long long *)` once
/// per module, returning the existing declaration on every later call --
/// `foreign_attr.rs`'s `obj_getattr_fn` pattern exactly, and for the same
/// reason: a second declaration of one name is an LLVM module-verifier
/// error.
fn obj_len_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_LEN_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    module.add_function(
        EXT_OBJ_LEN_SYMBOL,
        context.i32_type().fn_type(&[ptr.into(), ptr.into()], false),
        None,
    )
}

/// Declares the shim's `int pycc_ext_obj_truthy(PyObject *)` once per
/// module, on [`obj_len_fn`]'s pattern and for its reason.
fn obj_truthy_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_TRUTHY_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    module.add_function(
        EXT_OBJ_TRUTHY_SYMBOL,
        context.i32_type().fn_type(&[ptr.into()], false),
        None,
    )
}

/// Declares the shim's `int pycc_ext_obj_to_float(PyObject *, double *)`
/// once per module, on [`obj_len_fn`]'s pattern and for its reason.
fn obj_to_float_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_TO_FLOAT_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    module.add_function(
        EXT_OBJ_TO_FLOAT_SYMBOL,
        context.i32_type().fn_type(&[ptr.into(), ptr.into()], false),
        None,
    )
}

/// Routes a negative `status` to the module-exec failure edge, leaving the
/// builder positioned on the success continuation.
///
/// The scalar counterpart of `foreign_call.rs`'s `fail_on_null`: both shim
/// helpers here report failure as `-1` with CPython's exception already set,
/// so the test is `status < 0` rather than a null check. It is a *signed*
/// comparison against zero rather than an equality test against `-1` so that
/// any future negative status is fail-closed; `pycc_ext_obj_truthy`'s
/// success values are `0` and `1`, and `pycc_ext_obj_len`'s are `0` alone.
fn fail_on_negative<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    entry_fn: FunctionValue<'ctx>,
    status: IntValue<'ctx>,
    label: &str,
) {
    let failed = builder
        .build_int_compare(
            inkwell::IntPredicate::SLT,
            status,
            context.i32_type().const_zero(),
            &format!("{label}_failed"),
        )
        .expect("build_int_compare should not fail");
    let fail_bb = context.append_basic_block(entry_fn, &format!("{label}_fail"));
    let cont_bb = context.append_basic_block(entry_fn, &format!("{label}_cont"));
    builder
        .build_conditional_branch(failed, fail_bb, cont_bb)
        .expect("build_conditional_branch should not fail");
    builder.position_at_end(fail_bb);
    builder
        .build_return(Some(
            &context
                .i64_type()
                .const_int(EXT_MODULE_EXEC_FAILED as u64, true),
        ))
        .expect("build_return should not fail");
    builder.position_at_end(cont_bb);
}

/// Allocates one `slot_ty` out-slot named `name` in the *entry block* of
/// `entry_fn`, leaving the builder positioned exactly where it was.
///
/// The single-slot twin of `foreign_call.rs`'s `alloca_in_entry_block`, and
/// it exists for that function's documented reason rather than for tidiness:
/// an `alloca` is reclaimed only when its function returns, so emitting one
/// at the call site would make `while c: n = len(gc)` at module scope grow
/// the hosting interpreter's stack without bound.
fn out_slot_in_entry_block<'ctx>(
    builder: &Builder<'ctx>,
    entry_fn: FunctionValue<'ctx>,
    slot_ty: impl inkwell::types::BasicType<'ctx>,
    name: &str,
) -> PointerValue<'ctx> {
    let resume_at = builder
        .get_insert_block()
        .expect("the builder is positioned inside a block");
    let entry_block = entry_fn
        .get_first_basic_block()
        .expect("a function being emitted into has an entry block");
    // Never empty: a foreign object exists only because an `import` bound
    // it, and that import's own call was emitted into this block before any
    // expression could read the binding.
    let first = entry_block
        .get_first_instruction()
        .expect("the module-exec entry block already holds the foreign import's own call");
    builder.position_before(&first);
    let slot = builder
        .build_alloca(slot_ty, name)
        .expect("build_alloca should not fail");
    builder.position_at_end(resume_at);
    slot
}

/// Emits one `len(o)` against a CPython object and yields the length as an
/// already-D-141-encoded [`Scalar::Int`].
///
/// The encode happens inside the shim rather than here, which is the whole
/// reason this is not a `Ty::Object` arm bolted onto `emit_expr`'s scalar
/// `len` dispatch: that path produces a *raw* `i64` and re-tags it at the
/// call site, while `pycc_ext_obj_len` hands back a finished word through an
/// out-parameter. Fusing `PyObject_Size` and `pycc_rt_ext_int_encode` behind
/// one `-1` return also means one failure edge is emitted here instead of
/// two -- see the C side's own comment for why the encode arm is unreachable
/// for a real container.
///
/// # Why the enclosing function is always the module-exec entry
///
/// `expect_module_exec_entry` asserts it, before any block is appended, on
/// exactly `foreign_attr::emit`'s reasoning: `pycc_types` refuses reading a
/// foreign object inside a function body (`I0404`), so the failure edge's
/// `ret i64 -1` is always emitted into a function that returns `i64`.
pub(super) fn emit_len<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    base: Scalar<'ctx>,
) -> Scalar<'ctx> {
    let entry_fn = expect_module_exec_entry(builder);
    let len_fn = obj_len_fn(context, module);
    let base_ptr = expect_object_pointer(base);
    let out = out_slot_in_entry_block(builder, entry_fn, context.i64_type(), "foreign_len_out");
    let status = builder
        .build_call(len_fn, &[base_ptr.into(), out.into()], "foreign_len")
        .expect("build_call should not fail for pycc_ext_obj_len")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_len returns int")
        .into_int_value();
    fail_on_negative(context, builder, entry_fn, status, "foreign_len");
    let encoded = builder
        .build_load(context.i64_type(), out, "foreign_len_value")
        .expect("build_load should not fail")
        .into_int_value();
    Scalar::Int(encoded)
}

/// Emits one truth test against a CPython object and yields the `i1` an
/// `if`/`while`/comprehension-guard branch consumes.
///
/// Called from `lib.rs`'s `truthy`, whose `Scalar::Object` arm used to
/// panic: `pycc_types` refused a `Ty::Object` condition outright, precisely
/// *because* codegen had no answer here. PR 3a deleted those ten refusals
/// (`crates/pycc_types/src/foreign.rs`'s module documentation records it),
/// so this is the answer they were waiting on.
///
/// `PyObject_IsTrue` runs the operand's own `__bool__` or `__len__`, so it
/// can raise; the `-1` status takes the module-exec failure edge. The
/// success values are `0` and `1`, which the truncation to `i1` below maps
/// exactly.
///
/// The module-exec entry assertion is [`emit_len`]'s, unchanged. The one
/// `truthy` call site it does *not* cover is `MirExpr::Not` -- `not o` never
/// reaches here, because `pycc_types`' `unop.rs` answers `T0021` for a
/// non-`bool` operand, before and after PR 3a alike.
pub(super) fn emit_truthy<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    object: PointerValue<'ctx>,
) -> IntValue<'ctx> {
    let entry_fn = expect_module_exec_entry(builder);
    let truthy_fn = obj_truthy_fn(context, module);
    let status = builder
        .build_call(truthy_fn, &[object.into()], "foreign_truthy")
        .expect("build_call should not fail for pycc_ext_obj_truthy")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_truthy returns int")
        .into_int_value();
    fail_on_negative(context, builder, entry_fn, status, "foreign_truthy");
    builder
        .build_int_truncate(status, context.bool_type(), "foreign_truthy_bit")
        .expect("build_int_truncate should not fail")
}

/// Emits one `float(o)` against a CPython object and yields the converted
/// value as a [`Scalar::Float`].
///
/// `lib.rs`'s `to_float` panics on a `Scalar::Object` -- it knows only how
/// to widen pycc's own scalars -- so the `float` builtin's emission branches
/// on the operand before reaching it and lands here instead.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// D-244 rule 7 closes the type boundary at the *thunk export seam*, where
/// a value crosses implicitly and the annotation is the whole contract.
/// `float(o)` names its destination type in user source, so running
/// CPython's own `PyNumber_Float` protocol behind [`EXT_OBJ_TO_FLOAT_SYMBOL`]
/// is exactly what the author asked for. The same paragraph is recorded on
/// that constant, in the helper's C comment, and in `docs/TYPE_SYSTEM.md`'s
/// `object` row.
///
/// The shim releases the reference `PyNumber_Float` produces on every exit,
/// so nothing here adds to the #1092 leak-only set. Failure is `-1` with
/// CPython's error indicator set, taking [`fail_on_negative`]'s module-exec
/// failure edge -- `PyNumber_Float` raises `TypeError` for an operand with
/// no conversion and `ValueError` for an unparseable string, neither of
/// which pycc can rule out at compile time for an opaque pointee.
///
/// The module-exec entry assertion is [`emit_len`]'s, unchanged.
pub(super) fn emit_to_float<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    base: Scalar<'ctx>,
) -> Scalar<'ctx> {
    let entry_fn = expect_module_exec_entry(builder);
    let to_float_fn = obj_to_float_fn(context, module);
    let base_ptr = expect_object_pointer(base);
    let out = out_slot_in_entry_block(
        builder,
        entry_fn,
        context.f64_type(),
        "foreign_to_float_out",
    );
    let status = builder
        .build_call(
            to_float_fn,
            &[base_ptr.into(), out.into()],
            "foreign_to_float",
        )
        .expect("build_call should not fail for pycc_ext_obj_to_float")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_to_float returns int")
        .into_int_value();
    fail_on_negative(context, builder, entry_fn, status, "foreign_to_float");
    let value = builder
        .build_load(context.f64_type(), out, "foreign_to_float_value")
        .expect("build_load should not fail")
        .into_float_value();
    Scalar::Float(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileOptions, EXT_MODULE_EXEC_SYMBOL, compile_to_object_with_observer};
    use inkwell::values::AnyValue;
    use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

    /// `import <module>` followed by the items `build` makes from a read of
    /// that module's binding.
    fn program(module: &str, build: impl Fn(MirExpr) -> Vec<MirStmt>) -> Vec<MirItem> {
        let mut items = vec![MirItem::ForeignImport {
            local_name: module.to_string(),
            module_path: module.to_string(),
        }];
        items.extend(
            build(MirExpr::Name {
                name: module.to_string(),
                ty: Ty::Object,
            })
            .into_iter()
            .map(MirItem::TopLevelStmt),
        );
        items
    }

    /// `x = <conversion>(<module>)` at module scope, for the Part 4 arms.
    ///
    /// An assignment rather than a discarded expression statement so the
    /// converted value is actually consumed, which is what forces the load
    /// out of the out-slot to be emitted.
    fn convert(module: &str, callee: &str, ty: Ty) -> Vec<MirItem> {
        program(module, |base| {
            vec![MirStmt::Assign {
                target: "converted".to_string(),
                value: MirExpr::Call {
                    callee: callee.to_string(),
                    args: vec![base],
                    ty: ty.clone(),
                },
            }]
        })
    }

    /// One discarded `len(<module>)`.
    fn len_of(module: &str) -> Vec<MirItem> {
        program(module, |base| {
            vec![MirStmt::ExprStmt(MirExpr::ObjLen {
                base: Box::new(base),
            })]
        })
    }

    /// The LLVM text of the module-exec entry point after compiling `items`
    /// as an `ext` object -- `foreign_attr.rs`'s own `entry_ir`, which is
    /// where the rationale for compiling all the way to an object file
    /// (LLVM's verifier runs before any assertion is believed) lives.
    fn entry_ir(label: &str, items: Vec<MirItem>) -> String {
        let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
        let mut ir = String::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            if let Some(entry) = module.get_function(EXT_MODULE_EXEC_SYMBOL) {
                ir = crate::llvm_string_to_owned(entry.print_to_string());
            }
        };
        compile_to_object_with_observer(
            &MirModule {
                items,
                ..Default::default()
            },
            &dir.join(format!("{label}.o")),
            &CompileOptions {
                ext: true,
                ..CompileOptions::default()
            },
            Some(&mut observer),
        )
        .expect("ext codegen should succeed");
        assert!(!ir.is_empty(), "no {EXT_MODULE_EXEC_SYMBOL} was emitted");
        ir
    }

    /// How many times `needle` occurs in `haystack`.
    fn occurrences(haystack: &str, needle: &str) -> usize {
        let mut count = 0usize;
        let mut rest = haystack;
        while let Some(at) = rest.find(needle) {
            count += 1;
            rest = &rest[at + needle.len()..];
        }
        count
    }

    /// The call goes to the shared constant's symbol.
    ///
    /// Asserted through [`EXT_OBJ_LEN_SYMBOL`] rather than against a literal
    /// for `foreign_attr.rs`'s reason: the C definition and this declaration
    /// resolve lazily at load time, so a literal spelled twice would be a
    /// crash at first call rather than a link error.
    #[test]
    fn a_foreign_len_calls_the_shim_helper_by_its_shared_symbol() {
        let ir = entry_ir("foreign_len_call", len_of("numpy"));
        assert!(ir.contains(EXT_OBJ_LEN_SYMBOL), "{ir}");
    }

    /// A raising `PyObject_Size` stops the module body on the module-exec
    /// failure edge rather than continuing with an unwritten out-slot.
    #[test]
    fn a_failed_foreign_len_returns_on_the_module_exec_failure_edge() {
        let ir = entry_ir("foreign_len_fail_edge", len_of("numpy"));
        assert!(ir.contains("foreign_len_failed"), "{ir}");
        assert!(ir.contains("foreign_len_fail:"), "{ir}");
        assert!(ir.contains("foreign_len_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// The out-slot `alloca` is hoisted into the entry block, so a
    /// module-scope loop around a `len` does not grow the host's stack.
    ///
    /// Asserted positionally: the entry block is everything up to the first
    /// appended label, and the `alloca` must be inside it.
    #[test]
    fn the_out_slot_alloca_is_hoisted_into_the_entry_block() {
        let ir = entry_ir(
            "foreign_len_in_loop",
            program("numpy", |base| {
                vec![MirStmt::While {
                    test: MirExpr::BoolLiteral(false),
                    body: vec![MirStmt::ExprStmt(MirExpr::ObjLen {
                        base: Box::new(base),
                    })],
                }]
            }),
        );
        let alloca_at = ir
            .find("foreign_len_out = alloca")
            .unwrap_or_else(|| panic!("no out-slot alloca: {ir}"));
        let first_label_at = ir
            .find("\n\n")
            .unwrap_or_else(|| panic!("no second basic block: {ir}"));
        assert!(alloca_at < first_label_at, "{ir}");
    }

    /// Two `len` calls in one module share one extern declaration and one
    /// out-slot per call site.
    ///
    /// `obj_len_fn` returns the existing `FunctionValue` on every call after
    /// the first; a second `add_function` of one name is an LLVM
    /// module-verifier error, so the second `len` is what proves the early
    /// return is taken rather than merely present.
    #[test]
    fn a_second_foreign_len_reuses_the_one_extern_declaration() {
        let mut items = len_of("numpy");
        items.extend(len_of("scipy"));
        let ir = entry_ir("foreign_len_twice", items);
        assert_eq!(
            occurrences(&ir, EXT_OBJ_LEN_SYMBOL),
            2,
            "one call site per len: {ir}"
        );
    }

    /// An `if` on a CPython object calls the truth-testing helper and takes
    /// the module-exec failure edge when it raises.
    #[test]
    fn a_foreign_condition_calls_the_shim_helper_and_can_fail() {
        let ir = entry_ir(
            "foreign_truthy_if",
            program("numpy", |base| {
                vec![MirStmt::If {
                    test: base,
                    body: vec![],
                    orelse: vec![],
                }]
            }),
        );
        assert!(ir.contains(EXT_OBJ_TRUTHY_SYMBOL), "{ir}");
        assert!(ir.contains("foreign_truthy_failed"), "{ir}");
        assert!(ir.contains("foreign_truthy_fail:"), "{ir}");
        assert!(ir.contains("foreign_truthy_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// Every one of the five condition-position sites reaches the shim.
    ///
    /// These are exactly the sites `pycc_types` used to refuse outright
    /// (`reject_object_condition`'s ten call sites, five shapes checked in
    /// both a module body and a function body). `lib.rs` reaches `truthy`
    /// from each through a different `emit_stmt` arm, so one shape passing
    /// says nothing about the other four; the comprehension guards in
    /// particular sit behind their own loop scaffolding.
    ///
    /// `tests/issue_1082_foreign_len_and_truth.rs` asserts the front-end
    /// half of the same claim -- that each shape now type-checks at all.
    /// Builds the statement list that places a condition expression at one
    /// particular condition-position site, so the five shapes can be driven
    /// from a single table.
    type ConditionShape = fn(MirExpr) -> Vec<MirStmt>;

    #[test]
    fn every_condition_position_site_reaches_the_shim_helper() {
        let sites: [(&str, ConditionShape); 5] = [
            ("if", |test| {
                vec![MirStmt::If {
                    test,
                    body: vec![],
                    orelse: vec![],
                }]
            }),
            ("while", |test| vec![MirStmt::While { test, body: vec![] }]),
            ("listcomp", |test| {
                vec![MirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: pycc_mir::CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(3),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: Some(Box::new(test)),
                    elt: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }]
            }),
            ("setcomp", |test| {
                vec![MirStmt::SetCompAssign {
                    target: "ys".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: pycc_mir::CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(3),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: Some(Box::new(test)),
                    elt: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }]
            }),
            ("dictcomp", |test| {
                vec![MirStmt::DictCompAssign {
                    target: "zs".to_string(),
                    var: "i".to_string(),
                    var_ty: Ty::Int,
                    source: pycc_mir::CompSource::Range {
                        start: MirExpr::IntLiteral(0),
                        stop: MirExpr::IntLiteral(3),
                        step: MirExpr::IntLiteral(1),
                    },
                    cond: Some(Box::new(test)),
                    key: Box::new(MirExpr::StringLiteral("k".to_string())),
                    value: Box::new(MirExpr::Name {
                        name: "i".to_string(),
                        ty: Ty::Int,
                    }),
                }]
            }),
        ];
        for (label, build) in sites {
            let ir = entry_ir(
                &format!("foreign_truthy_site_{label}"),
                program("numpy", build),
            );
            assert!(ir.contains(EXT_OBJ_TRUTHY_SYMBOL), "{label}: {ir}");
            assert!(ir.contains("foreign_truthy_fail:"), "{label}: {ir}");
        }
    }

    /// Two foreign conditions in one module share one extern declaration --
    /// `obj_truthy_fn`'s early return, proved the way `obj_len_fn`'s is.
    #[test]
    fn a_second_foreign_condition_reuses_the_one_extern_declaration() {
        let ir = entry_ir(
            "foreign_truthy_twice",
            program("numpy", |base| {
                vec![
                    MirStmt::If {
                        test: base.clone(),
                        body: vec![],
                        orelse: vec![],
                    },
                    MirStmt::While {
                        test: base,
                        body: vec![],
                    },
                ]
            }),
        );
        assert_eq!(
            occurrences(&ir, EXT_OBJ_TRUTHY_SYMBOL),
            2,
            "one call site per condition: {ir}"
        );
    }

    /// `float(o)` reaches the Part 4 shim helper rather than `lib.rs`'s
    /// `to_float`, which panics on a `Scalar::Object`.
    ///
    /// Asserted through [`EXT_OBJ_TO_FLOAT_SYMBOL`] rather than against a
    /// literal for the lazy-link reason that constant records.
    #[test]
    fn a_foreign_float_conversion_calls_the_shim_helper_by_its_shared_symbol() {
        let ir = entry_ir(
            "foreign_to_float_call",
            convert("numpy", "float", Ty::Float),
        );
        assert!(ir.contains(EXT_OBJ_TO_FLOAT_SYMBOL), "{ir}");
    }

    /// A raising `PyNumber_Float` stops the module body on the module-exec
    /// failure edge rather than continuing with an unwritten out-slot.
    #[test]
    fn a_failed_foreign_float_conversion_returns_on_the_module_exec_failure_edge() {
        let ir = entry_ir(
            "foreign_to_float_fail_edge",
            convert("numpy", "float", Ty::Float),
        );
        assert!(ir.contains("foreign_to_float_failed"), "{ir}");
        assert!(ir.contains("foreign_to_float_fail:"), "{ir}");
        assert!(ir.contains("foreign_to_float_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// The conversion's out-slot is a `double` hoisted into the entry block,
    /// so a module-scope loop around a `float(o)` does not grow the host's
    /// stack -- `the_out_slot_alloca_is_hoisted_into_the_entry_block`'s claim
    /// for the second slot type `out_slot_in_entry_block` now serves.
    #[test]
    fn the_float_conversion_out_slot_is_a_double_in_the_entry_block() {
        let ir = entry_ir(
            "foreign_to_float_in_loop",
            program("numpy", |base| {
                vec![MirStmt::While {
                    test: MirExpr::BoolLiteral(false),
                    body: vec![MirStmt::Assign {
                        target: "converted".to_string(),
                        value: MirExpr::Call {
                            callee: "float".to_string(),
                            args: vec![base],
                            ty: Ty::Float,
                        },
                    }],
                }]
            }),
        );
        let alloca_at = ir
            .find("foreign_to_float_out = alloca double")
            .unwrap_or_else(|| panic!("no double out-slot alloca: {ir}"));
        let first_label_at = ir
            .find("\n\n")
            .unwrap_or_else(|| panic!("no second basic block: {ir}"));
        assert!(alloca_at < first_label_at, "{ir}");
    }

    /// Two `float(o)` conversions in one module share one extern
    /// declaration -- `obj_to_float_fn`'s early return.
    ///
    /// The needle carries the call's own `(` because the bare symbol name
    /// cannot tell the two outcomes apart: `LLVMAddFunction` does not reject
    /// a duplicate name, it renames the second declaration to
    /// `@pycc_ext_obj_to_float.1`, which still contains the bare symbol as a
    /// substring. Counting `@pycc_ext_obj_to_float(` instead counts only the
    /// call sites that reach the *first* declaration, so dropping the early
    /// return leaves one of the two conversions calling the renamed
    /// duplicate and the count falls to 1.
    #[test]
    fn a_second_foreign_float_conversion_reuses_the_one_extern_declaration() {
        let mut items = convert("numpy", "float", Ty::Float);
        items.extend(convert("scipy", "float", Ty::Float));
        let ir = entry_ir("foreign_to_float_twice", items);
        assert_eq!(
            occurrences(&ir, &format!("@{EXT_OBJ_TO_FLOAT_SYMBOL}(")),
            2,
            "one call site per conversion, both on the one declaration: {ir}"
        );
    }

    /// `bool(o)` adds no symbol of its own: it is PR 3a's truth test widened
    /// to the `i8` a `Scalar::Bool` carries, and it inherits that helper's
    /// module-exec failure edge unchanged.
    #[test]
    fn a_foreign_bool_conversion_reuses_the_truth_testing_helper() {
        let ir = entry_ir("foreign_bool_call", convert("numpy", "bool", Ty::Bool));
        assert!(ir.contains(EXT_OBJ_TRUTHY_SYMBOL), "{ir}");
        assert!(!ir.contains(EXT_OBJ_TO_FLOAT_SYMBOL), "{ir}");
        assert!(ir.contains("foreign_truthy_fail:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
        assert!(ir.contains("bool_from_object"), "{ir}");
    }
}
