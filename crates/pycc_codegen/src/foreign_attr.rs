//! Emission for `MirExpr::ObjAttrGet` (Part 2 of #1026, PR 2a of #1081).
//!
//! The counterpart to `foreign_import.rs`: that module knows how a foreign
//! `import numpy` becomes a module object in a global, this one knows how
//! `numpy.pi` becomes a `PyObject_GetAttr` call on it, with the name
//! interned once per module (#1515). Carved out of
//! `lib.rs` for the same reason (AGENTS.md's "Keep source files
//! decomposable"; the tracker for the rest of `lib.rs` is #545), and kept
//! separate from `foreign_import.rs` because the two sit at different
//! layers -- an *item* emitted once from `compile_to_object`'s item loop
//! versus an *expression* emitted wherever `emit_expr` reaches it.
//!
//! **Ownership** (`docs/RUNTIME.md`). `pycc_ext_obj_getattr` returns a new
//! reference and nothing here releases it. Whoever consumes the result
//! decides: since Part 1 of #1092 an *unbound temporary* -- the operand of
//! another object operation, a condition, a conversion, a discarded
//! statement value, the iterable of a `for` or a comprehension -- is
//! released by its consumer (`object_release.rs`), and since Part 1 of
//! #1499 a module global owns a result bound to it and releases it on
//! rebind (`object_slot.rs`); since Part 2 (#1502) so does a function-frame
//! slot, a user function's parameter (the result moves into it) and a
//! compiled caller of a function returning it (`object_frame.rs`); since
//! Part 4 (#1504) a compiled instance attribute releases the result it
//! replaces (`object_attr.rs`). What remains is the D-107 instance lifetime:
//! a dropped compiled instance keeps each attribute's last value.

use super::*;
use crate::foreign_fail::{ForeignFailEdge, route_null};
use inkwell::builder::Builder;

/// Declares the shim's
/// `PyObject *pycc_ext_obj_getattr(PyObject *, const char *, PyObject **)`
/// once per module, returning the existing declaration on every later call
/// -- `foreign_import.rs`'s `obj_import_fn` pattern exactly, and for the
/// same reason: a second declaration of one name is an LLVM module-verifier
/// error. The attribute load here and `foreign_call.rs`'s keyword method
/// call lookup both declare it through this one function, so the two can
/// never disagree about its signature.
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
        ptr.fn_type(&[ptr.into(), ptr.into(), ptr.into()], false),
        None,
    )
}

/// The LLVM name of the interned-name cache slot for attribute `attr`.
///
/// One slot per distinct name per module (#1515, Part 1 of #1514): an
/// attribute load and a method lookup of the same name share it, because
/// both pass the same interned `str` to CPython -- a positional method
/// call's lookup (`pycc_ext_obj_method_lookup`, #1517) included.
fn attr_name_slot_symbol(attr: &str) -> String {
    format!("pycc_foreign_attr_slot.{attr}")
}

/// The module's cache slot for attribute name `attr`: an internal,
/// zero-initialised `ptr` global the shim fills with the interned `str` on
/// the first load through it and reads on every later one
/// (`pycc_ext_obj_getattr` in `src/ext/pycc_ext_module.c`). Created on the
/// first request for a name and returned as-is on every later one.
pub(super) fn attr_name_slot<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
    attr: &str,
) -> PointerValue<'ctx> {
    let symbol = attr_name_slot_symbol(attr);
    if let Some(existing) = module.get_global(&symbol) {
        return existing.as_pointer_value();
    }
    let ptr = context.ptr_type(inkwell::AddressSpace::default());
    let slot = module.add_global(ptr, None, &symbol);
    slot.set_linkage(inkwell::module::Linkage::Internal);
    slot.set_initializer(&ptr.const_null());
    slot.as_pointer_value()
}

/// Emits one `pycc_ext_obj_getattr(base, "attr", &slot)` call and returns
/// its (possibly `NULL`) result, leaving the failure routing to the caller.
///
/// Shared by [`emit`] (an attribute load) and `foreign_call.rs`'s method
/// lookup. `string_symbol` names the global holding the UTF-8 spelling the
/// shim interns on a slot's first use; `label` names the call's result.
pub(super) fn emit_getattr_call<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    base_ptr: PointerValue<'ctx>,
    attr: &str,
    string_symbol: &str,
    label: &str,
) -> PointerValue<'ctx> {
    let getattr = obj_getattr_fn(context, module);
    let name = builder
        .build_global_string_ptr(attr, string_symbol)
        .expect("build_global_string_ptr should not fail")
        .as_pointer_value();
    let slot = attr_name_slot(context, module, attr);
    builder
        .build_call(getattr, &[base_ptr.into(), name.into(), slot.into()], label)
        .expect("build_call should not fail for pycc_ext_obj_getattr")
        .try_as_basic_value()
        .expect_basic("pycc_ext_obj_getattr returns PyObject *")
        .into_pointer_value()
}

