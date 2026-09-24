//! Module-level `import` statements and type-alias declarations: the
//! statement kinds `module::lower_all` resolves before it walks a module's
//! remaining items (D-135 for aliases, D-136/D-137 for stdlib imports,
//! D-229 for the `from __future__ import ...` compiler directive, which
//! lowers to nothing and is never treated as a module -- see
//! [`is_future_import`] and [`future_prologue_len`]).
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #547, Part 2). This is a low-fan-in cohesion unit: `lower_import_stmt`,
//! `lower_type_alias_stmt`, and `lower_legacy_type_alias_ann_assign` are
//! each called exactly once, and `import_local_name` twice, all from
//! `module::lower_top_level_item` -- which is why `lib.rs` re-exports them `pub(crate)`
//! rather than making them public. The project-import request/answer types
//! (`ProjectImportRequest`, `ResolvedImports`, #898) are the one public
//! surface here: the driver's `src/modules.rs` fills them in. The dependency runs the other way for
//! annotations: the two alias lowerings call `annotation_to_ty`, which
//! lives in the sibling `func` module.

use crate::class::ClassAnnotationInfo;
use crate::{
    HirClassDef, HirItem, HirModule, ImportBinding, ProjectBindingKind, Ty, annotation_to_ty,
    is_builtin_exception_class, top_level_bound_names, unresolved_symbol, unsupported,
};
use pycc_ast::{Expr, ModModule, Stmt, StmtImportFrom};
use pycc_diag::{Diagnostic, Span};
use std::collections::{BTreeSet, HashMap};

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

/// `true` for an absolute `from __future__ import ...` (#919, D-229). A
/// relative `from .__future__ import x` is an ordinary project import of
/// a module that happens to carry the name.
pub(crate) fn is_future_import(import: &StmtImportFrom) -> bool {
    import.level == 0
        && import
            .module
            .as_ref()
            .is_some_and(|module| module.as_str() == "__future__")
}

/// The `__future__` features CPython 3.14 accepts that lower to nothing
/// here: every one is either mandatory since Python 3.0 (already the
/// language's behaviour) or, for `annotations`, already how pycc evaluates
/// annotations -- statically, at compile time. Exactly
/// `__future__.all_feature_names` minus [`BARRY_AS_FLUFL`].
const NOOP_FUTURE_FEATURES: &[&str] = &[
    "nested_scopes",
    "generators",
    "division",
    "absolute_import",
    "with_statement",
    "print_function",
    "unicode_literals",
    "generator_stop",
    "annotations",
];

/// The one valid `__future__` feature that is *not* a no-op: it changes
/// the grammar (`<>` becomes the inequality operator and `!=` a syntax
/// error), which the vendored parser does not implement, so it stays a
/// `C0001` capability gap rather than an `L0001`.
const BARRY_AS_FLUFL: &str = "barry_as_FLUFL";

/// `true` when `name` is a `__future__` feature the compiler accepts as a
/// no-op -- shared by [`lower_import_stmt`] and `module::poisonable_names`
/// so the poison mirror cannot drift from the lowering's success condition.
pub(crate) fn is_noop_future_feature(name: &str) -> bool {
    NOOP_FUTURE_FEATURES.contains(&name)
}

/// Where a top-level statement sits relative to CPython's future-import
/// prologue: a `from __future__ import ...` is valid only in the
/// [`Prologue`](Self::Prologue), and a `Body` one is a `SyntaxError`
/// (`L0001`) regardless of the names it lists. Computed once per module by
/// [`future_prologue_len`] and threaded down to [`lower_import_stmt`]; only
/// ever matched, so no other trait is derived (each unused derive would be
/// an uncovered region under D-014).
#[derive(Clone, Copy)]
pub(crate) enum FuturePosition {
    Prologue,
    Body,
}

/// The number of leading statements of `body` that may precede or be a
/// future import: an optional docstring at index 0 (a bare
/// `Expr::StringLiteral` expression statement, the same shape
/// `class.rs`'s `__init_subclass__` walk accepts; an f-string or bytes
/// literal is *not* a docstring, matching CPython 3.14) followed by a
/// contiguous run of `from __future__ import ...` statements. A statement
/// at index `>= future_prologue_len(body)` is in [`FuturePosition::Body`].
/// The run is contiguous, so in `future / "doc" / future` the second
/// import is a `Body` one, exactly as CPython reports it.
pub(crate) fn future_prologue_len(body: &[Stmt]) -> usize {
    body.iter()
        .enumerate()
        .position(|(index, stmt)| match stmt {
            Stmt::Expr(expr_stmt) => {
                !(index == 0 && matches!(*expr_stmt.value, Expr::StringLiteral(_)))
            }
            Stmt::ImportFrom(import) => !is_future_import(import),
            _ => true,
        })
        .unwrap_or(body.len())
}

