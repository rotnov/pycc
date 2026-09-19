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
/* D-180 rule 6: a compiled function's scalar return value arrives retained,
 * so a heap-bigint result this boundary refuses still has to be released.
 * A *tuple* element does not arrive retained -- the aggregate return path
 * takes no per-field retain -- so a tuple egress takes its own reference
 * with `pycc_rt_bigint_retain` before handing the word to the packer that
 * discharges one. Both are no-ops for a smallint, a bool marker, and the
 * word `0`, so the pairing stays balanced on every classification. */
extern void pycc_rt_bigint_retain(long long word);
extern void pycc_rt_bigint_release(long long word);
/* The `str` boundary (Part 2 of #1037, #1049). `pycc_rt`'s own `i64` length
 * is `long long` here and its `usize` is `size_t`; `PyStrObj` stays an opaque
 * `void *` on this side, exactly as it is in the generated wrapper. */
extern void *pycc_rt_str_from_literal(const unsigned char *ptr, long long len);
extern void pycc_rt_str_decref(void *s);
extern const unsigned char *pycc_rt_ext_str_bytes(void *s, size_t *len);
/* The instance boundary (#1145). `pycc_rt_instance_new` allocates a
 * `PyInstanceObj` with `slot_count` attribute slots and returns it as the
 * opaque `void *` a compiled `Ty::Instance` parameter is. It is the same
 * call `MirExpr::Instantiate` emits for a native `Grid(3, 4)`, so a host-
 * constructed instance and a natively constructed one have identical
 * layout.
 *
 * There is deliberately no matching free. Compiled code never releases an
 * instance either (D-107's arena model, narrowed by D-154), so a
 * `pycc_rt_instance_free` called from `tp_dealloc` would be the only
 * deallocation in the program and could release storage a compiled
 * function still aliases -- attribute reads hand out interior pointers.
 * The inner object therefore outlives its carrier; see
 * `pycc_ext_instance_dealloc`. */
extern void *pycc_rt_instance_new(long long slot_count);

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
 * The pending tag's synthesized user exception class, or NULL when the tag
 * names no class this artifact registered. Defined in the generated
 * companion, which is `#include`d far below this point -- hence the forward
 * declaration here. The returned pointer is *borrowed*: the generated
 * file-scope cache owns the strong reference, exactly as every `PyExc_*`
 * below is a borrowed immortal.
 */
static PyObject *pycc_ext_user_exception_class(unsigned char tag);

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
 * The tag values are fixed by `pycc_hir::exception`'s
 * `BUILTIN_EXCEPTION_CLASSES` array order, which this file cannot see from
 * C: tags 0..=6 are the flat seven, which `pycc_rt::exception` also names as
 * `EXCEPTION_TYPE_*` constants, and tags 7..=22 are the PEP 3151 `OSError`
 * family. Two tests are this switch's drift guard -- `ext_bridge`'s
 * `exception_type_tags_match_the_c_shims_hardcoded_switch` for the seven
 * constants, and `ext_build_tests`'
 * `every_exception_tag_the_c_shim_switches_on_still_names_that_class` for
 * the full array.
 */
