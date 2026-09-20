//! The generated `METH_FASTCALL` wrapper and the slot machinery it and
//! `Py_tp_init` share (extracted while Part 1 of #1142 taught the slot
//! vector per-parameter writability).
//!
//! One cohesion: everything here turns a carried signature into the C text
//! that moves arguments across the boundary -- the per-parameter carrier
//! vector, the wrapper body, its argument locals, its unpack calls, its
//! buffer releases, its C parameter list and its return packer. Nothing
//! here decides *which* functions are exported or *whether* a type is
//! carriable; those stay in the parent module with the collectors.

use pycc_types::Ty;

use super::carrier::{
    BUFFER_VIEW_C_TYPE, BoundaryCarrier, SlotCleanup, boundary_carrier, return_c_type,
};
use super::{ExtExport, ExtReceiver, source_level_name};

/// One export's or constructor's slot vector, with Part 1 of #1142's
/// writability applied.
///
/// The single place a [`BoundaryCarrier::Buffer`]'s `writable` flag is ever
/// set, shared by [`wrapper_for`] and `method_types`' `tp_init_c` -- the
/// only two places a slot vector is built -- so a `METH_FASTCALL` wrapper
/// and a `Py_tp_init` can never request different buffer flags for the same
/// declared parameter of the same body.
pub(crate) fn slot_carriers(params: &[Ty], param_writable: &[bool]) -> Vec<BoundaryCarrier> {
    // `zip` truncates to the shorter side rather than failing, and a slot
    // vector shorter than `params` would emit a wrapper that unpacks fewer
    // arguments than it declares. The two vectors are built together in
    // `collect_exports` and `ctor_descriptor`, so a mismatch is a bug in a
    // fixture or in a future producer, and it is made loud here rather than
    // silently miscompiled.
    assert_eq!(
        params.len(),
        param_writable.len(),
        "pycc: internal error: a slot vector's writability flags must be parallel to its parameters"
    );
    params
        .iter()
        .zip(param_writable)
        .map(|(ty, writable)| {
            match boundary_carrier(ty).expect("only carriable parameters are collected") {
                BoundaryCarrier::Buffer { .. } => BoundaryCarrier::Buffer {
                    writable: *writable,
                },
                other => other,
            }
        })
        .collect()
}

