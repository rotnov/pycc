// Issue #977 (D-237): `print(<instance>)` and `f"{<instance>}"` of a class
// instance that neither MIR rewrite renders used to pass `pycc check` and
// panic in `pycc build` (or, for several shadowing shapes, abort at
// runtime). `pycc_types::string_conversion` now rejects every such shape
// with `C0001` before lowering. Every reject case below is a program that
// crashed at `f8e9d2e3`; every accept control is a program that rendered
// there and must keep rendering.

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

/// Runs `pycc check` on `source`, asserts it is rejected, and returns the
/// combined stdout+stderr text.
fn check_error(tag: &str, source: &str) -> String {
    let dir = ScratchDir::new(&format!("977_{tag}")).expect("failed to create scratch dir");
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

/// Asserts `pycc check` rejects `source` with `C0001` naming `class_name`.
#[track_caller]
fn assert_c0001(tag: &str, source: &str, class_name: &str) {
    let text = check_error(tag, source);
    assert!(
        text.contains("C0001"),
        "`{tag}`: expected C0001, got: {text}"
    );
    assert!(
        text.contains(class_name),
        "`{tag}`: expected the diagnostic to name `{class_name}`, got: {text}"
    );
}

/// Builds and runs `source`, returning `(ok, stdout, stderr)`.
fn build_and_run(tag: &str, source: &str) -> (bool, String, String) {
    let dir = ScratchDir::new(&format!("977_{tag}")).expect("failed to create scratch dir");
    let src = write_fixture(&dir, &format!("{tag}.py"), source);
    let out = dir.join(tag);
    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    if !build.status.success() {
        return (
            false,
            String::new(),
            String::from_utf8_lossy(&build.stderr).into_owned(),
        );
    }
    let output = Command::new(&out).output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[track_caller]
fn assert_prints(tag: &str, source: &str, expected: &str) {
    let (ok, stdout, stderr) = build_and_run(tag, source);
    assert!(ok, "`{tag}` failed to build or run: {stderr}");
    assert_eq!(stdout, expected, "`{tag}` printed the wrong output");
}

const PLAIN_CLASS: &str =
    "class C:\n    def __init__(self, x: int) -> None:\n        self.x = x\n\n";

// -- the issue's own shapes ------------------------------------------------

/// The issue's program: `check` passed and `build` panicked in
/// `pycc_codegen`'s `to_str`.
#[test]
fn a_plain_instance_printed_is_rejected_by_check() {
    assert_c0001(
        "plain",
        &format!("{PLAIN_CLASS}a = C(1)\nprint(a)\n"),
        "`C`",
    );
}

/// `pycc run` of the issue's program fails at the check stage with the
/// diagnostic, never reaching a backend panic.
#[test]
fn a_plain_instance_under_run_reports_c0001_and_does_not_panic() {
    let dir = ScratchDir::new("977_run").expect("failed to create scratch dir");
    let src = write_fixture(
        &dir,
        "run.py",
        &format!("{PLAIN_CLASS}a = C(1)\nprint(a)\n"),
    );
    let output = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "`pycc run` must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("C0001"),
        "expected C0001 on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("`C`"),
        "expected the class name, got: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "must not panic: {stderr}");
}

/// D-225's implicit constructor: a class with no `__init__` at all.
#[test]
fn a_class_without_init_printed_is_rejected_by_check() {
    assert_c0001("noinit", "class C:\n    pass\n\nprint(C())\n", "`C`");
}

/// A user-written `__repr__` is never called by MIR for a non-dataclass;
/// honouring it is a deferred slice, so this pin flips visibly when it lands.
#[test]
fn a_user_repr_on_a_plain_class_is_still_rejected_by_check() {
    assert_c0001(
        "userrepr",
        "class C:\n    def __init__(self, x: int) -> None:\n        self.x = x\n\n\
         \x20   def __repr__(self) -> str:\n        return \"C()\"\n\nprint(C(1))\n",
        "`C`",
    );
}

#[test]
fn a_plain_instance_interpolated_in_an_fstring_is_rejected_by_check() {
    assert_c0001(
        "fstring",
        &format!("{PLAIN_CLASS}a = C(1)\ns: str = f\"{{a}}\"\nprint(s)\n"),
        "`C`",
    );
}

#[test]
fn an_enum_member_printed_is_rejected_by_check() {
    assert_c0001(
        "enum",
        "from enum import Enum\n\nclass Color(Enum):\n    RED = 1\n    BLUE = 2\n\nprint(Color.RED)\n",
        "`Color`",
    );
}

#[test]
fn a_protocol_typed_parameter_printed_is_rejected_by_check() {
    let text = check_error(
        "protocol",
        "from typing import Protocol\n\n\
         class Shape(Protocol):\n    def area(self) -> int: ...\n\n\
         @dataclass\nclass Sq:\n    s: int\n\n    def area(self) -> int:\n        return self.s * self.s\n\n\
         def show(x: Shape) -> None:\n    print(x)\n\nshow(Sq(2))\n",
    );
    assert!(text.contains("C0001"), "expected C0001, got: {text}");
    assert!(
        text.contains("protocol `Shape`"),
        "expected the protocol message, got: {text}"
    );
}

// -- shadowing a builtin exception name (crashed at runtime before) ------

/// Null-pointer dereference at runtime before #977: MIR's name-first
/// exception resolution rewrote the plain instance to an exception message.
#[test]
fn a_user_class_named_value_error_printed_is_rejected_by_check() {
    assert_c0001(
        "shadow_plain",
        "class ValueError:\n    def __init__(self) -> None:\n        return\n\nv = ValueError()\nprint(v)\n",
        "`ValueError`",
    );
}

/// Misaligned-pointer abort at runtime before #977.
#[test]
fn a_dataclass_named_value_error_printed_is_rejected_by_check() {
    assert_c0001(
        "shadow_dataclass",
        "@dataclass\nclass ValueError:\n    x: int\n\nprint(ValueError(1))\n",
        "`ValueError`",
    );
}

#[test]
fn a_dataclass_named_exception_interpolated_is_rejected_by_check() {
    assert_c0001(
        "shadow_exception_fstring",
        "@dataclass\nclass Exception:\n    x: int\n\nv = Exception(1)\ns: str = f\"{v}\"\nprint(s)\n",
        "`Exception`",
    );
}

/// This one *rendered* before #977 (`FileNotFoundError(x=3)`); it is
/// rejected because D-237 decides a builtin exception name by provenance
/// before shape. Pinned so the loss stays visible.
#[test]
fn a_dataclass_named_after_an_os_error_family_member_is_rejected_by_check() {
    assert_c0001(
        "shadow_os_family",
        "@dataclass\nclass FileNotFoundError:\n    x: int\n\nprint(FileNotFoundError(3))\n",
        "`FileNotFoundError`",
    );
}

/// A user exception class with its own `__init__` instantiated as a value:
/// `check` passed and the program aborted (null dereference) before #977,
/// because the D-189 tag made MIR rewrite the plain instance to a message.
#[test]
fn a_user_exception_class_instantiated_and_printed_is_rejected_by_check() {
    assert_c0001(
        "user_exception_value",
        "class MyErr(Exception):\n    def __init__(self) -> None:\n        return\n\n\
         e = MyErr()\nprint(e)\n",
        "`MyErr`",
    );
}

// -- `except*` bindings typed `ExceptionGroup` unconditionally -------------

const EXCEPT_STAR_PRINT: &str = "def main() -> None:\n    try:\n        raise ValueError(\"boom\")\n\
    \x20   except* ValueError as eg:\n        print(eg)\n\nmain()\n";

const EXCEPT_STAR_FSTRING: &str = "def main() -> None:\n    try:\n        raise ValueError(\"boom\")\n\
    \x20   except* ValueError as eg:\n        s: str = f\"{eg}\"\n        print(s)\n\nmain()\n";

/// SIGSEGV before #977: the dataclass `__repr__` read field `x` off an
/// exception object.
#[test]
fn a_dataclass_named_exception_group_bound_by_except_star_is_rejected_by_check() {
    assert_c0001(
        "except_star_dataclass",
        &format!("@dataclass\nclass ExceptionGroup:\n    x: int\n\n{EXCEPT_STAR_PRINT}"),
        "`ExceptionGroup`",
    );
    assert_c0001(
        "except_star_dataclass_fstring",
        &format!("@dataclass\nclass ExceptionGroup:\n    x: int\n\n{EXCEPT_STAR_FSTRING}"),
        "`ExceptionGroup`",
    );
}

/// Codegen panic before #977: the shadow gate withheld the seeded table, so
/// `ExceptionGroup` had no class entry and no rewrite applied.
#[test]
fn an_unseeded_except_star_binding_is_rejected_by_check() {
    assert_c0001(
        "except_star_base_group",
        &format!("@dataclass\nclass BaseExceptionGroup:\n    x: int\n\n{EXCEPT_STAR_PRINT}"),
        "`ExceptionGroup`",
    );
    assert_c0001(
        "except_star_os_error",
        &format!("class OSError:\n    LIMIT: int = 1\n\n{EXCEPT_STAR_PRINT}"),
        "`ExceptionGroup`",
    );
}

// -- accept controls: everything that rendered before must still render ---

#[test]
fn a_dataclass_instance_still_renders_under_print_and_fstring() {
    assert_prints(
        "dataclass_ok",
        "@dataclass\nclass P:\n    x: int\n    y: int\n\np = P(1, 2)\nprint(p)\ns: str = f\"{p}\"\nprint(s)\n",
        "P(x=1, y=2)\nP(x=1, y=2)\n",
    );
}

/// The first public-CLI guard for rendering a caught flat-seven exception.
#[test]
fn a_caught_flat_seven_exception_still_renders() {
    assert_prints(
        "flat_seven_ok",
        "def main() -> None:\n    try:\n        raise ValueError(\"boom\")\n\
         \x20   except ValueError as e:\n        print(e)\n\nmain()\n",
        "boom\n",
    );
}

/// The unseeded flat seven: an unrelated `class OSError:` withholds the
/// seeded class table, but MIR still resolves `ValueError` by name.
#[test]
fn a_caught_flat_seven_exception_still_renders_when_the_table_is_unseeded() {
    assert_prints(
        "flat_seven_unseeded_ok",
        "class OSError:\n    LIMIT: int = 1\n\n\
         def main() -> None:\n    try:\n        raise ValueError(\"boom\")\n\
         \x20   except ValueError as e:\n        print(e)\n\nmain()\n",
        "boom\n",
    );
}

/// A `@dataclass` shadowing `OSError` leaves a plain flat-seven binding
/// renderable (the shadow only withholds seeding).
#[test]
fn a_dataclass_named_os_error_leaves_a_flat_seven_binding_renderable() {
    assert_prints(
        "dataclass_os_error_ok",
        "@dataclass\nclass OSError:\n    x: int\n\n\
         def main() -> None:\n    try:\n        raise ValueError(\"boom\")\n\
         \x20   except ValueError as e:\n        print(e)\n\nmain()\n",
        "boom\n",
    );
}

#[test]
fn a_caught_os_error_family_exception_still_renders() {
    assert_prints(
        "os_family_ok",
        "def main() -> None:\n    try:\n        raise FileNotFoundError(\"gone\")\n\
         \x20   except FileNotFoundError as e:\n        print(e)\n\nmain()\n",
        "gone\n",
    );
}

#[test]
fn a_seeded_except_star_binding_still_builds_and_runs() {
    let (ok, _stdout, stderr) = build_and_run("except_star_ok", EXCEPT_STAR_PRINT);
    assert!(ok, "seeded `except*` program failed: {stderr}");
}
