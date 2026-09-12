/*
 * The fixed half of a pycc `--ext` artifact (D-244, #1025 Part 1).
 *
 * This file is tracked, hand-written, and never generated. `pycc` embeds it
 * with `include_str!` (see `src/ext_build.rs`) and writes it into the build
 * scratch directory next to a small generated companion,
 * `pycc_ext_exports.inc`, which supplies the module name and the per-export
 * wrappers. Both are compiled as one translation unit by the same `cc`
 * invocation that links the artifact -- never during an ordinary
 * `cargo build`, which is what keeps CPython headers off the ordinary build
 * path entirely.
 *
 * Embedded rather than located by source-tree path on purpose: an installed
 * `pycc` has no source tree to look in, so a path-based lookup would work in
 * a checkout and break the moment the compiler is installed.
 *
 * Why C at all, rather than emitting `PyModuleDef`/`PyMethodDef` as LLVM IR:
 * those are exactly the struct layouts the stable ABI exists to hide. Hand-
 * modelling them in IR would re-introduce the version coupling
 * `Py_LIMITED_API` removes.
 *
 * Division of labour with `pycc_rt` (D-244 rule 2): every *decision* about
 * how a value is represented -- the inline-integer range, D-141's bool
 * marker words, the four-way classification of an encoded word, the pending
 * exception's type and message -- belongs to `pycc_rt::ext_bridge` and
 * `pycc_rt::exception`. This file only moves `PyObject*`s according to those
 * answers. That is what keeps `libpycc_rt.a` free of CPython symbols, which
 * matters because the same archive is linked into every `native` executable,
 * where no interpreter exists.
 */

#define Py_LIMITED_API 0x030D0000
#include <Python.h>
#include <string.h>

/*
 * `pycc_rt`'s boundary, declared by hand. `pycc_rt` exposes no C header and
 * needs none: these are plain `extern "C"` symbols with fixed signatures.
 */
extern int pycc_rt_ext_int_encode(long long value, long long *out);
extern long long pycc_rt_ext_bool_encode(int value);
extern int pycc_rt_ext_int_classify(long long encoded);
extern long long pycc_rt_ext_int_decode(long long encoded);
extern int pycc_rt_ext_pending_type(void);
extern const unsigned char *pycc_rt_ext_pending_message(size_t *len);
extern void pycc_rt_exception_clear(void);
/* D-180 rule 6: a compiled function's return value arrives retained, so a
 * heap-bigint result this boundary refuses still has to be released. */
extern void pycc_rt_bigint_release(long long word);

/* `pycc_rt::ext_bridge`'s classification codes. */
#define PYCC_EXT_INT_SMALLINT 0
#define PYCC_EXT_INT_FALSE 1
#define PYCC_EXT_INT_TRUE 2
#define PYCC_EXT_INT_BIGINT 3

/*
 * The compiled module body, emitted by `pycc_codegen` under
 * `EXT_MODULE_EXEC_SYMBOL`. Returns 0 on success, -1 with the pycc-side
 * exception state pending. Deliberately not named `main`: an extension
 * module that exported `main` would collide with the host interpreter's own.
 */
extern long long pycc_ext_module_exec(void);

/*
 * Translates a pending pycc exception into a CPython one and clears it.
 * Returns 1 when it raised, 0 when nothing was pending.
 *
 * This is what stops an `ext` artifact killing its host. `native` mode's
 * handler, `pycc_rt_exception_print_and_exit`, calls `exit(1)`; inside an
 * imported extension module that terminates the interpreter rather than
 * failing the call. Both `ext` callers use this instead, with the two
 * different conventions CPython imposes on them: a per-export wrapper then
 * returns NULL, and the `Py_mod_exec` slot returns -1.
 *
 * The tag values are `pycc_rt::exception`'s seven `EXCEPTION_TYPE_*`
 * constants, which this file cannot see from C. `ext_bridge`'s
 * `exception_type_tags_match_the_c_shims_hardcoded_switch` test is this
 * switch's drift guard.
 */
