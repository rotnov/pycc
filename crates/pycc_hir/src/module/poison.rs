//! Cascade suppression's two halves (D-219, D-222): [`poisonable_names`],
//! which names what a failing top-level statement would have bound, and the
//! cascade-shaped `C0001` message builders plus [`cascade_name`], which
//! recognizes a later diagnostic that names a poisoned binding.
//!
//! Extracted from `module.rs` (issue #1280) per AGENTS.md's
//! file-decomposition rule, unchanged; `module.rs` re-exports every item at
//! its previous `crate::module::` path.

use crate::import::{is_future_import, is_noop_future_feature};
use pycc_ast::{Expr, Stmt};
use pycc_diag::Diagnostic;

/// The class, type-alias, or import names a top-level statement would bind
/// -- the binding kinds that can be the root of an HIR cascade (D-219, P1;
/// #898 added the import kind, amending D-219 rule 3 -- see D-222).
///
/// `class C` -> `[C]`; `type X = ...` -> `[X]`; legacy `X: TypeAlias = ...`
/// -> `[X]`.
///
/// An import yields names exactly when it *fails*, and then the names are
/// the ones it would have bound locally: `from geometry import Point, Line`
/// -> `[Point, Line]`, `import pkg.dep as d` -> `[d]`, and a rejected
/// `from math import *` -> `math`'s whole export list. Since Part 1 of
/// #883 (#962, D-231) a `Stmt::Import` lowers -- and so poisons nothing --
/// exactly when it has one alias and a `pycc_std`-resolvable module, with
/// or without an `asname`: `import math as m` binds `m` and yields nothing,
/// while `import numpy as np` yields `[np]`. Both import arms therefore
/// mirror `import::lower_import_stmt`'s own success conditions for
/// every shape decidable from the statement alone, one arm per
/// statement kind, so such a shape that lowers poisons nothing and
/// every such shape that does not poisons. An import that lowers binds
/// a name the two cascade lookups (`annotation_to_ty`'s bare-name arm
/// and `validate_bases`) cannot resolve anyway -- they consult only the
/// class table and the alias table -- so a later annotation naming it
/// fails today either way, and that diagnostic is a genuine,
/// independent gap that must stay reported.
///
/// Recorded divergence (Part 1 of #1026): `lower_import_stmt` has a third
/// success condition this mirror deliberately does not model. A bare,
/// unaliased, undotted name that is neither a `pycc_std` module nor a
/// project module lowers to an `ImportBinding::Foreign` when -- and only
/// when -- the driver's `ResolvedImports` table answers
/// `ResolvedImport::Foreign` for that statement's span. That answer is not
/// derivable from the statement alone, which is all this function sees, so
/// `import numpy` is still classified here as a poisoning shape even in a
/// build where it lowers. What keeps the stale prediction harmless is the
/// *asymmetry* in `lower_module`'s loop, not a surviving biconditional: the
/// loop consults `poisonable_names` on both arms, but on `Ok` it only
/// `retain`s -- un-poisoning what the statement actually bound -- so a name
/// predicted for a statement that then lowered is dropped rather than
/// suppressing anything. Do not "repair" the `Stmt::Import` arm by teaching
/// it this case without a span-keyed answer table in hand, and do not
/// assume the biconditional above still holds for a bare foreign name.
/// `module::tests::a_foreign_import_lowers_and_still_poisons_its_name`
/// pins this state.
///
/// Every remaining statement kind -- `def`, assignment, expression
/// statement -- yields nothing on purpose, for that same reason: those
/// lookups can never resolve a function- or variable-bound name.
/// This is deliberately narrower than
/// `exception::expr_bound_builtin_exception_name`'s destructuring scan,
/// which answers a different question (does the module shadow a name at
/// all).
///
/// The legacy predicate is exactly `lower_legacy_type_alias_ann_assign`'s
/// shape, value included: a `Stmt::AnnAssign` whose annotation is the bare
/// name `TypeAlias`, whose target is a `Name`, and which carries a value. A
/// valueless `X: TypeAlias` binds no alias -- it falls through to ordinary
/// `AnnAssign` lowering and fails on `TypeAlias` itself -- so it yields
/// nothing here, and a later `X` diagnostic stays reported.
pub(crate) fn poisonable_names(stmt: &Stmt) -> Vec<&str> {
    match stmt {
        Stmt::ClassDef(def) => vec![def.name.as_str()],
        // Same `.expect` as `lower_type_alias_stmt`: ruff unconditionally
        // parses a `type` statement's name as `Expr::Name`.
        Stmt::TypeAlias(alias) => vec![
            alias
                .name
                .as_name_expr()
                .expect("ruff always parses a `type` statement's name as Expr::Name")
                .id
                .as_str(),
        ],
        Stmt::AnnAssign(ann) => {
            let Expr::Name(annotation) = ann.annotation.as_ref() else {
                return Vec::new();
            };
            if annotation.id.as_str() != "TypeAlias" {
                return Vec::new();
            }
            let Expr::Name(target) = ann.target.as_ref() else {
                return Vec::new();
            };
            // A valueless `X: TypeAlias` binds nothing:
            // `import::lower_legacy_type_alias_ann_assign` records no alias
            // for it and lets it fall through as an ordinary annotated
            // assignment, so poisoning `X` would suppress a genuine later
            // `X` diagnostic (Codex review on #875).
            if ann.value.is_none() {
                return Vec::new();
            }
            vec![target.id.as_str()]
        }
        Stmt::Import(import) => {
            // `import::lower_import_stmt` accepts exactly one shape this
            // function can recognize: a single alias (with or without an
            // `asname` -- Part 1 of #883, #962) and a module name `pycc_std`
            // resolves. The condition is exact rather than an approximation
            // of that arm -- its earlier `ResolvedImport::Found` branch
            // cannot fire for a stdlib-resolving name, because
            // `project_import_request` returns `None` for one, so no answer
            // is ever recorded for its span. Its `ResolvedImport::Foreign`
            // branch is the recorded divergence documented above: it turns
            // on a span-keyed driver answer this function does not have, so
            // a bare foreign name falls through and poisons even though it
            // lowers.
            if let [alias] = import.names.as_slice()
                && pycc_std::resolve_module(alias.name.as_str()).is_some()
            {
                return Vec::new();
            }
            // Every other shape fails, so poison what it would have bound:
            // the alias when present, and otherwise the first dotted segment,
            // since `import pkg.dep` binds `pkg`. `import a, b` fails as a
            // whole statement, so both of its names are poisoned.
            import
                .names
                .iter()
                .map(|alias| {
                    alias.asname.as_ref().map_or_else(
                        || {
                            let name = alias.name.as_str();
                            name.split_once('.').map_or(name, |(head, _)| head)
                        },
                        |asname| asname.as_str(),
                    )
                })
                .collect()
        }
        Stmt::ImportFrom(import) => {
            // D-229: a `from __future__ import ...` lowers (to nothing) when
            // every name is a no-op feature and none carries an `asname`;
            // `barry_as_FLUFL` is CPython-accepted but pycc-rejected, so it
            // poisons like any other failing shape. This mirror is
            // position-blind -- it sees one statement, not its index -- so
            // a *late* future import fails with the position `L0001` and
            // poisons nothing. That is a recorded divergence from D-222's
            // "a failing import poisons what it would have bound" rule and
            // is harmless: an accepted future import binds nothing either,
            // so no later diagnostic can be a cascade of it. Likewise a
            // `from __future__ import *` poisons the literal name `*` below
            // (the stdlib arm expands a wildcard to the module's export
            // list; `__future__` has none to expand), which no later
            // statement can name.
            if is_future_import(import)
                && import.names.iter().all(|alias| {
                    alias.asname.is_none() && is_noop_future_feature(alias.name.as_str())
                })
            {
                return Vec::new();
            }
            // A `level == 0` statement naming a module `pycc_std` resolves
            // takes `lower_import_stmt`'s stdlib arm; everything else (a
            // relative import, an unresolvable module) takes the project arm
            // or is rejected outright, and poisons below.
            if let Some(module) = (import.level == 0)
                .then_some(import.module.as_ref())
                .flatten()
                .and_then(|module| pycc_std::resolve_module(module.as_str()))
            {
                // Mirror that arm's success condition exactly, as the
                // `Stmt::Import` arm above mirrors its own: the statement
                // lowers when no name is the wildcard, none carries an
                // `asname`, and every name is a symbol of `module`. Anything
                // else fails, and a failed stdlib import poisons what it
                // would have bound just like a failed project one -- a
                // wildcard binds the module's whole export list, every other
                // shape binds its own aliases.
                if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                    return pycc_std::module_symbol_names(module).collect();
                }
                if import.names.iter().all(|alias| {
                    alias.asname.is_none()
                        && pycc_std::resolve_symbol(module, alias.name.as_str()).is_some()
                }) {
                    return Vec::new();
                }
            }
            // The poisoned name is the one this statement would have *bound*
            // locally, not the one it reads from the other module: under
            // `from .dep import helper as h` a later `h` is the cascade to
            // suppress, and a later `helper` is a genuine unknown name.
            import
                .names
                .iter()
                .map(|alias| {
                    alias
                        .asname
                        .as_ref()
                        .map_or(alias.name.as_str(), |asname| asname.as_str())
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

const UNKNOWN_ANNOTATION_PREFIX: &str = "type annotation `";
const UNKNOWN_ANNOTATION_SUFFIX: &str = "` is not supported yet";
const UNKNOWN_BASE_PREFIX: &str = "class `";
const UNKNOWN_BASE_INFIX: &str = "` inherits from unknown class `";
const UNKNOWN_BASE_SUFFIX: &str = "` -- base classes must be defined earlier in the same module";
const BARE_CONTAINER_PREFIX: &str = "a bare `";
const BARE_CONTAINER_INFIX: &str =
    "` type annotation is not supported yet -- write the parameterized form, e.g. `";
const BARE_CONTAINER_SUFFIX: &str = "`";

/// The `C0001` message for a bare annotation name that is neither a known
/// class nor a type alias (`func::annotation_to_ty`'s bare-name arm). The
/// only producer of this message; `cascade_name` parses it back, and a unit
/// test round-trips the two so the wording cannot drift apart.
pub(crate) fn unknown_annotation_name_message(name: &str) -> String {
    format!("{UNKNOWN_ANNOTATION_PREFIX}{name}{UNKNOWN_ANNOTATION_SUFFIX}")
}

/// The `C0001` message for a *bare* builtin container annotation -- `list`,
/// `set`, `dict` or `tuple` written with no type arguments (D-228, issue
/// #918). Split out from [`unknown_annotation_name_message`] because a bare
/// container is no longer an unknown name: the parameterized form now lowers,
/// so the actionable advice is "write `list[int]`", not "this name means
/// nothing here".
///
/// Cascade-shaped like the other two, and parsed back by [`cascade_name`]
/// through its own prefix/infix/suffix triple. That is load-bearing rather
/// than cosmetic: the classifier's job is not to decide whether *this*
/// diagnostic poisons a name, it is to name the annotation so `lower_module`
/// can suppress this diagnostic when a *failed earlier item* already poisoned
/// that same name. A module whose `class list:` fails to lower poisons the
/// name `list`, and a later `x: list` must be suppressed exactly as `x: Foo`
/// is after a failed `class Foo:`.
///
/// `frozenset` and `type` deliberately keep the generic unknown-name message:
/// neither has a `Ty` variant, so steering a user toward `frozenset[int]`
/// would point at a form this version rejects just as hard.
pub(crate) fn bare_container_annotation_message(name: &str, example: &str) -> String {
    format!("{BARE_CONTAINER_PREFIX}{name}{BARE_CONTAINER_INFIX}{example}{BARE_CONTAINER_SUFFIX}")
}

/// The `C0001` message for a base class that is not defined earlier in the
/// module (`class::mro::validate_bases`). The only producer of this message;
/// `cascade_name` parses it back under the same round-trip test.
pub(crate) fn unknown_base_message(class_name: &str, base_name: &str) -> String {
    format!("{UNKNOWN_BASE_PREFIX}{class_name}{UNKNOWN_BASE_INFIX}{base_name}{UNKNOWN_BASE_SUFFIX}")
}

/// Classifies a failed item's diagnostic (D-219, P2): `Some(name)` when it
/// is one of the three cascade-shaped `C0001`s -- the bare-name annotation
/// message naming `name`, the unknown-base message whose base is `name`, or
/// the bare-container message naming `name` -- and `None` for every other
/// diagnostic. Only `lower_module` decides whether `name` is actually
/// poisoned; a `Some` for an un-poisoned name is an ordinary, reported gap.
pub(crate) fn cascade_name(diagnostic: &Diagnostic) -> Option<&str> {
    if diagnostic.code != "C0001" {
        return None;
    }
    unknown_annotation_name(&diagnostic.message)
        .or_else(|| unknown_base_name(&diagnostic.message))
        .or_else(|| bare_container_name(&diagnostic.message))
}

fn unknown_annotation_name(message: &str) -> Option<&str> {
    message
        .strip_prefix(UNKNOWN_ANNOTATION_PREFIX)?
        .strip_suffix(UNKNOWN_ANNOTATION_SUFFIX)
}

fn bare_container_name(message: &str) -> Option<&str> {
    let (name, _) = message
        .strip_prefix(BARE_CONTAINER_PREFIX)?
        .split_once(BARE_CONTAINER_INFIX)?;
    Some(name)
}

fn unknown_base_name(message: &str) -> Option<&str> {
    let (_, base) = message
        .strip_prefix(UNKNOWN_BASE_PREFIX)?
        .split_once(UNKNOWN_BASE_INFIX)?;
    base.strip_suffix(UNKNOWN_BASE_SUFFIX)
}
