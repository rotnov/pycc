//! Type-checking tests for the synthetic builtin exception classes
//! (Part 1 of #541, extending D-173).
//!
//! HIR lowering now seeds a real `HirClassDef` for each of the seven
//! builtin exception names, which puts them into `Environment::classes`
//! for the first time. Everything here pins the consequences of that:
//! that the pre-existing `except`/`raise` surface is unchanged, that the
//! new surface a real class table unlocks (subclassing, `issubclass`,
//! annotations) behaves, and that a synthetic class is still not a value.
//!
//! In its own submodule rather than appended to the crate's `tests.rs`,
//! per AGENTS.md's decomposability rule (#695).

use super::*;
use crate::{HirClassDef, HirModule, check, check_and_resolve};

fn parse_lower(source: &str) -> HirModule {
    let module = pycc_parser::parse(source).expect("test fixture must parse");
    pycc_hir::lower_checked(&module).expect("test fixture must lower")
}

fn check_source(source: &str) -> Result<(), Diagnostic> {
    check(&parse_lower(source))
}

fn resolve_source(source: &str) -> Result<HirModule, Diagnostic> {
    check_and_resolve(&parse_lower(source))
}

fn environment_for(source: &str) -> Environment {
    let hir = parse_lower(source);
    let mut env = Environment::new();
    crate::class::bind_classes(&mut env, &hir);
    env
}

// -- the shadowing predicate (the highest-risk change) ---------------

#[test]
fn seeded_builtin_exception_classes_do_not_read_as_shadowed() {
    // Before #541 these seven names were absent from `Environment::classes`
    // and `is_unshadowed_builtin_exception` read that absence as "not
    // shadowed". Now they are present, so the predicate has to distinguish
    // a synthetic entry from a user one; if it ever stops doing so, every
    // `except`/`raise` in the language starts being rejected.
    let env = environment_for("print(1)\n");
    for name in pycc_hir::BUILTIN_EXCEPTION_CLASSES {
        assert!(env.is_synthetic_class(name), "`{name}` must be synthetic");
        assert!(
            is_unshadowed_builtin_exception(&env, &[], name),
            "`{name}` must not read as shadowed"
        );
    }
}

#[test]
fn a_user_class_of_the_same_name_still_reads_as_shadowed() {
    let env = environment_for("class ValueError:\n    def __init__(self) -> None:\n        pass\n");
    assert!(!env.is_synthetic_class("ValueError"));
    assert!(!is_unshadowed_builtin_exception(&env, &[], "ValueError"));
}

#[test]
fn rebinding_a_synthetic_name_with_a_user_definition_clears_the_marking() {
    // `bind_class` is the sole mutator of both tables, so a later user
    // definition under the same name must un-mark it -- otherwise a
    // monomorphization pass that re-registers a class could leave a user
    // class marked synthetic and silently uninstantiable.
    let mut env = environment_for("print(1)\n");
    assert!(env.is_synthetic_class("ValueError"));
    env.bind_class(
        "ValueError".to_string(),
        HirClassDef {
            name: "ValueError".to_string(),
            bases: Vec::new(),
            mro: vec!["ValueError".to_string()],
            attrs: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
            static_methods: Vec::new(),
            class_methods: Vec::new(),
            type_param: None,
            enum_members: Vec::new(),
            is_dataclass: false,
            dataclass_fields: Vec::new(),
            is_protocol: false,
            runtime_checkable: false,
            protocol_members: Vec::new(),
            abstract_methods: Vec::new(),
            is_abstract: false,
        },
    );
    assert!(!env.is_synthetic_class("ValueError"));
    assert!(!is_unshadowed_builtin_exception(&env, &[], "ValueError"));
}

#[test]
fn an_unregistered_name_is_not_synthetic() {
    let env = environment_for("print(1)\n");
    assert!(!env.is_synthetic_class("NotAnException"));
}

// -- the pre-#541 `except`/`raise` surface is unchanged ---------------

#[test]
fn the_existing_raise_and_except_surface_still_checks() {
    check_source("raise ValueError(\"bad\")\n").expect("`raise ValueError(...)` must still check");
    check_source("raise Exception(\"bad\")\n").expect("`raise Exception(...)` must still check");
    check_source("try:\n    print(1)\nexcept ValueError:\n    print(2)\n")
        .expect("`except ValueError:` must still check");
    check_source("try:\n    print(1)\nexcept Exception as e:\n    print(2)\n")
        .expect("`except Exception as e:` must still check");
}

#[test]
fn raise_of_a_bound_builtin_exception_instance_still_checks() {
    check_source("try:\n    print(1)\nexcept ValueError as e:\n    raise e\n")
        .expect("re-raising a bound handler binding must still check");
}

#[test]
fn a_user_class_shadowing_a_builtin_exception_name_still_closes_the_gates() {
    // T0021's gates must stay closed against a user class: shadowing
    // `ValueError` makes `except ValueError:` an unrecognized handler
    // again, exactly as before #541.
    let err = check_source(
        "class ValueError:\n    def __init__(self) -> None:\n        pass\n\n\
         try:\n    print(1)\nexcept ValueError:\n    print(2)\n",
    )
    .expect_err("a shadowed handler class must be rejected");
    assert_eq!(err.code, "T0021");
}

// -- a synthetic builtin exception class is not a value ---------------

