//! Runtime exception objects and per-thread propagation state (D-173).
//!
//! Generated code uses explicit state checks rather than platform unwinding.
//! The state is thread-local so independent Rust test threads cannot race and
//! a future generated thread cannot overwrite another thread's pending
//! exception. Current generated programs remain single-threaded.

use super::{PyStrObj, PyStrPayload};
use std::cell::Cell;

pub const EXCEPTION_TYPE_EXCEPTION: u8 = 0;
pub const EXCEPTION_TYPE_VALUE_ERROR: u8 = 1;
pub const EXCEPTION_TYPE_TYPE_ERROR: u8 = 2;
pub const EXCEPTION_TYPE_KEY_ERROR: u8 = 3;
pub const EXCEPTION_TYPE_INDEX_ERROR: u8 = 4;
pub const EXCEPTION_TYPE_ZERO_DIV_ERROR: u8 = 5;
pub const EXCEPTION_TYPE_RUNTIME_ERROR: u8 = 6;

/// Heap-allocated builtin exception object. Exception lifetime management is
/// intentionally leak-only in this first implementation: clearing a pending
/// exception drops the runtime reference but does not free the object.
pub struct PyExceptionObj {
    pub(crate) type_tag: u8,
    /// The exception class's source name, as a pointer to UTF-8 bytes with no
    /// terminator, plus `name_len`. Part 2 of #541 (D-189): the name used to
    /// be derived from `type_tag` by a `match` in this module, which could
    /// only ever name the seven builtin classes. User-defined exception
    /// classes carry module-assigned tags this runtime knows nothing about, so
    /// the name now travels with the object. Null means "unknown", which
    /// prints as `Exception`.
    pub(crate) name: *const u8,
    pub(crate) name_len: usize,
    pub(crate) message: *mut PyStrObj,
    /// Explicit `raise ... from cause` chain.
    pub(crate) cause: *mut PyExceptionObj,
    /// Implicit handler context. Reserved but not wired in this part.
    pub(crate) _context: *mut PyExceptionObj,
    /// Part 3 of #382 (#542, PEP 654): an `ExceptionGroup`/`BaseExceptionGroup`
    /// instance's member exceptions, as an owned, heap-allocated array of
    /// `exceptions_len` pointers. Null (with `exceptions_len == 0`) for every
    /// ordinary, non-group exception -- which is every `PyExceptionObj` this
    /// runtime constructs outside [`pycc_rt_exception_group_alloc`] and
    /// [`pycc_rt_exception_group_partition`]. `exceptions_len == 0` doubles
    /// as "not a group" throughout this module: [`pycc_rt_exception_group_partition`]
    /// reads it to decide whether to treat `group` itself as an implicit
    /// single member.
    pub(crate) exceptions: *mut *mut PyExceptionObj,
    pub(crate) exceptions_len: usize,
}

#[derive(Clone, Copy)]
struct ExceptionState {
    active: i8,
    value: *mut PyExceptionObj,
}

impl ExceptionState {
    const CLEAR: Self = Self {
        active: 0,
        value: std::ptr::null_mut(),
    };
}

