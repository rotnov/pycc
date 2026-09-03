//! Unit tests for the project-import half of `import.rs` (#898, Part 1 of
//! #881, D-222): what a resolved dependency binds into the importing
//! module, and every shape the lookup chain rejects.
//!
//! Kept here rather than in `crates/pycc_hir/src/tests.rs`, which #663
//! already tracks as oversized.

use super::*;
use crate::pycc_parser_test_helper::parse;
use crate::{LoweredModule, lower_module};

const DEP: &str = "dep.py";

/// Lowers `source` as a standalone module (no project imports), the way
/// the driver lowers a dependency before its importer.
fn lower_dependency(source: &str) -> HirModule {
    lower_module(&parse(source), &ResolvedImports::default())
        .expect("a dependency fixture must lower")
        .hir
}

/// One already-loaded dependency, registered under [`DEP`].
struct Fixture {
    origin: HirModule,
}

impl Fixture {
    fn new(source: &str) -> Self {
        Self {
            origin: lower_dependency(source),
        }
    }

    /// Answers every project import request in `source` with this
    /// dependency, offering `submodules` as its submodule names.
    fn lower(&self, source: &str, submodules: &[&str]) -> Result<LoweredModule, Vec<Diagnostic>> {
        let parsed = parse(source);
        let mut resolved = ResolvedImports::default();
        resolved.add_module(DEP.to_string(), &self.origin);
        for request in project_import_requests(&parsed) {
            resolved.insert(
                request.span,
                ResolvedImport::Module(ResolvedModule {
                    display_path: DEP.to_string(),
                    hir: &self.origin,
                    submodule_names: submodules.iter().map(|name| (*name).to_string()).collect(),
                }),
            );
        }
        lower_module(&parsed, &resolved)
    }

    fn lower_ok(&self, source: &str) -> LoweredModule {
        self.lower(source, &[])
            .unwrap_or_else(|diagnostics| panic!("must lower: {:?}", diagnostics[0].message))
    }

    fn first_error(&self, source: &str, submodules: &[&str]) -> Diagnostic {
        let mut diagnostics = self
            .lower(source, submodules)
            .expect_err("fixture must fail to lower");
        diagnostics.remove(0)
    }
}

/// Lowers `source` with every project import answered by `answer`.
fn lower_with_answer(source: &str, answer: &ResolvedImport<'_>) -> Vec<Diagnostic> {
    let parsed = parse(source);
    let mut resolved = ResolvedImports::default();
    for request in project_import_requests(&parsed) {
        resolved.insert(request.span, answer.clone());
    }
    lower_module(&parsed, &resolved).expect_err("fixture must fail to lower")
}

const DEFINITIONS: &str = "class Point:\n    def __init__(self, x: int) -> None:\n        \
                           self.x = x\n\n\ntype Alias = int\n\n\ndef helper(n: int) -> int:\n    \
                           return n\n\n\nvalue = 7\n";

fn binding_kinds(module: &LoweredModule) -> Vec<(&str, ProjectBindingKind)> {
    module
        .hir
        .imports
        .iter()
        .filter_map(|binding| match binding {
            ImportBinding::Project {
                local_name, kind, ..
            } => Some((local_name.as_str(), *kind)),
            _ => None,
        })
        .collect()
}

#[test]
fn every_project_binding_kind_binds_from_a_resolved_dependency() {
    let fixture = Fixture::new(DEFINITIONS);
    let lowered = fixture.lower_ok(
        "from dep import Point, Alias, helper, value\n\np = Point(1)\nn: Alias = helper(value)\n",
    );
    assert_eq!(
        binding_kinds(&lowered),
        vec![
            ("Point", ProjectBindingKind::Class),
            ("Alias", ProjectBindingKind::TypeAlias),
            ("helper", ProjectBindingKind::Function),
            ("value", ProjectBindingKind::Variable),
        ]
    );
    // The copies the importer needed in scope are stripped again, so
    // `program::link` sees each definition exactly once.
    assert!(
        !lowered
            .hir
            .class_defs
            .iter()
            .any(|(name, _)| name == "Point")
    );
    assert!(
        !lowered
            .hir
            .type_aliases
            .iter()
            .any(|(name, _)| name == "Alias")
    );
    assert!(
        lowered
            .definition_spans
            .iter()
            .all(|(name, _)| name != "Point")
    );
}