static int pycc_ext_raise_pending(void)
{
    int tag = pycc_rt_ext_pending_type();
    PyObject *exc_type;
    PyObject *user_class;
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
    /*
     * Tags 7..=22, the PEP 3151 `OSError` family, in
     * `BUILTIN_EXCEPTION_CLASSES` order. Every one of them is reachable from
     * an `int`-only exported function through a bare `raise`, so flattening
     * them to `Exception` would make `except FileNotFoundError:` on the
     * Python side silently stop matching. Each constructs from a single
     * message argument, which is what `PyErr_SetObject` passes below.
     */
    case 7:
        exc_type = PyExc_OSError;
        break;
    case 8:
        exc_type = PyExc_BlockingIOError;
        break;
    case 9:
        exc_type = PyExc_ChildProcessError;
        break;
    case 10:
        exc_type = PyExc_ConnectionError;
        break;
    case 11:
        exc_type = PyExc_FileExistsError;
        break;
    case 12:
        exc_type = PyExc_FileNotFoundError;
        break;
    case 13:
        exc_type = PyExc_InterruptedError;
        break;
    case 14:
        exc_type = PyExc_IsADirectoryError;
        break;
    case 15:
        exc_type = PyExc_NotADirectoryError;
        break;
    case 16:
        exc_type = PyExc_PermissionError;
        break;
    case 17:
        exc_type = PyExc_ProcessLookupError;
        break;
    case 18:
        exc_type = PyExc_TimeoutError;
        break;
    case 19:
        exc_type = PyExc_BrokenPipeError;
        break;
    case 20:
        exc_type = PyExc_ConnectionAbortedError;
        break;
    case 21:
        exc_type = PyExc_ConnectionRefusedError;
        break;
    case 22:
        exc_type = PyExc_ConnectionResetError;
        break;
    /* Part A of #1038 (#1063): `OverflowError`, appended past the PEP 654
     * groups so every earlier tag keeps its value. Unlike those two it has a
     * `PyExc_*` object that a lone message constructs, so it is switched on. */
    case 25:
        exc_type = PyExc_OverflowError;
        break;
    default:
        /*
         * Tag 0 is `Exception`. So, deliberately, are the two remaining
         * builtin tags -- a hole in the otherwise contiguous switched range,
         * since tag 25 above sits past them -- and every user-defined class:
         *
         *  - tags 23..=24 are `BaseExceptionGroup`/`ExceptionGroup`. The C
         *    API exposes no `PyExc_ExceptionGroup` at all, and the type it
         *    does expose cannot be constructed from a lone message -- PEP
         *    654 requires `(msg, exceptions)`, so `PyErr_SetObject` would
         *    fail during normalization and surface a `TypeError` about the
         *    constructor instead of the program's own error. `Exception`
         *    with the right message is the more truthful of the two.
         *  - a user-defined exception class carries a module-assigned tag
         *    this shim knows nothing about, so the lookup below -- not any
         *    tag arithmetic here -- decides whether the artifact registered
         *    a class for it. A hit replaces `Exception` with the real
         *    class, restoring `except m.MyError:`, `except ValueError:` for
         *    a builtin subclass, and `type(e).__name__`. A miss keeps
         *    `Exception`, which is what leaves the two group tags above (and
         *    any class derived from them, which the table excludes for the
         *    same reason) on the honest fallback.
         */
        exc_type = PyExc_Exception;
        user_class = pycc_ext_user_exception_class((unsigned char)tag);
        if (user_class != NULL) {
            exc_type = user_class;
        }
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
 * The longest `, element %zd` clause `pycc_ext_element_clause` can produce.
 * `Py_ssize_t` is 64-bit on every target this shim builds for, so the widest
 * rendering is `, element -9223372036854775808` at 30 bytes including the
 * terminator; 48 leaves room and is still a stack buffer.
 */
#define PYCC_EXT_ELEMENT_CLAUSE_MAX 48

/*
 * Renders the `, element N` half of an argument-position clause into `buf`,
 * or the empty string when `element` is negative -- the "this value is the
 * argument itself, not one of its tuple elements" sentinel every scalar
 * entry point below passes.
 *
 * Returned rather than written in place so an error arm can splice it into
 * `PyErr_Format` with a single `%s` and pay for it only when it raises. The
 * function name is *not* folded in here: it is an arbitrary-length Python
 * identifier and a fixed buffer would truncate it, so it stays its own `%s`.
 *
 * Both indices are rendered one-based, matching the argument index the
 * scalar messages already use (CPython's own convention, `f() argument 1`).
 * `element 2` is therefore `t[1]`; the alternative -- a one-based argument
 * beside a zero-based element in the same sentence -- reads as a typo.
 */
static const char *pycc_ext_element_clause(char *buf, size_t cap, Py_ssize_t element)
{
    if (element < 0) {
        return "";
    }
    PyOS_snprintf(buf, cap, ", element %zd", element + 1);
    return buf;
}

/*
 * Checks that one argument is a `tuple` of exactly `arity` elements.
 * Returns 0, or -1 with a CPython exception set. The elements themselves
 * are unpacked by the `_at` entry points below, one per declared element
 * type.
 *
 * `PyTuple_Check` and not `PyTuple_CheckExact`: a `tuple` subclass *is* a
 * tuple, and D-244 rule 7's boundary is closed against duck typing, not
 * against subtyping -- the same reading that lets `pycc_ext_unpack_str`
 * accept a `str` subclass. The elements are copied out by value, so the
 * subclass identity does not survive the crossing; `docs/RUNTIME.md` records
 * that narrowing alongside `str`'s.
 *
 * The arity check is what makes `PyTuple_GetItem` infallible at every call
 * site the generated wrapper emits afterwards, so it is never skipped: D-116
 * fixes a tuple type's arity, and a shorter tuple would otherwise reach a
 * `GetItem` that returns NULL with an `IndexError` the caller does not test
 * for.
 */
static int pycc_ext_unpack_tuple(PyObject *obj, const char *fn_name, Py_ssize_t index,
                                 Py_ssize_t arity)
{
    PyObject *type_name;
    Py_ssize_t size;

    if (!PyTuple_Check(obj)) {
        type_name = PyType_GetName(Py_TYPE(obj));
        if (type_name == NULL) {
            PyErr_SetString(PyExc_TypeError, "object cannot be interpreted as a tuple");
        } else {
            PyErr_Format(PyExc_TypeError,
                         "%s() argument %zd: '%U' object cannot be interpreted as a tuple",
                         fn_name, index + 1, type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    size = PyTuple_Size(obj);
    if (size != arity) {
        PyErr_Format(PyExc_TypeError,
                     "%s() argument %zd: expected a tuple of length %zd, got %zd",
                     fn_name, index + 1, arity, size);
        return -1;
    }
    return 0;
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
static int pycc_ext_unpack_int_at(PyObject *obj, const char *fn_name, Py_ssize_t index,
                                  Py_ssize_t element, long long *out)
{
    long long raw;
    int overflow = 0;
    PyObject *type_name;
    char where[PYCC_EXT_ELEMENT_CLAUSE_MAX];

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
                         "%s() argument %zd%s: '%U' object cannot be interpreted as an integer",
                         fn_name, index + 1,
                         pycc_ext_element_clause(where, sizeof where, element), type_name);
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
                     "%s() argument %zd%s: int is outside the inline-integer range "
                     "[-2**62, 2**62-1] this pycc version's `ext` boundary supports "
                     "(see #1040)",
                     fn_name, index + 1,
                     pycc_ext_element_clause(where, sizeof where, element));
        return -1;
    }
    return 0;
}

/*
 * The plain-argument entry point for a `int` parameter: the same check,
 * with no tuple-element clause in its message. Kept as its own symbol rather
 * than folded into the caller because a scalar parameter is by far the
 * common case and its generated call site should say what it means -- and
 * because the `.inc` fixtures that pin the scalar boundary predate #1050 and
 * stay byte-for-byte unchanged by it.
 */
static int pycc_ext_unpack_int(PyObject *obj, const char *fn_name, Py_ssize_t index,
                               long long *out)
{
    return pycc_ext_unpack_int_at(obj, fn_name, index, -1, out);
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

/*
 * Unpacks one argument at a `float` parameter. Returns 0, or -1 with a
 * CPython exception set.
 *
 * `PyFloat_Check` before `PyFloat_AsDouble`, and no fallback: the converter
 * alone accepts an `int`, a `bool` and every `__float__` duck type, and
 * `docs/TYPE_SYSTEM.md` rule 4 (D-086) forbids implicit numeric widening as
 * well as narrowing at an annotated boundary -- which D-244 rule 7 defers to
 * for conformance. So `f(1)` at a `float` parameter is a `TypeError` here,
 * deliberately unlike every C-API converter's habit.
 */
static int pycc_ext_unpack_float_at(PyObject *obj, const char *fn_name, Py_ssize_t index,
                                    Py_ssize_t element, double *out)
{
    PyObject *type_name;
    char where[PYCC_EXT_ELEMENT_CLAUSE_MAX];

    if (!PyFloat_Check(obj)) {
        type_name = PyType_GetName(Py_TYPE(obj));
        if (type_name == NULL) {
            PyErr_SetString(PyExc_TypeError, "object cannot be interpreted as a float");
        } else {
            PyErr_Format(PyExc_TypeError,
                         "%s() argument %zd%s: '%U' object cannot be interpreted as a float",
                         fn_name, index + 1,
                         pycc_ext_element_clause(where, sizeof where, element), type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    *out = PyFloat_AsDouble(obj);
    return 0;
}

/*
 * The plain-argument entry point for a `float` parameter: the same check,
 * with no tuple-element clause in its message. Kept as its own symbol rather
 * than folded into the caller because a scalar parameter is by far the
 * common case and its generated call site should say what it means -- and
 * because the `.inc` fixtures that pin the scalar boundary predate #1050 and
 * stay byte-for-byte unchanged by it.
 */
static int pycc_ext_unpack_float(PyObject *obj, const char *fn_name, Py_ssize_t index,
                               double *out)
{
    return pycc_ext_unpack_float_at(obj, fn_name, index, -1, out);
}

/*
 * Unpacks one argument at a `bool` parameter. Returns 0, or -1 with a
 * CPython exception set.
 *
 * `PyBool_Check`, never `PyObject_IsTrue`: rule 7's boundary is closed, so
 * a truthy object is not a `bool`. An `int` is refused for the same reason
 * `float` refuses one -- rule 4 / D-086 -- and the `int` parameter's own
 * acceptance of `bool` does not run backwards, subtyping being
 * one-directional.
 *
 * The out parameter is a one-byte `char` because the compiled function's
 * own ABI slot is an `i8` holding 0/1 (`ty_to_basic_type`), and the
 * generated wrapper reaches it through an unchecked `void *` cast.
 */
static int pycc_ext_unpack_bool_at(PyObject *obj, const char *fn_name, Py_ssize_t index,
                                   Py_ssize_t element, char *out)
{
    PyObject *type_name;
    char where[PYCC_EXT_ELEMENT_CLAUSE_MAX];

    if (!PyBool_Check(obj)) {
        type_name = PyType_GetName(Py_TYPE(obj));
        if (type_name == NULL) {
            PyErr_SetString(PyExc_TypeError, "object cannot be interpreted as a bool");
        } else {
            PyErr_Format(PyExc_TypeError,
                         "%s() argument %zd%s: '%U' object cannot be interpreted as a bool",
                         fn_name, index + 1,
                         pycc_ext_element_clause(where, sizeof where, element), type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    *out = (char)(obj == Py_True);
    return 0;
}

/*
 * The plain-argument entry point for a `bool` parameter: the same check,
 * with no tuple-element clause in its message. Kept as its own symbol rather
 * than folded into the caller because a scalar parameter is by far the
 * common case and its generated call site should say what it means -- and
 * because the `.inc` fixtures that pin the scalar boundary predate #1050 and
 * stay byte-for-byte unchanged by it.
 */
static int pycc_ext_unpack_bool(PyObject *obj, const char *fn_name, Py_ssize_t index,
                               char *out)
{
    return pycc_ext_unpack_bool_at(obj, fn_name, index, -1, out);
}

/*
 * Packs a `float`-typed result. No function name and no failure path:
 * `PyFloat_FromDouble` carries every `double`, including the infinities and
 * NaN, so unlike `pycc_ext_pack_int` there is no value it has to refuse.
 */
static PyObject *pycc_ext_pack_float(double value)
{
    return PyFloat_FromDouble(value);
}

/*
 * Packs a `bool`-typed result. `PyBool_FromLong` returns the interned
 * singleton, so `m.f() is True` holds at the boundary rather than merely
 * comparing equal.
 */
static PyObject *pycc_ext_pack_bool(char value)
{
    return PyBool_FromLong(value != 0);
}

/*
 * Unpacks one argument at a `str` parameter. Returns 0, or -1 with a
 * CPython exception set.
 *
 * `PyUnicode_Check` before the converter, and no fallback, for the same
 * reason `pycc_ext_unpack_float` refuses an `int`: D-244 rule 7 defers to
 * `docs/TYPE_SYSTEM.md` rule 4 (D-086), so nothing that merely knows how to
 * become a `str` -- `__str__`, `os.PathLike`, a buffer -- is admitted here.
 * `PyUnicode_Check` does accept a `str` subclass, which is flattened to a
 * plain pycc `str` by the copy below; `docs/RUNTIME.md` records that
 * narrowing.
 *
 * `PyUnicode_AsUTF8AndSize` is in the limited API from `Py_LIMITED_API`
 * 0x030D0000, this shim's floor. It fails on a string holding a lone
 * surrogate, which has no UTF-8 encoding; the resulting `UnicodeEncodeError`
 * is propagated verbatim rather than translated, per the D-244 amendment.
 * The length is carried explicitly, never re-derived with `strlen`, because
 * a Python `str` may contain embedded NUL bytes.
 *
 * The bytes belong to `obj` and are copied by `pycc_rt_str_from_literal`, so
 * the caller need not keep them alive. That call hands back a fresh
 * reference with refcount 1, which becomes the compiled function's own
 * parameter slot.
 */
static int pycc_ext_unpack_str(PyObject *obj, const char *fn_name, Py_ssize_t index,
                               void **out)
{
    PyObject *type_name;
    const char *utf8;
    Py_ssize_t size;

    if (!PyUnicode_Check(obj)) {
        type_name = PyType_GetName(Py_TYPE(obj));
        if (type_name == NULL) {
            PyErr_SetString(PyExc_TypeError, "object cannot be interpreted as a str");
        } else {
            PyErr_Format(PyExc_TypeError,
                         "%s() argument %zd: '%U' object cannot be interpreted as a str",
                         fn_name, index + 1, type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    utf8 = PyUnicode_AsUTF8AndSize(obj, &size);
    if (utf8 == NULL) {
        return -1;
    }
    *out = pycc_rt_str_from_literal((const unsigned char *)utf8, (long long)size);
    return 0;
}

/*
 * Packs a `str`-typed result. No function name, like `pycc_ext_pack_float`:
 * there is no value this has to refuse, and a `PyStrObj` provably cannot
 * hold invalid UTF-8 (every constructor takes already-valid Rust input).
 *
 * The reference is released unconditionally before returning, including on
 * the `PyUnicode_FromStringAndSize` failure path: the compiled function's
 * return hands this boundary a retained `str` (D-180 rule 6, the same
 * ownership `pycc_ext_pack_int`'s bigint arm releases), and the CPython
 * object is an independent copy, so identity does not survive the crossing
 * (#1043).
 *
 * `result` is never NULL on this path -- the generated wrapper checks for a
 * pending pycc exception first, and that branch returns the `NULL` carrier
 * without reaching here -- but the guard costs nothing in C and keeps the
 * accessor's own no-null-arm contract honest.
 */
static PyObject *pycc_ext_pack_str(void *result)
{
    const unsigned char *bytes;
    size_t len = 0;
    PyObject *packed;

    if (result == NULL) {
        PyErr_SetString(PyExc_SystemError, "str result was NULL");
        return NULL;
    }
    bytes = pycc_rt_ext_str_bytes(result, &len);
    packed = PyUnicode_FromStringAndSize((const char *)bytes, (Py_ssize_t)len);
    pycc_rt_str_decref(result);
    return packed;
}

/*
 * The `{ ptr, len }` pair a `memoryview` parameter crosses into compiled
 * code as (Part 1 of #1027, D-244). Declared here rather than derived from
 * `Py_buffer` on purpose, and it is the *whole* of what the compiled body
 * ever sees of an acquired buffer.
 *
 * `len` is a copy of `buf.shape[0]`, the element count -- not `buf.len`,
 * which is a byte count (a 27-element `float64` view reports `len == 216`
 * and `itemsize == 8`). `shape[0]` is what CPython's own `len()` and
 * `IndexError` bound use, so it is the authoritative bound for the indexing
 * Part 2 adds.
 *
 * It is a copy because `Py_buffer.shape` is a `Py_ssize_t *` owned by the
 * *exporter* and valid only until `PyBuffer_Release`. Carrying the pointer
 * would make every generated wrapper's own release a use-after-free for
 * anything that read through it afterwards. `pycc_ext_obj_*`-style opacity
 * applies here too: nothing on the compiled side may reach the `Py_buffer`
 * or the owning `PyObject *` at all, which is what keeps the buffer's
 * lifetime wholly wrapper-owned.
 *
 * `long long` and not `Py_ssize_t` so the field matches the `i64` that
 * `pycc_codegen`'s `ty_to_basic_type` gives every pycc `int`, exactly as
 * `boundary_carrier`'s `Ty::Int` slot does.
 */
typedef struct {
    void *ptr;
    long long len;
} PyccExtBufferView;

/*
 * Unpacks one argument at a buffer parameter -- spelled `memoryview`,
 * `ndarray` or `NDArray` in the source, one pycc type whichever was
 * written. Returns 0 with `*out`
 * holding an acquired buffer the caller must release, or -1 with a CPython
 * exception set and nothing acquired.
 *
 * Four refusals, in this order, and the order is the contract:
 *
 * 1. The object exports no buffer at all -- a pycc-authored `TypeError`,
 *    nothing acquired. `PyObject_CheckBuffer` and not `PyMemoryView_Check`:
 *    #1129 cashed in the widening D-244's Part-1-of-#1027 amendment
 *    statement (a) pre-authorized, so `bytes`, a `bytearray`, an
 *    `array.array` and a bare NumPy array now all pass *this* arm on the
 *    strength of exporting a buffer, and are answered -- or accepted -- by
 *    arms 2-4 on the properties of the buffer they export. This is what
 *    lets a host hand a compiled export a bare `ndarray` without wrapping
 *    it in `memoryview(...)` first, and it widens the `memoryview`
 *    annotation by exactly the same set: every source spelling of the
 *    carrier -- `memoryview`, `ndarray` (#1129) and `NDArray` (#1134) --
 *    lowers to one pycc type and reaches this helper identically.
 * 2. `PyObject_GetBuffer` fails -- the exporter's own exception is
 *    propagated verbatim, because it says more about the operand than a
 *    translated message could, and nothing is acquired when it fails. The
 *    *type* is the exporter's choice and not fixed: a strided `memoryview`
 *    raises CPython's `BufferError: memoryview: underlying buffer is not
 *    C-contiguous`, while a strided NumPy array raises numpy's own
 *    `ValueError: ndarray is not C-contiguous` for the same condition
 *    (D-244's #1129 amendment statement (g)).
 * 3. `ndim != 1` -- released first, then a pycc-authored `TypeError`.
 *    Part 1 admits one dimension only.
 * 4. The element format is not `"d"` -- released first, then a
 *    pycc-authored `TypeError` naming the format seen and the one required.
 *    This is the arm a buffer of the wrong element type takes, e.g. a
 *    `bytes` or `memoryview(b"abc")`, whose format is `'B'`, or a float32
 *    NumPy array, whose format is `'f'`. `itemsize` is checked
 *    alongside the format string rather than instead of it: the format is
 *    the exporter's own claim, and the size is the arithmetic the compiled
 *    code would do.
 *
 * Arms 3 and 4 say "a buffer", not "a memoryview". Arm 1's widening is
 * what makes that accurate rather than cosmetic: the operand that reaches
 * them is any conforming exporter, and a `bytes` or a bare NumPy array is
 * not a `memoryview` by any reading (D-244's #1129 amendment statement
 * (f)). Arm 2 needs no such wording -- it propagates the exporter's own
 * exception and authors no text at all.
 *
 * `PyBUF_C_CONTIGUOUS | PyBUF_FORMAT` is the request. C-contiguity is what
 * makes a plain pointer walk correct at all, and it implies `PyBUF_STRIDES`
 * and so `PyBUF_ND`, which is what makes `ndim` and `shape` populated for
 * arms 3 and 4 to read. `PyBUF_WRITABLE` is deliberately *not* requested:
 * Part 1 never writes through the pointer, and requesting it would refuse
 * the read-only views this boundary is meant to accept.
 *
 * Every symbol used here is in the limited API at this shim's
 * `Py_LIMITED_API 0x030D0000` floor.
 */
static int pycc_ext_unpack_memoryview(PyObject *obj, const char *fn_name, Py_ssize_t index,
                                      Py_buffer *out)
{
    PyObject *type_name;
    const char *declared;
    int ndim;

    if (!PyObject_CheckBuffer(obj)) {
        type_name = PyType_GetName(Py_TYPE(obj));
        if (type_name == NULL) {
            PyErr_SetString(PyExc_TypeError, "object does not export a buffer");
        } else {
            PyErr_Format(PyExc_TypeError,
                         "%s() argument %zd: '%U' object does not export a buffer",
                         fn_name, index + 1, type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    if (PyObject_GetBuffer(obj, out, PyBUF_C_CONTIGUOUS | PyBUF_FORMAT) != 0) {
        return -1;
    }
    ndim = out->ndim;
    if (ndim != 1) {
        PyBuffer_Release(out);
        PyErr_Format(PyExc_TypeError,
                     "%s() argument %zd: a buffer with ndim %d is not supported yet -- "
                     "only a one-dimensional buffer is",
                     fn_name, index + 1, ndim);
        return -1;
    }
    /*
     * `out->format` points into storage the exporter owns, and the message
     * below names what this boundary was actually handed (D-244 statement
     * (e)). The exporter chooses that string and a `ctypes.Structure`'s is
     * routinely dozens of characters (`T{<d:x:<d:y:}`), so it is *not*
     * copied into a fixed buffer first -- that only ever named a prefix.
     * The message is built while the buffer is still held instead, and the
     * release moved after it: `PyErr_Format` copies the characters it reads
     * as it formats, and it runs no interpreted code that could invalidate
     * the view in between, so the release below cannot leave the exception
     * naming freed storage. The same reasoning already governs the `%U`
     * of the arm above that #1129 widened to `PyObject_CheckBuffer`.
     */
    declared = (out->format == NULL) ? "" : out->format;
    if (strcmp(declared, "d") != 0 || out->itemsize != (Py_ssize_t)sizeof(double)) {
        PyErr_Format(PyExc_TypeError,
                     "%s() argument %zd: a buffer of format '%s' is not supported -- "
                     "only format 'd' (a contiguous float64 buffer) is",
                     fn_name, index + 1, declared);
        PyBuffer_Release(out);
        return -1;
    }
    return 0;
}

/*
 * Part 1 of #1026: the module-import helper compiled code calls for a
 * foreign `import numpy`.
 *
 * A thin `PyImport_ImportModule` wrapper and nothing more -- it returns a
 * new reference to the imported module, or NULL with the CPython exception
 * (a `ModuleNotFoundError` for a module that is not on the interpreter's
 * `sys.path`) already set, which is exactly the convention the generated
 * call site checks for. Every *decision* about the import stays with
 * CPython, keeping this file's "only moves `PyObject*`s" division of labour
 * with `pycc_rt` intact.
 *
 * Not `static`, unlike every other helper here: LLVM-generated code
 * declares and calls it by this name (`EXT_OBJ_IMPORT_SYMBOL` in
 * `crates/pycc_codegen/src/ext.rs`).
 *
 * The returned reference is deliberately never released. The generated
 * module body stores it in a module-level global that lives for the
 * artifact's lifetime, and the artifact has no teardown hook to release it
 * from; the module object is in the interpreter's `sys.modules` for that
 * whole lifetime anyway. `docs/RUNTIME.md` records the rule.
 */
PyObject *pycc_ext_obj_import(const char *name)
{
    return PyImport_ImportModule(name);
}

/*
 * Part 2 of #1026: the attribute-load helper compiled code calls for
 * `numpy.pi` on a value whose static type is the opaque `object`.
 *
 * As thin as its neighbour above, and for the same reason: every decision
 * about the lookup -- the descriptor protocol, `__getattr__`, the
 * `AttributeError` a missing name raises -- stays with CPython. It returns a
 * new reference to the attribute's value, or NULL with the CPython
 * exception already set.
 *
 * Not `static`: LLVM-generated code declares and calls it by this name
 * (`EXT_OBJ_GETATTR_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * `obj` is borrowed -- the caller holds the reference for at least the
 * duration of this call, because in Part 2 the only object a load can be
 * rooted at is a module global that is never released. The returned
 * reference is deliberately never released either, on the same leak-only
 * rule `pycc_ext_obj_import` documents; `docs/RUNTIME.md` records it and
 * names the deferral.
 *
 * Deliberately *not* translated into `pycc_rt`'s pending-exception state
 * here. The two failure protocols are kept apart; the caller's own contract
 * (`crates/pycc_codegen/src/foreign_attr.rs`) states which side owns the
 * transition, and why Part 2a does not need it: it tests this function's
 * result for NULL and returns `-1` from the `Py_mod_exec` slot with
 * CPython's exception left exactly as set here.
 *
 * A NULL `obj` is still decided here. `a.b.c` lowers to nested
 * `ObjAttrGet` nodes, and although the caller's own NULL check now stops an
 * inner failure before the outer call is reached, this guard is what makes
 * that a defence in depth rather than the only line: `PyObject_GetAttrString`
 * dereferences `Py_TYPE(obj)` with no guard of its own, so any caller that
 * reaches here with NULL would crash the hosting interpreter instead of
 * raising. Returning NULL unchanged is also the *correct* CPython state --
 * the inner lookup already set its `AttributeError`, so propagating NULL
 * leaves exactly one exception set. Overwriting it with a second, synthetic
 * error would be worse.
 */
PyObject *pycc_ext_obj_getattr(PyObject *obj, const char *name)
{
    if (obj == NULL) {
        return NULL;
    }
    return PyObject_GetAttrString(obj, name);
}

/*
 * Part 2 of #1026, PR 2b of #1081: argument marshalling for
 * `pycc_ext_obj_call` below.
 *
 * Four packers, one per scalar `pycc_types` admits as an argument to a
 * foreign method call. Each takes a pycc-side value and returns a *new*
 * reference to its CPython equivalent, or NULL with a CPython exception
 * already set. They are non-`static` for the same reason their `obj_`
 * neighbours are: LLVM-generated code declares and calls them by these
 * exact names (`EXT_OBJ_PACK_*_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * **Every packer borrows its argument.** This is the one place where they
 * deliberately differ from the result packers above (`pycc_ext_pack_int`'s
 * bigint arm releases, `pycc_ext_pack_str` releases unconditionally), which
 * receive an already-transferred return value. An argument is an ordinary
 * module-body temporary that pycc's own machinery still owns, so consuming
 * it here would mean the boundary and `pycc_rt` both believe they hold the
 * last reference. Borrowing keeps ownership entirely on the pycc side, and
 * the price -- a bigint word that reaches the overflow arm is not released
 * -- is the same leak-only rule `docs/RUNTIME.md` already records for this
 * boundary, on a path that aborts the module load anyway.
 */
PyObject *pycc_ext_obj_pack_int(long long encoded)
{
    switch (pycc_rt_ext_int_classify(encoded)) {
    case PYCC_EXT_INT_SMALLINT:
        return PyLong_FromLongLong(pycc_rt_ext_int_decode(encoded));
    case PYCC_EXT_INT_FALSE:
        Py_RETURN_FALSE;
    case PYCC_EXT_INT_TRUE:
        Py_RETURN_TRUE;
    case PYCC_EXT_INT_BIGINT:
        PyErr_SetString(PyExc_OverflowError,
                        "an int argument to a CPython object's method is outside the "
                        "inline-integer range [-2**62, 2**62-1] this pycc version's `ext` "
                        "boundary supports (see #1040)");
        return NULL;
    default:
        PyErr_SetString(PyExc_SystemError,
                        "an int argument to a CPython object's method was an "
                        "unrecognized int word");
        return NULL;
    }
}

PyObject *pycc_ext_obj_pack_float(double value)
{
    return PyFloat_FromDouble(value);
}

/*
 * `PyBool_FromLong` hands back one of the two interned singletons, so
 * `x is True` holds on the host side exactly as `pycc_ext_pack_bool`
 * already guarantees for a returned `bool`.
 */
PyObject *pycc_ext_obj_pack_bool(char value)
{
    return PyBool_FromLong(value != 0);
}

/*
 * Borrowing, unlike `pycc_ext_pack_str` above: see the block comment on
 * `pycc_ext_obj_pack_int`. The `PyStrObj` stays owned by the compiled
 * module body, and `PyUnicode_FromStringAndSize` copies the bytes, so the
 * CPython string is an independent object with no shared identity (#1043).
 */
PyObject *pycc_ext_obj_pack_str(void *value)
{
    const unsigned char *bytes;
    size_t len = 0;

    if (value == NULL) {
        PyErr_SetString(PyExc_SystemError,
                        "a str argument to a CPython object's method was NULL");
        return NULL;
    }
    bytes = pycc_rt_ext_str_bytes(value, &len);
    return PyUnicode_FromStringAndSize((const char *)bytes, (Py_ssize_t)len);
}

/*
 * Part 2 of #1026, PR 2b of #1081: the method-call helper compiled code
 * calls for `gc.disable()` on a value whose static type is the opaque
 * `object`.
 *
 * Deliberately *not* fused with the attribute load. An earlier revision
 * took `(obj, method)` and performed the `PyObject_GetAttrString` here, so
 * that the bound method never became a pycc value and the whole operation
 * presented one failure edge instead of two. That ordering is observable
 * and wrong: CPython resolves a call's callable *before* it evaluates the
 * arguments, so `obj.missing(1 // 0)` must raise `AttributeError`, while
 * the fused shim raised `ZeroDivisionError` -- the generated code had
 * already evaluated every argument by the time the lookup ran. Codegen now
 * emits `pycc_ext_obj_getattr` before the argument expressions and passes
 * the resulting callable here, which costs a second NULL test and a second
 * failure edge and buys back CPython's own evaluation order. The bound
 * method still never becomes a pycc value: it lives in an LLVM temporary
 * that dominates this call, and this function releases it.
 *
 * Not `static`: LLVM-generated code declares and calls it by this name
 * (`EXT_OBJ_CALL_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * `bound` is an *owned* reference -- the one `pycc_ext_obj_getattr`
 * returned -- and this function releases it on every path. `args` points at
 * `nargs` slots of owned references produced by the `pycc_ext_obj_pack_*`
 * helpers above; this function consumes every one of them on every path, so
 * the generated code never has to. The returned reference is deliberately
 * never released, on the leak-only rule `docs/RUNTIME.md` records for this
 * boundary -- the *result* is the only thing that leaks, because it is the
 * only thing that escapes into compiled code as an `object` value.
 *
 * A packer that failed stored NULL in its slot with a CPython exception
 * already set. Scanning for that here rather than testing each packer's
 * result in LLVM keeps the emitted code one straight line per argument, and
 * propagating the *first* exception unchanged is the correct CPython state:
 * a second, synthetic error would overwrite the real one.
 *
 * A NULL `bound` is defence in depth: the caller's own NULL check on the
 * lookup already routed a failed attribute load to the module-exec failure
 * edge before this call is reached.
 */
PyObject *pycc_ext_obj_call(PyObject *bound, PyObject **args, long long nargs)
{
    PyObject *result;
    long long i;
    int packed = 1;

    for (i = 0; i < nargs; i++) {
        if (args[i] == NULL) {
            packed = 0;
        }
    }
    result = (bound == NULL || !packed)
                 ? NULL
                 : PyObject_Vectorcall(bound, args, (size_t)nargs, NULL);
    Py_XDECREF(bound);
    for (i = 0; i < nargs; i++) {
        Py_XDECREF(args[i]);
    }
    return result;
}

/*
 * Part 3 of #1026 (PR 3a of #1082): `len(o)` on a CPython object value
 * (`EXT_OBJ_LEN_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * `o` is borrowed and its refcount is untouched on every path --
 * `PyObject_Size` neither steals nor retains, and the result is a plain
 * `Py_ssize_t`, so no new reference exists to leak under the #1092
 * leak-only regime.
 *
 * The D-141 encode is fused in here rather than emitted by codegen so the
 * whole operation presents *one* failure edge to the caller: `PyObject_Size`
 * raises `TypeError` for an operand with no length, and the encode refuses a
 * value outside the inline range `[-2**62, 2**62-1]`. Two `-1` returns from
 * one symbol let `foreign_len.rs` emit a single branch to the module-exec
 * failure edge instead of two. The encode arm is unreachable for a real
 * container -- no object has 2**62 elements -- and exists as defence in
 * depth; `PyErr_NoMemory` is the honest report for a length that large.
 *
 * As with `pycc_ext_obj_getattr`, a NULL `o` is decided here as defence in
 * depth: `PyObject_Size` dereferences `Py_TYPE(o)` with no guard of its own,
 * and the caller's own NULL check already routed a failed producer to the
 * module-exec failure edge with its exception set, so returning `-1` without
 * setting a second one leaves exactly one exception pending.
 */
int pycc_ext_obj_len(PyObject *o, long long *out)
{
    Py_ssize_t size;

    if (o == NULL) {
        return -1;
    }
    size = PyObject_Size(o);
    if (size == -1 && PyErr_Occurred()) {
        return -1;
    }
    if (pycc_rt_ext_int_encode((long long)size, out) != 0) {
        PyErr_NoMemory();
        return -1;
    }
    return 0;
}

/*
 * Part 3 of #1026 (PR 3a of #1082): truth testing a CPython object value
 * (`EXT_OBJ_TRUTHY_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * Returns `1` for a truthy operand, `0` for a falsy one, and `-1` with a
 * CPython exception set on failure -- `PyObject_IsTrue` calls the operand's
 * `__bool__` or `__len__`, which is arbitrary Python code and really can
 * raise. `o` is borrowed and its refcount is untouched on every path.
 *
 * The NULL guard is the same defence in depth `pycc_ext_obj_len` documents.
 */
int pycc_ext_obj_truthy(PyObject *o)
{
    if (o == NULL) {
        return -1;
    }
    return PyObject_IsTrue(o);
}

/*
 * Part 3 of #1026 (PR 3b of #1082): `o[k]` on a CPython object value
 * (`EXT_OBJ_GETITEM_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * `o` is borrowed, exactly as `pycc_ext_obj_getattr` borrows its own base.
 * `k` is an *owned* reference produced by one of the `pycc_ext_obj_pack_*`
 * helpers above, and this function releases it on every path -- the same
 * arg-slot contract `pycc_ext_obj_call` imposes on the values those helpers
 * produce. Keeping one rule for every packed value is the point: generated
 * code creates a packed reference and hands it to a shim helper, and the
 * shim helper is what releases it, so codegen never has to.
 *
 * The result is a *new* reference that is deliberately never released, on
 * the leak-only rule `docs/RUNTIME.md` records for this boundary --
 * `PyObject_GetItem` hands back a new reference exactly as
 * `PyObject_GetAttrString` does, and the result is the only thing that
 * escapes into compiled code as an `object` value.
 *
 * A NULL `k` means the packer already failed with a CPython exception set
 * (an out-of-range `int`, #1040). Testing for that here rather than in LLVM
 * keeps the emitted code one straight line for the key and leaves the whole
 * operation with one failure edge instead of two, which is the identical
 * argument `pycc_ext_obj_call`'s own NULL scan records. A NULL `o` is
 * defence in depth: `PyObject_GetItem` dereferences `Py_TYPE(o)` with no
 * guard of its own, and the caller's own NULL check already routed a failed
 * producer to the module-exec failure edge with its exception set, so
 * returning NULL without setting a second one leaves exactly one exception
 * pending.
 */
PyObject *pycc_ext_obj_getitem(PyObject *o, PyObject *k)
{
    PyObject *result;

    result = (o == NULL || k == NULL) ? NULL : PyObject_GetItem(o, k);
    Py_XDECREF(k);
    return result;
}

/*
 * Part 3 of #1026 (PR 3c of #1082): `iter(o)` for a `for x in <object>:`
 * loop (`EXT_OBJ_GET_ITER_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * `o` is borrowed. The result is a *new* reference that is deliberately
 * never released -- one leaked iterator per `for` statement, on the same
 * leak-only rule `docs/RUNTIME.md` records for the rest of this boundary.
 *
 * A non-iterable operand makes `PyObject_GetIter` set `TypeError` and
 * return NULL, which the caller routes to the module-exec failure edge, so
 * "this object cannot be iterated" needs no compile-time test: pycc knows
 * nothing about the pointee and could not perform one.
 *
 * The NULL guard is the same defence in depth `pycc_ext_obj_len` documents.
 */
PyObject *pycc_ext_obj_get_iter(PyObject *o)
{
    if (o == NULL) {
        return NULL;
    }
    return PyObject_GetIter(o);
}

/*
 * Part 3 of #1026 (PR 3c of #1082): one step of a `for x in <object>:` loop
 * (`EXT_OBJ_ITER_NEXT_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * Three-valued, unlike every other helper here:
 *
 *   1  -- a *new* reference to the next item has been written to `*out`;
 *   0  -- the iterator is cleanly exhausted, `*out` untouched;
 *  -1  -- iteration failed with a CPython exception already set.
 *
 * `PyIter_Next` collapses the last two into NULL and only `PyErr_Occurred()`
 * tells them apart. That discrimination lives here rather than in emitted
 * LLVM IR on purpose: it is a CPython calling convention, and open-coding it
 * in the code generator would put a second, independently maintained copy of
 * that convention in a place where it could silently drift. It is also what
 * keeps *exhaustion off the failure edge* -- a loop that simply ends is not
 * a module-exec failure, and fusing the two would have made every `for` loop
 * over a foreign object terminate the module body.
 *
 * Each item written through `*out` is a new reference that is never
 * released, which is what makes the leak trip-count-linear for a `for` loop
 * rather than a fixed cost per statement (#1092).
 *
 * `it` and `out` are NULL-guarded as defence in depth, exactly like
 * `pycc_ext_obj_len`'s own operand: returning -1 without setting an
 * exception would be wrong, so this path sets one itself -- unlike the
 * NULL-returning helpers above, whose caller has already seen a real
 * CPython exception from the producer that returned NULL.
 */
long long pycc_ext_obj_iter_next(PyObject *it, PyObject **out)
{
    PyObject *item;

    if (it == NULL || out == NULL) {
        PyErr_SetString(PyExc_SystemError,
                        "pycc_ext_obj_iter_next called with a NULL argument");
        return -1;
    }
    item = PyIter_Next(it);
    if (item != NULL) {
        *out = item;
        return 1;
    }
    return PyErr_Occurred() == NULL ? 0 : -1;
}

/*
 * Part 4 of #1026 (PR 4a of #1083): `float(o)` on a CPython object value
 * (`EXT_OBJ_TO_FLOAT_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * Writes the converted C double through `*out` and answers `0`, or answers
 * `-1` with a CPython exception already set.
 *
 * # Why running CPython's own conversion protocol here is not a D-244
 * # rule-7 violation
 *
 * D-244 rule 7 keeps the type boundary *closed at the thunk export seam*:
 * where a value crosses implicitly, the annotation is the whole contract,
 * so no conversion protocol may run behind the author's back. This helper
 * is on the other side of that distinction. `float(o)` in user source is an
 * *explicit conversion request*: the author wrote the destination type, so
 * running `PyNumber_Float` -- the operand's own `__float__`, `__index__` or
 * string parse -- is exactly what they asked for, not an implicit crossing.
 * The same paragraph is recorded in `docs/TYPE_SYSTEM.md`'s `object` row.
 *
 * # Ownership
 *
 * `PyNumber_Float` hands back a *new* reference, and this helper releases it
 * on every exit, not only the successful one. Part 4's conversions therefore
 * add nothing to the #1092 leak-only set: the converted value is a pycc-side
 * `double` and no CPython reference escapes into compiled code.
 *
 * `PyFloat_AsDouble` on the result of `PyNumber_Float` cannot itself fail --
 * that result is a `float` by construction -- but its `-1.0`-plus-
 * `PyErr_Occurred()` convention is still tested, as defence in depth and so
 * that the ownership rule above holds on a path that is meant to be
 * unreachable.
 *
 * The NULL guard is the same defence in depth `pycc_ext_obj_len` documents.
 */
int pycc_ext_obj_to_float(PyObject *o, double *out)
{
    PyObject *converted;
    double value;

    if (o == NULL || out == NULL) {
        return -1;
    }
    converted = PyNumber_Float(o);
    if (converted == NULL) {
        return -1;
    }
    value = PyFloat_AsDouble(converted);
    Py_DECREF(converted);
    if (value == -1.0 && PyErr_Occurred()) {
        return -1;
    }
    *out = value;
    return 0;
}

/*
 * Part 4 of #1026 (PR 4b of #1083): `int(o)` on a CPython object value
 * (`EXT_OBJ_TO_INT_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * Writes a D-141 *encoded* integer word through `*out` and answers `0`, or
 * answers `-1` with a CPython exception already set.
 *
 * The rule-7 paragraph on `pycc_ext_obj_to_float` above governs this helper
 * too: an explicit `int(o)` in user source names its destination type, so
 * running CPython's own `PyNumber_Long` protocol -- the operand's
 * `__int__`/`__index__` or a string parse -- is what the author asked for,
 * not the implicit thunk-seam crossing D-244 rule 7 closes.
 *
 * # Why this is not `pycc_ext_unpack_int_at`
 *
 * That helper guards `PyBool_Check` and `PyLong_Check` before converting,
 * because rule 7 closes the *thunk* boundary: a `bool` must keep its D-141
 * marker word across an int-shaped parameter slot, and an `__index__` duck
 * type is not an `int`. Neither applies to an explicit conversion, where
 * CPython's own answers -- `int(True)` is `1`, `int(x)` honours
 * `__index__` -- are precisely the contract. Only the *overflow tail* is
 * shared, and it is shared verbatim: the inline-range gate runs after
 * CPython's own overflow check and never against `i64`, because pycc's
 * inline-integer range is [-2**62, 2**62-1] (D-244's 2026-09-12 amendment,
 * #1040). There is deliberately no bigint path here.
 *
 * # Ownership
 *
 * `PyNumber_Long` hands back a *new* reference, released on *every* exit --
 * including the `OverflowError` path, which still holds it at the point it
 * decides to fail. Nothing but an encoded word escapes into compiled code,
 * so this conversion adds nothing to the #1092 leak-only set.
 *
 * The NULL guard is the same defence in depth `pycc_ext_obj_len` documents.
 */
int pycc_ext_obj_to_int(PyObject *o, long long *out)
{
    PyObject *converted;
    long long raw;
    int overflow = 0;

    if (o == NULL || out == NULL) {
        return -1;
    }
    converted = PyNumber_Long(o);
    if (converted == NULL) {
        return -1;
    }
    raw = PyLong_AsLongLongAndOverflow(converted, &overflow);
    Py_DECREF(converted);
    /* `PyLong_AsLongLongAndOverflow` signals an out-of-range value through
     * `overflow` *without* setting an exception, so this arm sees only a real
     * failure -- `pycc_ext_unpack_int_at`'s own ordering, unchanged. */
    if (raw == -1 && PyErr_Occurred()) {
        return -1;
    }
    if (overflow != 0 || pycc_rt_ext_int_encode(raw, out) != 0) {
        PyErr_SetString(PyExc_OverflowError,
                        "int() of a CPython object is outside the inline-integer range "
                        "[-2**62, 2**62-1] this pycc version's `ext` boundary supports "
                        "(see #1040)");
        return -1;
    }
    return 0;
}

/*
 * Part 4 of #1026 (PR 4b of #1083): `str(o)` on a CPython object value
 * (`EXT_OBJ_TO_STR_SYMBOL` in `crates/pycc_codegen/src/ext.rs`).
 *
 * Writes a pycc `PyStrObj *` (opaque `void *` on this side, exactly as in
 * `pycc_ext_unpack_str`) through `*out` and answers `0`, or answers `-1`
 * with a CPython exception already set. The handle arrives at refcount 1
 * and becomes the compiled code's own, which is the identical ownership a
 * `str` literal's own `pycc_rt_str_from_literal` produces -- so codegen
 * needs no new rule for it.
 *
 * The rule-7 paragraph on `pycc_ext_obj_to_float` above governs this helper
 * too; `PyObject_Str` *is* `str()`, so there is no other defensible answer.
 * Unlike `pycc_ext_unpack_str` this does not `PyUnicode_Check` its operand:
 * refusing a non-`str` is exactly what an explicit conversion must not do.
 * A `__str__` that raises, and a result holding a lone surrogate (which
 * `PyUnicode_AsUTF8AndSize` refuses with `UnicodeEncodeError`), are both
 * propagated verbatim. The length is carried explicitly, never re-derived
 * with `strlen`, because a Python `str` may contain embedded NUL bytes.
 *
 * # Ownership -- the copy must complete before the release
 *
 * `PyObject_Str` hands back a *new* reference, and `PyUnicode_AsUTF8AndSize`
 * returns a pointer *into that object's own buffer*. So
 * `pycc_rt_str_from_literal`, which copies those bytes, has to run *before*
 * the `Py_DECREF`; releasing first and copying after is a use-after-free.
 * That ordering is load-bearing and is why this helper is not a copy of
 * `pycc_ext_unpack_str`, which merely borrows its caller's argument and may
 * order the two calls freely. The reference is released on every exit,
 * including the `PyUnicode_AsUTF8AndSize` failure path, so nothing here
 * joins the #1092 leak-only set.
 *
 * The NULL guard is the same defence in depth `pycc_ext_obj_len` documents.
 */
int pycc_ext_obj_to_str(PyObject *o, void **out)
{
    PyObject *converted;
    const char *utf8;
    Py_ssize_t size;
    void *copied;

    if (o == NULL || out == NULL) {
        return -1;
    }
    converted = PyObject_Str(o);
    if (converted == NULL) {
        return -1;
    }
    utf8 = PyUnicode_AsUTF8AndSize(converted, &size);
    if (utf8 == NULL) {
        Py_DECREF(converted);
        return -1;
    }
    copied = pycc_rt_str_from_literal((const unsigned char *)utf8, (long long)size);
    Py_DECREF(converted);
    *out = copied;
    return 0;
}

/*
 * Part 4 of #1026 (PR 4c of #1083): unpacking a CPython object into a
 * fixed-arity all-`float` `tuple` annotation at a module-level annotated
 * assignment (`EXT_OBJ_UNPACK_FLOAT_TUPLE_SYMBOL` in
 * `crates/pycc_codegen/src/ext.rs`).
 *
 * Writes `arity` C doubles through `out[0..arity)` and answers `0`, or
 * answers `-1` with a CPython exception already set.
 *
 * `arity` is a parameter rather than a compile-time constant on purpose:
 * the admission rule is "any fixed arity, every element `float`", so
 * nothing on either side of this seam may hard-code the three of
 * `tuple[float, float, float]`.
 *
 * # Strict container, converting elements
 *
 * The *container* is checked and the *elements* are converted, and the two
 * halves answer two different questions.
 *
 * `PyTuple_Check` with an exact-arity test is the same closed-boundary
 * reading `pycc_ext_unpack_tuple` documents: D-116 fixes a tuple type's
 * arity, pycc holds the result as a by-value LLVM struct of that exact
 * width (D-115), and there is no shape to write a shorter or longer
 * sequence into. `PyTuple_Check` and not `PyTuple_CheckExact` for that
 * helper's reason too -- a `tuple` subclass *is* a tuple, and the elements
 * are copied out by value so the subclass identity does not survive the
 * crossing. A `list`, a generator or any other iterable is refused: this is
 * an assignment to a declared `tuple`, not an unpacking protocol.
 *
 * The *elements*, by contrast, run `PyNumber_Float` -- the operand's own
 * `__float__`, `__index__` or string parse -- which is the rule-7 paragraph
 * on `pycc_ext_obj_to_float` above, unchanged: the author wrote `float` in
 * the annotation, so CPython's own conversion to `float` is what they asked
 * for. It is therefore deliberately *not* `pycc_ext_unpack_float_at`, whose
 * `PyFloat_Check` refusal exists because the *thunk export seam* is closed.
 *
 * # Ownership
 *
 * Each `PyNumber_Float` temporary is released inside the same loop
 * iteration that produced it, before the next one is created. So the
 * failing exit holds nothing: an element that fails to convert leaves the
 * previous iterations' temporaries already released and produces no
 * temporary of its own. Nothing but doubles escapes into compiled code, and
 * this helper adds nothing to the #1092 leak-only set -- Part 4's property,
 * unchanged.
 *
 * `PyTuple_GetItem` returns a *borrowed* reference and is infallible here,
 * because the exact-arity check above has already run --
 * `pycc_ext_unpack_tuple` records that the arity gate exists precisely to
 * make the later `GetItem`s infallible, and the same gate serves the same
 * purpose here.
 *
 * `PyFloat_AsDouble` on the result of `PyNumber_Float` cannot itself fail,
 * that result being a `float` by construction; its `-1.0`-plus-
 * `PyErr_Occurred()` convention is still tested for the reason
 * `pycc_ext_obj_to_float` records, as defence in depth and so that the
 * ownership rule above holds on a path that is meant to be unreachable.
 *
 * The NULL guard is the same defence in depth `pycc_ext_obj_len` documents.
 * `arity` is guarded with it: codegen only ever emits a call for an arity
 * of at least one (the type-checker's own admission rule requires a
 * non-empty tuple), so a non-positive arity can only mean a corrupted call.
 */
int pycc_ext_obj_unpack_float_tuple(PyObject *o, long long arity, double *out)
{
    PyObject *type_name;
    PyObject *converted;
    Py_ssize_t size;
    Py_ssize_t index;
    double value;

    if (o == NULL || out == NULL || arity < 1) {
        PyErr_SetString(PyExc_SystemError,
                        "pycc_ext_obj_unpack_float_tuple called with an invalid argument");
        return -1;
    }
    if (!PyTuple_Check(o)) {
        type_name = PyType_GetName(Py_TYPE(o));
        if (type_name == NULL) {
            PyErr_Format(PyExc_TypeError,
                         "expected a tuple of %zd floats, got a non-tuple object",
                         (Py_ssize_t)arity);
        } else {
            PyErr_Format(PyExc_TypeError,
                         "expected a tuple of %zd floats, got a '%U' object",
                         (Py_ssize_t)arity, type_name);
            Py_DECREF(type_name);
        }
        return -1;
    }
    size = PyTuple_Size(o);
    if (size != (Py_ssize_t)arity) {
        PyErr_Format(PyExc_TypeError,
                     "expected a tuple of %zd floats, got a tuple of length %zd",
                     (Py_ssize_t)arity, size);
        return -1;
    }
    for (index = 0; index < size; index++) {
        converted = PyNumber_Float(PyTuple_GetItem(o, index));
        if (converted == NULL) {
            return -1;
        }
        value = PyFloat_AsDouble(converted);
        Py_DECREF(converted);
        if (value == -1.0 && PyErr_Occurred()) {
            return -1;
        }
        out[index] = value;
    }
    return 0;
}

/*
 * The host-side carrier for a compiled instance (#1145): one CPython object
 * per `mod.Class(...)`, holding nothing but the opaque `PyInstanceObj *`
 * that every compiled method of that class takes as its receiver.
 *
 * Every constructible class's generated `PyType_Spec` sets
 * `basicsize = sizeof(PyccExtInstance)` and uses `PyType_GenericNew`, which
 * zeroes the allocation -- so `inst` is NULL until `tp_init` stores one, and
 * `mod.Class.__new__(mod.Class)` (which never runs `tp_init`) yields a
 * carrier whose NULL every generated wrapper checks before dereferencing.
 *
 * `PyObject_HEAD` is available under `Py_LIMITED_API` and is what makes this
 * a well-formed object layout without naming any field of `PyObject`.
 */
typedef struct {
    PyObject_HEAD
    void *inst;
} PyccExtInstance;

/*
 * The shared `Py_tp_dealloc` for every constructible class -- one function
 * rather than one per class, because none of it is class-dependent.
 *
 * The documented heap-type pattern: a `PyType_FromSpec` type is a heap type,
 * every instance of it holds a reference to it, and `tp_dealloc` is what
 * discharges that reference. Omitting the `Py_DECREF(tp)` leaks the type
 * object invisibly -- nothing observable fails, the module just never
 * releases its own types. `Py_tp_free` is fetched through `PyType_GetSlot`
 * because the limited API exposes no other way to reach the type's
 * deallocator, and it is called *before* the type reference is released so
 * `tp` is still live while the object is freed.
 *
 * The inner `PyInstanceObj` is deliberately not freed; the `extern` above
 * carries the reason.
 */
static void pycc_ext_instance_dealloc(PyObject *self)
{
    PyTypeObject *tp = Py_TYPE(self);
    freefunc tp_free = (freefunc)PyType_GetSlot(tp, Py_tp_free);
    tp_free(self);
    Py_DECREF(tp);
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
    /*
     * The synthesized user exception classes are created and published as
     * module attributes before the module body runs, so a body that raises
     * one already finds it registered. This is the only place the module
     * object is reachable (`m_size = 0`, so there is no module state), and
     * a failure here fails the import loudly rather than importing a module
     * whose `except m.MyError:` silently never matches.
     */
    if (pycc_ext_register_exception_classes(module) != 0) {
        return -1;
    }
    /*
     * One non-instantiable type object per class that exports a method,
     * published under the bare class name. After the exception classes,
     * because a user exception class is already registered under that same
     * attribute and a second `PyModule_AddObjectRef` would clobber it --
     * which is why a class carrying an exception tag never gets a type
     * object at all. Before the module body, because nothing here depends
     * on what the body produces: the wrappers reach the compiled code
     * through the `fnptr_` globals the body stores, and the module is not
     * importable until this function returns. Same -1-with-exception-set
     * convention as everything else in this slot.
     */
    if (pycc_ext_register_method_types(module) != 0) {
        return -1;
    }
    if (pycc_ext_module_exec() != 0) {
        /*
         * The generic `ImportError` is a last resort, not the default. A
         * failing body reports through one of two channels: `pycc_rt`'s
         * thread-local pending state, which `pycc_ext_raise_pending` turns
         * into a live CPython exception, or -- for a body that called into
         * CPython directly -- an exception CPython already set, with no
         * pycc pending state at all. `pycc_ext_raise_pending` returns 0 in
         * that second case, so raising unconditionally here would replace
         * the real exception (a `ModuleNotFoundError` from a failed host
         * import, say) with a message that names neither the cause nor the
         * culprit. Check `PyErr_Occurred` before overwriting, and keep
         * `pycc_ext_raise_pending` as the left operand: it is the call that
         * sets the exception, so short-circuiting must not skip it.
         */
        if (!pycc_ext_raise_pending() && !PyErr_Occurred()) {
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
