//! The generated per-class type objects that publish an exported
//! `@staticmethod`, `@classmethod` or instance method as
//! `mod.Class.method`, and -- for a constructible class -- the `tp_init`
//! that makes `mod.Class(...)` build one.
//!
//! Extracted from `src/ext_build.rs` by #1143 under `AGENTS.md`'s
//! decomposability rule, together with `export_name.rs`. #1145 added the
//! constructor half.

use super::export_name::ExtReceiver;
use super::{
    ExtCtor, ExtPublishedClass, arg_slot_locals, buffer_releases, c_param_list, source_level_name,
    unpack_args,
};

/// The exact C declaration of the generated method-class registration entry
/// point, called from `pycc_ext_exec_module` for the same reason and in the
/// same way as [`USER_EXCEPTION_REGISTER_DECL`]: the `.inc` is included
/// first, so it needs no forward declaration, and both the shim test and
/// the generated-text test assert this one constant so a spelling drift
/// fails an ordinary `cargo test`.
///
/// Emitted unconditionally -- with an empty body when the program exports
/// no method -- so every artifact links.
///
/// [`USER_EXCEPTION_REGISTER_DECL`]: super::USER_EXCEPTION_REGISTER_DECL
pub(crate) const METHOD_TYPE_REGISTER_DECL: &str =
    "static int pycc_ext_register_method_types(PyObject *module)";

