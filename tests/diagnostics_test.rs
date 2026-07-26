use std::path::Path;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// `pycc check` embeds whatever `path` string it was invoked with verbatim
/// into its diagnostic output (`render_human`'s ` --> {file_path}:...`
/// line, `render_json`'s `"file"` field). If this harness passed an
/// absolute path (e.g. one built from `CARGO_MANIFEST_DIR`), the checked-in
/// `.expected.txt` fixtures would bake in a machine-specific checkout path,
/// failing on every other machine and violating DIAGNOSTICS.md's
/// byte-identical-across-platforms bar. Instead, `pycc` is invoked with its
/// `current_dir` set to the repo root and a repo-relative, forward-slash
/// path literal (stable on every OS -- this exact string is what gets
/// embedded, not a `PathBuf`'s platform-dependent `Display`).
fn assert_diagnostic_matches_fixture(fixture_stem: &str) {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected_path = repo_root
        .join("tests/diagnostics")
        .join(format!("{fixture_stem}.expected.txt"));
    // pycc's diagnostic renderer always emits `\n` line endings, matching
    // DIAGNOSTICS.md's byte-identical-across-platforms bar. Git on Windows
    // checks these fixtures out with `\r\n` under the default `core.autocrlf`
    // text conversion, so the raw file bytes must be normalized before
    // comparison rather than compared as checked out.
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()))
        .replace("\r\n", "\n");

    let relative_py_path = format!("tests/diagnostics/{fixture_stem}.py");
    let output = Command::new(pycc_bin())
        .args(["check", &relative_py_path])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let actual = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        actual, expected,
        "diagnostic output for {fixture_stem} did not match its .expected.txt fixture"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{fixture_stem} should be a compile error"
    );
}

fn assert_json_diagnostic_matches_fixture(fixture_stem: &str) {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected_path = repo_root
        .join("tests/diagnostics")
        .join(format!("{fixture_stem}.expected.json"));
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()))
        .replace("\r\n", "\n");

    let relative_py_path = format!("tests/diagnostics/{fixture_stem}.py");
    let output = Command::new(pycc_bin())
        .args(["check", &relative_py_path, "--error-format", "json"])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let actual = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        actual, expected,
        "JSON diagnostic output for {fixture_stem} did not match its .expected.json fixture"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{fixture_stem} should be a compile error"
    );
}

#[test]
fn c0001_unsupported_valid_python() {
    assert_diagnostic_matches_fixture("c0001_unsupported_valid_python");
}

#[test]
fn d0001_missing_public_annotation() {
    assert_diagnostic_matches_fixture("d0001_missing_public_annotation");
}

#[test]
fn d0002_any_forbidden() {
    assert_diagnostic_matches_fixture("d0002_any_forbidden");
}

#[test]
fn d0021_range_argument_type() {
    assert_diagnostic_matches_fixture("d0021_range_argument_type");
}

#[test]
fn d0021_unbound_local() {
    assert_diagnostic_matches_fixture("d0021_unbound_local");
    assert_json_diagnostic_matches_fixture("d0021_unbound_local");
}

#[test]
fn d0022_missing_return() {
    assert_diagnostic_matches_fixture("d0022_missing_return");
}

#[test]
fn d0023_incompatible_assignment() {
    assert_diagnostic_matches_fixture("d0023_incompatible_assignment");
}

#[test]
fn d0024_return_outside_function() {
    assert_diagnostic_matches_fixture("d0024_return_outside_function");
}
