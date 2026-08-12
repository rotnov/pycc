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

fn build_and_run(dir: &std::path::Path, src: &std::path::Path, bin_name: &str) -> (bool, Vec<u8>) {
    let out = dir.join(bin_name);
    let build_status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    if !build_status.success() {
        return (false, Vec::new());
    }
    let output = Command::new(&out).output().unwrap();
    (output.status.success(), output.stdout)
}

fn check_fails(dir: &std::path::Path, src: &std::path::Path) -> bool {
    let out = dir.join("should_not_compile");
    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    !status.success()
}

/// #379: `Color.RED.value` returns the integer value assigned to the member.
#[test]
fn enum_member_value_lookup() {
    let dir = std::env::temp_dir().join(format!("pycc_379_value_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "value.py",
        "class Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\nprint(Color.RED.value)\nprint(Color.GREEN.value)\nprint(Color.BLUE.value)\n",
    );
    let (ok, stdout) = build_and_run(&dir, &src, "value");
    assert!(
        ok,
        "pycc build and run should succeed for enum value lookup"
    );
    assert_eq!(stdout, b"1\n2\n3\n", "enum member values should be 1, 2, 3");
}

/// #379: `Color.RED.name` returns the member's name as a string.
#[test]
fn enum_member_name_lookup() {
    let dir = std::env::temp_dir().join(format!("pycc_379_name_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "name.py",
        "class Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\nprint(Color.RED.name)\nprint(Color.GREEN.name)\nprint(Color.BLUE.name)\n",
    );
    let (ok, stdout) = build_and_run(&dir, &src, "name");
    assert!(ok, "pycc build and run should succeed for enum name lookup");
    assert_eq!(
        stdout, b"RED\nGREEN\nBLUE\n",
        "enum member names should be RED, GREEN, BLUE"
    );
}

/// #379: `for c in Color:` iterates members in declaration order.
#[test]
fn enum_iteration_declaration_order() {
    let dir = std::env::temp_dir().join(format!("pycc_379_iter_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "iter.py",
        "class Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\nfor c in Color:\n    print(c.value)\n    print(c.name)\n",
    );
    let (ok, stdout) = build_and_run(&dir, &src, "iter");
    assert!(ok, "pycc build and run should succeed for enum iteration");
    assert_eq!(
        stdout, b"1\nRED\n2\nGREEN\n3\nBLUE\n",
        "enum iteration should produce members in declaration order"
    );
}

/// #379: a non-integer member value is rejected with C0001.
#[test]
fn non_integer_member_value_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_379_nonint_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "nonint.py", "class Color(Enum):\n    RED = 1.5\n");
    assert!(
        check_fails(&dir, &src),
        "a non-integer enum member value should be a compile error"
    );
}

/// #379: a duplicate member name is rejected with C0001.
#[test]
fn duplicate_member_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_379_dup_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "dup.py",
        "class Color(Enum):\n    RED = 1\n    RED = 2\n",
    );
    assert!(
        check_fails(&dir, &src),
        "a duplicate enum member name should be a compile error"
    );
}

/// #379: `class C(Enum, Other):` — multiple bases with Enum is rejected.
#[test]
fn multiple_bases_with_enum_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_379_multibase_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "multibase.py",
        "class Other:\n    def __init__(self) -> None:\n        return\nclass C(Enum, Other):\n    RED = 1\n",
    );
    assert!(
        check_fails(&dir, &src),
        "class C(Enum, Other) with multiple bases should be a compile error"
    );
}

/// #379: a method definition in an enum body is rejected.
#[test]
fn method_in_enum_body_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_379_method_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "method.py",
        "class Color(Enum):\n    RED = 1\n    def f(self) -> int:\n        return 1\n",
    );
    assert!(
        check_fails(&dir, &src),
        "a method definition in an enum body should be a compile error"
    );
}

/// #379: enum iteration inside a function body works (nested unrolling).
#[test]
fn enum_iteration_inside_function() {
    let dir = std::env::temp_dir().join(format!("pycc_379_fniter_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "fniter.py",
        "class Color(Enum):\n    RED = 1\n    GREEN = 2\ndef print_values() -> None:\n    for c in Color:\n        print(c.value)\nprint_values()\n",
    );
    let (ok, stdout) = build_and_run(&dir, &src, "fniter");
    assert!(
        ok,
        "pycc build and run should succeed for enum iteration inside a function"
    );
    assert_eq!(
        stdout, b"1\n2\n",
        "enum iteration inside a function should produce member values in order"
    );
}

/// #379: accessing a non-member attribute on an enum class is rejected
/// with T0044 (the existing class-attribute rejection).
#[test]
fn non_member_attribute_on_enum_class_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_379_nonmember_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "nonmember.py",
        "class Color(Enum):\n    RED = 1\nprint(Color.NONEXISTENT.value)\n",
    );
    assert!(
        check_fails(&dir, &src),
        "accessing a non-member on an enum class should be a compile error"
    );
}
