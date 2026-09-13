//! The §3.3 completeness guard for D-244 rule 7's refusal set: every type
//! the `ext` boundary admits at a parameter position emits a refusal arm,
//! and the shim declares exactly the unpack helpers those arms name.
//!
//! `tests/issue_1067_neg004_ext_conformance.rs` pins what a refusal *says*
//! to a host, one shape at a time, through a real CPython. It cannot say
//! that no admitted type was left without an arm at all -- a new carrier
//! added without a refusal path would simply not appear in its table. This
//! file is the closed-set half of that statement, and it is deliberately
//! non-`#[ignore]`d: the coverage job runs `llvm-cov` without
//! `--include-ignored`, so this is where the lines of this change that sit
//! inside `scripts/check_diff_coverage.py`'s denominator are executed.

use super::*;
use std::collections::BTreeSet;

/// Every `pycc_ext_unpack_*` suffix the tracked shim mentions.
///
/// Scanned out of the shim text rather than listed by hand on both sides:
/// the point of the assertion below is that the two sets agree, which a
/// second hand-written list could not establish. The suffix runs to the
/// first character a C identifier cannot continue with, so
/// `pycc_ext_unpack_int_at` yields `int_at` and never a spurious `int`.
fn shim_unpack_suffixes() -> BTreeSet<String> {
    const PREFIX: &str = "pycc_ext_unpack_";
    let shim = shim_c();
    let mut found = BTreeSet::new();
    for (start, _) in shim.match_indices(PREFIX) {
        let rest = &shim[start + PREFIX.len()..];
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_');
        found.insert(rest[..end.unwrap_or(rest.len())].to_string());
    }
    found
}

/// The exact refusal arms one argument slot emits, from each `if` up to
/// and including the `!= 0) {` that opens its failure block.
///
/// The `match` carries no wildcard arm on purpose. A third
/// [`BoundaryCarrier`] variant -- a new shape the boundary admits -- then
/// fails to compile here instead of silently inheriting a scalar's single
/// arm, which is the whole completeness property this module exists to
/// state. `docs/TESTING.md`'s coverage notes forbid the catch-all that
/// would otherwise absorb it unexercised.
fn refusal_arms(carrier: &BoundaryCarrier, name: &str, index: usize) -> Vec<String> {
    match carrier {
        BoundaryCarrier::Scalar(_, helper) => vec![format!(
            "if (pycc_ext_unpack_{helper}(args[{index}], \"{name}\", {index}, &a{index}) != 0) {{"
        )],
        BoundaryCarrier::Tuple(elements) => {
            let len = elements.len();
            let head = format!(
                "if (pycc_ext_unpack_tuple(args[{index}], \"{name}\", {index}, {len}) != 0) {{"
            );
            let arms = elements.iter().enumerate().map(|(element, (_, helper))| {
                format!(
                    "if (pycc_ext_unpack_{helper}_at(PyTuple_GetItem(args[{index}], {element}), \
                     \"{name}\", {index}, {element}, &a{index}_{element}) != 0) {{"
                )
            });
            std::iter::once(head).chain(arms).collect()
        }
    }
}

/// Asserts that `arm` appears in `inc` and that the block it opens returns
/// `NULL` before it closes -- a refusal arm that fell through to the call
/// would contain the arm text just the same.
fn assert_arm_refuses(inc: &str, arm: &str) {
    assert!(inc.contains(arm), "no {arm} in\n{inc}");
    let tail = &inc[inc.find(arm).expect("just asserted") + arm.len()..];
    let end = tail.find("\n    }").expect("the arm block is closed");
    assert!(tail[..end].contains("return NULL;"), "{inc}");
}

#[test]
fn the_shim_declares_exactly_the_unpack_helpers_the_boundary_refuses_through() {
    let expected: BTreeSet<String> = [
        "bool", "bool_at", "float", "float_at", "int", "int_at", "str", "tuple",
    ]
    .iter()
    .map(|suffix| (*suffix).to_string())
    .collect();
    assert_eq!(shim_unpack_suffixes(), expected);
}

