//! The setter of a slot descriptor (Part 1 of #1443): what a host
//! `obj.x = v` and `del obj.x` run on a constructible published class.
//!
//! A store converts `v` by the *parameter row* of the boundary table for
//! the slot's declared type (`docs/RUNTIME.md`), with the same
//! `pycc_ext_unpack_*` helper a parameter of that type uses, so a slot
//! admits exactly the values an argument of its type admits. The converted
//! word is the one a compiled `self.x = v` stores (`pycc_codegen`'s
//! `scalar_to_slot_word`), and `pycc_rt_ext_instance_store_slot` releases
//! the replaced word by the slot's kind byte -- #1455's
//! [`copy_kind`] -- exactly as the compiled store does, so a host store and
//! a compiled store leave the slot in the same ownership state.
//!
//! A `del` un-assigns the slot through `pycc_rt_ext_instance_delete_slot`,
//! which raises the checked read's own `AttributeError` when the slot is
//! not assigned, as CPython does for `del` of a missing attribute.

use pycc_hir::Ty;

use super::super::instance_copy::copy_kind;

/// The C setter function name for `class`'s attribute `name`, length
/// prefixed like the getter's.
pub(super) fn setter_symbol(class: &str, name: &str) -> String {
    format!("pycc_ext_set_{}_{class}_{}_{name}", class.len(), name.len())
}

/// The local the unpack helper fills, its call, and the statement that
/// turns it into the slot word `word`, for a slot of type `ty`. `fn_name`
/// is what the helper's refusal message names (`<fn_name>() argument 1`).
fn unpack_value(fn_name: &str, ty: &Ty) -> (&'static str, String, &'static str) {
    let call = |helper: &str| format!("pycc_ext_unpack_{helper}(value, \"{fn_name}\", 0, &v)");
    match ty {
        Ty::Int => ("long long v;", call("int"), "word = v;"),
        // The `f64` bit pattern, the inverse of the getter's `memcpy`.
        Ty::Float => (
            "double v;",
            call("float"),
            "memcpy(&word, &v, sizeof word);",
        ),
        // `0` or `1`, widened with no sign extension, as the compiled store
        // zero-extends an `i1`.
        Ty::Bool => ("char v;", call("bool"), "word = (unsigned char)v;"),
        Ty::Str => ("void *v;", call("str"), "word = (long long)(intptr_t)v;"),
        // An initialized carrier of this module whose class has the declared
        // class on its MRO, as for a parameter of that class (#1449).
        Ty::Instance(declared) => (
            "void *v;",
            format!("pycc_ext_unpack_instance(value, \"{fn_name}\", 0, \"{declared}\", &v)"),
            "word = (long long)(intptr_t)v;",
        ),
        // `collect_getsets` admits exactly the `carried` types, so the
        // remaining one is the opaque object, which takes a new reference.
        _ => ("void *v;", call("object"), "word = (long long)(intptr_t)v;"),
    }
}

/// The C text of the setter for slot `index`, named `name` and declared
/// `ty`, of class `class`.
pub(super) fn slot_setter_c(class: &str, name: &str, index: usize, ty: &Ty) -> String {
    let symbol = setter_symbol(class, name);
    let kind = char::from(copy_kind(ty).expect("every carried slot type has a copy kind"));
    let (local, unpack, to_word) = unpack_value(&format!("{class}.{name}"), ty);
    // A carrier `tp_init` never filled has no instance. Its `del` answers the
    // getter's `AttributeError`, which is CPython's for `del` of an unset
    // attribute; a store is refused, because the shim cannot allocate the
    // instance (its layout is a compiled-code constant).
    format!(
        "static int {symbol}(PyObject *self, PyObject *value, void *closure)\n{{\n    \
         void *inst = ((PyccExtInstance *)self)->inst;\n    {local}\n    long long word;\n    \
         (void)closure;\n    if (inst == NULL) {{\n        if (value == NULL) {{\n            \
         PyErr_SetString(PyExc_AttributeError, \
         \"'{class}' object has no attribute '{name}'\");\n        }} else {{\n            \
         PyErr_SetString(PyExc_AttributeError, \
         \"cannot set '{name}' on a '{class}' object whose __init__ never ran\");\n        \
         }}\n        return -1;\n    }}\n    if (value == NULL) {{\n        \
         if (pycc_rt_ext_instance_delete_slot(inst, {index}, '{kind}') != 0) {{\n            \
         pycc_ext_raise_pending();\n            return -1;\n        }}\n        \
         return 0;\n    }}\n    if ({unpack} != 0) {{\n        return -1;\n    }}\n    \
         {to_word}\n    pycc_rt_ext_instance_store_slot(inst, {index}, '{kind}', word);\n    \
         return 0;\n}}\n\n"
    )
}