/// The `__future__` arm of [`lower_import_stmt`] (#919, D-229). The
/// precedence ladder is CPython 3.14's, verified against `compile()`:
/// (1) a future import after the prologue is a position `SyntaxError`
/// whatever it names; (2) names are checked left to right -- `braces` is
/// `not a chance`, `*` and any other unknown name is `future feature <name>
/// is not defined` -- all `L0001` via `context_invalid`, CPython's wording
/// verbatim; then, for a statement CPython would accept, (3) an `as` alias
/// is the same `C0001` every other `from ... import x as y` reports (CPython
/// binds a `_Feature` object pycc never models), and (4) `barry_as_FLUFL`
/// is a `C0001` naming the feature. Everything left contributes nothing:
/// no binding, no `HirItem`, and no name is bound -- a deliberate,
/// recorded divergence from CPython, which binds each feature name to its
/// `__future__._Feature` object.
fn lower_future_import(
    import: &StmtImportFrom,
    position: FuturePosition,
) -> Result<LoweredImport, Diagnostic> {
    if let FuturePosition::Body = position {
        return Err(crate::context_invalid(
            "from __future__ imports must occur at the beginning of the file",
            import.range,
        ));
    }
    for alias in &import.names {
        let name = alias.name.as_str();
        if name == "braces" {
            return Err(crate::context_invalid("not a chance", import.range));
        }
        if !(is_noop_future_feature(name) || name == BARRY_AS_FLUFL) {
            return Err(crate::context_invalid(
                format!("future feature {name} is not defined"),
                import.range,
            ));
        }
    }
    for alias in &import.names {
        check_alias_shape(import, alias)?;
        if alias.name.as_str() == BARRY_AS_FLUFL {
            return Err(unsupported(
                "the `barry_as_FLUFL` future feature (`<>` in place of `!=`) is not supported yet",
                import.range,
            ));
        }
    }
    Ok(LoweredImport::default())
}

fn statement_span<R>(range: R) -> Span
where
    std::ops::Range<u32>: From<R>,
{
    let range = std::ops::Range::<u32>::from(range);
    Span::new(range.start, range.end)
}

/// A project module the driver loaded and lowered ahead of the module that
/// imports it: its display path (the non-canonical path diagnostics
/// render), its lowered HIR, and the names of its submodules (a
/// `pkg/name.py` file or `pkg/name/` directory next to a package's
/// `__init__.py`; always empty for a plain `.py` module) so `from pkg
/// import name` can tell a submodule from a top-level definition.
#[derive(Debug, Clone)]
pub struct ResolvedModule<'a> {
    pub display_path: String,
    pub hir: &'a HirModule,
    pub submodule_names: Vec<String>,
}

/// The driver's answer to one [`ProjectImportRequest`].
#[derive(Debug, Clone)]
pub enum ResolvedImport<'a> {
    /// `from m import ...`: `m` resolved to a project file that lowered
    /// successfully, so its names can be bound.
    Module(ResolvedModule<'a>),
    /// `import m`: `m` resolved to a project file or package directory.
    /// Part 1 recognizes the shape but binds no module namespace
    /// (`C0001`); the driver does not load the file.
    Found,
    /// The import cannot be satisfied. `code` and `message` are exactly
    /// what [`lower_import_stmt`] reports at the statement's span: a
    /// `T0021` for a CPython-rejected import (a relative import outside a
    /// package, a target that resolves nowhere), an `E0108` for an import
    /// cycle, or a `C0001` for a shape the compiler does not support yet
    /// (a namespace package, an absolute module that resolves nowhere).
    NotFound { code: &'static str, message: String },
    /// `import X`: `X` is neither a project module nor a `pycc_std` one,
    /// so Part 1 of #1026 binds it as an opaque CPython object
    /// ([`ImportBinding::Foreign`]). Recorded only for the bare, undotted,
    /// unaliased `import X` shape, or one such name of `import X, Y`
    /// (#1280) -- see `src/modules.rs`'s own `missing` for why every other
    /// foreign shape stays unanswered.
    Foreign,
}

/// The driver's answers for every [`ProjectImportRequest`] of one module,
/// keyed by the request's own span (an alias's span for a plain `import`,
/// the statement's span for a `from ... import`; see
/// [`ProjectImportRequest`]), plus every already-lowered module by display
/// path so a re-export (`from pkg import Point` where `pkg/__init__.py`
/// itself did `from .geometry import Point`) can be followed to the module
/// that defines the name. A request absent from the map lowers exactly as
/// a single-file compilation would (`lower_all` passes an empty map), which
/// is what keeps single-file behaviour byte-identical.
#[derive(Debug, Clone, Default)]
pub struct ResolvedImports<'a> {
    by_span: HashMap<Span, ResolvedImport<'a>>,
    modules: HashMap<String, &'a HirModule>,
    /// Issue #1188: the container method names (`append`, `pop`, `get`,
    /// `add`) that a class in the module's transitive import closure defines
    /// as a method. The driver unions its direct dependencies'
    /// [`crate::LoweredModule::container_method_names`] in here; it is not
    /// every loaded module, since a sibling this module never imports cannot
    /// hand it an instance of that class.
    container_method_names: BTreeSet<&'static str>,
}