/// One export's `METH_FASTCALL` wrapper.
///
/// `METH_FASTCALL` rather than `METH_VARARGS` for two reasons. It is the
/// only limited-API calling convention that receives the argument count
/// directly, so arity and type checking cost no tuple; and CPython itself
/// raises the keyword `TypeError` before the wrapper is entered, which is
/// exactly D-244 rule 7's closed boundary for free -- a `METH_VARARGS`
/// wrapper would silently accept `f(x=1)` at the C level.
///
/// A scalar-only signature's call goes through `fnptr_<name>`, the
/// module-level function-pointer global codegen emits for each `def`'s
/// binding, and *not* through the compiled function's own symbol. That is
/// what makes a rebound name (`f = g` at module level) call what Python
/// says it calls.
///
/// A signature carrying a `tuple` calls `pycc_ext_thunk_<name>` instead
/// (#1050), which dispatches through that same global one LLVM frame
/// further in and so keeps the rebinding property. The indirection exists
/// because C cannot spell a pycc aggregate: pycc's own convention for
/// passing and returning one is not the platform C struct ABI, so the thunk
/// presents the signature as scalars and out-pointers and every aggregate
/// stays on the LLVM side. That declaration is a real `extern` *function*
/// declaration and the call is direct -- deliberately not the scalar path's
/// `void *` global cast to a function-pointer type. The cast form was
/// measured to fault (SIGBUS) on aarch64-apple-darwin for an out-pointer
/// signature, and it is also the only one of the two that no C compiler can
/// type-check at all.
pub(crate) fn wrapper_for(export: &ExtExport) -> String {
    let name = &export.name;
    let arity = export.params.len();
    // Every local, unpack helper and call slot below is a function of the
    // declared type alone and never of the object that arrives: `def
    // f(x: int)` gets `long long a0` and `pycc_ext_unpack_int` even though
    // that helper also accepts `True`, because widening the accepted object
    // set is the helper's business and never narrows the local.
    // `collect_exports` refused every type these two lookups cannot name, so
    // an export in hand always has both.
    let slots: Vec<BoundaryCarrier> = slot_carriers(&export.params, &export.param_writable);
    let return_c = return_c_type(&export.return_ty).expect("a carriable return type");
    let returns_none = export.return_ty == Ty::None;
    // One entry per element of a returned `tuple`, and empty for every
    // other return type: these become trailing out-pointer arguments, not
    // a return value.
    let out_slots: Vec<(&'static str, &'static str)> = match boundary_carrier(&export.return_ty) {
        Some(BoundaryCarrier::Tuple(elements)) => elements,
        _ => Vec::new(),
    };
    // #1050 keeps two index spaces apart on purpose. `arity` and every
    // `a{index}` below are per *Python argument* -- what `nargs` counts,
    // what the arity message names, and what an unpack failure has to clean
    // up after -- while the C call's own argument list is the flattened
    // one, built only at the call expression further down. Renumbering
    // `a{index}` to follow the flattened list would make
    // `def f(t: tuple[int, int])` report "takes exactly 2 arguments" for a
    // one-argument function.
    // `ext_thunk_required` asks only whether a *declared* type is a tuple,
    // and a receiver is never one, so the receiver-free tail gives the same
    // verdict the codegen side reaches from the MIR function's full
    // parameter list. The two therefore stay in agreement about whether a
    // thunk exists at all.
    let use_thunk = pycc_codegen::ext_thunk_required(name, &export.params, &export.return_ty);
    let thunk = pycc_codegen::ext_thunk_symbol(name);
    // A method's `name` is dotted, and these four sites paste it into C
    // identifiers: the thunk `extern` (through `ext_thunk_symbol`, which
    // mangles for itself), the `fnptr_` `extern`, the `fnptr_` cast at the
    // call, and this wrapper's own definition -- plus the `PyMethodDef` row
    // `generate_exports_inc` emits for it. The mangling is the identity for
    // a dot-free name, so every module-level function's wrapper is
    // byte-identical to what it was.
    let symbol = pycc_codegen::mangle_ext_name(name);
    // The `PyErr_Format` arity message is the one interpolation of `name`
    // that is *not* a C identifier: the host reads it, so it renders the
    // source-level spelling. Left alone it would say
    // `Grid.scale.static() takes exactly 1 argument` next to CPython's own
    // `Grid.scale() takes no keyword arguments` on the same object. The
    // unpack helpers below take the same spelling for the same reason.
    let source_name = source_level_name(name);
    // A `@classmethod`'s compiled signature leads with `cls` and an
    // instance method's with `self` (`ExtExport::receiver`); neither is a
    // carried argument. The leading `void *` is reinstated textually here,
    // so this declaration and `pycc_codegen`'s thunk -- which builds its own
    // list from the MIR function's parameters -- declare the same arity for
    // the same symbol. Only the *call* argument differs between the two:
    // `NULL` for `cls`, the unwrapped instance pointer for `self`.
    let params = {
        let carried = c_param_list(&slots, &out_slots);
        if export.receiver == ExtReceiver::None {
            carried
        } else if carried == "void" {
            // `c_param_list` answers `"void"` for an empty list, because an
            // empty C parameter list means "unspecified". With a receiver
            // the list is not empty.
            "void *".to_string()
        } else {
            format!("void *, {carried}")
        }
    };
    let mut out = String::new();
    if use_thunk {
        out.push_str(&format!("extern {return_c} {thunk}({params});\n"));
    } else {
        out.push_str(&format!("extern void *fnptr_{symbol};\n"));
    }
    out.push_str(&format!(
        "static PyObject *pycc_ext_wrap_{symbol}(PyObject *self, PyObject *const *args, \
         Py_ssize_t nargs)\n{{\n"
    ));
    // A `METH_STATIC` wrapper is handed `NULL` in `self` and a `METH_CLASS`
    // one the *type object*; both discard it, exactly as
    // `MirExpr::NullInstance` discards `cls` at a native call site -- the
    // method was compiled for one class, so nothing in its body reads the
    // receiver, and forwarding a CPython type pointer into a slot typed
    // `Ty::Instance` would be type confusion even though nothing
    // dereferences it.
    //
    // A plain `METH_FASTCALL` instance-method wrapper (#1145) is handed the
    // carrier object itself and *does* read it. The NULL guard is not
    // defence in depth: `mod.Grid.__new__(mod.Grid)` runs
    // `PyType_GenericNew`, which zeroes the carrier and never runs
    // `tp_init`, so `inst` really is NULL at the wrapper's entry and the
    // guard is what makes that a `TypeError` instead of a segfault.
    // CPython's own method-descriptor machinery has already refused a
    // `self` of the wrong type before this point, so no type check is
    // needed here.
    if export.receiver == ExtReceiver::SelfInstance {
        out.push_str(&format!(
            "    void *self_inst = ((PyccExtInstance *)self)->inst;\n    \
             if (self_inst == NULL) {{\n        PyErr_SetString(PyExc_TypeError, \
             \"{source_name}() called on an uninitialized instance\");\n        \
             return NULL;\n    }}\n"
        ));
    } else {
        out.push_str("    (void)self;\n");
    }
    out.push_str("    (void)args;\n");
    // A `-> None` export has no result to hold: codegen emits its return as
    // LLVM `void`, so a result local would be a C type error, not a waste.
    // A `tuple` return has no single result either -- its elements arrive
    // in the `r{index}` locals below, through the thunk's out-pointers.
    if !returns_none && out_slots.is_empty() {
        out.push_str(&format!("    {return_c} result;\n"));
    }
    for (index, (c_type, _)) in out_slots.iter().enumerate() {
        out.push_str(&format!("    {c_type} r{index};\n"));
        out.push_str(&format!("    PyObject *e{index};\n"));
    }
    if !out_slots.is_empty() {
        out.push_str("    PyObject *packed;\n");
    }
    out.push_str(&arg_slot_locals(&slots));
    out.push_str(&format!(
        "    if (nargs != {arity}) {{\n        PyErr_Format(PyExc_TypeError, \
         \"{source_name}() takes exactly {arity} argument{plural} (%zd given)\", nargs);\n        \
         return NULL;\n    }}\n",
        plural = if arity == 1 { "" } else { "s" },
    ));
    out.push_str(&unpack_args(
        &slots,
        source_name,
        &|index| format!("args[{index}]"),
        "        return NULL;\n",
    ));
    let mut call_args: Vec<String> = Vec::new();
    match export.receiver {
        ExtReceiver::None => {}
        ExtReceiver::NullCls => call_args.push("NULL".to_string()),
        ExtReceiver::SelfInstance => call_args.push("self_inst".to_string()),
    }
    for (index, slot) in slots.iter().enumerate() {
        match slot {
            BoundaryCarrier::Scalar(..) => call_args.push(format!("a{index}")),
            BoundaryCarrier::Tuple(elements) => {
                call_args.extend((0..elements.len()).map(|element| format!("a{index}_{element}")))
            }
            BoundaryCarrier::Buffer { .. } => call_args.push(format!("&a{index}")),
        }
    }
    call_args.extend((0..out_slots.len()).map(|index| format!("&r{index}")));
    let call_args = call_args.join(", ");
    // Nothing is assigned on the `-> None` arm (a `void` call has no value)
    // nor on the `tuple` arm (its elements arrive through the out-pointers).
    let assign = if returns_none || !out_slots.is_empty() {
        ""
    } else {
        "result = "
    };
    if use_thunk {
        out.push_str(&format!("    {assign}{thunk}({call_args});\n"));
    } else {
        out.push_str(&format!(
            "    {assign}(({return_c} (*)({params}))fnptr_{symbol})({call_args});\n"
        ));
    }
    // A compiled function that raised returns a neutral carrier and leaves
    // the runtime's thread-local flag set (see `pycc_codegen`'s
    // `exception_exit` block), so the carrier must never be packed: the
    // pending exception is checked first and translated into a CPython one.
    // This reads no part of the call's return value, so it stands unchanged
    // on the `-> None` arm -- where it is the only thing between a raised
    // exception and a fabricated `None`.
    //
    // #1050: its *position*, before the pack below, is load-bearing for a
    // `tuple` return in a way it is not for a scalar one. A raising call
    // leaves every `r{index}` out-pointer local exactly as uninitialized as
    // it found it, so a pack that ran first would read indeterminate
    // storage -- undefined behaviour, not merely a wrong value.
    // The success-path half of the cleanup discipline, and the half nothing
    // else in the tree would notice was missing: a `Py_buffer` acquired
    // before the call is still held after it returns. Emitted once, at the
    // single point every remaining exit passes through -- the pending-
    // exception bail and the pack below both sit after it -- rather than
    // duplicated at each `return`. Releasing before the pack is safe and
    // deliberate: the pack reads only `result`/`r{index}`, machine words the
    // compiled function already produced, never the buffer's storage.
    //
    // Empty for an export with no `memoryview` parameter, so every wrapper
    // generated before Part 1 of #1027 is byte-identical to what it was.
    let release: String = buffer_releases(&slots, "    ");
    out.push_str(&format!(
        "    if (pycc_rt_ext_pending_type() >= 0) {{\n{}        pycc_ext_raise_pending();\n        \
         return NULL;\n    }}\n{release}",
        buffer_releases(&slots, "        ")
    ));
    match &export.return_ty {
        Ty::None => out.push_str("    Py_RETURN_NONE;\n}\n\n"),
        Ty::Tuple(_) => out.push_str(&pack_tuple_return(source_name, &out_slots)),
        // `pack_int` is the one packer whose failure is a property of the
        // *value*, and the only one whose message therefore names the
        // function: D-141's bigint egress (#1040). `PyFloat_FromDouble` and
        // `PyBool_FromLong` cannot fail at all, and `pack_str` can only fail
        // the way any allocation can -- it refuses no `str` -- so none of
        // the three take a name. Arity is uniform across them, so every
        // packer but `int` shares the generic arm below.
        Ty::Int => out.push_str(&format!(
            "    return pycc_ext_pack_int(\"{source_name}\", result);\n}}\n\n"
        )),
        ty => {
            let (_, helper) = boundary_carrier(ty)
                .and_then(BoundaryCarrier::into_scalar)
                .expect("a carriable scalar return type");
            out.push_str(&format!(
                "    return pycc_ext_pack_{helper}(result);\n}}\n\n"
            ));
        }
    }
    out
}

