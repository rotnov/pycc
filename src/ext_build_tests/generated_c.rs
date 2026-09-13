//! Generated-text tests for the `--ext` build seam: the exact C the driver
//! writes into the exports companion, and the properties of the tracked shim
//! the generated wrappers depend on by name.
//!
//! Split out of `ext_build_tests.rs` by #1049 under `AGENTS.md`'s
//! decomposability rule; the tests themselves are unchanged except for the
//! `str` coverage #1049 added.

use super::*;

#[test]
fn the_generated_companion_defines_the_module_name_in_both_spellings() {
    let inc = generate_exports_inc("fastmath", &[]);
    assert!(
        inc.contains("#define PYCC_EXT_MODULE_NAME fastmath\n"),
        "{inc}"
    );
    assert!(
        inc.contains("#define PYCC_EXT_MODULE_NAME_STR \"fastmath\"\n"),
        "{inc}"
    );
    // An export-free module still emits a terminated method table, which is
    // what `PyModuleDef` requires.
    assert!(
        inc.contains("static PyMethodDef pycc_ext_methods[] = {"),
        "{inc}"
    );
    assert!(inc.contains("    {NULL, NULL, 0, NULL},\n};\n"), "{inc}");
}

#[test]
fn a_nullary_export_declares_a_void_parameter_list_and_checks_its_arity() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "answer".to_string(),
            params: Vec::new(),
            return_ty: Ty::Int,
        }],
    );
    assert!(inc.contains("extern void *fnptr_answer;"), "{inc}");
    assert!(
        inc.contains("result = ((long long (*)(void))fnptr_answer)();"),
        "{inc}"
    );
    assert!(
        inc.contains("takes exactly 0 arguments (%zd given)"),
        "{inc}"
    );
    assert!(
        inc.contains("(PyCFunction)(void (*)(void))pycc_ext_wrap_answer, METH_FASTCALL"),
        "{inc}"
    );
}

#[test]
fn a_unary_export_uses_the_singular_arity_message_and_unpacks_one_argument() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "square".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::Int,
        }],
    );
    assert!(
        inc.contains("takes exactly 1 argument (%zd given)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[0], \"square\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((long long (*)(long long))fnptr_square)(a0);"),
        "{inc}"
    );
    assert!(
        inc.contains("return pycc_ext_pack_int(\"square\", result);"),
        "{inc}"
    );
}

#[test]
fn a_binary_export_unpacks_each_argument_at_its_own_index() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "add".to_string(),
            params: vec![Ty::Int, Ty::Int],
            return_ty: Ty::Int,
        }],
    );
    assert!(
        inc.contains("takes exactly 2 arguments (%zd given)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[0], \"add\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[1], \"add\", 1, &a1)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((long long (*)(long long, long long))fnptr_add)(a0, a1);"),
        "{inc}"
    );
}

#[test]
fn every_wrapper_checks_the_runtime_exception_flag_before_packing_a_result() {
    // A compiled function that raised returns a neutral carrier and leaves
    // the thread-local flag set, so packing it first would hand Python a
    // fabricated value instead of the exception.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "risky".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::Int,
        }],
    );
    let check = inc
        .find("pycc_rt_ext_pending_type() >= 0")
        .expect("the wrapper checks the pending flag");
    let pack = inc
        .find("pycc_ext_pack_int(\"risky\"")
        .expect("the wrapper packs a result");
    assert!(
        check < pack,
        "the pending check must precede the pack:\n{inc}"
    );
}

#[test]
fn a_float_export_carries_a_double_through_every_slot_of_the_wrapper() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "scale".to_string(),
            params: vec![Ty::Float],
            return_ty: Ty::Float,
        }],
    );
    assert!(inc.contains("    double result;"), "{inc}");
    // The half-widened wrapper is the sharpest failure mode here: a
    // `long long result` left behind while the cast and the pack move to
    // `double` compiles clean, warns nothing, and returns 2 for 2.5.
    assert!(!inc.contains("long long result"), "{inc}");
    assert!(inc.contains("    double a0;"), "{inc}");
    assert!(
        inc.contains("pycc_ext_unpack_float(args[0], \"scale\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((double (*)(double))fnptr_scale)(a0);"),
        "{inc}"
    );
    // The float packer cannot fail on a value, so it takes no function name.
    assert!(inc.contains("return pycc_ext_pack_float(result);"), "{inc}");
}

