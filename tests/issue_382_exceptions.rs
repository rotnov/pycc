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

// -- Caught ValueError --

#[test]
fn caught_value_error_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_382_cve_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "cve.py",
        "try:\n    raise ValueError(\"bad\")\nexcept ValueError:\n    print(\"caught\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught\n");
}

// -- Catch-all Exception --

#[test]
fn catch_all_exception_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_382_ca_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "ca.py",
        "try:\n    raise ValueError(\"bad\")\nexcept Exception:\n    print(\"caught all\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught all\n");
}

// -- Handler ordering: first matching handler wins --

#[test]
fn handler_ordering_first_match_wins() {
    let dir = std::env::temp_dir().join(format!("pycc_382_ho_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "ho.py",
        "try:\n    raise ValueError(\"bad\")\nexcept ValueError:\n    print(\"value\")\nexcept Exception:\n    print(\"generic\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"value\n");
}

// -- Else body runs when no exception --

#[test]
fn else_body_runs_when_no_exception() {
    let dir = std::env::temp_dir().join(format!("pycc_382_else_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "else.py",
        "try:\n    print(\"body\")\nexcept ValueError:\n    print(\"handler\")\nelse:\n    print(\"else\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"body\nelse\n");
}

// -- Finally always runs --

#[test]
fn finally_always_runs_on_success() {
    let dir = std::env::temp_dir().join(format!("pycc_382_fin1_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "fin1.py",
        "try:\n    print(\"body\")\nfinally:\n    print(\"finally\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"body\nfinally\n");
}

#[test]
fn finally_runs_after_handler() {
    let dir = std::env::temp_dir().join(format!("pycc_382_fin2_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "fin2.py",
        "try:\n    raise ValueError(\"bad\")\nexcept ValueError:\n    print(\"handler\")\nfinally:\n    print(\"finally\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"handler\nfinally\n");
}

// -- Uncaught exception exits with non-zero --

#[test]
fn uncaught_exception_exits_nonzero() {
    let dir = std::env::temp_dir().join(format!("pycc_382_uncaught_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, _out, err) = build_and_run(
        &dir,
        "uncaught.py",
        "raise ValueError(\"uncaught\")\n",
    );
    assert!(!ok, "expected non-zero exit for uncaught exception");
    assert!(err.contains("ValueError"), "stderr should contain ValueError: {err}");
}

// -- Division by zero is caught --

#[test]
fn division_by_zero_is_caught() {
    let dir = std::env::temp_dir().join(format!("pycc_382_zdiv_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "zdiv.py",
        "try:\n    x = 1 // 0\nexcept ZeroDivisionError:\n    print(\"caught zdiv\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught zdiv\n");
}

// -- Float division by zero is caught --

#[test]
fn float_division_by_zero_is_caught() {
    let dir = std::env::temp_dir().join(format!("pycc_382_fzdiv_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "fzdiv.py",
        "try:\n    x = 1.0 / 0.0\nexcept ZeroDivisionError:\n    print(\"caught fzdiv\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught fzdiv\n");
}

// -- Missing dictionary key is caught --

#[test]
fn missing_dict_key_is_caught() {
    let dir = std::env::temp_dir().join(format!("pycc_382_key_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "key.py",
        "d = {\"a\": 1}\ntry:\n    v = d[\"b\"]\nexcept KeyError:\n    print(\"caught key\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught key\n");
}

// -- List index out of range is caught --

#[test]
fn list_index_out_of_range_is_caught() {
    let dir = std::env::temp_dir().join(format!("pycc_382_idx_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "idx.py",
        "xs = [1, 2, 3]\ntry:\n    v = xs[10]\nexcept IndexError:\n    print(\"caught idx\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught idx\n");
}

// -- raise from (PEP 409) --

#[test]
fn raise_from_builds_and_runs() {
    let dir = std::env::temp_dir().join(format!("pycc_382_from_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, out, err) = build_and_run(
        &dir,
        "from.py",
        "try:\n    raise ValueError(\"bad\") from TypeError(\"cause\")\nexcept ValueError:\n    print(\"caught\")\n",
    );
    assert!(ok, "build/run failed: {err}");
    assert_eq!(out, b"caught\n");
}

// -- Bare re-raise propagates --

#[test]
fn bare_reraise_propagates() {
    let dir = std::env::temp_dir().join(format!("pycc_382_reraise_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, _out, err) = build_and_run(
        &dir,
        "reraise.py",
        "try:\n    raise ValueError(\"orig\")\nexcept ValueError:\n    raise\n",
    );
    assert!(!ok, "expected non-zero exit for re-raised exception");
    assert!(err.contains("ValueError"), "stderr should contain ValueError: {err}");
}

// -- except* is rejected (C0001) --

#[test]
fn except_star_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_382_star_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, combined) = check_only(
        &dir,
        "star.py",
        "try:\n    pass\nexcept* ValueError:\n    pass\n",
    );
    assert!(!ok, "except* should be rejected");
    assert!(combined.contains("C0001"), "should mention C0001: {combined}");
}

// -- Bare raise outside handler is rejected --

#[test]
fn bare_raise_outside_handler_is_rejected() {
    let dir = std::env::temp_dir().join(format!("pycc_382_bare_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (ok, _combined) = check_only(
        &dir,
        "bare.py",
        "raise\n",
    );
    assert!(!ok, "bare raise outside handler should be rejected");
}