impl<'a> ResolvedImports<'a> {
    /// Adds container method names that one of this module's dependencies
    /// can reach (issue #1188).
    pub fn inherit_container_method_names(
        &mut self,
        names: impl IntoIterator<Item = &'static str>,
    ) {
        self.container_method_names.extend(names);
    }

    pub(crate) fn container_method_names(&self) -> &BTreeSet<&'static str> {
        &self.container_method_names
    }

    /// Records the answer for the request at `span`.
    pub fn insert(&mut self, span: Span, resolved: ResolvedImport<'a>) {
        self.by_span.insert(span, resolved);
    }

    /// Registers an already-lowered module under its display path so
    /// `ImportBinding::Project` re-exports pointing at it can be followed.
    pub fn add_module(&mut self, display_path: String, hir: &'a HirModule) {
        self.modules.insert(display_path, hir);
    }

    fn get(&self, span: Span) -> Option<&ResolvedImport<'a>> {
        self.by_span.get(&span)
    }

    /// The module a `Project` binding's `module_path` names. Every such
    /// binding was created from a module already registered here (the
    /// driver loads and registers a dependency before the module importing
    /// it), so the lookup cannot miss for a binding the driver produced;
    /// the `.expect` follows the crate's coverage convention.
    fn origin(&self, module_path: &str) -> &'a HirModule {
        self.modules
            .get(module_path)
            .copied()
            .expect("a Project binding's origin module is registered before its importer lowers")
    }
}

/// What one import statement contributes to the importing module's tables:
/// the bindings it records, plus -- for a project import of a class or type
/// alias -- the class definitions (the class and its whole MRO) and alias
/// entries the importer's own lowering needs in scope to resolve
/// annotations and base classes. `module::lower_module` strips the copied
/// classes and aliases again before building its `HirModule`, so
/// `program::link` sees each definition exactly once.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LoweredImport {
    pub(crate) bindings: Vec<ImportBinding>,
    pub(crate) classes: Vec<(String, HirClassDef)>,
    pub(crate) aliases: Vec<(String, Ty)>,
}

