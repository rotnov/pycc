// Issue #767: `typing.cast(T, value)` is a special-cased builtin call.
//
// `cast` is a runtime no-op in CPython -- it only changes a static
// checker's view of `value`'s type -- so pycc type-checks the call as `T`
// and lowers the whole expression to `value` alone at MIR time. Nothing
// about `cast` ever reaches codegen.
//
// These tests exercise the fix end-to-end through the public CLI: `pycc
// check` accepts the import and the call, and `pycc build` produces a
// binary whose output is identical to the same program with every `cast(T,
// v)` replaced by `v` alone -- the observable definition of "runtime
// no-op".

use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

/// The `cast`-using program. Covers a scalar cast inside an annotated
/// function, a scalar cast at module level, and a cast to a user-defined
/// class whose attribute is then read through the cast result.
const WITH_CAST: &str = "\
from typing import cast

class Box:
    def __init__(self, v: int) -> None:
        self.v = v

def bump(x: int) -> int:
    return cast(int, x) + 1

def unwrap(b: Box) -> int:
    return cast(Box, b).v

print(bump(41))
print(cast(str, \"ok\"))
print(unwrap(Box(7)))
";

/// The same program with every `cast(T, v)` replaced by `v` alone. If
/// `cast` really is a no-op, the two binaries print byte-identical output.
const WITHOUT_CAST: &str = "\
class Box:
    def __init__(self, v: int) -> None:
        self.v = v

def bump(x: int) -> int:
    return x + 1

def unwrap(b: Box) -> int:
    return b.v

print(bump(41))
print(\"ok\")
print(unwrap(Box(7)))
";

fn build_and_run(dir: &std::path::Path, stem: &str, source: &str) -> String {
    let src = write_fixture(dir, &format!("{stem}.py"), source);
    let exe = dir.join(stem);
    let build = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", exe.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "pycc build should succeed for `{stem}`; stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&exe).output().unwrap();
    assert!(
        run.status.success(),
        "`{stem}` should exit successfully; stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// #767: `pycc check` accepts `from typing import cast` together with
/// scalar and user-class casts. Before this change the import alone failed
/// with `C0002` ("module `typing` has no importable symbol named `cast`").
#[test]
fn typing_cast_import_and_calls_check_successfully() {
    let dir = std::env::temp_dir().join(format!("pycc_767_check_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(&dir, "with_cast.py", WITH_CAST);
    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pycc check should succeed for `from typing import cast`; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// #767: `pycc build` compiles the `cast`-using program, and the binary
/// prints exactly what the cast-free equivalent prints -- the observable
/// definition of `cast` being a runtime no-op, and the check that the MIR
/// seam really elides the call instead of emitting one codegen cannot
/// lower.
#[test]
fn typing_cast_build_output_matches_the_cast_free_equivalent() {
    let dir = std::env::temp_dir().join(format!("pycc_767_build_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let with_cast = build_and_run(&dir, "with_cast", WITH_CAST);
    let without_cast = build_and_run(&dir, "without_cast", WITHOUT_CAST);
    assert_eq!(
        with_cast, without_cast,
        "`cast(T, v)` must produce exactly the output of `v` alone"
    );
    assert_eq!(with_cast, "42\nok\n7\n", "unexpected program output");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #767: a program defining its own `def cast(...)` calls that function,
/// not the builtin special case -- the same user-definition-takes-priority
/// rule `float`/`isinstance`/`issubclass` follow, verified at runtime
/// rather than only in the type checker.
#[test]
fn a_user_defined_cast_function_takes_priority_end_to_end() {
    let dir = std::env::temp_dir().join(format!("pycc_767_shadow_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = build_and_run(
        &dir,
        "user_cast",
        "def cast(a: int, b: int) -> int:\n    return a * b\n\nprint(cast(6, 7))\n",
    );
    assert_eq!(
        out, "42\n",
        "a user-defined `cast` must be called, not elided to its second argument"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
