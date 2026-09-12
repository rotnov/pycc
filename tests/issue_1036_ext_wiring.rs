//! `pycc build --ext` end to end, through the real CLI (D-244, #1036,
//! Part 1 of #1025).
//!
//! The non-`#[ignore]`d tests here need no CPython installation: they drive
//! the flag through `main()`'s own `Command::Build` arm and assert the
//! diagnostics and exit codes `docs/CLI_SPEC.md` promises. That matters for
//! more than tidiness -- `.github/workflows/ci.yml`'s coverage job runs
//! `llvm-cov` without `--include-ignored`, so an ignored test contributes
//! exactly zero coverage while its lines stay in
//! `scripts/check_diff_coverage.py`'s denominator.
//!
//! The `#[ignore]`d test at the end is the real thing: it builds an artifact
//! against an installed CPython and imports it. It is opt-in because it
//! needs a `python3` with development headers, which is a property of the
//! machine, not of the change under test. CI installs CPython 3.13 -- D-244's
//! stable-ABI floor -- on every Tier-1 leg of `native-build-test` and runs it
//! there through that job's existing `cargo test --workspace --
//! --include-ignored`.
//!
//! `PYCC_PYTHON`/`PYCC_PYTHON_INCLUDE` are set on the *child* `Command`
//! here, never on this test process: `std::env::set_var` is process-global
//! and would race every other test in this binary.

use pycc_scratch::ScratchDir;
use std::path::Path;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn write(dir: &Path, body: &str) -> std::path::PathBuf {
    let src = dir.join("m.py");
    std::fs::write(&src, body).expect("write the fixture source");
    src
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// An include directory that exists but holds no `Python.h`, so the build
/// reaches the compiler and fails there deterministically on any host.
fn header_less_build(dir: &Path, src: &Path, out: &Path) -> Output {
    pycc()
        .arg("build")
        .arg(src)
        .arg("-o")
        .arg(out)
        .arg("--ext")
        .env("PYCC_PYTHON_INCLUDE", dir)
        .output()
        .expect("pycc should spawn")
}

#[test]
fn ext_rejects_a_public_function_the_boundary_cannot_carry_with_c0003() {
    let dir = ScratchDir::new("ext_cli_gap").expect("scratch");
    let src = write(
        &dir,
        "def scale(factor: float) -> float:\n    return factor\n",
    );
    let output = header_less_build(&dir, &src, &dir.join("m"));
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_of(&output);
    assert!(stderr.contains("error[C0003]"), "{stderr}");
    assert!(stderr.contains("`scale`"), "{stderr}");
    // The advice is actionable and names both ways out.
    assert!(stderr.contains("`_scale`"), "{stderr}");
    assert!(stderr.contains("without --ext"), "{stderr}");
}

#[test]
fn ext_reports_every_capability_gap_in_one_build() {
    let dir = ScratchDir::new("ext_cli_gaps").expect("scratch");
    let src = write(
        &dir,
        "def a(x: float) -> int:\n    return 1\n\ndef b(y: int) -> str:\n    return \"s\"\n",
    );
    let output = header_less_build(&dir, &src, &dir.join("m"));
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_of(&output);
    assert_eq!(stderr.matches("error[C0003]").count(), 2, "{stderr}");
}

#[test]
fn a_private_function_is_not_in_the_export_set_and_raises_no_gap() {
    let dir = ScratchDir::new("ext_cli_private").expect("scratch");
    // D-038: the leading underscore is the whole opt-out mechanism.
    let src = write(
        &dir,
        "def _scale(factor: float) -> float:\n    return factor\n\n\
         def twice(x: int) -> int:\n    return x * 2\n",
    );
    let output = header_less_build(&dir, &src, &dir.join("m"));
    let stderr = stderr_of(&output);
    assert!(!stderr.contains("C0003"), "{stderr}");
    // It got past the export scan and died in the compiler instead, which is
    // the expected end of the road without CPython headers.
    assert!(stderr.contains("Python.h"), "{stderr}");
}

#[test]
fn an_ext_output_path_that_names_no_module_is_an_invocation_failure() {
    let dir = ScratchDir::new("ext_cli_out").expect("scratch");
    let src = write(&dir, "def f() -> int:\n    return 1\n");
    let output = header_less_build(&dir, &src, Path::new("/"));
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("--ext output path"),
        "{output:?}"
    );
}

#[test]
fn an_ext_output_path_with_an_interpreter_specific_suffix_is_rejected() {
    let dir = ScratchDir::new("ext_cli_tagged").expect("scratch");
    let src = write(&dir, "def f() -> int:\n    return 1\n");
    let out = dir.join("m.cpython-314-x86_64-linux-gnu.so");
    let output = header_less_build(&dir, &src, &out);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("interpreter-specific"),
        "{output:?}"
    );
}

#[test]
fn an_unrunnable_interpreter_is_an_environment_failure_naming_the_override() {
    let dir = ScratchDir::new("ext_cli_interp").expect("scratch");
    let src = write(&dir, "def f() -> int:\n    return 1\n");
    let output = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("m"))
        .arg("--ext")
        .env("PYCC_PYTHON", "pycc-no-such-interpreter-1036")
        .env_remove("PYCC_PYTHON_INCLUDE")
        .output()
        .expect("pycc should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(stderr.contains("pycc-no-such-interpreter-1036"), "{stderr}");
    assert!(stderr.contains("PYCC_PYTHON_INCLUDE"), "{stderr}");
}

#[test]
fn a_native_build_of_the_same_source_is_unaffected_by_the_new_flag() {
    // The regression guard for the whole change: `--ext` must not have
    // altered the native path it was threaded through.
    let dir = ScratchDir::new("ext_cli_native").expect("scratch");
    let src = write(
        &dir,
        "def twice(x: int) -> int:\n    return x * 2\n\nprint(twice(21))\n",
    );
    let out = dir.join("native");
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let run = Command::new(&out).output().expect("the binary should run");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

/// The real artifact, against a real interpreter: builds an extension
/// module and asks CPython to import it and call it.
///
/// `#[ignore]`d because it needs a CPython with development headers on the
/// machine. Everything it proves that the tests above cannot -- that the
/// generated C compiles against a real `Python.h`, that the multi-phase
/// init runs the module body before any wrapper is reachable, and that the
/// `int` boundary round-trips -- is a property of a built artifact, and
/// there is no way to observe it without building one.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_built_ext_module_imports_and_answers_through_the_int_boundary() {
    let dir = ScratchDir::new("ext_oracle").expect("scratch");
    let src = write(
        &dir,
        "def twice(x: int) -> int:\n    return x * 2\n\n\
         def add(a: int, b: int) -> int:\n    return a + b\n\n\
         def zero() -> int:\n    return 0\n",
    );
    let build = pycc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("pycc_oracle_mod"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));

    let script = "import sys, pycc_oracle_mod as m\n\
                  assert m.twice(21) == 42, m.twice(21)\n\
                  assert m.add(1, 2) == 3\n\
                  assert m.zero() == 0\n\
                  assert m.twice(True) == 2\n\
                  try:\n    m.twice('x')\nexcept TypeError:\n    pass\n\
                  else:\n    raise AssertionError('str must not be accepted')\n\
                  try:\n    m.twice(1, 2)\nexcept TypeError:\n    pass\n\
                  else:\n    raise AssertionError('arity must be checked')\n\
                  try:\n    m.twice(x=1)\nexcept TypeError:\n    pass\n\
                  else:\n    raise AssertionError('keywords must be refused')\n\
                  print('ok')\n";
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(script)
        .current_dir(&*dir)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        stderr_of(&run)
    );
}
