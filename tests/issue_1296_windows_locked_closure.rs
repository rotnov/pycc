//! Part 1 of #1287 (#1296): on a Windows host `pycc lock` writes the
//! `x86_64-pc-windows-msvc` section, and `pycc build` bundles the locked
//! pure-Python closure into `OUT.pycc\closure\`, the third module search
//! path after `Lib` and `DLLs` (D-249's and D-253's 2026-09-24
//! amendments under `docs/decisions/`). A closure holding a PE image is
//! refused until #1297 scans its dependencies.
//!
//! Windows-only: every other host runs the same code on the fake Windows
//! layout in `src/embed/windows_lock_tests.rs`. The tests lock a real
//! `python -m venv --without-pip` environment of CPython 3.14.7
//! (`PYCC_PYTHON`, default `python3.14.exe`) holding a test-authored
//! `tinypkg`, so nothing downloads anything and no test calls `pip`, `uv`
//! or an index. On a Windows GitHub Actions leg a missing precondition
//! fails the test instead of skipping it, the pattern of
//! `tests/issue_1286_windows_embedded_executable.rs`.

#![cfg(windows)]

use pycc_scratch::ScratchDir;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// pycc's own SHA-256, shared so the fixtures' RECORD hashes need no
/// hashing crate.
#[allow(dead_code)]
#[path = "../src/embed/sha256.rs"]
mod sha256;

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

/// Whether this run is a Windows GitHub Actions leg, where every
/// precondition must hold.
fn windows_required() -> bool {
    std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
}

/// The interpreter the venv is created from.
fn interpreter() -> OsString {
    std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3.14.exe".into())
}