#[test]
fn a_bool_export_uses_a_one_byte_c_type_to_match_the_compiled_i8_slot() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "negate".to_string(),
            params: vec![Ty::Bool],
            return_ty: Ty::Bool,
        }],
    );
    assert!(inc.contains("    char result;"), "{inc}");
    assert!(inc.contains("    char a0;"), "{inc}");
    assert!(
        inc.contains("pycc_ext_unpack_bool(args[0], \"negate\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((char (*)(char))fnptr_negate)(a0);"),
        "{inc}"
    );
    assert!(inc.contains("return pycc_ext_pack_bool(result);"), "{inc}");
    // `_Bool` is not `i1`-shaped here and `int` is four bytes: either would
    // disagree with the callee across an unchecked `void *` cast.
    assert!(!inc.contains("_Bool"), "{inc}");
    assert!(!inc.contains("(int (*)"), "{inc}");
}

#[test]
fn a_none_returning_export_casts_to_void_and_declares_no_result_at_all() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "sink".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::None,
        }],
    );
    // LLVM emits a `None` return as `void`, so there is nothing to hold and
    // nothing to assign -- both would be C type errors.
    assert!(!inc.contains("result"), "{inc}");
    assert!(
        inc.contains("((void (*)(long long))fnptr_sink)(a0);"),
        "{inc}"
    );
    assert!(inc.contains("    Py_RETURN_NONE;"), "{inc}");
    assert!(!inc.contains("pycc_ext_pack"), "{inc}");
}

#[test]
fn a_mixed_signature_gives_each_slot_its_own_c_type_and_unpack_helper() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "mix".to_string(),
            params: vec![Ty::Int, Ty::Float, Ty::Bool],
            return_ty: Ty::Float,
        }],
    );
    assert!(
        inc.contains("    long long a0;\n    double a1;\n    char a2;\n"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_int(args[0], \"mix\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_float(args[1], \"mix\", 1, &a1)"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_bool(args[2], \"mix\", 2, &a2)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((double (*)(long long, double, char))fnptr_mix)(a0, a1, a2);"),
        "{inc}"
    );
}

#[test]
fn a_none_returning_wrapper_checks_the_exception_flag_before_returning_none() {
    // The arm with no result to inspect is exactly the one where skipping
    // the check would hand Python a fabricated `None` instead of the
    // exception the compiled function raised.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "risky".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::None,
        }],
    );
    let check = inc
        .find("pycc_rt_ext_pending_type() >= 0")
        .expect("the wrapper checks the pending flag");
    let egress = inc
        .find("Py_RETURN_NONE;")
        .expect("the wrapper returns None");
    assert!(
        check < egress,
        "the pending check must precede the None egress:\n{inc}"
    );
}

#[test]
fn a_str_export_carries_an_opaque_pointer_in_both_positions() {
    // Part 2 of #1037 (#1049). `ty_to_basic_type` gives `Ty::Str` a pointer,
    // so the cast the wrapper calls through must spell `void *` at the
    // parameter and the return alike -- a width that disagrees with the
    // callee is a silent miscompile, not a compile error. Note the space
    // after the star: the declaration is `{c_type} a{index}`.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "shout".to_string(),
            params: vec![Ty::Str],
            return_ty: Ty::Str,
        }],
    );
    assert!(inc.contains("    void * result;\n"), "{inc}");
    assert!(inc.contains("    void * a0;\n"), "{inc}");
    assert!(
        inc.contains("pycc_ext_unpack_str(args[0], \"shout\", 0, &a0)"),
        "{inc}"
    );
    assert!(
        inc.contains("result = ((void * (*)(void *))fnptr_shout)(a0);"),
        "{inc}"
    );
    // `pack_str` takes no function name: unlike `pack_int` it refuses no
    // value, so there is no message for a name to appear in.
    assert!(inc.contains("return pycc_ext_pack_str(result);"), "{inc}");
}