#[test]
fn every_admitted_argument_type_refuses_before_the_call_and_after_the_arity_check() {
    // One row per type the boundary admits at a parameter position, and so
    // at least one row per `BoundaryCarrier` variant: D-116 fixes a tuple's
    // element types to exactly the three below, so the `_at` helpers are
    // covered here as well. Each export takes two arguments of its row's
    // type, which is what makes "every slot" more than "the first slot".
    let rows: Vec<(&str, Ty)> = vec![
        ("take_int", Ty::Int),
        ("take_float", Ty::Float),
        ("take_bool", Ty::Bool),
        ("take_str", Ty::Str),
        (
            "take_tuple_int",
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])),
        ),
        ("take_tuple_float", Ty::Tuple(Box::new(vec![Ty::Float]))),
        ("take_tuple_bool", Ty::Tuple(Box::new(vec![Ty::Bool]))),
    ];
    for (name, ty) in rows {
        let params = [("a", ty.clone()), ("b", ty.clone())];
        let hir = module(vec![func(name, &params, Ty::Int)]);
        let exports = collect_exports(&hir).expect("every row is a carriable signature");
        let carrier = boundary_carrier(&ty).expect("every row is an admitted argument type");
        let inc = generate_exports_inc("m", &exports, &[]);
        for index in 0..params.len() {
            for arm in refusal_arms(&carrier, name, index) {
                assert_arm_refuses(&inc, &arm);
            }
        }
        // Rule 7 refuses a non-conforming call "before the compiled body
        // runs", and a wrong argument *count* is the one shape that cannot
        // be diagnosed per slot: reading `args[1]` of a one-argument call
        // is out of bounds, not a `TypeError`. So the arity check precedes
        // every unpack, and this is the assertion that says so.
        let arity = inc.find("if (nargs != 2)").expect("the arity check");
        let first = inc
            .find("pycc_ext_unpack_")
            .expect("at least one refusal arm");
        assert!(arity < first, "{inc}");
    }
}

/// Whether the boundary is expected to carry `ty` at a parameter position,
/// stated here once for every variant of [`Ty`] with no wildcard arm.
///
/// [`boundary_carrier`] ends in `_ => None`, so the closed-set assertions
/// above guard only the `BoundaryCarrier` axis: a new `Ty` admitted by a
/// new arm there would map to an existing carrier shape and leave both the
/// hand-maintained row list and the shim's helper set untouched. This
/// function is the other axis. Adding a variant to `Ty` fails to compile
/// here, and admitting an existing one flips an assertion below -- either
/// way the change is stated rather than inherited.
fn expected_to_carry(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Str => true,
        // D-116 fixes a tuple's element types to the three scalars a
        // `pycc_ext_unpack_*_at` helper exists for: `str` is a single C
        // slot at a top-level position but has no element shim, and a
        // nested tuple is not a single slot at all.
        Ty::Tuple(elements) => elements
            .iter()
            .all(|element| matches!(element, Ty::Int | Ty::Float | Ty::Bool)),
        Ty::None
        | Ty::Infer
        | Ty::Param(_)
        | Ty::List(_)
        | Ty::Dict(_)
        | Ty::Set(_)
        | Ty::Instance(_)
        | Ty::Protocol(_)
        | Ty::Optional(_) => false,
    }
}

#[test]
fn no_type_outside_the_admitted_set_is_carried_at_a_parameter_position() {
    let name = || Box::new("T".to_string());
    let samples: Vec<Ty> = vec![
        Ty::Int,
        Ty::Float,
        Ty::Bool,
        Ty::Str,
        Ty::None,
        Ty::Infer,
        Ty::Param(name()),
        Ty::List(Box::new(Ty::Int)),
        Ty::Dict(Box::new((Ty::Str, Ty::Int))),
        Ty::Set(Box::new(Ty::Int)),
        Ty::Instance(name()),
        Ty::Protocol(name()),
        Ty::Optional(Box::new(Ty::Int)),
        Ty::Tuple(Box::new(vec![Ty::Int, Ty::Float, Ty::Bool])),
        // The two element shapes the boundary refuses: neither has an
        // `_at` helper, and both are unreachable from source today only
        // because `T0039` refuses the annotation first.
        Ty::Tuple(Box::new(vec![Ty::Str])),
        Ty::Tuple(Box::new(vec![Ty::Tuple(Box::new(vec![Ty::Int]))])),
    ];
    for ty in samples {
        assert_eq!(carries_param(&ty), expected_to_carry(&ty), "{ty:?}");
    }
}
