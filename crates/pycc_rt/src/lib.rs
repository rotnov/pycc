//! # Build order: this crate's staticlib is not a normal Cargo dependency
//!
//! This crate's real consumer is not Rust code but pycc-generated object
//! files, which reference `pycc_rt_print_i64` by symbol name and are
//! linked against `libpycc_rt.a` (this crate's `staticlib` output) via a
//! raw `cc` invocation -- in `pycc_codegen`'s own tests
//! (`link_object_with_runtime`) and in `pycc`'s real `build`/`run`
//! (`src/main.rs`). Nothing in Cargo's normal dependency graph expresses
//! that relationship, so `libpycc_rt.a` only exists once this crate has
//! actually been built.
//!
//! **Practical consequence:** commands scoped to a single other crate --
//! `cargo test -p pycc_codegen`, `cargo run --bin pycc -- build ...` run in
//! isolation -- need this crate built first: `cargo build -p pycc_rt`.
//! `cargo build --workspace` / `cargo test --workspace` (what CI always
//! runs) builds every workspace member including this one, so the ordering
//! issue never surfaces there.
//!
//! A `build.rs` in the consuming crates that shells out to `cargo build -p
//! pycc_rt` was tried and reverted: pointed at the same `target-dir` as the
//! outer build, it deadlocks on Cargo's own build lock (the outer build
//! holds it for the whole build-script execution; the nested `cargo build`
//! blocks forever waiting for it). Fixing this for real needs either a
//! separate target-dir for the nested build or embedding `pycc_rt` into the
//! `pycc` binary directly instead of linking a sibling staticlib -- both
//! bigger changes than this sharp edge currently justifies.

fn format_i64_line(value: i64) -> String {
    format!("{value}\n")
}

/// # Safety
/// Called only from pycc-generated code with a plain i64 argument; no
/// pointers involved, so there is nothing for the caller to uphold beyond
/// standard `extern "C"` calling-convention correctness.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_i64(value: i64) {
    print!("{}", format_i64_line(value));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_i64_matches_cpython_format() {
        // Confirmed against `python3.14 -c "print(42)"` / `print(-7)`:
        // exactly the digits, then a single trailing newline, nothing else.
        assert_eq!(format_i64_line(42), "42\n");
        assert_eq!(format_i64_line(-7), "-7\n");
    }

    #[test]
    fn extern_c_entry_point_runs_for_positive_negative_and_zero() {
        // This crate's staticlib output is linked into pycc-compiled
        // binaries by a separate `cc` step (Task 8), which cargo-llvm-cov
        // never instruments -- so this is the only place
        // pycc_rt_print_i64 itself (not just its formatting helper) is
        // exercised for the D-014 gate. stdout is captured by the test
        // harness and only shown on failure.
        pycc_rt_print_i64(42);
        pycc_rt_print_i64(-7);
        pycc_rt_print_i64(0);
    }
}
