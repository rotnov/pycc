//! Runtime `NameError` for a call that reaches a function-pointer slot
//! whose `def` has not executed yet (issue #22).
//!
//! pycc's type checker rejects a call to a not-yet-`def`ined function in
//! top-level code statically, but a function body may call a sibling -- or
//! construct a class -- whose definition has not executed at the time the
//! caller runs. CPython raises `NameError: name '<name>' is not defined`
//! there, and so does this.
//!
//! Since #1490 the error is an ordinary pending pycc exception rather than a
//! panic: a panic cannot unwind past the `extern "C"` boundary, so under
//! `--ext` it aborted the host interpreter instead of raising. pycc models no
//! `NameError` class (`except NameError` is `T0021`), so the exception
//! carries `Exception`'s tag -- `except Exception` catches it, as in CPython
//! -- and the class name `NameError`, which is what an uncaught one prints.
//!
//! This is the native half. A module compiled for a CPython host (an `--ext`
//! artifact or an embedded executable) never calls it: codegen calls the C
//! shim's `pycc_ext_name_error` instead, which raises CPython's own
//! `NameError` and bridges it, so the host sees the real class
//! (`ext_thunk::emit_name_error_raise` in `pycc_codegen`).

use crate::exception::{EXCEPTION_TYPE_EXCEPTION, raise_builtin};

/// Formats the message and raises it. `name` is `len` bytes of UTF-8 with no
/// terminator; a null `name` reads as `<unknown>`.
fn name_error(name: *const u8, len: usize) {
    let name_str = if name.is_null() {
        std::borrow::Cow::Borrowed("<unknown>")
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, len) })
    };
    raise_builtin(
        EXCEPTION_TYPE_EXCEPTION,
        "NameError",
        &format!("name '{name_str}' is not defined"),
    );
}

/// Sets a pending `NameError` for `name` and returns; the caller branches to
/// its innermost exception target.
///
/// # Safety
///
/// A non-null `name` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_name_error(name: *const u8, len: usize) {
    name_error(name, len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exception::{pycc_rt_exception_clear, pycc_rt_exception_value};
    use crate::tests::pending_tag_and_message;

    fn pending_class_name() -> String {
        let obj = pycc_rt_exception_value();
        let (name, len) = unsafe { ((*obj).name, (*obj).name_len) };
        String::from_utf8(unsafe { std::slice::from_raw_parts(name, len) }.to_vec()).unwrap()
    }

    #[test]
    fn name_error_raises_a_pending_exception_naming_the_function() {
        pycc_rt_exception_clear();
        let name = b"foo";
        unsafe { pycc_rt_name_error(name.as_ptr(), name.len()) };
        let (tag, message) = pending_tag_and_message();
        assert_eq!(tag, EXCEPTION_TYPE_EXCEPTION);
        assert_eq!(message, "name 'foo' is not defined");
        assert_eq!(pending_class_name(), "NameError");
        pycc_rt_exception_clear();
    }

    #[test]
    fn name_error_reads_only_len_bytes() {
        // Codegen passes a NUL-terminated constant and its length without
        // the terminator; a dotted method name is passed whole.
        pycc_rt_exception_clear();
        let name = b"C.__init__\0";
        unsafe { pycc_rt_name_error(name.as_ptr(), name.len() - 1) };
        let (_, message) = pending_tag_and_message();
        assert_eq!(message, "name 'C.__init__' is not defined");
        pycc_rt_exception_clear();
    }

    #[test]
    fn name_error_on_null_pointer_uses_unknown() {
        pycc_rt_exception_clear();
        name_error(std::ptr::null(), 0);
        let (_, message) = pending_tag_and_message();
        assert_eq!(message, "name '<unknown>' is not defined");
        pycc_rt_exception_clear();
    }
}