std::thread_local! {
    static EXCEPTION_STATE: Cell<ExceptionState> = const { Cell::new(ExceptionState::CLEAR) };
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_active() -> i8 {
    EXCEPTION_STATE.with(|state| state.get().active)
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_value() -> *mut PyExceptionObj {
    EXCEPTION_STATE.with(|state| state.get().value)
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_clear() {
    EXCEPTION_STATE.with(|state| state.set(ExceptionState::CLEAR));
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_alloc(
    type_tag: u8,
    name: *const u8,
    name_len: usize,
    message: *mut PyStrObj,
) -> *mut PyExceptionObj {
    Box::into_raw(Box::new(PyExceptionObj {
        type_tag,
        name,
        name_len,
        message,
        cause: std::ptr::null_mut(),
        _context: std::ptr::null_mut(),
        exceptions: std::ptr::null_mut(),
        exceptions_len: 0,
    }))
}

/// Copies `exceptions_len` member pointers out of the caller-supplied
/// `exceptions` array into a freshly heap-allocated, owned slice, returning
/// its thin pointer and length (`(null, 0)` for an empty array). Shared by
/// [`pycc_rt_exception_group_alloc`] (a source-level `ExceptionGroup(...)`
/// construction) and [`pycc_rt_exception_group_partition`] (each subgroup it
/// builds), so both leave the exact same "owned, independently freed later
/// under this runtime's leak-only model" shape on `PyExceptionObj::exceptions`.
///
/// # Safety
/// `exceptions` must be null (iff `exceptions_len == 0`) or point to
/// `exceptions_len` valid `*mut PyExceptionObj` values, readable for the
/// duration of this call.
unsafe fn copy_member_array(
    exceptions: *const *mut PyExceptionObj,
    exceptions_len: usize,
) -> (*mut *mut PyExceptionObj, usize) {
    if exceptions_len == 0 {
        return (std::ptr::null_mut(), 0);
    }
    let slice = unsafe { std::slice::from_raw_parts(exceptions, exceptions_len) };
    let boxed: Box<[*mut PyExceptionObj]> = slice.to_vec().into_boxed_slice();
    (Box::leak(boxed).as_mut_ptr(), exceptions_len)
}

/// Builds a fresh `ExceptionGroup`-shaped [`PyExceptionObj`] wrapping
/// `members`, or returns null when `members` is empty. Used by
/// [`pycc_rt_exception_group_partition`] for both the matched and remaining
/// halves of a partition -- an empty half means "this handler caught
/// nothing" / "nothing is left to reraise", which codegen tests via a null
/// pointer check exactly like every other D-173 exception-object slot.
fn build_group_or_null(
    members: Vec<*mut PyExceptionObj>,
    group_type_tag: u8,
    group_name: *const u8,
    group_name_len: usize,
    message: *mut PyStrObj,
) -> *mut PyExceptionObj {
    if members.is_empty() {
        return std::ptr::null_mut();
    }
    // Safety: `members` was just built from live `*mut PyExceptionObj`
    // values (either `pycc_rt_exception_group_partition`'s own `group`, its
    // existing member array, or both), so the pointer and length passed to
    // `copy_member_array` are valid for the duration of this call.
    let (exceptions_ptr, exceptions_len) =
        unsafe { copy_member_array(members.as_ptr(), members.len()) };
    Box::into_raw(Box::new(PyExceptionObj {
        type_tag: group_type_tag,
        name: group_name,
        name_len: group_name_len,
        message,
        cause: std::ptr::null_mut(),
        _context: std::ptr::null_mut(),
        exceptions: exceptions_ptr,
        exceptions_len,
    }))
}

/// Constructs a heap-allocated `ExceptionGroup`/`BaseExceptionGroup`
/// instance carrying `exceptions_len` member exceptions, copied out of
/// `exceptions` (Part 3 of #382, #542, PEP 654). The one production caller
/// is codegen's lowering of a literal-list `ExceptionGroup(msg, [e1, e2])`
/// construction (`pycc_types`/`pycc_mir` restrict the second argument to a
/// literal list -- see D-202 -- so codegen always has a fixed-size array of
/// already-evaluated member pointers to hand this in).
///
/// # Safety
/// `exceptions` must be null (iff `exceptions_len == 0`) or point to
/// `exceptions_len` valid `*mut PyExceptionObj` values, readable for the
/// duration of this call; the array is copied, not retained by pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_group_alloc(
    type_tag: u8,
    name: *const u8,
    name_len: usize,
    message: *mut PyStrObj,
    exceptions: *const *mut PyExceptionObj,
    exceptions_len: usize,
) -> *mut PyExceptionObj {
    // Safety: forwarded from this function's own safety contract.
    let (exceptions_ptr, exceptions_len) =
        unsafe { copy_member_array(exceptions, exceptions_len) };
    Box::into_raw(Box::new(PyExceptionObj {
        type_tag,
        name,
        name_len,
        message,
        cause: std::ptr::null_mut(),
        _context: std::ptr::null_mut(),
        exceptions: exceptions_ptr,
        exceptions_len,
    }))
}

/// Partitions `group`'s member exceptions by whether they match any tag in
/// `tags` (Part 3 of #382, #542, PEP 654 `except*` dispatch): an empty
/// `tags` (`tags_len == 0`) matches every member, mirroring a bare
/// `except*:`; a nonempty `tags` matches a member whose own `type_tag`
/// equals [`EXCEPTION_TYPE_EXCEPTION`] (the universal catch-all, consistent
/// with [`pycc_rt_exception_type_matches`]) or any entry in `tags`
/// (Part 2 of #541's multi-tag subclass matching, carried over unchanged).
///
/// `matched_out` receives a fresh `ExceptionGroup`-shaped group wrapping the
/// matched members, or null when none matched; `rest_out` receives the same
/// for the unmatched members, or null when none remain. Both constructed
/// groups are tagged `group_type_tag`/`group_name`/`group_name_len` --
/// supplied by codegen (which resolves `ExceptionGroup`'s fixed builtin tag
/// and name at MIR-lowering time) rather than hardcoded here, so this
/// runtime module carries no compile-time knowledge of that tag's numeric
/// value.
///
/// A `group` whose own `exceptions_len` is 0 is treated as an implicit
/// single-member group containing `group` itself: an ordinary exception
/// raised inside a `try*` body that is not already an `ExceptionGroup`,
/// which a matching `except*` clause still catches (and still wraps in a
/// fresh one-member group on the way out, matching CPython's own PEP 654
/// semantics).
///
/// # Safety
/// `group` must be non-null and point to a live `PyExceptionObj`. `tags`
/// must be null (iff `tags_len == 0`) or point to `tags_len` readable
/// bytes. `matched_out`/`rest_out` must be valid, non-null, writable
/// out-pointers.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_group_partition(
    group: *mut PyExceptionObj,
    tags: *const u8,
    tags_len: usize,
    group_type_tag: u8,
    group_name: *const u8,
    group_name_len: usize,
    matched_out: *mut *mut PyExceptionObj,
    rest_out: *mut *mut PyExceptionObj,
) {
    let obj = unsafe { &*group };
    let members: Vec<*mut PyExceptionObj> = if obj.exceptions_len == 0 {
        vec![group]
    } else {
        unsafe { std::slice::from_raw_parts(obj.exceptions, obj.exceptions_len) }.to_vec()
    };
    let tag_slice: &[u8] = if tags_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(tags, tags_len) }
    };
    let mut matched = Vec::new();
    let mut rest = Vec::new();
    for member in members {
        let member_tag = unsafe { (*member).type_tag };
        let is_match = tag_slice.is_empty()
            || tag_slice
                .iter()
                .any(|tag| *tag == EXCEPTION_TYPE_EXCEPTION || member_tag == *tag);
        if is_match {
            matched.push(member);
        } else {
            rest.push(member);
        }
    }
    let message = obj.message;
    unsafe {
        *matched_out =
            build_group_or_null(matched, group_type_tag, group_name, group_name_len, message);
        *rest_out =
            build_group_or_null(rest, group_type_tag, group_name, group_name_len, message);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_raise(obj: *mut PyExceptionObj) {
    EXCEPTION_STATE.with(|state| {
        state.set(ExceptionState {
            active: 1,
            value: obj,
        });
    });
}

/// Raises `obj` and records its explicit cause.
///
/// # Safety
///
/// A non-null `obj` and `cause` must point to live `PyExceptionObj`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_raise_with_cause(
    obj: *mut PyExceptionObj,
    cause: *mut PyExceptionObj,
) {
    if !obj.is_null() {
        unsafe { (*obj).cause = cause };
    }
    pycc_rt_exception_raise(obj);
}

/// Returns whether `obj` matches the requested builtin exception tag.
/// Every supported builtin exception is a direct subclass of `Exception`.
///
/// # Safety
///
/// A non-null `obj` must point to a live `PyExceptionObj`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_type_matches(
    obj: *mut PyExceptionObj,
    type_tag: u8,
) -> i8 {
    if obj.is_null() {
        return 0;
    }
    let obj_tag = unsafe { (*obj).type_tag };
    i8::from(type_tag == EXCEPTION_TYPE_EXCEPTION || obj_tag == type_tag)
}

