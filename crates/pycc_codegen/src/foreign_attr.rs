//! Emission for `MirExpr::ObjAttrGet` (Part 2 of #1026, PR 2a of #1081).
//!
//! The counterpart to `foreign_import.rs`: that module knows how a foreign
//! `import numpy` becomes a module object in a global, this one knows how
//! `numpy.pi` becomes a `PyObject_GetAttrString` call on it. Carved out of
//! `lib.rs` for the same reason (AGENTS.md's "Keep source files
//! decomposable"; the tracker for the rest of `lib.rs` is #545), and kept
//! separate from `foreign_import.rs` because the two sit at different
//! layers -- an *item* emitted once from `compile_to_object`'s item loop
//! versus an *expression* emitted wherever `emit_expr` reaches it.
//!
//! **Ownership** (`docs/RUNTIME.md`). `pycc_ext_obj_getattr` returns a new
//! reference and nothing here releases it, matching the module object's own
//! leak-only rule. Unlike the module object, though, an attribute load sits
//! inside ordinary control flow, so a load in a loop leaks once per
//! iteration -- the leak is trip-count-linear rather than once per process.
//! Part 2 accepts that deliberately: D-244 rule 6's kill criterion is a
//! speed measurement on a hot function, and a release protocol needs the
//! borrow/own distinction that only arrives with `MirExpr::ObjMethodCall`'s
//! argument marshalling. The deferral is recorded in `docs/RUNTIME.md`.

use super::*;
use inkwell::builder::Builder;

/// Declares the shim's
/// `PyObject *pycc_ext_obj_getattr(PyObject *, const char *)` once per
/// module, returning the existing declaration on every later call --
/// `foreign_import.rs`'s `obj_import_fn` pattern exactly, and for the same
/// reason: a second declaration of one name is an LLVM module-verifier
/// error.
fn obj_getattr_fn<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(EXT_OBJ_GETATTR_SYMBOL) {
        return existing;
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    module.add_function(
        EXT_OBJ_GETATTR_SYMBOL,
        ptr.fn_type(&[ptr.into(), ptr.into()], false),
        None,
    )
}

