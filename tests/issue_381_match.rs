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

fn build_and_run(dir: &std::path::Path, src_name: &str, source: &str) -> (bool, Vec<u8>, String) {
    let src = write_fixture(dir, src_name, source);
    let out = dir.join(src_name.replace(".py", ""));
    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    if !output.status.success() {
        return (
            false,
            Vec::new(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        );
    }
    let run = Command::new(&out).output().unwrap();
    (
        run.status.success(),
        run.stdout.clone(),
        String::from_utf8_lossy(&run.stderr).to_string(),
    )
}

fn check_only(dir: &std::path::Path, src_name: &str, source: &str) -> (bool, String) {
    let src = write_fixture(dir, src_name, source);
    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

// -- Literal patterns --

#[test]
fn match_literal_int_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_lit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "lit.py",
        "x = 2\nmatch x:\n    case 1:\n        print(1)\n    case 2:\n        print(2)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"2\n");
}

#[test]
fn match_literal_string_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_str_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "str.py",
        "x = \"hi\"\nmatch x:\n    case \"hi\":\n        print(1)\n    case \"bye\":\n        print(2)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"1\n");
}

// -- Capture patterns --

#[test]
fn match_capture_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_cap_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "cap.py",
        "x = 42\nmatch x:\n    case y:\n        print(y)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"42\n");
}

// -- Wildcard patterns --

#[test]
fn match_wildcard_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_wild_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "wild.py",
        "x = 5\nmatch x:\n    case 1:\n        print(1)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"0\n");
}

// -- Singleton patterns: True, False, None --

#[test]
fn match_singleton_true_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_true_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "true.py",
        "x = True\nmatch x:\n    case True:\n        print(1)\n    case False:\n        print(2)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"1\n");
}

#[test]
fn match_singleton_false_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_false_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "false.py",
        "x = False\nmatch x:\n    case True:\n        print(1)\n    case False:\n        print(2)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"2\n");
}

#[test]
fn match_singleton_none_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_none_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "none.py",
        "def f() -> None:\n    pass\nmatch f():\n    case None:\n        print(1)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"1\n");
}

// -- Exhaustive bool matching --

#[test]
fn match_bool_exhaustive_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_bool_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "bool.py",
        "x = True\nmatch x:\n    case True:\n        print(1)\n    case False:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"1\n");
}

// -- Guards --

#[test]
fn match_guard_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_guard_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "guard.py",
        "x = 5\nmatch x:\n    case y if y > 3:\n        print(y)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"5\n");
}

#[test]
fn match_guard_false_falls_through() {
    let dir = std::env::temp_dir().join(format!("pycc_381_guard2_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "guard2.py",
        "x = 1\nmatch x:\n    case y if y > 3:\n        print(y)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"0\n");
}

// -- Or-patterns --

#[test]
fn match_or_pattern_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_or_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "or.py",
        "x = 2\nmatch x:\n    case 1 | 2 | 3:\n        print(1)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"1\n");
}

// -- Non-exhaustive diagnostics --

#[test]
fn match_non_exhaustive_is_a_check_error() {
    let dir = std::env::temp_dir().join(format!("pycc_381_nonex_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, err) = check_only(
        &dir,
        "nonex.py",
        "def f(x: int) -> None:\n    match x:\n        case 0:\n            print(0)\n        case 1:\n            print(1)\n",
    );
    assert!(!ok, "non-exhaustive match should fail check");
    assert!(
        err.contains("T0030"),
        "should report T0030 for non-exhaustive match, got: {err}"
    );
}

// -- Pass-only case bodies --

#[test]
fn match_pass_only_body_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_pass_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "pass.py",
        "x = 1\nmatch x:\n    case 1:\n        pass\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"");
}

// -- Capture availability in case body --

#[test]
fn match_capture_available_in_body() {
    let dir = std::env::temp_dir().join(format!("pycc_381_capbody_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "capbody.py",
        "x = 7\nmatch x:\n    case y:\n        print(y)\n        print(y + 1)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"7\n8\n");
}

// -- Definite-assignment joining --

#[test]
fn match_definite_assignment_join() {
    let dir = std::env::temp_dir().join(format!("pycc_381_def_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "def.py",
        "x = 3\nresult = 0\nmatch x:\n    case 1:\n        result = 10\n    case 2:\n        result = 20\n    case _:\n        result = 30\nprint(result)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"30\n");
}

// -- Type mismatch diagnostics --

#[test]
fn match_guard_not_bool_is_a_check_error() {
    let dir = std::env::temp_dir().join(format!("pycc_381_guardty_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, err) = check_only(
        &dir,
        "guardty.py",
        "def f(x: int) -> None:\n    match x:\n        case y if y:\n            print(y)\n        case _:\n            print(0)\n",
    );
    assert!(!ok, "non-bool guard should fail check");
    assert!(
        err.contains("T0021"),
        "should report T0021 for non-bool guard, got: {err}"
    );
}

// -- Nested match --

#[test]
fn match_nested_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_nested_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "nested.py",
        "x = 1\ny = 2\nmatch x:\n    case 1:\n        match y:\n            case 2:\n                print(12)\n            case _:\n                print(10)\n    case _:\n        print(0)\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"12\n");
}

// -- Match inside a function --

#[test]
fn match_inside_function_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_fn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "fn.py",
        "def classify(n: int) -> str:\n    match n:\n        case 0:\n            return \"zero\"\n        case _:\n            return \"other\"\nprint(classify(0))\nprint(classify(5))\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"zero\nother\n");
}

// -- Match with return in each arm --

#[test]
fn match_with_return_in_each_arm_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_381_ret_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "ret.py",
        "def f(x: int) -> int:\n    match x:\n        case 1:\n            return 10\n        case 2:\n            return 20\n        case _:\n            return 0\nprint(f(1))\nprint(f(2))\nprint(f(3))\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"10\n20\n0\n");
}