/// The C local declarations one generated function needs for its argument
/// slots: `a{index}` per scalar, one `a{index}_{element}` per `tuple`
/// element, and a `Py_buffer b{index}` beside the view pair for a
/// `memoryview`.
///
/// Shared by [`wrapper_for`] and the generated `Py_tp_init`
/// (`method_types_c`) so a constructor and an ordinary export declare the
/// same locals for the same declared type.
pub(crate) fn arg_slot_locals(slots: &[BoundaryCarrier]) -> String {
    let mut out = String::new();
    for (index, slot) in slots.iter().enumerate() {
        match slot {
            BoundaryCarrier::Scalar(c_type, _) => {
                out.push_str(&format!("    {c_type} a{index};\n"));
            }
            BoundaryCarrier::Tuple(elements) => {
                for (element, (c_type, _)) in elements.iter().enumerate() {
                    out.push_str(&format!("    {c_type} a{index}_{element};\n"));
                }
            }
            // Two locals, both owned by the generated function for the
            // whole call: the `Py_buffer` the shim acquires (and the
            // function releases on every exit past that point), and the
            // `{ptr, len}` pair that is all the compiled body ever sees of
            // it.
            BoundaryCarrier::Buffer { .. } => {
                out.push_str(&format!("    Py_buffer b{index};\n"));
                out.push_str(&format!("    {BUFFER_VIEW_C_TYPE} a{index};\n"));
            }
        }
    }
    out
}

