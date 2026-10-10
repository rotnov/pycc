//! #1517 (Part 3 of #1514): in a `pycc build --ext` artifact, a positional
//! method call on a CPython object, `o.m(args)`, no longer builds a bound
//! method only to release it after the call. For an exact builtin receiver
//! whose method is a method descriptor (`list.append`, `dict.get`,
//! `str.split`) the shim calls the unbound descriptor with the receiver
//! prepended, which is CPython's own `LOAD_ATTR`-method form; every other
//! receiver keeps `getattr(o, "m")(args)`. The lookup still runs before the
//! arguments are evaluated, which is CPython's order and the reason the
//! call is not `PyObject_VectorcallMethod` (`docs/RUNTIME.md`).
//!
//! Every test compares the artifact's stdout with CPython's for the same
//! source: the fast path on builtin receivers (including a site that sees
//! several receiver types), and on the generic path every shape the fast
//! path must not change -- an instance attribute shadowing a method, a
//! callable in a slot, `staticmethod`/`classmethod`, a property and a
//! `__getattr__` with side effects and their order relative to the
//! arguments, a missing method whose arguments must not be evaluated, an
//! argument that rebinds the method, an argument that drops the last other
//! reference to the receiver, CPython's own error text, and a user class
//! that stays collectable after a call site has seen it. A second test pins
//! that the fast path is an explicit allowlist of exact core builtin types:
//! `range` and `memoryview` receivers take the generic path.
//!
//! Every test here is hosted, so `#[ignore]`d and contributing no line
//! coverage; the Tier-1 `native-build-test` leg runs them with `cargo test
//! --workspace -- --include-ignored`. The changed codegen lines are covered
//! by `crates/pycc_codegen/src/foreign_call/tests/method_tests.rs` and the
//! other `pycc_codegen` unit tests, which also pin the IR shape (no
//! `pycc_ext_obj_getattr` and no `pycc_ext_obj_call` on the positional
//! path).

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

/// Builds `body` (already written to `source`) as the extension `m` in
/// `compiled_dir`.
fn build_ext(source: &Path, compiled_dir: &Path) {
    let build = pycc()
        .arg("build")
        .arg(source)
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
}

