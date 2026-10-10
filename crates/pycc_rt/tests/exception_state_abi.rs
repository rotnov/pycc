//! #1518: the address of the pending-exception flag, read through the `rlib`
//! the way a compiled function reads it -- one `pycc_rt_exception_state()`
//! call, then a plain load per check.
//!
//! `crates/pycc_rt/src/exception/state.rs`'s unit test owns the semantics;
//! this file pins them from outside the crate, which is also where the
//! diff-coverage gate measures this exported function (`docs/TESTING.md`,
//! "A runtime function an integration test links is measured from that
//! binary").

use pycc_rt::{
    EXCEPTION_TYPE_VALUE_ERROR, pycc_rt_exception_active, pycc_rt_exception_alloc,
    pycc_rt_exception_clear, pycc_rt_exception_raise, pycc_rt_exception_state,
};

/// The flag behind the returned address follows a raise and a clear, the
/// address is stable on one thread, and another thread gets its own flag.
#[test]
fn the_state_address_reads_this_threads_pending_flag() {
    pycc_rt_exception_clear();
    let flag = pycc_rt_exception_state();
    assert_eq!(flag, pycc_rt_exception_state());
    assert_eq!(unsafe { flag.read() }, 0);

    let name = b"ValueError";
    let exc = pycc_rt_exception_alloc(
        EXCEPTION_TYPE_VALUE_ERROR,
        name.as_ptr(),
        name.len(),
        std::ptr::null_mut(),
    );
    pycc_rt_exception_raise(exc);
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