/// Recognizes a module-level `Stmt::Import`/`Stmt::ImportFrom` and resolves
/// it against `pycc_std`'s registry (D-136/D-137) or, for a statement the
/// driver answered in `resolved`, against the loaded project module (#898).
/// Returns `Ok(None)` for any other statement kind, leaving it to the
/// caller's own dispatch -- mirroring `lower_type_alias_stmt`'s shape
/// exactly.
///
/// D-137 is fail-closed: every recognized-but-out-of-scope shape (an `as`
/// alias on the `from` form, a relative import the driver did not resolve,
/// an unresolvable module) is
/// `C0001`, the same
/// generic "statement kind not supported yet" diagnostic the crate already
/// uses for every other unimplemented statement kind -- matching the plan's
/// explicit instruction to reuse `C0001` rather than add a new code for "we
/// recognize this is an import but don't support this particular shape." A
/// recognized module with one unresolvable symbol inside an otherwise-valid
/// `from math import ...` list is instead `C0002` (D-136's own decision
/// text), distinguishing "we don't support this import shape at all" from
/// "we support `math`, just not `math.<this-symbol>`" -- and it fails the
/// whole statement, not a partial bind of the names that did resolve.
///
/// `import <stdlib module> as <alias>` is the one aliasing shape that
/// lowers (Part 1 of #883, #962): it binds the alias as the module's
/// `local_name`. `from ... import ... as ...` stays `C0001` (Part 2, #963).
///
/// A project import follows the same fail-closed split (D-222): a name the
/// origin module does not define is `T0021` (CPython's own `ImportError`
/// class of failure), while a name that *is* a submodule, a bare `import m`
/// of a project file, `import *`, and `as` aliasing are `C0001` capability
/// gaps. The lookup order for `from m import n` is: submodule probe, class,
/// top-level function, type alias, a name `m` itself imported (a
/// re-export, followed to the defining module), then any other top-level
/// bound name.
///
/// A `from __future__ import ...` (D-229) takes neither arm: it is routed
/// to [`lower_future_import`] before the stdlib registry is consulted, and
/// `position` (from [`future_prologue_len`]) is what decides whether it is
/// where CPython allows it at all.
pub(crate) fn lower_import_stmt(
    stmt: &Stmt,
    resolved: &ResolvedImports<'_>,
    position: FuturePosition,
    item_index: usize,
) -> Result<Option<LoweredImport>, Diagnostic> {
    match stmt {
        Stmt::Import(import) => {
            // #1280: `import a, b` is `import a` followed by `import b`, so
            // each alias lowers on its own, in source order. The first
            // alias that fails fails the whole statement with its one
            // diagnostic, and no binding of that statement survives --
            // D-219/D-222's one-diagnostic-per-failing-statement contract,
            // which `poisonable_names` mirrors.
            let statement = statement_span(import.range);
            let bindings = import
                .names
                .iter()
                .map(|alias| lower_import_alias(statement, alias, resolved, item_index))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(LoweredImport {
                bindings,
                ..LoweredImport::default()
            }))
        }
        Stmt::ImportFrom(import) => {
            match resolved.get(statement_span(import.range)) {
                Some(ResolvedImport::Module(module)) => {
                    return lower_project_from_import(import, module, resolved).map(Some);
                }
                Some(ResolvedImport::NotFound { code, message }) => {
                    return Err(Diagnostic::error(
                        code,
                        message.clone(),
                        statement_span(import.range),
                    ));
                }
                // `Found` and `Foreign` are only ever the answer to a bare
                // `import m`; an unanswered `from` import lowers as a
                // single-file compilation would.
                Some(ResolvedImport::Found | ResolvedImport::Foreign) | None => {}
            }
            // No answer is ever recorded for a future import
            // (`project_import_request` skips it), so this always runs
            // before the registry fallback below can report `__future__`
            // as an unsupported module.
            if is_future_import(import) {
                return lower_future_import(import, position).map(Some);
            }
            if import.level != 0 {
                return Err(unsupported(
                    "a relative import (`from . import ...`) is not supported yet",
                    import.range,
                ));
            }
            // A `level == 0` `Stmt::ImportFrom` always carries a module name
            // -- the only way to reach `module: None` is a relative import
            // (`from . import x`, `from .. import x`, ...), which always
            // has `level >= 1` and is already rejected above. Verified
            // directly against the vendored parser: `from import x` (no
            // dots, no module name) is a parse error (`L0001`, "Expected a
            // module name"), so `lower_checked` never sees this shape at
            // all, matching this file's existing precedent of verifying an
            // "impossible" shape against the real parser rather than
            // assuming it.
            let module_name = import
                .module
                .as_ref()
                .expect("a non-relative `from ... import ...` always names a module")
                .as_str();
            let Some(module) = pycc_std::resolve_module(module_name) else {
                return Err(unsupported(
                    format!("import of module `{module_name}` is not supported yet"),
                    import.range,
                ));
            };
            check_from_import_shape(import)?;
            let mut bound = Vec::with_capacity(import.names.len());
            for alias in &import.names {
                check_alias_shape(import, alias)?;
                let symbol_name = alias.name.as_str();
                let Some(symbol) = pycc_std::resolve_symbol(module, symbol_name) else {
                    return Err(unresolved_symbol(
                        format!(
                            "module `{module_name}` has no importable symbol named `{symbol_name}`"
                        ),
                        import.range,
                    ));
                };
                bound.push(ImportBinding::Symbol {
                    local_name: symbol_name.to_string(),
                    module,
                    symbol,
                });
            }
            Ok(Some(LoweredImport {
                bindings: bound,
                ..LoweredImport::default()
            }))
        }
        _ => Ok(None),
    }
}