/// One type object per exporting class, plus the registration entry point
/// `pycc_ext_exec_module` calls.
///
/// **`PyType_FromSpec`, not `PyType_FromModuleAndSpec`.** The shim's
/// `m_size` is `0` and its file-scope statics are licensed by its refusal of
/// subinterpreters and of free-threaded hosts, so nothing here needs
/// `PyType_GetModule`.
///
/// **The class list and each table's rows are resolved upstream.**
/// `publications` is [`collect_class_publications`]' output: one entry per
/// class that resolves at least one exported member through its MRO, each
/// carrying that MRO-resolved method set with a derived override already
/// shadowing its base's definition. A class that resolves nothing -- one
/// with a perfectly carriable `__init__` but no public method anywhere in
/// its MRO -- gets no type object at all and so cannot be constructed. This
/// function renders that decision and never re-derives it, so the MRO walk
/// exists once (`AGENTS.md`'s canonical-statement rule).
///
/// [`collect_class_publications`]: super::collect_class_publications
///
/// A class in `ctors` is **constructible**: its spec carries
/// `basicsize = sizeof(PyccExtInstance)`, the slots `Py_tp_new`
/// (`PyType_GenericNew` directly), the generated per-class
/// `pycc_ext_tp_init_<Class>` below and the shim's shared
/// `pycc_ext_instance_dealloc`, and it drops
/// `Py_TPFLAGS_DISALLOW_INSTANTIATION`. Every other class keeps Part 1's
/// shape byte for byte -- `basicsize` `0`, no slots but `Py_tp_methods`, and
/// the flag that makes `mod.Class()` raise `TypeError: cannot create
/// '<mod>.<Class>' instances`.
///
/// Both shapes keep `Py_TPFLAGS_IMMUTABLETYPE` and neither adds
/// `Py_TPFLAGS_BASETYPE`, so the type's attributes cannot be replaced and
/// `class S(mod.Class): pass` raises `TypeError: type '<mod>.<Class>' is not
/// an acceptable base type`. Subclassing is out of scope rather than
/// forbidden on principle: a subclass would inherit `tp_init` and get an
/// instance whose slot layout is its *base's*.
///
/// `spec.name` is `PYCC_EXT_MODULE_NAME_STR ".<Class>"` as adjacent string
/// literals, which is what gives the type the right `__module__` and
/// `__qualname__`.
///
/// The registration function is emitted unconditionally, with an empty body
/// when nothing exports a method, so every artifact links -- the same
/// discipline [`exception_classes_c`] follows.
///
/// [`exception_classes_c`]: super::exception_classes_c
pub(crate) fn method_types_c(publications: &[ExtPublishedClass], ctors: &[ExtCtor]) -> String {
    let mut out = String::new();
    for published in publications {
        let class = &published.class;
        let ctor = ctors.iter().find(|ctor| ctor.class == *class);
        out.push_str(&format!(
            "static PyMethodDef pycc_ext_type_methods_{class}[] = {{\n"
        ));
        for export in &published.methods {
            let method = export
                .method
                .as_deref()
                .expect("an export with a class carries a method name");
            // `METH_STATIC` delivers `self == NULL`; `METH_CLASS` delivers
            // the type object, which the wrapper discards; plain
            // `METH_FASTCALL` delivers the instance, which an instance
            // method's wrapper unwraps (#1145). All three keep
            // `METH_FASTCALL`, so CPython still raises the keyword
            // `TypeError` before the wrapper is entered -- D-244 rule 7's
            // closed boundary, which the generated `tp_init` has to
            // reimplement by hand precisely because it is *not* a
            // `METH_FASTCALL` entry point.
            let flags = match export.receiver {
                ExtReceiver::None => "METH_FASTCALL | METH_STATIC",
                ExtReceiver::NullCls => "METH_FASTCALL | METH_CLASS",
                ExtReceiver::SelfInstance => "METH_FASTCALL",
            };
            out.push_str(&format!(
                "    {{\"{method}\", (PyCFunction)(void (*)(void))pycc_ext_wrap_{symbol}, \
                 {flags}, NULL}},\n",
                symbol = pycc_codegen::mangle_ext_name(&export.name)
            ));
        }
        out.push_str("    {NULL, NULL, 0, NULL},\n};\n\n");
        if let Some(ctor) = ctor {
            out.push_str(&tp_init_c(ctor));
        }
        out.push_str(&format!(
            "static PyType_Slot pycc_ext_type_slots_{class}[] = {{\n    \
             {{Py_tp_methods, pycc_ext_type_methods_{class}}},\n"
        ));
        if ctor.is_some() {
            // `PyType_GenericNew` directly rather than a generated
            // `tp_new`: it zeroes the whole object, so `inst` starts NULL
            // and `mod.Class.__new__(mod.Class)` -- which never runs
            // `tp_init` -- yields a carrier the wrappers' NULL guard
            // refuses rather than one holding indeterminate storage.
            out.push_str(&format!(
                "    {{Py_tp_new, PyType_GenericNew}},\n    \
                 {{Py_tp_init, pycc_ext_tp_init_{class}}},\n    \
                 {{Py_tp_dealloc, pycc_ext_instance_dealloc}},\n"
            ));
        }
        out.push_str("    {0, NULL},\n};\n\n");
        // A non-constructible class keeps `basicsize` `0` and the
        // `DISALLOW_INSTANTIATION` flag Part 1 emitted; a constructible one
        // needs room for the carrier and must not refuse its own
        // constructor.
        let (basicsize, flags) = match ctor {
            Some(_) => (
                "sizeof(PyccExtInstance)",
                "Py_TPFLAGS_DEFAULT | Py_TPFLAGS_IMMUTABLETYPE",
            ),
            None => (
                "0",
                "Py_TPFLAGS_DEFAULT | Py_TPFLAGS_DISALLOW_INSTANTIATION | \
                 Py_TPFLAGS_IMMUTABLETYPE",
            ),
        };
        out.push_str(&format!(
            "static PyType_Spec pycc_ext_type_spec_{class} = {{\n    \
             PYCC_EXT_MODULE_NAME_STR \".{class}\",\n    {basicsize},\n    0,\n    \
             {flags},\n    pycc_ext_type_slots_{class},\n}};\n\n"
        ));
    }
    out.push_str(&format!("{METHOD_TYPE_REGISTER_DECL}\n{{\n"));
    if publications.is_empty() {
        out.push_str("    (void)module;\n    return 0;\n}\n");
        return out;
    }
    out.push_str("    PyObject *type;\n");
    for class in publications.iter().map(|published| &published.class) {
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

/// One class's `Py_tp_init`: the boundary `mod.Class(...)` crosses.
///
/// This is `wrapper_for`'s ingress half rewritten for a slot that is not a
/// `METH_FASTCALL` entry point, and every difference is forced by that:
///
/// * **Keywords.** `METH_FASTCALL` gives every other export D-244 rule 7's
///   closed keyword boundary for free, because CPython refuses a keyword
///   before the wrapper is entered. `tp_init` is handed `kwds` and is the
///   only thing that will ever look at it, so the same refusal is written
///   out by hand. Without it `mod.Grid(3, 4, nope=1)` would silently
///   *ignore* the keyword.
/// * **Arity.** `nargs` becomes `PyTuple_Size(args)`, with `wrapper_for`'s
///   message shape and spelling. CPython names the same subject for a
///   Python class -- `Grid.__init__() takes ...` -- which is exactly what
///   [`source_level_name`] returns for the mangled `Grid.__init__`.
/// * **Argument access.** `args[i]` becomes `PyTuple_GetItem(args, i)`.
///   Everything past that bridge is [`unpack_args`], shared verbatim with
///   `wrapper_for`: the unpack helper is a function of the declared type
///   alone, so `mod.Grid(True, 4)` and `mod.Grid(3, 4).scale(True)` admit
///   the same object set for the same declared `int`. `docs/RUNTIME.md`
///   claims one admissibility matrix, not two.
/// * **Failure.** Every bail returns `-1`, not `NULL`.
///
/// The parameter list is rendered from the *carried* tail -- `ExtCtor`
/// already dropped `self` -- and the leading `void *` is prepended
/// textually, exactly as `wrapper_for` does for a receiver, so this
/// declaration and codegen's own definition of the same symbol agree.
///
/// **No null guard on the `fnptr_` slot,** matching `wrapper_for`'s own
/// bare call: the module entry point binds every slot at import, so no host
/// call can reach an unbound one, and guarding only here would make the
/// constructor behave differently from every other export.
///
/// The `pycc_rt_instance_new` allocation is not released when the
/// constructor raises. That is D-107/D-154's no-free design, the same
/// reason the instance survives `tp_dealloc`, not an oversight of this path.
fn tp_init_c(ctor: &ExtCtor) -> String {
    let class = &ctor.class;
    let arity = ctor.params.len();
    let slots: Vec<_> = super::slot_carriers(&ctor.params, &ctor.param_writable);
    let symbol = pycc_codegen::mangle_ext_name(&ctor.name);
    let source_name = source_level_name(&ctor.name);
    // A constructor never returns a value at all, so it can carry neither a
    // tuple out-slot nor #1179's buffer sub-range out-slots.
    let carried = c_param_list(&slots, &[], false);
    let params = if carried == "void" {
        "void *".to_string()
    } else {
        format!("void *, {carried}")
    };
    let slot_count = ctor.slot_count;
    let mut out = format!("extern void *fnptr_{symbol};\n");
    out.push_str(&format!(
        "static int pycc_ext_tp_init_{class}(PyObject *self, PyObject *args, PyObject *kwds)\n\
         {{\n    void *inst;\n"
    ));
    out.push_str(&arg_slot_locals(&slots));
    out.push_str(&format!(
        "    if (kwds != NULL && PyDict_Size(kwds) != 0) {{\n        \
         PyErr_SetString(PyExc_TypeError, \"{source_name}() takes no keyword arguments\");\n        \
         return -1;\n    }}\n"
    ));
    out.push_str(&format!(
        "    if (PyTuple_Size(args) != {arity}) {{\n        PyErr_Format(PyExc_TypeError, \
         \"{source_name}() takes exactly {arity} argument{plural} (%zd given)\", \
         PyTuple_Size(args));\n        return -1;\n    }}\n",
        plural = if arity == 1 { "" } else { "s" },
    ));
    out.push_str(&unpack_args(
        &slots,
        source_name,
        &|index| format!("PyTuple_GetItem(args, {index})"),
        "        return -1;\n",
    ));
    out.push_str(&format!("    inst = pycc_rt_instance_new({slot_count});\n"));
    let mut call_args = vec!["inst".to_string()];
    for (index, slot) in slots.iter().enumerate() {
        // Deliberately two arms where `wrapper_for` has three.
        // `ctor_descriptor` refuses a `tuple` parameter outright -- no
        // `pycc_ext_thunk_` exists for a constructor, so there would be no
        // callable C entry point for the flattened elements -- which leaves
        // a scalar as the only other carrier a constructor slot can hold. A
        // third arm for `BoundaryCarrier::Tuple` would be a line no test
        // could ever execute, which the 100%-changed-lines invariant does
        // not admit (D-242 rule 1).
        call_args.push(match slot {
            super::BoundaryCarrier::Buffer { .. } => format!("&a{index}"),
            _ => format!("a{index}"),
        });
    }
    out.push_str(&format!(
        "    ((void (*)({params}))fnptr_{symbol})({call_args});\n",
        call_args = call_args.join(", ")
    ));
    // The same order `wrapper_for` uses, for the same reason: a compiled
    // function that raised left the runtime's thread-local flag set and the
    // carrier must never be published, so the flag is read before `inst` is
    // stored -- and every `Py_buffer` this slot acquired is released on
    // both arms.
    out.push_str(&format!(
        "    if (pycc_rt_ext_pending_type() >= 0) {{\n{}        \
         pycc_ext_raise_pending();\n        return -1;\n    }}\n{}",
        buffer_releases(&slots, "        "),
        buffer_releases(&slots, "    ")
    ));
    out.push_str("    ((PyccExtInstance *)self)->inst = inst;\n    return 0;\n}\n\n");
    out
}
