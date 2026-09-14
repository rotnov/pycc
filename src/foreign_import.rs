//! The native-mode gate on a foreign CPython import (Part 1 of #1026).
//!
//! `import numpy` binds an opaque CPython module object, which only exists
//! while a CPython interpreter is running the artifact. `pycc build --ext`
//! produces exactly that: an extension module whose `Py_mod_exec` slot runs
//! inside the interpreter that loaded it. A plain `pycc build` produces a
//! standalone native executable with no interpreter at all, so there is
//! nothing to import *from*, and the program is refused here rather than
//! compiled into a call that could only fail at run time.
//!
//! This is the reason `crates/pycc_codegen/src/foreign_import.rs` may
//! silently ignore a `MirItem::ForeignImport` under `!options.ext` instead
//! of asserting: this gate has already refused every program that could
//! reach it.

use pycc_diag::Diagnostic;
use pycc_hir::{HirModule, ImportBinding};

/// One `I0403` per foreign import in `hir`, in source order, or `Ok(())`
/// when the program has none.
///
/// Shaped like `src/ext_build.rs`'s `collect_exports`: a driver-side
/// refusal against typed HIR that returns every gap at once rather than
/// only the first, so one build reports the whole list.
///
/// Each gap is paired with the import's own position in `hir.imports`. A
/// multi-file program's foreign import usually lives in a dependency, not
/// in the entry file, and that position is what says which --
/// `pycc_hir::link` concatenates each module's import table in the
/// program's file order, so `src/frontend.rs`'s `ProgramSources` turns the
/// position back into the owning file and the `I0403` renders against that
/// file's path and source instead of being attributed wholesale to the
/// entry (PR 1c of #1080 review finding 2).
///
/// The position, not `item_index`: an import's recorded item index is the
/// item count at the moment it lowered, so a *trailing* import in one file
/// and a *leading* import in the next record the same linked index and no
/// arithmetic on the per-file item bounds can tell them apart. The import
/// table has no such boundary ambiguity.
pub(crate) fn refuse_in_native_mode(hir: &HirModule) -> Result<(), Vec<(usize, Diagnostic)>> {
    let gaps: Vec<(usize, Diagnostic)> = hir
        .imports
        .iter()
        .enumerate()
        .filter_map(|(position, binding)| match binding {
            ImportBinding::Foreign {
                module_path, span, ..
            } => Some((
                position,
                Diagnostic::error(
                    "I0403",
                    format!(
                        "`import {module_path}` imports a CPython module, which requires \
                         `pycc build --ext`: a native executable embeds no CPython \
                         interpreter to import it into"
                    ),
                    // The import statement's own range, carried on the
                    // binding. Before it was, every `I0403` was built with
                    // `Span::new(0, 0)`, so a foreign import that was not
                    // the first statement reported at `<file>:1:1` and
                    // highlighted an unrelated line (PR 1c of #1080 review
                    // round 4).
                    *span,
                ),
            )),
            ImportBinding::Module { .. }
            | ImportBinding::Symbol { .. }
            | ImportBinding::Project { .. } => None,
        })
        .collect();
    if gaps.is_empty() {
        return Ok(());
    }
    Err(gaps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_diag::Span;
    use pycc_hir::ProjectBindingKind;

    fn hir(imports: Vec<ImportBinding>) -> HirModule {
        HirModule {
            seeded_builtin_exception_classes: false,
            items: Vec::new(),
            type_aliases: Vec::new(),
            imports,
            class_defs: Vec::new(),
        }
    }

    fn foreign(name: &str) -> ImportBinding {
        ImportBinding::Foreign {
            local_name: name.to_string(),
            module_path: name.to_string(),
            item_index: 0,
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn a_program_without_a_foreign_import_is_admitted() {
        // A project import is the admitted binding spelled here because
        // the driver crate deliberately does not depend on `pycc_std`,
        // which `ImportBinding::Module`/`Symbol` need to construct; the
        // stdlib arms are covered by `crates/pycc_types/src/foreign/tests.rs`,
        // whose filter is the same shape.
        let admitted = hir(vec![ImportBinding::Project {
            local_name: "helper".to_string(),
            module_path: "pkg.helper".to_string(),
            kind: ProjectBindingKind::Function,
        }]);
        assert!(refuse_in_native_mode(&admitted).is_ok());
    }

    #[test]
    fn every_foreign_import_is_reported_not_only_the_first() {
        let gaps = refuse_in_native_mode(&hir(vec![
            foreign("numpy"),
            ImportBinding::Project {
                local_name: "helper".to_string(),
                module_path: "pkg.helper".to_string(),
                kind: ProjectBindingKind::Function,
            },
            foreign("scipy"),
        ]))
        .expect_err("a foreign import is refused in native mode");
        let messages: Vec<&str> = gaps.iter().map(|(_, gap)| gap.message.as_str()).collect();
        assert_eq!(gaps.len(), 2, "{messages:?}");
        assert!(
            gaps.iter().all(|(_, gap)| gap.code == "I0403"),
            "{messages:?}"
        );
        assert!(messages[0].contains("`import numpy`"), "{messages:?}");
        assert!(messages[1].contains("`import scipy`"), "{messages:?}");
        // Each gap carries its position in the whole import table -- not its
        // position among the foreign ones -- because that is the index the
        // driver joins against the program's per-file import bounds to name
        // the file that owns the import.
        let positions: Vec<usize> = gaps.iter().map(|(index, _)| *index).collect();
        assert_eq!(positions, vec![0, 2], "{messages:?}");
    }
}
