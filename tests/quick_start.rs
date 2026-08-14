use std::path::Path;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

#[test]
fn quick_start_fixture_builds_and_prints_the_documented_sequence() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quick_start.py");
    let dir = std::env::temp_dir().join(format!("pycc_quick_start_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("hello");
    let status = Command::new(pycc_bin())
        .args([
            "build",
            fixture.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "`pycc build` failed for quick_start fixture"
    );

    let output = Command::new(&out).output().unwrap();
    assert!(
        output.status.success(),
        "quick_start binary exited non-zero"
    );
    assert_eq!(
        output.stdout, b"0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n",
        "quick_start fixture stdout must match the README/site documented output"
    );
}
