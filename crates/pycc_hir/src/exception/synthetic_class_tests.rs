//! Unit tests for the synthetic builtin exception classes (Part 1 of #541).
//!
//! Kept in their own submodule rather than appended to the crate's existing
//! `tests.rs`, per AGENTS.md's decomposability rule.

use super::*;
use crate::pycc_parser_test_helper::parse;
use crate::{HirModule, lower_checked};

fn lower(source: &str) -> HirModule {
    lower_checked(&parse(source)).expect("test fixture must lower")
}

fn class_named<'m>(hir: &'m HirModule, name: &str) -> Option<&'m HirClassDef> {
    hir.class_defs
        .iter()
        .find(|(class_name, _)| class_name == name)
        .map(|(_, def)| def)
}

// -- the synthetic definitions themselves ---------------------------

#[test]
fn every_builtin_exception_name_gets_exactly_one_definition() {
    let defs = builtin_exception_class_defs();
    assert_eq!(defs.len(), BUILTIN_EXCEPTION_CLASSES.len());
    for name in BUILTIN_EXCEPTION_CLASSES {
        let count = defs.iter().filter(|(n, _)| n == name).count();
        assert_eq!(count, 1, "`{name}` must appear exactly once");
    }
}

#[test]
fn exception_is_the_root_and_owns_the_only_constructor() {
    let defs = builtin_exception_class_defs();
    let (_, root) = defs
        .iter()
        .find(|(name, _)| name == "Exception")
        .expect("`Exception` must be synthesized");
    assert!(root.bases.is_empty());
    assert_eq!(root.mro, vec!["Exception".to_string()]);
    assert_eq!(
        root.methods,
        vec![(
            "__init__".to_string(),
            EXCEPTION_INIT_MANGLED_NAME.to_string()
        )]
    );
}

#[test]
fn every_non_root_builtin_exception_inherits_init_from_its_mro() {
    for (name, def) in builtin_exception_class_defs() {
        if name == "Exception" {
            continue;
        }
        assert!(
            def.methods.is_empty(),
            "`{name}` must inherit `__init__` through its MRO, not redeclare it"
        );
        // Every non-root's `mro` must end at `Exception`, whatever its depth.
        assert_eq!(def.mro.last(), Some(&"Exception".to_string()));
    }
}

/// Part 2 of #543 (#739): every class whose real parent is `Exception`
/// directly (the original flat six plus `OSError`) still gets the
/// historical two-entry MRO.
#[test]
fn direct_children_of_exception_get_a_two_entry_mro() {
    let defs = builtin_exception_class_defs();
    for name in [
        "ValueError",
        "TypeError",
        "KeyError",
        "IndexError",
        "ZeroDivisionError",
        "RuntimeError",
        "OSError",
    ] {
        let (_, def) = defs
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("`{name}` must be synthesized"));
        assert_eq!(def.bases, vec!["Exception".to_string()]);
        assert_eq!(def.mro, vec![name.to_string(), "Exception".to_string()]);
    }
}

/// Part 2 of #543 (#739): `BrokenPipeError`'s real MRO is 3 ancestor levels
/// deep (`ConnectionError` -> `OSError` -> `Exception`), the deepest in the
/// hierarchy -- this is the regression lock for `builtin_exception_class_defs`'s
/// MRO-walk loop reaching its 2nd and 3rd iterations.
#[test]
fn broken_pipe_error_has_the_full_four_entry_mro() {
    let defs = builtin_exception_class_defs();
    let (_, def) = defs
        .iter()
        .find(|(name, _)| name == "BrokenPipeError")
        .expect("`BrokenPipeError` must be synthesized");
    assert_eq!(def.bases, vec!["ConnectionError".to_string()]);
    assert_eq!(
        def.mro,
        vec![
            "BrokenPipeError".to_string(),
            "ConnectionError".to_string(),
            "OSError".to_string(),
            "Exception".to_string(),
        ]
    );
}

/// `OSError`'s own MRO is the two-entry shape (it derives directly from
/// `Exception`), spot-checked separately from the loop above since it is the
/// root of the new subtree.
#[test]
fn os_error_has_a_two_entry_mro() {
    let defs = builtin_exception_class_defs();
    let (_, def) = defs
        .iter()
        .find(|(name, _)| name == "OSError")
        .expect("`OSError` must be synthesized");
    assert_eq!(def.bases, vec!["Exception".to_string()]);
    assert_eq!(
        def.mro,
        vec!["OSError".to_string(), "Exception".to_string()]
    );
}

