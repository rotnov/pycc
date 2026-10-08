//! Part 1 of #1092: a CPython object temporary is released by the
//! operation that consumes it, on the fallthrough and on every failure edge.
//!
//! Every shape below reads its operands out of a foreign stub module, so
//! each operand is a *produced* new reference (`object_release::is_produced`)
//! that the consumer must release exactly once. The probe compares
//! `sys.getrefcount` of every stub attribute before and after a loop of the
//! shapes, at module level and in a function body, at two trip counts. A
//! leak shows as a positive delta that grows with the trip count; an
//! over-release as a negative one, and as a crash once an object's count
//! reaches zero. The expected delta is exactly `0` for every attribute.
//!
//! **PEP 683.** Every measured attribute is mortal (instances of a stub
//! class, a function, a class, a `float`, a large `int`), for the reason
//! `tests/issue_1084_refcount_probe.rs` records: an immortal object's count
//! never moves and would make the probe prove nothing.
//!
//! The failure shapes raise a `ValueError` with no arguments while an
//! operand is held -- a later sibling's failure (`s.T[s.U.fail()]`) and the
//! consuming operation's own failure (`s.BAD[s.T]`) -- inside a function's
//! `try`, out of a function into the module body's `try`, inside a
//! module-level `try`, and uncaught out of a module body (the module-exec
//! failure return).
//!
//! The hosted tests are `#[ignore]`d and contribute no line coverage; the
//! Tier-1 `native-build-test` leg runs them with `cargo test --workspace --
//! --include-ignored`. The changed lines are covered by the unit tests in
//! `crates/pycc_codegen/src/object_release_tests.rs`.

use pycc_scratch::ScratchDir;
use std::path::Path;
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

/// The foreign stub. `T`, `U`, `V`, `ITEM`, `BAD` and `FALSY` are instances of
/// module-local classes, so every one of them is mortal.
const STUB: &str = "\
class Thing:
    def __init__(self, tag):
        self.tag = tag

    def __len__(self):
        return 3

    def __getitem__(self, i):
        return ITEM

    def __contains__(self, x):
        return True

    def __lt__(self, other):
        return ITEM

    def __bool__(self):
        return True

    def __iter__(self):
        return iter((1, 2))

    def m(self, a, k=None):
        return ITEM

    def fail(self):
        raise ValueError()


class Falsy:
    def __bool__(self):
        return False


class Bad:
    def __getitem__(self, i):
        raise ValueError()


def f(a, k=None):
    return ITEM


T = Thing('t')
U = Thing('u')
V = Thing('v')
ITEM = Thing('item')
BAD = Bad()
FALSY = Falsy()
SCALE = 2.5
BIG = 10 ** 9 + 7
";

/// The attributes whose reference counts the probe pins.
const MEASURED: &[&str] = &[
    "T", "U", "V", "ITEM", "BAD", "FALSY", "Thing", "f", "SCALE", "BIG",
];

/// Every consumer shape, once each. Indented by `indent` so it serves as
/// both a module-level loop body and a function's loop body.
fn shapes(indent: &str) -> String {
    [
        "len(s.T)",
        "s.T[0]",
        "s.T < s.U",
        "s.T in s.U",
        "s.T[1:2]",
        "isinstance(s.T, s.Thing)",
        "s.T.m(s.U)",
        "s.f(s.T)",
        "s.T.m(s.U, k=s.V)",
        "s.f(s.T, k=s.V)",
        "type(s.T)",
        "x = float(s.SCALE)",
        "y = bool(s.SCALE)",
        "n = int(s.BIG)",
        "t = str(s.BIG)",
        "z = not s.T",
        "if s.T:\n{i}    x = 1.5",
        "while s.FALSY:\n{i}    x = 1.5",
        "s.T.slot = s.U",
        "del s.T.slot",
        "c = [e for e in s.T if s.U]",
        "d = [k for k in range(2) if s.U]",
    ]
    .iter()
    .map(|line| format!("{indent}{}\n", line.replace("{i}", indent)))
    .collect()
}

/// The failure shapes inside one `try` each, every handler counting. A
/// list display is admitted only when bound to an `object` slot, which a
/// module-level loop body cannot declare, so `in_function` adds it.
fn failures(indent: &str, in_function: bool) -> String {
    let mut lines = vec![
        "s.T[s.U.fail()]",
        "s.T.m(s.U, s.V.fail())",
        "s.f(s.T, s.U.fail())",
        "s.T < s.V.fail()",
        "s.BAD[s.T]",
    ];
    if in_function {
        lines.push("lst: object = [s.T, s.U, s.V.fail()]");
    }
    lines
        .iter()
    .map(|line| {
        format!(
            "{indent}try:\n{indent}    {line}\n{indent}except ValueError:\n{indent}    caught += 1\n"
        )
    })
    .collect()
}

