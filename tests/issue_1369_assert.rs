//! End-to-end coverage for #1369: the appended `AssertionError` builtin
//! exception class (tag 28).
//!
//! Everything here goes through the public `pycc` CLI, mirroring
//! `tests/issue_1292_import_error.rs`'s harness. The whole-program tests show
//! what the `pycc_hir` unit tests cannot: that HIR tag assignment,
//! `builtin_exception_parent`'s new arm, MIR handler tag sets and the
//! runtime's name-carrying `PyExceptionObj` agree on CPython's real
//! hierarchy, `AssertionError` -> `Exception`.
//!
//! Every program raises with a literal message and prints each bound
//! exception at most once, for the same pre-existing `pycc_rt_str_decref`
//! reason `tests/issue_1292_import_error.rs` records.
//!
//! The `ext`-mode tests are `#[ignore]`d because they ask an installed
//! CPython 3.13+ to import a built artifact, which is a property of the
//! machine. CI runs them on every Tier-1 `native-build-test` leg through that
//! job's `cargo test --workspace -- --include-ignored`.

use pycc_scratch::ScratchDir;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn scratch(tag: &str) -> ScratchDir {
    ScratchDir::new(&format!("1369_{tag}")).expect("failed to create scratch dir")
}

fn write_fixture(dir: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(source.as_bytes()).unwrap();
    path
}

/// `(exit_success, stdout, stderr)` of the built program, with `\r\n`
/// normalized to `\n`.
fn build_and_run(tag: &str, source: &str) -> (bool, String, String) {
    let dir = scratch(tag);
    let src = write_fixture(&dir, &format!("{tag}.py"), source);
    let out = dir.join(tag);
    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&out).output().unwrap();
    (
        run.status.success(),
        String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&run.stderr).replace("\r\n", "\n"),
    )
}

