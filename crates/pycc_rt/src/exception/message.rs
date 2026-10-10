//! An exception's message, as compiled code and the `ext` boundary read it,
//! including the lazily produced message of a bridged CPython exception
//! (#1511).
//!
//! A `PyExceptionObj` that compiled code allocates always carries its
//! message. One the `ext` C shim's bridge allocates for a CPython exception
//! does not: CPython never formats an exception it only propagates, so the
//! bridge must not call `str(exc)` when an exception merely passes through a
//! compiled function -- a user `__str__` can observe the call (#1511). Such
//! an object starts with a null `message`, and the first read through
//! [`materialized_message`] asks the registered
//! [`ExceptionMessageResolver`] for CPython's own `str(exc)`, then caches the
//! answer on the object.
//!
//! The resolver is a function pointer the C shim registers, not a symbol
//! this crate links against: `libpycc_rt.a` is linked into every `native`
//! executable too, where no interpreter exists (see `ext_bridge`'s module
//! docs). A `native` program never registers one and never allocates a
//! message-less exception.
//!
//! The answer is cached rather than recomputed per read because the
//! accessor's result is a *borrowed* `str` (see
//! [`pycc_rt_exception_message`]): replacing it on a later read would free a
//! string an earlier read in the same expression may still be using. So a
//! bridged exception rendered twice calls `__str__` once, where CPython calls
//! it twice; an exception that is only propagated calls it never, as in
//! CPython.

use super::{EXCEPTION_STATE, PyExceptionObj, alloc_exception_message, exception_type_name};
use crate::PyStrObj;
use std::sync::atomic::{AtomicPtr, Ordering};

/// Produces `obj`'s message on its first read: a new `str` whose single
/// reference passes to the exception, or null when no message can be
/// produced (the CPython original is no longer held, or `str(exc)` failed).
pub type ExceptionMessageResolver = unsafe extern "C" fn(*mut PyExceptionObj) -> *mut PyStrObj;

/// The registered [`ExceptionMessageResolver`], or null. Process-wide rather
/// than per-thread: the C shim's resolver is one function for every thread,
/// and it consults that thread's own bridge table.
static RESOLVER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Serializes every test that registers a resolver or reads a message-less
/// exception: the registration is process-wide and the test harness runs
/// tests on parallel threads.
#[cfg(test)]
pub(super) static RESOLVER_REGISTRATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Registers `resolver` as the producer of every message-less exception's
/// message, replacing any earlier one; `None` unregisters. The `ext` C shim
/// registers its bridge-table lookup before it allocates the first bridged
/// exception.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_set_message_resolver(
    resolver: Option<ExceptionMessageResolver>,
) {
    let raw = resolver.map_or(std::ptr::null_mut(), |f| f as *mut ());
    RESOLVER.store(raw, Ordering::Release);
}

fn registered_resolver() -> Option<ExceptionMessageResolver> {
    let raw = RESOLVER.load(Ordering::Acquire);
    if raw.is_null() {
        return None;
    }
    // Safety: the only non-null value ever stored is an
    // `ExceptionMessageResolver` cast to `*mut ()` above.
    Some(unsafe { std::mem::transmute::<*mut (), ExceptionMessageResolver>(raw) })
}

/// `obj`'s message, producing and caching it first when `obj` carries none.
///
/// The resolver runs arbitrary code (a user `__str__`), which may call back
/// into compiled code and raise there; this thread's pending-exception state
/// is restored afterwards, so the read never leaves or clears a pending pycc
/// exception. When the resolver produces nothing, the message is the class
/// name -- the text the C shim's degraded bridge path already uses.
///
/// # Safety
///
/// `obj` must point to a live `PyExceptionObj`.
pub(crate) unsafe fn materialized_message(obj: *mut PyExceptionObj) -> *mut PyStrObj {
    let existing = unsafe { (*obj).message };
    if !existing.is_null() {
        return existing;
    }
    let saved = EXCEPTION_STATE.with(|state| state.get());
    let resolved = match registered_resolver() {
        Some(resolver) => unsafe { resolver(obj) },
        None => std::ptr::null_mut(),
    };
    EXCEPTION_STATE.with(|state| state.set(saved));
    // A `__str__` that rendered this same exception re-entrantly has already
    // cached a message; keep that one so no earlier borrowed read dangles.
    let cached = unsafe { (*obj).message };
    if !cached.is_null() {
        if !resolved.is_null() {
            unsafe { crate::pycc_rt_str_decref(resolved) };
        }
        return cached;
    }
    let message = if !resolved.is_null() {
        resolved
    } else if unsafe { (*obj).exceptions_len } == 1 {
        // The rest group `except*` derived from a message-less exception
        // (see `pycc_rt_exception_group_partition`) renders as that member.
        unsafe { materialized_message(*(*obj).exceptions) }
    } else {
        alloc_exception_message(exception_type_name(unsafe { &*obj }))
    };
    unsafe { (*obj).message = message };
    message
}