/// The per-argument ingress: one `pycc_ext_unpack_*` call per slot, each
/// bailing with `fail` after releasing whatever the earlier slots hold.
///
/// `arg_expr` renders the `PyObject *` for argument `index`, because the two
/// callers receive their arguments differently: a `METH_FASTCALL` wrapper
/// gets a `PyObject *const *` vector and indexes it, while the generated
/// `Py_tp_init` gets a real tuple and has to bridge through
/// `PyTuple_GetItem`. Everything else -- which helper, which local, what the
/// bail path owes -- is shared, which is the point: the unpack helper is a
/// function of the declared type alone, so `mod.Grid(True, 4)` and
/// `mod.Grid(3, 4).scale(True)` must admit exactly the same object set for
/// the same declared `int`. `docs/RUNTIME.md` claims one admissibility
/// matrix, not two.
///
/// Emitted inline rather than behind a shared `goto` label: the cleanup
/// differs per argument index, and neither caller has another exit that owes
/// anything at this point. A `tuple` argument owes nothing -- its elements
/// are copied out by value.
pub(crate) fn unpack_args(
    slots: &[BoundaryCarrier],
    source_name: &str,
    arg_expr: &dyn Fn(usize) -> String,
    fail: &str,
) -> String {
    let mut out = String::new();
    for (index, slot) in slots.iter().enumerate() {
        // Each `str` argument already unpacked holds a fresh reference that
        // only the compiled function's own parameter slot ever consumes, and
        // this branch bails before the call -- so release them here, or a
        // `TypeError` on argument 2 would leak argument 1's `PyStrObj` on
        // every raising call.
        let cleanup: String = slots[..index]
            .iter()
            .enumerate()
            .filter_map(|(earlier, carrier)| Some((earlier, carrier.cleanup()?)))
            .map(|(earlier, owed)| match owed {
                SlotCleanup::StrDecref => format!("        pycc_rt_str_decref(a{earlier});\n"),
                SlotCleanup::BufferRelease => format!("        PyBuffer_Release(&b{earlier});\n"),
            })
            .collect();
        let arg = arg_expr(index);
        match slot {
            BoundaryCarrier::Scalar(_, helper) => out.push_str(&format!(
                "    if (pycc_ext_unpack_{helper}({arg}, \"{source_name}\", {index}, &a{index}) \
                 != 0) {{\n{cleanup}{fail}    }}\n"
            )),
            BoundaryCarrier::Tuple(elements) => {
                let elements_len = elements.len();
                out.push_str(&format!(
                    "    if (pycc_ext_unpack_tuple({arg}, \"{source_name}\", {index}, \
                     {elements_len}) != 0) {{\n{cleanup}{fail}    }}\n"
                ));
                for (element, (_, helper)) in elements.iter().enumerate() {
                    // `PyTuple_GetItem` cannot fail at this call: the check
                    // just emitted refused every non-tuple and every length
                    // but this one, so the index is always in range.
                    out.push_str(&format!(
                        "    if (pycc_ext_unpack_{helper}_at(PyTuple_GetItem({arg}, \
                         {element}), \"{source_name}\", {index}, {element}, &a{index}_{element}) != 0) \
                         {{\n{cleanup}{fail}    }}\n"
                    ));
                }
            }
            // The shim refuses everything that is not an exact,
            // C-contiguous, one-dimensional `float` buffer and leaves
            // nothing acquired when it does, so this arm owes no cleanup of
            // its own -- only the earlier slots'. On success the caller
            // takes the two words it is allowed to keep: the data pointer,
            // and a *copy* of `shape[0]`. `b{index}.shape` itself is
            // exporter-owned storage that dies at `PyBuffer_Release`, so it
            // is never carried across the boundary.
            BoundaryCarrier::Buffer { writable } => {
                // The sole consumer of Part 1 of #1142's writability bit.
                // `1` asks the shim for `PyBUF_WRITABLE`, which is the only
                // thing that makes the element store this parameter's body
                // performs a write to storage the exporter agreed to share
                // mutably; a read-only exporter is refused here, by
                // CPython's own `BufferError`, rather than scribbled over.
                let writable = i32::from(*writable);
                out.push_str(&format!(
                    "    if (pycc_ext_unpack_memoryview({arg}, \"{source_name}\", {index}, \
                     {writable}, &b{index}) != 0) {{\n{cleanup}{fail}    }}\n"
                ));
                out.push_str(&format!("    a{index}.ptr = b{index}.buf;\n"));
                out.push_str(&format!(
                    "    a{index}.len = (long long)b{index}.shape[0];\n"
                ));
            }
        }
    }
    out
}