/// The object operand of a foreign-object operation -- an attribute load's
/// or method call's base, a direct call's callee, a subscript's base, or a
/// `for` loop's iterable -- as a `PyObject *`.
///
/// `pycc_mir`'s lowering builds each of those nodes only where the
/// operand's inferred type is `Ty::Object` (for example `pycc_mir::expr`'s
/// `HirExpr::AttrGet` arm, and its `HirExpr::Call` arm for `ObjCall`,
/// #1313), and `ty_to_basic_type` maps that to a pointer, so every other
/// `Scalar` here is a lowering defect rather than a program the front end
/// let through.
pub(super) fn expect_object_pointer(scalar: Scalar<'_>) -> PointerValue<'_> {
    let Scalar::Object(ptr) = scalar else {
        panic!(
            "pycc_codegen: internal error: a foreign-object operand did not evaluate to a \
             CPython object -- pycc_mir lowers every foreign-object operation only for a \
             `Ty::Object` operand"
        )
    };
    ptr
}

/// Emits one `obj.attr` load against a CPython object and yields the
/// attribute's value as an opaque [`Scalar::Object`].
///
/// # Which side owns the "CPython raised" transition
///
/// `pycc_ext_obj_getattr` returns `NULL` with *CPython's* error indicator
/// set. That is a second failure protocol next to pycc's own pending state
/// (D-173, `pycc_rt_exception_active`), and the standard pending-exception
/// guard `emit_expr` emits right after this call -- `expression_can_set_exception`
/// answers `true` for this node -- reads pycc's state, which a CPython-set
/// exception leaves untouched. It therefore takes the no-exception edge and
/// cannot stop the module body on its own. PR 2a's own review found what
/// that costs: `import numpy` followed by `numpy.definitely_not_there` ran
/// the whole module body to completion and CPython reported `SystemError:
/// execution of module n raised unreported exception` instead of the real
/// `AttributeError`.
///
/// So this arm emits the `NULL` check itself and routes the failure to the
/// **module-exec failure edge** `foreign_import.rs` already uses for a
/// failed `pycc_ext_obj_import`: return [`EXT_MODULE_EXEC_FAILED`]
/// immediately, leaving CPython's own exception set and unmodified. The
/// remaining statements never run, and the interpreter reports the real
/// exception. Bridging the two protocols -- naming the exception in
/// `pycc_rt::exception`, which carries no `AttributeError` tag -- stays PR
/// 2b's, together with `MirExpr::ObjMethodCall`; nothing here needs pycc to
/// name it.
///
/// # Why the enclosing function is always the module-exec entry
///
/// That failure edge exists only inside `pycc_ext_module_exec`, so this
/// asserts it is the function being emitted into. PR 2a's review settled
/// the reachability question with evidence rather than argument: `import
/// numpy` + `def _helper(): return numpy.pi` + `_helper()` built *and ran*
/// on this branch, so an `ObjAttrGet` really could be emitted inside a
/// function body, where returning an `i64` from a function of another
/// return type would not even verify. The answer is at the `pycc_types`
/// layer -- `expr.rs`'s `HirExpr::Name` arm refuses reading a foreign
/// object inside a function body at all (`docs/TYPE_SYSTEM.md`), which PR
/// 2a can afford because it ships no user-visible capability. That leaves
/// exactly two admitted shapes, both at module scope: a discarded
/// `ExprStmt`, and the base of a *further* `ObjAttrGet`, because `a.b.c`
/// nests this node inside itself. The nested case is answered twice over
/// now -- this check stops the outer load from ever seeing the inner
/// `NULL`, and the shim's own NULL-`obj` guard in
/// `src/ext/pycc_ext_module.c` still holds the line for any caller that
/// reaches it another way.
pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    base: Scalar<'ctx>,
    attr: &str,
) -> Scalar<'ctx> {
    let entry_fn = expect_module_exec_entry(builder);
    let getattr = obj_getattr_fn(context, module);
    let base_ptr = expect_object_pointer(base);
    let name = builder
        .build_global_string_ptr(attr, &format!("pycc_foreign_attr_{attr}"))
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let loaded = builder
        .build_call(getattr, &[base_ptr.into(), name.into()], "foreign_attr")
        .expect("build_call should not fail for pycc_ext_obj_getattr")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_getattr returns PyObject *")
        .into_pointer_value();
    let failed = builder
        .build_is_null(loaded, "foreign_attr_failed")
        .expect("build_is_null should not fail");
    let fail_bb = context.append_basic_block(entry_fn, "foreign_attr_fail");
    let cont_bb = context.append_basic_block(entry_fn, "foreign_attr_cont");
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
    Scalar::Object(loaded)
}

