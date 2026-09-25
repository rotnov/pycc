//! #1325: binding a CPython object value to a module-level name, and a
//! bare-name `for` over such a name.
//!
//! Kept apart from `foreign/tests.rs` (see #1314); the fixture helpers are
//! copied from `call_tests.rs` because a sibling module cannot reach them.

/// Lowers `source` with every `import` request answered `Foreign`, the way
/// the driver answers an undotted non-`pycc_std` root.
fn lower_all_foreign(source: &str) -> pycc_hir::HirModule {
    let module = pycc_parser::parse(source).expect("test source must parse");
    let mut resolved = pycc_hir::ResolvedImports::default();
    for request in pycc_hir::project_import_requests(&module) {
        resolved.insert(request.span, pycc_hir::ResolvedImport::Foreign);
    }
    pycc_hir::lower_module(&module, &resolved, None)
        .unwrap_or_else(|diagnostics| panic!("{source:?} must lower: {diagnostics:#?}"))
        .hir
}

const FROM_FORM: &str = "from itertools import product\n";

/// The phrase `check_assignment`'s function-body guard reports.
const BINDING: &str = "binding a CPython object to a name";

fn admitted(source: &str) {
    crate::check_all(&lower_all_foreign(source))
        .unwrap_or_else(|diagnostics| panic!("{source:?}: {diagnostics:#?}"));
}

/// The single diagnostic `source` is refused with, asserted by code and a
/// message phrase.
fn refused(source: &str, code: &str, phrase: &str) {
    let diagnostics = crate::check_all(&lower_all_foreign(source)).expect_err(source);
    assert_eq!(diagnostics.len(), 1, "{source:?}: {diagnostics:#?}");
    assert_eq!(diagnostics[0].code, code, "{source:?}: {diagnostics:#?}");
    assert!(
        diagnostics[0].message.contains(phrase),
        "{source:?}: {diagnostics:#?}"
    );
}

/// Every producer shape binds at module scope, and the bound name is
/// itself an `object` other names can alias.
#[test]
fn a_module_level_binding_is_admitted_for_every_producer() {
    admitted(&format!("{FROM_FORM}x = product(\"ab\")\ny = x\n"));
    admitted("import m\nx = m.f()\n");
    admitted("import m\nx = m.pi\n");
    admitted("import m\nx = m.pi[0]\n");
    admitted("import m\nx = m\n");
    // Rebinding to another `object` is the same type, so no `T0023`.
    admitted(&format!(
        "{FROM_FORM}x = product(\"ab\")\nx = product(\"c\")\n"
    ));
}

/// A bare-name `for` over an `object` name: nested (the inner loop's
/// iterable is the outer loop's `object` target), repeated over the same
/// name, and over a from-imported foreign name directly.
#[test]
fn a_bare_name_for_over_an_object_is_admitted() {
    admitted(&format!(
        "{FROM_FORM}x = product(\"ab\")\nfor t in x:\n    for u in t:\n        pass\nfor t in x:\n    pass\n"
    ));
    admitted("from sys import path\nfor p in path:\n    pass\n");
}

/// The binding keeps the name's type fixed, so mixing `object` with any
/// other type is the ordinary redefinition refusal, in either order and
/// against either annotation form.
#[test]
fn a_type_change_is_refused_like_any_other() {
    refused(
        &format!("{FROM_FORM}x = 1\nx = product()\n"),
        "T0023",
        "cannot assign `object` to `x`, previously inferred as `int`",
    );
    refused(
        &format!("{FROM_FORM}x = product()\nx = 1\n"),
        "T0023",
        "cannot assign `int` to `x`, previously inferred as `object`",
    );
    refused(
        &format!("{FROM_FORM}x: int\nx = product()\n"),
        "T0026",
        "cannot assign `object` to `x`, previously declared as `x: int`",
    );
    refused(
        &format!("{FROM_FORM}x: int = product()\n"),
        "T0025",
        "cannot assign `object` to `x: int`",
    );
}

