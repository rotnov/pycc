//! Shared RAII scratch-directory abstraction for pycc's production and test
//! code (issue #781, Part 1 of #779's 5-part decomposition).
//!
//! #779 found that ad hoc `std::env::temp_dir().join(...)` call sites across
//! ~36 test files (plus two production sites in `src/main.rs`, fixed
//! separately in Part 3) leaked directories whenever a test panicked before
//! its own manual `remove_dir_all` cleanup ran, and could collide when two
//! call sites in the same process picked the same name. [`ScratchDir`] fixes
//! both: `Drop`-based cleanup that runs even during a panic unwind, and a
//! naming scheme that cannot collide within a process and is vanishingly
//! unlikely to collide across processes.
//!
//! This crate intentionally has no dependencies beyond `std`. See the
//! decision record for why `tempfile`/`rand` were not added instead.

use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-process counter that disambiguates two [`ScratchDir::new`] calls that
/// land on the same nanosecond tick (possible on platforms where
/// `SystemTime`'s resolution is coarser than a nanosecond, and routine when
/// several threads race to create a scratch directory at once — exactly the
/// scenario the parallel-creation regression test below exercises).
static SEQ: AtomicU64 = AtomicU64::new(0);

/// RAII handle for a uniquely-named, pycc-owned scratch directory created
/// under the OS temp directory (`std::env::temp_dir()`).
///
/// Derefs to [`Path`], so it can be used anywhere a `&Path` is expected
/// (`scratch_dir.join("out.o")`, passed by reference, etc.) without first
/// unwrapping to the inner `PathBuf`.
///
/// `Drop` removes the directory tree, including while a panic is unwinding
/// through the stack frame that owns the handle — `std::fs::remove_dir_all`
/// is called unconditionally from `drop`, which Rust runs on every unwind
/// path, not only on ordinary scope exit.
///
/// # Naming and stability
///
/// Every directory this type creates is named
/// `pycc_{category}_{pid}_{nanos}_{seq}` under `std::env::temp_dir()`, where:
///
/// - `pid` is `std::process::id()`,
/// - `nanos` is the **full epoch nanosecond count** (a `u128` from
///   `SystemTime::now().duration_since(UNIX_EPOCH)`'s `as_nanos()`), not just
///   the sub-second remainder — the field must span real time so that two
///   process restarts a few seconds apart that happen to reuse the same PID
///   still get distinguishable names. A wall clock set before the Unix
///   epoch degrades this field to `0` rather than panicking — `build`/`run`
///   promise an actionable exit-2 environment error, never a panic, and a
///   real collision under a degraded clock still surfaces as
///   `create_dir`'s `AlreadyExists` error through the fallible path below,
/// - `seq` is [`SEQ`], a per-process atomic counter that rules out any
///   same-process collision even when two calls land on the same nanosecond.
///
/// This field order and format are a stability commitment: other tooling
/// (e.g. a later stale-scratch-root sweep) may need to parse a directory
/// name back into its `pid`/`nanos` fields without a live handle. Changing
/// the format is a breaking change for any such consumer.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Creates a fresh, guaranteed-unique directory named
    /// `pycc_{category}_{pid}_{nanos}_{seq}` under `std::env::temp_dir()`
    /// and returns a handle that removes it on `Drop`.
    ///
    /// `category` is a short, caller-supplied label describing what the
    /// scratch directory is for (e.g. `"codegen_test"`, `"obj"`, `"run"`).
    /// It must be a short, static-ish string literal chosen by code inside
    /// this repository — never derived from external, user-, or
    /// file-supplied input — since it is spliced directly into the
    /// directory name without validation against the fixed-field format
    /// above. This is a caller discipline, not an enforced invariant: no
    /// workspace crate has a caller that needs to pass anything else, so a
    /// validation branch here would be dead code under the project's 100%
    /// coverage gate.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] if `std::fs::create_dir` fails —
    /// for example, a full disk, a permissions failure, or (deliberately
    /// unsupported input) a `category` containing a byte that is invalid in
    /// a path component on the current platform, such as a NUL byte on
    /// every platform this project targets. There is no retry: `create_dir`
    /// already fails atomically if the path already exists, and the
    /// `pid`/`nanos`/`seq` combination makes a same-process collision
    /// impossible and a cross-process collision unreachable in practice, so
    /// there is no genuine collision case for a retry loop to handle.
    pub fn new(category: &str) -> io::Result<Self> {
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);

        let path = std::env::temp_dir().join(format!("pycc_{category}_{pid}_{nanos}_{seq}"));
        std::fs::create_dir(&path)?;
        Ok(ScratchDir { path })
    }
}

