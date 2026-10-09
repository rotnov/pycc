//! The fixed C shim's iteration and collection helpers, as codegen names
//! them: acquiring an iterator, advancing it, and building the CPython
//! `list` or `set` a comprehension over an object collects into. Split out
//! of `ext.rs` (AGENTS.md's "Keep source files decomposable") when Part 3
//! of #1499 changed who owns an advanced item; `ext.rs` re-exports every
//! item, so the public paths are unchanged.

/// The fixed C shim's iterator-acquisition helper (Part 3 of #1026, PR 3c
/// of #1082): it takes a borrowed `PyObject *` and returns a *new*
/// reference to `iter(o)` -- `PyObject_GetIter` -- or `NULL` with the
/// CPython exception already set (a non-iterable operand raises
/// `TypeError` there, which is exactly the behaviour pycc wants to
/// surface).
///
/// The iterator is read once, in the loop preheader, and released when the
/// loop ends or fails (Part 3 of #1092). The iterable
/// it was taken from is released right after this call when it is a produced
/// temporary (Part 1 of #1092, `object_release.rs`).
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_GET_ITER_SYMBOL: &str = "pycc_ext_obj_get_iter";

/// The fixed C shim's iterator-advance helper (Part 3 of #1026, PR 3c of
/// #1082). Unlike every other helper here it is *three-valued*: given a
/// borrowed iterator and an out-parameter, it returns `1` having written a
/// *new* reference to the next item through `*out`, `0` on clean
/// exhaustion, or `-1` with a CPython exception already set.
///
/// **The discrimination lives in C, not in LLVM IR.** `PyIter_Next`
/// signals both exhaustion and failure with `NULL`, and only
/// `PyErr_Occurred()` tells the two apart; open-coding that in emitted IR
/// would put a second, independently-maintained copy of a CPython calling
/// convention into this crate. Collapsing the two into one value here is
/// what keeps exhaustion off the module-exec failure edge: a `for` loop
/// that simply ends is not a failure.
///
/// Each item written through `*out` is a new reference. A module-global
/// `for` target owns it and releases it on the next trip (Part 1 of #1499);
/// a comprehension holds it for its own trip and releases it at the trip's
/// end or on the trip's failure edge (Part 3 of #1499, `docs/RUNTIME.md`).
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_ITER_NEXT_SYMBOL: &str = "pycc_ext_obj_iter_next";

/// The fixed C shim's collection constructor (Part 1 of #1255): given a
/// [`ObjCollectionKind`] code it returns a *new* reference to an empty
/// CPython `list` (`0`) or `set` (`1`), or `NULL` with the CPython
/// exception already set. A list or set comprehension over a CPython object
/// builds its result in it; the reference is the comprehension's value,
/// released by its consumer when unbound (Part 3 of #1092), owned by a
/// module global that binds it (Part 1 of #1499), and leaked otherwise.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_NEW_COLLECTION_SYMBOL: &str = "pycc_ext_obj_new_collection";

/// The fixed C shim's collection insert (Part 1 of #1255): given a borrowed
/// collection from [`EXT_OBJ_NEW_COLLECTION_SYMBOL`], the same kind code and
/// a *packed* item, it appends the item to the list or adds it to the set
/// and returns `0`, or returns `-1` with the CPython exception already set
/// (an unhashable set item raises `TypeError` there).
///
/// **Ownership.** The item is a new reference a `pycc_ext_obj_pack_*`
/// helper produced, and it is consumed on every path, exactly as
/// `pycc_ext_obj_call` consumes its arguments: a `NULL` item is a packer
/// that already set the exception, and is reported as a failure.
///
/// Spelled once here for the same lazy-link reason as [`EXT_OBJ_LEN_SYMBOL`](super::EXT_OBJ_LEN_SYMBOL).
pub const EXT_OBJ_COLLECT_SYMBOL: &str = "pycc_ext_obj_collect";

/// Which CPython collection a comprehension over an object builds (Part 1
/// of #1255): the code [`EXT_OBJ_NEW_COLLECTION_SYMBOL`] and
/// [`EXT_OBJ_COLLECT_SYMBOL`] take. Spelled once here so the two helpers'
/// shared numbering cannot drift between their callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjCollectionKind {
    /// A `list`, from `[elt for x in <object>]`.
    List,
    /// A `set`, from `{elt for x in <object>}`.
    Set,
}

impl ObjCollectionKind {
    /// The kind code the shim helpers switch on.
    pub fn shim_code(self) -> u64 {
        match self {
            ObjCollectionKind::List => 0,
            ObjCollectionKind::Set => 1,
        }
    }
}
