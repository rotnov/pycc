use std::path::{Path, PathBuf};
use std::process::Command;

fn pycc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// The pinned CPython 3.14.6 oracle (D-001's "python3.14" pin, upgraded to
/// 3.14.6 per this PR's own Task 1). A missing or wrong-version oracle is a
/// clean, actionable panic, not a silently-skipped or falsely-passing check.
///
/// Windows needs its own file name here: `std::process::Command::new` on
/// Windows resolves a bare program name by searching `PATH` for that exact
/// file name and does not append `.exe` when the name already contains a
/// `.` (the version dot in "python3.14" reads as an extension to that
/// resolver). A CI-side `python3.14.exe` alias on `PATH` is therefore
/// invisible to this lookup even though shells like bash find it fine
/// (bash's own exec resolution does try appending `.exe`). Passing the
/// extension explicitly on Windows sidesteps the mismatch (D-080 addendum).
///
/// Testable core of `oracle_python_bin`: takes the "is this a Windows
/// target" check as a parameter instead of calling `cfg!(windows)` directly,
/// so one test can assert both filenames regardless of the host OS actually
/// running the test (matching `pycc_rt_lib_filename` in `src/main.rs`,
/// which parameterizes an analogous host-vs-target naming choice the same
/// way after an earlier host-`cfg!`-keyed version was caught in review).
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

/// CPython's own stdio layer translates `\n` to `\r\n` on Windows even when
/// stdout is piped/redirected to a child-process capture like `Command::output()`
/// uses here (`sys.stdout`'s `TextIOWrapper` opens with `newline=None` there;
/// see CPython's bpo-11990 and bpo-13119, both intentional, stable behavior
/// since 3.2.4/3.3). `pycc_rt`'s `println!`/`print!`-based `print()` never
/// performs any such translation on any target. That translation is a
/// platform-specific quirk of CPython's own C runtime, not part of the
/// Python language's `print()` semantics, so it is stripped out of the
/// oracle's output before comparing -- this keeps the assertion checking
/// actual program-output content, not which OS's libc happened to run the
/// oracle (D-082).
fn strip_windows_newline_translation(bytes: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.into_iter().peekable();
    while let Some(byte) = iter.next() {
        if byte == b'\r' && iter.peek() == Some(&b'\n') {
            continue;
        }
        out.push(byte);
    }
    out
}

/// Builds `py_path` with `pycc build --debug` (the default profile), runs
/// the resulting binary, separately runs the pinned CPython oracle on the
/// identical source, and returns both stdouts for the caller to diff.
fn run_conformance_fixture(label: &str, py_path: &Path) -> (Vec<u8>, Vec<u8>) {
    run_conformance_fixture_with_profile(label, py_path, false)
}

