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
fn run_subcommand_accepts_unicode_and_dash_prefixed_program_arguments() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_run_args_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello_run_args.py", "print(42)\n");

    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap(), "--", "one", "olá", "--flag"])
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
fn build_and_run_report_private_workspace_creation_failures() {
    let parent = tempfile::tempdir().unwrap();
    let missing_temp_root = parent.path().join("does-not-exist");

    for arguments in [
        vec!["build", "unused.py", "-o", "unused"],
        vec!["run", "unused.py"],
    ] {
        let output = Command::new(pycc_bin())
            .args(arguments)
            .env("TMPDIR", &missing_temp_root)
            .env("TMP", &missing_temp_root)
            .env("TEMP", &missing_temp_root)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("could not create a private temporary workspace"),
            "{stderr}"
        );
    }
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
fn unsupported_valid_python_is_a_compile_diagnostic_not_a_panic() {
    let cases = [
        ("assignment", "x = 1\n"),
        ("non-call expression", "42\n"),
        ("attribute callee", "foo.bar()\n"),
        ("user-function positional argument", "foo(42)\n"),
        ("print argument count", "print(1, 2)\n"),
        ("print float", "print(3.14)\n"),
        (
            "integer outside i64",
            "print(99999999999999999999999999999999)\n",
        ),
        ("keyword argument", "print(42, end=\"\")\n"),
        ("function parameter", "def f(x) -> None:\n    print(42)\n"),
        ("unsupported builtin", "bool()\n"),
        ("unsupported builtin exception", "ValueError()\n"),
        ("builtin shadowing", "def len() -> None:\n    print(42)\n"),
    ];

    for (label, source) in cases {
        let dir = std::env::temp_dir().join(format!(
            "pycc_e2e_unsupported_{}_{}",
            label.replace(' ', "_"),
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = write_fixture(&dir, "unsupported.py", source);
        let out = dir.join("unsupported");

        let output = Command::new(pycc_bin())
            .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "case `{label}`: {stderr}");
        assert!(stderr.contains("L0003"), "case `{label}`: {stderr}");
        assert!(
            stderr.contains("unsupported.py:1:"),
            "case `{label}`: {stderr}"
        );
        assert!(
            stderr
                .lines()
                .any(|line| line.contains('|') && line.contains('^')),
            "case `{label}`: {stderr}"
        );
        assert!(!stderr.contains("panicked"), "case `{label}`: {stderr}");
    }
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
fn calling_a_function_before_its_definition_is_a_runtime_name_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_call_before_def_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "call_before_def.py",
        "foo()\ndef foo() -> None:\n    print(42)\n",
    );
    let out = dir.join("call_before_def");

    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success());

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&output.stderr).contains("name 'foo' is not defined"));

    let run_output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(run_output.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&run_output.stderr).contains("name 'foo' is not defined"));
}

#[test]
fn repeated_function_definitions_take_effect_in_execution_order() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_redefinition_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "redefinition.py",
        "def foo() -> None:\n    print(1)\nfoo()\ndef foo() -> None:\n    print(2)\nfoo()\n",
    );
    let out = dir.join("redefinition");

    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success());

    let output = Command::new(&out).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n2\n");
}

#[test]
fn function_body_calls_observe_the_current_global_binding() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_body_binding_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "body_binding.py",
        "def call_foo() -> None:\n    foo()\ndef foo() -> None:\n    print(1)\ncall_foo()\ndef foo() -> None:\n    print(2)\ncall_foo()\n",
    );
    let out = dir.join("body_binding");

    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success());

    let output = Command::new(&out).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n2\n");
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
fn an_unavailable_target_compiler_is_a_clean_link_error() {
    let dir = std::env::temp_dir().join(format!("pycc_e2e_missing_clang_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "hello.py", "print(42)\n");
    let out = dir.join("hello");

    let output = Command::new(pycc_bin())
        .env("PYCC_CLANG", "__pycc_missing_compiler_driver__")
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("could not start target-aware compiler driver"));
    assert!(!stderr.contains("panicked"));
}
