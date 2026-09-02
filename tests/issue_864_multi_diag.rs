//! #864 Part 1 (D-217): `pycc check` reports every diagnostic the failing
//! frontend pass collected for a file, and `pycc build`/`pycc run` print the
//! same renders to stderr before stopping. The byte-exact pins live in
//! `tests/diagnostics_test.rs` (`l0001_two_syntax_errors`,
//! `c0001_issue_864_repro`); the tests here check the *structure* of the
//! multi-diagnostic output that the whole-stdout snapshots cannot express
//! on their own: one JSON object per line with no blank separators, per-file
//! output concatenated byte-for-byte across files, and the `build` stderr
//! mirror of `check`'s human stdout.

use pycc_scratch::ScratchDir;
use std::path::Path;
use std::process::{Command, Output};

/// `def main(:\n` -- exactly two ruff parse errors (see the fixture's
/// comment in `tests/diagnostics_test.rs`).
const FIXTURE: &str = "tests/diagnostics/l0001_two_syntax_errors.py";

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Runs `pycc check <paths...> [--error-format json]` from the repo root so
/// the repo-relative fixture path is embedded verbatim (the same convention
/// `tests/diagnostics_test.rs` uses).
fn check(paths: &[&str], json: bool) -> Output {
    let mut cmd = Command::new(pycc_bin());
    cmd.arg("check").args(paths).current_dir(repo_root());
    if json {
        cmd.args(["--error-format", "json"]);
    }
    cmd.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(repo_root().join("tests/diagnostics").join(name))
        .unwrap()
        .replace("\r\n", "\n")
}

#[test]
fn json_output_is_one_object_per_line_with_no_separators() {
    let output = check(&[FIXTURE], true);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    // `lines()` deliberately not filtered: a blank separator line between the
    // two objects would count as a third line here.
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "expected exactly two JSON lines:\n{text}");
    for line in &lines {
        let object: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(object["code"], "L0001");
        assert_eq!(object["format_version"], 1);
    }
    // Every object ends with exactly one newline (kills `print!` without a
    // newline, a missing trailing newline, and a doubled separator).
    assert_eq!(text, format!("{}\n{}\n", lines[0], lines[1]));
}

#[test]
fn multi_file_output_is_the_concatenation_of_per_file_output() {
    let dir = ScratchDir::new("issue_864_multi_file").expect("failed to create scratch dir");
    let second = dir.join("second.py");
    // Two ruff errors as well, on different spans than the fixture's, so a
    // within-file reorder or a cross-file interleave changes the bytes.
    std::fs::write(&second, "x = (1 +\ny = 2\n").unwrap();
    let second = second.to_string_lossy().into_owned();

    for json in [false, true] {
        let a = check(&[FIXTURE], json);
        let b = check(&[&second], json);
        let both = check(&[FIXTURE, &second], json);
        assert_eq!(both.status.code(), Some(1));
        assert_eq!(
            stdout(&both),
            format!("{}{}", stdout(&a), stdout(&b)),
            "json={json}: multi-file stdout must be the per-file outputs, in order"
        );
        assert!(
            stdout(&a).matches("L0001").count() >= 2 && stdout(&b).matches("L0001").count() >= 2,
            "json={json}: both inputs must contribute several diagnostics"
        );
    }
}

#[test]
fn unreadable_middle_path_keeps_every_diagnostic_and_exits_2() {
    let dir = ScratchDir::new("issue_864_unreadable_middle").expect("failed to create scratch dir");
    let missing = dir.join("missing.py").to_string_lossy().into_owned();
    let second = dir.join("second.py");
    std::fs::write(&second, "x = (1 +\ny = 2\n").unwrap();
    let second = second.to_string_lossy().into_owned();

    let a = check(&[FIXTURE], true);
    let b = check(&[&second], true);
    let all = check(&[FIXTURE, &missing, &second], true);
    // `2` (unreadable input) takes precedence over `1` (compile diagnostics)
    // in the existing per-file `max` fold; the diagnostics themselves still
    // reach stdout while the `Input` failure goes to stderr.
    assert_eq!(all.status.code(), Some(2));
    assert_eq!(stdout(&all), format!("{}{}", stdout(&a), stdout(&b)));
    assert!(
        stderr(&all).contains("error: could not read"),
        "stderr was: {}",
        stderr(&all)
    );
}

#[test]
fn build_prints_every_diagnostic_to_stderr_and_stops_before_codegen() {
    let dir = ScratchDir::new("issue_864_build_stderr").expect("failed to create scratch dir");
    let out = dir.join("out");
    let output = Command::new(pycc_bin())
        .args(["build", FIXTURE, "-o"])
        .arg(&out)
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty(), "build must not write to stdout");
    assert!(!out.exists(), "no output artifact may be produced");
    // `build`'s stderr is exactly `check`'s human stdout for the same file:
    // every collected diagnostic, same renders, same order. A first-only
    // `report_build_failure` fails here.
    assert_eq!(
        stderr(&output),
        read_fixture("l0001_two_syntax_errors.expected.txt")
    );
}