#[test]
fn a_name_the_dependency_does_not_define_is_a_t0021_import_error() {
    let fixture = Fixture::new(DEFINITIONS);
    let diagnostic = fixture.first_error("from dep import Pont\n", &[]);
    assert_eq!(diagnostic.code, "T0021");
    assert_eq!(
        diagnostic.message,
        "module `dep` (`dep.py`) has no top-level name `Pont`"
    );
}

#[test]
fn a_relative_import_with_no_module_names_the_package_in_its_missing_name_error() {
    let fixture = Fixture::new(DEFINITIONS);
    let diagnostic = fixture.first_error("from . import Pont\n", &[]);
    assert_eq!(diagnostic.code, "T0021");
    assert_eq!(
        diagnostic.message,
        "package `dep.py` has no top-level name `Pont`"
    );
}

#[test]
fn importing_a_submodule_name_is_a_capability_gap_not_a_missing_name() {
    let fixture = Fixture::new(DEFINITIONS);
    let diagnostic = fixture.first_error("from dep import sub\n", &["sub"]);
    assert_eq!(diagnostic.code, "C0001");
    assert_eq!(
        diagnostic.message,
        "module namespace bindings (`from pkg import submodule`) are not supported yet"
    );
}

#[test]
fn a_bare_import_of_a_resolved_project_module_names_the_module_namespace_gap() {
    let diagnostics = lower_with_answer("import geometry\n", &ResolvedImport::Found);
    assert_eq!(diagnostics[0].code, "C0001");
    assert_eq!(
        diagnostics[0].message,
        "module namespace bindings (`import geometry`) are not supported yet"
    );
}

#[test]
fn a_from_import_answered_with_found_falls_back_to_the_single_file_path() {
    // `Found` is only ever the answer to a bare `import m`; a `from`
    // import carrying it must lower exactly as it would unanswered.
    let diagnostics = lower_with_answer("from nowhere import x\n", &ResolvedImport::Found);
    assert_eq!(diagnostics[0].code, "C0001");
    assert_eq!(
        diagnostics[0].message,
        "import of module `nowhere` is not supported yet"
    );
}

#[test]
fn a_not_found_answer_is_reported_verbatim_at_the_statement_span() {
    let source = "from a.b import x\n";
    let diagnostics = lower_with_answer(
        source,
        &ResolvedImport::NotFound {
            code: "E0108",
            message: "import cycle: `a.py` -> `b.py` -> `a.py`".to_string(),
        },
    );
    assert_eq!(diagnostics[0].code, "E0108");
    assert_eq!(
        diagnostics[0].message,
        "import cycle: `a.py` -> `b.py` -> `a.py`"
    );
    assert_eq!(
        diagnostics[0].span,
        Some(Span::new(0, source.trim_end().len() as u32))
    );
}

#[test]
fn a_synthetic_builtin_exception_class_is_not_importable() {
    // The dependency raises, so HIR lowering seeded the 25 builtin
    // exception classes into its class table; they are not definitions the
    // module can re-export.
    let fixture = Fixture::new("def f() -> int:\n    raise ValueError\n");
    let diagnostic = fixture.first_error("from dep import ValueError\n", &[]);
    assert_eq!(diagnostic.code, "T0021");
    assert_eq!(
        diagnostic.message,
        "module `dep` (`dep.py`) has no top-level name `ValueError`"
    );
}

#[test]
fn an_imported_class_arrives_with_its_whole_ancestor_chain() {
    let fixture = Fixture::new(
        "class Base:\n    def __init__(self, x: int) -> None:\n        self.x = x\n\n\nclass Sub\
         (Base):\n    def tag(self) -> int:\n        return self.x\n",
    );
    // `Base` is never imported by name, but `Sub`'s MRO needs it, so the
    // copy walks the whole chain.
    let lowered = fixture.lower_ok(
        "from dep import Sub\n\n\nclass Deeper(Sub):\n    def more(self) -> int:\n        \
         return self.tag()\n",
    );
    let deeper = lowered
        .hir
        .class_defs
        .iter()
        .find(|(name, _)| name == "Deeper")
        .expect("the local subclass is defined here");
    assert_eq!(deeper.1.mro, vec!["Deeper", "Sub", "Base"]);
}

