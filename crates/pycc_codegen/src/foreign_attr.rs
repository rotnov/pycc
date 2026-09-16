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

/// The base of a foreign attribute load, as a `PyObject *`.
///
/// `pycc_mir`'s lowering builds `MirExpr::ObjAttrGet` only where the base's
/// inferred type is `Ty::Object` (`pycc_mir::expr`'s `HirExpr::AttrGet`
/// arm), and `ty_to_basic_type` maps that to a pointer, so every other
/// `Scalar` here is a lowering defect rather than a program the front end
/// let through.
fn expect_object_pointer(scalar: Scalar<'_>) -> PointerValue<'_> {
    let Scalar::Object(ptr) = scalar else {
        panic!(
            "pycc_codegen: internal error: a foreign attribute base did not evaluate to a \
             CPython object -- pycc_mir lowers `ObjAttrGet` only for a `Ty::Object` base"
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
/// (D-173, `pycc_rt_exception_active`), and this arm deliberately does not
/// bridge them, so no `NULL` check is emitted here at all.
///
/// The reason is reachability, not convenience. `expression_can_set_exception`
/// answers `true` for this node, so `emit_expr` already emits the standard
/// pending-exception guard immediately after this call -- but that guard
/// reads pycc's pending state, which a CPython-set exception leaves
/// untouched, so the no-exception edge is taken and a `NULL`
/// `Scalar::Object` continues. In PR 2a that `NULL` is never dereferenced
/// by pycc-generated code: `pycc_types` refuses a `Ty::Object` operand at
/// every consuming site (`docs/TYPE_SYSTEM.md`'s `I0404` rules), which
/// leaves exactly three shapes -- a discarded `ExprStmt`, the return of an
/// unannotated private helper, which D-137's amendment makes unexportable
/// because `object` cannot be spelled in an annotation, and the base of a
/// *further* `ObjAttrGet`, because `a.b.c` nests this node inside itself.
/// The first two hand the value to nobody. The third does: it passes the
/// `NULL` back into `pycc_ext_obj_getattr` as the next call's `obj`, and
/// `PyObject_GetAttrString` dereferences its argument's type without a
/// guard. That case is answered once, in the shim -- a NULL `obj` returns
/// NULL unchanged, preserving the inner lookup's own `AttributeError` --
/// rather than by a check emitted at every load site here; the shim's own
/// comment in `src/ext/pycc_ext_module.c` carries the reasoning.
///
/// PR 2b owns the transition, because `MirExpr::ObjMethodCall` is what
/// first makes the loaded value reachable from a host call. It is the
/// *shim's* side to own when it lands -- translating CPython's exception
/// into pycc's pending state there keeps every CPython API call inside
/// `src/ext/pycc_ext_module.c`, and the reverse trip already exists as
/// `pycc_ext_raise_pending`. Note that pycc's builtin exception tags
/// (`pycc_rt::exception`) carry no `AttributeError`, so 2b must either add
/// one or accept a documented downgrade -- a CPython-semantics deviation,
/// and therefore D-242 rule 3 decision-file work rather than something this
/// arm may settle silently.
pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    base: Scalar<'ctx>,
    attr: &str,
) -> Scalar<'ctx> {
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
    Scalar::Object(loaded)
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
    #[should_panic(expected = "a foreign attribute base did not evaluate to a CPython object")]
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
