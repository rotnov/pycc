//! Issue #148 (D-178): public-CLI coverage for `int` literals outside D-061's
//! tagged 63-bit range.
//!
//! Two families live here, both deliberately kept out of
//! `tests/fixtures/oversized_int_literal.py`'s differential fixture:
//!
//! * **Newly reachable runtime `int` boundaries.** Making an out-of-range
//!   literal compile means it can now flow into the D-141 runtime `int`
//!   boundaries that the retired codegen panic used to shadow. CPython
//!   accepts the value in most of these positions, so a differential fixture
//!   containing one would fail by construction -- they are pinned here
//!   instead, by exit code and exact message. One member of this family, the
//!   `range` operand, left it in #147 (D-179) and is now pinned as a success
//!   case; see `tests/issue_147_bigint_range.rs`.
//! * **Still-open bigint operations.** `*`, `//`, `%`, `**`, `/`, `int`->`float`
//!   conversion, and comparison remain accepted failure boundaries; before
//!   #148 they were unreachable from a literal, so their current behavior is
//!   pinned here too.

use pycc_scratch::ScratchDir;
use std::io::Write;
use std::process::Command;

/// The one out-of-range literal every case below uses: `2^62`, the smallest
/// magnitude that does not round-trip through the tagged 63-bit encoding.
const OVERSIZED: &str = "4611686018427387904";

const BOUNDARY_MESSAGE: &str = "pycc_rt: int boundary does not support bigint-valued values yet";

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

/// Compiles and runs `source` through `pycc run` and asserts the documented
/// D-072 exit `101` boundary carrying `message` on stderr.
///
/// `pycc run` (rather than `pycc build` plus a direct exec) is deliberate:
/// `pycc_rt`'s panic unwinds across an `extern "C"` boundary and becomes a
/// non-unwinding process abort, so the raw child is killed by a signal and
/// reports no exit code at all. The `101` in `docs/CLI_SPEC.md`'s boundary
/// list is the driver's own mapping of that abort.
fn assert_runtime_abort(case: &str, source: &str, message: &str) {
    let dir = ScratchDir::new(&format!("issue148_{case}")).expect("failed to create scratch dir");
    let src = dir.join("case.py");
    std::fs::File::create(&src)
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();

    // The literal itself must still *compile* -- that is what #148 changed.
    let check = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "pycc check should accept {case}: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let run = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(101),
        "{case} should hit the documented exit-101 boundary, got {:?}: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(message),
        "{case} should report {message:?}, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Newly reachable D-141 runtime `int` boundaries.
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_literal_as_a_container_value_hits_the_runtime_int_boundary() {
    // `list_validate_element` -- the container-value guard family.
    assert_runtime_abort(
        "list_element",
        &format!("x = [{OVERSIZED}]\nprint(len(x))\n"),
        BOUNDARY_MESSAGE,
    );
}

#[test]
fn an_oversized_literal_appended_to_a_list_hits_the_runtime_int_boundary() {
    assert_runtime_abort(
        "list_append",
        &format!("x = [1]\nx.append({OVERSIZED})\nprint(len(x))\n"),
        BOUNDARY_MESSAGE,
    );
}

#[test]
fn an_oversized_literal_added_to_a_set_hits_the_runtime_int_boundary() {
    assert_runtime_abort(
        "set_add",
        &format!("x = {{1}}\nx.add({OVERSIZED})\nprint(len(x))\n"),
        BOUNDARY_MESSAGE,
    );
}

#[test]
fn an_oversized_literal_as_a_comprehension_element_hits_the_runtime_int_boundary() {
    assert_runtime_abort(
        "listcomp_element",
        &format!("x = [{OVERSIZED} for i in range(2)]\nprint(len(x))\n"),
        BOUNDARY_MESSAGE,
    );
}

#[test]
fn an_oversized_literal_as_a_list_index_hits_the_runtime_int_boundary() {
    // `list_untag_index` -- the index/count guard family. CPython raises
    // `IndexError` here, so this is a divergence in kind, not a lost
    // capability.
    assert_runtime_abort(
        "list_index",
        &format!("x = [1, 2, 3]\nprint(x[{OVERSIZED}])\n"),
        BOUNDARY_MESSAGE,
    );
}

#[test]
fn an_oversized_literal_as_a_str_repeat_count_hits_the_runtime_int_boundary() {
    assert_runtime_abort(
        "str_repeat_count",
        &format!("print(\"ab\" * {OVERSIZED})\n"),
        BOUNDARY_MESSAGE,
    );
}

#[test]
fn an_oversized_literal_as_a_range_argument_no_longer_hits_the_runtime_int_boundary() {
    // #147 (D-179) removed `range_untag_operand`: `range` operands are now
    // normalized rather than decoded, so a bigint bound drives the loop.
    //
    // A bounded stop is essential. The naive spelling this test used to
    // carry -- `range(4611686018427387904)` -- now *succeeds*, and would
    // run ~4.6e18 iterations. The pair below crosses exactly the same
    // representation boundary in two.
    //
    // The success paths themselves live in
    // `tests/issue_147_bigint_range.rs`; this case stays here so the #148
    // boundary inventory records which position left the list and why.
    let dir = ScratchDir::new("issue148_range").expect("failed to create scratch dir");
    let src = dir.join("range.py");
    std::fs::File::create(&src)
        .unwrap()
        .write_all(
            format!("for i in range({OVERSIZED}, 4611686018427387906):\n    print(i)\n").as_bytes(),
        )
        .unwrap();

    let run = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(0),
        "a bigint range should run to completion since #147, got {:?}: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "4611686018427387904\n4611686018427387905\n"
    );
}

