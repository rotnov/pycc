use std::path::{Path, PathBuf};
use std::process::Command;

fn pycc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// The pinned CPython 3.14.6 oracle (D-001's "python3.14" pin, upgraded to
/// 3.14.6 per this PR's own Task 1). A missing or wrong-version oracle is a
/// clean, actionable panic, not a silently-skipped or falsely-passing check.
fn oracle_python_bin() -> PathBuf {
    let bin = PathBuf::from("python3.14");
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

/// Builds `py_path` with `pycc build --debug` (the default profile), runs
/// the resulting binary, separately runs the pinned CPython oracle on the
/// identical source, and returns both stdouts for the caller to diff.
fn run_conformance_fixture(label: &str, py_path: &Path) -> (Vec<u8>, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("pycc_conformance_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join(label);
    let status = Command::new(pycc_bin())
        .args(["build", py_path.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "`pycc build` failed for {label}");
    let pycc_output = Command::new(&out).output().unwrap();
    assert!(pycc_output.status.success(), "compiled {label} binary exited non-zero");

    let cpython_output = Command::new(oracle_python_bin())
        .arg(py_path)
        .output()
        .unwrap();
    assert!(cpython_output.status.success(), "CPython oracle exited non-zero for {label}");

    (pycc_output.stdout, cpython_output.stdout)
}

#[test]
fn fib_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance_fib.py");
    let (pycc_stdout, cpython_stdout) = run_conformance_fixture("conformance_fib", &fixture);
    assert_eq!(
        pycc_stdout, cpython_stdout,
        "pycc and CPython 3.14.6 disagree on tests/fixtures/conformance_fib.py"
    );
}