/// The function `builder` is currently emitting into, once it is known to
/// be the `--ext` module-body entry point.
///
/// The guard runs *before* any block is appended: the failure edge returns
/// `i64 -1`, so emitting it into a function with a different return type
/// would be an LLVM verifier error rather than a diagnosable one. See
/// [`emit`]'s own doc comment for why every admitted `ObjAttrGet` really is
/// emitted here -- it is a `pycc_types` refusal, so reaching this arm from
/// anywhere else is a front-end defect.
///
/// Shared by every foreign-object operation that has a failure edge, not
/// just the attribute load this module owns: `foreign_call.rs`'s method
/// call and `foreign_len.rs`'s `len` and truth test call it too, on the
/// identical reasoning and behind the identical `pycc_types` refusal. The
/// message therefore names the class of operation rather than one of them.
pub(super) fn expect_module_exec_entry<'ctx>(builder: &Builder<'ctx>) -> FunctionValue<'ctx> {
    let function = builder
        .get_insert_block()
        .expect("the builder is positioned inside a block")
        .get_parent()
        .expect("every basic block belongs to a function");
    if function.get_name().to_bytes() != EXT_MODULE_EXEC_SYMBOL.as_bytes() {
        panic!(
            "pycc_codegen: internal error: an operation on a CPython object was emitted \
             outside `{EXT_MODULE_EXEC_SYMBOL}` -- pycc_types refuses reading a CPython \
             object anywhere but a module body, and only the module-exec entry has the \
             failure edge a failed operation takes"
        )
    }
    function
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileOptions, EXT_MODULE_EXEC_SYMBOL, compile_to_object_with_observer};
    use inkwell::values::AnyValue;
    use pycc_mir::{MirExpr, MirItem, MirModule, MirStmt, Ty};

    /// `import <module>` followed by one discarded `<module>.<attr>` load.
    ///
    /// A discarded `ExprStmt` is used rather than an assignment because it
    /// is the shape PR 2a actually admits end to end: `pycc_types` refuses
    /// binding a CPython object to a name (`I0404`), so no type-checked
    /// program can produce an `ObjAttrGet` in any other statement position.
    fn load(module: &str, attr: &str) -> Vec<MirItem> {
        vec![
            MirItem::ForeignImport {
                local_name: module.to_string(),
                module_path: module.to_string(),
                from: None,
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjAttrGet {
                base: Box::new(MirExpr::Name {
                    name: module.to_string(),
                    ty: Ty::Object,
                }),
                attr: attr.to_string(),
                ty: Ty::Object,
            })),
        ]
    }

    /// The LLVM text of the module-exec entry point after compiling `items`
    /// as an `ext` object -- `foreign_import.rs`'s own `entry_ir`, which is
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

    /// The call goes to the shared constant's symbol, and the attribute
    /// name reaches it as a global string.
    ///
    /// The symbol is asserted through [`EXT_OBJ_GETATTR_SYMBOL`] rather
    /// than against a literal on purpose: the C definition in
    /// `src/ext/pycc_ext_module.c` and this declaration are resolved
    /// lazily at load time, so a literal spelled twice would be a crash at
    /// first call rather than a link error, and a test that spelled it a
    /// third time would agree with neither.
    #[test]
    fn a_foreign_attribute_load_calls_the_shim_helper_by_its_shared_symbol() {
        let ir = entry_ir("foreign_attr_call", load("numpy", "pi"));
        assert!(ir.contains(EXT_OBJ_GETATTR_SYMBOL), "{ir}");
        assert!(ir.contains("pycc_foreign_attr_pi"), "{ir}");
    }

    /// A failed lookup stops the module body on the module-exec failure
    /// edge, rather than continuing with a `NULL` object.
    ///
    /// PR 2a's review finding 1: without this, `numpy.definitely_not_there`
    /// left CPython's `AttributeError` set, ran the rest of the module
    /// body, and surfaced as `SystemError: execution of module n raised
    /// unreported exception`. The assertion is on the emitted edge --
    /// a null test on the shim's result, and a `ret i64 -1`
    /// ([`EXT_MODULE_EXEC_FAILED`]) on its failure arm, which is the same
    /// edge `foreign_import.rs` takes for a failed import.
    #[test]
    fn a_failed_attribute_load_returns_on_the_module_exec_failure_edge() {
        let ir = entry_ir("foreign_attr_fail_edge", load("numpy", "pi"));
        assert!(ir.contains("foreign_attr_failed"), "{ir}");
        assert!(ir.contains("foreign_attr_fail:"), "{ir}");
        assert!(ir.contains("foreign_attr_cont:"), "{ir}");
        assert!(
            ir.contains(&format!("ret i64 {EXT_MODULE_EXEC_FAILED}")),
            "{ir}"
        );
    }

    /// Two attribute loads in one module share one extern declaration.
    ///
    /// `obj_getattr_fn` returns the existing `FunctionValue` on every call
    /// after the first; a second `add_function` of one name is an LLVM
    /// module-verifier error, so the second load is what proves the early
    /// return is taken rather than merely present.
    #[test]
    fn a_second_attribute_load_reuses_the_one_extern_declaration() {
        let mut items = load("numpy", "pi");
        items.extend(load("scipy", "e"));
        let ir = entry_ir("foreign_attr_twice", items);
        let mut calls = 0usize;
        let mut rest = ir.as_str();
        while let Some(at) = rest.find(EXT_OBJ_GETATTR_SYMBOL) {
            calls += 1;
            rest = &rest[at + EXT_OBJ_GETATTR_SYMBOL.len()..];
        }
        assert_eq!(calls, 2, "one call site per load: {ir}");
    }

    /// The defensive arm in [`expect_object_pointer`]: only `pycc_mir`'s
    /// own lowering builds this node, and only over a `Ty::Object` base, so
    /// any other `Scalar` is a lowering defect. Reached here by handing the
    /// node an `int` base directly, which no `lower_expr` path produces.
    #[test]
    #[should_panic(expected = "did not evaluate to a CPython object")]
    fn a_non_object_base_is_an_internal_error() {
        entry_ir(
            "foreign_attr_bad_base",
            vec![MirItem::TopLevelStmt(MirStmt::ExprStmt(
                MirExpr::ObjAttrGet {
                    base: Box::new(MirExpr::IntLiteral(1)),
                    attr: "pi".to_string(),
                    ty: Ty::Object,
                },
            ))],
        );
    }
}