#[test]
fn a_str_unpack_failure_releases_every_str_argument_already_taken() {
    // `pycc_ext_unpack_str` hands back a fresh `PyStrObj` that only the
    // compiled function's own parameter slot ever consumes. A `TypeError` on
    // a later argument bails before the call, so without this cleanup every
    // raising call would leak each earlier `str`.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "join".to_string(),
            params: vec![Ty::Str, Ty::Str],
            return_ty: Ty::Str,
        }],
    );
    // The first argument's own failure branch owes nothing -- nothing has
    // been taken yet -- and the second's owes exactly the first.
    assert!(
        inc.contains(
            "    if (pycc_ext_unpack_str(args[0], \"join\", 0, &a0) != 0) {\n        return NULL;\n    }\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains(
            "    if (pycc_ext_unpack_str(args[1], \"join\", 1, &a1) != 0) {\n        pycc_rt_str_decref(a0);\n        return NULL;\n    }\n"
        ),
        "{inc}"
    );

    // The invariant is positional, not an absence: a release may appear
    // before the call and must never appear after it. A decref after the
    // call would be a use-after-free, because the callee's parameter slot
    // already owns the reference and `decref_str_slot_before_store` releases
    // it on any reassignment. Anchored on `))fnptr_` rather than `fnptr_`:
    // the wrapper's first line is `extern void *fnptr_join;`, so a bare
    // search would land at offset 0 and invert the comparison.
    let call = inc.find("))fnptr_join)").expect("the indirect call");
    let egress = inc
        .find("return pycc_ext_pack_str(result);")
        .expect("the egress");
    assert!(call < egress, "{inc}");
    for (offset, _) in inc.match_indices("pycc_rt_str_decref") {
        assert!(
            offset < call,
            "a release at {offset} lands at or after the call at {call}: the callee's \
             parameter slot owns the reference once the call is made, so releasing it \
             here is a use-after-free\n{inc}"
        );
    }
}

#[test]
fn the_embedded_shim_is_the_tracked_c_file_and_declares_the_limited_api_floor() {
    assert!(
        SHIM_C.contains("#define Py_LIMITED_API 0x030D0000"),
        "{SHIM_C}"
    );
    // The multi-phase init contract: `PyInit_` only returns the def.
    assert!(SHIM_C.contains("PyModuleDef_Init(&pycc_ext_moduledef)"));
    // The free-threaded guard, and the detector that actually works:
    // the version string, not `sys._is_gil_enabled()`.
    assert!(SHIM_C.contains("strstr(Py_GetVersion(), \"free-threading build\")"));
    // It includes the generated companion under the exact name this module
    // writes it as, so the two really do land side by side.
    assert!(SHIM_C.contains(&format!("#include \"{EXPORTS_INC_NAME}\"")));
    assert_eq!(SHIM_C_NAME, "pycc_ext_module.c");
    // The helpers the generated wrappers call by name for each carried
    // type. A wrapper that names a helper the shim does not define fails
    // at C compile time, which no unit test here can reach.
    for helper in [
        "static int pycc_ext_unpack_int(PyObject *obj",
        "static int pycc_ext_unpack_float(PyObject *obj",
        "static int pycc_ext_unpack_bool(PyObject *obj",
        "static PyObject *pycc_ext_pack_int(",
        "static PyObject *pycc_ext_pack_float(double value)",
        "static PyObject *pycc_ext_pack_bool(char value)",
        "static int pycc_ext_unpack_str(PyObject *obj",
        "static PyObject *pycc_ext_pack_str(void *result)",
    ] {
        assert!(SHIM_C.contains(helper), "{helper}");
    }
    // The module body runs in the exec slot, never in `PyInit_`.
    assert!(SHIM_C.contains("{Py_mod_exec, (void *)pycc_ext_exec_module}"));
    assert!(SHIM_C.contains("Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED"));
    // The symbol codegen emits for the module body under `--ext`.
    assert!(SHIM_C.contains(pycc_codegen::EXT_MODULE_EXEC_SYMBOL));
}

#[test]
fn the_shims_bigint_egress_releases_the_reference_before_it_raises() {
    // D-180 rule 6 hands the wrapper a retained return value, so the arm
    // the D-244 amendment turns into an `OverflowError` has to drop it --
    // otherwise every refused bigint result leaks a `BigIntObj`.
    assert!(SHIM_C.contains("extern void pycc_rt_bigint_release(long long word);"));
    let body = &SHIM_C[SHIM_C
        .find("static PyObject *pycc_ext_pack_int(")
        .expect("the pack helper")..];
    let bigint_arm = body.find("case PYCC_EXT_INT_BIGINT:").expect("bigint arm");
    let release = body
        .find("pycc_rt_bigint_release(encoded);")
        .expect("the release");
    let raise = body.find("PyExc_OverflowError").expect("the OverflowError");
    assert!(
        bigint_arm < release && release < raise,
        "the reference must be dropped on the bigint arm, before the raise"
    );
    // The unclassifiable-word arm must not release: it is not a pointer
    // this code knows to be live.
    let fallback = &body[body.find("    default:").expect("the default arm")..];
    assert!(!fallback.contains("pycc_rt_bigint_release"), "{fallback}");
}