impl Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn the_directory_exists_while_the_handle_is_alive_and_is_gone_after_a_normal_drop() {
        let dir = ScratchDir::new("lib_normal_drop").expect("ScratchDir::new should succeed");
        let path = dir.to_path_buf();
        assert!(path.is_dir(), "ScratchDir::new must create the directory");
        drop(dir);
        assert!(
            !path.exists(),
            "Drop must remove the scratch directory on normal scope exit"
        );
    }

    #[test]
    fn the_directory_is_removed_even_when_a_panic_unwinds_through_the_handle() {
        let dir = ScratchDir::new("lib_panic_unwind").expect("ScratchDir::new should succeed");
        let path = dir.to_path_buf();
        assert!(path.is_dir(), "ScratchDir::new must create the directory");
        // `dir` is moved into the closure by value (not merely referenced),
        // so its `Drop` genuinely runs while the panic is unwinding through
        // this stack frame -- proving cleanup happens on the unwind path
        // itself, not just via ordinary end-of-scope drop after
        // `catch_unwind` returns.
        let result = panic::catch_unwind(panic::AssertUnwindSafe(move || {
            let _dir = dir;
            panic!("simulated test failure while the scratch directory is still live");
        }));
        assert!(result.is_err(), "the inner closure must have panicked");
        assert!(
            !path.exists(),
            "Drop must remove the scratch directory even when unwinding through a panic"
        );
    }

    #[test]
    fn concurrent_creation_with_the_same_category_from_many_threads_never_collides() {
        // 32 threads race to call `ScratchDir::new("lib_parallel")` at
        // (as close as possible to) the same instant, using a `Barrier` to
        // force the race. The `seq` counter -- not exact nanosecond timing
        // -- is what actually guarantees uniqueness under concurrency, so
        // this assertion holds regardless of how tightly the threads
        // actually synchronize. This is the scenario that the previous
        // PID-only naming scheme (`pycc_codegen_test_{label}_{pid}`, no
        // counter, no timestamp) got wrong: two calls with the same label
        // in the same process shared one PID and therefore one name.
        const THREADS: usize = 32;
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    ScratchDir::new("lib_parallel").expect("ScratchDir::new should not collide")
                })
            })
            .collect();

        let dirs: Vec<ScratchDir> = handles
            .into_iter()
            .map(|h| h.join().expect("worker thread should not panic"))
            .collect();

        let mut paths: Vec<PathBuf> = dirs.iter().map(|d| d.to_path_buf()).collect();
        for path in &paths {
            assert!(path.is_dir(), "every created scratch directory must exist");
        }
        paths.sort();
        paths.dedup();
        assert_eq!(
            paths.len(),
            THREADS,
            "all {THREADS} concurrently created directories must be distinct"
        );

        let all_paths: Vec<PathBuf> = dirs.iter().map(|d| d.to_path_buf()).collect();
        drop(dirs);
        for path in &all_paths {
            assert!(
                !path.exists(),
                "Drop must remove every concurrently created scratch directory"
            );
        }
    }

    #[test]
    fn a_category_that_produces_an_invalid_path_component_propagates_the_create_dir_error() {
        // A NUL byte is invalid in a path component on every platform this
        // repo targets, so `std::fs::create_dir` fails predictably and
        // portably here, giving `ScratchDir::new` a deterministic `Err` to
        // propagate without needing filesystem permission tricks or a
        // raced collision.
        let category = "bad\0category";
        let result = ScratchDir::new(category);
        assert!(
            result.is_err(),
            "a NUL byte in the category should make create_dir fail"
        );

        // No directory should have been left behind under any name derived
        // from this category -- `create_dir` failing means nothing was
        // created at all, but assert the specific prefix is absent too, so
        // a future implementation change that silently swallowed the error
        // and created a sanitized directory anyway would also be caught.
        //
        // A sentinel entry keeps the `read_dir` scan below from ever running
        // over zero entries: under a freshly isolated `TMPDIR` (e.g. this
        // repository's sandboxed CI coverage job), `std::env::temp_dir()` can
        // otherwise be empty at this point, which would make the closures
        // below never execute and vacuously pass without ever comparing a
        // real entry against `prefix`.
        let _sentinel = ScratchDir::new("lib_scan_sentinel").expect("sentinel scratch dir");
        let temp_dir = std::env::temp_dir();
        let prefix = format!("pycc_{category}_");
        let leaked = std::fs::read_dir(&temp_dir)
            .expect("temp dir should be readable")
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix));
        assert!(
            !leaked,
            "a failed ScratchDir::new must not leave a directory behind"
        );
    }
}
