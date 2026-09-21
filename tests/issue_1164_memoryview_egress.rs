//! Part 2b of #1142 (#1164): handing a real `memoryview` back to the CPython
//! host over storage the artifact owns.
//!
//! What this file owns is the *end-to-end* half of the statement -- a program
//! that allocates a buffer and returns it is admitted by `pycc build --ext`,
//! and the host receives a working `memoryview` over the artifact's own
//! storage whose lifetime is the exporter's rather than the call's. The
//! front-end refusals that bound the admission live with the issues that
//! introduced them (`tests/issue_1112_ext_memoryview.rs`,
//! `tests/issue_1129_ndarray_buffer_carrier.rs`), the generated C is pinned by
//! `src/ext_build_tests/generated_c.rs`, and the emitted IR by
//! `crates/pycc_codegen/src/tests.rs`.
//!
//! Only the arm that builds and loads an artifact is `#[ignore]`d, for the
//! reason every `ext` test is. The admission arm is not: it asserts that no
//! *refusal* is reported, which the frontend decides before the toolchain is
//! ever probed, so it runs on a host without CPython development headers.

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

fn fixture(category: &str, source: &str) -> ScratchDir {
    let dir = ScratchDir::new(category).expect("scratch");
    std::fs::write(dir.join("egress_probe.py"), source).expect("write the subject");
    dir
}

fn build_ext(dir: &Path) -> Output {
    pycc()
        .arg("build")
        .arg(dir.join("egress_probe.py"))
        .arg("-o")
        .arg(dir.join("egress_probe"))
        .arg("--ext")
        .output()
        .expect("pycc should spawn")
}

/// The subject: every admitted shape at once. `make` allocates and returns;
/// `fill` allocates, writes through the buffer and returns it; `mixed` takes
/// a host buffer *and* returns an artifact-owned one, which is the pair that
/// proves the two provenances do not collide in one wrapper.
const SUBJECT: &str = "\
def make(n: int) -> memoryview:
    a = ndarray(n)
    return a


def fill(n: int) -> ndarray:
    a = ndarray(n)
    i = 0
    while i < len(a):
        a[i] = float(i) * 2.0
        i = i + 1
    return a


def mixed(v: memoryview) -> NDArray:
    a = ndarray(len(v))
    i = 0
    while i < len(v):
        a[i] = v[i] + 1.0
        i = i + 1
    return a
";