#[test]
fn the_shims_str_egress_releases_the_reference_unconditionally() {
    // D-180 rule 6 hands the wrapper a retained `str`, and the CPython
    // object `PyUnicode_FromStringAndSize` builds is an independent copy --
    // so the pycc-side reference dies here on every path, the
    // allocation-failure one included. Releasing only on success would leak
    // a `PyStrObj` exactly when the interpreter is already out of memory.
    assert!(SHIM_C.contains("extern void pycc_rt_str_decref(void *s);"));
    assert!(
        SHIM_C.contains("extern const unsigned char *pycc_rt_ext_str_bytes(void *s, size_t *len);")
    );
    let shim = shim_c();
    let body = &shim[shim
        .find("static PyObject *pycc_ext_pack_str(void *result)")
        .expect("the pack helper")..];
    let end = body.find("\n}\n").expect("the helper's end");
    let body = &body[..end];
    let pack = body
        .find("packed = PyUnicode_FromStringAndSize(")
        .expect("the CPython copy");
    let release = body
        .find("pycc_rt_str_decref(result);")
        .expect("the release");
    let ret = body.find("return packed;").expect("the return");
    assert!(
        pack < release && release < ret,
        "the reference must be released after the copy and before the return, on \
         both the success and the failure path:\n{body}"
    );
    // No `if (packed == NULL)` guard around the release: an early return on
    // the failure path is exactly the leak this pins shut.
    assert!(!body.contains("if (packed"), "{body}");
}

#[test]
fn the_shims_str_ingress_checks_the_type_before_the_converter_and_carries_a_length() {
    // `PyUnicode_Check` first and no converter fallback, for the same reason
    // `unpack_float` refuses an `int`: D-244 rule 7 defers to
    // `docs/TYPE_SYSTEM.md` rule 4 (D-086), so an `os.PathLike` or a
    // `__str__` duck type is a `TypeError` here rather than a coercion.
    let shim = shim_c();
    let body = &shim[shim
        .find("static int pycc_ext_unpack_str(PyObject *obj")
        .expect("the unpack helper")..];
    let check = body.find("!PyUnicode_Check(obj)").expect("the type check");
    let type_error = body.find("PyExc_TypeError").expect("the refusal");
    let convert = body
        .find("utf8 = PyUnicode_AsUTF8AndSize(obj, &size);")
        .expect("the conversion");
    assert!(
        check < type_error && type_error < convert,
        "the type must be refused before the converter runs:\n{body}"
    );
    // The length is carried explicitly rather than re-derived: a Python
    // `str` may hold embedded NUL bytes, and the empty string is legal.
    assert!(!body.contains("strlen"), "{body}");
    assert!(
        body.contains("pycc_rt_str_from_literal((const unsigned char *)utf8, (long long)size)"),
        "{body}"
    );
    // A lone surrogate has no UTF-8 encoding: `PyUnicode_AsUTF8AndSize`
    // raises `UnicodeEncodeError`, which the D-244 amendment propagates
    // verbatim rather than translating into a `TypeError`.
    assert!(
        body.contains("if (utf8 == NULL) {\n        return -1;\n    }"),
        "{body}"
    );
}

