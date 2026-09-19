//! The export-name grammar: which mangled `HirItem::Function` name is an
//! `--ext` export, and what host-visible identity it carries.
//!
//! Extracted from `src/ext_build.rs` by #1143 under `AGENTS.md`'s
//! decomposability rule -- that file crossed the ~1,000-line threshold and
//! this pull request's own work touches it. The functions themselves are
//! unchanged by the move.

use super::ExtExport;
use pycc_hir::is_public_name;

/// The host-visible identity an export is deduplicated on: `(None, name)`
/// for a module-level function, `(Some(class), method)` for a method.
pub(crate) fn export_dedup_key(export: &ExtExport) -> (Option<&str>, &str) {
    match (&export.class, &export.method) {
        (Some(class), Some(method)) => (Some(class.as_str()), method.as_str()),
        _ => (None, export.name.as_str()),
    }
}

/// What a compiled function's name says about its place in the export set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportName {
    /// A public module-level `def`. [`ExtExport::name`] is also its
    /// `PyMethodDef` `ml_name`.
    ModuleLevel,
    /// A public `@staticmethod` or `@classmethod` of a public class, whose
    /// `ml_name` is `method` inside `class`'s own table.
    Method {
        class: String,
        method: String,
        /// True for a `@classmethod`, whose compiled signature leads with
        /// an injected `cls: Ty::Instance(Class)`.
        receiver: bool,
    },
}

/// The **purely lexical** half of the export verdict: what, if anything,
/// `name` may be exported as, disregarding its signature, its class's
/// exception tag and everything else the name itself does not say.
///
/// This is the mirror of `pycc_codegen::is_ext_exportable_name`, which
/// answers the same question from the other side of a deliberate
/// duplication (`pycc_codegen` does not depend on `pycc_hir`). The two are
/// pinned equal over a shared table of names by
/// `ext_build_tests::exports`' parity test: drift is silent, because
/// [`wrapper_for`] asks `pycc_codegen::ext_thunk_required` -- which calls
/// that mirror -- to choose between the thunk `extern` and the `fnptr_`
/// `extern`, so a disagreement emits the wrong C declaration for a
/// `tuple`-carrying method rather than failing the build.
///
/// **Nothing outside `name` may enter this verdict.** The mirror receives a
/// bare `&str` and cannot see `HirModule::class_defs`, so a verdict that
/// consulted the class table would have no mirror-comparable form and the
/// parity test would stop being well-formed. `class_defs` may still be read
/// for *diagnostics*, and [`collect_exports`] layers its exception-class
/// exclusion on top of this verdict -- which makes the mirror a superset of
/// the driver's admitted set, and a superset is harmless here: at worst a
/// thunk is emitted for a name no wrapper calls.
///
/// The `0gen_` refusal is applied to the whole name before the split and
/// takes precedence. That is defense in depth rather than a live hazard:
/// `pycc_types::monomorphize` puts the substitution suffix *last*
/// (`0gen_<Class>.<method>__<P>_<C>`), so a `0gen_` name's final segment is
/// never `static` and a split-first rule would refuse it anyway.
pub(crate) fn classify_export_name(name: &str) -> Option<ExportName> {
    if name.starts_with("0gen_") {
        return None;
    }
    let mut segments = name.split('.');
    let class_or_fn = segments.next()?;
    // An empty segment is not a name `is_public_name` was written to judge:
    // `!"".starts_with('_')` is `true`, which would admit `""` and `".static"`
    // as public. Neither is a name the lowerer can emit, so both are refused
    // fail-closed here -- and the codegen mirror refuses them too, which is
    // what keeps the two predicates equal.
    if class_or_fn.is_empty() || !is_public_name(class_or_fn) {
        return None;
    }
    let Some(method) = segments.next() else {
        return Some(ExportName::ModuleLevel);
    };
    if method.is_empty() || !is_public_name(method) {
        return None;
    }
    match segments.next() {
        // `<Class>.<method>`: `MethodKind::Regular`, `PropertyGetter` or
        // `AbstractMethod`, which this spelling cannot tell apart. Refused
        // as representation -- see [`collect_exports`]' doc.
        None => None,
        Some(kind) => {
            // A fourth segment cannot arise: `pycc_hir` refuses a class
            // nested in a class or in a function, so no `A.B.method` name
            // exists. Refusing it is the fail-closed reading rather than a
            // reachable branch.
            if segments.next().is_some() {
                return None;
            }
            match kind {
                "static" => Some(ExportName::Method {
                    class: class_or_fn.to_string(),
                    method: method.to_string(),
                    receiver: false,
                }),
                "classmethod" => Some(ExportName::Method {
                    class: class_or_fn.to_string(),
                    method: method.to_string(),
                    receiver: true,
                }),
                // `<Class>.<property>.setter`: refused as representation,
                // for the same reason the bare spelling is. A `@property`
                // is attribute syntax on the host side, not a method.
                _ => None,
            }
        }
    }
}

/// The source-level spelling of a compiled function name: the name the user
/// actually wrote, for a message a user reads.
///
/// `Grid.scale.static` renders `Grid.scale`; `f` renders `f`. Two messages
/// need it and must not diverge: the `C0003` subject
/// ([`capability_gap`]) and the generated wrapper's arity `TypeError`,
/// which sits on the same object CPython itself describes as
/// `Grid.scale() takes no keyword arguments`.
///
/// This repeats only the *first* step of `pycc_mir`'s `source_frame_name`
/// (its `without_suffix` local), which is the canonical decomposition of a
/// mangled method name; that function then splits off the class and returns
/// the method alone, which is not what a message naming a class member
/// wants. A separate copy is deliberate rather than making that private
/// `fn` public: `crates/pycc_mir/src/lib.rs` is itself well over the
/// decomposability threshold, and exporting it would add a `cargo doc`
/// public-API obligation for one prefix strip.
pub(crate) fn source_level_name(name: &str) -> &str {
    for suffix in [".static", ".classmethod", ".setter"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped;
        }
    }
    name
}