/// The combined diagnostic text of a rejected `pycc check`.
fn check_error(tag: &str, source: &str) -> String {
    let dir = scratch(tag);
    let src = write_fixture(&dir, &format!("{tag}.py"), source);
    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "expected `{tag}` to be rejected");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn python() -> Command {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

/// Builds `body` as the extension module `module` inside `dir`, with
/// `dir/hostlib` on `PYTHONPATH`.
fn build_ext(dir: &Path, module: &str, body: &str) {
    let src = write_fixture(dir, "m.py", body);
    let build = Command::new(pycc_bin())
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .env("PYTHONPATH", dir.join("hostlib"))
        .output()
        .expect("pycc should spawn");
    assert!(
        build.status.success(),
        "{}{}",
        stdout_of(&build),
        stderr_of(&build)
    );
}

/// Runs `script` in the host interpreter with `dir` as the working directory
/// and `dir/hostlib` -- the host-only helper modules, which pycc must not
/// compile as sibling modules -- on `PYTHONPATH`.
fn run_host(dir: &Path, script: &str) -> Output {
    let run = python()
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env("PYTHONPATH", dir.join("hostlib"))
        .env("PYTHONUNBUFFERED", "1")
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    run
}

/// `AssertionError` is raisable and catchable by its own name and by
/// `except Exception:`, and `issubclass` follows its real parentage.
#[test]
fn assertion_error_is_catchable_by_its_own_name_and_by_exception() {
    let (ok, stdout, stderr) = build_and_run(
        "raise_catch",
        "def check() -> None:\n\
         \x20   raise AssertionError(\"broken\")\n\n\n\
         def main() -> None:\n\
         \x20   try:\n\
         \x20       check()\n\
         \x20   except AssertionError as e:\n\
         \x20       print(\"own:\", e)\n\
         \x20   try:\n\
         \x20       check()\n\
         \x20   except ValueError as e:\n\
         \x20       print(\"wrong handler:\", e)\n\
         \x20   except Exception as e:\n\
         \x20       print(\"base:\", e)\n\
         \x20   print(issubclass(AssertionError, Exception), \
         issubclass(AssertionError, ValueError))\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "own: broken\nbase: broken\nTrue False\n");
}

/// A user subclass of `AssertionError` is raisable and caught by
/// `except AssertionError:`.
#[test]
fn a_user_assertion_error_subclass_is_caught_by_assertion_error() {
    let (ok, stdout, stderr) = build_and_run(
        "user_subclass",
        "class InvariantBroken(AssertionError):\n    pass\n\n\n\
         def main() -> None:\n\
         \x20   try:\n\
         \x20       raise InvariantBroken(\"inv\")\n\
         \x20   except AssertionError as e:\n\
         \x20       print(\"caught:\", e)\n\n\n\
         main()\n",
    );
    assert!(ok, "program failed: {stderr}");
    assert_eq!(stdout, "caught: inv\n");
}

/// An uncaught `AssertionError` names itself at the top level, which pins the
/// tag-to-name round trip through `PyExceptionObj`'s carried name.
#[test]
fn an_uncaught_assertion_error_reports_its_own_class_name() {
    let (ok, _stdout, stderr) = build_and_run(
        "uncaught",
        "def main() -> None:\n\
         \x20   raise AssertionError(\"top\")\n\n\n\
         main()\n",
    );
    assert!(!ok, "the program should have exited non-zero");
    assert_eq!(
        stderr.lines().last(),
        Some("AssertionError: top"),
        "unexpected stderr: {stderr}"
    );
}

/// The all-or-nothing shadow gate now covers the new name (documented in
/// `docs/RUNTIME.md`'s shadow-gate paragraph): a module declaring its own
/// `class AssertionError(Exception)` withholds seeding, so its base
/// `Exception` is unknown -- exactly as a user `class ImportError(Exception)`
/// already did.
#[test]
fn a_user_class_named_assertion_error_withholds_seeding() {
    let text = check_error(
        "user_assertion_error",
        "class AssertionError(Exception):\n    pass\n\n\n\
         def main() -> None:\n\
         \x20   try:\n\
         \x20       raise AssertionError(\"m\")\n\
         \x20   except AssertionError:\n\
         \x20       print(\"caught\")\n\n\n\
         main()\n",
    );
    assert!(text.contains("C0001"), "unexpected diagnostic: {text}");
    assert!(
        text.contains("class `AssertionError` inherits from unknown class `Exception`"),
        "unexpected diagnostic: {text}"
    );
}

/// The `ext`-mode half: the C shim's `case 28:` hands the host CPython's own
/// `AssertionError`; without it tag 28 falls through `default:` and the host
/// sees a plain `Exception`.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_built_ext_module_raises_cpython_assertion_error() {
    let dir = ScratchDir::new("1369_ext").expect("scratch");
    build_ext(
        &dir,
        "pycc_assertion_error_mod",
        "def f(n: int) -> int:\n\
         \x20   if n == 0:\n\
         \x20       raise AssertionError(\"explicit\")\n\
         \x20   return n\n",
    );
    let run = run_host(
        &dir,
        "import pycc_assertion_error_mod as m\n\
         try:\n\
         \x20   m.f(0)\n\
         \x20   raise RuntimeError('expected an AssertionError')\n\
         except AssertionError as e:\n\
         \x20   print(type(e).__name__, type(e).__mro__[1].__name__, repr(str(e)))\n\
         print(m.f(7))\n",
    );
    assert_eq!(stdout_of(&run), "AssertionError Exception 'explicit'\n7\n");
}

/// The #1316 foreign-to-pycc direction: a host `AssertionError` raised by a
/// foreign call inside a compiled function now maps to tag 28, so the
/// function's own `except AssertionError:` catches it (before #1369 it was
/// bridged as a plain `Exception` and the handler could not name it).
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_foreign_assertion_error_is_caught_by_except_assertion_error() {
    let dir = ScratchDir::new("1369_bridge").expect("scratch");
    let hostlib = dir.join("hostlib");
    std::fs::create_dir_all(&hostlib).expect("create hostlib");
    write_fixture(
        &hostlib,
        "pycc_1369_helper.py",
        "def boom():\n    raise AssertionError('from the host')\n",
    );
    build_ext(
        &dir,
        "pycc_1369_bridge",
        "import pycc_1369_helper\n\n\n\
         def f() -> int:\n\
         \x20   try:\n\
         \x20       pycc_1369_helper.boom()\n\
         \x20   except ValueError:\n\
         \x20       return 1\n\
         \x20   except AssertionError:\n\
         \x20       return 2\n\
         \x20   return 0\n",
    );
    let run = run_host(&dir, "import pycc_1369_bridge as m\nprint(m.f())\n");
    assert_eq!(stdout_of(&run), "2\n");
}