#[test]
fn the_shims_unpack_order_puts_bool_before_int_before_the_type_error() {
    // Searched inside the function body, past its own doc comment, so the
    // comment's prose ordering cannot stand in for the code's. `_at` is the
    // body (#1050 added the element-index parameter there); the
    // `pycc_ext_unpack_int` of the scalar generated C is now a one-line
    // forwarder onto it, and both share this one ordering.
    let body = &SHIM_C[SHIM_C
        .find("static int pycc_ext_unpack_int_at(PyObject *obj")
        .expect("the unpack helper")..];
    let bool_check = body.find("PyBool_Check(obj)").expect("bool arm");
    let long_check = body.find("!PyLong_Check(obj)").expect("int arm");
    let convert = body
        .find("raw = PyLong_AsLongLongAndOverflow")
        .expect("conversion");
    assert!(
        bool_check < long_check && long_check < convert,
        "D-141's bool markers must be read before the int conversion flattens them"
    );
    // The inline-integer range, never `i64`: the gate is the runtime's own
    // encoder, called after CPython's overflow check (#1040).
    assert!(SHIM_C.contains("pycc_rt_ext_int_encode(raw, out) != 0"));
    assert!(SHIM_C.contains("2**62"));
}

#[test]
fn a_tuple_parameter_is_checked_once_then_unpacked_element_by_element() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "total".to_string(),
            params: vec![Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float]))],
            return_ty: Ty::Int,
        }],
    );
    // One `tuple` argument, so the arity message still says one: the
    // wrapper's argument space is the Python one and never the flattened C
    // one (#1050).
    assert!(
        inc.contains("takes exactly 1 argument (%zd given)"),
        "{inc}"
    );
    // Shape and length first -- that check is what makes every
    // `PyTuple_GetItem` below infallible.
    assert!(
        inc.contains("pycc_ext_unpack_tuple(args[0], \"total\", 0, 2) != 0"),
        "{inc}"
    );
    assert!(
        inc.contains(
            "pycc_ext_unpack_int_at(PyTuple_GetItem(args[0], 0), \"total\", 0, 0, &a0_0) != 0"
        ),
        "{inc}"
    );
    assert!(
        inc.contains(
            "pycc_ext_unpack_float_at(PyTuple_GetItem(args[0], 1), \"total\", 0, 1, &a0_1) != 0"
        ),
        "{inc}"
    );
    // The locals are per element and the call spreads them in place; no
    // aggregate is ever spelled in C.
    assert!(
        inc.contains("    long long a0_0;\n    double a0_1;\n"),
        "{inc}"
    );
    assert!(
        inc.contains("    result = pycc_ext_thunk_total(a0_0, a0_1);\n"),
        "{inc}"
    );
    assert!(
        inc.contains("extern long long pycc_ext_thunk_total(long long, double);\n"),
        "{inc}"
    );
}

#[test]
fn a_tuple_return_arrives_through_out_pointers_and_is_packed_afterwards() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "split".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])),
        }],
    );
    assert!(
        inc.contains("extern void pycc_ext_thunk_split(long long, long long *, char *);\n"),
        "{inc}"
    );
    // No `result` local: a tuple return has no single value, and declaring
    // one would be an unused local at best and a type error at worst.
    assert!(!inc.contains("    result;"), "{inc}");
    assert!(!inc.contains("result = pycc_ext_thunk_split"), "{inc}");
    assert!(
        inc.contains("    pycc_ext_thunk_split(a0, &r0, &r1);\n"),
        "{inc}"
    );
    // The pending-exception check stands between the call and any read of
    // the out-pointer locals: a raising call leaves them uninitialized, so
    // packing first would be undefined behaviour rather than a wrong value.
    let call = inc.find("pycc_ext_thunk_split(a0").expect("the call");
    let pending = inc
        .find("pycc_rt_ext_pending_type() >= 0")
        .expect("the check");
    let first_pack = inc.find("e0 = pycc_ext_pack_int").expect("the first pack");
    assert!(call < pending && pending < first_pack, "{inc}");
    // Every element is packed before any failure is acted on: the retain
    // above gives this wrapper a reference the packer then discharges, so
    // bailing at the first failure would leak the rest.
    let second_pack = inc
        .find("e1 = pycc_ext_pack_bool(r1);")
        .expect("the second");
    let null_test = inc.find("if (e0 == NULL || e1 == NULL)").expect("the test");
    assert!(second_pack < null_test, "{inc}");
    assert!(
        inc.contains("        Py_XDECREF(e0);\n        Py_XDECREF(e1);\n"),
        "{inc}"
    );
    // `PyTuple_New` is reached with a fully-owned set, so its own failure
    // path releases unconditionally rather than with `Py_XDECREF`.
    assert!(
        inc.contains(
            "    packed = PyTuple_New(2);\n    if (packed == NULL) {\n        \
             Py_DECREF(e0);\n        Py_DECREF(e1);\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains(
            "    PyTuple_SetItem(packed, 0, e0);\n    PyTuple_SetItem(packed, 1, e1);\n    \
             return packed;\n"
        ),
        "{inc}"
    );
}

