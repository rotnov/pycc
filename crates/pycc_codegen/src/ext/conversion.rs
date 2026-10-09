//! The fixed C shim's explicit conversion, unboxing and unpacking helpers,
//! as codegen names them: `float(o)`, `int(o)`, `str(o)`, `format`, the
//! narrowed-read unboxers and tuple unpacking. Split out of `ext.rs`
//! (AGENTS.md's "Keep source files decomposable") to bring it under the
//! ~1,000-line threshold; `ext.rs` re-exports every item, so the public
//! paths are unchanged.

/// The fixed C shim's `float(o)` conversion helper (Part 4 of #1026, PR 4a
/// of #1083): it takes a borrowed `PyObject *` and a `double *`
/// out-parameter, writes the converted value and returns `0`, or returns
/// `-1` with the CPython exception already set.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// D-244 rule 7 keeps the type boundary closed at the *thunk export seam*,
/// where a value crosses implicitly and the annotation is the whole
/// contract. `float(o)` in user source names its destination type, so
/// running CPython's own `PyNumber_Float` protocol -- the operand's
/// `__float__`, `__index__` or string parse -- is precisely what the author
/// asked for. `docs/TYPE_SYSTEM.md`'s `object` row and the helper's own C
/// comment record the same distinction.
///
/// **Ownership.** The helper releases the temporary `PyNumber_Float`
/// produces on *every* exit, including the failing one, and nothing but a
/// `double` escapes into compiled code -- so unlike an attribute load or a
/// subscript, this operation adds nothing to the #1092 leak-only set.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_TO_FLOAT_SYMBOL: &str = "pycc_ext_obj_to_float";

/// The fixed C shim's `int(o)` conversion helper (Part 4 of #1026, PR 4b
/// of #1083): it takes a borrowed `PyObject *` and a `long long *`
/// out-parameter, writes a **D-141 encoded** integer word and returns `0`,
/// or returns `-1` with the CPython exception already set.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// The distinction [`EXT_OBJ_TO_FLOAT_SYMBOL`] records applies unchanged:
/// D-244 rule 7 closes the type boundary at the *thunk export seam*, and
/// `int(o)` in user source names its destination type, so running CPython's
/// own `PyNumber_Long` protocol is what the author asked for. It is
/// therefore *not* `pycc_ext_unpack_int_at`, whose `PyBool_Check` and
/// `PyLong_Check` guards exist precisely because that seam is closed.
///
/// **Overflow.** The encode is fused into the helper, exactly as
/// [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL)'s is and for its reason (one failure edge rather
/// than two). A value outside pycc's inline-integer range
/// `[-2**62, 2**62-1]` raises `OverflowError` citing #1040 -- there is no
/// bigint path across this boundary.
///
/// **Ownership.** The helper releases the `PyNumber_Long` temporary on
/// *every* exit, including the `OverflowError` path that still holds it, so
/// this operation adds nothing to the #1092 leak-only set.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_TO_INT_SYMBOL: &str = "pycc_ext_obj_to_int";

/// The fixed C shim's `str(o)` conversion helper (Part 4 of #1026, PR 4b of
/// #1083): it takes a borrowed `PyObject *` and a `void **` out-parameter,
/// writes a pycc `PyStrObj *` at refcount 1 and returns `0`, or returns `-1`
/// with the CPython exception already set.
///
/// **This is an explicit conversion, not an implicit boundary crossing.**
/// See [`EXT_OBJ_TO_FLOAT_SYMBOL`]; `PyObject_Str` *is* `str()`, so no other
/// answer is defensible. Unlike `pycc_ext_unpack_str` it does not
/// `PyUnicode_Check` its operand -- refusing a non-`str` is exactly what an
/// explicit conversion must not do.
///
/// **Ownership.** The handle written through the out-parameter is produced
/// by `pycc_rt_str_from_literal`, the same call a `str` literal's own
/// emission uses, and arrives as compiled code's own reference -- so this
/// crate needs no new rule for it. On the C side the copy must complete
/// *before* the `PyObject_Str` result is released, because
/// `PyUnicode_AsUTF8AndSize` points into that result's buffer; the helper's
/// own comment records why that ordering is load-bearing. The temporary is
/// released on every exit, so nothing joins the #1092 leak-only set.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_TO_STR_SYMBOL: &str = "pycc_ext_obj_to_str";

/// The fixed C shim's f-string interpolation helper (#1340): it takes a
/// borrowed `PyObject *` and a `void **` out-parameter, writes a pycc
/// `PyStrObj *` at refcount 1 and returns `0`, or returns `-1` with the
/// CPython exception already set.
///
/// [`EXT_OBJ_TO_STR_SYMBOL`]'s contract, ownership and copy-before-release
/// ordering exactly, with `PyObject_Format(o, NULL)` in place of
/// `PyObject_Str(o)`: CPython renders `f"{value}"` as `format(value, '')`,
/// which reaches the value's `__format__`, not its `__str__`. `print(o)`
/// writes `str(o)` and so reaches [`EXT_OBJ_TO_STR_SYMBOL`] instead.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_FORMAT_SYMBOL: &str = "pycc_ext_obj_format";