/// Same as `run_conformance_fixture`, but lets the caller choose the build
/// profile. New PEP fixtures (PR-9 on) must be proven in both `--debug` and
/// `--release` before their `docs/PYTHON_STANDARDS.md` row can flip to ✅ --
/// `docs/TESTING.md`'s "both profiles" rule stopped having a v0.1-only
/// exception once `--release` shipped in PR-8. `fib`/`mandelbrot` predate
/// that rule (neither is a PEP-matrix row) and stay on the plain,
/// `--debug`-only helper above rather than being retrofitted here.
fn run_conformance_fixture_with_profile(
    label: &str,
    py_path: &Path,
    release: bool,
) -> (Vec<u8>, Vec<u8>) {
    let profile = if release { "release" } else { "debug" };
    let dir = std::env::temp_dir().join(format!(
        "pycc_conformance_{label}_{profile}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join(label);
    let mut build_command = Command::new(pycc_bin());
    build_command.args(["build", py_path.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    if release {
        build_command.arg("--release");
    }
    let status = build_command.status().unwrap();
    assert!(status.success(), "`pycc build` ({profile}) failed for {label}");
    let pycc_output = Command::new(&out).output().unwrap();
    assert!(
        pycc_output.status.success(),
        "compiled {label} binary ({profile}) exited non-zero"
    );

    let cpython_output = Command::new(oracle_python_bin())
        .arg(py_path)
        .output()
        .unwrap();
    assert!(cpython_output.status.success(), "CPython oracle exited non-zero for {label}");

    (
        pycc_output.stdout,
        strip_windows_newline_translation(cpython_output.stdout),
    )
}

#[test]
fn oracle_binary_name_appends_the_exe_extension_only_for_windows() {
    assert_eq!(oracle_binary_name(true), "python3.14.exe");
    assert_eq!(oracle_binary_name(false), "python3.14");
}

#[test]
fn strip_windows_newline_translation_removes_cr_before_lf_only() {
    let input = b"line one\r\nline two\nline three\r\n".to_vec();
    let expected = b"line one\nline two\nline three\n".to_vec();
    assert_eq!(strip_windows_newline_translation(input), expected);

    // A lone `\r` not immediately followed by `\n` is not CPython's Windows
    // newline translation and must be left untouched.
    let lone_cr = b"a\rb\n".to_vec();
    assert_eq!(strip_windows_newline_translation(lone_cr.clone()), lone_cr);
}

// Ignored by default: this test shells out to a real "python3.14" oracle,
// which the D-014 coverage gate's isolated `nobody` sandbox deliberately does
// not provision (installing a network-fetched interpreter inside that
// trust boundary would expand it far beyond what the gate is reviewed for).
// CI runs these explicitly with `-- --include-ignored` in a step placed
// after that sandboxed gate, on every Tier-1 target (D-085/D-080).
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn fib_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance_fib.py");
    let (pycc_stdout, cpython_stdout) = run_conformance_fixture("conformance_fib", &fixture);
    assert_eq!(
        pycc_stdout, cpython_stdout,
        "pycc and CPython 3.14.6 disagree on tests/fixtures/conformance_fib.py"
    );
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn mandelbrot_ascii_matches_cpython_3_14_6_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance_mandelbrot.py");
    let (pycc_stdout, cpython_stdout) = run_conformance_fixture("conformance_mandelbrot", &fixture);
    assert_eq!(
        pycc_stdout, cpython_stdout,
        "pycc and CPython 3.14.6 disagree on tests/fixtures/conformance_mandelbrot.py"
    );
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn run_conformance_fixture_with_profile_builds_both_debug_and_release() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance_fib.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("profile_check_debug", &fixture, false);
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("profile_check_release", &fixture, true);
    assert_eq!(debug_pycc, debug_cpython, "debug profile must match CPython");
    assert_eq!(release_pycc, release_cpython, "release profile must match CPython");
    assert_eq!(
        debug_pycc, release_pycc,
        "debug and release builds of the same fixture must produce identical stdout"
    );
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0526_var_annotations_matches_cpython_3_14_6_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0526_var_annotations.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0526_var_annotations_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0526_var_annotations.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0526_var_annotations_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0526_var_annotations.py"
    );
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0238_division_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0238_division.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0238_division_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0238_division.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0238_division_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0238_division.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_3105_print_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3105_print.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3105_print_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_3105_print.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3105_print_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_3105_print.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_3107_annotations_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3107_annotations.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3107_annotations_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_3107_annotations.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3107_annotations_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_3107_annotations.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_3131_unicode_ids_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3131_unicode_ids.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3131_unicode_ids_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_3131_unicode_ids.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3131_unicode_ids_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_3131_unicode_ids.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0414_u_literal_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0414_u_literal.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0414_u_literal_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0414_u_literal.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0414_u_literal_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0414_u_literal.py");
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.6) oracle on PATH"]
fn pep_0484_type_hints_matches_cpython_3_14_6_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0484_type_hints.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0484_type_hints_debug", &fixture, false);
    assert_eq!(debug_pycc, debug_cpython, "pycc (--debug) and CPython 3.14.6 disagree on tests/fixtures/pep_0484_type_hints.py");
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0484_type_hints_release", &fixture, true);
    assert_eq!(release_pycc, release_cpython, "pycc (--release) and CPython 3.14.6 disagree on tests/fixtures/pep_0484_type_hints.py");
}