/// One alias of a plain `import` statement (#1280): the binding it
/// contributes, or the diagnostic that fails the whole statement.
///
/// The driver's answer is looked up under the alias's own span (the key
/// [`project_import_request`] records); every diagnostic, and a foreign
/// binding's `span`, stay at the whole `statement`, so a single-alias
/// statement reports byte-for-byte what it did before #1280. Every alias of
/// one statement shares its `item_index`: `pycc_mir`'s
/// `splice_foreign_imports` inserts equal positions in binding order, so
/// the foreign imports of `import a, b` run `a` first, as CPython does.
fn lower_import_alias(
    statement: Span,
    alias: &pycc_ast::Alias,
    resolved: &ResolvedImports<'_>,
    item_index: usize,
) -> Result<ImportBinding, Diagnostic> {
    let module_name = alias.name.as_str();
    let answer = resolved.get(statement_span(alias.range));
    // `Found` is the driver's answer to a bare `import m` of a project
    // file. It is unreachable for a name `pycc_std` resolves
    // (`project_import_request` never asks the driver about one) and
    // unreachable for an aliased `import m as n` (it never asks about those
    // either -- project-module aliasing is Part 3 of #883, #964), so an
    // aliased project import falls through to the "not supported yet" arm
    // below.
    if matches!(answer, Some(ResolvedImport::Found)) {
        return Err(unsupported(
            format!("module namespace bindings (`import {module_name}`) are not supported yet"),
            statement.start..statement.end,
        ));
    }
    // Part 1 of #1026: a foreign root binds an opaque CPython object rather
    // than failing. `item_index` is the number of `HirItem`s the statements
    // before this one produced, which is where `pycc_mir` splices the
    // import back into the module body so the generated
    // `pycc_ext_obj_import` call runs in source order rather than hoisted
    // (see `MirItem::ForeignImport`).
    if matches!(answer, Some(ResolvedImport::Foreign)) {
        return Ok(ImportBinding::Foreign {
            local_name: module_name.to_string(),
            module_path: module_name.to_string(),
            item_index,
            span: statement,
        });
    }
    let Some(module) = pycc_std::resolve_module(module_name) else {
        return Err(unsupported(
            format!("import of module `{module_name}` is not supported yet"),
            statement.start..statement.end,
        ));
    };
    // Part 1 of #883 (#962): `import math as m` binds the alias the user
    // wrote; every later `m.<attr>` receiver resolves through this binding
    // (see `expr::std_receiver`), so the alias is visible to items lowered
    // after this statement, in source order, exactly like a class or a type
    // alias.
    let local_name = alias.asname.as_ref().map_or(module_name, |n| n.as_str());
    Ok(ImportBinding::Module {
        local_name: local_name.to_string(),
        module,
    })
}

/// `C0001` for `from ... import *` -- shared by the stdlib and project
/// arms so both reject the wildcard identically.
fn check_from_import_shape(import: &StmtImportFrom) -> Result<(), Diagnostic> {
    if import.names.is_empty() || import.names.iter().any(|alias| alias.name.as_str() == "*") {
        return Err(unsupported(
            "`from ... import *` (wildcard import) is not supported yet",
            import.range,
        ));
    }
    Ok(())
}

/// `C0001` for `from ... import x as y` -- shared like
/// `check_from_import_shape`.
fn check_alias_shape(import: &StmtImportFrom, alias: &pycc_ast::Alias) -> Result<(), Diagnostic> {
    if alias.asname.is_some() {
        return Err(unsupported(
            "`from ... import x as y` aliasing is not supported yet",
            import.range,
        ));
    }
    Ok(())
}

/// The project arm of [`lower_import_stmt`]: binds every name of a
/// `from m import a, b` against the loaded module `m`.
fn lower_project_from_import(
    import: &StmtImportFrom,
    module: &ResolvedModule<'_>,
    resolved: &ResolvedImports<'_>,
) -> Result<LoweredImport, Diagnostic> {
    check_from_import_shape(import)?;
    let mut lowered = LoweredImport::default();
    for alias in &import.names {
        check_alias_shape(import, alias)?;
        let name = alias.name.as_str();
        if module
            .submodule_names
            .iter()
            .any(|submodule| submodule == name)
        {
            return Err(unsupported(
                "module namespace bindings (`from pkg import submodule`) are not supported yet",
                import.range,
            ));
        }
        if !bind_project_name(
            name,
            module,
            resolved,
            statement_span(import.range),
            &mut lowered,
        )? {
            let module_name = match &import.module {
                Some(module_name) => format!("module `{module_name}` (`{}`)", module.display_path),
                None => format!("package `{}`", module.display_path),
            };
            return Err(Diagnostic::error(
                "T0021",
                format!("{module_name} has no top-level name `{name}`"),
                statement_span(import.range),
            ));
        }
    }
    Ok(lowered)
}

