//! #1199: exports are bound on the module as their definitions execute,
//! not before the body runs, and every C function that calls compiled code
//! refuses an unbound `fnptr_` slot with a `NameError`.
//!
//! The host-observable behaviour is pinned end to end by
//! `tests/issue_1199_ext_reentrant_init.rs` (ignored: it needs a CPython
//! with development headers); these generated-text tests pin the pieces
//! the coverage gate runs.

use super::exports::constructible_module;
use super::generated_c::{inc_no_classes, static_export};
use super::*;

fn scalar_export(name: &str) -> ExtExport {
    ExtExport {
        defaults: Vec::new(),
        name: name.to_string(),
        class: None,
        method: None,
        returns_buffer_slice: false,
        receiver: ExtReceiver::None,
        params: vec![Ty::Int],
        param_writable: vec![false],
        return_ty: Ty::Int,
        keyword_names: None,
    }
}

/// The text of the generated C function whose definition line starts with
/// `head`, up to its closing brace.
fn function_text<'a>(text: &'a str, head: &str) -> &'a str {
    let start = text
        .find(head)
        .unwrap_or_else(|| panic!("no {head:?} in:\n{text}"));
    let body = &text[start..];
    &body[..body.find("\n}\n").expect("the function is closed")]
}

fn guard(symbol: &str, source_name: &str, fail: &str) -> String {
    format!(
        "    if (fnptr_{symbol} == NULL) {{\n        PyErr_SetString(PyExc_NameError, \
         \"name '{source_name}' is not defined\");\n        return {fail};\n    }}\n"
    )
}

#[test]
fn the_shim_has_no_creation_time_method_table() {
    let shim = shim_c();
    assert!(
        shim.contains("    0,\n    /*")
            && shim.contains("    NULL,\n    pycc_ext_slots,\n    NULL,\n"),
        "the moduledef must carry m_methods = NULL"
    );
}

#[test]
fn the_shim_defines_the_publication_entry_point_codegen_calls() {
    let shim = shim_c();
    let head = format!(
        "int {}(const char *name)\n{{",
        pycc_codegen::EXT_PUBLISH_SYMBOL
    );
    let publish = function_text(&shim, &head);
    // The name is resolved against the function table and then the class
    // lookup before the exec-target key is read, so an unknown name -- every
    // name, in the embedded launcher -- touches no state.
    let methods = publish
        .find("for (def = pycc_ext_methods; def->ml_name != NULL; def++)")
        .expect("the function table is searched");
    let class = publish
        .find("class_slot = pycc_ext_publish_class_slot(name);")
        .expect("the class lookup is consulted");
    let unknown = publish
        .find("        if (class_slot == NULL) {\n            return 0;\n        }")
        .expect("an unknown name is a no-op");
    let key = publish
        .find("PyThread_tss_get(pycc_ext_exec_target_key)")
        .expect("the target module is read");
    assert!(
        methods < class && class < unknown && unknown < key,
        "{publish}"
    );
    assert!(!publish.contains("pycc_ext_module_hooks"), "{publish}");
    // A function is bound the way `PyModule_AddFunctions` binds a row, and
    // both new references are released on every path.
    for needle in [
        "modname = PyModule_GetNameObject(module);",
        "func = PyCFunction_NewEx(row, module, modname);\n    Py_DECREF(modname);",
        "status = PyModule_AddObjectRef(module, row->ml_name, func);\n    Py_DECREF(func);",
        "return PyModule_AddObjectRef(module, name, *class_slot);",
    ] {
        assert!(
            publish.contains(needle),
            "missing {needle:?} in:\n{publish}"
        );
    }
    // The generated lookup it calls is the one the companion defines.
    let lookup = PUBLISH_CLASS_SLOT_DECL
        .split('(')
        .next()
        .and_then(|head| head.rsplit(['*', ' ']).next())
        .expect("a function name");
    assert_eq!(lookup, "pycc_ext_publish_class_slot");
}

#[test]
fn the_shim_binds_unpublished_classes_after_the_body_and_before_the_hooks() {
    let shim = shim_c();
    let net = function_text(
        &shim,
        "static int pycc_ext_publish_unbound_classes(PyObject *module)\n{",
    );
    assert!(
        net.contains(&format!(
            "for (name = {PUBLISH_CLASS_NAMES}; *name != NULL; name++)"
        )),
        "{net}"
    );
    assert!(
        net.contains("PyDict_GetItemString(dict, *name) != NULL"),
        "{net}"
    );
    let body = shim
        .find("exec_status = pycc_ext_module_exec();")
        .expect("the body runs");
    let net_call = shim
        .find("    if (pycc_ext_publish_unbound_classes(module) != 0) {")
        .expect("the safety net runs");
    let hooks = shim
        .find("if (PyModule_AddFunctions(module, pycc_ext_module_hooks) != 0) {")
        .expect("the hooks are added");
    assert!(body < net_call && net_call < hooks);
}

#[test]
fn a_program_publishing_no_class_still_defines_both_publication_tables() {
    let inc = inc_no_classes("m", &[]);
    assert!(
        inc.contains(&format!(
            "static const char *const {PUBLISH_CLASS_NAMES}[] = {{\n    NULL,\n}};\n\n\
             {PUBLISH_CLASS_SLOT_DECL}\n{{\n    (void)name;\n    return NULL;\n}}\n"
        )),
        "{inc}"
    );
}

