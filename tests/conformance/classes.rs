//! Conformance cohort: class model and object-oriented semantics.
//!
//! A `#[path]`-declared submodule of the `tests/conformance.rs` harness (see
//! its `harness_modules!` block). The helpers, `pycc_bin`, and
//! `oracle_python_bin` are the root's private items, visible here through
//! `use super::*;`. Every fixture stays flat under `tests/fixtures/` (D-102).

use super::*;

#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0435_enum_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0435_enum.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0435_enum_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0435_enum.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0435_enum_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0435_enum.py"
    );
}

// PEP 673 (#387 Part 1): `Self` as a method return-type annotation. A
// method returning `Self` yields the class's own instance type, exactly
// like CPython 3.14's deferred-evaluation semantics.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0673_self_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0673_self.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0673_self_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0673_self.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0673_self_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0673_self.py"
    );
}

// PEP 649/749 (#387 Part 2): self-referential deferred annotations. A
// class's method may use the class's own name as a parameter/return type
// annotation, even though the class is not fully defined at the point the
// annotation text appears in source. CPython 3.14 defers evaluation by
// default; pycc resolves the class name at HIR-lowering time since the class
// name is already in scope within its own body.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0649_deferred_ann_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0649_deferred_ann.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0649_deferred_ann_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0649_deferred_ann.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0649_deferred_ann_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0649_deferred_ann.py"
    );
}

// PEP 695 (#387 Part 3): scoped generic classes with one type parameter.
// `class C[T]:` defines a generic class; `C[int](args)` instantiates it
// with a concrete scalar type. pycc monomorphizes the class's methods at
// each call site, reusing PR-13's D-133/D-134 call-site-substitution
// infrastructure.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0695_generic_classes_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0695_generic_classes.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0695_generic_classes_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0695_generic_classes.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0695_generic_classes_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0695_generic_classes.py"
    );
}

// #377: `@property` getter -- `obj.x` is transparently rewritten to
// `obj.x()` (a call to the getter method) at the HIR/MIR level. The
// fixture exercises a read-only property on a class with a backing slot.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn property_basic_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/property_basic.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("property_basic_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/property_basic.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("property_basic_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/property_basic.py"
    );
}

// #377: `@property` getter + `@<name>.setter` -- `obj.x = value` is
// transparently rewritten to `obj._set_x(value)` (a call to the setter
// method) at the HIR/MIR level. The fixture exercises a read-write
// property with a backing slot.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn property_setter_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/property_setter.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("property_setter_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/property_setter.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("property_setter_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/property_setter.py"
    );
}

// #432: basic single inheritance with method override. A derived class
// overrides a base class method; the derived method is called (static
// dispatch via the C3 MRO). A third class inherits without overriding,
// demonstrating inherited method resolution.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn inheritance_basic_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inheritance_basic.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("inheritance_basic_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/inheritance_basic.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("inheritance_basic_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/inheritance_basic.py"
    );
}

// PEP 544 (#380): Protocols and structural typing. A @runtime_checkable
// protocol with a method requirement, two conforming concrete classes,
// a non-conforming class, protocol-typed variable assignments, isinstance
// against the runtime_checkable protocol, and a protocol-typed function
// parameter. The protocol is a compile-time-only interface in pycc and a
// deferred-annotation type hint in CPython 3.14, so only the concrete
// method calls and isinstance results produce observable output.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0544_protocol_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0544_protocol.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0544_protocol_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0544_protocol.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0544_protocol_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0544_protocol.py"
    );
}

// PEP 3119 (#380): ABC and @abstractmethod. An ABC base class with an
// @abstractmethod, two concrete subclasses that override it (one with
// constructor parameters), super().__init__() chaining, and instantiation
// + method calls. The ABC and @abstractmethod are compile-time-only
// markers in pycc and runtime enforcement in CPython 3.14, but only the
// concrete method calls produce observable output.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_3119_abc_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3119_abc.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3119_abc_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_3119_abc.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3119_abc_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_3119_abc.py"
    );
}

