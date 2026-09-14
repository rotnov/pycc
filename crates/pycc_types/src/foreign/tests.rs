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

/// The same lowering helper, with the foreign binding recorded at an
/// arbitrary position rather than always at 0 -- which is what the
/// positional claims below are about.
fn with_foreign_import_at(mut hir: pycc_hir::HirModule, item_index: usize) -> pycc_hir::HirModule {
    hir.imports.push(ImportBinding::Foreign {
        local_name: "numpy".to_string(),
        module_path: "numpy".to_string(),
        item_index,
    });
    hir
}

/// The recorded position of the single foreign binding in `imports`.
fn foreign_position(imports: &[ImportBinding]) -> usize {
    imports
        .iter()
        .find_map(|binding| match binding {
            ImportBinding::Foreign { item_index, .. } => Some(*item_index),
            _ => None,
        })
        .expect("the fixture records exactly one foreign binding")
}

/// PR 1c of #1080 review finding 1, the panic arm. `monomorphize` drops
/// every original generic function, so an import recorded after two of
/// them used to survive as index 2 against an item list of length 0 --
/// `pycc_mir::splice_foreign_imports` then panicked in `Vec::insert`
/// ("insertion index (is 2) should be <= len (is 0)"). The position is
/// recomputed to 0, which is both in range and where the import belongs:
/// nothing that preceded it still exists.
#[test]
fn a_foreign_import_after_dropped_generics_is_repositioned_to_the_surviving_prefix() {
    let source = "def _a[T](x: T) -> T:\n    return x\n\n\ndef _b[T](x: T) -> T:\n    return x\n";
    let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import_at(lower(source), 2))
        .expect("the fixture type-checks");
    assert!(resolved.items.is_empty(), "{:?}", resolved.items);
    assert_eq!(foreign_position(&resolved.imports), 0);
}

/// The same finding's silent arm, which is the one that reached an
/// artifact: one dropped generic followed by a surviving `def` left the
/// import at index 1 against a one-item list, so `insert(1, ..)` *appended*
/// it -- emitting the import after the function instead of before it, in
/// violation of D-244 rule 3. The recomputed position is 0, so the import
/// still precedes the item that followed it in the source.
#[test]
fn a_foreign_import_before_a_surviving_item_keeps_preceding_it() {
    let source = "def _a[T](x: T) -> T:\n    return x\n\n\ndef f() -> int:\n    return 1\n";
    let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import_at(lower(source), 1))
        .expect("the fixture type-checks");
    assert_eq!(foreign_position(&resolved.imports), 0);
    let names: Vec<&str> = resolved
        .items
        .iter()
        .filter_map(|item| match item {
            pycc_hir::HirItem::Function { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["f"], "{:?}", resolved.items);
}

/// `monomorphize`'s other exit -- the early return a module with no
/// generic, no generic class and no protocol parameter takes -- returns
/// `hir.items` unchanged, so every recorded position must survive
/// untouched. Asserted rather than argued, because "this exit does not
/// rewrite the list" is exactly the kind of claim a later change breaks.
#[test]
fn the_non_generic_exit_leaves_a_recorded_position_alone() {
    let source = "x = 1\n\n\ndef f() -> int:\n    return 1\n";
    let resolved = crate::check_and_resolve_all_keyed(&with_foreign_import_at(lower(source), 1))
        .expect("the fixture type-checks");
    assert_eq!(foreign_position(&resolved.imports), 1);
    assert_eq!(resolved.items.len(), 2);
}

/// PR 1c of #1080 review finding 2. The pre-seed above cannot supersede a
/// binding the source-order pass makes *later*, so `def json()` followed by
/// `import json` left `json` marked def-rebound, the call gate skipped the
/// `I0404` refusal, and `json()` compiled into a call to the shadowed
/// function -- where CPython raises `TypeError: 'module' object is not
/// callable`. Applying the binding at its recorded position is what refuses
/// it.
#[test]
fn a_foreign_import_supersedes_an_earlier_def_of_the_same_name() {
    let source = "def json() -> int:\n    return 1\n\n\nx = json()\n";
    let hir = with_foreign_import_named(lower(source), "json", 1);
    let diagnostics = crate::check_all(&hir).expect_err("the call must be refused");
    assert!(
        diagnostics.iter().any(|d| d.code == "I0404"),
        "{diagnostics:?}"
    );
}

/// The opposite order is a program CPython *runs*: the `def` executes after
/// the import and rebinds the name to a function, so the call is an
/// ordinary call and must keep compiling. Refusing this one would be an
/// over-rejection introduced by the fix above, which is why both orders are
/// pinned.
#[test]
fn a_def_after_a_foreign_import_rebinds_the_name_and_still_compiles() {
    let source = "def json() -> int:\n    return 1\n\n\nx = json()\n";
    let hir = with_foreign_import_named(lower(source), "json", 0);
    crate::check_all(&hir).expect("the `def` below the import wins");
}

/// As [`with_foreign_import_at`], for a fixture that needs the binding to
/// collide with a name the source itself defines.
fn with_foreign_import_named(
    mut hir: pycc_hir::HirModule,
    local_name: &str,
    item_index: usize,
) -> pycc_hir::HirModule {
    hir.imports.push(ImportBinding::Foreign {
        local_name: local_name.to_string(),
        module_path: local_name.to_string(),
        item_index,
    });
    hir
}

/// `unroll_enum_loops` runs *after* `monomorphize` and is the one remaining
/// pass that changes how many items the list holds: a top-level
/// `for c in Color:` becomes one loop-variable assignment plus one body copy
/// per enum member. A position recorded after such a loop therefore has to
/// be recomputed as well, or `pycc_mir::splice_foreign_imports` emits the
/// import *inside* the unrolled sequence -- running it before top-level
/// statements that precede it in source. The non-foreign binding alongside
/// it pins that the remap carries the rest of the table through untouched.
#[test]
fn a_foreign_import_after_an_unrolled_enum_loop_is_repositioned_past_it() {
    let source =
        "class Color(Enum):\n    RED = 1\n    GREEN = 2\nfor c in Color:\n    print(c.value)\n";
    let mut hir = with_foreign_import_at(lower(source), 1);
    hir.imports.push(ImportBinding::Module {
        local_name: "math".to_string(),
        module: pycc_std::StdModule::Math,
    });
    let resolved = crate::check_and_resolve_all_keyed(&hir).expect("the fixture type-checks");
    assert_eq!(resolved.items.len(), 4, "{:?}", resolved.items);
    assert_eq!(foreign_position(&resolved.imports), 4);
    assert!(
        matches!(resolved.imports[1], ImportBinding::Module { .. }),
        "{:?}",
        resolved.imports
    );
}