/// Looks `name` up in `module`'s top level in the documented order and,
/// when found, records the binding (and any class/alias copies it needs)
/// into `lowered`. Returns `Ok(false)` when the module has no such name,
/// and `Err` when the name resolves to a shape this part cannot re-export
/// (see the re-export branch). `span` is the importing statement's source
/// span, which is where such a diagnostic is reported.
fn bind_project_name(
    name: &str,
    module: &ResolvedModule<'_>,
    resolved: &ResolvedImports<'_>,
    span: Span,
    lowered: &mut LoweredImport,
) -> Result<bool, Diagnostic> {
    let origin = module.hir;
    let is_synthetic = |class_name: &str| {
        origin.seeded_builtin_exception_classes && is_builtin_exception_class(class_name)
    };
    let project = |kind| ImportBinding::Project {
        local_name: name.to_string(),
        module_path: module.display_path.clone(),
        kind,
    };
    if origin
        .class_defs
        .iter()
        .any(|(class_name, _)| class_name == name && !is_synthetic(class_name))
    {
        copy_class_with_ancestors(origin, name, &mut lowered.classes);
        lowered.bindings.push(project(ProjectBindingKind::Class));
        return Ok(true);
    }
    if origin
        .items
        .iter()
        .any(|item| matches!(item, HirItem::Function { name: function_name, .. } if function_name == name))
    {
        lowered.bindings.push(project(ProjectBindingKind::Function));
        return Ok(true);
    }
    if let Some(alias) = origin
        .type_aliases
        .iter()
        .find(|(alias_name, _)| alias_name == name)
    {
        lowered.aliases.push(alias.clone());
        lowered
            .bindings
            .push(project(ProjectBindingKind::TypeAlias));
        return Ok(true);
    }
    if let Some(binding) = origin
        .imports
        .iter()
        .find(|binding| import_local_name(binding) == name)
    {
        // A re-export: `pkg/__init__.py` did `from .geometry import Point`
        // and the importer asks `pkg` for `Point`. The recorded binding
        // already names the defining module, so copy the class/alias from
        // there and keep pointing at it (one hop always suffices). A
        // re-exported stdlib binding is cloned as-is.
        if let ImportBinding::Project {
            module_path, kind, ..
        } = binding
        {
            let defining = resolved.origin(module_path);
            match kind {
                ProjectBindingKind::Class => {
                    copy_class_with_ancestors(defining, name, &mut lowered.classes);
                }
                ProjectBindingKind::TypeAlias => {
                    let alias = defining
                        .type_aliases
                        .iter()
                        .find(|(alias_name, _)| alias_name == name)
                        .expect("a TypeAlias binding names an alias its origin module defines");
                    lowered.aliases.push(alias.clone());
                }
                ProjectBindingKind::Function | ProjectBindingKind::Variable => {}
            }
        }
        if let ImportBinding::Foreign { module_path, .. } = binding {
            // Part 1 of #1026 binds a foreign import at its own source
            // position in its own module: `item_index` counts the items
            // *that* module's preceding statements produced, and
            // `program::link` rebases it onto the linked program as though
            // it belonged to the module that recorded it. Cloning the
            // binding into the importer would hand the importer's offset
            // to a dependency-local index (an out-of-range splice in
            // `pycc_mir`) and would run a second CPython import for one
            // source statement. The importer produces no item of its own
            // here, so there is no position in its item list that could
            // carry the binding honestly; representing this shape is a
            // later part's work.
            return Err(unsupported(
                format!(
                    "`{}` binds `{name}` to the CPython module object `{module_path}`; \
                     re-exporting a foreign import across project modules is not \
                     supported yet",
                    module.display_path
                ),
                span.start..span.end,
            ));
        }
        lowered.bindings.push(binding.clone());
        return Ok(true);
    }
    if top_level_bound_names(&origin.items).contains(name) {
        // #1244: CPython answers `from m import x` with `ImportError` when
        // `m`'s top level has deleted `x` by the time the import runs.
        // Refused whenever `m`'s top level deletes `x` anywhere, even if it
        // binds `x` again afterwards -- conservative, and sound.
        if crate::stmt::del::top_level_deleted_names(&origin.items).contains(name) {
            return Err(unsupported(
                format!(
                    "`{}` deletes its top-level `{name}` with `del`, so importing `{name}` \
                     from it is not supported",
                    module.display_path
                ),
                span.start..span.end,
            ));
        }
        lowered.bindings.push(project(ProjectBindingKind::Variable));
        return Ok(true);
    }
    Ok(false)
}

/// Copies `name`'s class definition and every class in its MRO (which
/// starts with the class itself) from `origin` into `classes`, skipping
/// any already copied. `class::lower_class` looks every MRO entry of a base
/// up in the importer's class table (`.expect("every class in the MRO must
/// be in defined_classes")`), so a class cannot be imported without its
/// ancestors; the seeded builtin exception ancestors come along too and
/// `module::lower_module` reconciles them with the importer's own seeding.
fn copy_class_with_ancestors(
    origin: &HirModule,
    name: &str,
    classes: &mut Vec<(String, HirClassDef)>,
) {
    let (_, def) = origin
        .class_defs
        .iter()
        .find(|(class_name, _)| class_name == name)
        .expect("a class binding names a class its origin module defines");
    for ancestor in &def.mro {
        if classes.iter().any(|(copied, _)| copied == ancestor) {
            continue;
        }
        let entry = origin
            .class_defs
            .iter()
            .find(|(class_name, _)| class_name == ancestor)
            .expect("every class in an MRO is in its module's class table");
        classes.push(entry.clone());
    }
}

