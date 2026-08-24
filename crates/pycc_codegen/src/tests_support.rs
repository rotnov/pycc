//! Shared test-only helper extracted out of `tests.rs` (AGENTS.md "Keep
//! source files decomposable": `tests.rs` runs to 11,000+ lines, well past
//! the ~1,000-line threshold, so a cohesive standalone piece touched by a
//! change to that file gets its own submodule rather than growing the
//! original further). This module is `tests::support`, declared with
//! `#[path]` from `tests.rs`; `tempfile_dir` is re-exported there as
//! `pub(crate) use support::tempfile_dir` so existing call sites --
//! `crate::tests::tempfile_dir` in `bigint_rc.rs`'s own test module, and the
//! ~260 unqualified call sites inside `tests.rs` itself -- keep resolving
//! unchanged.

/// RAII handle for a per-test scratch directory under the OS temp dir.
/// Derefs to `Path` so existing call sites (`dir.join(...)`, `&dir` passed
/// where `&Path` is expected) keep working unchanged; `Drop` removes the
/// directory tree when the handle goes out of scope, including on an early
/// return via a failed `assert!`/`.expect()` panic, which a plain
/// `PathBuf` return plus a manual `std::fs::remove_dir_all` call at the end
/// of each test could not guarantee -- a panicking assertion partway
/// through a test skipped that manual cleanup and left the directory behind
/// in `$TMPDIR`.
pub(crate) struct TempTestDir(std::path::PathBuf);

impl std::ops::Deref for TempTestDir {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn tempfile_dir(label: &str) -> TempTestDir {
    let dir =
        std::env::temp_dir().join(format!("pycc_codegen_test_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    TempTestDir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic;

    #[test]
    fn the_directory_exists_while_the_handle_is_alive_and_is_gone_after_a_normal_drop() {
        let dir = tempfile_dir("support_normal_drop");
        let path = dir.to_path_buf();
        assert!(path.is_dir(), "tempfile_dir must create the directory");
        drop(dir);
        assert!(
            !path.exists(),
            "Drop must remove the scratch directory on normal scope exit"
        );
    }

    #[test]
    fn the_directory_is_removed_even_when_a_panic_unwinds_through_the_handle() {
        let path = {
            let dir = tempfile_dir("support_panic_unwind");
            let path = dir.to_path_buf();
            assert!(path.is_dir(), "tempfile_dir must create the directory");
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                let _keep_dir_alive = &dir;
                panic!("simulated test failure while the scratch directory is still live");
            }));
            assert!(result.is_err(), "the inner closure must have panicked");
            path
        };
        assert!(
            !path.exists(),
            "Drop must remove the scratch directory even when unwinding through a panic"
        );
    }

    #[test]
    fn deref_reaches_path_methods_directly() {
        let dir = tempfile_dir("support_deref");
        // `join` is a `Path` method reached only through `Deref`; a
        // regression that dropped the `Deref` impl (or narrowed its
        // target) would fail to compile here.
        let child = dir.join("child.txt");
        assert!(child.starts_with(&*dir));
    }
}
