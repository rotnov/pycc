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

/// Issue #22: a call before the first `def` of that name must fail at
/// compile time (`pycc check`), matching CPython's `NameError`.
#[test]
fn call_before_def_is_a_compile_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_call_before_def_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "call_before_def.py",
        "foo()\n\ndef foo() -> None:\n    print(42)\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "pycc check should reject call-before-def with exit code 1"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cannot call function `foo` before its definition"),
        "stdout should mention call-before-def, got: {stdout}"
    );
}

/// Issue #22: a call before the first `def` must also fail at build time.
#[test]
fn call_before_def_is_a_build_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_build_before_def_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "call_before_def.py",
        "foo()\n\ndef foo() -> None:\n    print(42)\n",
    );
    let out = dir.join("hello");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "pycc build should reject call-before-def with exit code 1"
    );
}

/// Issue #22: a redefinition affects only calls executed after that
/// definition. `foo()` before the second `def` prints `1`; `foo()` after
/// it prints `2` -- matching CPython exactly.
#[test]
fn redefinition_affects_only_subsequent_calls() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_redef_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "redef.py",
        "def foo() -> None:\n    print(1)\n\nfoo()\n\ndef foo() -> None:\n    print(2)\n\nfoo()\n",
    );
    let out = dir.join("redef");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "pycc build should succeed for redefinition");

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"1\n2\n",
        "redefinition should print 1 then 2, matching CPython"
    );
}

/// Issue #22: recursion after binding works -- a function can call itself
/// in its own body, since the function body sees all module functions.
#[test]
fn recursion_after_binding_works() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_recursion_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "recursion.py",
        "def count(n: int) -> int:\n    if n == 0:\n        return 0\n    return count(n - 1) + 1\n\nprint(count(5))\n",
    );
    let out = dir.join("recursion");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "pycc build should succeed for recursion");

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"5\n", "recursion should print 5");
}

/// Issue #22: a function body may call a sibling defined later in the
/// module -- Python's late binding evaluates the body at call time, by
/// which point all module-level `def`s have executed.
#[test]
fn function_body_calls_sibling_defined_later() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_sibling_later_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "sibling_later.py",
        "def caller() -> int:\n    return callee()\n\ndef callee() -> int:\n    return 42\n\nprint(caller())\n",
    );
    let out = dir.join("sibling_later");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for a function calling a later sibling"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n", "sibling call should print 42");
}

/// Issue #22: a function body may call a sibling defined earlier in the
/// module -- the common case, and it should still work.
#[test]
fn function_body_calls_sibling_defined_earlier() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_sibling_earlier_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "sibling_earlier.py",
        "def callee() -> int:\n    return 42\n\ndef caller() -> int:\n    return callee()\n\nprint(caller())\n",
    );
    let out = dir.join("sibling_earlier");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for a function calling an earlier sibling"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(output.stdout, b"42\n", "sibling call should print 42");
}

/// Issue #22: the redefinition regression fixture in
/// `tests/regress/issue_22_execution_order.py` compiles and produces
/// CPython-matching output (`1\n2\n`).
#[test]
fn redefinition_fixture_matches_cpython() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue22_fixture_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/regress/issue_22_execution_order.py");
    let out = dir.join("execution_order");

    let status = Command::new(pycc_bin())
        .args(["build", fixture.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for the redefinition fixture"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"1\n2\n",
        "redefinition fixture should print 1 then 2"
    );
}
