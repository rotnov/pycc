//! Whole-program linking (#898, D-222): combines any number of
//! `module::lower_module` results into the single `HirModule` the rest of
//! the pipeline (`pycc_types`, `pycc_mir`, `pycc_codegen`) consumes, then
//! runs the program-wide phases that used to close `lower_all`.
//!
//! Part 1 of #881 links modules into one flat namespace: every module's
//! items are concatenated in the driver's dependency order (each module
//! after the modules it imports, the entry file last), the alias, import,
//! and class tables are unioned, and two definitions of one top-level name
//! in different modules are rejected (`C0001`) rather than silently
//! shadowed. A per-module namespace is a later part of #881.
//!
//! `pycc_hir` stays filesystem-free: the driver's `src/modules.rs` finds,
//! reads, and orders the files and hands lowered modules in here.

use crate::module::LoweredModule;
use crate::{
    FIRST_USER_EXCEPTION_TYPE_TAG, HirModule, ImportBinding, MAX_USER_EXCEPTION_CLASSES,
    builtin_exception_class_defs, builtin_exception_init_item, is_builtin_exception_class,
    unsupported,
};
use pycc_diag::{Diagnostic, Span};
use std::collections::{HashMap, HashSet};

/// One module to link: its display path (the non-canonical path
/// diagnostics render, also the `module_path` its `ImportBinding::Project`
/// importers recorded) and its lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkInput {
    pub display_path: String,
    pub module: LoweredModule,
}

