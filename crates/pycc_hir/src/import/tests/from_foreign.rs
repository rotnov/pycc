//! A foreign `from X import a, b` (#1278): each name binds the CPython
//! object `X.<name>`, one `ImportBinding::Foreign` per name, and every
//! shape the channel does not carry keeps its `C0001`.

use super::*;
use crate::{FromImport, foreign_bound_object, foreign_import_statement, opens_foreign_statement};

/// Lowers `source`, answering every request for a module in `foreign` with
/// `ResolvedImport::Foreign` and leaving every other request unanswered.
fn lower_foreign(source: &str, foreign: &[&str]) -> Result<LoweredModule, Vec<Diagnostic>> {
    let parsed = parse(source);
    let mut resolved = ResolvedImports::default();
    for request in project_import_requests(&parsed) {
        if request
            .module
            .as_deref()
            .is_some_and(|module| foreign.contains(&module))
        {
            resolved.insert(request.span, ResolvedImport::Foreign);
        }
    }
    lower_module(&parsed, &resolved, None)
}

fn only_error(result: Result<LoweredModule, Vec<Diagnostic>>) -> Diagnostic {
    let diagnostics = result.expect_err("fixture must fail to lower");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    diagnostics.into_iter().next().expect("one diagnostic")
}

fn from_import(name: &str, fromlist: &[&str], index: usize) -> FromImport {
    FromImport {
        name: name.to_string(),
        fromlist: fromlist.iter().map(ToString::to_string).collect(),
        index,
    }
}

#[test]
fn every_name_of_a_foreign_from_import_binds_its_own_object_in_order() {
    let source = "from itertools import product, chain\n";
    let lowered = lower_foreign(source, &["itertools"]).expect("must lower");
    let statement = Span::new(0, source.trim_end().len() as u32);
    let expected = vec![
        ImportBinding::Foreign {
            local_name: "product".to_string(),
            module_path: "itertools".to_string(),
            from: Some(from_import("product", &["product", "chain"], 0)),
            site: crate::ForeignImportSite::Item(0),
            span: statement,
        },
        ImportBinding::Foreign {
            local_name: "chain".to_string(),
            module_path: "itertools".to_string(),
            from: Some(from_import("chain", &["product", "chain"], 1)),
            site: crate::ForeignImportSite::Item(0),
            span: statement,
        },
    ];
    assert_eq!(lowered.hir.imports, expected);
}

#[test]
fn a_name_pycc_resolves_by_its_spelling_is_refused() {
    let source = "from builtins import range\n";
    let diagnostic = only_error(lower_foreign(source, &["builtins"]));
    assert_eq!(diagnostic.code, "C0001");
    assert_eq!(
        diagnostic.message,
        "binding the CPython object `builtins.range` to `range`, a name pycc resolves by its \
         spelling (a Python builtin, a stdlib module, or a typing, decorator or base-class \
         marker), is not supported yet"
    );
    assert_eq!(
        diagnostic.span,
        Some(Span::new(0, source.trim_end().len() as u32))
    );
}

#[test]
fn a_spelling_later_in_the_list_refuses_the_whole_statement() {
    let diagnostic = only_error(lower_foreign(
        "from numpy import array, ndarray\n",
        &["numpy"],
    ));
    assert!(
        diagnostic.message.contains("`numpy.ndarray` to `ndarray`"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn an_aliased_foreign_from_import_keeps_its_c0001() {
    let diagnostic = only_error(lower_foreign(
        "from itertools import product as p\n",
        &["itertools"],
    ));
    assert_eq!(
        diagnostic.message,
        "`from ... import x as y` aliasing is not supported yet"
    );
}

#[test]
fn a_wildcard_foreign_from_import_keeps_its_c0001() {
    let diagnostic = only_error(lower_foreign("from itertools import *\n", &["itertools"]));
    assert_eq!(
        diagnostic.message,
        "`from ... import *` (wildcard import) is not supported yet"
    );
}

/// `import copy` binds the module and `from copy import copy` binds the
/// function: the same local name, two different objects.
#[test]
fn a_from_import_shadowing_the_module_import_of_its_own_name_is_refused() {
    let diagnostic = only_error(lower_foreign(
        "import copy\nfrom copy import copy\n",
        &["copy"],
    ));
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic.message.contains("shadowing a foreign import"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn two_from_imports_of_one_name_from_different_modules_are_refused() {
    let diagnostic = only_error(lower_foreign(
        "from json import dumps\nfrom pickle import dumps\n",
        &["json", "pickle"],
    ));
    assert!(
        diagnostic.message.contains("shadowing a foreign import"),
        "{}",
        diagnostic.message
    );
}

/// The same object bound twice is harmless, as `import numpy` twice is.
#[test]
fn the_same_name_imported_twice_from_one_module_lowers() {
    let lowered = lower_foreign(
        "from itertools import product, product\nfrom itertools import product\n",
        &["itertools"],
    )
    .expect("must lower");
    assert_eq!(lowered.hir.imports.len(), 3);
}

#[test]
fn re_exporting_a_dependency_s_foreign_from_import_is_refused() {
    let parsed = parse("from itertools import product\n");
    let mut resolved = ResolvedImports::default();
    for request in project_import_requests(&parsed) {
        resolved.insert(request.span, ResolvedImport::Foreign);
    }
    let fixture = Fixture {
        origin: lower_module(&parsed, &resolved, None)
            .expect("a dependency fixture must lower")
            .hir,
    };
    let diagnostic = fixture.first_error("from dep import product\n", &[]);
    assert_eq!(diagnostic.code, "C0001");
    assert_eq!(
        diagnostic.message,
        "`dep.py` binds `product` to the CPython object `itertools.product`; re-exporting a \
         foreign import across project modules is not supported yet"
    );
}

#[test]
fn the_statement_and_object_renderings_cover_both_forms() {
    let from = from_import("chain", &["product", "chain"], 1);
    assert_eq!(
        foreign_import_statement("itertools", None),
        "import itertools"
    );
    assert_eq!(
        foreign_import_statement("itertools", Some(&from)),
        "from itertools import product, chain"
    );
    assert_eq!(
        foreign_bound_object("itertools", None),
        "the CPython module `itertools`"
    );
    assert_eq!(
        foreign_bound_object("itertools", Some(&from)),
        "the CPython object `itertools.chain`"
    );
    assert!(opens_foreign_statement(None));
    assert!(!opens_foreign_statement(Some(&from)));
    assert!(opens_foreign_statement(Some(&from_import(
        "product",
        &["product", "chain"],
        0
    ))));
}