static int pycc_ext_raise_pending(void)
{
    int tag = pycc_rt_ext_pending_type();
    PyObject *exc_type;
    PyObject *message;
    size_t len = 0;
    const unsigned char *bytes;

    if (tag < 0) {
        return 0;
    }
    switch (tag) {
    case 1:
        exc_type = PyExc_ValueError;
        break;
    case 2:
        exc_type = PyExc_TypeError;
        break;
    case 3:
        exc_type = PyExc_KeyError;
        break;
    case 4:
        exc_type = PyExc_IndexError;
        break;
    case 5:
        exc_type = PyExc_ZeroDivisionError;
        break;
    case 6:
        exc_type = PyExc_RuntimeError;
        break;
    default:
        /*
         * Tag 0 is `Exception`; a user-defined exception class carries a
         * module-assigned tag this runtime knows nothing about, and the
         * closest true statement about it is that it is an `Exception`.
         */
        exc_type = PyExc_Exception;
        break;
    }
    bytes = pycc_rt_ext_pending_message(&len);
    /* Not NUL-terminated: a `PyStrObj` carries an explicit length. */
    message = (bytes == NULL) ? PyUnicode_FromString("")
                              : PyUnicode_FromStringAndSize((const char *)bytes, (Py_ssize_t)len);
    if (message == NULL) {
        /* A MemoryError is already set; keep it and drop the pycc-side one. */
        pycc_rt_exception_clear();
        return 1;
    }
    PyErr_SetObject(exc_type, message);
    Py_DECREF(message);
    pycc_rt_exception_clear();
    return 1;
}

/*
 * Unpacks one argument at an `int` parameter into a D-141 encoded word.
 * Returns 0, or -1 with a CPython exception set.
 *
 * The order is `PyBool_Check` -> `PyLong_Check` -> TypeError, and only then
 * the conversion, and every step of it is load-bearing:
 *
 *  - `bool` first, because D-244 rule 7 admits it and D-141 encodes
 *    `False`/`True` as dedicated marker words specifically so identity
 *    survives an int-compatible slot. `PyLong_AsLongLongAndOverflow` would
 *    flatten `True` to 1 and destroy that silently.
 *  - `PyLong_Check` before converting, because
 *    `PyLong_AsLongLongAndOverflow` accepts anything implementing
 *    `__index__`. A duck type is not an `int`, and rule 7's type boundary is
 *    closed.
 *  - The inline-range gate *after* CPython's own overflow check, never
 *    against `i64`: `pycc_rt`'s inline-integer range is [-2^62, 2^62-1], so
 *    an `i64`-shaped check would admit 4611686018427387905 -- a fully
 *    conforming call -- and the compiled body would then abort the process
 *    on it. See D-244's dated amendment and #1040.
 */