/// A binding on only one path leaves the name maybe-bound, which both a
/// read and a loop over it refuse.
#[test]
fn a_maybe_bound_object_name_is_refused_by_a_read_and_a_loop() {
    let prefix = format!("{FROM_FORM}import sys\nif len(sys.argv) > 5:\n    x = product()\n");
    for tail in ["y = x\n", "for t in x:\n    pass\n"] {
        refused(
            &format!("{prefix}{tail}"),
            "T0041",
            "may not be bound on every path reaching this use",
        );
    }
}

/// The binding is module-level only: every function-body shape -- an
/// annotated `def`, a method, an unannotated private helper and a generic
/// function -- keeps the `I0404` (#1333).
#[test]
fn a_function_body_binding_is_still_refused() {
    for source in [
        format!("{FROM_FORM}def g() -> None:\n    x = product()\n"),
        format!("{FROM_FORM}class C:\n    def m(self) -> None:\n        x = product()\n"),
        format!("{FROM_FORM}def _h():\n    x = product()\n    return 1\n"),
        format!(
            "{FROM_FORM}def g[T](a: T) -> T:\n    x = product()\n    return a\n\n\nprint(g(1))\n"
        ),
    ] {
        refused(&source, "I0404", BINDING);
    }
}

/// A function body may not read a module-level `object` name the module
/// bound by assignment rather than by a foreign import, whether to alias it
/// or to iterate it (#1333); nor may a comprehension iterate it anywhere.
#[test]
fn reading_a_bound_object_name_outside_the_admitted_positions_is_refused() {
    let phrase = "using `x`, which is bound to a CPython object";
    for tail in [
        "def g() -> None:\n    y = x\n",
        "def g() -> None:\n    for t in x:\n        pass\n",
        "ys = [t for t in x]\n",
    ] {
        refused(
            &format!("{FROM_FORM}x = product()\n{tail}"),
            "I0404",
            phrase,
        );
    }
}

/// `del` keeps its own pre-existing refusal: releasing the object could run
/// a foreign finalizer.
#[test]
fn deleting_a_bound_object_name_keeps_its_own_refusal() {
    refused(
        &format!("{FROM_FORM}x = product()\ndel x\n"),
        "C0001",
        "it holds a CPython object",
    );
}

/// The constraint solver sees the module-level binding and loop alongside
/// an unannotated helper it has to infer, and neither disturbs the other.
#[test]
fn an_unannotated_helper_beside_the_binding_still_solves() {
    admitted(&format!(
        "{FROM_FORM}def _h(a):\n    return a\n\n\nx = product()\nfor t in x:\n    pass\nprint(_h(1))\n"
    ));
}

/// The monomorphization pass re-walks the module body without the foreign
/// names (the gap `foreign/tests.rs`'s
/// `the_monomorphization_pass_walks_a_for_loop_iterable` pins for an
/// attribute iterable), so in a module that also defines a generic function
/// the binding the check phase admits is refused there with `T0021`. #1325
/// widens that pre-existing gap to the binding rather than opening it; #1101
/// tracks it.
#[test]
fn a_module_with_a_generic_function_refuses_the_binding_in_monomorphization() {
    let source = format!(
        "{FROM_FORM}def g[T](a: T) -> T:\n    return a\n\n\nx = product(\"ab\")\nprint(g(1))\n"
    );
    let hir = lower_all_foreign(&source);
    assert!(crate::check_all(&hir).is_ok(), "the check phase admits it");
    let Err(diagnostics) = crate::check_and_resolve_all_keyed(&hir) else {
        panic!("monomorphization refuses the binding");
    };
    let [(_, diagnostic)] = diagnostics.as_slice() else {
        panic!("exactly one diagnostic: {diagnostics:?}");
    };
    assert_eq!(diagnostic.code, "T0021", "{diagnostic:?}");
    assert!(diagnostic.message.contains("`product`"), "{diagnostic:?}");
}
