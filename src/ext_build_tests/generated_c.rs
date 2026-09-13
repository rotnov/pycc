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
    let body = &SHIM_C[SHIM_C
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
    let body = &SHIM_C[SHIM_C
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
    // comment's prose ordering cannot stand in for the code's.
    let body = &SHIM_C[SHIM_C
        .find("static int pycc_ext_unpack_int(PyObject *obj")
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