#[test]
fn synthetic_definitions_carry_no_shape_beyond_the_hierarchy() {
    for (name, def) in builtin_exception_class_defs() {
        assert!(def.attrs.is_empty(), "`{name}` must have no slots");
        assert!(def.properties.is_empty());
        assert!(def.static_methods.is_empty());
        assert!(def.class_methods.is_empty());
        assert!(def.type_param.is_none());
        assert!(def.enum_members.is_empty());
        assert!(!def.is_dataclass);
        assert!(def.dataclass_fields.is_empty());
        assert!(!def.is_protocol);
        assert!(!def.runtime_checkable);
        assert!(def.protocol_members.is_empty());
        assert!(
            def.abstract_methods.is_empty(),
            "`{name}` must not be abstract -- an abstract class cannot be subclassed \
             without overriding every abstract method"
        );
        assert!(!def.is_abstract);
    }
}

#[test]
fn the_synthetic_constructor_takes_self_and_a_str_message() {
    let HirItem::Function {
        name,
        params,
        return_ty,
        body,
    } = builtin_exception_init_item()
    else {
        panic!("the synthetic constructor must lower to an ordinary function item");
    };
    assert_eq!(name, EXCEPTION_INIT_MANGLED_NAME);
    assert_eq!(
        params,
        vec![
            (
                "self".to_string(),
                Ty::Instance(Box::new("Exception".to_string()))
            ),
            ("message".to_string(), Ty::Str),
        ]
    );
    assert_eq!(return_ty, Ty::None);
    assert_eq!(body, vec![HirStmt::Return(None)]);
}

#[test]
fn lowering_records_provenance_for_the_seeded_names() {
    // The provenance record (D-188) is set exactly when the seven
    // definitions were seeded, and is the *only* thing downstream marks
    // synthetic membership from.
    let seeded = lower("raise ValueError(\"boom\")\n");
    assert!(seeded.seeded_builtin_exception_classes);
    assert_eq!(seeded.class_defs.len(), BUILTIN_EXCEPTION_CLASSES.len());

    // No reference to any of the seven: no seeding, no provenance.
    let unseeded = lower("def main() -> None:\n    pass\n");
    assert!(!unseeded.seeded_builtin_exception_classes);
    assert!(unseeded.class_defs.is_empty());

    // A user class that is byte-for-byte the synthetic `Exception` still
    // carries no provenance, because lowering did not produce it. Both
    // halves are asserted together: the structural-equality half keeps this
    // test honest if the synthetic shape ever changes.
    let user = lower("class Exception:\n    def __init__(self) -> None:\n        pass\n");
    assert!(!user.seeded_builtin_exception_classes);
    let synthetic_exception = builtin_exception_class_defs()
        .into_iter()
        .find(|(name, _)| name == "Exception")
        .expect("`Exception` must be synthesized")
        .1;
    assert_eq!(
        class_named(&user, "Exception"),
        Some(&synthetic_exception),
        "fixture must stay structurally identical to the synthetic definition"
    );
}

// -- the all-or-nothing shadow pre-scan ------------------------------

fn shadows(source: &str) -> bool {
    module_shadows_builtin_exception_name(&parse(source))
}

#[test]
fn an_ordinary_module_shadows_nothing() {
    assert!(!shadows("print(1)\n"));
    assert!(!shadows(
        "class Other:\n    def __init__(self) -> None:\n        pass\n"
    ));
    assert!(!shadows("def helper() -> int:\n    return 1\n"));
    assert!(!shadows("type Alias = int\n"));
    assert!(!shadows("x: int = 1\n"));
    assert!(!shadows("x = 1\n"));
}

#[test]
fn a_class_definition_shadows() {
    assert!(shadows(
        "class ValueError:\n    def __init__(self) -> None:\n        pass\n"
    ));
}

#[test]
fn a_function_definition_shadows() {
    assert!(shadows("def TypeError() -> int:\n    return 1\n"));
}

#[test]
fn a_type_alias_statement_shadows() {
    assert!(shadows("type KeyError = int\n"));
}

#[test]
fn an_annotated_assignment_shadows() {
    assert!(shadows("IndexError: int = 1\n"));
}

#[test]
fn a_plain_assignment_shadows() {
    assert!(shadows("RuntimeError = 1\n"));
}

#[test]
fn an_unpacking_assignment_target_shadows() {
    assert!(shadows("ZeroDivisionError, other = 1, 2\n"));
    assert!(shadows("[Exception, other] = [1, 2]\n"));
    assert!(shadows("*ValueError, other = [1, 2]\n"));
}

