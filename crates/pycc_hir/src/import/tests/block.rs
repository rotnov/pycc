//! A CPython-backed `import` nested in a module-level `if`/`try` block
//! (#1291): which nested imports lower to `HirStmt::ForeignImport`, which
//! keep a diagnostic, and what the driver is asked to answer.

use super::*;
use crate::{HirItem, HirStmt};

/// Lowers `source` the way the driver answers it: a request for a name in
/// `found` is a project module, any other undotted plain `import` is
/// foreign, and everything else is left unanswered.
fn lower_with_driver(source: &str, found: &[&str]) -> Result<LoweredModule, Vec<Diagnostic>> {
    let parsed = parse(source);
    let mut resolved = ResolvedImports::default();
    for request in project_import_requests(&parsed) {
        let Some(module) = request.module.as_deref() else {
            continue;
        };
        if found.contains(&module) {
            resolved.insert(request.span, ResolvedImport::Found);
        } else if request.names.is_empty() && request.level == 0 && !module.contains('.') {
            resolved.insert(request.span, ResolvedImport::Foreign);
        }
    }
    lower_module(&parsed, &resolved, None)
}

fn lower_ok(source: &str) -> LoweredModule {
    lower_with_driver(source, &[])
        .unwrap_or_else(|diagnostics| panic!("{source:?} must lower: {diagnostics:#?}"))
}

fn errors(source: &str, found: &[&str]) -> Vec<Diagnostic> {
    lower_with_driver(source, found).expect_err("fixture must fail to lower")
}

fn only_message(source: &str) -> String {
    let diagnostics = errors(source, &[]);
    assert_eq!(diagnostics.len(), 1, "{source:?}: {diagnostics:#?}");
    diagnostics[0].message.clone()
}

