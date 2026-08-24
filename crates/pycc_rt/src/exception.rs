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
    }))
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
}