#[test]
fn a_non_name_assignment_target_does_not_shadow() {
    // An attribute or subscript target rebinds something other than a bare
    // module-level name, so it cannot shadow a builtin exception name.
    assert!(!shadows("holder.ValueError = 1\n"));
    assert!(!shadows("holder[\"ValueError\"] = 1\n"));
}

// -- seeding during lowering ------------------------------------------

#[test]
fn lowering_seeds_every_builtin_exception_class() {
    // `raise` is the cheapest spelling that makes the module a referencing
    // one; the seeding gate is exercised name-by-name further below.
    let hir = lower("raise ValueError(\"boom\")\n");
    assert_eq!(hir.class_defs.len(), BUILTIN_EXCEPTION_CLASSES.len());
    for name in BUILTIN_EXCEPTION_CLASSES {
        assert!(class_named(&hir, name).is_some(), "`{name}` must be seeded");
    }
}

#[test]
fn a_module_that_references_no_builtin_exception_name_is_seeded_with_none() {
    // The gate that keeps `frontend-perf-gate` green: every entry in
    // `class_defs` costs per-item work in lowering and per-function class
    // binding in `pycc_types`, and a module that never names one of the
    // seven cannot observe the difference.
    let hir = lower("print(1)\n");
    assert!(hir.class_defs.is_empty());
    assert!(
        !hir.items
            .iter()
            .any(|item| matches!(item, HirItem::Function { name, .. } if name
                == EXCEPTION_INIT_MANGLED_NAME))
    );
}

#[test]
fn a_class_bearing_module_that_names_no_builtin_exception_is_seeded_with_none() {
    let hir = lower("class A:\n    def __init__(self) -> None:\n        pass\n");
    assert_eq!(hir.class_defs.len(), 1);
    assert_eq!(hir.class_defs[0].0, "A");
}

#[test]
fn the_module_s_own_classes_come_first_in_source_order() {
    let hir = lower(
        "class A(Exception):\n    pass\n\n\
         class B:\n    def __init__(self) -> None:\n        pass\n",
    );
    assert_eq!(hir.class_defs[0].0, "A");
    assert_eq!(hir.class_defs[1].0, "B");
    assert_eq!(hir.class_defs.len(), 2 + BUILTIN_EXCEPTION_CLASSES.len());
}

// -- the reference gate -----------------------------------------------

fn references(source: &str) -> bool {
    module_references_builtin_exception_name(&parse(source))
}

#[test]
fn every_builtin_exception_name_is_recognized_as_a_reference() {
    for name in BUILTIN_EXCEPTION_CLASSES {
        assert!(references(&format!("x = {name}\n")), "`{name}` must count");
    }
}

#[test]
fn a_module_naming_none_of_the_seven_references_nothing() {
    assert!(!references("print(1)\n"));
    assert!(!references(
        "class A:\n    def __init__(self) -> None:\n        pass\n"
    ));
    // A *similar* name is not one of the seven.
    assert!(!references("x = ValueErrors\n"));
    // A string that merely spells one is not a reference: `annotation_to_ty`
    // does not resolve string forward references either.
    assert!(!references("x = \"ValueError\"\n"));
}

#[test]
fn every_spelling_that_can_reach_the_class_table_counts_as_a_reference() {
    // Each of these is a position from which the frontend consults the class
    // table for one of the seven, so each must seed the module. A spelling
    // missed here fails silently -- as a spurious `C0001` or an
    // internal-compiler-error abort -- which is why the scan is ruff's own
    // exhaustive walker rather than a hand-rolled match.
    for source in [
        // a base class
        "class MyError(ValueError):\n    pass\n",
        // a `raise` operand
        "raise ValueError(\"boom\")\n",
        // an `except` type, at nested depth
        "def f() -> None:\n    try:\n        pass\n    except ValueError:\n        pass\n",
        // an `except ... as` type inside a nested function
        "def outer() -> None:\n    def inner() -> None:\n        try:\n            pass\n        except KeyError as e:\n            print(e)\n",
        // a parameter annotation
        "def f(e: ValueError) -> None:\n    return\n",
        // a return annotation
        "def f() -> TypeError:\n    return\n",
        // an annotated assignment
        "x: IndexError = 1\n",
        // an `isinstance` argument
        "def f(x: int) -> None:\n    print(isinstance(x, RuntimeError))\n",
        // an `issubclass` argument
        "def f() -> None:\n    print(issubclass(ZeroDivisionError, Exception))\n",
        // attribute access on the bare class name
        "def f() -> None:\n    print(ValueError.args)\n",
        // a type alias
        "type Alias = ValueError\n",
        // buried inside a comprehension
        "xs = [ValueError for i in range(3)]\n",
        // buried inside an f-string interpolation
        "s = f\"{Exception}\"\n",
        // buried inside a subscript
        "x = holder[KeyError]\n",
        // a decorator
        "@Exception\ndef f() -> None:\n    return\n",
    ] {
        assert!(references(source), "must be a reference: {source:?}");
    }
}

