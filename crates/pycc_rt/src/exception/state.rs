//! D-173's per-thread pending-exception state and its C ABI accessors.
//!
//! The state is thread-local so independent Rust test threads cannot race
//! and a thread that calls compiled code cannot observe another thread's
//! pending exception.
//!
//! Generated code reads the `active` flag after every operation that can set
//! it. Reading a Rust `thread_local!` costs more than the load itself where
//! the runtime is linked into a shared object that is loaded at run time
//! (an `--ext` artifact): there, every access to a general-dynamic TLS
//! variable is a call to `__tls_get_addr`, and stable Rust cannot choose a
//! cheaper TLS model. So [`pycc_rt_exception_state`] hands generated code
//! the address of the current thread's `active` flag. A function looks it up
//! once per invocation and every later check is a plain byte load from it
//! (#1518, `docs/RUNTIME.md`'s "The pending-exception check"). An invocation
//! never changes threads, so the address stays the right one for its whole
//! duration.

use super::PyExceptionObj;
use std::cell::Cell;

/// The pending exception, if any.
///
/// `#[repr(C)]` with `active` first: generated code loads `active` as a
/// byte at offset 0 of the address [`pycc_rt_exception_state`] returns.
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct ExceptionState {
    pub(super) active: i8,
    pub(super) value: *mut PyExceptionObj,
}

const _: () = assert!(std::mem::offset_of!(ExceptionState, active) == 0);

impl ExceptionState {
    pub(super) const CLEAR: Self = Self {
        active: 0,
        value: std::ptr::null_mut(),
    };
}

std::thread_local! {
    pub(super) static EXCEPTION_STATE: Cell<ExceptionState> =
        const { Cell::new(ExceptionState::CLEAR) };
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_active() -> i8 {
    EXCEPTION_STATE.with(|state| state.get().active)
}

/// The address of the calling thread's pending-exception flag: an `i8` that
/// is non-zero exactly when [`pycc_rt_exception_active`] would return
/// non-zero on this thread.
///
/// The address is valid, and keeps naming this thread's flag, for as long
/// as the thread runs: the state has a constant initializer and no
/// destructor, so it is never re-created or torn down while the thread
/// lives. Callers only read through it; every write goes through this
/// module's functions.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_state() -> *const i8 {
    EXCEPTION_STATE.with(|state| state.as_ptr().cast::<i8>().cast_const())
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_value() -> *mut PyExceptionObj {
    EXCEPTION_STATE.with(|state| state.get().value)
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_exception_clear() {
    EXCEPTION_STATE.with(|state| state.set(ExceptionState::CLEAR));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag behind the returned address follows the pending state
    /// through a raise and a clear, and each thread gets its own flag.
    #[test]
    fn the_state_address_reads_this_threads_pending_flag() {
        pycc_rt_exception_clear();
        let flag = pycc_rt_exception_state();
        assert_eq!(flag, pycc_rt_exception_state());
        assert_eq!(unsafe { flag.read() }, 0);
        EXCEPTION_STATE.with(|state| {
            state.set(ExceptionState {
                active: 1,
                value: std::ptr::null_mut(),
            });
        });
        assert_eq!(unsafe { flag.read() }, 1);
        assert_eq!(pycc_rt_exception_active(), 1);

        let other = std::thread::spawn(|| {
            let flag = pycc_rt_exception_state();
            (flag as usize, unsafe { flag.read() })
        })
        .join()
        .unwrap();
        assert_ne!(other.0, flag as usize);
        assert_eq!(other.1, 0);

        pycc_rt_exception_clear();
        assert_eq!(unsafe { flag.read() }, 0);
    }
}
