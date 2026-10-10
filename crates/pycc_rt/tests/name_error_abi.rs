//! #1490: `pycc_rt_name_error` raises a pending exception and returns,
//! exercised through the `rlib` the way a native build's codegen calls it
//! -- name bytes plus length, then a branch to the exception target.
//!
//! `crates/pycc_rt/src/name_error.rs`'s unit tests own the semantics
//! (including the class name, which no exported accessor reads); this file
//! pins them from outside the crate, which is also where the diff-coverage
//! gate measures this exported function (`docs/TESTING.md`, "A runtime
//! function an integration test links is measured from that binary").

use pycc_rt::{
    EXCEPTION_TYPE_EXCEPTION, pycc_rt_exception_active, pycc_rt_exception_clear,
    pycc_rt_ext_pending_message, pycc_rt_ext_pending_type, pycc_rt_name_error,
};

/// The pending exception's message bytes.
fn pending_message() -> String {
    let mut len = 0usize;
    let bytes = unsafe { pycc_rt_ext_pending_message(&mut len) };
    assert!(!bytes.is_null());
    String::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()).unwrap()
}

#[test]
fn an_unbound_name_raises_a_pending_exception_and_returns() {
    pycc_rt_exception_clear();
    // Codegen passes the NUL-terminated `fnname_` constant with the
    // terminator excluded from the length.
    let name = b"late\0";
    unsafe { pycc_rt_name_error(name.as_ptr(), name.len() - 1) };
    assert_ne!(pycc_rt_exception_active(), 0);
    assert_eq!(
        pycc_rt_ext_pending_type(),
        i32::from(EXCEPTION_TYPE_EXCEPTION)
    );
    assert_eq!(pending_message(), "name 'late' is not defined");
    pycc_rt_exception_clear();
    assert_eq!(pycc_rt_exception_active(), 0);
}