#[test]
fn a_tuple_return_retains_each_int_element_before_packing_it() {
    // #1050 regression: a returned tuple's fields arrive *borrowed*. Codegen
    // retains at a `return` only for a `Scalar::Int`, and the export thunk
    // `extractvalue`s each field straight into its out-pointer, so a stored
    // tuple (`saved = (2**62,)`; `return saved`) hands the wrapper a word the
    // module global still owns. `pycc_ext_pack_int` releases that word on its
    // `OverflowError` path, so without this retain the second call to the
    // export faults inside the host interpreter.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "split".to_string(),
            params: vec![],
            return_ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float])),
        }],
    );
    assert!(
        inc.contains(
            "    pycc_rt_bigint_retain(r0);\n    e0 = pycc_ext_pack_int(\"split\", r0);\n"
        ),
        "{inc}"
    );
    // Retain and release share one predicate, so the pairing has to come from
    // one list: only the `int` slots take a reference, because only the `int`
    // packer discharges one. A retain at a `bool` or `float` slot would be an
    // unbalanced +1 on every successful call.
    assert!(!inc.contains("pycc_rt_bigint_retain(r1)"), "{inc}");
    assert!(!inc.contains("pycc_rt_bigint_retain(r2)"), "{inc}");
    assert_eq!(inc.matches("pycc_rt_bigint_retain(").count(), 1, "{inc}");
}

#[test]
fn a_one_element_tuple_keeps_its_tuple_shape_in_both_directions() {
    // Arity 1 is the shape a flattening bug erases first: `tuple[int]` and
    // `int` occupy the same single slot at the thunk, and only the
    // `PyTuple_Check` on the way in and the `PyTuple_New(1)` on the way out
    // keep the Python-level types apart.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "wrap".to_string(),
            params: vec![Ty::Tuple(Box::new(vec![Ty::Int]))],
            return_ty: Ty::Tuple(Box::new(vec![Ty::Int])),
        }],
    );
    assert!(
        inc.contains("extern void pycc_ext_thunk_wrap(long long, long long *);\n"),
        "{inc}"
    );
    assert!(
        inc.contains("pycc_ext_unpack_tuple(args[0], \"wrap\", 0, 1) != 0"),
        "{inc}"
    );
    assert!(inc.contains("    packed = PyTuple_New(1);\n"), "{inc}");
}

#[test]
fn several_tuple_parameters_keep_one_local_namespace_each() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "dot".to_string(),
            params: vec![
                Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])),
                Ty::Tuple(Box::new(vec![Ty::Float, Ty::Bool])),
            ],
            return_ty: Ty::Float,
        }],
    );
    // `a{argument}_{element}` and never a single running counter: the second
    // tuple's first element is `a1_0`, not `a2`.
    assert!(
        inc.contains(
            "    long long a0_0;\n    long long a0_1;\n    double a1_0;\n    char a1_1;\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains("    result = pycc_ext_thunk_dot(a0_0, a0_1, a1_0, a1_1);\n"),
        "{inc}"
    );
    // Each tuple is checked against its own arity, at its own argument
    // index, and each element reports its own position.
    assert!(
        inc.contains("pycc_ext_unpack_tuple(args[1], \"dot\", 1, 2) != 0"),
        "{inc}"
    );
    assert!(
        inc.contains(
            "pycc_ext_unpack_bool_at(PyTuple_GetItem(args[1], 1), \"dot\", 1, 1, &a1_1) != 0"
        ),
        "{inc}"
    );
    assert!(
        inc.contains("takes exactly 2 arguments (%zd given)"),
        "{inc}"
    );
}

