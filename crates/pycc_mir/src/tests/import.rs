//! Import-binding carry-through (`MirModule::foreign_imports`, Part 1 of
//! #1026).
//!
//! Every `ImportBinding` variant that exists today is compile-time-only, so
//! none of them contributes a runtime import. These tests pin that: the
//! channel is present and stays empty for the imports current programs write.

use crate::*;
use pycc_hir::{HirModule, ImportBinding, ProjectBindingKind};

fn module_with_imports(imports: Vec<ImportBinding>) -> HirModule {
    HirModule {
        seeded_builtin_exception_classes: false,
        items: Vec::new(),
        type_aliases: Vec::new(),
        imports,
        class_defs: Vec::new(),
    }
}

#[test]
fn carries_an_empty_foreign_import_list_for_a_module_without_imports() {
    let mir = build(&module_with_imports(Vec::new()));
    assert!(mir.foreign_imports.is_empty());
}

#[test]
fn drops_the_stdlib_module_and_symbol_bindings() {
    let hir = module_with_imports(vec![
        ImportBinding::Module {
            local_name: "math".to_string(),
            module: pycc_std::StdModule::Math,
        },
        ImportBinding::Symbol {
            local_name: "sqrt".to_string(),
            module: pycc_std::StdModule::Math,
            symbol: pycc_std::resolve_symbol(pycc_std::StdModule::Math, "sqrt")
                .expect("`math.sqrt` is a registered stdlib symbol"),
        },
    ]);
    assert!(build(&hir).foreign_imports.is_empty());
}

#[test]
fn drops_a_project_binding() {
    let hir = module_with_imports(vec![ImportBinding::Project {
        local_name: "helper".to_string(),
        module_path: "pkg.helper".to_string(),
        kind: ProjectBindingKind::Function,
    }]);
    assert!(build(&hir).foreign_imports.is_empty());
}
