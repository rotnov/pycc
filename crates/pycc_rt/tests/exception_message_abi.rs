//! #1511: a bridged exception's lazily produced message, exercised through
//! the `rlib` the way the `ext` C shim drives it -- allocate without a
//! message, register a resolver, read the message only when rendered.
//!
//! `crates/pycc_rt/src/exception/message.rs`'s unit tests own the
//! semantics; this file pins them from outside the crate, which is also
//! where the diff-coverage gate measures these exported functions
//! (`docs/TESTING.md`, "A runtime function an integration test links is
//! measured from that binary"). The resolver registration is process-wide,
//! so every check lives in one test.

use std::sync::atomic::{AtomicUsize, Ordering};

use pycc_rt::{
    EXCEPTION_TYPE_EXCEPTION, EXCEPTION_TYPE_TYPE_ERROR, EXCEPTION_TYPE_VALUE_ERROR,
    PyExceptionObj, PyStrObj, pycc_rt_exception_alloc, pycc_rt_exception_clear,
    pycc_rt_exception_group_partition, pycc_rt_exception_message, pycc_rt_exception_raise,
    pycc_rt_exception_set_message_resolver, pycc_rt_ext_pending_message, pycc_rt_str_from_literal,
};

static CALLS: AtomicUsize = AtomicUsize::new(0);

/// Counts its calls and answers with a fixed message.
unsafe extern "C" fn resolver(_obj: *mut PyExceptionObj) -> *mut PyStrObj {
    CALLS.fetch_add(1, Ordering::SeqCst);
    let text = b"resolved";
    unsafe { pycc_rt_str_from_literal(text.as_ptr(), text.len() as i64) }
}

fn lazy_value_error() -> *mut PyExceptionObj {
    let name = "ValueError";
    pycc_rt_exception_alloc(
        EXCEPTION_TYPE_VALUE_ERROR,
        name.as_ptr(),
        name.len(),
        std::ptr::null_mut(),
    )
}

/// Raises `obj`, reads the pending message's bytes, and clears it again.
fn pending_text(obj: *mut PyExceptionObj) -> Vec<u8> {
    pycc_rt_exception_raise(obj);
    let mut len = 0usize;
    let bytes = unsafe { pycc_rt_ext_pending_message(&mut len) };
    assert!(!bytes.is_null());
    let text = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();
    pycc_rt_exception_clear();
    text
}

#[test]
fn a_bridged_message_is_produced_only_when_rendered() {
    pycc_rt_exception_set_message_resolver(Some(resolver));

    // Nothing pending reads as no message.
    pycc_rt_exception_clear();
    let mut len = usize::MAX;
    assert!(unsafe { pycc_rt_ext_pending_message(&mut len) }.is_null());
    assert_eq!(len, 0);

    // Raising and partitioning never resolve the message.
    let obj = lazy_value_error();
    pycc_rt_exception_raise(obj);
    pycc_rt_exception_clear();
    let tags = [EXCEPTION_TYPE_TYPE_ERROR];
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
    assert!(matched.is_null());
    assert!(!rest.is_null());
    assert_eq!(CALLS.load(Ordering::SeqCst), 0);

    // The first render resolves it, once; later reads reuse the answer.
    let first = unsafe { pycc_rt_exception_message(obj) };
    assert_eq!(first, unsafe { pycc_rt_exception_message(obj) });
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(pending_text(obj), b"resolved");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);

    // A matched group carries CPython's `''` wrapper message, unresolved.
    let other = lazy_value_error();
    let match_tags = [EXCEPTION_TYPE_VALUE_ERROR];
    unsafe {
        pycc_rt_exception_group_partition(
            other,
            match_tags.as_ptr(),
            match_tags.len(),
            EXCEPTION_TYPE_EXCEPTION,
            group_name.as_ptr(),
            group_name.len(),
            &raw mut matched,
            &raw mut rest,
        );
    }
    assert!(rest.is_null());
    assert_eq!(pending_text(matched), b"");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);

    pycc_rt_exception_set_message_resolver(None);
}
