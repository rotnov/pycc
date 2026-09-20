//! Part 2a of #1142 (#1165): artifact-owned buffer storage, exercised end
//! to end through the real `pycc build --ext` CLI and a hosted CPython run.
//!
//! Every `Ty::MemoryView` before this part was a view the generated wrapper
//! borrowed from the host for the duration of one call. `a = ndarray(n)`
//! adds the second provenance: storage the artifact allocates, uses, and
//! frees inside the allocating frame, with nothing able to carry it out.
//!
//! This file owns the halves no in-crate unit test can reach:
//!
//! * the diagnostics a real program gets when the producer appears anywhere
//!   but an assignment's right-hand side, at module scope, or with a
//!   non-`int` length;
//! * the native-mode refusal, which is a property of the artifact mode and
//!   so needs the driver rather than a crate;
//! * the hosted arms that prove a compiled allocation is real memory the
//!   compiled code can store into and read back;
//! * and the leak arm, which reads `pycc_rt_buffer_live_views` -- the
//!   allocator pair's own balance counter -- out of the built extension
//!   module with `ctypes` and asserts it is back at zero after every call.
//!   That symbol is reachable because `pycc_rt` links into the module as a
//!   staticlib; asserting on it is what distinguishes "the epilogue ran"
//!   from "the process had enough memory not to notice".
//!
//! None of these contributes line coverage: CI's coverage job runs
//! `llvm-cov` without `--include-ignored` and `scripts/check_diff_coverage.py`
//! excludes `tests/` from its denominator either way.

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

/// Writes `source` as the entry module of a fresh scratch directory.
fn fixture(category: &str, source: &str) -> ScratchDir {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("alloc_probe.py"), source).expect("write the subject");
    dir
}

/// Builds the entry module as a CPython extension module directly into
/// `dir`, so a CPython run with `dir` as its working directory imports it.
fn build_ext(dir: &Path) -> Output {
    pycc()
        .arg("build")
        .arg(dir.join("alloc_probe.py"))
        .arg("-o")
        .arg(dir.join("alloc_probe"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn")
}

/// Builds the entry module in the default `native` mode.
fn build_native(dir: &Path) -> Output {
    pycc()
        .arg("build")
        .arg(dir.join("alloc_probe.py"))
        .arg("-o")
        .arg(dir.join("alloc_probe_native"))
        .output()
        .expect("pycc should spawn")
}

/// Runs `program` under CPython with `dir` on the import path.
fn run_hosted(dir: &Path, program: &str) -> Output {
    Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(program)
        .current_dir(dir)
        .output()
        .expect("python3 should spawn")
}

/// The subject: one export that allocates, fills and sums its own storage,
/// and one that reallocates inside the same frame so the free-before-store
/// path (D-074) runs on a real artifact rather than only in codegen's own
/// unit test.
const SUBJECT: &str = "\
def build_and_sum(n: int) -> float:
    a = ndarray(n)
    i = 0
    while i < len(a):
        a[i] = 2.0
        i = i + 1
    s = 0.0
    j = 0
    while j < len(a):
        s = s + a[j]
        j = j + 1
    return s


def rebuild(n: int) -> float:
    a = ndarray(n)
    a = ndarray(n + 1)
    return float(len(a))
";

/// The `NDArray` spelling reaches the same producer, so the second
/// spelling is proven at the artifact level and not only in the checker.
const SUBJECT_NDARRAY: &str = "\
def size(n: int) -> float:
    a = NDArray(n)
    return float(len(a))
";

/// A producer outside an assignment's right-hand side is the named
/// position refusal, not an incidental type mismatch: the bare-statement
/// shape is the one that would otherwise type-check and leak one
/// allocation per call, because `ExprStmt` discards the inferred type.
#[test]
fn a_bare_producer_statement_is_the_named_position_refusal() {
    let dir = fixture(
        "1165_bare_statement",
        "\
def leak(n: int) -> None:
    ndarray(n)
",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[C0001]"), "{err}");
    assert!(
        err.contains("admitted only as the whole right-hand side of an assignment"),
        "{err}"
    );
}

/// The same refusal covers a call argument, which before this part fell out
/// only incidentally as a parameter-type mismatch.
#[test]
fn a_producer_in_an_argument_position_is_the_same_refusal() {
    let dir = fixture(
        "1165_argument",
        "\
def take(x: float) -> float:
    return x


def go(n: int) -> float:
    return take(ndarray(n))
",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[C0001]"), "{err}");
    assert!(
        err.contains("admitted only as the whole right-hand side of an assignment"),
        "{err}"
    );
}

/// Module scope gets its own refusal rather than the position one: the
/// ground differs, because a module frame has no epilogue to free the
/// allocation from.
#[test]
fn a_producer_at_module_scope_is_its_own_refusal() {
    let dir = fixture("1165_module_scope", "a = ndarray(4)\n");
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[C0001]"), "{err}");
    assert!(
        err.contains("artifact-owned buffer storage is freed when the allocating function returns"),
        "{err}"
    );
}

