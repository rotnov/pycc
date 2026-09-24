//! The request half of the driver contract for project and foreign
//! imports (#898, D-222): the scan of a parsed module's top-level
//! statements for the imports the driver must resolve on the filesystem
//! before `module::lower_module` runs. Extracted from `import.rs` (#1291)
//! with no logic change.

use super::{is_future_import, statement_span};
use pycc_ast::{ModModule, Stmt};
use pycc_diag::Span;

/// One project-import request -- a whole module-level `from ... import`
/// statement, or one qualifying alias of a plain `import` (#1280) -- that
/// `pycc_std`'s registry does not answer, so the driver must resolve it on
/// the filesystem before
/// `module::lower_module` runs (#898, D-222). `pycc_hir` itself never
/// touches the filesystem: this is the request half of the contract, and
/// [`ResolvedImports`] is the answer half.
///
/// `names` is empty exactly for a bare `import m` (which binds a module
/// namespace, a shape Part 1 only recognizes) and lists every imported
/// name, in source order, for `from ... import a, b`. `module` is `None`
/// only for a relative `from . import x` with no module segment. `span` is
/// the key the driver answers under: the whole statement's span for a
/// `from ... import`, and the one alias's own span for a plain `import`,
/// which yields one request per qualifying alias so `import sys, re` gets
/// an answer for each module (#1280).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectImportRequest {
    pub level: u32,
    pub module: Option<String>,
    pub names: Vec<String>,
    pub span: Span,
}

/// Scans a parsed module's top-level statements for the imports the driver
/// must resolve: every relative `from` import, and every absolute
/// `import`/`from ... import` naming a module `pycc_std::resolve_module`
/// rejects. A plain `import` contributes one request per alias that has no
/// `as` and that `pycc_std` does not resolve, so each name of `import a, b`
/// is asked about on its own (#1280). Everything else (stdlib imports,
/// `import ... as ...`, non-import statements) is left to
/// [`lower_import_stmt`]'s own single-file dispatch, so a module with no
/// project import yields an empty list and lowers exactly as before.
pub fn project_import_requests(module: &ModModule) -> Vec<ProjectImportRequest> {
    module
        .body
        .iter()
        .flat_map(project_import_request)
        .collect()
}

fn project_import_request(stmt: &Stmt) -> Vec<ProjectImportRequest> {
    match stmt {
        Stmt::Import(import) => import
            .names
            .iter()
            .filter(|alias| {
                alias.asname.is_none() && pycc_std::resolve_module(alias.name.as_str()).is_none()
            })
            .map(|alias| ProjectImportRequest {
                level: 0,
                module: Some(alias.name.to_string()),
                names: Vec::new(),
                span: statement_span(alias.range),
            })
            .collect(),
        Stmt::ImportFrom(import) => {
            // A `from __future__ import ...` is a compiler directive, not a
            // module (D-229): the driver must never probe the project for a
            // sibling `__future__.py`, which CPython would only reach at
            // run time, *after* applying the directive.
            if is_future_import(import) {
                return Vec::new();
            }
            let module = import.module.as_ref().map(ToString::to_string);
            if import.level == 0
                && module
                    .as_deref()
                    .is_some_and(|name| pycc_std::resolve_module(name).is_some())
            {
                return Vec::new();
            }
            vec![ProjectImportRequest {
                level: import.level,
                module,
                names: import
                    .names
                    .iter()
                    .map(|alias| alias.name.to_string())
                    .collect(),
                span: statement_span(import.range),
            }]
        }
        _ => Vec::new(),
    }
}