#[test]
fn an_earlier_str_argument_is_released_when_a_later_tuple_is_refused() {
    // The only owning argument type is `str`, and a refused `tuple` after it
    // exits before the call that would have consumed the reference. A tuple
    // owes nothing itself: its elements are copied out by value.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "tag".to_string(),
            params: vec![Ty::Str, Ty::Tuple(Box::new(vec![Ty::Int]))],
            return_ty: Ty::Str,
        }],
    );
    assert!(
        inc.contains(
            "    if (pycc_ext_unpack_tuple(args[1], \"tag\", 1, 1) != 0) {\n        \
             pycc_rt_str_decref(a0);\n        return NULL;\n    }\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains(
            "    if (pycc_ext_unpack_int_at(PyTuple_GetItem(args[1], 0), \"tag\", 1, 0, &a1_0) \
             != 0) {\n        pycc_rt_str_decref(a0);\n        return NULL;\n    }\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains("    result = pycc_ext_thunk_tag(a0, a1_0);\n"),
        "{inc}"
    );
}

#[test]
fn a_nullary_export_returning_a_tuple_declares_only_its_out_pointers() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "origin".to_string(),
            params: Vec::new(),
            return_ty: Ty::Tuple(Box::new(vec![Ty::Float, Ty::Float])),
        }],
    );
    // The combined list is non-empty even though the export takes nothing,
    // so `void` as a parameter list would be wrong here -- the out-pointers
    // are real parameters.
    assert!(
        inc.contains("extern void pycc_ext_thunk_origin(double *, double *);\n"),
        "{inc}"
    );
    assert!(
        inc.contains("    pycc_ext_thunk_origin(&r0, &r1);\n"),
        "{inc}"
    );
    assert!(
        inc.contains("takes exactly 0 arguments (%zd given)"),
        "{inc}"
    );
    // `pack_float` takes no function name: `PyFloat_FromDouble` cannot fail
    // on a value, unlike D-141's bigint egress.
    assert!(inc.contains("    e0 = pycc_ext_pack_float(r0);\n"), "{inc}");
}

#[test]
fn the_thunk_is_declared_as_a_function_and_called_without_a_cast() {
    // The load-bearing structural property of the whole seam. pycc's
    // aggregate calling convention is not the platform C struct ABI --
    // measured on aarch64-apple-darwin, a pycc function returning
    // `tuple[int x 5]` hands the words back in `x0`-`x4` where clang would
    // pass a hidden `sret` pointer -- so the thunk exists to keep every
    // aggregate on the LLVM side. Declaring it the way the scalar path
    // declares `fnptr_<name>` (a `void *` global, cast at the call to a
    // function-pointer type) reproduced a SIGBUS: the cast form type-checks
    // against nothing, and the `--ext` link defers undefined symbols
    // (`-undefined dynamic_lookup` on Mach-O, `-Bsymbolic` on ELF) so a
    // disagreement here links cleanly and dies on the first call.
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "pair".to_string(),
            params: vec![Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int]))],
            return_ty: Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])),
        }],
    );
    assert!(
        inc.contains(
            "extern void pycc_ext_thunk_pair(long long, long long, long long *, \
             long long *);\n"
        ),
        "{inc}"
    );
    assert!(!inc.contains("extern void *pycc_ext_thunk_"), "{inc}");
    assert!(
        !inc.contains("(*)(long long, long long, long long *"),
        "{inc}"
    );
    assert!(!inc.contains("fnptr_pair"), "{inc}");
    // A scalar-only export in the same module keeps the legacy spelling
    // byte for byte: #1050 widens the boundary without rewriting what
    // #1036 through #1049 already emit.
    let scalar = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "square".to_string(),
            params: vec![Ty::Int],
            return_ty: Ty::Int,
        }],
    );
    assert!(scalar.contains("extern void *fnptr_square;"), "{scalar}");
    assert!(
        scalar.contains("result = ((long long (*)(long long))fnptr_square)(a0);"),
        "{scalar}"
    );
    assert!(!scalar.contains("pycc_ext_thunk_"), "{scalar}");
}

#[test]
fn a_tuple_carrying_export_returning_none_assigns_nothing_and_fabricates_none() {
    let inc = generate_exports_inc(
        "m",
        &[ExtExport {
            name: "record".to_string(),
            params: vec![Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool]))],
            return_ty: Ty::None,
        }],
    );
    assert!(
        inc.contains("extern void pycc_ext_thunk_record(long long, char);\n"),
        "{inc}"
    );
    assert!(
        inc.contains("    pycc_ext_thunk_record(a0_0, a0_1);\n"),
        "{inc}"
    );
    assert!(!inc.contains("result"), "{inc}");
    assert!(inc.contains("    Py_RETURN_NONE;\n"), "{inc}");
}