/// The admission arm. Every one of the three spellings is accepted at the
/// return position of a public module-level export, and nothing in the
/// frontend refuses the program -- which is what the `C0003` capability gap
/// and the `I0405` signature refusal each used to do.
///
/// Asserted as the absence of any diagnostic rather than as a successful
/// build: the build reaches `clang` and fails there on a host with no CPython
/// development headers, and that failure is not what this arm owns.
#[test]
fn a_buffer_returning_export_is_admitted_by_an_ext_build() {
    let dir = fixture("1164_admit", SUBJECT);
    let build = build_ext(&dir);
    let err = stderr_of(&build);
    assert!(!err.contains("error[C0003]"), "{err}");
    assert!(!err.contains("error[C0001]"), "{err}");
    assert!(!err.contains("error[I0405]"), "{err}");
    assert!(!err.contains("error[T0"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

/// ...and a native build still refuses it, unchanged. Egress is a property
/// of the `pycc build --ext` boundary: a native executable has no CPython
/// host to hand the buffer to, so the `I0405` signature refusal is exactly as
/// it was. Two-directional so a regression that admitted the signature
/// natively -- where nothing would own the storage -- cannot pass.
#[test]
fn a_native_build_still_refuses_the_same_signature() {
    let dir = fixture("1164_native", SUBJECT);
    let build = pycc()
        .arg("build")
        .arg(dir.join("egress_probe.py"))
        .arg("-o")
        .arg(dir.join("egress_probe"))
        .output()
        .expect("pycc should spawn");
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[I0405]"), "{err}");
    assert!(err.contains("pycc build --ext"), "{err}");
}

/// A `tuple` of the carrier is still refused, at the return position as at
/// every other. The egress arm lives in `src/ext_build/carrier.rs`'s
/// `return_c_type` and deliberately *not* in `BoundaryCarrier::into_scalar`,
/// whose admissible set is what a tuple element is drawn from: loosening the
/// latter would have admitted `tuple[memoryview]` for free, with no shim to
/// carry an element.
#[test]
fn a_tuple_of_the_carrier_is_still_refused_at_a_return_position() {
    let dir = fixture(
        "1164_tuple_return",
        "def make(n: int) -> tuple[memoryview, int]:\n    a = ndarray(n)\n    return (a, n)\n",
    );
    let build = build_ext(&dir);
    assert!(!build.status.success(), "{}", stdout_of(&build));
    let err = stderr_of(&build);
    assert!(err.contains("error[T0039]"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

/// The hosted arm, and the property no front-end assertion can make: the
/// returned `memoryview` outlives the call.
///
/// Every `memoryview` this artifact hands back is backed by an exporter
/// object the artifact owns, so the storage is released when the host drops
/// its last view and not when the exported call returns. The probe is
/// therefore a *use after the call returned*: reading and writing elements,
/// and `bytes()` over the whole view, all happen with the compiled frame long
/// gone. A wrapper that handed back a view over the frame's own slot would
/// pass a same-call assertion and fail these.
#[test]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_returned_buffer_outlives_the_call_and_is_readable_and_writable() {
    let dir = fixture("1164_hosted", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(
            "import egress_probe\n\
             v = egress_probe.make(4)\n\
             assert isinstance(v, memoryview), type(v)\n\
             assert v.format == 'd' and v.itemsize == 8, (v.format, v.itemsize)\n\
             assert v.ndim == 1 and v.shape == (4,) and v.strides == (8,)\n\
             assert not v.readonly and v.c_contiguous\n\
             v[1] = 7.5\n\
             assert v[1] == 7.5\n\
             assert len(bytes(v)) == 32\n\
             f = egress_probe.fill(3)\n\
             assert list(f) == [0.0, 2.0, 4.0], list(f)\n\
             store = bytearray(24)\n\
             host = memoryview(store).cast('d')\n\
             host[0], host[1], host[2] = 1.0, 2.0, 3.0\n\
             m = egress_probe.mixed(host)\n\
             assert list(m) == [2.0, 3.0, 4.0], list(m)\n\
             host.release()\n\
             store.append(0)\n\
             del v, f, m\n\
             for _ in range(200):\n\
             \x20   egress_probe.make(8)\n\
             print('ok')\n",
        )
        .current_dir(&*dir)
        .output()
        .expect("python3 should spawn");
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}

/// The ownership-transfer arm, and the one property the functional probe
/// above cannot make: the returned storage is owned by the exporter object
/// the host now holds, and by nothing else.
///
/// `pycc_rt_buffer_live_views` is the allocator pair's own balance counter,
/// linked into the module from the `pycc_rt` staticlib and read back through
/// `ctypes`. Egress moves an allocation *out* of the frame, so the balance is
/// no longer zero-after-every-call: it is one while the host holds the
/// returned `memoryview`, and back to zero once the host drops it. Both
/// directions are asserted, and both are **looped**: a transfer that leaked
/// one view per call and a transfer that double-freed would each show the
/// right balance on a single iteration and diverge over sixty-four.
///
/// Not compiled on Windows, matching `issue_1165_ext_buffer_alloc.rs`'s own
/// gate and for its reason: the counter is linked into the `.pyd` from a
/// static archive but is absent from its export table. The property is
/// codegen plus runtime bookkeeping and is not platform-specific.
#[test]
#[cfg(not(target_os = "windows"))]
#[ignore = "requires a CPython 3.13+ with development headers on PATH"]
fn a_returned_buffer_transfers_ownership_rather_than_leaking_or_double_freeing() {
    let dir = fixture("1164_hosted_live_views", SUBJECT);
    let build = build_ext(&dir);
    assert!(build.status.success(), "{}", stderr_of(&build));

    let run = Command::new(std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| "python3".into()))
        .arg("-c")
        .arg(
            "import ctypes, egress_probe\n\
             live = ctypes.CDLL(egress_probe.__file__).pycc_rt_buffer_live_views\n\
             live.restype = ctypes.c_longlong\n\
             live.argtypes = []\n\
             assert live() == 0, live()\n\
             for _ in range(64):\n\
             \x20   v = egress_probe.make(8)\n\
             \x20   assert live() == 1, live()\n\
             \x20   del v\n\
             \x20   assert live() == 0, live()\n\
             held = [egress_probe.make(4) for _ in range(64)]\n\
             assert live() == 64, live()\n\
             del held\n\
             assert live() == 0, live()\n\
             print('ok')\n",
        )
        .current_dir(&*dir)
        .output()
        .expect("python3 should spawn");
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run), "ok\n");
}
