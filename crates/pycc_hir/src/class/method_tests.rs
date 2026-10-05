//! Unit tests for the method part of #1140: a default parameter value on a
//! method is admitted under the module-level rules and recorded in
//! `HirClassDef::method_defaults` for the `--ext` host boundary.

use pycc_diag::Diagnostic;

use crate::{HirClassDef, HirExpr, ResolvedImports, lower_module};

/// Lowers `source` as an `ext` module (`ext == true`) or a `native` one.
fn lower(source: &str, ext: bool) -> Result<crate::HirModule, Vec<Diagnostic>> {
    let parsed = crate::pycc_parser_test_helper::parse(source);
    let mut resolved = ResolvedImports::default();
    resolved.set_ext_module(ext);
    lower_module(&parsed, &resolved, None).map(|lowered| lowered.hir)
}

fn lowered(source: &str, ext: bool) -> crate::HirModule {
    lower(source, ext).unwrap_or_else(|diagnostics| panic!("{source:?}: {diagnostics:#?}"))
}

fn only_error(source: &str, ext: bool) -> Diagnostic {
    let diagnostics = lower(source, ext).expect_err("the fixture must be rejected");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    diagnostics.into_iter().next().expect("one diagnostic")
}

fn class<'m>(module: &'m crate::HirModule, name: &str) -> &'m HirClassDef {
    module
        .class_defs
        .iter()
        .find_map(|(held, def)| (held == name).then_some(def))
        .unwrap_or_else(|| panic!("no class `{name}`"))
}

/// `name`'s recorded defaults in class `C`, `None` when it has no entry.
fn defaults_of(module: &crate::HirModule, name: &str) -> Option<Vec<Option<HirExpr>>> {
    class(module, "C")
        .method_defaults
        .iter()
        .find_map(|(held, defaults)| (held == name).then(|| defaults.clone()))
}

#[test]
fn each_method_kind_records_its_defaults_parallel_to_its_full_parameters() {
    let module = lowered(
        "class C:\n\
         \x20   def __init__(self, a: int, b: int = 2, c: str = 'x') -> None:\n        return\n\
         \x20   def m(self, a: bool = True, /, b: float = -1.5) -> None:\n        return\n\
         \x20   @staticmethod\n    def s(a: int, b: int = -3) -> int:\n        return a\n\
         \x20   @classmethod\n    def k(cls, a: int | None = None) -> None:\n        return\n\
         \x20   def plain(self, a: int) -> None:\n        return\n",
        false,
    );
    assert_eq!(
        defaults_of(&module, "C.__init__"),
        Some(vec![
            None,
            None,
            Some(HirExpr::IntLiteral(2)),
            Some(HirExpr::StringLiteral("x".to_string())),
        ])
    );
    // A positional-only defaulted parameter is recorded exactly as an
    // ordinary one: the host passes both positionally.
    assert_eq!(
        defaults_of(&module, "C.m"),
        Some(vec![
            None,
            Some(HirExpr::BoolLiteral(true)),
            Some(HirExpr::FloatLiteral(-1.5)),
        ])
    );
    // A `@staticmethod` has no receiver entry.
    assert_eq!(
        defaults_of(&module, "C.s.static"),
        Some(vec![None, Some(HirExpr::IntLiteral(-3))])
    );
    assert_eq!(
        defaults_of(&module, "C.k.classmethod"),
        Some(vec![None, Some(HirExpr::NoneLiteral)])
    );
    // A method without a default records nothing.
    assert_eq!(defaults_of(&module, "C.plain"), None);
}

#[test]
fn a_redefinition_replaces_or_removes_the_recorded_defaults() {
    let module = lowered(
        "class C:\n\
         \x20   def m(self, a: int = 1) -> None:\n        return\n\
         \x20   def m(self, a: int = 5) -> None:\n        return\n\
         \x20   def n(self, a: int = 1) -> None:\n        return\n\
         \x20   def n(self, a: int) -> None:\n        return\n",
        false,
    );
    assert_eq!(
        defaults_of(&module, "C.m"),
        Some(vec![None, Some(HirExpr::IntLiteral(5))])
    );
    assert_eq!(defaults_of(&module, "C.n"), None);
}

#[test]
fn an_unadmitted_method_default_keeps_the_module_level_capability_error() {
    for default in ["[]", "{}", "f()", "1 + 2"] {
        let source =
            format!("class C:\n    def m(self, a: int = {default}) -> None:\n        return\n");
        let diagnostic = only_error(&source, false);
        assert_eq!(diagnostic.code, "C0001", "{source}");
        assert_eq!(
            diagnostic.message,
            crate::func::params::UNADMITTED_DEFAULT,
            "{source}"
        );
    }
}

#[test]
fn an_unassignable_method_default_is_a_type_error() {
    let diagnostic = only_error(
        "class C:\n    def m(self, a: int = 'x') -> None:\n        return\n",
        false,
    );
    assert_eq!(diagnostic.code, "T0021");
    assert_eq!(
        diagnostic.message,
        "default value of parameter `a` of `m` expects `int`, got `str`"
    );
}

#[test]
fn none_defaults_an_object_parameter_of_a_method_but_not_of_a_function() {
    let module = lowered(
        "from typing import Any\n\
         class C:\n\
         \x20   def __init__(self, a: Any = None, b: object = None) -> None:\n        return\n",
        true,
    );
    assert_eq!(
        defaults_of(&module, "C.__init__"),
        Some(vec![
            None,
            Some(HirExpr::NoneLiteral),
            Some(HirExpr::NoneLiteral),
        ])
    );
    // A module-level default is spliced into a native call, so it keeps the
    // strict rule.
    let diagnostic = only_error(
        "from typing import Any\ndef f(a: Any = None) -> None:\n    return\n",
        true,
    );
    assert_eq!(diagnostic.code, "T0021");
    assert_eq!(
        diagnostic.message,
        "default value of parameter `a` of `f` expects `object`, got `None`"
    );
}

#[test]
fn only_none_is_admitted_at_an_object_method_parameter() {
    let diagnostic = only_error(
        "from typing import Any\n\
         class C:\n    def m(self, a: Any = 1) -> None:\n        return\n",
        true,
    );
    assert_eq!(diagnostic.code, "T0021");
    assert_eq!(
        diagnostic.message,
        "default value of parameter `a` of `m` expects `object`, got `int`"
    );
}