/// Recognizes a PEP 695 `type X = <expr>` statement and evaluates its RHS as
/// a type expression, reusing `annotation_to_ty` (D-135) -- the same
/// resolver used for parameter/return/variable annotations, since a type
/// alias's RHS is syntactically just another type expression. Returns
/// `Ok(None)` for any other statement kind, leaving it to the caller's own
/// dispatch.
///
/// A generic alias (`type X[T] = ...`) is rejected with `T0042`, not the
/// generic `unsupported`/`C0001` catch-all: D-134/D-135 explicitly scope a
/// generic alias out of this PR, but -- unlike, say, `async def`, which is
/// simply unrecognized syntax -- this shape *is* recognized and type-checked
/// far enough to name precisely why it is rejected, the same reasoning
/// `check_generic_function`'s own `T0042` diagnostics already use for a
/// generic function's out-of-scope shapes.
pub(crate) fn lower_type_alias_stmt(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Option<(String, Ty)>, Diagnostic> {
    let Stmt::TypeAlias(type_alias) = stmt else {
        return Ok(None);
    };
    // `type_alias.type_params` being `Some(_)` at all is enough to reject:
    // `ruff_python_parser`'s own `parse_type_params` reports a parse error
    // (`EmptyTypeParams`, surfaced by this crate's own `pycc_parser::parse`
    // as `L0001` before this function ever runs) for an empty `[]`, so a
    // `Some(type_params)` reaching this point always has at least one entry
    // -- there is no valid parsed input where an extra `.type_params.is_empty()`
    // check here would ever be reached with a `false` result to skip on
    // (confirmed against the pinned `ruff_python_parser = "0.0.6"` registry
    // source, the same way this function's own name-target extraction below
    // documents its own unreachable shape).
    if type_alias.type_params.is_some() {
        let range = std::ops::Range::<u32>::from(type_alias.range);
        return Err(Diagnostic::error(
            "T0042",
            "a generic type alias (`type X[T] = ...`) is not supported yet".to_string(),
            Span::new(range.start, range.end),
        ));
    }
    // Unlike the legacy `AnnAssign` form's target (which can be an
    // `Attribute`/`Subscript`, see `lower_legacy_type_alias_ann_assign`
    // below), `ruff_python_parser`'s own `parse_type_alias_statement`
    // unconditionally builds this field as `Expr::Name(self.parse_name(...))`
    // -- there is no valid source text that parses a `type` statement with a
    // non-name target, so there is no `unsupported`/unreachable fallback
    // branch to write or cover here (confirmed against the pinned
    // `ruff_python_parser = "0.0.6"` registry source). `.expect(...)`, not a
    // hand-rolled panic arm, per this crate's own documented coverage
    // convention (`pycc_ast::re_exported_grammar_types_resolve_and_have_the_expected_shape`'s
    // comment): the panic path lives in libcore, invisible to instrumented
    // regions, the same way `.unwrap()`'s does.
    let name = type_alias
        .name
        .as_name_expr()
        .expect("ruff always parses a `type` statement's name as Expr::Name");
    let ty = annotation_to_ty(&type_alias.value, None, None, aliases, class_defs)
        .map_err(|error| crate::with_bare_container_advice(error, &type_alias.value))?;
    Ok(Some((name.id.to_string(), ty)))
}

/// Recognizes the legacy `X: TypeAlias = <expr>` annotated-assignment form
/// of a type alias (PEP 613). Real Python requires `from typing import
/// TypeAlias` before this annotation is meaningful, but requiring that
/// import here is not merely inconsistent with existing precedent -- it is
/// currently infeasible: `pycc_hir` has no `Stmt::Import`/`Stmt::ImportFrom`
/// handling anywhere in this crate, so `from typing import TypeAlias` would
/// itself be unconditionally rejected with the generic `C0001` ("statement
/// kind not supported yet") diagnostic if pycc tried to require it first.
/// There is no accepted-bare-typing-name precedent to lean on either --
/// `Any` is the only other typing-shaped bare name `annotation_to_ty`
/// currently recognizes, and it is rejected with `T0002`, not accepted. So
/// this function accepts the bare annotation name `TypeAlias`
/// unconditionally, not by analogy to an existing precedent, but because
/// real import verification cannot be expressed with this crate's current
/// statement coverage (plan-deviation note, since the design doc leaves
/// this specific question open; import support is PR-14's).
///
/// Returns `Ok(None)` for any statement that is not this exact shape --
/// including an ordinary `X: TypeAlias` with no value, which is invalid as a
/// type alias and instead falls through to the ordinary `AnnAssign` lowering
/// path, where `annotation_to_ty` rejects the bare name `TypeAlias` with the
/// same `C0001` catch-all as any other unrecognized annotation name.
pub(crate) fn lower_legacy_type_alias_ann_assign(
    stmt: &Stmt,
    aliases: &[(String, Ty)],
    class_defs: &[ClassAnnotationInfo],
) -> Result<Option<(String, Ty)>, Diagnostic> {
    let Stmt::AnnAssign(ann) = stmt else {
        return Ok(None);
    };
    let Expr::Name(annotation_name) = ann.annotation.as_ref() else {
        return Ok(None);
    };
    if annotation_name.id.as_str() != "TypeAlias" {
        return Ok(None);
    }
    let Some(value) = ann.value.as_deref() else {
        return Ok(None);
    };
    let Expr::Name(target) = ann.target.as_ref() else {
        return Ok(None);
    };
    let ty = annotation_to_ty(value, None, None, aliases, class_defs)
        .map_err(|error| crate::with_bare_container_advice(error, value))?;
    Ok(Some((target.id.to_string(), ty)))
}

/// The bound local name of an import, regardless of which `ImportBinding`
/// variant it is -- used by `module::lower_top_level_item`'s class-name-collision check
/// (D-068 review finding on #385) so it does not need to duplicate the
/// match on both variants at its own call site.
pub(crate) fn import_local_name(binding: &ImportBinding) -> &str {
    match binding {
        ImportBinding::Module { local_name, .. }
        | ImportBinding::Symbol { local_name, .. }
        | ImportBinding::Project { local_name, .. }
        | ImportBinding::Foreign { local_name, .. } => local_name,
    }
}

/// Refuses a module in which any other top-level binding spells the local
/// name of a foreign import (Part 1 of #1026, PR 1c of #1080).
///
/// Part 1's containment invariant is that the single producer of a
/// `Ty::Object` value is a read of a foreign binding, so refusing that read
/// refuses every derived operation. A module that binds the same name twice,
/// once foreign and once not, breaks that premise: the name's meaning then
/// depends on the position of every read, and each pass that walks the
/// module -- the check pass, the constraint solver, MIR lowering, export
/// discovery -- would have to reproduce the same positional rule
/// independently. Three review rounds on #1080 found three passes that did
/// not, most seriously `collect_exports`, which kept a `PyMethodDef` for a
/// `def` the import supersedes, so the host called a stale function where
/// CPython would hand back a module object.
///
/// Refusing the shape instead is one rule at one site, fail-closed, and
/// consistent with the cross-module case, which Part 1 already refuses.
/// Supporting either order is later work; see #1026.
///
/// The colliding binding comes from either of two tables, because a module's
/// top level binds names in both. `definition_spans` is every definition the
/// module makes, with its span, and never carries an import's own binding --
/// the import arm of `module::lower_module_item` records nothing there. So
/// `imports` is consulted as well: a second `import` statement binding the
/// same local name is invisible to `definition_spans` but supersedes the
/// foreign binding exactly as a `def` does (review round 4 on #1080, which
/// found `import json` followed by `import math as json` reaching the
/// solver and reporting a misleading receiver diagnostic against the wrong
/// statement when the name was used, and passing silently when it was not).
///
/// Two foreign imports of the same local name are refused on the same rule
/// rather than exempted as benign. The shape is degenerate either way, and
/// admitting it would mean this predicate has to reason about which of two
/// `Ty::Object` producers a read resolves to -- the positional question the
/// refusal exists to avoid.
///
/// At most one diagnostic per name, so a duplicated import reports once. A
/// definition's span is preferred over the import's when both exist: it is
/// the statement that is unusual, the import being ordinary on its own.
pub(crate) fn reject_shadowed_foreign_imports(
    imports: &[ImportBinding],
    definition_spans: &[(String, Span)],
) -> Vec<Diagnostic> {
    let mut reported: Vec<&str> = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, binding) in imports.iter().enumerate() {
        let ImportBinding::Foreign {
            local_name, span, ..
        } = binding
        else {
            continue;
        };
        if reported.contains(&local_name.as_str()) {
            continue;
        }
        let definition = definition_spans
            .iter()
            .find(|(name, _)| name == local_name)
            .map(|(_, span)| *span);
        let shadowed_by_import = imports
            .iter()
            .enumerate()
            .any(|(other, candidate)| other != index && import_local_name(candidate) == local_name);
        let Some(span) = definition.or(shadowed_by_import.then_some(*span)) else {
            continue;
        };
        reported.push(local_name.as_str());
        // `C0001` by hand rather than through `unsupported`: the span is
        // already a `Span` recorded during lowering, not an AST
        // `TextRange`, and that helper takes only the range shape.
        diagnostics.push(Diagnostic::error(
            "C0001",
            format!(
                "`{local_name}` is bound both by a foreign `import` and by another top-level \
                 statement in this module; shadowing a foreign import is not supported yet"
            ),
            span,
        ));
    }
    diagnostics
}

#[cfg(test)]
mod tests;