/// The module under test: the shapes and the failures in a module-level
/// loop of 200 trips, then the same in functions the host calls. A `for`
/// over an object is admitted only in a module body, so only the
/// module-level loop iterates `s.T` with a statement.
fn module_source() -> String {
    format!(
        "import pycc_t1092_stub as s\n\
         \n\
         x: float = 0.0\n\
         y: bool = False\n\
         n: int = 0\n\
         t: str = \"\"\n\
         z: bool = False\n\
         caught: int = 0\n\
         for i in range(200):\n\
         {top_shapes}\
         \x20   for w in s.T:\n\
         \x20       x = 1.5\n\
         {top_failures}\
         \n\
         \n\
         def body() -> None:\n\
         \x20   x: float = 0.0\n\
         \x20   y: bool = False\n\
         \x20   n: int = 0\n\
         \x20   t: str = \"\"\n\
         \x20   z: bool = False\n\
         {fn_shapes}\
         \n\
         \n\
         def run(trips: int) -> None:\n\
         \x20   i = 0\n\
         \x20   while i < trips:\n\
         \x20       body()\n\
         \x20       i += 1\n\
         \n\
         \n\
         def fails(trips: int) -> int:\n\
         \x20   caught = 0\n\
         \x20   i = 0\n\
         \x20   while i < trips:\n\
         {fn_failures}\
         \x20       i += 1\n\
         \x20   return caught\n\
         \n\
         \n\
         def escape() -> None:\n\
         \x20   s.BAD[s.T]\n\
         \n\
         \n\
         def escapes(trips: int) -> int:\n\
         \x20   caught = 0\n\
         \x20   i = 0\n\
         \x20   while i < trips:\n\
         \x20       try:\n\
         \x20           escape()\n\
         \x20       except ValueError:\n\
         \x20           caught += 1\n\
         \x20       i += 1\n\
         \x20   return caught\n\
         \n\
         \n\
         def total() -> int:\n\
         \x20   return caught\n",
        top_shapes = shapes("    "),
        top_failures = failures("    ", false),
        fn_shapes = shapes("    "),
        fn_failures = failures("        ", true),
    )
}

/// Writes the stub into the scratch root and `source` as `src/<module>.py`
/// (the stub must not sit beside the entry module; see
/// `tests/issue_1084_loop_shape.rs`), then builds the extension into the
/// scratch root.
fn build(tag: &str, module: &str, source: &str) -> ScratchDir {
    let dir = ScratchDir::new(tag).expect("scratch");
    std::fs::write(dir.join("pycc_t1092_stub.py"), STUB).expect("write the stub");
    std::fs::create_dir_all(dir.join("src")).expect("create the entry directory");
    let entry = dir.join("src").join(format!("{module}.py"));
    std::fs::write(&entry, source).expect("write the module");
    let build = pycc()
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    dir
}

fn python(dir: &Path, script: &str) -> Output {
    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .output()
        .expect("python3 should spawn");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(&run),
        stderr_of(&run)
    );
    run
}

/// A host prelude defining `deltas(step)`: runs `step`, then returns the
/// change in every measured attribute's reference count as one line.
fn prelude() -> String {
    format!(
        "import sys\n\
         sys.path.insert(0, '.')\n\
         import pycc_t1092_stub as s\n\
         names = {MEASURED:?}\n\
         def counts():\n\
         \x20   return [sys.getrefcount(getattr(s, k)) for k in names]\n\
         def deltas(step):\n\
         \x20   before = counts()\n\
         \x20   result = step()\n\
         \x20   after = counts()\n\
         \x20   return ' '.join(str(a - b) for a, b in zip(after, before)), result\n"
    )
}

/// One line of zeros, one per measured attribute.
fn zeros() -> String {
    vec!["0"; MEASURED.len()].join(" ")
}

/// Every consumer shape and every held-operand failure, at module level and
/// in a function body, leaves every attribute's count where it found it --
/// at two trip counts, so a per-trip error cannot hide behind a constant.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn every_consumer_releases_its_temporaries_exactly_once() {
    let dir = build("t1092_release", "pycc_t1092_mod", &module_source());
    let script = format!(
        "{prelude}\
         d, _ = deltas(lambda: __import__('pycc_t1092_mod'))\n\
         import pycc_t1092_mod as mod\n\
         print('import', d, mod.total())\n\
         for trips in (100, 300):\n\
         \x20   d, _ = deltas(lambda: mod.run(trips))\n\
         \x20   print('run', trips, d)\n\
         \x20   d, r = deltas(lambda: mod.fails(trips))\n\
         \x20   print('fails', trips, d, r)\n\
         \x20   d, r = deltas(lambda: mod.escapes(trips))\n\
         \x20   print('escapes', trips, d, r)\n",
        prelude = prelude()
    );
    let out = stdout_of(&python(&dir, &script));
    let z = zeros();
    assert_eq!(
        out,
        format!(
            "import {z} 1000\n\
             run 100 {z}\n\
             fails 100 {z} 600\n\
             escapes 100 {z} 100\n\
             run 300 {z}\n\
             fails 300 {z} 1800\n\
             escapes 300 {z} 300\n"
        )
    );
}

/// An uncaught failure in a module body leaves through the module-exec
/// failure return, which releases the operands held at that point: a
/// failed import moves no count, however many times it is retried.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_failed_module_body_releases_its_held_operands() {
    let dir = build(
        "t1092_exec_fail",
        "pycc_t1092_boom",
        "import pycc_t1092_stub as s\n\nlen(s.T)\ns.BAD[s.U]\n",
    );
    let script = format!(
        "{prelude}\
         def attempt():\n\
         \x20   caught = 0\n\
         \x20   for _ in range(50):\n\
         \x20       try:\n\
         \x20           import pycc_t1092_boom\n\
         \x20       except ValueError:\n\
         \x20           caught += 1\n\
         \x20       sys.modules.pop('pycc_t1092_boom', None)\n\
         \x20   return caught\n\
         d, caught = deltas(attempt)\n\
         print(d, caught)\n",
        prelude = prelude()
    );
    assert_eq!(
        stdout_of(&python(&dir, &script)),
        format!("{} 50\n", zeros())
    );
}