/// Returns the exception's own message string, borrowed and unretained
/// (Part 3A of #541, #736): `print(e)`/f-string interpolation of a caught
/// exception binding must render CPython's `str(e)` semantics -- the message
/// alone, e.g. `boom` -- never `exception_print_and_exit`'s own uncaught-
/// exception `"{type}: {message}"` format, which this function does not
/// touch. No refcount/retain work is needed here: like
/// `pycc_rt_print_write_str`/`pycc_rt_str_concat`, this only borrows an
/// existing `PyStrObj` pointer rather than producing a new owned reference.
///
/// # Safety
///
/// A non-null `obj` must point to a live `PyExceptionObj` whose `message`
/// field is a live `PyStrObj` pointer -- true of every `PyExceptionObj` this
/// compiler's own codegen ever constructs (`pycc_rt_exception_alloc` always
/// receives a message, defaulting to `"unknown"` when the source `raise` has
/// no argument -- see `pycc_mir::lower_exception_value`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_message(obj: *mut PyExceptionObj) -> *mut PyStrObj {
    unsafe { (*obj).message }
}

/// The exception class's name, or `Exception` when the object carries none.
///
/// # Safety
///
/// A non-null `exc.name` must point to `exc.name_len` readable UTF-8 bytes
/// that outlive this call. Codegen only ever supplies a static string
/// constant, and `raise_builtin` only ever supplies a `&'static str`.
fn exception_type_name(exc: &PyExceptionObj) -> &str {
    if exc.name.is_null() {
        return "Exception";
    }
    let bytes = unsafe { std::slice::from_raw_parts(exc.name, exc.name_len) };
    std::str::from_utf8(bytes).unwrap_or("Exception")
}

