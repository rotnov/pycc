//! #921 (PEP 435): calling an enum class (`Color()`, `Color(1)`) is rejected
//! with `C0001` by `pycc_types::class::binding::resolve_instantiation` and
//! never reaches the internal-error panic its `__init__` MRO walk keeps for
//! a genuinely inconsistent class table. Before this fix both shapes aborted
//! the compiler with `internal error: no `__init__` found in class
//! `Color`'s MRO`.
//!
//! The unit tests beside the guard prove it fires; these prove the binary no
//! longer panics on the issue's own program and its sibling shapes.

use pycc_scratch::ScratchDir;
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

/// Runs `pycc <subcommand>` on `source`, asserting the compiler exits with
/// status 1 (a diagnostic, not the 101 a panic exits with), reports the
/// enum-call `C0001` naming `class_name`, and never prints a panic or
/// internal-error line. `pycc check` renders its diagnostic on stdout and
/// `pycc build` on stderr, so both streams are searched together. Asserted
/// on the message rather than the span: the diagnostic is emitted with a
/// zero span and renders against line 1.
fn assert_enum_call_rejected(slug: &str, subcommand: &str, source: &str, class_name: &str) {
    let dir = ScratchDir::new(slug).expect("failed to create scratch dir");
    let src = write_fixture(&dir, "prog.py", source);
    let out = dir.join("prog");

    let mut cmd = Command::new(pycc_bin());
    cmd.arg(subcommand).arg(src.to_str().unwrap());
    if subcommand == "build" {
        cmd.args(["-o", out.to_str().unwrap()]);
    }
    let result = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let combined = format!("{stdout}{stderr}");
    assert_eq!(
        result.status.code(),
        Some(1),
        "pycc {subcommand} should fail with a diagnostic, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        result.status
    );
    assert!(
        combined.contains("error[C0001]"),
        "the output should carry a C0001 diagnostic, got: {combined}"
    );
    assert!(
        combined.contains(&format!("cannot call enum class `{class_name}`")),
        "the output should name the enum class, got: {combined}"
    );
    for forbidden in ["panicked", "internal error"] {
        assert!(
            !combined.contains(forbidden),
            "the compiler must diagnose, not abort; found {forbidden:?} in: {combined}"
        );
    }
    assert!(!out.exists(), "a failing build must not leave a binary");
}

/// The issue's own reproduction: a zero-argument call inside a function.
#[test]
fn a_zero_argument_enum_call_in_a_function_is_c0001_not_a_panic() {
    assert_enum_call_rejected(
        "921_no_args",
        "check",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n\n\ndef main() -> None:\n    c = Color()\n    print(c.value)\n\n\nmain()\n",
        "Color",
    );
}

/// CPython's by-value member lookup, at module scope.
#[test]
fn a_by_value_enum_call_at_module_scope_is_c0001_not_a_panic() {
    assert_enum_call_rejected(
        "921_by_value",
        "check",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n\n\nc = Color(1)\nprint(c.value)\n",
        "Color",
    );
}

/// `raise Color()` takes the ordinary-call inference path (an enum is not
/// an exception class) and lands on the same guard.
#[test]
fn raising_an_enum_call_is_c0001_not_a_panic() {
    assert_enum_call_rejected(
        "921_raise",
        "check",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n\n\nraise Color()\n",
        "Color",
    );
}

/// A docstring-only enum (#744) has an empty member table; the guard keys
/// on `HirClassDef::is_enum`, so it is rejected all the same.
#[test]
fn calling_a_member_less_enum_is_c0001_not_a_panic() {
    assert_enum_call_rejected(
        "921_member_less",
        "check",
        "from enum import Enum\n\n\nclass E(Enum):\n    \"doc\"\n\n\ne = E()\nprint(1)\n",
        "E",
    );
}

/// The `StrEnum` spelling (#892) shares `lower_enum_class`, so it carries
/// the same marker and the same rejection.
#[test]
fn calling_a_str_enum_class_is_c0001_not_a_panic() {
    assert_enum_call_rejected(
        "921_str_enum",
        "check",
        "from enum import StrEnum\n\n\nclass S(StrEnum):\n    A = \"a\"\n\n\ns = S(\"a\")\nprint(s.value)\n",
        "S",
    );
}

/// `pycc build` stops at the type checker too, so `pycc_mir`'s own
/// `Instantiate` MRO-walk panic is unreachable for an enum class.
#[test]
fn building_an_enum_call_is_c0001_and_never_reaches_mir() {
    assert_enum_call_rejected(
        "921_build",
        "build",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n\n\ndef main() -> None:\n    c = Color()\n    print(c.value)\n\n\nmain()\n",
        "Color",
    );
}