/// Returns the exception's own message string, borrowed and unretained
/// (Part 3A of #541, #736): `print(e)`/f-string interpolation of a caught
/// exception binding must render CPython's `str(e)` semantics -- the message
/// alone, e.g. `boom` -- never `exception_print_and_exit`'s own uncaught-
/// exception `"{type}: {message}"` format, which this function does not
/// touch. No refcount/retain work is needed here: like
/// `pycc_rt_print_write_str`/`pycc_rt_str_concat`, this only borrows a
/// `PyStrObj` the exception owns rather than producing a new owned
/// reference. A bridged exception's message is produced here, on its first
/// read (#1511, see this module's docs).
///
/// # Safety
///
/// A non-null `obj` must point to a live `PyExceptionObj` whose `message`
/// field is null or a live `PyStrObj` pointer -- true of every
/// `PyExceptionObj` this compiler's own codegen ever constructs
/// (`pycc_rt_exception_alloc` always receives a message, defaulting to
/// `"unknown"` when the source `raise` has no argument -- see
/// `pycc_mir::lower_exception_value`) and of every one the `ext` bridge
/// allocates without one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_exception_message(obj: *mut PyExceptionObj) -> *mut PyStrObj {
    unsafe { materialized_message(obj) }
}

/// The pending exception's message as UTF-8 bytes, writing its length through
/// `len`. Null (with `*len == 0`) when nothing is pending.
///
/// The bytes belong to the pending exception object, which this runtime never
/// frees (see [`PyExceptionObj`]'s leak-only note), so they stay readable
/// until the caller has copied them into a CPython exception -- which the
/// shim does immediately, before [`super::pycc_rt_exception_clear`]. The shim
/// reads it only for an exception it could not restore from its bridge
/// table, so a bridged exception's message is produced here only when its
/// CPython original is already gone.
///
/// # Safety
///
/// `len` must be non-null and point to a writable, aligned `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_ext_pending_message(len: *mut usize) -> *const u8 {
    let pending = EXCEPTION_STATE.with(|state| state.get());
    if pending.active == 0 || pending.value.is_null() {
        unsafe { *len = 0 };
        return std::ptr::null();
    }
    let message = unsafe { materialized_message(pending.value) };
    let bytes = unsafe { (*message).bytes() };
    unsafe { *len = bytes.len() };
    bytes.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::super::{
        EXCEPTION_TYPE_EXCEPTION, EXCEPTION_TYPE_TYPE_ERROR, EXCEPTION_TYPE_VALUE_ERROR,
        pycc_rt_exception_alloc, pycc_rt_exception_clear, pycc_rt_exception_group_partition,
        pycc_rt_exception_raise, pycc_rt_exception_value,
    };
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    fn lazy_value_error() -> *mut PyExceptionObj {
        let name = "ValueError";
        pycc_rt_exception_alloc(
            EXCEPTION_TYPE_VALUE_ERROR,
            name.as_ptr(),
            name.len(),
            std::ptr::null_mut(),
        )
    }

    fn text(message: *mut PyStrObj) -> Vec<u8> {
        unsafe { (*message).bytes() }.to_vec()
    }

    unsafe extern "C" fn counting(_obj: *mut PyExceptionObj) -> *mut PyStrObj {
        CALLS.fetch_add(1, Ordering::SeqCst);
        // Arbitrary code -- a `__str__` calling into compiled code -- may
        // raise and clear pycc exceptions of its own.
        pycc_rt_exception_raise(lazy_value_error());
        alloc_exception_message("from the resolver")
    }

    unsafe extern "C" fn failing(_obj: *mut PyExceptionObj) -> *mut PyStrObj {
        std::ptr::null_mut()
    }

    unsafe extern "C" fn reentrant(obj: *mut PyExceptionObj) -> *mut PyStrObj {
        unsafe { (*obj).message = alloc_exception_message("inner") };
        alloc_exception_message("outer")
    }

    unsafe extern "C" fn reentrant_failing(obj: *mut PyExceptionObj) -> *mut PyStrObj {
        unsafe { (*obj).message = alloc_exception_message("inner only") };
        std::ptr::null_mut()
    }

    #[test]
    fn a_lazy_message_is_produced_once_on_first_read_and_restores_pending_state() {
        let _guard = RESOLVER_REGISTRATION
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pycc_rt_exception_set_message_resolver(Some(counting));
        CALLS.store(0, Ordering::SeqCst);
        let obj = lazy_value_error();
        // Allocating and raising never consults the resolver (#1511).
        pycc_rt_exception_raise(obj);
        pycc_rt_exception_clear();
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);

        let first = unsafe { pycc_rt_exception_message(obj) };
        assert_eq!(text(first), b"from the resolver");
        assert!(
            pycc_rt_exception_value().is_null(),
            "pending state restored"
        );
        let second = unsafe { pycc_rt_exception_message(obj) };
        assert_eq!(first, second, "the first answer is cached");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        pycc_rt_exception_set_message_resolver(None);
    }

    #[test]
    fn a_failed_or_missing_resolver_falls_back_to_the_class_name() {
        let _guard = RESOLVER_REGISTRATION
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pycc_rt_exception_set_message_resolver(Some(failing));
        let failed = lazy_value_error();
        assert_eq!(
            text(unsafe { pycc_rt_exception_message(failed) }),
            b"ValueError"
        );
        pycc_rt_exception_set_message_resolver(None);
        let unregistered = lazy_value_error();
        assert_eq!(
            text(unsafe { pycc_rt_exception_message(unregistered) }),
            b"ValueError"
        );
    }

    #[test]
    fn a_reentrantly_cached_message_wins_over_the_outer_answer() {
        let _guard = RESOLVER_REGISTRATION
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pycc_rt_exception_set_message_resolver(Some(reentrant));
        let obj = lazy_value_error();
        assert_eq!(text(unsafe { pycc_rt_exception_message(obj) }), b"inner");
        pycc_rt_exception_set_message_resolver(Some(reentrant_failing));
        let other = lazy_value_error();
        assert_eq!(
            text(unsafe { pycc_rt_exception_message(other) }),
            b"inner only"
        );
        pycc_rt_exception_set_message_resolver(None);
    }

    #[test]
    fn partitioning_a_message_less_exception_never_resolves_its_message() {
        let _guard = RESOLVER_REGISTRATION
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pycc_rt_exception_set_message_resolver(Some(counting));
        CALLS.store(0, Ordering::SeqCst);
        let obj = lazy_value_error();
        let tags = [EXCEPTION_TYPE_VALUE_ERROR];
        let group_name = "ExceptionGroup";
        let mut matched = std::ptr::null_mut();
        let mut rest = std::ptr::null_mut();
        unsafe {
            pycc_rt_exception_group_partition(
                obj,
                tags.as_ptr(),
                tags.len(),
                EXCEPTION_TYPE_EXCEPTION,
                group_name.as_ptr(),
                group_name.len(),
                &raw mut matched,
                &raw mut rest,
            );
        }
        assert!(rest.is_null());
        assert_eq!(text(unsafe { pycc_rt_exception_message(matched) }), b"");
        assert!(unsafe { (*obj).message }.is_null());
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);

        // Unmatched, the rest group stays message-less until rendered, then
        // renders as its sole member, whose own message it produces.
        let unmatched_tags = [EXCEPTION_TYPE_TYPE_ERROR];
        let mut none = std::ptr::null_mut();
        unsafe {
            pycc_rt_exception_group_partition(
                obj,
                unmatched_tags.as_ptr(),
                unmatched_tags.len(),
                EXCEPTION_TYPE_EXCEPTION,
                group_name.as_ptr(),
                group_name.len(),
                &raw mut none,
                &raw mut rest,
            );
        }
        assert!(none.is_null());
        assert!(unsafe { (*rest).message }.is_null());
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
        pycc_rt_exception_set_message_resolver(Some(failing_for_groups));
        let rendered = unsafe { pycc_rt_exception_message(rest) };
        assert_eq!(text(rendered), b"from the resolver");
        assert_eq!(unsafe { (*obj).message }, rendered);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        pycc_rt_exception_set_message_resolver(None);
    }

    /// The C shim's answer: a group `except*` built is never in the bridge
    /// table, its naked member is.
    unsafe extern "C" fn failing_for_groups(obj: *mut PyExceptionObj) -> *mut PyStrObj {
        if unsafe { (*obj).exceptions_len } != 0 {
            return std::ptr::null_mut();
        }
        unsafe { counting(obj) }
    }

    #[test]
    fn the_pending_message_materializes_a_lazy_one() {
        let _guard = RESOLVER_REGISTRATION
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pycc_rt_exception_set_message_resolver(None);
        let mut len = usize::MAX;
        pycc_rt_exception_clear();
        assert!(unsafe { pycc_rt_ext_pending_message(&mut len) }.is_null());
        assert_eq!(len, 0);

        pycc_rt_exception_raise(lazy_value_error());
        let bytes = unsafe { pycc_rt_ext_pending_message(&mut len) };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(bytes, len) },
            b"ValueError"
        );
        pycc_rt_exception_clear();
    }
}