static int pycc_ext_unpack_int(PyObject *obj, const char *fn_name, Py_ssize_t index,
                               long long *out)
{
    long long raw;
    int overflow = 0;
    PyObject *type_name;

    if (PyBool_Check(obj)) {
        *out = pycc_rt_ext_bool_encode(obj == Py_True);
        return 0;
    }
    if (!PyLong_Check(obj)) {
        type_name = PyType_GetName(Py_TYPE(obj));
        if (type_name == NULL) {
            PyErr_SetString(PyExc_TypeError,
                            "object cannot be interpreted as an integer");
        } else {
            /* CPython's own wording for a non-int at an int converter. */
            PyErr_Format(PyExc_TypeError,
                         "%s() argument %zd: '%U' object cannot be interpreted as an integer",
                         fn_name, index + 1, type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    raw = PyLong_AsLongLongAndOverflow(obj, &overflow);
    if (raw == -1 && PyErr_Occurred()) {
        return -1;
    }
    if (overflow != 0 || pycc_rt_ext_int_encode(raw, out) != 0) {
        PyErr_Format(PyExc_OverflowError,
                     "%s() argument %zd: int is outside the inline-integer range "
                     "[-2**62, 2**62-1] this pycc version's `ext` boundary supports "
                     "(see #1040)",
                     fn_name, index + 1);
        return -1;
    }
    return 0;
}

/*
 * Packs an `int`-typed result. Mirrors `classify_encoded_int`'s four-way
 * split rather than inventing a two-way one: the bool markers are both even
 * and both carry low tag 0b10, so an "odd means smallint, otherwise raise"
 * egress would answer OverflowError for `f(True)`.
 */
static PyObject *pycc_ext_pack_int(const char *fn_name, long long encoded)
{
    switch (pycc_rt_ext_int_classify(encoded)) {
    case PYCC_EXT_INT_SMALLINT:
        return PyLong_FromLongLong(pycc_rt_ext_int_decode(encoded));
    case PYCC_EXT_INT_FALSE:
        Py_RETURN_FALSE;
    case PYCC_EXT_INT_TRUE:
        Py_RETURN_TRUE;
    case PYCC_EXT_INT_BIGINT:
        /* The caller owns this reference (D-180 rule 6) and it dies here:
         * dropping it without releasing would leak the `BigIntObj` on every
         * call the D-244 amendment turns into an `OverflowError`. */
        pycc_rt_bigint_release(encoded);
        PyErr_Format(PyExc_OverflowError,
                     "%s() produced an int outside the inline-integer range "
                     "[-2**62, 2**62-1] this pycc version's `ext` boundary supports "
                     "(see #1040)",
                     fn_name);
        return NULL;
    default:
        /* Deliberately no release: an unclassifiable word is not a pointer
         * this code knows to be live, so freeing through it is worse than
         * leaking it. */
        PyErr_Format(PyExc_SystemError, "%s() returned an unrecognized int word", fn_name);
        return NULL;
    }
}

/* Generated companion: module name macros, per-export wrappers, method table. */
#include "pycc_ext_exports.inc"

/*
 * PEP 489 multi-phase initialization. The module body cannot run in
 * `PyInit_`: at that point no module object exists, so there is nothing to
 * execute against and no way to report a failure. It runs here, in the
 * `Py_mod_exec` slot CPython calls after creating the module object, whose
 * failure convention is -1 with the exception set.
 *
 * Running it at all is not optional. `pycc` binds each `def` by *storing*
 * its address into a module-level function-pointer slot at the def's own
 * source position, so until the module body has executed every exported
 * wrapper would call a null pointer.
 */
static int pycc_ext_exec_module(PyObject *module)
{
    (void)module;
    if (pycc_ext_module_exec() != 0) {
        if (!pycc_ext_raise_pending()) {
            PyErr_SetString(PyExc_ImportError, "pycc module body failed");
        }
        return -1;
    }
    return 0;
}

static PyModuleDef_Slot pycc_ext_slots[] = {
    {Py_mod_exec, (void *)pycc_ext_exec_module},
    /*
     * `pycc_rt`'s refcounts are non-atomic `Cell<u32>` and its exception
     * state is thread-local, both of which are safe only under one GIL in
     * one interpreter. Refuse a subinterpreter explicitly rather than
     * corrupting state in one.
     */
    {Py_mod_multiple_interpreters, Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED},
    {0, NULL},
};

static struct PyModuleDef pycc_ext_moduledef = {
    PyModuleDef_HEAD_INIT,
    PYCC_EXT_MODULE_NAME_STR,
    NULL,
    0,
    pycc_ext_methods,
    pycc_ext_slots,
    NULL,
    NULL,
    NULL,
};

#define PYCC_EXT_CONCAT_(a, b) a##b
#define PYCC_EXT_CONCAT(a, b) PYCC_EXT_CONCAT_(a, b)

PyMODINIT_FUNC PYCC_EXT_CONCAT(PyInit_, PYCC_EXT_MODULE_NAME)(void)
{
    /*
     * The free-threaded guard, and it has to be here -- before
     * `PyModuleDef_Init` -- because it must refuse the module outright.
     *
     * A free-threaded host's `EXTENSION_SUFFIXES` does include `.abi3.so`,
     * so its finder locates this artifact happily; with no guard the failure
     * is a bare `SystemError: init function of <mod> returned uninitialized
     * object`. `sys._is_gil_enabled()` is not a usable detector here: it
     * returned True on a free-threaded 3.14 at `PyInit_` time. The version
     * string is, verified across 3.13 GIL, 3.14 GIL and 3.14t.
     */
    if (strstr(Py_GetVersion(), "free-threading build") != NULL) {
        PyErr_SetString(PyExc_ImportError,
                        PYCC_EXT_MODULE_NAME_STR " was built for a GIL-enabled CPython "
                        "(stable ABI); this interpreter is a free-threaded build, which is "
                        "not supported");
        return NULL;
    }
    return PyModuleDef_Init(&pycc_ext_moduledef);
}
