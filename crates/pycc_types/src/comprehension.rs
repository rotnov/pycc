//! Comprehension checking (PR-12, D-117): the iterable's element type, the
//! produced container type and its element gate, and the `name = <comp>`
//! statement forms at module and function scope.
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (D-185
//! tracking issue #544). The statement arms in `check_stmt` (module scope,
//! `local_names` empty) and `check_stmt_in_function` each build a
//! [`CompView`] and call [`check_comp_assign`], so both scopes share one
//! body that differs only in `local_names` -- `infer_expr(env, e)` is exactly
//! `infer_expr_in(env, &[], e)`.

use crate::{
    Environment, check_assignment, check_range_operand_in, infer_expr_in, lookup_bound_name,
};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{CompIter, HirExpr, Ty};

/// The element expressions of one comprehension, by kind.
pub(crate) enum CompElts<'a> {
    /// `[elt for ...]`.
    List(&'a HirExpr),
    /// `{elt for ...}`.
    Set(&'a HirExpr),
    /// `{key: value for ...}`.
    Dict(&'a HirExpr, &'a HirExpr),
}

/// A borrowed view of one comprehension's parts.
pub(crate) struct CompView<'a> {
    /// The D-117 synthesized loop-variable name.
    pub(crate) var: &'a str,
    pub(crate) iter: &'a CompIter,
    pub(crate) cond: Option<&'a HirExpr>,
    pub(crate) elts: CompElts<'a>,
}

/// Resolves a comprehension's iterable (`pycc_hir::CompIter`) to the loop
/// variable's type, without binding it -- mirrors `HirStmt::ForList`'s own
/// resolution exactly (`check_stmt`/`check_stmt_in_function`'s existing
/// `ForList` arms), reused rather than duplicated a third time (PR-12,
/// D-117). Range/list/dict/set element-type resolution is identical to
/// `ForList`'s; a comprehension adds nothing new here.
pub(crate) fn resolve_comp_iter(
    env: &Environment,
    local_names: &[&str],
    iter: &CompIter,
) -> Result<Ty, Diagnostic> {
    match iter {
        CompIter::Range { start, stop, step } => {
            check_range_operand_in(env, local_names, "start", start)?;
            check_range_operand_in(env, local_names, "stop", stop)?;
            check_range_operand_in(env, local_names, "step", step)?;
            Ok(Ty::Int)
        }
        CompIter::Name(name) => {
            let base_ty = lookup_bound_name(env, local_names, name)?;
            match base_ty {
                Ty::List(elem_ty) => Ok(*elem_ty),
                Ty::Dict(kv) => Ok(kv.0),
                Ty::Set(elem_ty) => Ok(*elem_ty),
                other => Err(Diagnostic::error(
                    "T0033",
                    format!(
                        "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                        other.name()
                    ),
                    Span::new(0, 0),
                )),
            }
        }
    }
}

/// Checks `cond` and the element expressions against `env`, in which the
/// loop variable is already bound, and returns the produced container type.
/// The element gate is the one the matching display already applies (D-119
/// reuses `T0034`/`T0038`/`T0036`, identical to `ListLiteral`/`SetLiteral`/
/// `DictLiteral`'s own gates; no new diagnostic code is minted).
pub(crate) fn comp_container_ty(
    env: &Environment,
    local_names: &[&str],
    cond: Option<&HirExpr>,
    elts: &CompElts<'_>,
) -> Result<Ty, Diagnostic> {
    if let Some(cond) = cond {
        infer_expr_in(env, local_names, cond)?;
    }
    match *elts {
        CompElts::List(elt) => {
            let elt_ty = infer_expr_in(env, local_names, elt)?;
            if elt_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list codegen only supports `list[int]` in v0.2, got a comprehension producing `list[{}]`",
                        elt_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::List(Box::new(Ty::Int)))
        }
        CompElts::Set(elt) => {
            let elt_ty = infer_expr_in(env, local_names, elt)?;
            if elt_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "set codegen only supports `set[int]` in v0.2, got a comprehension producing `set[{}]`",
                        elt_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::Set(Box::new(Ty::Int)))
        }
        CompElts::Dict(key, value) => {
            let key_ty = infer_expr_in(env, local_names, key)?;
            let value_ty = infer_expr_in(env, local_names, value)?;
            if key_ty != Ty::Str || value_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "dict codegen only supports `dict[str, int]` in v0.2, got a comprehension producing `dict[{}, {}]`",
                        key_ty.name(),
                        value_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        }
    }
}

/// PR-12 Task 3 (D-117): `target = <comp>` at module scope (`local_names`
/// empty) or in a function body. `var` is resolved and bound exactly like
/// `ForList`'s own loop variable *before* `cond` and the elements are
/// checked, so a reference to the loop variable inside either resolves
/// correctly; `target` is bound to the produced container type last.
pub(crate) fn check_comp_assign(
    env: &mut Environment,
    local_names: &[&str],
    target: &str,
    comp: CompView<'_>,
) -> Result<(), Diagnostic> {
    let var_ty = resolve_comp_iter(env, local_names, comp.iter)?;
    check_assignment(env, comp.var, var_ty)?;
    let container_ty = comp_container_ty(env, local_names, comp.cond, &comp.elts)?;
    check_assignment(env, target, container_ty)
}