// PEP 3135 (#580, Part 4 of #572): zero-argument `super()`. A three-level
// inheritance chain exercising `super().__init__()` with and without
// arguments and an overridden method calling `super().<method>()`, so each
// level's own `super()` must resolve against its *defining* class rather
// than the runtime type of `self` (pycc lowers it with static dispatch per
// D-160; CPython's zero-arg form reads the same `__class__` cell).
//
// #587 also covers `super().<attr>`, split by what a `super` object
// actually proxies. A base class `@property` is a class-level descriptor
// found along the MRO, so `super().power` calls the base getter rather
// than the subclass's override -- that half is exercised here. An
// *instance* attribute established by `self.<attr> = ...` is not proxied,
// and CPython raises `AttributeError`; pycc rejects that form at compile
// time with `T0047`, so it cannot appear in a fixture that must match the
// oracle byte-for-byte and stays a declared gap in the breadth manifest
// instead (`tests/issue_433_super.rs` asserts the rejection).
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_3135_super_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3135_super.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3135_super_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_3135_super.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3135_super_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_3135_super.py"
    );
}

// PEP 557 (#579, Part 3 of #572): dataclasses. `@dataclass` with required
// annotated fields, the synthesized `__init__`/`__eq__`/`__repr__`, and
// dataclass inheritance (parent fields first). The fixture carries
// `from dataclasses import dataclass` because CPython evaluates decorators
// eagerly and would otherwise raise `NameError`; pycc recognizes the bare
// name without the import and only needs the import itself to resolve.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0557_dataclasses_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0557_dataclasses.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0557_dataclasses_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0557_dataclasses.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0557_dataclasses_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0557_dataclasses.py"
    );
}

// PEP 698 (#579, Part 3 of #572): `@override`. A pure runtime marker in
// CPython (it returns the decorated function unchanged), so the observable
// output comes entirely from ordinary method overriding; pycc additionally
// verifies at compile time that the decorated name exists in a base class.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0698_override_matches_cpython_3_14_7_byte_for_byte() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0698_override.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0698_override_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0698_override.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0698_override_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0698_override.py"
    );
}

// PEP 3129 (#579, Part 3 of #572): class decorators. Exercises the one
// class-decorator form pycc supports and CPython agrees with (`@dataclass`);
// `@dataclass_transform()` is deliberately absent because pycc treats it as
// `@dataclass` while CPython synthesizes nothing, a divergence that cannot
// appear in a byte-for-byte fixture and is tracked as #248.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_3129_class_deco_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_3129_class_deco.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_3129_class_deco_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_3129_class_deco.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_3129_class_deco_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_3129_class_deco.py"
    );
}

// #610 (PEP 560): value-position `C[x]` dispatches to
// `C.__class_getitem__(x)`. Covers both the `@staticmethod` and the
// `@classmethod` spelling of the hook, inheritance of the hook through the
// MRO, dispatch from inside a function body, and that an ordinary instance
// attribute on the same class is unaffected.
#[test]
#[ignore = "requires a pinned python3.14 (CPython 3.14.7) oracle on PATH"]
fn pep_0560_class_getitem_matches_cpython_3_14_7_byte_for_byte() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pep_0560_class_getitem.py");
    let (debug_pycc, debug_cpython) =
        run_conformance_fixture_with_profile("pep_0560_class_getitem_debug", &fixture, false);
    assert_eq!(
        debug_pycc, debug_cpython,
        "pycc (--debug) and CPython 3.14.7 disagree on tests/fixtures/pep_0560_class_getitem.py"
    );
    let (release_pycc, release_cpython) =
        run_conformance_fixture_with_profile("pep_0560_class_getitem_release", &fixture, true);
    assert_eq!(
        release_pycc, release_cpython,
        "pycc (--release) and CPython 3.14.7 disagree on tests/fixtures/pep_0560_class_getitem.py"
    );
}
