//! Bare-container annotation advice (D-228, issue #918): upgrading
//! [`crate::annotation_to_ty`]'s generic unknown-name `C0001` for a bare
//! `list`/`set`/`dict`/`tuple` into a message naming the parameterized form
//! to write, in the positions that lower one. Extracted from `func.rs` per
//! AGENTS.md's file-decomposition rule when #1264 added
//! [`with_bare_list_or_dict_advice`]; the items are moved verbatim apart
//! from their visibility and that one addition.

use crate::unsupported;
use pycc_ast::Expr;
use pycc_diag::Diagnostic;

/// The four builtin container types this version lowers from a parameterized
/// annotation (D-228, issue #918). `frozenset[T]` and `type[T]` are absent on
/// purpose: neither has a `Ty` variant, and adding one would have to clear
/// D-109's 16-byte `size_of::<Ty>()` ceiling first.
pub(super) const CONTAINER_ANNOTATION_NAMES: [&str; 4] = ["list", "set", "dict", "tuple"];

/// A worked parameterized example for a bare container annotation's `C0001`,
/// or `None` for a name that is not one of the four. `tuple` gets its own
/// two-argument example: `tuple[int]` is legal but atypical, and a
/// single-element example would read as if `tuple` were homogeneous.
pub(super) fn bare_container_example(name: &str) -> Option<&'static str> {
    match name {
        "list" => Some("list[int]"),
        "set" => Some("set[int]"),
        "dict" => Some("dict[str, int]"),
        "tuple" => Some("tuple[int, int]"),
        _ => None,
    }
}

/// Upgrades [`crate::annotation_to_ty`]'s generic unknown-name `C0001` into the
/// bare-container message that names the parameterized form (D-228, issue
/// #918) -- for the callers whose annotation position actually lowers a
/// container.
///
/// Only these do: a function or method parameter, a function or method
/// return annotation (#925), a local or module-level `AnnAssign`, and a type
/// alias. An annotated attribute target (#1264) lowers only `list[int]` and
/// `dict[str, int]`, so it opts in through [`with_bare_list_or_dict_advice`]. Class-attribute, dataclass-field and protocol-attribute positions
/// each reject `list[int]` with a `C0001` of their own, so advising the
/// parameterized form there would walk the user straight into a second
/// error. They opt out simply by not calling this, which is why the advice
/// is an opt-in upgrade rather than a position argument threaded through
/// `annotation_to_ty`: a position added later is correct without touching
/// this file -- return position joined the advising set that way, by adding
/// one `map_err` in `lower_return_annotation`.
///
/// Discarding `error` in the upgrade arm is sound because a bare
/// `Expr::Name` has exactly one failure mode in `annotation_to_ty` -- the
/// alias-table miss that builds `unknown_annotation_name_message` -- so the
/// message being replaced is always that one.
///
/// The name is reached through [`strip_transparent_wrappers`], because
/// `annotation_to_ty` propagates that same failure out of `Final[list]` and
/// `Annotated[list, "meta"]` unchanged while accepting `Final[list[int]]`:
/// matching only the outermost expression would drop the advice in exactly
/// the positions that can act on it.
pub(crate) fn with_bare_container_advice(error: Diagnostic, annotation: &Expr) -> Diagnostic {
    let stripped = strip_transparent_wrappers(annotation);
    let Expr::Name(name) = stripped else {
        return error;
    };
    match bare_container_example(name.id.as_str()) {
        // The span is the bare name, not the wrapper: that is the token the
        // user replaces, and it is where `annotation_to_ty` already pointed.
        Some(example) => unsupported(
            crate::module::bare_container_annotation_message(name.id.as_str(), example),
            pycc_ast::expr_range(stripped),
        ),
        None => error,
    }
}

/// [`with_bare_container_advice`] restricted to a bare `list`/`dict`, for
/// a position that lowers only those two parameterized forms (an annotated
/// attribute target, #1264): a bare `set`/`tuple` there keeps `error`, so the
/// advice never names a form the position refuses too.
pub(crate) fn with_bare_list_or_dict_advice(error: Diagnostic, annotation: &Expr) -> Diagnostic {
    match strip_transparent_wrappers(annotation) {
        Expr::Name(name) if matches!(name.id.as_str(), "list" | "dict") => {
            with_bare_container_advice(error, annotation)
        }
        _ => error,
    }
}

/// Peels the wrappers `annotation_to_ty` lowers by recursing into their
/// inner type, so a diagnostic about that inner type can be recognized from
/// the outside.
///
/// Only `Final[X]` (PEP 591) and `Annotated[X, ...]` (PEP 593) qualify: both
/// lower to `X` itself. The shapes accepted here mirror `annotation_to_ty`'s
/// own arms exactly -- `Final` takes one argument, `Annotated` takes a tuple
/// of at least two -- so a malformed wrapper keeps its own diagnostic rather
/// than being reported against whatever it wraps.
fn strip_transparent_wrappers(annotation: &Expr) -> &Expr {
    let Expr::Subscript(sub) = annotation else {
        return annotation;
    };
    let Expr::Name(base) = sub.value.as_ref() else {
        return annotation;
    };
    let inner = match base.id.as_str() {
        "Final" => match sub.slice.as_ref() {
            Expr::Tuple(tuple) if tuple.elts.len() != 1 => return annotation,
            Expr::Tuple(tuple) => &tuple.elts[0],
            other => other,
        },
        "Annotated" => match sub.slice.as_ref() {
            Expr::Tuple(tuple) if tuple.elts.len() >= 2 => &tuple.elts[0],
            _ => return annotation,
        },
        _ => return annotation,
    };
    strip_transparent_wrappers(inner)
}
