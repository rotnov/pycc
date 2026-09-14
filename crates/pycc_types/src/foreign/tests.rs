//! Unit coverage for the foreign-binding helpers themselves; the
//! end-to-end refusals live in `tests/issue_1080_foreign_object.rs`.

use super::*;
use pycc_hir::ProjectBindingKind;

#[test]
fn lists_only_the_foreign_bindings() {
    let imports = vec![
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
        ImportBinding::Project {
            local_name: "helper".to_string(),
            module_path: "pkg.helper".to_string(),
            kind: ProjectBindingKind::Function,
        },
        ImportBinding::Foreign {
            local_name: "numpy".to_string(),
            module_path: "numpy".to_string(),
            item_index: 0,
        },
    ];
    assert_eq!(foreign_object_names(&imports), vec!["numpy"]);
}

#[test]
fn the_guard_admits_every_other_type_and_refuses_the_object() {
    assert!(reject_object_read("x", &Ty::Int).is_ok());
    let diagnostic = reject_object_read("numpy", &Ty::Object).expect_err("object is refused");
    assert_eq!(diagnostic.code, "I0404");
    assert!(diagnostic.message.contains("numpy"), "{diagnostic:?}");
}

/// Lowers a source snippet exactly as `crate::module`'s own tests do. No
/// resolver participates, so the snippet must not contain an `import`:
/// a foreign binding is classified by the driver (`src/modules.rs`) and is
/// attached to the lowered module by hand below.
fn lower(source: &str) -> pycc_hir::HirModule {
    let module = pycc_parser::parse(source).expect("test source must parse");
    pycc_hir::lower_checked(&module).expect("test source must lower")
}

fn with_foreign_import(mut hir: pycc_hir::HirModule) -> pycc_hir::HirModule {
    hir.imports.push(ImportBinding::Foreign {
        local_name: "numpy".to_string(),
        module_path: "numpy".to_string(),
        item_index: 0,
    });
    hir
}

/// Part 1 of #1026 gave `HirModule::imports` a reader downstream of the
/// type checker -- `pycc_mir::build` splices a `MirItem::ForeignImport` per
/// foreign binding, and `src/main.rs`'s `I0403` gate reads the same list --
/// where before it was fully discharged during HIR lowering.
/// `monomorphize` used to drop the field on the strength of having no such
/// reader, which silently produced an artifact that imported nothing.
///
/// Both of its exits are asserted: the early return taken by a module with
/// no generic function, and the rewriting path taken by one with.
#[test]
fn the_resolved_module_still_carries_its_foreign_imports() {
    for source in [
        "def f(x: int) -> int:\n    return x\n",
        "def f[T](x: T) -> T:\n    return x\n\n\ndef g() -> int:\n    return f(1)\n",
    ] {
        let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import(lower(source)))
            .expect("the fixture type-checks");
        assert_eq!(
            foreign_object_names(&resolved.imports),
            vec!["numpy"],
            "{source}"
        );
    }
}