#[test]
fn a_published_class_is_registered_but_bound_only_through_its_publication() {
    let hir = constructible_module("Grid");
    let exports = collect_exports(&hir).expect("a carriable program");
    let publications = collect_class_publications(&hir, &exports);
    let ctors = collect_constructors(&hir, &publications);
    let inc = generate_exports_inc("m", &exports, &[], &publications, &ctors, &[]);
    assert!(
        inc.contains(&format!(
            "static const char *const {PUBLISH_CLASS_NAMES}[] = {{\n    \"Grid\",\n    NULL,\n}};"
        )),
        "{inc}"
    );
    assert!(
        inc.contains(
            "    if (strcmp(name, \"Grid\") == 0) {\n        \
             return &pycc_ext_type_object_Grid;\n    }\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_carrier_register(\"Grid\", type)"),
        "{inc}"
    );
    assert!(
        !inc.contains("PyModule_AddObjectRef(module, \"Grid\""),
        "{inc}"
    );

    // The constructor refuses an unbound `__init__` slot after its local
    // declarations and before it binds keywords, unpacks or allocates.
    let init = function_text(
        &inc,
        "static int pycc_ext_tp_init_Grid(PyObject *self, PyObject *args, PyObject *kwds)",
    );
    let symbol = pycc_codegen::mangle_ext_name("Grid.__init__");
    let at = init
        .find(&guard(&symbol, "Grid.__init__", "-1"))
        .unwrap_or_else(|| panic!("no guard in:\n{init}"));
    for later in [
        "kwds",
        "pycc_ext_unpack_",
        "pycc_rt_instance_new",
        "pycc_ext_bridge_mark",
    ] {
        let index = init.rfind(later).expect("the constructor body");
        assert!(at < index, "{later:?} precedes the guard:\n{init}");
    }
    assert!(init[..at].contains("    void *inst;\n"), "{init}");
}

/// The guard is the wrapper's first statement, before the receiver is
/// unwrapped, any argument is unpacked or the bridge mark is taken, for
/// the scalar call, the thunk call and a method alike.
#[test]
fn every_wrapper_refuses_an_unbound_slot_before_anything_else() {
    let tuple = ExtExport {
        params: vec![Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float]))],
        ..scalar_export("pair")
    };
    let cases = [
        (scalar_export("plain"), "plain", "plain"),
        (tuple, "pair", "pair"),
        (
            static_export("Grid", "scale", vec![Ty::Int], Ty::Int),
            "Grid.scale.static",
            "Grid.scale",
        ),
    ];
    for (export, name, source_name) in cases {
        let text = wrapper_for(&export);
        let symbol = pycc_codegen::mangle_ext_name(name);
        assert!(
            text.contains(&format!("extern void *fnptr_{symbol};\n")),
            "{text}"
        );
        let head = format!("static PyObject *pycc_ext_wrap_{symbol}(");
        let wrapper = function_text(&text, &head);
        let opening = wrapper.find("{\n").expect("the body opens") + 2;
        assert!(
            wrapper[opening..].starts_with(&guard(&symbol, source_name, "NULL")),
            "{wrapper}"
        );
    }
}

#[test]
fn an_instance_method_wrapper_guards_before_unwrapping_its_receiver() {
    let hir = constructible_module("Grid");
    let exports = collect_exports(&hir).expect("a carriable program");
    let area = exports
        .iter()
        .find(|export| export.name == "Grid.area")
        .expect("the instance method is exported");
    let text = wrapper_for(area);
    let symbol = pycc_codegen::mangle_ext_name("Grid.area");
    let at = text
        .find(&guard(&symbol, "Grid.area", "NULL"))
        .unwrap_or_else(|| panic!("no guard in:\n{text}"));
    let receiver = text.find("void *self_inst").expect("the receiver");
    assert!(at < receiver, "{text}");
}

/// The items of `source` in order, lowered the way the build lowers it
/// (with the inherited-method copies appended): a `MirItem::Function` by
/// its name, any other item as `"<stmt>"`.
fn item_order(source: &str) -> Vec<String> {
    let module = pycc_parser::parse(source).expect("test fixture must parse");
    let hir = pycc_hir::lower_checked(&module).expect("test fixture must lower");
    let hir = pycc_types::check_and_resolve(&hir).expect("test fixture must check");
    pycc_mir::build(&hir)
        .items
        .iter()
        .map(|item| match item {
            pycc_mir::MirItem::Function { name, .. } => name.clone(),
            _ => "<stmt>".to_string(),
        })
        .collect()
}

/// `pycc_codegen`'s `ext_publish` publishes a class after the largest
/// ordinal of the items its MRO owns, which is "right after the class
/// statement" only because `pycc_hir` lowers a class's own items as one
/// contiguous run at the statement's position. This pins that layout,
/// including the synthesized `__init__` (D-225) of a base-less class that
/// declares none, and a module-level statement between two classes. A
/// subclass that declares nothing (`class E(P): pass`) owns no item at all:
/// `ext_publish` publishes it with its bases instead, the residual
/// `docs/RUNTIME.md`'s #1199 paragraph records.
#[test]
fn a_class_statement_lowers_its_own_items_contiguously_at_its_position() {
    let order = item_order(
        "def a() -> int:\n    return 1\n\n\n\
         class P:\n    def __init__(self, v: int) -> None:\n        self.v = v\n\n    \
         def get(self) -> int:\n        return self.v\n\n\n\
         def b() -> int:\n    return 2\n\n\n\
         class Q(P):\n    def more(self) -> int:\n        return 3\n\n\n\
         class R:\n    x: int = 0\n\n    def get(self) -> int:\n        return self.x\n\n\n\
         class S:\n    pass\n\n\n\
         print(1)\n\n\n\
         class E(P):\n    pass\n\n\n\
         def c() -> int:\n    return 3\n",
    );
    assert_eq!(
        order,
        [
            "a",
            "P.__init__",
            "P.get",
            "b",
            "Q.more",
            "R.get",
            "R.__init__",
            "S.__init__",
            "<stmt>",
            "c",
        ]
    );
}
