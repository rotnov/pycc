//! Generated-text tests for the compiled-class `isinstance` the `--ext`
//! build emits for Part 7 of #1371: the published-family dispatch and the
//! per-class type-object statics it reads.
//!
//! Kept out of `generated_c.rs`, which is past `AGENTS.md`'s ~1,000-line
//! decomposability threshold.

use super::generated_c::{inc_no_classes, static_export};
use super::*;

/// Part 7 of #1371: a class name answers through every published type
/// whose MRO contains it, in publication order -- a base through itself
/// and its published subclass, a subclass through itself only -- each
/// shared ancestor gets one test per name, not one per class, and `object`,
/// never a compiled class argument, gets none.
#[test]
fn the_compiled_class_isinstance_tests_each_published_type_whose_mro_names_the_class() {
    let published = |class: &str, mro: &[&str]| ExtPublishedClass {
        class: class.to_string(),
        methods: vec![static_export(class, "m", vec![], Ty::Int)],
        mro: mro.iter().map(|name| name.to_string()).collect(),
    };
    let c = compiled_class_isinstance_c(&[
        published("Base", &["Base", "object"]),
        published("Derived", &["Derived", "Base", "object"]),
    ]);
    let test = |class: &str| {
        format!(
            "        found = PyObject_IsInstance(o, pycc_ext_type_object_{class});\n        \
             if (found != 0) {{\n            return found;\n        }}\n"
        )
    };
    let expected = format!(
        "{COMPILED_CLASS_ISINSTANCE_DECL}\n{{\n    int found;\n    \
         if (strcmp(name, \"Base\") == 0) {{\n{base}{derived}        return 0;\n    }}\n    \
         if (strcmp(name, \"Derived\") == 0) {{\n{derived}        return 0;\n    }}\n    \
         return {UNPUBLISHED_CLASS_ISINSTANCE}(o);\n}}\n\n",
        base = test("Base"),
        derived = test("Derived"),
    );
    assert_eq!(c, expected);
}

#[test]
fn every_published_class_keeps_its_type_object_in_a_file_static() {
    let inc = inc_no_classes(
        "m",
        &[static_export("Grid", "scale", vec![Ty::Int], Ty::Int)],
    );
    assert!(
        inc.contains("static PyObject *pycc_ext_type_object_Grid;\n"),
        "{inc}"
    );
    // The static is declared before the `isinstance` that reads it.
    let declared = inc
        .find("static PyObject *pycc_ext_type_object_Grid;")
        .unwrap();
    let read = inc.find(COMPILED_CLASS_ISINSTANCE_DECL).unwrap();
    assert!(declared < read, "{inc}");
}
