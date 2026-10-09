//! Part 3 of #1499 (#1503): an object-typed conditional expression or
//! value-selecting `and`/`or` whose arms mix a produced value with a
//! borrowed one owns its result, so its consumer releases it exactly once
//! whichever arm was selected.
//!
//! **The oracle is CPython itself.** Each module below is also run as plain
//! Python source, and the probe expects the compiled extension to print
//! what CPython prints: the `sys.getrefcount` delta of a *mortal* probe
//! object (PEP 683, see `tests/issue_1084_refcount_probe.rs`) after a loop
//! of the shapes, at two trip counts. Before Part 3 a mixed node was not a
//! producer, so the produced arm leaked one reference per trip whenever it
//! was selected and the delta grew with the trip count; a borrowed arm
//! released without a retain would instead drive it negative.
//!
//! The hosted tests are `#[ignore]`d and contribute no line coverage; the
//! Tier-1 `native-build-test` leg runs them with `cargo test --workspace --
//! --include-ignored`. The changed lines are covered by the unit tests in
//! `crates/pycc_codegen/src/object_release_operand_tests.rs`.

use pycc_scratch::ScratchDir;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn pycc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pycc"))
}

fn host_python() -> Command {
    let mut python =
        Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()));
    python.env("PYTHONIOENCODING", "utf-8");
    python
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

fn python(dir: &Path, script: &str) -> Output {
    host_python()
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .output()
        .expect("python3 should spawn")
}

fn assert_ok(run: &Output) {
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        stdout_of(run),
        stderr_of(run)
    );
}

/// Builds `body` as an extension, imports it, runs the same source under
/// CPython, asserts both printed the same thing and returns it.
fn assert_matches_cpython(tag: &str, module: &str, body: &str) -> String {
    let dir = ScratchDir::new(tag).expect("scratch");
    let build = pycc()
        .arg("build")
        .arg(write(&dir, "m.py", body))
        .arg("-o")
        .arg(dir.join(module))
        .arg("--ext")
        .output()
        .expect("pycc should spawn");
    assert!(build.status.success(), "{}", stderr_of(&build));
    let compiled = python(&dir, &format!("import {module}"));
    assert_ok(&compiled);
    let oracle = python(&dir, "import runpy\nrunpy.run_path('m.py')");
    assert_ok(&oracle);
    assert_eq!(stdout_of(&compiled), stdout_of(&oracle));
    stdout_of(&compiled)
}

/// Every mixed shape selects the produced arm (`src[0]`, a new reference
/// to `probe`) on one pass and the borrowed arm (`probe` itself) on the
/// other, and hands the result to a foreign call argument, which releases
/// it; `probe or 1` and `probe and 0` mix a borrowed arm with a boxed
/// native one. An all-borrowed shape stays a borrow with no traffic, so it
/// moves nothing even where its consumer does not release yet: a function
/// local (`bind`'s `y`, `z`, `w`, read once `bind` has returned, when
/// CPython has released them), a compiled function's argument and its
/// returned value (`ident`, #1502). Then the module global `g` is rebound to each
/// mixed shape's result in turn, releasing the previous value (Part 1 of
/// #1499). The probe delta must be `0`, as it is under CPython.
const BODY: &str = "import builtins\n\
    import sys\n\
    \n\
    probe = builtins.object()\n\
    src = builtins.list(builtins.dict.fromkeys(builtins.range(1), probe).values())\n\
    g = probe\n\
    \n\
    \n\
    def ident(o: object) -> object:\n    \
    return o\n\
    \n\
    \n\
    def bind(flag: bool) -> None:\n    \
    y = probe or probe\n    \
    z = probe if flag else probe\n    \
    w = ident(probe if flag else probe)\n\
    \n\
    \n\
    def pin(n: int, flag: bool) -> None:\n    \
    before = int(sys.getrefcount(probe))\n    \
    for i in range(n):\n        \
    builtins.id(src[0] if flag else probe)\n        \
    builtins.id(probe if flag else src[0])\n        \
    builtins.id(src[0] or probe)\n        \
    builtins.id(probe or src[0])\n        \
    builtins.id(probe and src[0])\n        \
    builtins.id(src[0] and probe)\n        \
    builtins.id(probe if flag else probe)\n        \
    builtins.id(probe or probe)\n        \
    builtins.id(probe or 1)\n        \
    builtins.id(probe and 0)\n        \
    bind(flag)\n        \
    ident(probe or probe)\n    \
    print(n, flag, int(sys.getrefcount(probe)) - before)\n\
    \n\
    \n\
    for n in range(100, 300, 100):\n    \
    pin(n, True)\n    \
    pin(n, False)\n    \
    before = int(sys.getrefcount(probe))\n    \
    for i in range(n):\n        \
    g = src[0] if n else probe\n        \
    g = probe if n else src[0]\n        \
    g = src[0] or probe\n        \
    g = probe and src[0]\n        \
    g = probe or probe\n    \
    g = probe\n    \
    print(n, int(sys.getrefcount(probe)) - before)\n";

#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_mixed_arm_object_selection_releases_whichever_arm_it_selected() {
    let out = assert_matches_cpython("obj_select_pin", "pycc_obj_select_pin", BODY);
    assert_eq!(
        out,
        "100 True 0\n100 False 0\n100 0\n200 True 0\n200 False 0\n200 0\n"
    );
}