/// A non-`int` length is a typed refusal, not the position one.
#[test]
fn a_non_int_length_is_a_typed_refusal() {
    let dir = fixture(
        "1165_length_type",
        "\
def go(k: str) -> float:
    a = ndarray(k)
    return float(len(a))
",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[T0033]"), "{err}");
    assert!(err.contains("expects an `int` element count"), "{err}");
}

/// Reading an owned buffer beyond the three implemented operations is the
/// *owned* refusal, whose wording differs from the parameter one on
/// purpose: egress does not exist yet (Part 2b of #1142, #1164), whereas
/// handing back a borrowed view would be a use-after-free.
#[test]
fn using_owned_storage_beyond_the_implemented_operations_is_the_owned_refusal() {
    let dir = fixture(
        "1165_owned_use",
        "\
def go(n: int) -> float:
    a = ndarray(n)
    b = a
    return float(len(b))
",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[C0001]"), "{err}");
    assert!(
        err.contains("bound to buffer storage this `pycc build --ext` artifact allocated"),
        "{err}"
    );
}

/// A program that binds the spelling itself keeps its own meaning, which is
/// D-244's #1129 statement (h) applied to the call position. The subject
/// would be the position refusal if the producer won.
#[test]
fn a_program_that_defines_the_spelling_keeps_its_own_meaning() {
    let dir = fixture(
        "1165_shadowed",
        "\
def ndarray(n: int) -> float:
    return float(n)


def go(n: int) -> float:
    a = ndarray(n)
    return a
",
    );
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));
}

/// The native gate is a property of the artifact mode, so it needs the
/// driver. Its message states the `--ext` boundary rather than the missing
/// interpreter, because the allocation itself would link and run natively.
#[test]
fn allocating_buffer_storage_natively_is_refused_in_its_own_words() {
    let dir = fixture("1165_native", SUBJECT);
    let build = build_native(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[I0405]"), "{err}");
    assert!(
        err.contains("a native executable has no host to carry it to"),
        "{err}"
    );
}

/// The whole of Part 2a in one hosted run: the artifact allocates its own
/// storage, stores into it, reads it back, and frees it.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_artifact_allocates_fills_and_sums_its_own_storage() {
    let dir = fixture("1165_hosted_roundtrip", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = run_hosted(
        &dir,
        "import alloc_probe\n\
         assert alloc_probe.build_and_sum(4) == 8.0, alloc_probe.build_and_sum(4)\n\
         assert alloc_probe.build_and_sum(0) == 0.0\n\
         assert alloc_probe.rebuild(3) == 4.0, alloc_probe.rebuild(3)\n\
         print('ok')\n",
    );
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}

/// The `NDArray` spelling produces the same artifact behavior.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn the_ndarray_spelling_allocates_the_same_storage() {
    let dir = fixture("1165_hosted_ndarray", SUBJECT_NDARRAY);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = run_hosted(
        &dir,
        "import alloc_probe\n\
         assert alloc_probe.size(6) == 6.0, alloc_probe.size(6)\n\
         print('ok')\n",
    );
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}

/// A negative length raises `ValueError` through D-173's pending-exception
/// protocol rather than aborting, and the failed allocation leaves nothing
/// for the epilogue to free.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_negative_length_raises_value_error_at_the_boundary() {
    let dir = fixture("1165_hosted_negative", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = run_hosted(
        &dir,
        "import alloc_probe\n\
         try:\n\
         \x20   alloc_probe.build_and_sum(-1)\n\
         except ValueError as error:\n\
         \x20   assert 'negative' in str(error), str(error)\n\
         else:\n\
         \x20   raise AssertionError('a negative length returned normally')\n\
         print('ok')\n",
    );
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}

/// The leak arm. `pycc_rt_buffer_live_views` is the allocator pair's own
/// balance counter, linked into the module as part of the `pycc_rt`
/// staticlib and read back through `ctypes`. It must be zero before any
/// call, zero after an ordinary call, zero after the reallocating call --
/// which frees the first view before storing the second (D-074) -- and
/// zero after a call that raised, since the failed allocation returned
/// null and the epilogue must skip it rather than free it.
///
/// Asserting a *balance* rather than watching memory is the point: a frame
/// that never freed would show identical behavior on every other
/// observation in this file.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn no_allocation_outlives_the_call_that_made_it() {
    let dir = fixture("1165_hosted_live_views", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = run_hosted(
        &dir,
        "import ctypes, alloc_probe\n\
         live = ctypes.CDLL('./alloc_probe.abi3.so').pycc_rt_buffer_live_views\n\
         live.restype = ctypes.c_longlong\n\
         live.argtypes = []\n\
         assert live() == 0, live()\n\
         for _ in range(64):\n\
         \x20   alloc_probe.build_and_sum(16)\n\
         assert live() == 0, live()\n\
         for _ in range(64):\n\
         \x20   alloc_probe.rebuild(16)\n\
         assert live() == 0, live()\n\
         for _ in range(64):\n\
         \x20   try:\n\
         \x20       alloc_probe.build_and_sum(-1)\n\
         \x20   except ValueError:\n\
         \x20       pass\n\
         assert live() == 0, live()\n\
         print('ok')\n",
    );
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}