/// One `PyBuffer_Release` line per `memoryview` slot, indented with
/// `indent`, or the empty string when the export has none.
///
/// Shared by the two success-path emission points (inside the pending-
/// exception block, and just before the egress) so the two can never
/// release different sets.
pub(crate) fn buffer_releases(slots: &[BoundaryCarrier], indent: &str) -> String {
    slots
        .iter()
        .enumerate()
        .filter(|(_, carrier)| carrier.cleanup() == Some(SlotCleanup::BufferRelease))
        .map(|(index, _)| format!("{indent}PyBuffer_Release(&b{index});\n"))
        .collect()
}

/// The C parameter-type list of the call a wrapper makes into the compiled
/// program: every declared parameter flattened to the slots it occupies,
/// then one out-pointer per element of a returned `tuple`.
///
/// `"void"` and not `""` for the empty list, because an empty C parameter
/// list means "unspecified", not "none". The rule applies to the *combined*
/// list: only a nullary export with no `tuple` return has one.
pub(crate) fn c_param_list(
    slots: &[BoundaryCarrier],
    out_slots: &[(&'static str, &'static str)],
) -> String {
    let mut types: Vec<String> = Vec::new();
    for slot in slots {
        match slot {
            BoundaryCarrier::Scalar(c_type, _) => types.push((*c_type).to_string()),
            BoundaryCarrier::Tuple(elements) => {
                types.extend(elements.iter().map(|(c_type, _)| (*c_type).to_string()));
            }
            BoundaryCarrier::Buffer { .. } => types.push(format!("{BUFFER_VIEW_C_TYPE} *")),
        }
    }
    types.extend(out_slots.iter().map(|(c_type, _)| format!("{c_type} *")));
    if types.is_empty() {
        "void".to_string()
    } else {
        types.join(", ")
    }
}

