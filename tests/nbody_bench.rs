use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const RUNS: usize = 5;

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("timings are never NaN"));
    values[values.len() / 2]
}

fn time_command(mut command: Command) -> f64 {
    let start = Instant::now();
    let status = command.status().expect("command must spawn");
    let elapsed = start.elapsed().as_secs_f64();
    assert!(status.success(), "command failed: {command:?}");
    elapsed
}

/// The pinned CPython 3.14.6 oracle (D-001's "python3.14" pin). Duplicated
/// from `tests/conformance.rs` rather than shared through a common module --
/// this repository's existing integration-test convention (e.g.
/// `tests/pycc_toml_release_default.rs`'s own private `pycc_bin()`) is each
/// `tests/*.rs` file carrying its own small self-contained helpers, since
/// Rust integration-test binaries don't share code across files without a
/// `tests/common/mod.rs`-style module none of these files currently use.
///
/// Windows needs its own file name here: `std::process::Command::new` on
/// Windows resolves a bare program name by searching `PATH` for that exact
/// file name and does not append `.exe` when the name already contains a
/// `.` (the version dot in "python3.14" reads as an extension to that
/// resolver). A CI-side `python3.14.exe` alias on `PATH` is therefore
/// invisible to this lookup even though shells like bash find it fine.
/// Passing the extension explicitly on Windows sidesteps the mismatch,
/// exactly like `tests/conformance.rs::oracle_binary_name` (D-080 addendum).
fn oracle_binary_name(is_windows: bool) -> &'static str {
    if is_windows { "python3.14.exe" } else { "python3.14" }
}

fn oracle_python_bin() -> PathBuf {
    let bin = PathBuf::from(oracle_binary_name(cfg!(windows)));
    let output = Command::new(&bin)
        .arg("--version")
        .output()
        .unwrap_or_else(|e| panic!("conformance oracle `python3.14` not found on PATH: {e}"));
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        version.trim() == "Python 3.14.6",
        "conformance oracle must be exactly Python 3.14.6, found {version:?}"
    );
    bin
}

/// D-090's nbody measurement contract (design doc's own §1): same-machine
/// paired comparison, `K = 5` runs each, ratio of medians, `--release` pycc
/// vs. the pinned CPython 3.14.6 oracle, gate at ratio >= 20. `#[ignore]`d
/// like `tests/conformance.rs`'s two fixtures -- genuinely slow (a full
/// `--release` LLVM build plus ten total program executions) -- and run
/// explicitly via `--include-ignored`, already passed workspace-wide in
/// both `build-test-coverage` and every `native-build-test` matrix leg
/// (`.github/workflows/ci.yml`), so no CI change was needed to wire this in.
///
/// As of this commit this test fails: measured ratio is ~10-11x, not >= 20x.
/// This is not a case of `--release` "not taking effect" in the trivial
/// sense -- `pycc_codegen`'s `release_mode_actually_runs_llvm_optimization_
/// passes` unit test still passes, confirming the O3 pipeline measurably
/// changes the emitted object code for the compiled module itself. Two
/// compounding, real causes, verified empirically (see this PR's Task 5
/// report for the full measurements):
/// 1. `src/main.rs::find_pycc_rt_lib_dir_in` always links
///    `target/debug/libpycc_rt.a` (or the `--target`-qualified equivalent),
///    regardless of `--release` -- the flag only optimizes the compiled
///    module's own LLVM IR, never selects an optimized `pycc_rt` build.
///    Every `pycc_rt_float_pow` call this fixture's ten unrolled pairwise
///    updates make per iteration (200,000 total) therefore runs through an
///    unoptimized runtime. Linking a `--release`-built `pycc_rt` instead
///    (verified locally by temporarily hand-editing the lookup path, not
///    committed) drops the fixture's own median from ~6.5ms to ~5.5ms --
///    a real but insufficient-alone improvement.
/// 2. v0.2 has no cross-module LTO (D-090): `pycc_rt_float_pow` is an
///    opaque `extern "C"` call from the compiled module's perspective, so
///    LLVM can never inline the domain-check-then-`powf` body into the
///    hot loop no matter which `pycc_rt` build is linked.
///
/// Both are real, structural, out of this test's own scope to fix --
/// changing `--release`'s runtime-library selection or pursuing real
/// cross-module optimization are follow-up implementation work, not a
/// benchmark-harness change. The gate itself is a design-doc-mandated
/// threshold (design doc's §1) and stays at 20 here unmodified; lowering
/// it to make this test pass would defeat the point of building the
/// measurement in the first place.
#[test]
#[ignore] // slow: builds a --release binary and runs both programs 5 times each
fn nbody_release_binary_is_at_least_20x_faster_than_cpython() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nbody.py");

    // Build the pycc binary once, --release, outside the timed loop.
    let bin_path = std::env::temp_dir().join(format!("pycc_nbody_{}", std::process::id()));
    let build_status = Command::new(env!("CARGO_BIN_EXE_pycc"))
        .arg("build")
        .arg(&fixture)
        .arg("-o")
        .arg(&bin_path)
        .arg("--release")
        .status()
        .expect("pycc build must spawn");
    assert!(build_status.success(), "pycc --release build of nbody.py failed");

    let pycc_times: Vec<f64> = (0..RUNS)
        .map(|_| time_command(Command::new(&bin_path)))
        .collect();
    let cpython_times: Vec<f64> = (0..RUNS)
        .map(|_| {
            let mut cpython = Command::new(oracle_python_bin());
            cpython.arg(&fixture);
            time_command(cpython)
        })
        .collect();

    let pycc_median = median(pycc_times);
    let cpython_median = median(cpython_times);
    let ratio = cpython_median / pycc_median;

    assert!(
        ratio >= 20.0,
        "nbody speedup ratio {ratio:.2}x is below the required 20x gate \
         (cpython median {cpython_median:.4}s, pycc --release median {pycc_median:.4}s)"
    );
}
