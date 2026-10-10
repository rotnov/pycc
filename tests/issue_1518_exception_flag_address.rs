//! #1518 (Part 4 of #1514), item A: compiled code no longer calls
//! `pycc_rt_exception_active()` -- a `__tls_get_addr` call in a dlopen'd
//! `--ext` artifact -- after every operation that can raise. Each compiled
//! function asks `pycc_rt_exception_state()` once, in its entry block, for
//! the address of the calling thread's pending-exception flag, and every
//! later check is a byte load from that address (`docs/RUNTIME.md`'s "The
//! pending-exception check").
//!
//! The address is per thread, so the risk this change carries is a check
//! that reads another thread's flag. The test runs compiled functions that
//! raise and catch on every other iteration from four host threads at once,
//! with the switch interval at its minimum and a host call that releases the
//! GIL inside each `try`, so the threads interleave between a raise and its
//! check. It also covers an exception raised by a host callable and caught
//! in compiled code, a `finally` that runs a host call while an exception is
//! pending, and a compiled raise caught by the host. Every result is
//! compared with CPython running the same source. (Generators are refused in
//! `--ext` builds, so no resume function is exercised here; the codegen unit
//! test pins that every function looks the address up in its own entry
//! block.)
//!
//! The test is hosted, so `#[ignore]`d and contributing no line coverage;
//! the Tier-1 `native-build-test` leg runs it with `cargo test --workspace --
//! --include-ignored`. The changed lines are covered by
//! `crates/pycc_codegen/src/exception_check.rs`'s and
//! `crates/pycc_rt/src/exception/state.rs`'s unit tests and the other
//! `pycc_codegen` tests that compile `try`/`except`/`finally` shapes.

use pycc_scratch::ScratchDir;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn host_python() -> Command {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

fn write(dir: &Path, file: &str, body: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, body).expect("write the fixture source");
    path
}

fn artifact_name() -> &'static str {
    if cfg!(windows) { "m.pyd" } else { "m.abi3.so" }
}

fn run_in(dir: &Path, script: &str) -> Output {
    host_python()
        .args(["-B", "-c", script])
        .current_dir(dir)
        .env("PYTHONUNBUFFERED", "1")
        .output()
        .expect("python3 should spawn")
}

const MODULE: &str = "from typing import Any


def check(n: int) -> int:
    if n < 0:
        raise ValueError(\"negative\")
    return n


def count(f: Any, n: int) -> int:
    caught = 0
    for i in range(n):
        try:
            f()
            check(i % 2 - 1)
        except ValueError:
            caught += 1
    return caught


def relay(f: Any) -> int:
    try:
        f()
    except KeyError:
        return 1
    return 0


def nested(f: Any) -> int:
    try:
        try:
            check(-1)
        finally:
            f()
    except ValueError:
        return 2
    return 0
";

const SCRIPT: &str = "import m, sys, threading, time
sys.setswitchinterval(1e-6)
def pause():
    time.sleep(0)
res = [None] * 4
def worker(k):
    res[k] = m.count(pause, 300 + k)
ts = [threading.Thread(target=worker, args=(k,)) for k in range(4)]
for t in ts: t.start()
for t in ts: t.join()
print(res)
def raise_key():
    raise KeyError('k')
print(m.relay(raise_key), m.relay(lambda: None), m.nested(pause))
try:
    m.check(-5)
except ValueError as e:
    print('host', type(e).__name__, e)
print(m.check(3))
";

#[test]
#[ignore = "hosted: builds an --ext artifact and runs it under CPython"]
fn each_thread_checks_its_own_pending_exception_flag() {
    let compiled_dir = ScratchDir::new("issue_1518_flag").expect("scratch");
    let source_dir = ScratchDir::new("issue_1518_flag_src").expect("scratch");
    let source = write(&source_dir, "m.py", MODULE);
    let build = pycc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(compiled_dir.join("m"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(
        build.status.success(),
        "{}{}",
        stdout_of(&build),
        stderr_of(&build)
    );
    assert!(compiled_dir.join(artifact_name()).is_file());

    let compiled = run_in(&compiled_dir, SCRIPT);
    let oracle = run_in(&source_dir, SCRIPT);
    for (what, run) in [("pycc", &compiled), ("cpython", &oracle)] {
        assert!(
            run.status.success(),
            "{what}: {}{}",
            stdout_of(run),
            stderr_of(run)
        );
    }
    assert_eq!(stdout_of(&compiled), stdout_of(&oracle));
    assert_eq!(
        stdout_of(&compiled),
        "[150, 151, 151, 152]\n1 0 2\nhost ValueError negative\n3\n"
    );
}
