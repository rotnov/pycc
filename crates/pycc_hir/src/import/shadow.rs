//! The import side table's local-name accessor and the rule that refuses a
//! second binding of a foreign import's local name (Part 1 of #1026, PR 1c
//! of #1080). Extracted from `import.rs` (#1291) with no logic change.

use crate::ImportBinding;
use pycc_diag::{Diagnostic, Span};

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