/// Builds `body` as the extension `m`, runs `script` against it and against
/// CPython importing the same source, and asserts both succeed with the same
/// stdout, which is returned.
fn assert_matches_cpython(tag: &str, body: &str, script: &str) -> String {
    let compiled_dir = ScratchDir::new(tag).expect("scratch");
    let source_dir = ScratchDir::new(&format!("{tag}_src")).expect("scratch");
    let source = write(&source_dir, "m.py", body);
    build_ext(&source, &compiled_dir);
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

/// The compiled module: positional method calls on CPython objects in
/// functions, in a loop, inside a `try`, and in the module body.
const MODULE: &str = "from typing import Any\n\
import gc\n\
import string\n\
ACC = string.digits.split('5')\n\
ACC.append(1)\n\
ACC.append('two')\n\
ONES = ACC.count(1)\n\
ENABLED = gc.isenabled()\n\
def acc() -> Any:\n    return ACC\n\
def ones() -> Any:\n    return ONES\n\
def enabled() -> Any:\n    return ENABLED\n\
def fill(o: Any, n: int) -> Any:\n    for i in range(n):\n        o.append(i)\n    return o\n\
def call0(o: Any) -> Any:\n    return o.m()\n\
def call1(o: Any, a: Any) -> Any:\n    return o.m(a)\n\
def count(o: Any, x: Any) -> Any:\n    return o.count(x)\n\
def get(o: Any, k: Any, d: Any) -> Any:\n    return o.get(k, d)\n\
def split(s: Any, sep: str) -> Any:\n    return s.split(sep)\n\
def ordered(o: Any, f: Any) -> Any:\n    return o.m(f('arg'))\n\
def missing(o: Any, f: Any) -> Any:\n    return o.nope(f('arg'))\n\
def guarded(o: Any, f: Any) -> str:\n    try:\n        o.nope(f('arg'))\n    except AttributeError:\n        return 'caught'\n    return 'not raised'\n\
def drop(h: Any, f: Any) -> Any:\n    return h[0].count(f(h))\n\
def append0(o: Any) -> Any:\n    return o.append()\n\
def append2(o: Any) -> Any:\n    return o.append(1, 2)\n\
def index(o: Any, x: Any) -> Any:\n    return o.index(x)\n\
def with_tb(e: Any) -> Any:\n    return e.with_traceback(None)\n\
def mro(t: Any) -> Any:\n    return t.mro()\n";

/// Exercises every shape against the module above; each line is compared
/// with CPython's own output for the same source.
const SCRIPT: &str = "import m\n\
log = []\n\
def f(x):\n    log.append(('f', x))\n    return x\n\
def show(thunk):\n\
\x20   try:\n        return repr(thunk())\n\
\x20   except Exception as e:\n        return type(e).__name__ + ': ' + str(e)\n\
print(m.acc(), m.ones(), m.enabled())\n\
print(m.fill([], 5), m.fill([9], 0))\n\
print(m.count([1, 2, 1], 1), m.count((1, 1, 1), 1), m.count('banana', 'a'))\n\
class L(list):\n    def count(self, x):\n        return 'overridden'\n\
print(m.count(L([1]), 1), m.count([1, 1], 1), m.count(b'aa', 97))\n\
print(m.get({'a': 1}, 'a', 0), m.get({}, 'a', 'dflt'))\n\
class D(dict):\n    pass\n\
d = D(a=2)\n\
print(m.get(d, 'a', 0))\n\
print(m.split('a,b,c', ','))\n\
class Shadow:\n    def m(self, a=None):\n        return 'class'\n\
s = Shadow()\n\
s.m = lambda *a: ('instance', a)\n\
print(m.call0(s), m.call1(s, 1), m.call0(Shadow()))\n\
class Slot:\n    __slots__ = ('m',)\n\
o = Slot()\n\
o.m = lambda *a: ('slot', a)\n\
print(m.call1(o, 2))\n\
print(show(lambda: m.call0(Slot())))\n\
class SC:\n    @staticmethod\n    def m(*a):\n        return ('static', a)\n\
class CM:\n    @classmethod\n    def m(cls, *a):\n        return ('class', cls.__name__, a)\n\
print(m.call1(SC(), 3), m.call1(SC, 4), m.call1(CM(), 5), m.call1(CM, 6))\n\
class P:\n    @property\n    def m(self):\n        log.append(('property',))\n        return lambda a: ('called', a)\n\
log.clear()\n\
print(m.ordered(P(), f), log)\n\
class G:\n    def __getattr__(self, name):\n        log.append(('getattr', name))\n        if name == 'm':\n            return lambda a: ('got', a)\n        raise AttributeError(name)\n\
log.clear()\n\
print(m.ordered(G(), f), log)\n\
log.clear()\n\
print(show(lambda: m.missing(G(), f)), log)\n\
log.clear()\n\
print(show(lambda: m.missing([], f)), log)\n\
print(show(lambda: m.missing(object(), f)), log)\n\
print(m.guarded([], f), m.guarded(G(), f), log)\n\
class R:\n    def m(self, a):\n        return ('old', a)\n\
def rebind(x):\n    R.m = lambda self, a: ('new', a)\n    return x\n\
print(m.ordered(R(), rebind), m.ordered(R(), f))\n\
def clear(h):\n    h.clear()\n    return 1\n\
print(m.drop([[1, 1, 2]], clear))\n\
print(show(lambda: m.append0([])))\n\
print(show(lambda: m.append2([])))\n\
print(show(lambda: m.index([1, 2], 3)), m.index([1, 2], 2))\n\
e = ValueError('x')\n\
e.with_traceback = lambda tb: 'shadowed'\n\
print(m.with_tb(e), type(m.with_tb(KeyError('k'))).__name__)\n\
print(m.mro(bool))\n\
print(show(lambda: m.call0(None)))\n\
print(show(lambda: m.call0(1)))\n\
import gc, weakref\n\
class W:\n    def m(self):\n        return 'w'\n\
w = W()\n\
print(m.call0(w))\n\
r = weakref.ref(W)\n\
del w, W\n\
gc.collect()\n\
print('class collected', r() is None)\n";

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_positional_method_call_without_a_bound_method_behaves_as_in_cpython() {
    let stdout = assert_matches_cpython("1517_method_call", MODULE, SCRIPT);
    // A few pinned lines, so a shared regression in the oracle (the same
    // source run by CPython) cannot hide a wrong order or a wrong callable.
    assert!(
        stdout.contains("('got', 'arg') [('getattr', 'm'), ('f', 'arg')]\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("AttributeError: nope [('getattr', 'nope')]\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("AttributeError: 'list' object has no attribute 'nope' []\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("('old', 'arg') ('new', 'arg')\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("('called', 'arg') [('property',), ('f', 'arg')]\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("('instance', ()) ('instance', (1,)) class\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("TypeError: list.append() takes exactly one argument (0 given)\n"),
        "{stdout}"
    );
    // A call site's cached negative answer only names a heap class, so the
    // class stays collectable once the site has seen it.
    assert!(stdout.ends_with("w\nclass collected True\n"), "{stdout}");
}

/// The fast path is an explicit allowlist of exact core builtin types
/// (`docs/RUNTIME.md`, #1517): every other receiver type -- here `range`
/// and `memoryview`, static immutable types with generic attribute access
/// and no instance `__dict__`, which a structural test would have admitted
/// -- takes the generic `getattr(o, name)` path.
///
/// The path taken is observable through the descriptor's reference count:
/// only a call site's positive (fast-path) entry owns a reference to the
/// unbound descriptor, and keeps it for the life of the process, so after
/// one call `list.count` gains exactly one reference while `range.count` and
/// `memoryview.tolist` gain none (their bound method is released after each
/// call). The results themselves are compared with CPython's.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_receiver_type_outside_the_allowlist_takes_the_generic_path() {
    let body = "from typing import Any\n\
def lcount(o: Any, x: Any) -> Any:\n    return o.count(x)\n\
def rcount(o: Any, x: Any) -> Any:\n    return o.count(x)\n\
def tolist(o: Any) -> Any:\n    return o.tolist()\n";
    let script = "import sys, m\n\
def delta(descr, thunk):\n\
\x20   before = sys.getrefcount(descr)\n\
\x20   result = [thunk() for _ in range(3)]\n\
\x20   return result, sys.getrefcount(descr) - before\n\
print('range', *delta(range.__dict__['count'], lambda: m.rcount(range(5), 3)))\n\
print('memoryview', *delta(memoryview.__dict__['tolist'], lambda: m.tolist(memoryview(b'ab'))))\n\
print('list', *delta(list.__dict__['count'], lambda: m.lcount([3, 3], 3)))\n";
    let compiled_dir = ScratchDir::new("1517_allowlist").expect("scratch");
    let source_dir = ScratchDir::new("1517_allowlist_src").expect("scratch");
    let source = write(&source_dir, "m.py", body);
    build_ext(&source, &compiled_dir);
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
    assert_eq!(
        stdout_of(&oracle),
        "range [1, 1, 1] 0\nmemoryview [[97, 98], [97, 98], [97, 98]] 0\nlist [2, 2, 2] 0\n"
    );
    assert_eq!(
        stdout_of(&compiled),
        "range [1, 1, 1] 0\nmemoryview [[97, 98], [97, 98], [97, 98]] 0\nlist [2, 2, 2] 1\n"
    );
}