/// Links `inputs` (in dependency order, entry last) into one program.
/// `Err` carries the index of the input the diagnostic belongs to, so the
/// driver can render it against that file's source; it is never empty and
/// today always holds exactly one entry, the first problem found.
///
/// Seeding reconciliation: each module decided its own builtin-exception
/// seeding (`lower_module`), so the synthetic class set may be present in
/// several inputs. Every input's synthetic entries are stripped and one
/// set is appended at the back iff any input seeded, keeping the
/// single-module invariant (`seeded_builtin_exception_classes` identifies
/// the trailing entries exactly). That invariant also requires that no
/// linked module binds one of the 26 names at its top level: un-seeding
/// the program would leave the seeded module's `class MyError(ValueError)`
/// resolving a base the table no longer holds, and keeping the seed would
/// let the shadowing module's definition collide with the synthetic one --
/// so a seeded input plus a shadowing input is rejected (`C0001`, at the
/// shadowing definition).
///
/// Foreign-shadow check (Part 1 of #1026): a top-level definition of a
/// name that a *different* linked module binds with `ImportBinding::Foreign`
/// is rejected (`C0001`, at the shadowing definition), in either dependency
/// order. A module shadowing its own foreign import is not this check's
/// business -- `module::lower_module` reports that case itself.
///
/// Collision check: a top-level class, function, type alias, or bound
/// variable name defined by two different inputs is `C0001` at the later
/// input's definition. Names a module only *imports* are not definitions,
/// so `from a import Point` in two modules is fine, and a module's own
/// rebinding of its own name (`x = 1; x = 2`) is as legal as it was.
pub fn link(inputs: Vec<LinkInput>) -> Result<HirModule, Vec<(usize, Diagnostic)>> {
    let any_seeded = inputs
        .iter()
        .any(|input| input.module.hir.seeded_builtin_exception_classes);
    if any_seeded
        && let Some((index, name)) = inputs.iter().enumerate().find_map(|(index, input)| {
            input
                .module
                .shadowed_builtin_exception_name
                .as_deref()
                .map(|name| (index, name))
        })
    {
        let shadowing = &inputs[index];
        let seeded = inputs
            .iter()
            .find(|input| input.module.hir.seeded_builtin_exception_classes)
            .expect("any_seeded guarantees a seeded input");
        return Err(vec![(
            index,
            unsupported(
                format!(
                    "module `{}` defines `{name}`, which `{}` uses as the builtin exception; \
                     shadowing a builtin exception across modules is not supported yet",
                    shadowing.display_path, seeded.display_path
                ),
                span_range(definition_span(&shadowing.module, name)),
            ),
        )]);
    }
    // Part 1 of #1026: a foreign import binds its local name to a real
    // runtime `PyObject *`, but it is an import rather than a definition,
    // so `definition_spans` never records it and the `owners` collision
    // check below cannot see it. Without this gate a module's
    // `import json` and another module's `def json()` both survive
    // linking into one flat namespace, and the dependency's `json(...)`
    // silently resolves to the entry module's function instead of raising
    // `TypeError` the way CPython does. Both directions are caught because
    // this runs over all inputs before any of them are consumed, so a
    // definition that precedes the foreign module in dependency order is
    // rejected as well. Same-module shadowing (`import json` then `def
    // json()` in one file) is deliberately excluded: `lower_module`
    // already reports it (`I0404`/`T0023`) with a more specific message.
    let foreign_locals: Vec<(&str, usize)> = inputs
        .iter()
        .enumerate()
        .flat_map(|(index, input)| {
            input
                .module
                .hir
                .imports
                .iter()
                .filter_map(move |binding| match binding {
                    ImportBinding::Foreign { local_name, .. } => Some((local_name.as_str(), index)),
                    _ => None,
                })
        })
        .collect();
    for (index, input) in inputs.iter().enumerate() {
        for (name, span) in &input.module.definition_spans {
            if let Some((_, owner)) = foreign_locals
                .iter()
                .find(|(local_name, owner)| local_name == name && *owner != index)
            {
                return Err(vec![(
                    index,
                    unsupported(
                        format!(
                            "module `{}` defines `{name}`, which `{}` binds to a CPython \
                             module object; shadowing a foreign import across modules is \
                             not supported yet",
                            input.display_path, inputs[*owner].display_path
                        ),
                        span_range(*span),
                    ),
                )]);
            }
        }
    }
    let display_paths: Vec<String> = inputs
        .iter()
        .map(|input| input.display_path.clone())
        .collect();
    let mut owners: HashMap<String, usize> = HashMap::new();
    let mut items = Vec::new();
    let mut type_aliases = Vec::new();
    let mut imports = Vec::new();
    let mut class_defs = Vec::new();
    for (index, input) in inputs.into_iter().enumerate() {
        let LoweredModule {
            hir,
            shadowed_builtin_exception_name: _,
            definition_spans,
            // The driver (`src/modules.rs`) consumes this before `link` runs:
            // it decides whether the entry module is seeded at all, so by the
            // time modules reach linking the answer has already been applied.
            mentions_dunder_name: _,
            // Consumed by the driver too (issue #1188): it feeds each
            // importer's lowering, and linking has no use for it.
            container_method_names: _,
        } = input.module;
        let mut own: HashSet<&str> = HashSet::new();
        for (name, span) in &definition_spans {
            if !own.insert(name) {
                continue;
            }
            if let Some(owner) = owners.get(name) {
                return Err(vec![(
                    index,
                    unsupported(
                        format!(
                            "top-level name `{name}` is already defined by `{}`; a separate \
                             namespace per module is not supported yet",
                            display_paths[*owner]
                        ),
                        span_range(*span),
                    ),
                )]);
            }
        }
        for name in own {
            owners.insert(name.to_string(), index);
        }
        // Part 1 of #1026: `ImportBinding::Foreign::item_index` is the
        // position of the import in its *own* module's item list, so it
        // has to be rebased onto the concatenated program the moment that
        // list is appended after the preceding modules' items. Captured
        // before the `extend` below, which is what makes it the offset of
        // this module's first item in the linked program.
        let item_offset = items.len();
        items.extend(hir.items);
        type_aliases.extend(hir.type_aliases);
        imports.extend(hir.imports.into_iter().map(|binding| match binding {
            ImportBinding::Foreign {
                local_name,
                module_path,
                item_index,
                span,
            } => ImportBinding::Foreign {
                local_name,
                module_path,
                item_index: item_index + item_offset,
                span,
            },
            other => other,
        }));
        let seeded = hir.seeded_builtin_exception_classes;
        class_defs.extend(
            hir.class_defs
                .into_iter()
                .filter(|(name, _)| !(seeded && is_builtin_exception_class(name))),
        );
    }
    if any_seeded {
        class_defs.extend(builtin_exception_class_defs());
    }
    Ok(HirModule {
        items,
        type_aliases,
        imports,
        class_defs,
        seeded_builtin_exception_classes: any_seeded,
    })
}

