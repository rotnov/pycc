//! The generated per-class type objects that publish an exported
//! `@staticmethod` or `@classmethod` as `mod.Class.method`.
//!
//! Extracted from `src/ext_build.rs` by #1143 under `AGENTS.md`'s
//! decomposability rule, together with `export_name.rs`. The generator
//! itself is unchanged by the move.

use super::ExtExport;

/// The exact C declaration of the generated method-class registration entry
/// point, called from `pycc_ext_exec_module` for the same reason and in the
/// same way as [`USER_EXCEPTION_REGISTER_DECL`]: the `.inc` is included
/// first, so it needs no forward declaration, and both the shim test and
/// the generated-text test assert this one constant so a spelling drift
/// fails an ordinary `cargo test`.
///
/// Emitted unconditionally -- with an empty body when the program exports
/// no method -- so every artifact links.
pub(crate) const METHOD_TYPE_REGISTER_DECL: &str =
    "static int pycc_ext_register_method_types(PyObject *module)";

/// One non-instantiable type object per exporting class, plus the
/// registration entry point `pycc_ext_exec_module` calls.
///
/// **`PyType_FromSpec`, not `PyType_FromModuleAndSpec`.** The shim's
/// `m_size` is `0` and its file-scope statics are licensed by its refusal of
/// subinterpreters and of free-threaded hosts, so nothing here needs
/// `PyType_GetModule`.
///
/// `basicsize` is `0` and the flags carry
/// `Py_TPFLAGS_DISALLOW_INSTANTIATION` and `Py_TPFLAGS_IMMUTABLETYPE`, so
/// `mod.Class()` raises `TypeError: cannot create '<mod>.<Class>'
/// instances` and the type's attributes cannot be replaced. That flag is
/// **load-bearing**, not decoration: it is what makes a later change that
/// admits instance methods -- raising `basicsize`, adding `Py_tp_new` and
/// `Py_tp_init`, dropping the flag -- purely additive at the host-visible
/// surface, so no host program written against this artifact stops working.
///
/// `spec.name` is `PYCC_EXT_MODULE_NAME_STR ".<Class>"` as adjacent string
/// literals, which is what gives the type the right `__module__` and
/// `__qualname__`.
///
/// The registration function is emitted unconditionally, with an empty body
/// when nothing exports a method, so every artifact links -- the same
/// discipline [`exception_classes_c`] follows.
pub(crate) fn method_types_c(exports: &[ExtExport]) -> String {
    // Classes in first-export order, deduplicated: the emitted `.inc` must
    // be byte-identical across runs, so this never walks a hash map.
    let mut class_order: Vec<&str> = Vec::new();
    for export in exports {
        if let Some(class) = &export.class
            && !class_order.contains(&class.as_str())
        {
            class_order.push(class.as_str());
        }
    }
    let mut out = String::new();
    for class in &class_order {
        out.push_str(&format!(
            "static PyMethodDef pycc_ext_type_methods_{class}[] = {{\n"
        ));
        for export in exports
            .iter()
            .filter(|export| export.class.as_deref() == Some(*class))
        {
            let method = export
                .method
                .as_deref()
                .expect("an export with a class carries a method name");
            // `METH_STATIC` delivers `self == NULL`; `METH_CLASS` delivers
            // the type object, which the wrapper discards. Both keep
            // `METH_FASTCALL`, so CPython still raises the keyword
            // `TypeError` before the wrapper is entered -- D-244 rule 7's
            // closed boundary, unchanged by the move onto a type object.
            let flags = if export.receiver {
                "METH_FASTCALL | METH_CLASS"
            } else {
                "METH_FASTCALL | METH_STATIC"
            };
            out.push_str(&format!(
                "    {{\"{method}\", (PyCFunction)(void (*)(void))pycc_ext_wrap_{symbol}, \
                 {flags}, NULL}},\n",
                symbol = pycc_codegen::mangle_ext_name(&export.name)
            ));
        }
        out.push_str("    {NULL, NULL, 0, NULL},\n};\n\n");
        out.push_str(&format!(
            "static PyType_Slot pycc_ext_type_slots_{class}[] = {{\n    \
             {{Py_tp_methods, pycc_ext_type_methods_{class}}},\n    {{0, NULL}},\n}};\n\n"
        ));
        out.push_str(&format!(
            "static PyType_Spec pycc_ext_type_spec_{class} = {{\n    \
             PYCC_EXT_MODULE_NAME_STR \".{class}\",\n    0,\n    0,\n    \
             Py_TPFLAGS_DEFAULT | Py_TPFLAGS_DISALLOW_INSTANTIATION | \
             Py_TPFLAGS_IMMUTABLETYPE,\n    pycc_ext_type_slots_{class},\n}};\n\n"
        ));
    }
    out.push_str(&format!("{METHOD_TYPE_REGISTER_DECL}\n{{\n"));
    if class_order.is_empty() {
        out.push_str("    (void)module;\n    return 0;\n}\n");
        return out;
    }
    out.push_str("    PyObject *type;\n");
    for class in &class_order {
        // `PyModule_AddObjectRef` takes its own reference, so the local one
        // is released on both arms. Releasing it on the failing arm too is
        // what keeps a failed registration from leaking the type.
        out.push_str(&format!(
            "    type = PyType_FromSpec(&pycc_ext_type_spec_{class});\n    \
             if (type == NULL) {{\n        return -1;\n    }}\n    \
             if (PyModule_AddObjectRef(module, \"{class}\", type) < 0) {{\n        \
             Py_DECREF(type);\n        return -1;\n    }}\n    Py_DECREF(type);\n"
        ));
    }
    out.push_str("    return 0;\n}\n");
    out
}