// ---------------------------------------------------------------------------
// Still-open bigint operations (out of scope for #148, pinned here).
// ---------------------------------------------------------------------------

#[test]
fn bigint_operations_unreachable_before_issue_148_keep_their_accepted_boundaries() {
    for (case, expr, message) in [
        (
            "compare",
            format!("{OVERSIZED} > 0"),
            "pycc_rt: comparing a bigint-valued `int` is not supported yet",
        ),
        (
            "multiply",
            format!("{OVERSIZED} * 2"),
            "pycc_rt: multiplying a bigint-valued `int` is not supported yet",
        ),
        (
            "floordiv",
            format!("{OVERSIZED} // 2"),
            "pycc_rt: dividing a bigint-valued `int` is not supported yet",
        ),
        (
            "modulo",
            format!("{OVERSIZED} % 2"),
            "pycc_rt: computing the modulo of a bigint-valued `int` is not supported yet",
        ),
        (
            "power",
            format!("{OVERSIZED} ** 2"),
            "pycc_rt: exponentiating a bigint-valued `int` is not supported yet",
        ),
        (
            "truediv",
            format!("{OVERSIZED} / 2"),
            "pycc_rt: converting a bigint-valued `int` is not supported yet",
        ),
        (
            "mixed_float",
            format!("{OVERSIZED} + 1.5"),
            "pycc_rt: converting a bigint-valued `int` is not supported yet",
        ),
    ] {
        assert_runtime_abort(case, &format!("print({expr})\n"), message);
    }
}

// ---------------------------------------------------------------------------
// Completion criterion 3: a literal outside `i64` range stays a compile-time
// capability diagnostic, not a runtime abort.
// ---------------------------------------------------------------------------

#[test]
fn a_literal_outside_i64_range_is_still_a_spanned_capability_diagnostic() {
    let dir = ScratchDir::new("issue148_beyond_i64").expect("failed to create scratch dir");
    let src = dir.join("beyond.py");
    std::fs::File::create(&src)
        .unwrap()
        .write_all(b"x: int = 99999999999999999999999999\nprint(x)\n")
        .unwrap();

    let check = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(1),
        "a literal beyond i64 range should still be rejected at check time"
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(
        text.contains("error[C0001]") && text.contains("integer literal does not fit in i64"),
        "expected the spanned C0001 capability diagnostic, got: {text}"
    );
}