fn span_range(span: Span) -> std::ops::Range<u32> {
    span.start..span.end
}

/// The span of `name`'s definition in `module`, or the module start when
/// the shadow is a shape `lower_module` records no definition span for
/// (the shadow scan is an AST scan and sees e.g. a valueless `ValueError:
/// int`, which binds nothing at runtime).
fn definition_span(module: &LoweredModule, name: &str) -> Span {
    module
        .definition_spans
        .iter()
        .find(|(defined, _)| defined == name)
        .map(|(_, span)| *span)
        .unwrap_or(Span::new(0, 0))
}

/// The program-wide phases that close lowering, run once over the linked
/// program (or, via `lower_all`, over a single module): assigns each
/// raisable user class its runtime exception type tag in program order and
/// emits the synthetic `Exception.__init__` when some user class inherits
/// it. `Err` is a single `C0001` when the program declares more raisable
/// classes than the `u8` tag space holds.
pub fn finalize(mut hir: HirModule) -> Result<HirModule, Vec<Diagnostic>> {
    let mut any_user_exception_class = false;
    if hir.seeded_builtin_exception_classes {
        // Part 2 of #541 (D-189): assign each raisable user class its runtime
        // exception type tag here, in program order, so every downstream
        // consumer (`pycc_types`, `pycc_mir`, `pycc_codegen`) reads the same
        // number for the same class without re-deriving it. Program order is
        // the only ordering available that is stable across runs -- a hash
        // map's iteration order is not (risk R3 of this issue's plan).
        //
        // A class is raisable when its MRO reaches one of the seeded builtin
        // exception classes. The seed's shadow gate guarantees no user class
        // carries one of the 26 names, so `is_builtin_exception_class` on
        // the entry's own name identifies the synthetic entries exactly and
        // this loop never mistakes a user class named `Exception` for the
        // builtin one.
        let mut next_tag: u16 = u16::from(FIRST_USER_EXCEPTION_TYPE_TAG);
        for (name, def) in &mut hir.class_defs {
            if is_builtin_exception_class(name)
                || !def
                    .mro
                    .iter()
                    .any(|ancestor| is_builtin_exception_class(ancestor))
            {
                continue;
            }
            any_user_exception_class = true;
            if next_tag > u16::from(u8::MAX) {
                // The tag is a `u8` in `PyExceptionObj` and in every runtime
                // entry point that carries one, so the hierarchy cannot grow
                // past 256 types. No span is available here: `class_defs`
                // records no source range, and the diagnostic is about the
                // program's class count rather than any one declaration.
                // Reached only when every item lowered, so this stays a
                // one-element `Err` (P6).
                return Err(vec![Diagnostic::error(
                    "C0001",
                    format!(
                        "program declares more than {} exception classes; pycc \
                         supports at most {} user-defined exception classes \
                         per program",
                        MAX_USER_EXCEPTION_CLASSES, MAX_USER_EXCEPTION_CLASSES
                    ),
                    Span::new(0, 0),
                )]);
            }
            def.exception_type_tag = Some(next_tag as u8);
            next_tag += 1;
        }
    }
    // The synthetic `Exception.__init__` body is emitted only when a user
    // class actually inherits it -- that is, when some user class's computed
    // MRO reaches one of the seeded builtin exception classes, which is
    // exactly the condition that assigned at least one tag above. The
    // class-table entries above are metadata every program needs for name
    // and base resolution; this is *code*, and emitting an uncallable
    // constructor into every compiled program would put a dead function in
    // every object file. The synthetic classes themselves can never call
    // it: instantiating one is rejected by the type checker
    // (`pycc_types::class::resolve_instantiation`).
    if any_user_exception_class {
        hir.items.push(builtin_exception_init_item());
    }
    Ok(hir)
}

#[cfg(test)]
mod tests;
