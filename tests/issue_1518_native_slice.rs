//! #1518 (Part 4 of #1514), item C: a slice of a CPython object with no
//! step and only pycc `int` bounds (`o[i:j]`, `o[-n:]`, `del o[i:]`) passes
//! the bound words to `pycc_ext_obj_getslice_int` or
//! `pycc_ext_obj_delslice_int`. Those shims slice an exact `list` or
//! `tuple` with `PyList_GetSlice`, `PyTuple_GetSlice` or `PyList_SetSlice`
//! over the range CPython's own slice resolves to, and pack the bounds for the
//! general helper in every other case (`docs/RUNTIME.md`'s "A step-less slice
//! with `int` bounds").
//!
//! The risk is a resolved range that differs from CPython's, so the test
//! loads every `(start, stop)` pair in `[-7, 7]` from a `list`, a `tuple`, a
//! `str`, `bytes`, a `range`, empty sequences, a `list` subclass and a custom
//! sequence that override `__getitem__`, and bases that raise. It also checks
//! the one-sided, step and temporary-bound forms, the identity of a `tuple`
//! sliced whole, and deletion from a `list`, a `bytearray`, the subclass, the
//! custom sequence and bases that refuse it. Every result is compared with
//! CPython running the same source.
//!
//! The test is hosted, so `#[ignore]`d and contributing no line coverage;
//! the Tier-1 `native-build-test` leg runs it with `cargo test --workspace --
//! --include-ignored`. The changed codegen lines are covered by
//! `crates/pycc_codegen/src/tests/object_membership_slice.rs`.

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

const MODULE: &str = r#"from typing import Any


def get(o: Any, i: int, j: int) -> Any:
    return o[i:j]


def head(o: Any, j: int) -> Any:
    return o[:j]


def tail(o: Any, i: int) -> Any:
    return o[i:]


def whole(o: Any) -> Any:
    return o[:]


def shifted(o: Any, i: int) -> Any:
    return o[i + 1:i + 3]


def stepped(o: Any, i: int) -> Any:
    return o[i::2]


def drop(o: Any, i: int, j: int) -> Any:
    del o[i:j]
    return o


def drop_tail(o: Any, n: int) -> Any:
    del o[-n:]
    return o


def drop_all(o: Any) -> Any:
    del o[:]
    return o
"#;

const SCRIPT: &str = r#"import copy
import m


class L(list):
    def __getitem__(self, k):
        return ('L.getitem', k.start, k.stop, k.step)

    def __delitem__(self, k):
        print('L.delitem', k)


class Seq:
    def __getitem__(self, k):
        return ('Seq', k)

    def __delitem__(self, k):
        print('Seq.del', k)

    def __repr__(self):
        return 'Seq()'


def show(f, *args):
    try:
        r = f(*args)
        print(type(r).__name__, r)
    except Exception as e:
        print(type(e).__name__, e)


bases = [[0, 1, 2, 3, 4], (0, 1, 2, 3, 4), 'abcde', b'abcde', range(5), [], (), L([1, 2, 3]), Seq(),
         {'a': 1}, 7, None]
def res(f, *args):
    try:
        r = f(*args)
        return type(r).__name__ + ' ' + repr(r)
    except Exception as e:
        return type(e).__name__ + ' ' + str(e)


for base in bases:
    grid = [res(m.get, base, i, j) for i in range(-7, 8) for j in range(-7, 8)]
    print('|'.join(grid))
    for i in range(-7, 8):
        print(res(m.head, base, i), res(m.tail, base, i), res(m.shifted, base, i), res(m.stepped, base, i))
    show(m.whole, base)
t = (1, 2, 3)
print(m.whole(t) is t, m.get(t, 0, 3) is t, m.get(t, -9, 9) is t, m.tail(t, 1) is t)
l = [1, 2, 3]
print(m.whole(l) is l, m.whole(l) == l)
for base in ([0, 1, 2, 3, 4], bytearray(b'abcde'), L([1, 2, 3]), Seq(), (1, 2), 'ab', {'a': 1}, range(3)):
    for i, j in ((1, 3), (-2, 9), (3, 1), (-9, -9), (0, 0), (-1, 5)):
        b = copy.copy(base)
        show(m.drop, b, i, j)
    for k in (0, 1, 2, 9):
        b = copy.copy(base)
        show(m.drop_tail, b, k)
    b = copy.copy(base)
    show(m.drop_all, b)
"#;

#[test]
#[ignore = "hosted: builds an --ext artifact and runs it under CPython"]
fn a_slice_with_int_bounds_matches_cpython_for_every_base_and_range() {
    let compiled_dir = ScratchDir::new("issue_1518_slice").expect("scratch");
    let source_dir = ScratchDir::new("issue_1518_slice_src").expect("scratch");
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
    let out = stdout_of(&compiled);
    // A whole exact `tuple` comes back as itself, a `list` as a copy.
    assert!(out.contains("True True True False\nFalse True\n"), "{out}");
    assert!(out.contains("L.delitem slice(1, 3, None)\n"), "{out}");
}