/// The fixed C shim's narrowed-read helpers (#1476, Part 3 of #1387): the
/// read of an `object` name an `isinstance(o, int|float|bool|str)` guard
/// narrowed (`pycc_mir::MirExpr::ObjectUnbox`). Each takes a borrowed
/// `PyObject *` and an out-parameter of the native ABI type (`long long`
/// D-141 word, `double`, one-byte `char`, `PyStrObj *`), writes the value
/// and returns `0`, or returns `-1` with the CPython exception set.
///
/// The read is an implicit crossing, so each helper keeps D-244 rule 7's
/// closed type check rather than CPython's conversion protocol: a `bool`
/// under an `int` guard keeps its D-141 marker word, an `int` outside the
/// inline range raises `OverflowError` citing #1040, and a `str` subclass
/// is copied into a plain pycc `str` at refcount 1 (the ownership
/// [`EXT_OBJ_TO_STR_SYMBOL`] documents). No CPython reference is taken.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_UNBOX_INT_SYMBOL: &str = "pycc_ext_obj_unbox_int";
/// See [`EXT_OBJ_UNBOX_INT_SYMBOL`].
pub const EXT_OBJ_UNBOX_FLOAT_SYMBOL: &str = "pycc_ext_obj_unbox_float";
/// See [`EXT_OBJ_UNBOX_INT_SYMBOL`].
pub const EXT_OBJ_UNBOX_BOOL_SYMBOL: &str = "pycc_ext_obj_unbox_bool";
/// See [`EXT_OBJ_UNBOX_INT_SYMBOL`].
pub const EXT_OBJ_UNBOX_STR_SYMBOL: &str = "pycc_ext_obj_unbox_str";
/// The narrowed read under `isinstance(o, C)` for a regular class `C`
/// compiled in this module (#1476): a borrowed `PyObject *`, the class's
/// NUL-terminated name and a `void **` out-parameter; writes the compiled
/// instance the carrier holds, borrowed (compiled instances are never
/// freed), and returns `0`, or returns `-1` with a `TypeError` set for an
/// uninitialized carrier or an object that is not a carrier of this module.
pub const EXT_OBJ_UNBOX_INSTANCE_SYMBOL: &str = "pycc_ext_obj_unbox_instance";

/// The fixed C shim's fixed-arity all-`float` tuple unpack helper (Part 4
/// of #1026, PR 4c of #1083): it takes a borrowed `PyObject *`, the
/// declared arity and a `double *` out-array, writes that many converted
/// doubles and returns `0`, or returns `-1` with the CPython exception
/// already set.
///
/// **Strict container, converting elements.** The helper checks
/// `PyTuple_Check` with an *exact*-arity test and then converts each item
/// with `PyNumber_Float`. The two halves answer two different questions:
/// D-115/D-116 hold a tuple as a by-value LLVM struct of fixed width, so
/// there is no shape a `list`, a generator or a differently-sized tuple
/// could be written into -- while the elements' `float` is a type the
/// author wrote in the annotation, which makes running CPython's own
/// conversion protocol on them the same explicit-conversion case
/// [`EXT_OBJ_TO_FLOAT_SYMBOL`] records. It is therefore *not*
/// `pycc_ext_unpack_float_at`, whose `PyFloat_Check` refusal exists because
/// the thunk export seam is closed.
///
/// **The arity is a parameter, never a constant.** The admission rule is
/// any fixed arity with every element `float`, so neither this declaration
/// nor the shim may hard-code the three of `tuple[float, float, float]`.
///
/// **Ownership.** Each `PyNumber_Float` temporary is released inside the
/// same loop iteration that produced it, so the failing exit holds nothing
/// and this operation adds nothing to the #1092 leak-only set -- Part 4's
/// property, unchanged.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_UNPACK_FLOAT_TUPLE_SYMBOL: &str = "pycc_ext_obj_unpack_float_tuple";

/// The fixed C shim's tuple-unpacking helper (Part 1 of #891): it takes a
/// borrowed `PyObject *` and the target count `n`, and returns a *new*
/// reference to a `tuple` of exactly `n` items taken from the object by
/// CPython's own unpack protocol, or `NULL` with CPython's own exception
/// set -- `TypeError` for a non-iterable, `ValueError` for too many or too
/// few values. The tuple is bound to the unpacking temporary: leaked on the
/// #1092 rule for a bound value in a function body, owned and released on
/// rebind by a module-global temporary (Part 1 of #1499).
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_UNPACK_SYMBOL: &str = "pycc_ext_obj_unpack";
