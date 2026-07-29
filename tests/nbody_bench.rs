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
        .unwrap_or_else(|e| panic!("nbody benchmark oracle `python3.14` not found on PATH: {e}"));
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        version.trim() == "Python 3.14.6",
        "nbody benchmark oracle must be exactly Python 3.14.6, found {version:?}"
    );
    bin
}

#[test]
fn median_returns_the_middle_of_five_sorted_values() {
    assert_eq!(median(vec![3.0, 1.0, 2.0, 5.0, 4.0]), 3.0);
}

#[test]
fn oracle_binary_name_appends_the_exe_extension_only_for_windows() {
    assert_eq!(oracle_binary_name(true), "python3.14.exe");
    assert_eq!(oracle_binary_name(false), "python3.14");
}

/// D-094's nbody measurement contract (design doc's own §1): same-machine
/// paired comparison, `K = 5` runs each, ratio of medians, `--release` pycc
/// vs. the pinned CPython 3.14.6 oracle, gate at ratio >= 20. `#[ignore]`d
/// like `tests/conformance.rs`'s two fixtures -- genuinely slow (a full
/// `--release` LLVM build plus ten total program executions) -- and run
/// explicitly via `--include-ignored`, already passed workspace-wide in
/// both `build-test-coverage` and every `native-build-test` matrix leg
/// (`.github/workflows/ci.yml`), so no further CI test-wiring change was
/// needed beyond D-092's own release-`pycc_rt`-build step addition there.
///
/// As of this commit this test still fails, but for a narrower, better
/// understood reason than the ~10-11x this benchmark first measured
/// (D-093): that first measurement conflated a real methodology gap with
/// what turned out to be a real, separate implementation bug, both now
/// addressed:
/// 1. `src/main.rs::find_pycc_rt_lib_dir_in` used to always link
///    `target/debug/libpycc_rt.a` regardless of `--release` (the flag only
///    optimized the compiled module's own LLVM IR, never selected an
///    optimized `pycc_rt` to link) -- fixed (D-092): it now takes a
///    `release: bool` and links `target/release` when the caller's already-
///    resolved `--release` state says to.
/// 2. At `DEFAULT_ITERATIONS = 20000` (pyperformance's own upstream
///    constant), pycc's own ~3ms fixed process-spawn overhead was ~45-50%
///    of its ~6ms total nbody runtime, mechanically compressing the
///    measured ratio far below the actual compute-only speedup -- fixed
///    (D-093): `tests/fixtures/nbody.py`'s iteration count is raised to
///    `525000`, keeping both sides' own fixed-overhead fraction in the
///    single digits (pycc ~4.3%, CPython ~1.6%) without changing any
///    physics, constant, or update-order fidelity to the reference
///    benchmark. This fixture's own 10 pairwise `pycc_rt_float_pow` calls
///    per iteration now total 5,250,000 over a full run, not 200,000.
///
/// With both fixes in place, the measured ratio is a stable, reproducible
/// ~18.0-18.24x (see D-093 for five consecutive runs' worth of numbers) --
/// still short of the 20x gate, but now a genuine, well-bounded compute
/// ceiling rather than a measurement artifact: real timing at 300k/400k/
/// 525k/800k/1M iterations shows the ratio is not still climbing (17.34x/
/// 17.72x/17.78x/18.03x/18.14x), so raising the iteration count further
/// would not close this gap. `pycc_rt_float_pow` remains an opaque
/// `extern "C"` call from the compiled module's own LLVM IR (v0.2 has no
/// cross-module LTO, D-094), so LLVM can never inline it into the hot loop
/// regardless of which `pycc_rt` build is linked -- closing the remaining
/// ~2x gap would need real cross-module optimization work, out of this
/// test's own scope. The gate itself is a design-doc-mandated threshold
/// (design doc's §1) and stays at 20 here unmodified; lowering it, or
/// rewriting this fixture's computation to dodge `pycc_rt_float_pow` calls,
/// would defeat the point of building this measurement in the first place.
/// See D-093 for the full investigation and the task dispatcher's own
/// decision on how to proceed.
///
/// Runs execute in two back-to-back blocks (all 5 pycc runs, then all 5
/// CPython runs) rather than interleaved -- matching the design doc's own
/// "both programs run K = 5 times; take the median of each" wording, which
/// specifies K and the aggregation but not an interleaving requirement.
/// Block ordering is slightly more exposed to monotonic drift (thermal
/// throttling, background-process ramp-up) than interleaving would be,
/// since drift penalizes whichever side runs second rather than being
/// averaged across both; taking the median (not the mean) of 5 same-block
/// runs already blunts most of that exposure.
#[test]
#[ignore = "slow: builds a --release binary and runs both programs 5 times each"]
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

    // Resolved once, not per run: `oracle_python_bin()` itself spawns a
    // `--version` check, and re-running that inside the loop below would
    // burn four redundant process spawns per test run for no benefit (the
    // oracle binary and its version can't change mid-test).
    let cpython_bin = oracle_python_bin();

    let pycc_times: Vec<f64> = (0..RUNS)
        .map(|_| time_command(Command::new(&bin_path)))
        .collect();
    let cpython_times: Vec<f64> = (0..RUNS)
        .map(|_| {
            let mut cpython = Command::new(&cpython_bin);
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
