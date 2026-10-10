//! #1515 (Part 1 of #1514): in a `pycc build --ext` artifact, an attribute
//! load or method lookup on a CPython object passes CPython an *interned*
//! `str` for the name, cached in one per-name slot of the module, instead of
//! building a fresh `str` from a C string on every load
//! (`PyObject_GetAttrString`). The fresh string cost an allocation, a hash
//! and a release per load, and was never pointer-equal to the dict key it
//! looked up, which made the conversion the largest single cost of the
//! #1207 lark subject's parse loop (`docs/TESTING.md`).
//!
//! Every test here is hosted, so `#[ignore]`d and contributing no line
//! coverage; the Tier-1 `native-build-test` leg runs them with `cargo test
//! --workspace -- --include-ignored`. The changed codegen lines are covered
//! by `crates/pycc_codegen/src/foreign_attr.rs`'s unit tests, which also pin
//! the IR shape (the slot argument, one slot per name).

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
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
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

/// Builds `body` as the extension `m`, runs `script` against it and against
/// CPython importing the same source, and asserts both succeed with the same
/// stdout, which is returned.
fn assert_matches_cpython(tag: &str, body: &str, script: &str) -> String {
    let compiled_dir = ScratchDir::new(tag).expect("scratch");
    let source_dir = ScratchDir::new(&format!("{tag}_src")).expect("scratch");
    let source = write(&source_dir, "m.py", body);
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
    let compiled = run_in(&compiled_dir, script);
    let oracle = run_in(&source_dir, script);
    for (what, run) in [("pycc", &compiled), ("cpython", &oracle)] {
        assert!(
            run.status.success(),
            "{what}: {}{}",
            stdout_of(run),
            stderr_of(run)
        );
    }
    assert_eq!(stdout_of(&compiled), stdout_of(&oracle));
    stdout_of(&compiled)
}

/// The compiled module under test: attribute loads and method lookups on a
/// CPython object, in a function and in the module body, including one that
/// fails and one that fails inside a `try`.
const MODULE: &str = "from typing import Any\n\
import keyword\n\
KWLIST = keyword.kwlist\n\
IS_KW = keyword.iskeyword('while')\n\
def kwlist() -> Any:\n    return KWLIST\n\
def is_kw() -> Any:\n    return IS_KW\n\
def read(o: Any) -> Any:\n    return o.value\n\
def call(o: Any) -> Any:\n    return o.get()\n\
def missing(o: Any) -> Any:\n    return o.nope\n\
def guarded(o: Any) -> str:\n    try:\n        x = o.nope\n    except AttributeError:\n        return 'caught'\n    return 'not raised'\n";

/// A host class whose `__getattribute__` records, for every name CPython
/// hands it, whether that name object is the interned one. CPython's own
/// bytecode always passes the interned constant; before #1515 the artifact
/// passed a fresh, uninterned `str` per load, so this is the observable
/// regression check that every load site now passes the interned name -- and
/// the same object on every load.
const SCRIPT: &str = "import sys\nimport m\n\
seen = []\n\
class Rec:\n    value = 7\n\
\x20   def get(self):\n        return 'got'\n\
\x20   def __getattribute__(self, name):\n\
\x20       seen.append((name, name is sys.intern(name), id(name)))\n\
\x20       return object.__getattribute__(self, name)\n\
r = Rec()\n\
print(m.read(r), m.read(r), m.call(r), m.call(r))\n\
print([(n, interned) for n, interned, _ in seen])\n\
print(len({i for n, _, i in seen if n == 'value'}), len({i for n, _, i in seen if n == 'get'}))\n\
print('if' in m.kwlist(), m.is_kw())\n\
try:\n    m.missing(r)\nexcept AttributeError as e:\n    print(type(e).__name__, e)\n\
print(m.guarded(r))\n\
print(m.read(r))\n";

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn attribute_names_reach_cpython_interned_and_behave_as_in_cpython() {
    let stdout = assert_matches_cpython("1515_interned_names", MODULE, SCRIPT);
    assert_eq!(
        stdout,
        "7 7 got got\n\
         [('value', True), ('value', True), ('get', True), ('get', True)]\n\
         1 1\n\
         True True\n\
         AttributeError 'Rec' object has no attribute 'nope'\n\
         caught\n\
         7\n"
    );
}