/// The object operand of a foreign-object operation -- for example an
/// attribute load's or method call's base, a direct call's callee, a
/// subscript's base, a `for` loop's iterable, or the operand of `len`, a
/// truth test, a conversion or a tuple unpack (`foreign_len`) -- as a
/// `PyObject *`.
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
/// So this arm emits the `NULL` check itself and routes the failure through
/// [`ForeignFailEdge`] (#1316), whose edge depends on the function being
/// emitted into:
///
/// - in `pycc_ext_module_exec` with no module-level `try` enclosing the
///   load it is the **module-exec failure edge**: return
///   [`EXT_MODULE_EXEC_FAILED`] immediately, leaving CPython's own
///   exception set and unmodified, so the interpreter reports the real
///   exception;
/// - anywhere else -- inside a module-level `try` (Part 1 of #1096) or in
///   any other function (a user function reading a module-level foreign
///   name) -- it calls `pycc_ext_obj_error_bridge`, which moves CPython's
///   exception into pycc's pending state, and branches to the innermost
///   exception target -- the enclosing `try`'s handler, or the function's
///   own `exception_exit` -- exactly as a failed pycc operation does.
///
/// `foreign_fail::emit_failure` owns that rule.
///
/// The nested case (`a.b.c` nests this node inside itself) is answered
/// twice over: this check stops the outer load from ever seeing the inner
/// `NULL`, and the shim's own NULL-`obj` guard in
/// `src/ext/pycc_ext_module.c` still holds the line for any caller that
/// reaches it another way.
pub(super) fn emit<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    rt: &RtFns<'ctx>,
    base: Scalar<'ctx>,
    attr: &str,
) -> Scalar<'ctx> {
    let edge = ForeignFailEdge::for_current(builder);
    let base_ptr = expect_object_pointer(base);
    let loaded = emit_getattr_call(
        context,
        builder,
        module,
        base_ptr,
        attr,
        &format!("pycc_foreign_attr_{attr}"),
        "foreign_attr",
    );
    route_null(context, builder, module, rt, edge, loaded, "foreign_attr");
    Scalar::Object(loaded)
}