#[test]
fn instantiating_a_synthetic_builtin_exception_class_is_rejected() {
    let env = environment_for("print(1)\n");
    let err = crate::class::resolve_instantiation(&env, "ValueError", &[Ty::Str])
        .expect_err("a builtin exception class must not be instantiable as a value");
    assert_eq!(err.code, "C0001");
    assert!(
        err.message
            .contains("cannot instantiate builtin exception class"),
        "unexpected message: {}",
        err.message
    );
}

#[test]
fn every_synthetic_builtin_exception_class_is_rejected_the_same_way() {
    let env = environment_for("print(1)\n");
    for name in pycc_hir::BUILTIN_EXCEPTION_CLASSES {
        let err = crate::class::resolve_instantiation(&env, name, &[Ty::Str])
            .expect_err("every builtin exception class must be uninstantiable");
        assert_eq!(err.code, "C0001");
    }
}

#[test]
fn a_user_class_shadowing_a_builtin_exception_name_stays_instantiable() {
    // The rejection is keyed on "synthetic", not on the name, so a user's
    // own `class ValueError:` is an ordinary, instantiable class.
    resolve_source(
        "class ValueError:\n    def __init__(self) -> None:\n        pass\n\n\
         x = ValueError()\nprint(1)\n",
    )
    .expect("a user class named `ValueError` must stay instantiable");
}

// -- the new surface a real class table unlocks ------------------------

#[test]
fn a_user_subclass_of_a_builtin_exception_class_is_instantiable() {
    // Part 1 of #541's headline gain: `MyError` inherits the synthetic
    // `Exception.__init__` through its MRO, so this resolves instead of
    // reaching `resolve_instantiation`'s internal-error panic.
    resolve_source("class MyError(ValueError):\n    pass\n\nx = MyError(\"boom\")\nprint(1)\n")
        .expect("a user subclass must inherit the synthetic constructor");
}

#[test]
fn a_user_subclass_constructor_still_checks_its_arguments() {
    let err = resolve_source("class MyError(ValueError):\n    pass\n\nx = MyError(1)\nprint(1)\n")
        .expect_err("the inherited constructor's `str` parameter must be enforced");
    assert_eq!(err.code, "T0021");
}

#[test]
fn a_user_class_may_inherit_from_two_builtin_exception_classes() {
    resolve_source("class E(ValueError, TypeError):\n    pass\n\nprint(1)\n")
        .expect("multiple builtin exception bases must linearize and check");
}

#[test]
fn a_builtin_exception_class_is_usable_as_an_annotation() {
    check_source("def handle(e: ValueError) -> int:\n    return 1\n\nprint(1)\n")
        .expect("a builtin exception class must be a valid parameter annotation");
}

#[test]
fn an_annotated_assignment_of_the_wrong_type_is_a_diagnostic_not_a_panic() {
    let err = check_source("x: ValueError = 1\nprint(1)\n")
        .expect_err("an `int` initializer must not satisfy a `ValueError` annotation");
    assert_eq!(err.code, "T0025");
}

#[test]
fn a_return_annotation_of_a_builtin_exception_class_is_a_diagnostic_not_a_panic() {
    let err = check_source("def f() -> ValueError:\n    print(1)\n\nprint(1)\n")
        .expect_err("a function that cannot return a `ValueError` must be rejected");
    assert_eq!(err.code, "T0022");
}

#[test]
fn issubclass_over_the_builtin_exception_hierarchy_is_evaluated() {
    // Only reachable because the hierarchy is now in the class table:
    // before #541 `Exception` had no `HirClassDef` for the MRO walk.
    resolve_source(
        "class MyError(Exception):\n    pass\n\nprint(issubclass(MyError, Exception))\n",
    )
    .expect("`issubclass` over a builtin exception base must resolve");
    resolve_source("print(isinstance(1, ValueError))\n")
        .expect("`isinstance` against a builtin exception class must resolve");
}

// -- attribute access through an `except ... as` binding -------------

/// Before Part 1, `except ValueError as e:` bound `e` to
/// `Ty::Instance("ValueError")` while no `HirClassDef` of that name
/// existed, so any attribute read on `e` aborted the compiler in
/// `class::expect_class` with `internal error: class `ValueError` has no
/// registered HirClassDef`. The seeded definition turns that abort into an
/// ordinary diagnostic. It is `T0044` and not a successful read because
/// the synthetic definitions declare no attribute slots (D-173 propagates
/// a raised exception through global runtime state, not through an
/// allocated instance, so there is no storage a `message`/`args` slot
/// could name).
#[test]
fn reading_an_attribute_off_a_caught_builtin_exception_reports_t0044() {
    let error = check_source(
        "def main() -> None:\n    try:\n        raise ValueError(\"x\")\n    except ValueError as e:\n        print(e.args)\n",
    )
    .expect_err("`args` is not an attribute of the synthetic class");
    assert_eq!(error.code, "T0044");
    assert!(
        error.message.contains("ValueError") && error.message.contains("args"),
        "unexpected message: {}",
        error.message
    );
}

/// The handler binding itself, without an attribute read, is unaffected.
#[test]
fn binding_a_caught_builtin_exception_without_reading_it_still_checks() {
    check_source(
        "def main() -> None:\n    try:\n        raise ValueError(\"x\")\n    except ValueError as e:\n        print(\"caught\")\n",
    )
    .expect("binding a caught builtin exception must keep checking");
}