#[test]
fn a_bare_shadowing_class_is_not_seeded_on_the_shadow_gate_alone() {
    // The shadow gate alone decides this source: the reference scan never
    // sees it. `ReferenceScan` overrides only `visit_expr`, and this source
    // contains no `Expr::Name` at all -- a `ClassDef`'s own name is a bare
    // `Identifier` on the statement node, and the `-> None` annotation
    // parses as `Expr::NoneLiteral`, not as a name.
    let source = "class ValueError:\n    def __init__(self) -> None:\n        pass\n";
    assert!(!references(source));
    assert!(shadows(source));
    let hir = lower(source);
    assert!(!hir.seeded_builtin_exception_classes);
    assert_eq!(hir.class_defs.len(), 1);
}

#[test]
fn a_module_passing_the_reference_gate_is_still_blocked_by_the_shadow_gate() {
    // Both gates are exercised here: the annotation's `Expr::Name` makes
    // this a genuine reference, and the class's own name makes it a shadow.
    // Shadowing wins, so nothing is seeded -- and the annotation resolves
    // against the user's own class.
    let source = "class ValueError:\n    def __init__(self) -> None:\n        pass\n\ndef f(e: ValueError) -> None:\n    pass\n";
    assert!(references(source));
    assert!(shadows(source));
    let hir = lower(source);
    assert!(!hir.seeded_builtin_exception_classes);
}

#[test]
fn a_module_that_shadows_one_name_is_seeded_with_none_of_them() {
    let hir = lower("class ValueError:\n    def __init__(self) -> None:\n        pass\n");
    assert_eq!(hir.class_defs.len(), 1);
    assert_eq!(hir.class_defs[0].0, "ValueError");
    assert!(class_named(&hir, "Exception").is_none());
    assert!(
        !hir.items
            .iter()
            .any(|item| matches!(item, HirItem::Function { name, .. } if name
                == EXCEPTION_INIT_MANGLED_NAME))
    );
}

#[test]
fn a_user_class_can_inherit_from_a_builtin_exception_class() {
    let hir = lower("class MyError(ValueError):\n    pass\n");
    let def = class_named(&hir, "MyError").expect("`MyError` must be lowered");
    assert_eq!(def.bases, vec!["ValueError".to_string()]);
    assert_eq!(
        def.mro,
        vec![
            "MyError".to_string(),
            "ValueError".to_string(),
            "Exception".to_string()
        ]
    );
}

#[test]
fn the_synthetic_constructor_is_emitted_only_when_a_user_class_inherits_it() {
    let without = lower("try:\n    raise ValueError(\"x\")\nexcept ValueError:\n    print(1)\n");
    assert!(
        !without
            .items
            .iter()
            .any(|item| matches!(item, HirItem::Function { name, .. } if name
                == EXCEPTION_INIT_MANGLED_NAME)),
        "a module with no builtin-exception subclass needs no constructor body"
    );

    let with = lower("class MyError(ValueError):\n    pass\n");
    assert_eq!(
        with.items
            .iter()
            .filter(|item| matches!(item, HirItem::Function { name, .. } if name
                == EXCEPTION_INIT_MANGLED_NAME))
            .count(),
        1,
        "a module with a builtin-exception subclass gets exactly one constructor body"
    );
}

#[test]
fn a_user_class_may_inherit_from_two_builtin_exception_classes() {
    let hir = lower("class E(ValueError, TypeError):\n    pass\n");
    let def = class_named(&hir, "E").expect("`E` must be lowered");
    assert_eq!(
        def.mro,
        vec![
            "E".to_string(),
            "ValueError".to_string(),
            "TypeError".to_string(),
            "Exception".to_string()
        ]
    );
}

// -- the seeded definitions take part in every existing name check ---

#[test]
fn a_class_after_a_shadowing_alias_is_still_rejected() {
    // The pre-scan sees the `type Exception = int` binding and seeds
    // nothing, so this stays the ordinary alias-vs-class collision it was
    // before #541 rather than a collision against a synthetic definition.
    let err = lower_checked(&parse(
        "type Exception = int\n\nclass Exception:\n    def __init__(self) -> None:\n        pass\n",
    ))
    .expect_err("a class colliding with an alias must be rejected");
    assert_eq!(err.code, "C0001");
}
