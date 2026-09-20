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
//!   The library is opened by `alloc_probe.__file__`, the sibling
//!   `issue_1054_ext_str_release.rs` probe's convention, so the handle
//!   refers to the same mapping the import created. That arm is the one
//!   thing here that is **not** compiled on Windows, for the reason that
//!   file states in full: MSVC exports from a `.pyd` only the single
//!   `PyInit_<name>` a D-244 `ext` module declares, so a counter linked in
//!   from the `pycc_rt` static archive is present but unreachable through
//!   `ctypes`. Every other arm, diagnostics included, runs on Windows.
//!   That symbol is reachable because `pycc_rt` links into the module as a
//!   staticlib; asserting on it is what distinguishes "the epilogue ran"
//!   from "the process had enough memory not to notice". Its last arm is
//!   the only place a *live* allocation leaves through the exception exit.
//!
//! Every non-`#[ignore]`d test here asserts a front-end diagnostic, and
//! deliberately so: a successful `--ext` build needs CPython development
//! headers at or above the `Py_LIMITED_API` floor, which CI's coverage job
//! does not have when it runs. See
//! `a_program_that_defines_the_spelling_keeps_its_own_meaning` for the one
//! property that had to be restated as a refusal to keep it in that pass.
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


def alloc_then_raise(n: int) -> float:
    a = ndarray(n)
    b = ndarray(-1)
    return a[0] + b[0]
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
/// D-244's #1129 statement (h) applied to the call position.
///
/// Written as a *refusal* rather than a successful build on purpose. A
/// successful `--ext` build links against CPython development headers, and
/// CI's coverage job probes the interpreter only in `src/main.rs`'s ext
/// emission -- after type checking -- so a success-shaped subject is the one
/// arm in this file that cannot run on a runner whose interpreter predates
/// the `Py_LIMITED_API` floor. Every other non-`#[ignore]`d test here
/// asserts a front-end diagnostic and passes there unchanged, so the
/// shadowing seam is proven the same way.
///
/// `a[0]` is what discriminates the two interpretations. If the producer
/// wrongly hijacked the name, `a` would be `Ty::MemoryView` and `a[0]` a
/// legal element read; because the program's own `def` wins, `a` is a
/// `float` and subscripting it is a typed refusal. The same subject without
/// the user's `def ndarray` builds cleanly, which is the counterfactual this
/// diagnostic stands in for.
#[test]
fn a_program_that_defines_the_spelling_keeps_its_own_meaning() {
    let dir = fixture(
        "1165_shadowed",
        "\
def ndarray(n: int) -> float:
    return float(n)


def go(n: int) -> float:
    a = ndarray(n)
    return a[0]
",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[T0033]"), "{err}");
    assert!(err.contains("`float` does not support indexing"), "{err}");
}

/// #1166 review finding F2: statement (h) covers a binding the *function*
/// makes, not only one the module makes.
///
/// `local_names` is a whole-body pre-pass, so CPython makes `ndarray` local
/// throughout `go` and the earlier call raises `UnboundLocalError`. Before
/// the fix the producer's statement-(h) guard consulted only the class,
/// generic, function and module-binding tables -- none of which holds a local
/// that the walk has not reached yet -- so the call was recognized as a
/// producer and the frame allocated. Both walkers now decline, and the
/// solver's own `is_local` gate reports the `UnboundLocalError` analogue.
#[test]
fn a_function_local_rebinding_of_the_spelling_wins_over_the_producer() {
    let dir = fixture(
        "1165_local_shadow",
        "\
def go() -> float:
    a = ndarray(4)
    ndarray = 1
    return float(ndarray)
",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[T0021]"), "{err}");
    assert!(
        err.contains("local name `ndarray` is not bound before this use"),
        "{err}"
    );
}