#[test]
fn an_imported_exception_subclass_can_be_subclassed_again() {
    let fixture = Fixture::new("class MyError(ValueError):\n    pass\n");
    let lowered =
        fixture.lower_ok("from dep import MyError\n\n\nclass Worse(MyError):\n    pass\n");
    let worse = lowered
        .hir
        .class_defs
        .iter()
        .find(|(name, _)| name == "Worse")
        .expect("the local subclass is defined here");
    assert_eq!(worse.1.mro[..2], ["Worse", "MyError"]);
}

#[test]
fn defining_a_class_the_module_already_imported_collides_with_the_import() {
    let fixture = Fixture::new(DEFINITIONS);
    let diagnostic = fixture.first_error(
        "from dep import Point\n\n\nclass Point:\n    def __init__(self) -> None:\n        \
         self.y = 1\n",
        &[],
    );
    assert_eq!(diagnostic.code, "C0001");
    assert!(
        diagnostic.message.contains("import of the same name"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn a_re_export_is_followed_one_hop_to_the_module_that_defines_the_name() {
    let base = lower_dependency(DEFINITIONS);
    // `pkg/__init__.py` re-exports every kind of binding, including a
    // stdlib symbol it imported itself.
    let package_source = "from base import Point, Alias, helper, value\nfrom math import sqrt\n";
    let parsed = parse(package_source);
    let mut package_resolved = ResolvedImports::default();
    package_resolved.add_module("base.py".to_string(), &base);
    for request in project_import_requests(&parsed) {
        package_resolved.insert(
            request.span,
            ResolvedImport::Module(ResolvedModule {
                display_path: "base.py".to_string(),
                hir: &base,
                submodule_names: Vec::new(),
            }),
        );
    }
    let package = lower_module(&parsed, &package_resolved)
        .expect("the package fixture must lower")
        .hir;

    let importer_source = "from pkg import Point, Alias, helper, value, sqrt\n\np = Point(1)\nn: Alias = \
         helper(value)\nr: float = sqrt(4.0)\n";
    let importer = parse(importer_source);
    let mut resolved = ResolvedImports::default();
    resolved.add_module("base.py".to_string(), &base);
    resolved.add_module("pkg/__init__.py".to_string(), &package);
    for request in project_import_requests(&importer) {
        resolved.insert(
            request.span,
            ResolvedImport::Module(ResolvedModule {
                display_path: "pkg/__init__.py".to_string(),
                hir: &package,
                submodule_names: Vec::new(),
            }),
        );
    }
    let lowered = lower_module(&importer, &resolved).expect("the re-export chain must lower");
    // The class and alias were copied from `base.py`, the module that
    // actually defines them, and every binding still points there.
    assert_eq!(
        binding_kinds(&lowered),
        vec![
            ("Point", ProjectBindingKind::Class),
            ("Alias", ProjectBindingKind::TypeAlias),
            ("helper", ProjectBindingKind::Function),
            ("value", ProjectBindingKind::Variable),
        ]
    );
    assert!(
        lowered
            .hir
            .imports
            .iter()
            .any(|binding| matches!(binding, ImportBinding::Symbol { local_name, .. } if local_name == "sqrt"))
    );
}

#[test]
fn project_import_requests_skips_everything_the_stdlib_registry_answers() {
    let module = parse(
        "import math\nfrom math import sqrt\nimport geometry\nimport a, b\nimport geometry as g\n\
         from .rel import x\nfrom pkg.sub import y\nvalue = 1\n",
    );
    let requests = project_import_requests(&module);
    let shapes: Vec<(u32, Option<&str>, usize)> = requests
        .iter()
        .map(|request| {
            (
                request.level,
                request.module.as_deref(),
                request.names.len(),
            )
        })
        .collect();
    assert_eq!(
        shapes,
        vec![
            (0, Some("geometry"), 0),
            (1, Some("rel"), 1),
            (0, Some("pkg.sub"), 1),
        ]
    );
}
