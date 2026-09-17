//! The native-mode gate on a `memoryview` annotation (Part 1 of #1027).
//!
//! A `memoryview` parameter is a borrowed view of storage a CPython
//! interpreter owns: the value only exists because a generated
//! `pycc build --ext` wrapper acquired a `Py_buffer` from a real
//! `memoryview` object and released it again when the call returned
//! (`src/ext_build.rs`'s wrapper generator owns that discipline). A plain
//! `pycc build` produces a standalone native executable with no interpreter
//! at all, so there is nothing to acquire a buffer *from*, and the program
//! is refused here rather than compiled into a signature nothing could ever
//! call.
//!
//! This is a sibling of `src/foreign_import.rs`, and for the same reason:
//! `crates/pycc_hir` and `crates/pycc_types` have no artifact-mode
//! awareness at all, so a refusal that depends on the mode cannot live in
//! either. `src/frontend.rs`'s `resolve_frontend_native` is the one seam
//! that knows.
//!
//! It refuses no miscompile, and that is recorded rather than left for a
//! later reader to rediscover as dead weight: Part 1 adds no expression
//! that *produces* a `memoryview` value (the constructor is still refused),
//! so no call site inside a native program could type-check against such a
//! signature in the first place. The gate exists to make the refusal
//! legible -- a named code, a sentence naming `--ext`, and a test pinning
//! both -- instead of leaving a `memoryview` annotation to fail later with
//! a message about something else.

use pycc_diag::{Diagnostic, Severity};
use pycc_hir::{HirItem, HirModule, Ty};

/// The `I04xx` code this gate emits. The family is the CPython interop
/// boundary (`docs/DIAGNOSTICS.md`): `I0403` is its nearest neighbour --
/// a CPython *import* in native mode -- and this is a CPython *buffer* in
/// native mode.
pub(crate) const NATIVE_MEMORYVIEW_CODE: &str = "I0405";

/// One [`NATIVE_MEMORYVIEW_CODE`] per function whose signature names
/// `memoryview`, paired with that function's index in `hir.items`, or
/// `Ok(())` when the program names it nowhere.
///
/// Shaped like `src/ext_build.rs`'s `collect_exports` rather than like
/// `src/foreign_import.rs`'s import walk: a `memoryview` annotation is not
/// an entry in the flat import table, it is a `Ty` on an `HirItem::Function`
/// in `hir.items`. Every offending function is reported, not only the
/// first, so one build names the whole list.
///
/// Span-less, exactly like `src/ext_build.rs`'s `capability_gap`:
/// `HirItem::Function` carries no source range -- that is the point of
/// `pycc_hir`'s lowered form -- and `pycc_diag::render_human` renders a
/// span-less diagnostic as plain `error[I0405]: <message>`. The item index
/// still names the owning *file*, which is what `src/frontend.rs` joins it
/// against.
///
/// Private functions are gated too, unlike the `--ext` export set: the
/// refusal is about the artifact having no interpreter, which a `_`-prefixed
/// name does not change.
///
/// A *signature* is the whole of this gate's job. `pycc_hir`'s
/// `annotation_to_ty` also parses a bare `x: memoryview` declaration, which
/// this walk never reaches -- it visits `params` and `return_ty`, not
/// statement bodies -- and which no artifact mode admits at all;
/// `crates/pycc_types`'s `reject_memoryview_declaration` refuses that
/// position with `C0001` in both modes.
pub(crate) fn refuse_in_native_mode(hir: &HirModule) -> Result<(), Vec<(usize, Diagnostic)>> {
    let gaps: Vec<(usize, Diagnostic)> = hir
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let HirItem::Function {
                name,
                params,
                return_ty,
                ..
            } = item
            else {
                return None;
            };
            let position = offending_position(params, return_ty)?;
            Some((index, gap(name, &position)))
        })
        .collect();
    if gaps.is_empty() {
        return Ok(());
    }
    Err(gaps)
}

/// Names the first part of a signature that mentions `memoryview`, as the
/// user wrote it, or `None` when it mentions it nowhere.
///
/// Both positions are checked, and only an exact `memoryview` matches: a
/// container of one (`list[memoryview]`) cannot be spelled at all --
/// `pycc_hir`'s `type_arg_name_to_ty` admits no such element -- so there is
/// no nested shape to recurse into.
fn offending_position(params: &[(String, Ty)], return_ty: &Ty) -> Option<String> {
    if let Some((name, _)) = params.iter().find(|(_, ty)| *ty == Ty::MemoryView) {
        return Some(format!("parameter `{name}: memoryview`"));
    }
    if *return_ty == Ty::MemoryView {
        return Some("return type `-> memoryview`".to_string());
    }
    None
}

fn gap(name: &str, position: &str) -> Diagnostic {
    Diagnostic {
        code: NATIVE_MEMORYVIEW_CODE,
        severity: Severity::Error,
        message: format!(
            "`{name}`'s {position} requires `pycc build --ext`: a `memoryview` is a borrowed \
             view of storage a CPython interpreter owns, and a native executable embeds no \
             interpreter to acquire a buffer from"
        ),
        span: None,
        label: None,
        help: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hir(items: Vec<HirItem>) -> HirModule {
        HirModule {
            seeded_builtin_exception_classes: false,
            items,
            type_aliases: Vec::new(),
            imports: Vec::new(),
            class_defs: Vec::new(),
        }
    }

    fn func(name: &str, params: Vec<(String, Ty)>, return_ty: Ty) -> HirItem {
        HirItem::Function {
            name: name.to_string(),
            params,
            return_ty,
            body: Vec::new(),
        }
    }

    #[test]
    fn a_program_without_a_memoryview_annotation_is_admitted() {
        let admitted = hir(vec![func(
            "scale",
            vec![("x".to_string(), Ty::Float)],
            Ty::Float,
        )]);
        assert!(refuse_in_native_mode(&admitted).is_ok());
    }

    #[test]
    fn every_memoryview_signature_is_reported_not_only_the_first() {
        let gaps = refuse_in_native_mode(&hir(vec![
            func("total", vec![("xs".to_string(), Ty::MemoryView)], Ty::Float),
            func("plain", vec![("x".to_string(), Ty::Int)], Ty::Int),
            // The return position is the second half of the gate: it is a
            // `C0003` capability gap under `--ext`, but in native mode it
            // is the same refusal a parameter gets.
            func("view", Vec::new(), Ty::MemoryView),
        ]))
        .expect_err("a memoryview annotation is refused in native mode");
        let messages: Vec<&str> = gaps.iter().map(|(_, gap)| gap.message.as_str()).collect();
        assert_eq!(gaps.len(), 2, "{messages:?}");
        assert!(
            gaps.iter()
                .all(|(_, gap)| gap.code == NATIVE_MEMORYVIEW_CODE && gap.span.is_none()),
            "{messages:?}"
        );
        assert!(messages[0].contains("`xs: memoryview`"), "{messages:?}");
        assert!(messages[1].contains("`-> memoryview`"), "{messages:?}");
        assert!(messages[0].contains("--ext"), "{messages:?}");
        // Each gap carries its own index in `hir.items` -- not its position
        // among the offending ones -- because that is what the driver joins
        // against the program's per-file item bounds to name the owning file.
        let positions: Vec<usize> = gaps.iter().map(|(index, _)| *index).collect();
        assert_eq!(positions, vec![0, 2], "{messages:?}");
    }
}
