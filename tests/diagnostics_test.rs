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
    let expected_path = repo_root.join("tests/diagnostics").join(format!("{fixture_stem}.expected.txt"));
    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()));

    let relative_py_path = format!("tests/diagnostics/{fixture_stem}.py");
    let output = Command::new(pycc_bin())
        .args(["check", &relative_py_path])
        .current_dir(repo_root)
        .output()
        .unwrap();
    let actual = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        actual.trim_end(),
        expected.trim_end(),
        "diagnostic output for {fixture_stem} did not match its .expected.txt fixture"
    );
    assert_eq!(output.status.code(), Some(1), "{fixture_stem} should be a compile error");
}

#[test]
fn d0001_missing_public_annotation() {
    assert_diagnostic_matches_fixture("d0001_missing_public_annotation");
}

#[test]
fn d0002_any_forbidden() {
    assert_diagnostic_matches_fixture("d0002_any_forbidden");
}
