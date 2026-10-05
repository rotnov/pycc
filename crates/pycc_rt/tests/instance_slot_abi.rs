//! #1388: the instance-slot entry points generated code calls --
//! `pycc_rt_instance_new` with its layout descriptor and the checked read
//! `pycc_rt_instance_get_slot_checked` -- exercised through the `rlib`.
//!
//! `crates/pycc_rt/src/instance.rs`'s unit tests own the semantics; this
//! file pins them from outside the crate, which is also where the
//! diff-coverage gate measures these exported functions (`docs/TESTING.md`,
//! "A runtime function an integration test links is measured from that
//! binary").

use pycc_rt::{
    EXCEPTION_TYPE_ATTRIBUTE_ERROR, pycc_rt_exception_active, pycc_rt_exception_clear,
    pycc_rt_ext_pending_message, pycc_rt_ext_pending_type, pycc_rt_instance_get_slot,
    pycc_rt_instance_get_slot_checked, pycc_rt_instance_new, pycc_rt_instance_set_slot,
};

/// The pending exception's tag and message; clears it afterwards so the
/// thread-local state never leaks into the next test on this thread.
fn take_pending() -> (i32, String) {
    assert_eq!(pycc_rt_exception_active(), 1, "an exception is pending");
    let tag = pycc_rt_ext_pending_type();
    let mut len = 0usize;
    let bytes = unsafe { pycc_rt_ext_pending_message(&mut len) };
    assert!(!bytes.is_null());
    let message = String::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec())
        .expect("the message is UTF-8");
    pycc_rt_exception_clear();
    (tag, message)
}

#[test]
fn a_checked_read_names_the_class_and_slot_from_the_descriptor() {
    static LAYOUT: &[u8] = b"Point\0x\0y";
    pycc_rt_exception_clear();
    let point = unsafe { pycc_rt_instance_new(2, LAYOUT.as_ptr(), LAYOUT.len()) };
    unsafe { pycc_rt_instance_set_slot(point, 0, 7) };
    assert_eq!(unsafe { pycc_rt_instance_get_slot_checked(point, 0) }, 7);
    assert_eq!(pycc_rt_exception_active(), 0);
    // Slot 1 is unassigned: the checked read raises and returns `0`.
    assert_eq!(unsafe { pycc_rt_instance_get_slot_checked(point, 1) }, 0);
    let (tag, message) = take_pending();
    assert_eq!(tag, i32::from(EXCEPTION_TYPE_ATTRIBUTE_ERROR));
    assert_eq!(message, "'Point' object has no attribute 'y'");
    // The unchecked read releases-before-store relies on stays silent.
    assert_eq!(unsafe { pycc_rt_instance_get_slot(point, 1) }, 0);
    assert_eq!(pycc_rt_exception_active(), 0);
}

#[test]
fn an_assigned_zero_word_reads_without_raising() {
    static LAYOUT: &[u8] = b"Flag\0on";
    pycc_rt_exception_clear();
    let flag = unsafe { pycc_rt_instance_new(1, LAYOUT.as_ptr(), LAYOUT.len()) };
    unsafe { pycc_rt_instance_set_slot(flag, 0, 0) };
    assert_eq!(unsafe { pycc_rt_instance_get_slot_checked(flag, 0) }, 0);
    assert_eq!(pycc_rt_exception_active(), 0);
}

#[test]
fn a_null_descriptor_still_raises_with_placeholder_names() {
    pycc_rt_exception_clear();
    let bare = unsafe { pycc_rt_instance_new(1, std::ptr::null(), 0) };
    assert_eq!(unsafe { pycc_rt_instance_get_slot_checked(bare, 0) }, 0);
    let (tag, message) = take_pending();
    assert_eq!(tag, i32::from(EXCEPTION_TYPE_ATTRIBUTE_ERROR));
    assert_eq!(message, "'?' object has no attribute '?'");
}