/// The egress of a `tuple`-returning wrapper (#1050): pack every element,
/// then build the tuple.
///
/// Each `r{index}` arrives **borrowed**, not retained. D-180 rule 6 retains
/// at a `return` only where the returned value is a scalar: codegen's
/// `MirStmt::Return` routes the value through `retain_if_int_duplicate`,
/// which acts on a `Scalar::Int` and does nothing for an aggregate, and the
/// `pycc_ext_thunk_` emitter then `extractvalue`s each field straight into
/// its out-pointer. So returning a *stored* tuple (`saved = (2**62,)`;
/// `return saved`) hands this function a word the module global still owns.
/// `pycc_ext_pack_int` discharges one reference on its `OverflowError` path,
/// which without a matching retain here decrements a count this wrapper
/// never took -- a refcount underflow, and on the next call a use-after-free
/// in the host interpreter.
///
/// So each `int` element takes its own reference with `pycc_rt_bigint_retain`
/// immediately before the packer that discharges it. Retain and release share
/// one predicate (`classify_encoded_int(word) == BigInt`), so the pairing is
/// exactly balanced on a smallint, a bool marker, the word `0`, and an
/// unclassifiable word alike -- and the retain is emitted from the same
/// `helper == "int"` arm as the packer, so the two can never drift apart.
///
/// Every element is packed *unconditionally*, before any failure is acted
/// on, and that ordering is the refcount discipline rather than a style
/// choice. Bailing out at the first failing element would leave every later
/// element's word undischarged, leaking one `BigIntObj` per call on exactly
/// the path that already raises.
///
/// The cost is that when two `int` elements both overflow, the second
/// `PyErr_Format` replaces the first. Both carry the same message text and
/// the same exception type, so the observable difference is nil, and
/// replacing a pending exception is well-defined in CPython -- unlike
/// dropping an owned word.
///
/// `PyTuple_New` is reached only once every element is packed, so its own
/// failure path has a fixed, fully-owned set to release.
pub(crate) fn pack_tuple_return(name: &str, out_slots: &[(&'static str, &'static str)]) -> String {
    let mut out = String::new();
    for (index, (_, helper)) in out_slots.iter().enumerate() {
        let argument = if *helper == "int" {
            out.push_str(&format!("    pycc_rt_bigint_retain(r{index});\n"));
            format!("\"{name}\", r{index}")
        } else {
            format!("r{index}")
        };
        out.push_str(&format!(
            "    e{index} = pycc_ext_pack_{helper}({argument});\n"
        ));
    }
    let arity = out_slots.len();
    let any_null = (0..arity)
        .map(|index| format!("e{index} == NULL"))
        .collect::<Vec<_>>()
        .join(" || ");
    let x_release: String = (0..arity)
        .map(|index| format!("        Py_XDECREF(e{index});\n"))
        .collect();
    out.push_str(&format!(
        "    if ({any_null}) {{\n{x_release}        return NULL;\n    }}\n"
    ));
    let release: String = (0..arity)
        .map(|index| format!("        Py_DECREF(e{index});\n"))
        .collect();
    out.push_str(&format!(
        "    packed = PyTuple_New({arity});\n    if (packed == NULL) {{\n{release}        \
         return NULL;\n    }}\n"
    ));
    out.push_str(
        "    /* Every index is in range and `packed` is a fresh tuple, so each\n       \
         PyTuple_SetItem succeeds and steals its element reference. */\n",
    );
    for index in 0..arity {
        out.push_str(&format!(
            "    PyTuple_SetItem(packed, {index}, e{index});\n"
        ));
    }
    out.push_str("    return packed;\n}\n\n");
    out
}