/// Every `ForeignImport` node reached through `if`/`try` nesting, in
/// source order.
fn nested_nodes(module: &LoweredModule) -> Vec<Vec<(String, String)>> {
    fn walk(stmts: &[HirStmt], out: &mut Vec<Vec<(String, String)>>) {
        for stmt in stmts {
            match stmt {
                HirStmt::ForeignImport { bindings, .. } => out.push(bindings.clone()),
                HirStmt::If { body, orelse, .. } => {
                    walk(body, out);
                    walk(orelse, out);
                }
                HirStmt::Try {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                }
                | HirStmt::TryStar {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                } => {
                    walk(body, out);
                    for handler in handlers {
                        walk(&handler.body, out);
                    }
                    walk(orelse, out);
                    walk(finalbody, out);
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for item in &module.hir.items {
        if let HirItem::TopLevelStmt(stmt) = item {
            walk(std::slice::from_ref(stmt), &mut out);
        }
    }
    out
}

fn foreign_sites(module: &LoweredModule) -> Vec<(&str, &str, ForeignImportSite)> {
    module
        .hir
        .imports
        .iter()
        .filter_map(|binding| match binding {
            ImportBinding::Foreign {
                local_name,
                module_path,
                site,
                ..
            } => Some((local_name.as_str(), module_path.as_str(), *site)),
            _ => None,
        })
        .collect()
}

fn pair(local: &str, module: &str) -> Vec<(String, String)> {
    vec![(local.to_string(), module.to_string())]
}

const BLOCK_TEXT: &str = "an `import` inside a block body";

#[test]
fn an_import_in_an_if_body_lowers_to_a_foreign_import_node() {
    let source = "if c:\n    import colorsys\n";
    let lowered = lower_ok(source);
    let statement = Span::new(10, source.trim_end().len() as u32);
    assert_eq!(
        lowered.hir.items,
        vec![HirItem::TopLevelStmt(HirStmt::If {
            test: crate::HirExpr::Name("c".to_string()),
            body: vec![HirStmt::ForeignImport {
                bindings: pair("colorsys", "colorsys"),
                span: statement,
            }],
            orelse: vec![],
        })]
    );
    assert_eq!(
        lowered.hir.imports,
        vec![ImportBinding::Foreign {
            local_name: "colorsys".to_string(),
            module_path: "colorsys".to_string(),
            site: ForeignImportSite::Block,
            span: statement,
        }]
    );
}

#[test]
fn every_try_and_elif_position_lowers_a_nested_import() {
    for source in [
        "try:\n    import colorsys\nexcept Exception:\n    pass\n",
        "try:\n    pass\nexcept Exception:\n    import colorsys\n",
        "try:\n    pass\nexcept Exception:\n    pass\nelse:\n    import colorsys\n",
        "try:\n    pass\nfinally:\n    import colorsys\n",
        "try:\n    pass\nexcept* ValueError:\n    import colorsys\n",
        "try:\n    if c:\n        import colorsys\nexcept Exception:\n    pass\n",
        "if c:\n    pass\nelif d:\n    import colorsys\n",
    ] {
        let lowered = lower_ok(source);
        assert_eq!(
            nested_nodes(&lowered),
            vec![pair("colorsys", "colorsys")],
            "{source:?}"
        );
        assert_eq!(
            foreign_sites(&lowered),
            vec![("colorsys", "colorsys", ForeignImportSite::Block)],
            "{source:?}"
        );
    }
}

#[test]
fn a_type_checking_body_binds_nothing_and_its_else_is_admitted() {
    let lowered = lower_ok(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import colorsys\nelse:\n    \
         import json\n",
    );
    assert_eq!(nested_nodes(&lowered), vec![pair("json", "json")]);
    assert_eq!(
        foreign_sites(&lowered),
        vec![("json", "json", ForeignImportSite::Block)]
    );

    // The same guard as an `elif` test.
    let lowered = lower_ok(
        "from typing import TYPE_CHECKING\nif c:\n    pass\nelif TYPE_CHECKING:\n    \
         import colorsys\n",
    );
    assert!(foreign_sites(&lowered).is_empty());
    assert!(nested_nodes(&lowered).is_empty());
}

#[test]
fn an_aliased_foreign_import_binds_the_alias_at_top_level_and_nested() {
    let lowered = lower_ok("import colorsys as c\n");
    assert_eq!(
        foreign_sites(&lowered),
        vec![("c", "colorsys", ForeignImportSite::Item(0))]
    );
    let lowered = lower_ok("if x:\n    import colorsys as c\n");
    assert_eq!(
        foreign_sites(&lowered),
        vec![("c", "colorsys", ForeignImportSite::Block)]
    );
    assert_eq!(nested_nodes(&lowered), vec![pair("c", "colorsys")]);
}

#[test]
fn nested_imports_that_are_not_all_foreign_keep_a_diagnostic() {
    for source in [
        "if c:\n    import math\n",
        "if c:\n    import colorsys, math\n",
        "if c:\n    from colorsys import hls_to_rgb\n",
    ] {
        let message = only_message(source);
        assert!(message.contains(BLOCK_TEXT), "{source:?}: {message}");
    }
    assert_eq!(
        only_message("if c:\n    import os.path\n"),
        "import of module `os.path` is not supported yet"
    );
    let diagnostics = errors("if c:\n    import pkg\n", &["pkg"]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(
        diagnostics[0].message.contains("module namespace"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn a_loop_body_import_keeps_the_block_diagnostic() {
    // A `with` statement is refused as a whole before its body is lowered.
    for source in [
        "for i in range(3):\n    import colorsys\n",
        "while c:\n    import colorsys\n",
    ] {
        let message = only_message(source);
        assert!(message.contains(BLOCK_TEXT), "{source:?}: {message}");
    }
}

#[test]
fn a_failed_block_reports_in_source_order_and_binds_nothing() {
    // The block fails after its import lowered: the unsupported statement
    // is the only diagnostic, and the truncated bindings mean the later
    // top-level `colorsys = 1` is not reported as shadowing anything.
    let diagnostics = errors(
        "if c:\n    import colorsys\n    lambda: 1\ncolorsys = 1\n",
        &[],
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(
        !diagnostics[0].message.contains("import"),
        "{}",
        diagnostics[0].message
    );

    // An earlier unsupported statement is reported, not the later import's
    // deferred diagnostic.
    let message = only_message("if c:\n    lambda: 1\n    import os.path\n");
    assert!(!message.contains("os.path"), "{message}");

    // The import comes first: its own deferred diagnostic replaces the
    // catch-all block text.
    assert_eq!(
        only_message("if c:\n    import os.path\n    lambda: 1\n"),
        "import of module `os.path` is not supported yet"
    );
}

#[test]
fn an_alias_shadowing_a_resolved_spelling_is_refused() {
    for alias in ["typing", "TYPE_CHECKING", "math", "range"] {
        for source in [
            format!("import colorsys as {alias}\n"),
            format!("if c:\n    import colorsys as {alias}\n"),
        ] {
            let message = only_message(&source);
            assert_eq!(
                message,
                format!(
                    "binding the CPython module `colorsys` to `{alias}`, a name pycc resolves \
                     by its spelling (a stdlib module, `range` or `TYPE_CHECKING`), \
                     is not supported yet"
                ),
                "{source:?}"
            );
        }
    }
}

/// The optional-dependency idiom, pinned so a later part of #1282 changes
/// it deliberately: the `except` arm's assignment is a second top-level
/// definition of the imported name.
#[test]
fn the_optional_dependency_idiom_is_a_shadowing_refusal_today() {
    assert_eq!(
        only_message("try:\n    import colorsys\nexcept Exception:\n    colorsys = None\n"),
        "`colorsys` is bound both by a foreign `import` and by another top-level statement in \
         this module; shadowing a foreign import is not supported yet"
    );
}

#[test]
fn shadowing_a_nested_import_follows_the_top_level_rule() {
    let message = only_message("if c:\n    import colorsys\ncolorsys = 1\n");
    assert!(message.contains("shadowing a foreign import"), "{message}");

    let lowered = lower_ok("if c:\n    import colorsys\nelse:\n    import colorsys\n");
    assert_eq!(
        nested_nodes(&lowered),
        vec![pair("colorsys", "colorsys"), pair("colorsys", "colorsys")]
    );

    let message = only_message("import colorsys as x\nimport json as x\n");
    assert!(message.contains("shadowing a foreign import"), "{message}");

    lower_ok("import numpy\nimport numpy\n");
}

#[test]
fn nested_and_aliased_imports_are_requested_under_their_alias_spans() {
    let source = "if c:\n    import a as x\ntry:\n    import b\nexcept Exception:\n    \
                  from d import e\n";
    let requests = project_import_requests(&parse(source));
    let modules: Vec<&str> = requests
        .iter()
        .filter_map(|request| request.module.as_deref())
        .collect();
    assert_eq!(modules, vec!["a", "b"]);
    let alias_start = |needle: &str| source.find(needle).expect("needle") as u32;
    assert_eq!(requests[0].span.start, alias_start("a as x"));
    assert_eq!(requests[1].span.start, alias_start("b\n"));
}

#[test]
fn function_and_loop_body_imports_are_not_requested_but_type_checking_ones_are() {
    let requests = project_import_requests(&parse(
        "def f() -> None:\n    import a\nfor i in range(3):\n    import b\nwhile c:\n    \
         import g\n",
    ));
    assert!(requests.is_empty(), "{requests:#?}");

    let requests = project_import_requests(&parse(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import a\nelif c:\n    \
         import b\nelse:\n    import d\n",
    ));
    let modules: Vec<&str> = requests
        .iter()
        .filter_map(|request| request.module.as_deref())
        .collect();
    assert_eq!(modules, vec!["a", "b", "d"]);
}
