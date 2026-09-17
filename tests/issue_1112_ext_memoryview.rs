//! Part 1 of #1027 (#1112): `memoryview` as a pycc type and an `ext`
//! boundary carrier, exercised end to end.
//!
//! The refusal *shapes* -- one per arm of the shim's
//! `pycc_ext_unpack_memoryview`, plus the two buffer-release probes -- are
//! owned by `tests/issue_1067_neg004_ext_conformance.rs`, which is where
//! every other D-244 rule 7 refusal is already pinned against a real
//! interpreter. The closed-set guard that a new carrier cannot be added
//! without a refusal arm is owned by
//! `src/ext_build_tests/refusal_completeness.rs`, and the generated C is
//! pinned by `src/ext_build_tests/generated_c.rs`. What is left, and what
//! this file owns, is the *mode* half of Part 1's statement: the same
//! annotation is admitted by `pycc build --ext` and refused by a native
//! `pycc build`, at both positions it can appear in.
//!
//! Neither mode refusal needs CPython development headers, because both are
//! decided in the frontend before the toolchain is probed, so the two
//! refusal arms below run everywhere. Only the arm that actually builds and
//! loads an artifact is `#[ignore]`d, for the reason every `ext` test is.
//! None of the three contributes line coverage: CI's coverage job runs
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
    std::fs::write(dir.join("view_probe.py"), source).expect("write the subject");
    dir
}

/// Builds the entry module as a CPython extension module directly into
/// `dir`, so a CPython run with `dir` as its working directory imports it.
///
/// The output path carries no extension suffix, exactly as
/// `tests/issue_1084_loop_shape.rs`'s own helper writes it: `pycc build
/// --ext` appends the one its target triple calls for and derives the
/// exported `PyInit_<mod>` name from the path's own spelling, so spelling
/// `.abi3.so` here builds on Unix and then fails the `--ext` name contract
/// on the required windows-latest Tier-1 leg.
fn build_ext(dir: &Path) -> Output {
    pycc()
        .arg("build")
        .arg(dir.join("view_probe.py"))
        .arg("-o")
        .arg(dir.join("view_probe"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn")
}

/// The subject: a `memoryview` at the one position Part 1 admits it, next
/// to an ordinary scalar export so the artifact also proves the carrier
/// changed nothing for every signature that predates it.
///
/// The body does not *read* `v`. Part 1's section 3.5 admits no operation
/// on the value -- a `memoryview` has no literal and no producing
/// expression, so refusing the read refuses aliasing, reassignment,
/// container stores and onward calls all at once -- and
/// `crates/pycc_types/src/tests.rs`'s
/// `reading_a_memoryview_parameter_is_a_capability_gap_rather_than_an_ice`
/// pins that refusal. Indexing the buffer is Part 2 of #1027.
const SUBJECT: &str = "\
def take_view(v: memoryview) -> int:
    return 11


def plain(x: int) -> int:
    return x + 1
";

/// A native `pycc build` refuses a `memoryview` *parameter* with `I0405`.
///
/// `pycc check` is deliberately not the command under test: like the
/// `I0403` foreign-import gate it sits beside, this refusal lives in
/// `src/frontend.rs`'s `resolve_frontend_native`, which only a native
/// `pycc build` reaches -- `check` is artifact-mode agnostic and stays so.
#[test]
fn a_memoryview_parameter_is_refused_by_a_native_build() {
    let dir = fixture("1112_native_param", SUBJECT);
    let build = pycc()
        .arg("build")
        .arg(dir.join("view_probe.py"))
        .arg("-o")
        .arg(dir.join("view_probe"))
        .output()
        .expect("pycc should spawn");
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[I0405]"), "{err}");
    assert!(
        err.contains("`take_view`'s parameter `v: memoryview`"),
        "{err}"
    );
    assert!(err.contains("pycc build --ext"), "{err}");
}

/// The other position, and the other mode: a `-> memoryview` return is
/// refused in *both*. Natively it is the same `I0405`; under `--ext` it is
/// `C0003`, because `BoundaryCarrier::into_scalar` answers `None` for the
/// buffer carrier and the export boundary has no way to hand a borrowed
/// view back to a host that did not lend it.
#[test]
fn a_memoryview_return_type_is_refused_in_both_modes() {
    const RETURNS_A_VIEW: &str = "\
def make() -> memoryview:
    return make()
";
    let dir = fixture("1112_return", RETURNS_A_VIEW);
    let native = pycc()
        .arg("build")
        .arg(dir.join("view_probe.py"))
        .arg("-o")
        .arg(dir.join("view_probe"))
        .output()
        .expect("pycc should spawn");
    assert!(!native.status.success(), "{}", stdout_of(&native));
    assert!(
        stderr_of(&native).contains("error[I0405]"),
        "{}",
        stderr_of(&native)
    );

    // No development headers are needed for this arm either: `plan_ext`
    // resolves the capability gap on the program itself before it probes
    // the host toolchain, which is exactly the ordering `run_build`
    // documents.
    let ext = build_ext(&dir);
    assert!(!ext.status.success(), "{}", stdout_of(&ext));
    let err = stderr_of(&ext);
    assert!(err.contains("error[C0003]"), "{err}");
    assert!(err.contains("its return type `-> memoryview`"), "{err}");
}

/// The hosted arm: the same annotation the two arms above refuse builds as
/// an extension module, and the host calls it with a real `memoryview`.
///
/// The release assertion is the one this file adds that no refusal shape
/// can make. `bytearray.append` raises `BufferError` while any exporter
/// holds a view of it, so an append that *succeeds* after the exported
/// call returned and the host's own view was released proves the wrapper
/// released the `Py_buffer` it acquired on the success path -- not only on
/// the bail paths, where a leak would be invisible to a returning call.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_memoryview_export_builds_and_releases_its_buffer_in_the_host() {
    let dir = fixture("1112_hosted", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(
            "import view_probe\n\
             store = bytearray(24)\n\
             view = memoryview(store).cast('d')\n\
             assert view_probe.take_view(view) == 11\n\
             assert view_probe.plain(41) == 42\n\
             view.release()\n\
             store.append(0)\n\
             print('ok')\n",
        )
        .current_dir(&*dir)
        .output()
        .expect("python3 should spawn");
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}
