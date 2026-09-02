//! Conformance cohort: `int`/`bool`/unary-operator semantics.
//!
//! A `#[path]`-declared submodule of the `tests/conformance.rs` harness (see
//! its `harness_modules!` block). The helpers, `pycc_bin`, and
//! `oracle_python_bin` are the root's private items, visible here through
//! `use super::*;`. Every fixture stays flat under `tests/fixtures/` (D-102).

use super::*;

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0238_division_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0238_division.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0238_division_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0238_division.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0238_division_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0238_division.py"
    );
}

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0515_underscores_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0515_underscores.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0515_underscores_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0515_underscores.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0515_underscores_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0515_underscores.py"
    );
}

// D-141: bool identity is preserved when a `bool` value crosses a
// statically int-typed boundary (assignment, parameter, return, container
// value, or `range` operand) instead of silently rendering as `1`/`0`.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn bool_int_runtime_identity_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bool_int_runtime_identity.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("bool_int_runtime_identity_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/bool_int_runtime_identity.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("bool_int_runtime_identity_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/bool_int_runtime_identity.py"
    );
}

// #148/D-178: an `int` literal (and an `enum` member discriminant) outside
// D-061's tagged 63-bit range now materializes a heap bigint through
// `pycc_rt_int_from_i64` instead of aborting codegen.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn oversized_int_literal_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oversized_int_literal.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("oversized_int_literal_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/oversized_int_literal.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("oversized_int_literal_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/oversized_int_literal.py"
    );
}

/// #147 (D-179): `range()` bounds, steps, and induction variables that cross
/// D-061's tagged 63-bit boundary. Registered separately from
/// `oversized_int_literal.py` because that fixture is deliberately restricted
/// to the operations a bigint supported *before* #147.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn bigint_range_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bigint_range.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("bigint_range_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/bigint_range.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("bigint_range_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/bigint_range.py"
    );
}

// #602 (Part 1 of #573): a source-level `+`/`-` applied directly to a numeric
// literal folds into that literal's own value. Covers expression position
// (assignment, arithmetic, comparison, argument, `print`) and `match`
// value-pattern position, for both `int` and `float`. Mapping-key position is
// left to `pycc_hir`'s own unit tests: mapping patterns have no end-to-end
// codegen fixture yet, so covering them here would exercise an unrelated gap.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn unary_literal_sign_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unary_literal_sign.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("unary_literal_sign_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/unary_literal_sign.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("unary_literal_sign_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/unary_literal_sign.py"
    );
}

// #603 (Part 2 of #573): a source-level `+`/`-` applied to an operand that is
// not a numeric literal, which #602's fold cannot reach -- a name, a call
// result, a parenthesized expression, an attribute, a subscript, and a nested
// unary. Covers `int`, `bool` (where `+` is not the identity: `+True` is the
// integer `1`), and `float` including `-0.0` and the infinities, plus an
// operand that has already been promoted to a bigint. The two representations
// matter because `pycc_mir` rewrites them into different binary shapes --
// `0 - x` / `0 + x` for `int`/`bool`, `x * -1.0` / `x * 1.0` for `float`.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn unary_general_operand_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unary_general_operand.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("unary_general_operand_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/unary_general_operand.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("unary_general_operand_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/unary_general_operand.py"
    );
}

// #604 (Part 3 of #573): `not x` and `~x`. `not` is defined by truthiness and
// spans every operand type this compiler computes a truth value for; `~` is
// `int -> int` only and decomposes into `-x - 1` at the MIR level, inheriting
// bigint promotion from `int_sub` the same way plain negation does.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn unary_not_invert_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unary_not_invert.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("unary_not_invert_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/unary_not_invert.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("unary_not_invert_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/unary_not_invert.py"
    );
}