/// #1166 review finding F3: a `bool` length is an `int` length.
///
/// `docs/TYPE_SYSTEM.md`'s representation table makes `bool` a subtype of
/// `int` (rule 4/D-086), the same rule that already admits a `bool` buffer
/// *index*, and `MirExpr::BufferAlloc`'s codegen already decodes one -- the
/// length goes through `to_numeric_encoded_int`, whose `Scalar::Bool` arm
/// zero-extends and re-tags before the shared checked untag. The `T0033` this
/// used to raise refused a one-element request the whole pipeline supports.
///
/// Written as the *owned-use* refusal for
/// `a_program_that_defines_the_spelling_keeps_its_own_meaning`'s reason: a
/// successful `--ext` build needs CPython development headers. `b = a` is
/// what discriminates the two interpretations -- that message is reachable
/// only if `a` is bound to artifact-owned buffer storage, so reaching it
/// proves `ndarray(True)` was admitted as the producer.
#[test]
fn a_bool_length_is_admitted_as_a_one_element_request() {
    let dir = fixture(
        "1165_bool_length",
        "\
def go() -> float:
    a = ndarray(True)
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
    assert!(!err.contains("error[T0033]"), "{err}");
}

/// #1166 review finding F4: a value-less module-level annotation is not a
/// binding, so it must not disarm the native gate.
///
/// `HirStmt::AnnAssign`'s `value` is an `Option`, and `ndarray: int` alone
/// binds nothing at run time -- the check phase records it in `declared`, not
/// in `Environment::bindings`, so `buffer::producer_assignment_ty` still
/// recognizes the producer. The native gate's shadow set nevertheless counted
/// the bare annotation, skipped its refusal, and let an artifact-owned buffer
/// allocation reach a **native** executable past the documented `--ext`-only
/// boundary. This is the counterpart of
/// `allocating_buffer_storage_natively_is_refused_in_its_own_words`, whose
/// subject carries no such annotation.
#[test]
fn a_value_less_module_annotation_does_not_disarm_the_native_gate() {
    let dir = fixture(
        "1165_native_bare_annotation",
        "\
ndarray: int


def go(n: int) -> float:
    a = ndarray(n)
    return a[0]
",
    );
    let build = build_native(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[I0405]"), "{err}");
    assert!(
        err.contains("a native executable has no host to carry it to"),
        "{err}"
    );
}

/// The load-bearing negative for the test above: an annotation that *does*
/// carry an initializer is a real binding, so it still keeps the program's
/// own meaning and the native gate stays silent about it. `T0021` rather than
/// `I0405` is the whole assertion -- calling an `int` is the program's own
/// error, which is exactly statement (h) working.
#[test]
fn an_initialized_module_annotation_still_keeps_the_programs_own_meaning() {
    let dir = fixture(
        "1165_native_initialized_annotation",
        "\
ndarray: int = 3


def go(n: int) -> float:
    a = ndarray(n)
    return a[0]
",
    );
    let build = build_native(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(!err.contains("error[I0405]"), "{err}");
    assert!(err.contains("error[T0021]"), "{err}");
    assert!(
        err.contains("name `ndarray` is bound to a non-callable value"),
        "{err}"
    );
}

/// #1166 review finding: the native gate must honour statement (h)'s
/// *function-local* arm too, not only the module-wide one.
///
/// This is `a_function_local_rebinding_of_the_spelling_wins_over_the_producer`
/// built natively. The two type-checking recognizers already decline here, so
/// `--ext` and `pycc check` both report the program's own `UnboundLocalError`
/// analogue; the native gate, unaware of function-local names, instead
/// reported `I0405` -- whose remedy is "rebuild with `--ext`", which lands on
/// a different error about a different cause. A diagnostic that misidentifies
/// the defect and prescribes an inapplicable remedy is a defect of its own,
/// so the assertion is that native mode now reports the same `T0021` the
/// other two modes do, and no `I0405` at all.
#[test]
fn a_function_local_rebinding_keeps_its_own_meaning_natively_too() {
    let dir = fixture(
        "1165_native_local_shadow",
        "\
def go(n: int) -> float:
    a = ndarray(4)
    ndarray = 1
    return float(ndarray)
",
    );
    let build = build_native(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(!err.contains("error[I0405]"), "{err}");
    assert!(
        err.contains("local name `ndarray` is not bound before this use"),
        "{err}"
    );
}

/// The parameter arm of the same finding, which the body-local arm above does
/// not cover: `function_local_names` starts from the parameter list, and a
/// parameter is a binding from the first statement rather than one the
/// whole-body pre-pass has to reach. Calling an `int` parameter is the
/// program's own error in every mode, so the native gate must stay silent.
#[test]
fn a_parameter_named_for_the_spelling_keeps_its_own_meaning_natively() {
    let dir = fixture(
        "1165_native_param_shadow",
        "\
def go(ndarray: int) -> float:
    a = ndarray(4)
    return float(a)
",
    );
    let build = build_native(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(!err.contains("error[I0405]"), "{err}");
    assert!(
        err.contains("name `ndarray` is bound to a non-callable value"),
        "{err}"
    );
}

/// The function-local layer is per *spelling*, not per function: a body that
/// binds `ndarray` says nothing about `NDArray`, so the producer call on the
/// unshadowed spelling is still a real allocation and still refused. This
/// pins the shape the gate is built out of -- collapsing the per-spelling
/// filter into "this function binds some producer spelling" would silently
/// let this program through the gate while the checker still admits it.
#[test]
fn one_shadowed_spelling_does_not_disarm_the_other() {
    let dir = fixture(
        "1165_native_one_spelling_shadowed",
        "\
def go(n: int) -> float:
    ndarray = 1
    a = NDArray(4)
    return a[0]
",
    );
    let build = build_native(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[I0405]"), "{err}");
    assert!(
        err.contains("allocates buffer storage with `NDArray(n)`"),
        "{err}"
    );
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

/// #1166 review finding F5, corroborated on a real artifact: an allocation
/// reached with an exception *already* pending must not happen at all.
///
/// `xs.pop()` on an empty list raises, and `MirExpr::ListPop` is in
/// `expression_can_set_exception`'s `false` group, so nothing guarded the
/// pending state between that raise and the allocator call. The allocation
/// succeeded and `emit_expr`'s own post-call guard then branched to the
/// handler before `MirStmt::Assign` could store the pointer into the frame's
/// owned slot, leaving the slot at its entry null: one leaked view per call,
/// which the balance counter reports and no other observation in this file
/// would notice.
///
/// `#[ignore]`d, and the codegen-side proof is
/// `pycc_codegen`'s own `a_buffer_allocation_checks_the_pending_state_before_it_allocates`,
/// which runs in CI's coverage job; this arm is corroboration on the built
/// module, not the primary gate. Windows is excluded for the reason the arm
/// below states.
#[test]
#[cfg(not(target_os = "windows"))]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn an_allocation_reached_with_a_pending_exception_never_happens() {
    let dir = fixture(
        "1165_hosted_stale_pending",
        "\
def stale_pending(n: int) -> float:
    xs: list[int] = []
    try:
        a = ndarray(n)
        b = ndarray(xs.pop())
        return a[0] + b[0]
    except IndexError:
        return -1.0
",
    );
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = run_hosted(
        &dir,
        "import ctypes, alloc_probe\n\
         live = ctypes.CDLL(alloc_probe.__file__).pycc_rt_buffer_live_views\n\
         live.restype = ctypes.c_longlong\n\
         live.argtypes = []\n\
         assert live() == 0, live()\n\
         for _ in range(64):\n\
         \x20   assert alloc_probe.stale_pending(8) == -1.0\n\
         assert live() == 0, live()\n\
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
/// The last arm is the one the exception-exit routing actually exists for:
/// `alloc_then_raise` allocates successfully, *then* raises on a second
/// allocation, so the frame leaves through its exception exit with one
/// **live** view rather than a null one. The `build_and_sum(-1)` arm above
/// cannot prove that -- its allocator returned null before `fetch_add`, so
/// an epilogue that freed nothing at all would still show a zero balance.
///
/// Asserting a *balance* rather than watching memory is the point: a frame
/// that never freed would show identical behavior on every other
/// observation in this file.
///
/// Not compiled on Windows, matching `issue_1054_ext_str_release.rs`'s
/// file-level gate and for exactly its reason: the counter is linked into
/// the `.pyd` from a static archive but is absent from its export table,
/// and opening a second shared `pycc_rt` would carry its own `BUFFER_LIVE`
/// and prove nothing. The property is codegen plus runtime bookkeeping and
/// is not platform-specific -- the `pycc_codegen` epilogue tests carry it
/// on every platform, Windows included.
#[test]
#[cfg(not(target_os = "windows"))]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn no_allocation_outlives_the_call_that_made_it() {
    let dir = fixture("1165_hosted_live_views", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = run_hosted(
        &dir,
        "import ctypes, alloc_probe\n\
         live = ctypes.CDLL(alloc_probe.__file__).pycc_rt_buffer_live_views\n\
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
         for _ in range(64):\n\
         \x20   try:\n\
         \x20       alloc_probe.alloc_then_raise(16)\n\
         \x20   except ValueError:\n\
         \x20       pass\n\
         \x20   else:\n\
         \x20       raise AssertionError('alloc_then_raise returned normally')\n\
         assert live() == 0, live()\n\
         print('ok')\n",
    );
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}