/// The function `builder` is currently emitting into, once it is known to
/// be the `--ext` module-body entry point.
///
/// Since #1316 only the operations that remain module-body-only call this:
/// `foreign_call.rs`'s `for` loop over a foreign iterable and
/// `foreign_len.rs`'s float-tuple unpack, both of which `pycc_types` still
/// admits only at module scope. Every other foreign operation routes its
/// failure through [`ForeignFailEdge`], which works in any function.
///
/// The guard runs *before* any block is appended: those callers build a
/// [`ForeignFailEdge::ModuleExec`] edge, which returns `i64 -1` when no
/// module-level `try` encloses the operation, so emitting it into a
/// function with a different return type would be an LLVM verifier error
/// rather than a diagnosable one. Reaching it from anywhere else is a
/// front-end defect: the operations are admitted only in a module body.
pub(super) fn expect_module_exec_entry<'ctx>(builder: &Builder<'ctx>) -> FunctionValue<'ctx> {
    let function = builder
        .get_insert_block()
        .expect("the builder is positioned inside a block")
        .get_parent()
        .expect("every basic block belongs to a function");
    if function.get_name().to_bytes() != EXT_MODULE_EXEC_SYMBOL.as_bytes() {
        panic!(
            "pycc_codegen: internal error: a module-body-only operation on a CPython object \
             was emitted outside `{EXT_MODULE_EXEC_SYMBOL}` -- pycc_types admits a foreign \
             `for` loop and a float-tuple unpack only in a module body"
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
    /// was the shape PR 2a admitted end to end: `pycc_types` then refused
    /// binding a CPython object to a name (`I0404`; admitted at module scope
    /// since #1325 and in a function body since Part 1 of #1333), and the
    /// discarded load keeps these tests independent of the store.
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
        compile_ir(label, items).0
    }

    /// The LLVM text of the whole module -- globals included -- after
    /// compiling `items` as an `ext` object, for the assertions about the
    /// interned-name cache slots (#1515), which are module-level globals the
    /// entry point's own text names but does not define.
    fn module_ir(label: &str, items: Vec<MirItem>) -> String {
        compile_ir(label, items).1
    }

    /// `(entry point IR, whole-module IR)` for [`entry_ir`] and [`module_ir`].
    fn compile_ir(label: &str, items: Vec<MirItem>) -> (String, String) {
        let dir = pycc_scratch::ScratchDir::new(label).expect("failed to create scratch dir");
        let mut ir = String::new();
        let mut whole = String::new();
        let mut observer = |module: &inkwell::module::Module<'_>, _: Option<&'static str>| {
            if let Some(entry) = module.get_function(EXT_MODULE_EXEC_SYMBOL) {
                ir = crate::llvm_string_to_owned(entry.print_to_string());
                whole = crate::llvm_string_to_owned(module.print_to_string());
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
        (ir, whole)
    }

    /// One discarded `<module>.<method>()` call with no arguments -- the
    /// method-lookup half of the shared helper (`foreign_call.rs`).
    fn method_call(module: &str, method: &str) -> Vec<MirItem> {
        vec![
            MirItem::ForeignImport {
                local_name: module.to_string(),
                module_path: module.to_string(),
                from: None,
            },
            MirItem::TopLevelStmt(MirStmt::ExprStmt(MirExpr::ObjMethodCall {
                base: Box::new(MirExpr::Name {
                    name: module.to_string(),
                    ty: Ty::Object,
                }),
                method: method.to_string(),
                args: Vec::new(),
            })),
        ]
    }

    /// How many times `needle` occurs in `haystack`.
    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    /// The definition line LLVM prints for `attr`'s cache slot.
    fn slot_definition(attr: &str) -> String {
        format!(
            "@{} = internal global ptr null",
            attr_name_slot_symbol(attr)
        )
    }

    /// #1515: the shim receives the attribute's cache slot as its third
    /// argument, and the slot is an internal, null-initialised pointer
    /// global -- the state the shim's first-use intern expects. The
    /// regression this pins is the hot-path shape itself: with no slot
    /// argument the shim can only build a fresh `str` per load
    /// (`PyObject_GetAttrString`), which was the largest single cost of the
    /// #1207 subject's parse loop (`docs/TESTING.md`).
    #[test]
    fn an_attribute_load_passes_its_interned_name_slot_to_the_shim() {
        let (entry, whole) = compile_ir("foreign_attr_slot", load("numpy", "pi"));
        let slot = format!("ptr @{})", attr_name_slot_symbol("pi"));
        assert!(entry.contains(&slot), "{entry}");
        assert_eq!(occurrences(&whole, &slot_definition("pi")), 1, "{whole}");
    }

    /// Two loads of one name, and a method lookup of that same name, share
    /// one slot: the slot is fetched back from the module on every request
    /// after the first, so the name is interned once per module rather than
    /// once per site. A second `add_global` of the same name would have
    /// been renamed by LLVM (`....1`), which the second assertion excludes.
    #[test]
    fn every_lookup_of_one_name_shares_one_slot() {
        let mut items = load("numpy", "pi");
        items.extend(load("scipy", "pi"));
        items.extend(method_call("math", "pi"));
        let whole = module_ir("foreign_attr_slot_shared", items);
        let symbol = attr_name_slot_symbol("pi");
        assert_eq!(occurrences(&whole, &slot_definition("pi")), 1, "{whole}");
        assert!(!whole.contains(&format!("@{symbol}.1")), "{whole}");
        // The two loads pass the slot last; the method lookup (#1517)
        // passes it before its call-site cache and out-parameter.
        assert_eq!(
            occurrences(&whole, &format!("ptr @{symbol})")),
            2,
            "{whole}"
        );
        assert_eq!(
            occurrences(&whole, &format!("ptr @{symbol},")),
            1,
            "{whole}"
        );
    }

    /// Different names get different slots: a slot caches exactly one
    /// interned `str`, so two names sharing one would make the second load
    /// read the first name's attribute.
    #[test]
    fn different_names_get_different_slots() {
        let mut items = load("numpy", "pi");
        items.extend(method_call("numpy", "e"));
        let whole = module_ir("foreign_attr_slot_distinct", items);
        assert_eq!(occurrences(&whole, &slot_definition("pi")), 1, "{whole}");
        assert_eq!(occurrences(&whole, &slot_definition("e")), 1, "{whole}");
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