#[cfg(not(test))]
fn exception_print_and_exit(obj: *mut PyExceptionObj) -> ! {
    if obj.is_null() {
        eprintln!("Exception");
        std::process::exit(1);
    }
    let exc = unsafe { &*obj };
    let type_name = exception_type_name(exc);
    if exc.message.is_null() {
        eprintln!("{type_name}");
    } else {
        let msg_bytes = unsafe { (*exc.message).bytes() };
        eprintln!("{type_name}: {}", String::from_utf8_lossy(msg_bytes));
    }
    std::process::exit(1);
}

/// Prints an uncaught exception and exits with status 1.
///
/// # Safety
///
/// A non-null `obj` must point to a live `PyExceptionObj`.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_print_and_exit(obj: *mut PyExceptionObj) -> ! {
    exception_print_and_exit(obj)
}

fn alloc_exception_message(msg: &str) -> *mut PyStrObj {
    let bytes = msg.as_bytes();
    let len = bytes.len();
    let payload = if len <= 22 {
        let mut buf = [0u8; 22];
        buf[..len].copy_from_slice(bytes);
        PyStrPayload::Inline(buf, len as u8)
    } else {
        PyStrPayload::Heap(bytes.into())
    };
    Box::into_raw(Box::new(PyStrObj {
        rc: Cell::new(1),
        payload,
    }))
}

