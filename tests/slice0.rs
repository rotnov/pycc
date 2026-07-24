use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

#[test]
fn a_missing_input_file_is_a_clean_error_not_a_panic() {
    // Regression test (found in PR review): an earlier version used
    // .expect() on read_to_string, so a typo'd path panicked (exit 101,
    // raw backtrace) instead of a clean CLI error.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_missing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing_path = dir.join("does_not_exist.py");
    let out = dir.join("out");

    let output = Command::new(pycc_bin())
        .args([
            "build",
            missing_path.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not read"));
}

#[test]
fn build_and_run_explicit_call_to_main() {
    // Python never auto-invokes a function merely because it's named
    // `main` -- the source has to call it, same as any other function.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "hello.py",
        "def main() -> None:\n    print(42)\n\nmain()\n",
    );
    let out = dir.join("hello");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn defining_main_without_calling_it_produces_no_output() {
    // Regression test for a confirmed bug (found in PR review, fixed in
    // the same PR): an earlier version of pycc_codegen treated a function
    // literally named `main` as an auto-invoked entry point, which isn't
    // real Python semantics -- CPython never runs a function just because
    // of its name. Verified manually against `python3.14` on this exact
    // source (not shelled out to from this test, since a hardcoded
    // interpreter path would be machine-specific and break CI; the real
    // conformance harness that runs this kind of check portably is
    // pycc_testkit, deferred per DECISIONS.md): zero bytes of stdout.
    let dir = std::env::temp_dir().join(format!("pycc_e2e_uncalled_main_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "hello_uncalled.py",
        "def main() -> None:\n    print(42)\n",
    );
    let out = dir.join("hello_uncalled");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"",
        "must match CPython's actual output for this source"
    );
}

#[test]
fn build_and_run_top_level_print_with_no_main() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_toplevel_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_toplevel.py", "print(42)\n");
    let out = dir.join("hello_toplevel");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn run_subcommand_builds_and_executes_in_one_step() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_run.py", "print(42)\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn run_subcommand_propagates_a_build_failure() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_fail_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad_run.py", "def main(:\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("L0001"));
}

#[test]
fn version_flag_prints_something() {
    let output = Command::new(pycc_bin())
        .args(["version", "--verbose"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
}

#[test]
fn unimplemented_subcommands_exit_with_code_2() {
    let output = Command::new(pycc_bin()).args(["clean"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_syntax_error_is_a_compile_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_synerr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "bad.py", "def main(:\n");
    let out = dir.join("bad");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("L0001"));
}

#[test]
fn defining_a_function_under_any_name_without_calling_it_succeeds() {
    // There's no "must be named main" restriction: any function name is
    // legal to define; only calling one runs it (matches CPython, which
    // has no such restriction either).
    let dir = std::env::temp_dir().join(format!("pycc_e2e_anyfn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "anyfn.py", "def helper() -> None:\n    print(1)\n");
    let out = dir.join("anyfn");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"");
}

#[test]
fn calling_an_undefined_function_is_a_codegen_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_undefined_fn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "undefined_fn.py", "does_not_exist()\n");
    let out = dir.join("undefined_fn");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does_not_exist"));
}

#[test]
fn a_bad_output_path_is_a_link_error_exit_code_1() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_badout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_badout.py", "print(42)\n");
    let bad_out = dir.join("does_not_exist_dir").join("hello");

    let status = Command::new(pycc_bin())
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            bad_out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn check_accepts_every_staged_file_in_one_invocation() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_many_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let first = write_fixture(&dir, "first.py", "print(1)\n");
    let second = write_fixture(&dir, "second.py", "def helper() -> None:\n    print(2)\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&first)
        .arg(&second)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_accepts_a_staged_filename_that_starts_with_a_hyphen() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_hyphen_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_fixture(&dir, "--staged.py", "print(1)\n");

    let output = Command::new(pycc_bin())
        .args(["check", "--staged.py"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_reports_every_failure_and_io_errors_take_exit_code_precedence() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_check_errors_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let invalid = write_fixture(&dir, "invalid.py", "def main(:\n");
    let missing = dir.join("missing.py");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&invalid)
        .arg(&missing)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains(invalid.to_str().unwrap()));
    assert!(stderr.contains("L0001"));
    assert!(stderr.contains(missing.to_str().unwrap()));
    assert!(stderr.contains("could not read"));
}

#[test]
fn check_rejects_a_currently_unsupported_construct_without_panicking() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_check_unsupported_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "assignment.py", "x = 1\n");

    let output = Command::new(pycc_bin())
        .arg("check")
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("C0001"));
    assert!(!stderr.contains("panicked"));
}

#[test]
fn check_without_files_is_a_clean_invocation_error() {
    let output = Command::new(pycc_bin()).arg("check").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least one Python file"));
}

#[test]
fn build_rejects_a_currently_unsupported_construct_without_panicking() {
    let dir =
        std::env::temp_dir().join(format!("pycc_e2e_build_unsupported_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "assignment.py", "x = 1\n");
    let out = dir.join("assignment");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("C0001"));
    assert!(!stderr.contains("panicked"));
}

#[test]
fn repository_publishes_the_pycc_check_pre_commit_hook() {
    let manifest_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".pre-commit-hooks.yaml");
    let manifest = std::fs::read_to_string(manifest_path).unwrap();

    assert_eq!(
        manifest,
        "\
- id: pycc-check
  name: pycc check
  description: Check typed Python files with the pycc frontend
  entry: pycc check
  language: rust
  types: [python]
  require_serial: true
"
    );
}