/// Whether the tests run here: only when the interpreter is CPython
/// 3.14.7, and a CI leg panics when it is not.
fn hosted() -> bool {
    let python = interpreter();
    let version = Command::new(&python)
        .arg("--version")
        .output()
        .map(|output| stdout_of(&output).trim().to_string());
    match version {
        Ok(version) if version == "Python 3.14.7" => true,
        other if windows_required() => {
            panic!("the embed interpreter {python:?} is not CPython 3.14.7: {other:?}")
        }
        _ => false,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the parent");
    std::fs::write(path, bytes).expect("write a fixture file");
}

/// Unpadded urlsafe base64.
fn urlsafe_b64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut acc = 0u32;
        for (i, byte) in chunk.iter().enumerate() {
            acc |= u32::from(*byte) << (16 - 8 * i);
        }
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((acc >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// One RECORD line for `path` holding `bytes`.
fn record_line(path: &str, bytes: &[u8]) -> String {
    let hex = sha256::sha256_hex(bytes);
    let raw: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
        .collect();
    format!("{path},sha256={},{}\n", urlsafe_b64(&raw), bytes.len())
}

const PROGRAM: &str = "import tinypkg\n\nprint(str(tinypkg.f()))\n";

/// A venv with `tinypkg` installed the way `pip` does, and `main.py`.
struct Fixture {
    dir: PathBuf,
    python: PathBuf,
    site: PathBuf,
    _scratch: ScratchDir,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let scratch = ScratchDir::new(tag).expect("scratch");
        let dir = std::fs::canonicalize(&*scratch).expect("canonicalize");
        let venv = dir.join("venv");
        let status = Command::new(interpreter())
            .args(["-m", "venv", "--without-pip"])
            .arg(&venv)
            .status()
            .expect("spawn the base interpreter");
        assert!(status.success());
        let python = venv.join("Scripts").join("python.exe");
        let site = Command::new(&python)
            .args([
                "-c",
                "import sysconfig; print(sysconfig.get_path('purelib'))",
            ])
            .output()
            .expect("spawn the venv interpreter");
        let site = PathBuf::from(stdout_of(&site).trim());
        let fixture = Self {
            dir,
            python,
            site,
            _scratch: scratch,
        };
        fixture.install(&[]);
        std::fs::write(fixture.dir.join("main.py"), PROGRAM).expect("write the program");
        fixture
    }

    /// Writes `tinypkg` 1.0 plus `extra` payload files, with a RECORD
    /// that also lists two console scripts outside the site directory
    /// (one spelled with `\`, as some installers write them) and an `MZ`
    /// launcher inside the package.
    fn install(&self, extra: &[(&str, &[u8])]) {
        let mut files: Vec<(&str, &[u8])> = vec![
            (
                "tinypkg/__init__.py",
                b"from tinypkg.sub.mod import g\n\n\ndef f():\n    return g() * 2\n",
            ),
            ("tinypkg/sub/__init__.py", b""),
            ("tinypkg/sub/mod.py", b"def g():\n    return 21\n"),
            ("tinypkg/cli.exe", b"MZ\x90\x00 a test-written launcher"),
            ("../../Scripts/tinypkg.exe", b"MZ a console script"),
            ("..\\..\\Scripts\\tinypkg-gui.exe", b"MZ a gui script"),
            (
                "tinypkg-1.0.dist-info/METADATA",
                b"Metadata-Version: 2.1\nName: tinypkg\nVersion: 1.0\n",
            ),
            ("tinypkg-1.0.dist-info/INSTALLER", b"pip\n"),
        ];
        files.extend_from_slice(extra);
        let mut record = String::new();
        for (path, bytes) in files {
            write(&self.site.join(path), bytes);
            record.push_str(&record_line(path, bytes));
        }
        record.push_str("tinypkg-1.0.dist-info/RECORD,,\n");
        write(
            &self.site.join("tinypkg-1.0.dist-info/RECORD"),
            record.as_bytes(),
        );
    }

    fn pycc(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pycc"))
            .args(args)
            .current_dir(&self.dir)
            .env("PYCC_PYTHON", &self.python)
            .output()
            .expect("pycc should spawn")
    }

    fn lock(&self) {
        let output = self.pycc(&["lock", "main.py"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    }
}

/// A scrubbed environment: only `SystemRoot`, and a `PATH` of the system
/// directories, so no other interpreter is reachable.
fn scrubbed(command: &mut Command) -> &mut Command {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let root = PathBuf::from(root);
    let path = std::env::join_paths([root.join("System32"), root.clone()]).expect("join PATH");
    command
        .env_clear()
        .env("SystemRoot", &root)
        .env("PATH", path)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir(to).expect("create the copy");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

/// Every file under `root`, by relative path, with its bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).expect("under root").to_path_buf();
                out.insert(rel, std::fs::read(&path).expect("read"));
            }
        }
    }
    out
}

/// (a) the Windows section is written with no native library; (b) the
/// build bundles the closure and matches the venv's CPython; (c) the
/// relocated executable still runs it with a scrubbed `PATH`.
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14.exe or PYCC_PYTHON on Windows; run with --include-ignored"]
fn a_windows_locked_closure_runs_from_the_sidecar_and_matches_cpython() {
    if !hosted() {
        return;
    }
    let fixture = Fixture::new("win_lock_closure");
    let dir = &fixture.dir;
    fixture.lock();
    let text = std::fs::read_to_string(dir.join("pycc.lock")).expect("the lock");
    assert!(text.contains("x86_64-pc-windows-msvc"), "{text}");
    assert!(!text.contains("[[target.native]]"), "{text}");
    // Plan risk R6: a Windows venv keeps purelib and platlib in one
    // `Lib\site-packages`, and the section must still record purelib.
    assert!(text.contains("site = \"purelib\""), "{text}");

    let output = fixture.pycc(&["build", "main.py", "-o", "app"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let closure = dir.join("app.pycc").join("closure");
    assert!(closure.join("tinypkg").join("sub").join("mod.py").is_file());
    assert!(closure.join("tinypkg").join("cli.exe").is_file());
    let oracle = Command::new(&fixture.python)
        .arg("main.py")
        .current_dir(dir)
        .output()
        .expect("CPython runs the program");
    assert_eq!(oracle.status.code(), Some(0), "{}", stderr_of(&oracle));
    assert_eq!(stdout_of(&oracle), "42\n");
    let embedded = Command::new(dir.join("app"))
        .output()
        .expect("the embedded executable runs");
    assert_eq!(embedded.status.code(), Some(0), "{}", stderr_of(&embedded));
    assert_eq!(stdout_of(&embedded), stdout_of(&oracle));

    let moved = ScratchDir::new("win_lock_moved").expect("scratch");
    std::fs::copy(dir.join("app"), moved.join("app")).expect("copy the stub");
    copy_tree(&dir.join("app.pycc"), &moved.join("app.pycc"));
    std::fs::rename(dir.join("venv"), dir.join("venv-moved")).expect("move the venv away");
    let run = scrubbed(&mut Command::new(moved.join("app")))
        .output()
        .expect("the relocated executable runs");
    assert_eq!(run.status.code(), Some(0), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), stdout_of(&oracle));
}

/// (d) a `.pyd` in the closure still locks, but the build refuses it
/// naming #1297, writing nothing for a fresh output and leaving an
/// earlier sidecar byte-identical.
#[test]
#[ignore = "needs CPython 3.14.7 as python3.14.exe or PYCC_PYTHON on Windows; run with --include-ignored"]
fn a_windows_closure_holding_a_pyd_locks_but_is_refused_by_the_build() {
    if !hosted() {
        return;
    }
    let fixture = Fixture::new("win_lock_pyd");
    let dir = &fixture.dir;
    fixture.lock();
    let output = fixture.pycc(&["build", "main.py", "-o", "app"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let before = snapshot(&dir.join("app.pycc"));

    fixture.install(&[("tinypkg/_speed.pyd", b"arbitrary bytes")]);
    fixture.lock();
    let output = fixture.pycc(&["build", "main.py", "-o", "fresh"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    let rendered = stderr_of(&output);
    assert!(rendered.contains("tinypkg/_speed.pyd"), "{rendered}");
    assert!(rendered.contains("#1297"), "{rendered}");
    assert!(!dir.join("fresh.pycc").exists());
    assert!(!dir.join("fresh").exists() && !dir.join("fresh.exe").exists());

    let output = fixture.pycc(&["build", "main.py", "-o", "app"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    assert_eq!(snapshot(&dir.join("app.pycc")), before);
}