pub(crate) fn raise_builtin(type_tag: u8, name: &'static str, msg: &str) {
    let message = alloc_exception_message(msg);
    pycc_rt_exception_raise(pycc_rt_exception_alloc(
        type_tag,
        name.as_ptr(),
        name.len(),
        message,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allocates an exception carrying a class name, the shape every
    /// production caller uses since Part 2 of #541 (D-189).
    fn alloc_named(type_tag: u8, name: &'static str, msg: &str) -> *mut PyExceptionObj {
        pycc_rt_exception_alloc(
            type_tag,
            name.as_ptr(),
            name.len(),
            alloc_exception_message(msg),
        )
    }

    #[test]
    fn class_name_round_trips_through_the_exception_object() {
        let obj = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "MyError", "boom");
        assert_eq!(exception_type_name(unsafe { &*obj }), "MyError");
    }

    #[test]
    fn a_nameless_exception_object_reports_the_root_class() {
        let obj = pycc_rt_exception_alloc(
            EXCEPTION_TYPE_VALUE_ERROR,
            std::ptr::null(),
            0,
            alloc_exception_message("boom"),
        );
        assert_eq!(exception_type_name(unsafe { &*obj }), "Exception");
    }

    #[test]
    fn a_non_utf8_class_name_reports_the_root_class() {
        // Codegen never emits one; the runtime still must not panic on it.
        const INVALID: [u8; 2] = [0xff, 0xfe];
        let obj = pycc_rt_exception_alloc(
            EXCEPTION_TYPE_VALUE_ERROR,
            INVALID.as_ptr(),
            INVALID.len(),
            alloc_exception_message("boom"),
        );
        assert_eq!(exception_type_name(unsafe { &*obj }), "Exception");
    }

    #[test]
    fn exception_message_returns_the_borrowed_message_pointer_unretained() {
        // Part 3A of #541 (#736): `pycc_rt_exception_message` returns the
        // exact `message` field, borrowed and unretained, matching this
        // module's own `bytes()`-comparison convention used elsewhere in
        // this file (e.g. `class_name_round_trips_through_the_exception_object`
        // sibling tests).
        let obj = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "boom");
        let message = unsafe { pycc_rt_exception_message(obj) };
        assert_eq!(unsafe { (*message).bytes() }, b"boom");
        assert_eq!(message, unsafe { (*obj).message });
    }

    #[test]
    fn state_starts_clear_and_clear_resets_value() {
        pycc_rt_exception_clear();
        assert_eq!(pycc_rt_exception_active(), 0);
        assert!(pycc_rt_exception_value().is_null());
        let obj = alloc_named(EXCEPTION_TYPE_RUNTIME_ERROR, "RuntimeError", "clear test");
        pycc_rt_exception_raise(obj);
        assert_eq!(pycc_rt_exception_active(), 1);
        assert_eq!(pycc_rt_exception_value(), obj);
        pycc_rt_exception_clear();
        assert_eq!(pycc_rt_exception_active(), 0);
        assert!(pycc_rt_exception_value().is_null());
    }

    #[test]
    fn state_is_isolated_between_threads() {
        pycc_rt_exception_clear();
        let child = std::thread::spawn(|| {
            let obj = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "child");
            pycc_rt_exception_raise(obj);
            assert_eq!(pycc_rt_exception_value(), obj);
            assert_eq!(pycc_rt_exception_active(), 1);
        });
        child.join().unwrap();
        assert_eq!(pycc_rt_exception_active(), 0);
        assert!(pycc_rt_exception_value().is_null());
    }

    #[test]
    fn type_matching_is_exact_except_for_exception_root() {
        let obj = alloc_named(EXCEPTION_TYPE_KEY_ERROR, "KeyError", "key");
        assert_eq!(
            unsafe { pycc_rt_exception_type_matches(obj, EXCEPTION_TYPE_KEY_ERROR) },
            1
        );
        assert_eq!(
            unsafe { pycc_rt_exception_type_matches(obj, EXCEPTION_TYPE_VALUE_ERROR) },
            0
        );
        assert_eq!(
            unsafe { pycc_rt_exception_type_matches(obj, EXCEPTION_TYPE_EXCEPTION) },
            1
        );
        assert_eq!(
            unsafe { pycc_rt_exception_type_matches(std::ptr::null_mut(), 0) },
            0
        );
    }

    #[test]
    fn explicit_cause_and_default_fields_are_preserved() {
        pycc_rt_exception_clear();
        let cause = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "cause");
        let exc = alloc_named(EXCEPTION_TYPE_RUNTIME_ERROR, "RuntimeError", "effect");
        assert_eq!(unsafe { (*(*exc).message).bytes() }, b"effect");
        assert!(unsafe { (*exc).cause }.is_null());
        assert!(unsafe { (*exc)._context }.is_null());
        unsafe { pycc_rt_exception_raise_with_cause(exc, cause) };
        assert_eq!(unsafe { (*exc).cause }, cause);
        assert_eq!(pycc_rt_exception_value(), exc);
        pycc_rt_exception_clear();
    }

    #[test]
    fn raise_with_cause_accepts_a_null_primary_object() {
        unsafe { pycc_rt_exception_raise_with_cause(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(pycc_rt_exception_active(), 1);
        assert!(pycc_rt_exception_value().is_null());
        pycc_rt_exception_clear();
    }

    #[test]
    fn message_allocation_covers_inline_and_heap_payloads() {
        let inline = alloc_exception_message("short");
        assert_eq!(unsafe { (*inline).bytes() }, b"short");
        let long = "this is a long exception message that exceeds 22 bytes";
        let heap = alloc_exception_message(long);
        assert_eq!(unsafe { (*heap).bytes() }, long.as_bytes());
    }

    #[test]
    fn builtin_raise_sets_the_requested_tag() {
        pycc_rt_exception_clear();
        raise_builtin(EXCEPTION_TYPE_ZERO_DIV_ERROR, "ZeroDivisionError", "zero");
        let obj = pycc_rt_exception_value();
        assert_eq!(unsafe { (*obj).type_tag }, EXCEPTION_TYPE_ZERO_DIV_ERROR);
        pycc_rt_exception_clear();
    }

    // -- Part 3 of #382 (#542, PEP 654): exception-group construction and
    // -- partition tests, written before `emit_try_star` (TDD, per this
    // -- issue's implementation plan).

    const GROUP_TAG: u8 = 24; // `ExceptionGroup`'s fixed builtin tag.
    const GROUP_NAME: &str = "ExceptionGroup";

    fn group_of(members: &[*mut PyExceptionObj]) -> *mut PyExceptionObj {
        unsafe {
            pycc_rt_exception_group_alloc(
                GROUP_TAG,
                GROUP_NAME.as_ptr(),
                GROUP_NAME.len(),
                alloc_exception_message("group"),
                members.as_ptr(),
                members.len(),
            )
        }
    }

    fn partition(
        group: *mut PyExceptionObj,
        tags: &[u8],
    ) -> (*mut PyExceptionObj, *mut PyExceptionObj) {
        let mut matched = std::ptr::null_mut();
        let mut rest = std::ptr::null_mut();
        unsafe {
            pycc_rt_exception_group_partition(
                group,
                tags.as_ptr(),
                tags.len(),
                GROUP_TAG,
                GROUP_NAME.as_ptr(),
                GROUP_NAME.len(),
                &mut matched,
                &mut rest,
            );
        }
        (matched, rest)
    }

    #[test]
    fn group_alloc_copies_the_member_array_and_reports_its_length() {
        let a = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "a");
        let b = alloc_named(EXCEPTION_TYPE_TYPE_ERROR, "TypeError", "b");
        let group = group_of(&[a, b]);
        unsafe {
            assert_eq!((*group).type_tag, GROUP_TAG);
            assert_eq!((*group).exceptions_len, 2);
            let members = std::slice::from_raw_parts((*group).exceptions, 2);
            assert_eq!(members, [a, b]);
        }
    }

    #[test]
    fn group_alloc_with_no_members_leaves_a_null_exceptions_pointer() {
        let group = group_of(&[]);
        unsafe {
            assert_eq!((*group).exceptions_len, 0);
            assert!((*group).exceptions.is_null());
        }
    }

    #[test]
    fn partition_splits_a_group_into_matched_and_unmatched_subgroups() {
        let value_err = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "v");
        let type_err = alloc_named(EXCEPTION_TYPE_TYPE_ERROR, "TypeError", "t");
        let key_err = alloc_named(EXCEPTION_TYPE_KEY_ERROR, "KeyError", "k");
        let group = group_of(&[value_err, type_err, key_err]);
        let (matched, rest) = partition(group, &[EXCEPTION_TYPE_VALUE_ERROR, EXCEPTION_TYPE_KEY_ERROR]);
        unsafe {
            assert!(!matched.is_null());
            assert_eq!((*matched).type_tag, GROUP_TAG);
            assert_eq!((*matched).exceptions_len, 2);
            assert_eq!(
                std::slice::from_raw_parts((*matched).exceptions, 2),
                [value_err, key_err]
            );
            assert!(!rest.is_null());
            assert_eq!((*rest).exceptions_len, 1);
            assert_eq!(std::slice::from_raw_parts((*rest).exceptions, 1), [type_err]);
        }
    }

    #[test]
    fn partition_where_every_member_matches_leaves_rest_null() {
        let value_err = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "v");
        let group = group_of(&[value_err]);
        let (matched, rest) = partition(group, &[EXCEPTION_TYPE_VALUE_ERROR]);
        unsafe {
            assert!(!matched.is_null());
            assert_eq!((*matched).exceptions_len, 1);
        }
        assert!(rest.is_null());
    }

    #[test]
    fn partition_where_no_member_matches_leaves_matched_null() {
        let value_err = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "v");
        let group = group_of(&[value_err]);
        let (matched, rest) = partition(group, &[EXCEPTION_TYPE_KEY_ERROR]);
        assert!(matched.is_null());
        unsafe {
            assert!(!rest.is_null());
            assert_eq!((*rest).exceptions_len, 1);
        }
    }

    #[test]
    fn partition_with_an_empty_tag_set_matches_every_member_like_bare_except_star() {
        let value_err = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "v");
        let type_err = alloc_named(EXCEPTION_TYPE_TYPE_ERROR, "TypeError", "t");
        let group = group_of(&[value_err, type_err]);
        let (matched, rest) = partition(group, &[]);
        unsafe {
            assert!(!matched.is_null());
            assert_eq!((*matched).exceptions_len, 2);
        }
        assert!(rest.is_null());
    }

    #[test]
    fn partition_with_the_exception_root_tag_matches_every_member() {
        // `EXCEPTION_TYPE_EXCEPTION` is the universal catch-all, consistent
        // with `pycc_rt_exception_type_matches`: a handler for it catches
        // every member regardless of that member's own tag.
        let value_err = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "v");
        let key_err = alloc_named(EXCEPTION_TYPE_KEY_ERROR, "KeyError", "k");
        let group = group_of(&[value_err, key_err]);
        let (matched, rest) = partition(group, &[EXCEPTION_TYPE_EXCEPTION]);
        unsafe {
            assert!(!matched.is_null());
            assert_eq!((*matched).exceptions_len, 2);
        }
        assert!(rest.is_null());
    }

    #[test]
    fn partition_treats_a_non_group_exception_as_an_implicit_single_member() {
        // An ordinary exception (`exceptions_len == 0`) raised inside a
        // `try*` body -- not already an `ExceptionGroup` -- is still caught
        // by a matching `except*` clause, wrapped as a single-member group.
        let plain = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "plain");
        assert_eq!(unsafe { (*plain).exceptions_len }, 0);
        let (matched, rest) = partition(plain, &[EXCEPTION_TYPE_VALUE_ERROR]);
        unsafe {
            assert!(!matched.is_null());
            assert_eq!((*matched).exceptions_len, 1);
            assert_eq!(std::slice::from_raw_parts((*matched).exceptions, 1), [plain]);
        }
        assert!(rest.is_null());
    }

    #[test]
    fn partition_of_a_non_group_exception_that_does_not_match_leaves_it_in_rest() {
        let plain = alloc_named(EXCEPTION_TYPE_VALUE_ERROR, "ValueError", "plain");
        let (matched, rest) = partition(plain, &[EXCEPTION_TYPE_KEY_ERROR]);
        assert!(matched.is_null());
        unsafe {
            assert!(!rest.is_null());
            assert_eq!(std::slice::from_raw_parts((*rest).exceptions, 1), [plain]);
        }
    }
}
