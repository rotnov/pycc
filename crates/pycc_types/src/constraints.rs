//! Private-helper constraint solver: the type-inference seam that runs when
//! a module's function signatures cannot be read off their annotations.
//!
//! This submodule holds the whole constraint-solving seam, extracted from
//! [`lib.rs`](crate) per the repository's source-file decomposition rule
//! (AGENTS.md "Keep source files decomposable", [D-185]), following the
//! precedent set by [`monomorphize`](crate::monomorphize),
//! [`binop`](crate::binop), and [`class`](crate::class).
//!
//! The seam is cohesive because every item in it exists to serve one
//! pipeline, entered exactly once from the crate root: `check_and_resolve`
//! calls [`checked_function_signatures`], which either accepts the
//! fully-annotated fast path ([`concrete_function_signatures`]) or falls
//! through to [`infer_function_signatures_with_solver`]. Everything else
//! here is one of that pipeline's stages, and none of them has any other
//! caller:
//!
//! * its own type vocabulary -- `TypeTerm`, `SignatureTerms`,
//!   `BinOpConstraint`, [`SolverConstraints`],
//!   [`AnnotationDefaultConstraint`], and [`ConstraintEnvironment`], none of
//!   which appear anywhere in the annotation-driven checker;
//! * the union-find over inference variables ([`fresh_variable`],
//!   [`fresh_term`], [`root`], [`resolved_term`], [`unify_terms`],
//!   [`merge_inferred_types`], [`term_for_type`]);
//! * constraint collection ([`collect_expr_constraints`],
//!   [`bind_comp_loop_var`], [`collect_block_constraints`]);
//! * constraint application ([`propagate_binop_constraints`],
//!   [`apply_annotation_defaults`]) and the signature materialization that
//!   validates a solved signature set against the ordinary checker
//!   ([`concrete_function_signatures`], [`infer_function_signatures_with_solver`],
//!   [`checked_function_signatures`]).
//!
//! Deliberately left in `lib.rs`: the annotation-driven checker itself.
//! `check_and_resolve`, `infer_expr`/`infer_expr_in`, [`check_stmt`](crate::check_stmt),
//! `check_stmt_in_function`, `check_with_signatures`, `check_assignment`,
//! [`Environment`], and the `bind_local_types_*` helpers
//! stay behind, together with the four issue-#118 `*_in_place*` fast-path
//! wrappers around `check_stmt`/`check_stmt_in_function`, which the solver
//! never calls. The solver *calls* the checker (it validates every candidate
//! signature set by re-running it), but it is the crate's primary entry
//! surface and is used by every other seam in the crate -- moving it would be
//! a different extraction, not this one.
//!
//! Deliberately *not* merged into [`solver`]: that module
//! already exists and holds a narrower, differently-motivated scope -- the
//! solver's definite-assignment control-flow join helpers, extracted for
//! issue #359. Before this extraction, `lib.rs` reached it through eight
//! qualified `solver::` paths and deliberately had no `pub use solver::*;`.
//! Folding this seam into `solver` would force adding that glob, which would
//! re-export those pre-existing definite-assignment helpers at the crate root
//! as a side effect -- a change to code outside this seam. A new sibling
//! module keeps the extraction a pure relocation. All eight of those call
//! sites moved here with the rest of the seam, so `constraints` is now the
//! only caller of `solver`, reaching it through the same `solver::` paths.
//!
//! [D-185]: https://github.com/rotnov/pycc/blob/main/docs/decisions/D-185-permit-a-dedicated-tracking-issue-per-oversized.md

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::binop::numeric_result_type;
use crate::unop::unary_result_type;
use crate::{
    Environment, annotation_marker_is_not_a_value, cast_marker_is_not_a_value,
    check_incompatible_redefinitions, check_with_signatures, enum_marker_is_not_a_value,
    is_assignable, is_generic_signature, is_known_callable_builtin, is_local, is_marker_kind,
    marker_is_not_a_value, non_callable_binding, solver, std_constant_is_not_callable,
    std_function_used_as_a_value, std_qualified_symbol, std_receiver_name, std_receiver_shadowed,
    std_scalar_to_ty, t0042, ty_contains_param, unbound_local, unsupported_callable_builtin,
};
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{
    BinOpKind, CompIter, FStringPart, HirExpr, HirItem, HirModule, HirStmt, Ty, UnaryOpKind,
};

type TypeTerm = Result<Ty, usize>;
type SignatureTerms = (Vec<String>, Vec<TypeTerm>, TypeTerm);
type BinOpConstraint = (BinOpKind, TypeTerm, TypeTerm, TypeTerm);

fn is_private_solver_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::None)
}

/// D-146 (#239): determines whether a list literal's collected element terms
/// represent a homogeneous scalar-element list this solver can carry as a
/// `Ty::List` element-type carrier. Returns the shared element `Ty` when every
/// element produced `Some(Ok(ty))`, all element types are exactly equal (exact
/// `Ty` equality, matching `infer_expr_in`'s own homogeneity rule -- NOT
/// `merge_inferred_types`, which would silently widen `bool` to `int`), and the
/// shared element type is a private-solver scalar (`is_private_solver_scalar`).
/// Returns `None` for heterogeneous lists, empty lists, non-scalar element
/// lists, or any element producing `None`/`Err` -- those keep the historical
/// `Ok(None)` behavior. The `is_private_solver_scalar` gate prevents
/// nested-container carriers (`list[list[int]]`) and non-scalar element types
/// this solver has no representation for. The returned carrier is destructured
/// by the `Subscript`/`ListPop` arms, never unified -- `unify_terms` and
/// `merge_inferred_types` are unchanged.
pub(crate) fn homogeneous_private_solver_scalar_list_element(
    element_terms: &[Option<TypeTerm>],
) -> Option<Ty> {
    let first = element_terms.first()?.as_ref()?.as_ref().ok()?;
    if !is_private_solver_scalar(first) {
        return None;
    }
    if !element_terms
        .iter()
        .all(|term| matches!(term, Some(Ok(ty)) if ty == first))
    {
        return None;
    }
    Some(first.clone())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AnnotationDefaultConstraint {
    pub(crate) initializer: TypeTerm,
    pub(crate) annotation: Ty,
}

#[derive(Debug, Default)]
pub(crate) struct SolverConstraints {
    binops: Vec<BinOpConstraint>,
    pub(crate) annotation_defaults: Vec<AnnotationDefaultConstraint>,
    non_scalar_local_terms: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct ConstraintEnvironment<'scope, 'hir> {
    pub(crate) bindings: HashMap<String, TypeTerm>,
    pub(crate) local_names: &'scope [&'hir str],
    /// Mirror of `Environment::def_rebound` (D-110): names whose net
    /// source-order module binding is a `def`, kept apart from the term
    /// bindings for the same reason -- terms must survive a `def` for
    /// representation purposes.
    pub(crate) defs_rebound: HashSet<String>,
    /// Issue #359 (Part 2 of #118): names whose binding is *maybe* —
    /// assigned in only one branch of an `if` (no `else`), or only in a
    /// loop body, or introduced as a `for` loop variable (the loop may
    /// execute zero times). Mirrors the validation pass's
    /// `BindingState::Maybe` distinction (D-147): a maybe-bound name's
    /// type term is still in `bindings` (if it IS bound, it has that
    /// type), but `collect_expr_constraints`'s `Name` arm skips
    /// unification for maybe-bound names — the validation pass's `T0041`
    /// diagnostic is the user-facing gate, not the solver's inferred type.
    pub(crate) maybe_bindings: HashSet<String>,
    /// Issue #771: names whose binding is definite (unconditionally
    /// assigned, unlike `maybe_bindings`) but whose right-hand-side
    /// expression is one the solver cannot represent as a type term at all
    /// (e.g. a class-target `cast`, a `.pop()`, an attribute read, a method
    /// call, or a heterogeneous container literal — see the `Ok(None)`
    /// arms throughout `collect_expr_constraints`). Without this set, such
    /// a name is left out of `bindings` entirely, and a later read of it
    /// falls through to the "not a solver-tracked local" case even though
    /// it *is* a syntactic local — producing a misleading `unbound_local`
    /// (`T0021`) diagnostic instead of `Ok(None)` (no term, but not an
    /// error). `HirExpr::Name` consults this set after `maybe_bindings`
    /// and after `bindings` itself, so a real term always takes priority
    /// over a stale opaque marker for the same name.
    pub(crate) opaque_bindings: HashSet<String>,
}

fn fresh_variable(parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> usize {
    let id = parents.len();
    parents.push(id);
    concrete.push(None);
    id
}

pub(crate) fn fresh_term(parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> TypeTerm {
    Err(fresh_variable(parents, concrete))
}

fn root(parents: &mut [usize], var: usize) -> usize {
    let parent = parents[var];
    if parent == var {
        parent
    } else {
        let root = root(parents, parent);
        parents[var] = root;
        root
    }
}

pub(crate) fn resolved_term(
    term: TypeTerm,
    parents: &mut [usize],
    concrete: &[Option<Ty>],
) -> Option<Ty> {
    match term {
        Ok(ty) => Some(ty),
        Err(var) => concrete[root(parents, var)].clone(),
    }
}

fn resolved_private_signature_term(
    term: TypeTerm,
    parents: &mut [usize],
    concrete: &[Option<Ty>],
    non_scalar_local_roots: &HashSet<usize>,
) -> Option<Ty> {
    match term {
        Ok(ty) => Some(ty),
        Err(var) => {
            let term_root = root(parents, var);
            let resolved = concrete[term_root].clone()?;
            (!non_scalar_local_roots.contains(&term_root) || is_private_solver_scalar(&resolved))
                .then_some(resolved)
        }
    }
}

fn inference_conflict(code: &'static str, context: &str, left: Ty, right: Ty) -> Diagnostic {
    Diagnostic::error(
        code,
        format!(
            "{context}: conflicting inferred types `{}` and `{}`",
            left.name(),
            right.name()
        ),
        Span::new(0, 0),
    )
}

pub(crate) fn unify_terms(
    left: TypeTerm,
    right: TypeTerm,
    parents: &mut [usize],
    concrete: &mut [Option<Ty>],
    code: &'static str,
    context: &str,
) -> Result<bool, Diagnostic> {
    match (left, right) {
        (Ok(left), Ok(right)) => merge_inferred_types(left.clone(), right.clone())
            .map(|_| false)
            .ok_or_else(|| inference_conflict(code, context, left, right)),
        (Err(var), Ok(ty)) | (Ok(ty), Err(var)) => {
            // D-133/D-134: this constraint solver exists only to infer a
            // `Ty::Infer` parameter/return of an unannotated private
            // helper -- it has no notion of PEP 695 call-site substitution
            // the way `infer_expr_in`'s own generic-dispatch arm does. A
            // `Ty::Param` merging into a genuinely unresolved inference
            // variable here (as opposed to the `(Ok, Ok)` arm above, an
            // ordinary structural comparison -- e.g. a generic function's
            // own body unifying its parameter's term against its return
            // term, both already `Ok(Ty::Param(_))`, ordinary and correct
            // per Task 2's "opaque, self-consistent" design) would
            // otherwise silently leak the internal type-parameter name
            // into a private helper's inferred signature, surfacing later
            // as a confusing, unnamed failure (e.g. "operator Add is not
            // defined for `T` and `int`") at a span pointing at the
            // generic `def` rather than the actual call site -- this
            // project's own practice (D-105) says a precisely nameable
            // case like this should instead get a clean, specific
            // diagnostic. Reuses `T0042` (the same family of
            // "generic-function shape or call-site instantiation
            // rejected" failures Task 2 introduced) rather than inventing
            // a new code, since this is exactly that family's
            // "instantiation rejected" shape, just discovered from the
            // inference side instead of the call side.
            //
            // Checked as a plain `if` inside this arm (not a separate
            // `(Err(_), Ok(ty)) | (Ok(ty), Err(_))`-guarded arm above it)
            // deliberately: every current production call site only ever
            // reaches this function with the "known-or-generic" side in
            // one fixed position per call site (e.g. `Return`'s own
            // `unify_terms(return_term, actual)` always puts the declared
            // return term first), so a guard on a second, differently-
            // ordered `Ok`/`Err` pattern alternative would be permanently
            // unreachable dead code under the D-014 coverage gate -- this
            // single shared check covers both orderings without splitting
            // into an unreachable region.
            if ty_contains_param(&ty) {
                return Err(t0042(format!(
                    "{context} cannot be inferred through a PEP 695 generic function's own type parameter -- add an explicit type annotation instead of relying on inference here"
                )));
            }
            let root = root(parents, var);
            let merged = match concrete[root].clone() {
                Some(current) => merge_inferred_types(current.clone(), ty.clone())
                    .ok_or_else(|| inference_conflict(code, context, current, ty))?,
                None => ty,
            };
            let changed = concrete[root] != Some(merged.clone());
            concrete[root] = Some(merged);
            Ok(changed)
        }
        (Err(left), Err(right)) => {
            let left_root = root(parents, left);
            let right_root = root(parents, right);
            if left_root == right_root {
                return Ok(false);
            }
            let merged = match (concrete[left_root].clone(), concrete[right_root].clone()) {
                (Some(left), Some(right)) => Some(
                    merge_inferred_types(left.clone(), right.clone())
                        .ok_or_else(|| inference_conflict(code, context, left, right))?,
                ),
                (Some(ty), None) | (None, Some(ty)) => Some(ty),
                (None, None) => None,
            };
            parents[right_root] = left_root;
            concrete[left_root] = merged;
            Ok(true)
        }
    }
}

fn merge_inferred_types(left: Ty, right: Ty) -> Option<Ty> {
    if left == right {
        Some(left)
    } else if matches!((left, right), (Ty::Bool, Ty::Int) | (Ty::Int, Ty::Bool)) {
        Some(Ty::Int)
    } else {
        None
    }
}

fn term_for_type(ty: Ty, parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> TypeTerm {
    if ty == Ty::Infer {
        fresh_term(parents, concrete)
    } else {
        Ok(ty)
    }
}

pub(crate) fn collect_expr_constraints(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    binops: &mut Vec<BinOpConstraint>,
    env: &ConstraintEnvironment<'_, '_>,
    expr: &HirExpr,
) -> Result<Option<TypeTerm>, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Some(Ok(Ty::Int))),
        HirExpr::FloatLiteral(_) => Ok(Some(Ok(Ty::Float))),
        HirExpr::BoolLiteral(_) => Ok(Some(Ok(Ty::Bool))),
        HirExpr::StringLiteral(_) => Ok(Some(Ok(Ty::Str))),
        HirExpr::NoneLiteral => Ok(Some(Ok(Ty::None))),
        HirExpr::Name(name) => {
            // D-136: a `pycc_hir`-qualified stdlib name (`"math.pi"`) is
            // checked before ordinary binding lookup. Post-review finding:
            // the qualified string itself can never collide with a real
            // binding (see `std_qualified_symbol`'s own doc comment), but
            // its *receiver* (`"math"`) can -- a real local/parameter
            // legally named `math` shadows the stdlib module the same way
            // `float`'s own user-definition-takes-priority guard elsewhere
            // in this function handles for that hand-recognized name (see
            // `std_receiver_shadowed`'s own doc comment).
            if let Some(symbol) = std_qualified_symbol(name) {
                let receiver = std_receiver_name(name);
                if env.bindings.contains_key(receiver) || is_local(env.local_names, receiver) {
                    return Err(std_receiver_shadowed(name));
                }
                return match symbol.kind {
                    pycc_std::StdSymbolKind::Constant { ty } => Ok(Some(Ok(std_scalar_to_ty(ty)))),
                    pycc_std::StdSymbolKind::Function { .. } => {
                        Err(std_function_used_as_a_value(name))
                    }
                    pycc_std::StdSymbolKind::EnumMarker => Err(enum_marker_is_not_a_value(name)),
                    pycc_std::StdSymbolKind::AnnotationMarker => {
                        Err(annotation_marker_is_not_a_value(name))
                    }
                    pycc_std::StdSymbolKind::CastMarker => Err(cast_marker_is_not_a_value(name)),
                    pycc_std::StdSymbolKind::ProtocolMarker
                    | pycc_std::StdSymbolKind::AbcMarker
                    | pycc_std::StdSymbolKind::DecoratorMarker => Err(marker_is_not_a_value(name)),
                };
            }
            // Issue #359 (Part 2 of #118): a maybe-bound name (assigned
            // in only one branch of an `if`, or only in a loop body) is
            // not safely readable — the validation pass's T0041 diagnostic
            // is the user-facing gate. In the solver, skip unification for
            // such names by returning `Ok(None)` (no type term available),
            // so the solver does not infer a return type from a value that
            // might not exist. This mirrors the validation pass's
            // `BindingState::Maybe` distinction (D-147).
            if env.maybe_bindings.contains(name.as_str()) {
                return Ok(None);
            }
            match env.bindings.get(name).cloned() {
                Some(term) => Ok(Some(term)),
                // Issue #771: a definitely-assigned name whose initializer
                // the solver couldn't represent as a term (see
                // `opaque_bindings`'s doc comment) is not an unbound local
                // — it just has no type term to offer. Return `Ok(None)`
                // instead of falling through to `unbound_local` below.
                None if env.opaque_bindings.contains(name.as_str()) => Ok(None),
                None if is_local(env.local_names, name) => Err(unbound_local(name)),
                None => Ok(None),
            }
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(expr) = part {
                    collect_expr_constraints(signatures, parents, concrete, binops, env, expr)?;
                }
            }
            Ok(Some(Ok(Ty::Str)))
        }
        HirExpr::Compare { left, right, .. } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, left)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, right)?;
            Ok(Some(Ok(Ty::Bool)))
        }
        // #603 (Part 2 of #573). An operand whose type is already concrete
        // is typed directly by `unary_result_type`, so a bad operand keeps
        // the unary diagnostic ("unary operator USub is not defined for
        // `str`") rather than a confusing binary one about the rewrite's
        // synthetic `0`. That path covers every operand the solver can see
        // a type for, which is the overwhelming majority.
        //
        // An operand that is still an inference variable has no type to
        // check yet, so it is deferred as the exact binary constraint
        // `pycc_mir` will lower the expression to -- `0 - x` / `0 + x`.
        // `numeric_result_type(Sub | Add, Int, operand)` *is* the unary
        // rule (`Int`/`Bool` give `Int` since `-True == -1`, `Float` gives
        // `Float`, anything else is `T0021`), so reusing it keeps the
        // solver's view of the expression identical to MIR's own rewrite
        // and the two cannot drift apart.
        HirExpr::UnaryOp { op, operand } => {
            let operand =
                collect_expr_constraints(signatures, parents, concrete, binops, env, operand)?;
            match operand {
                Some(Ok(operand_ty)) => Ok(Some(Ok(unary_result_type(*op, operand_ty)?))),
                Some(operand) => {
                    let result = fresh_term(parents, concrete);
                    let op = match op {
                        UnaryOpKind::USub => BinOpKind::Sub,
                        UnaryOpKind::UAdd => BinOpKind::Add,
                    };
                    binops.push((op, Ok(Ty::Int), operand, result.clone()));
                    Ok(Some(result))
                }
                None => Ok(None),
            }
        }
        HirExpr::BinOp { op, left, right } => {
            let left = collect_expr_constraints(signatures, parents, concrete, binops, env, left)?;
            let right =
                collect_expr_constraints(signatures, parents, concrete, binops, env, right)?;
            match (left, right) {
                (Some(left), Some(right)) => {
                    let result = fresh_term(parents, concrete);
                    binops.push((*op, left, right, result.clone()));
                    Ok(Some(result))
                }
                _ => Ok(None),
            }
        }
        HirExpr::Call { callee, args } => {
            // Mirror of `infer_expr_in`'s D-110 call-target rule (#133): an
            // active value binding shadows builtin and function lookup. At
            // module level `local_names` is empty and `bindings` holds the
            // accumulated top-level assignments in source order; for a
            // private-helper body the environment is seeded from those module
            // globals with the helper's own local names stripped and its
            // parameters re-inserted, so this gate sees a module binding
            // exactly when Python's name resolution would. Binding-first
            // ordering preserves the local diagnostics unchanged, as in
            // `infer_expr_in`. This mirror is load-bearing on its own when
            // the bound callee is neither `print` nor any `def`: without it,
            // `signatures.get` misses, the call stays unresolved, and the
            // solver dead-ends in the misleading "cannot infer return type"
            // error *before* pass 3 ever runs. For a shadowed `print`
            // specifically, the special case below would resolve the call
            // and pass 3's own gate would still catch the shadowing later --
            // there this mirror is fail-fast defense-in-depth, not the only
            // line of defense.
            if env.bindings.contains_key(callee) && !env.defs_rebound.contains(callee) {
                return Err(non_callable_binding(callee));
            }
            if is_local(env.local_names, callee) {
                return Err(unbound_local(callee));
            }
            let mut arg_terms = Vec::with_capacity(args.len());
            for arg in args {
                arg_terms.push(collect_expr_constraints(
                    signatures, parents, concrete, binops, env, arg,
                )?);
            }
            if callee == "print" {
                return Ok(Some(Ok(Ty::None)));
            }
            if callee == "len" {
                // D-105 point 3: `len(lst)` is a hand-recognized builtin
                // call, same as `print` above, not a user-declarable
                // signature. Its own return (`Ty::Int`) never depends on
                // the list's element type, so it's always producible here
                // regardless of whether the argument's own term has
                // resolved yet -- unlike `ListLiteral`/`Subscript`/
                // `ListAppend` above, there's no homogeneity-style check
                // to defer. Only a term that is *already* a known concrete
                // type can be validated at this point in constraint
                // collection (union-find resolution hasn't run yet); an
                // unresolved argument is left to the real check pass
                // (`infer_expr_in`) below, matching this solver's existing
                // lenient-until-known pattern. PR-11 Task 3 (D-123) relaxed
                // the argument-type check below to also accept `Ty::Dict`;
                // PR-11 Task 7 (D-123) relaxes it once more to also accept
                // `Ty::Set`.
                if arg_terms.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!("`len` expects exactly 1 argument, got {}", arg_terms.len()),
                        Span::new(0, 0),
                    )
                    .with_help("pass exactly 1 argument"));
                }
                if let Some(Ok(arg_ty)) = &arg_terms[0]
                    && !matches!(arg_ty, Ty::List(_) | Ty::Dict(_) | Ty::Set(_))
                {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `{}`",
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ).with_help("pass a `list[T]`, `dict[K, V]`, or `set[T]` value"));
                }
                return Ok(Some(Ok(Ty::Int)));
            }
            // #435: `isinstance`/`issubclass` are compile-time-evaluated
            // builtins that always return `Ty::Bool`. The constraint solver
            // only needs the result type — the actual validation and
            // compile-time evaluation happen in `infer_expr_in`'s own Call
            // arm. The class arguments (bare names or tuples of bare names)
            // produce `Ok(None)` terms in the solver (class names are not
            // value bindings), which is harmless.
            // A user-defined function named `isinstance`/`issubclass` takes
            // priority over the builtin (same pattern as `float` above).
            if (callee == "isinstance" || callee == "issubclass")
                && !signatures.contains_key(callee)
            {
                return Ok(Some(Ok(Ty::Bool)));
            }
            // #767: `cast(T, value)` is a compile-time-only construct whose
            // type is exactly `T`. Mirrors `infer_expr_in`'s own `cast`
            // interception; without this mirror the callee misses
            // `signatures`, the call stays unresolved, and the solver
            // dead-ends in "cannot infer return type" before the validation
            // pass ever reports the real diagnostic. `args[0]` is a bare
            // type name, which the `Name` arm above already resolved to a
            // harmless `Ok(None)` term (a type name is not a value
            // binding), exactly as for `isinstance`'s class argument.
            //
            // A malformed call shape (wrong arity, or a first argument that
            // is not a bare name) is reported here, from the same shared
            // helper `check_cast` uses, so the two passes cannot drift.
            //
            // The solver has no class table (`ConstraintEnvironment` carries
            // only value bindings), so it produces a *term* only for the
            // four builtin scalar target names it can recognize on its own;
            // an unverified class name yields `Ok(None)` and leaves the
            // decision to `check_cast`. Producing an unverified
            // `Ty::Instance(name)` here instead was measurably worse: for
            // `cast(Nope, x)` the solver's term reached the return-type
            // comparison first and reported `T0022` ("conflicting inferred
            // types `int` and `Nope`") in place of `check_cast`'s accurate
            // `T0001` ("`Nope` is not a known class or builtin type").
            //
            // #767 review fix (D-198): unlike `check_cast`, this arm does
            // not reject a representation-changing target (`cast(str, 5)`,
            // `cast(int, flag)`) — the
            // solver has no resolved `Ty` for `value` to compare against at
            // this point, only an unsolved term. This is the same asymmetry
            // `check_isinstance`'s own doc comment already accepts between
            // the two passes: full validation runs on the validation-pass
            // route (an annotated function's body, module level,
            // `AnnAssign`) and is absent for a return-type-inferred private
            // helper, which only ever reaches this solver arm.
            if callee == "cast" && !signatures.contains_key(callee) {
                let target = crate::class::cast_target_name(args)?;
                if pycc_hir::is_builtin_type_name(target) {
                    return Ok(Some(Ok(crate::class::cast_target_ty(target))));
                }
                return Ok(None);
            }
            if let Some(symbol) = std_qualified_symbol(callee) {
                // Post-review finding: see `std_receiver_shadowed`'s own
                // doc comment -- a real local/parameter named `math`
                // shadows the stdlib module.
                let receiver = std_receiver_name(callee);
                if env.bindings.contains_key(receiver) || is_local(env.local_names, receiver) {
                    return Err(std_receiver_shadowed(callee));
                }
                let pycc_std::StdSymbolKind::Function {
                    arg_tys: expected_arg_tys,
                    ret_ty,
                } = symbol.kind
                else {
                    return Err(if is_marker_kind(symbol.kind) {
                        if matches!(symbol.kind, pycc_std::StdSymbolKind::EnumMarker) {
                            enum_marker_is_not_a_value(callee)
                        } else if matches!(symbol.kind, pycc_std::StdSymbolKind::AnnotationMarker) {
                            annotation_marker_is_not_a_value(callee)
                        } else if matches!(symbol.kind, pycc_std::StdSymbolKind::CastMarker) {
                            cast_marker_is_not_a_value(callee)
                        } else {
                            marker_is_not_a_value(callee)
                        }
                    } else {
                        std_constant_is_not_callable(callee)
                    });
                };
                if arg_terms.len() != expected_arg_tys.len() {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`{callee}` expects {} argument(s), got {}",
                            expected_arg_tys.len(),
                            arg_terms.len()
                        ),
                        Span::new(0, 0),
                    )
                    .with_help(format!(
                        "pass exactly {} argument(s)",
                        expected_arg_tys.len()
                    )));
                }
                for (term, expected) in arg_terms.iter().zip(expected_arg_tys) {
                    if let Some(Ok(arg_ty)) = term
                        && *arg_ty != std_scalar_to_ty(*expected)
                    {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "`{callee}` expects `{}`, got `{}`",
                                std_scalar_to_ty(*expected).name(),
                                arg_ty.name()
                            ),
                            Span::new(0, 0),
                        )
                        .with_help(format!(
                            "pass a `{}` value",
                            std_scalar_to_ty(*expected).name()
                        )));
                    }
                }
                return Ok(Some(Ok(std_scalar_to_ty(ret_ty))));
            }
            if callee == "float" && !signatures.contains_key(callee) {
                // A user-defined `float` takes priority over the builtin -- see
                // `infer_expr_in`'s own identical guard and its comment for why
                // this differs from `len`/`print`, which need no such guard.
                // Mirrors the `len` arm immediately above for the same reason (D-105
                // point 3's rationale applies identically): `float`'s own return type
                // (`Ty::Float`) never depends on the argument's resolved type, so it
                // is always producible here regardless of whether the argument's own
                // term has resolved yet. Only an already-concretely-resolved argument
                // can be validated at this point (union-find resolution hasn't run);
                // an unresolved argument is left to the real check pass
                // (`infer_expr_in`) above, matching this solver's existing
                // lenient-until-known pattern.
                if arg_terms.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`float` expects exactly 1 argument, got {}",
                            arg_terms.len()
                        ),
                        Span::new(0, 0),
                    )
                    .with_help("pass exactly 1 argument"));
                }
                if let Some(Ok(arg_ty)) = &arg_terms[0]
                    && !matches!(arg_ty, Ty::Int | Ty::Float | Ty::Bool)
                {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "`float` expects an `int`, `float`, or `bool` argument, got `{}`",
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    )
                    .with_help("pass an `int`, `float`, or `bool` value"));
                }
                return Ok(Some(Ok(Ty::Float)));
            }
            let Some(signature) = signatures.get(callee) else {
                // Issue #142: a private helper calling a known callable
                // builtin (e.g. `ValueError("x")`) gets the same `C0001`
                // classification as the final validation pass, rather than
                // deferring with `Ok(None)` -- the builtin genuinely exists
                // in Python 3.14, so it is a capability gap, not an
                // unresolved callee. A genuinely unknown name still returns
                // `Ok(None)` and defers to final validation's `T0021`.
                if is_known_callable_builtin(callee) {
                    return Err(unsupported_callable_builtin(callee));
                }
                return Ok(None);
            };
            for (index, (arg, parameter)) in arg_terms.into_iter().zip(&signature.1).enumerate() {
                // Unify whenever either side is still an inference variable --
                // not just when the callee's own parameter is unresolved.
                // This used to only match `parameter: Err(_)`, so a concrete
                // (e.g. explicitly annotated) callee parameter never
                // constrained an unresolved *caller* argument variable in the
                // reverse direction, even though `unify_terms` itself already
                // handles that case symmetrically (self-review finding,
                // pre-merge).
                if let Some(arg) = arg
                    && matches!((&arg, parameter), (Err(_), _) | (_, Err(_)))
                {
                    // Defense in depth against a `Ty::Param` leak (finding
                    // #2, PR-13 fix round): this solver has no notion of
                    // `instantiate_generic_call` -- it constrains an
                    // unannotated parameter's fresh inference variable
                    // directly against whichever concrete side is
                    // available. When that "concrete" side is actually a
                    // still-generic function's own uninstantiated `x: T`
                    // parameter type, or a still-generic call's `-> T`
                    // return type flowing in as an argument, unifying it
                    // in would bind the unannotated parameter's resolved
                    // type to the raw internal `Ty::Param` representation
                    // -- which then surfaces to the user in a later
                    // diagnostic (e.g. an `Add`-not-defined error
                    // mentioning `T` instead of a real type). Reject this
                    // shape here, before that leak can happen, with a
                    // clear, dedicated diagnostic instead.
                    if matches!(&arg, Ok(ty) if ty_contains_param(ty))
                        || matches!(parameter, Ok(ty) if ty_contains_param(ty))
                    {
                        return Err(Diagnostic::error(
                            "T0042",
                            format!(
                                "cannot infer the type of argument {} of private helper `{callee}` from a generic function's uninstantiated type; add an explicit type annotation",
                                index + 1
                            ),
                            Span::new(0, 0),
                        ).with_help("add an explicit type annotation"));
                    }
                    unify_terms(
                        parameter.clone(),
                        arg,
                        parents,
                        concrete,
                        "T0021",
                        &format!("argument {} of private helper `{callee}`", index + 1),
                    )?;
                }
            }
            Ok(Some(signature.2.clone()))
        }
        // D-146 (#239): `TypeTerm` (`Result<Ty, usize>`) has no unification-
        // friendly representation for `Ty::List` -- this solver only exists to
        // infer scalar `Ty::Infer` parameters/returns of underscore-prefixed
        // private helpers (D-045), and list homogeneity/element-type checking
        // is `infer_expr_in`'s job, not this constraint collector's (see that
        // function's `HirExpr::ListLiteral`/`Subscript`/`ListAppend` arms
        // below). Recurse into every element to keep propagating genuine
        // errors (e.g. an unbound local used as a list element). When every
        // element produces `Some(Ok(ty))`, all element types are exactly equal
        // (exact `Ty` equality, matching `infer_expr_in`'s own homogeneity
        // rule -- NOT `merge_inferred_types`, which would silently widen
        // `bool` to `int`), and the shared element type is a private-solver
        // scalar (`is_private_solver_scalar` -- `Ty::Int`/`Ty::Float`/
        // `Ty::Bool`/`Ty::Str`/`Ty::None`), return `Some(Ok(Ty::List(...)))`
        // as a destructured element-type carrier -- never unified, only
        // destructured by the `Subscript`/`ListPop` arms below to extract the
        // scalar element type for a scalar return-type inference. The
        // `is_private_solver_scalar` gate prevents nested-container carriers
        // (`list[list[int]]`) and non-scalar element types this solver has no
        // representation for. Heterogeneous lists, empty lists, non-scalar
        // element lists, or any element producing `None`/`Err` keep the
        // historical `Ok(None)` behavior -- returning `Err` here for a case
        // this solver can't actually validate would wrongly preempt
        // `checked_function_signatures`' fallback to the real, list-aware
        // check pass (`check_with_signatures`) that runs after this solver.
        // `unify_terms` and `merge_inferred_types` are unchanged -- the
        // carrier is destructured, never unified.
        HirExpr::ListLiteral(elements) => {
            let mut element_terms = Vec::with_capacity(elements.len());
            for element in elements {
                element_terms.push(collect_expr_constraints(
                    signatures, parents, concrete, binops, env, element,
                )?);
            }
            if let Some(element_ty) = homogeneous_private_solver_scalar_list_element(&element_terms)
            {
                Ok(Some(Ok(Ty::List(Box::new(element_ty)))))
            } else {
                Ok(None)
            }
        }
        HirExpr::Subscript { base, index } => {
            let base_term =
                collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, index)?;
            // D-146 (#239): when the base resolves to a `Ty::List` element-
            // type carrier (produced by the `ListLiteral` arm above or a
            // `Ty::List`-bound name), extract the scalar element type -- the
            // carrier is destructured, never unified. Otherwise keep the
            // historical `Ok(None)` behavior (the base/index recursion above
            // already propagated genuine errors).
            if let Some(Ok(Ty::List(element_ty))) = base_term {
                Ok(Some(Ok(*element_ty)))
            } else {
                Ok(None)
            }
        }
        // PR-12 Task 7 (D-118): structurally identical to `Subscript` above
        // -- a slice's base/bounds are, like a subscript's base/index,
        // ordinary sub-expressions this solver needs to keep walking into
        // (e.g. `some_param[1:3]` inside a private helper, where
        // `some_param`'s own type is exactly what the solver is trying to
        // pin down), but a `Ty::List`/`Ty::Int` base-type or bound-type gate
        // is `infer_expr_in`'s job, not this constraint collector's. Recurse
        // into `base` and every present bound only to keep propagating
        // genuine errors (e.g. an unbound local used as a bound); produce no
        // term for the `Slice` expression's own overall type.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            for bound in [start, stop, step].into_iter().flatten() {
                collect_expr_constraints(signatures, parents, concrete, binops, env, bound)?;
            }
            Ok(None)
        }
        HirExpr::ListAppend { list: _, value } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            Ok(None)
        }
        // Same reasoning as `ListLiteral` above (PR-11 Task 3): dict
        // key/value homogeneity and the `dict[str, int]`-only gate are
        // `infer_expr_in`'s job, not this solver's. Recurse into every
        // key and value only to keep propagating genuine errors.
        HirExpr::DictLiteral(pairs) => {
            for (key, value) in pairs {
                collect_expr_constraints(signatures, parents, concrete, binops, env, key)?;
                collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            }
            Ok(None)
        }
        // Same reasoning as `ListLiteral`/`DictLiteral` above (PR-11 Task
        // 7): set element homogeneity and the `set[int]`-only gate are
        // `infer_expr_in`'s job, not this solver's. Recurse into every
        // element only to keep propagating genuine errors.
        HirExpr::SetLiteral(elements) => {
            for element in elements {
                collect_expr_constraints(signatures, parents, concrete, binops, env, element)?;
            }
            Ok(None)
        }
        // Same reasoning as `ListLiteral`/`DictLiteral`/`SetLiteral` above
        // (PR-11b Task 3, D-116): the per-element int/bool/float membership
        // gate is `infer_expr_in`'s job, not this solver's. Recurse into
        // every element only to keep propagating genuine errors.
        HirExpr::TupleLiteral(elements) => {
            for element in elements {
                collect_expr_constraints(signatures, parents, concrete, binops, env, element)?;
            }
            Ok(None)
        }
        // PR-12 Task 10 (D-119): the base-type gate (`T0033`) and the
        // value/key/default-type gate (`T0021`) are `infer_expr_in`'s job,
        // not this solver's -- same reasoning as `ListAppend` above. `list`
        // is a plain `String`, not a sub-expression, so there is nothing to
        // recurse into for `ListPop`. D-146 (#239): when `list` is bound in
        // `env.bindings` to a `Ty::List` element-type carrier (produced by
        // the `ListLiteral` arm above or a `Ty::List`-typed parameter),
        // extract the scalar element type -- the carrier is destructured,
        // never unified. Otherwise keep the historical `Ok(None)` behavior.
        HirExpr::ListPop { list } => {
            if let Some(Ok(Ty::List(element_ty))) = env.bindings.get(list).cloned() {
                Ok(Some(Ok(*element_ty)))
            } else {
                Ok(None)
            }
        }
        HirExpr::DictGetOrDefault {
            dict: _,
            key,
            default,
        } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, key)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, default)?;
            Ok(None)
        }
        HirExpr::SetAdd { set: _, value } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            Ok(None)
        }
        // D-154 (Part 1 of #375): same reasoning as `Subscript`/`ListPop`
        // above -- the base-instance-type gate and attribute/method
        // resolution are `infer_expr_in`'s job, not this solver's (which
        // only ever runs for a *private, unannotated* function's body).
        // Recurse into `base` (and, for `MethodCall`, every argument) only
        // to keep propagating genuine errors; produce no term for either
        // expression's own overall type, mirroring `HirExpr::Subscript`'s
        // own pre-D-146 "no unification term" default and `ListPop`'s own
        // doc comment's documented consequence (a private function
        // assigning from one of these expressions registers no real type
        // term for the target -- a pre-existing, not novel, gap this
        // project already accepts for every other container-shaped
        // expression). Issue #771/D-199: the target is still tracked as
        // definitely-but-opaquely bound (`opaque_bindings`), so a later
        // read of it no longer misfires as an unbound local; only the
        // missing type term itself remains unchanged.
        HirExpr::AttrGet { base, .. } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            Ok(None)
        }
        HirExpr::MethodCall { base, args, .. } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            for arg in args {
                collect_expr_constraints(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(None)
        }
        // PEP 695 (#387): `C[type_arg](args)` — a generic class
        // instantiation. The args are recursed into for constraint
        // collection. The expression itself produces a concrete
        // `Ty::Instance(class)` type term (not `Ok(None)` like
        // container-shaped expressions), because a GCI's result is a
        // class instance that the solver needs to track so a local
        // assigned from it (`b = C[int](1)`) gets bound in the solver's
        // environment. The type_arg is a compile-time `Ty`, not a runtime
        // expression, so it needs no constraint traversal. The actual
        // type-checking (verifying the class is generic, the type_arg is
        // scalar, and the args match `__init__`) happens in `infer_expr_in`,
        // not here.
        HirExpr::GenericClassInstantiate { class, args, .. } => {
            for arg in args {
                collect_expr_constraints(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(Some(Ok(Ty::Instance(Box::new(class.clone())))))
        }
        // #433: `Super` carries no sub-expressions to recurse into and
        // produces no unification term — it is a compile-time marker only
        // meaningful as the base of a `MethodCall`/`AttrGet`, which the
        // solver's own `MethodCall`/`AttrGet` arms already recurse past
        // (both return `Ok(None)` after recursing into `base`).
        HirExpr::Super => Ok(None),
        // PEP 572 (#774), deep-review follow-up (round 4): `target :=
        // value`. `name`'s own binding is registered by
        // `bind_named_expr_targets`, a separate pre-pass `collect_block_
        // constraints`'s own `If`/`While`/`ExprStmt` arms run over the whole
        // test/expression *before* reaching this function -- this function
        // takes `env: &ConstraintEnvironment`, immutable, since nearly every
        // other arm here only ever recurses for constraints without
        // touching bindings, so binding here directly is not possible
        // without widening every call site to `&mut` for this one arm's
        // sake. `value` is still recursed into here for its own constraint
        // collection (re-deriving the same constraints `bind_named_expr_
        // targets` already collected for it, exactly as `crate::
        // collect_named_expr_bindings`'s own doc comment documents and
        // accepts for the same reason in the full flow-sensitive checker),
        // and its term is propagated as this expression's own overall term,
        // since a walrus's value *is* the expression's value (mirroring
        // `infer_expr_in`'s own `NamedExpr` arm exactly).
        HirExpr::NamedExpr { name: _, value } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, value)
        }
    }
}

/// PEP 572 (#774), deep-review follow-up (round 4): binds each walrus
/// target's own inferred type into the solver's environment before the
/// containing `If`/`While` test or bare `ExprStmt` is walked for
/// unification by `collect_expr_constraints` above -- mirroring `crate::
/// collect_named_expr_bindings`'s own pre-pass pattern in the full
/// flow-sensitive checker exactly, including its documented tradeoff of
/// recomputing `value`'s constraints rather than threading the term through
/// from this pre-pass (see that function's own doc comment). Without this,
/// a walrus target reaches `local_names` (via `collect_named_expr_names_in_
/// expr`) but never gets a `bindings` entry, so a later read of that name
/// wrongly fails `collect_expr_constraints`'s own `Name` arm's `unbound_
/// local` check even though the walrus unconditionally executes whenever
/// the statement containing it is reached.
///
/// Walked in the same left-to-right evaluation order as `collect_expr_
/// constraints`'s own traversal, so a later sibling sub-expression that
/// reads an earlier walrus-bound name (`(a := 1) + (b := a + 1)`) resolves
/// correctly. Each target is bound as a *definite* binding -- not a
/// maybe-binding -- mirroring the `Assign` arm above exactly (`defs_
/// rebound.remove`, `maybe_bindings.remove`, `bindings.entry(name).or_
/// insert(term)`): a walrus nested anywhere in an `if`/`while` test
/// executes whenever the statement itself is reached, unlike a name
/// assigned in only one branch of the `if`, which is bound separately by
/// `collect_block_constraints`'s own branch-join logic, not by this walk.
fn bind_named_expr_targets(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    binops: &mut Vec<BinOpConstraint>,
    env: &mut ConstraintEnvironment<'_, '_>,
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    match expr {
        HirExpr::NamedExpr { name, value } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, value)?;
            if let Some(term) =
                collect_expr_constraints(signatures, parents, concrete, binops, env, value)?
            {
                env.defs_rebound.remove(name.as_str());
                env.maybe_bindings.remove(name.as_str());
                env.bindings.entry(name.clone()).or_insert(term);
            }
            Ok(())
        }
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::ListPop { .. }
        | HirExpr::Super => Ok(()),
        HirExpr::Call { args, .. } => {
            for arg in args {
                bind_named_expr_targets(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(())
        }
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, left)?;
            bind_named_expr_targets(signatures, parents, concrete, binops, env, right)
        }
        HirExpr::UnaryOp { operand, .. } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, operand)
        }
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(inner) = part {
                    bind_named_expr_targets(signatures, parents, concrete, binops, env, inner)?;
                }
            }
            Ok(())
        }
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            for e in es {
                bind_named_expr_targets(signatures, parents, concrete, binops, env, e)?;
            }
            Ok(())
        }
        HirExpr::Subscript { base, index } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, base)?;
            bind_named_expr_targets(signatures, parents, concrete, binops, env, index)
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, base)?;
            for bound in [start, stop, step].into_iter().flatten() {
                bind_named_expr_targets(signatures, parents, concrete, binops, env, bound)?;
            }
            Ok(())
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, value)
        }
        HirExpr::DictLiteral(pairs) => {
            for (k, v) in pairs {
                bind_named_expr_targets(signatures, parents, concrete, binops, env, k)?;
                bind_named_expr_targets(signatures, parents, concrete, binops, env, v)?;
            }
            Ok(())
        }
        HirExpr::DictGetOrDefault { key, default, .. } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, key)?;
            bind_named_expr_targets(signatures, parents, concrete, binops, env, default)
        }
        HirExpr::AttrGet { base, .. } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, base)
        }
        HirExpr::MethodCall { base, args, .. } => {
            bind_named_expr_targets(signatures, parents, concrete, binops, env, base)?;
            for arg in args {
                bind_named_expr_targets(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(())
        }
        HirExpr::GenericClassInstantiate { args, .. } => {
            for arg in args {
                bind_named_expr_targets(signatures, parents, concrete, binops, env, arg)?;
            }
            Ok(())
        }
    }
}

/// Binds a comprehension's synthesized loop variable (PR-12 Task 3, D-117)
/// in the constraint solver's environment, mirroring `collect_block_constraints`'s
/// own `ForRange`/`ForList` arms exactly rather than duplicating their logic
/// a third time: a `CompIter::Range` iterable gives the loop variable the
/// concrete `Ty::Int` fact (unifying against any existing term, exactly like
/// `ForRange`'s own loop variable), while a `CompIter::Name` iterable gives
/// it a fresh, unconstrained term (exactly like `ForList`'s own loop
/// variable) since this solver doesn't track a list/dict/set-typed name's
/// element type at all. Shared by all three comprehension statement kinds.
fn bind_comp_loop_var(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    binops: &mut Vec<BinOpConstraint>,
    env: &mut ConstraintEnvironment<'_, '_>,
    var: &str,
    iter: &CompIter,
) -> Result<(), Diagnostic> {
    match iter {
        CompIter::Range { start, stop, step } => {
            for (position, expr) in [("start", start), ("stop", stop), ("step", step)] {
                if let Some(term @ Err(_)) =
                    collect_expr_constraints(signatures, parents, concrete, binops, env, expr)?
                {
                    unify_terms(
                        term,
                        Ok(Ty::Int),
                        parents,
                        concrete,
                        "T0021",
                        &format!("comprehension range {position}"),
                    )?;
                }
            }
            if let Some(existing) = env.bindings.get(var).cloned() {
                unify_terms(
                    existing,
                    Ok(Ty::Int),
                    parents,
                    concrete,
                    "T0023",
                    &format!("assignment to comprehension loop variable `{var}`"),
                )?;
            } else {
                env.bindings.insert(var.to_string(), Ok(Ty::Int));
            }
        }
        CompIter::Name(_) => {
            if !env.bindings.contains_key(var) {
                let term = fresh_term(parents, concrete);
                env.bindings.insert(var.to_string(), term);
            }
        }
    }
    Ok(())
}

pub(crate) fn collect_block_constraints(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    constraints: &mut SolverConstraints,
    env: &mut ConstraintEnvironment<'_, '_>,
    body: &[HirStmt],
    return_term: Option<TypeTerm>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            HirStmt::Assign { target, value } => {
                // A value assignment re-shadows any earlier same-named `def`
                // (D-110), independent of the first-term-wins rule below.
                env.defs_rebound.remove(target.as_str());
                // Issue #359 (Part 2 of #118): an unconditional assignment
                // upgrades a maybe-bound name back to definitely bound
                // (mirrors the validation pass's contract: `if c: x = 1`
                // followed by `x = 2` makes `x` readable).
                env.maybe_bindings.remove(target.as_str());
                // Issue #771: this is a fresh unconditional (re)assignment,
                // so any previous opaque marker for `target` is stale
                // regardless of what the new initializer's term turns out
                // to be — it is either replaced by a real term below, or
                // reinstated as opaque by the `else` arm.
                env.opaque_bindings.remove(target.as_str());
                if let Some(term) = collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )? {
                    env.bindings.entry(target.clone()).or_insert(term);
                } else {
                    // The target is unconditionally assigned, but the
                    // solver produced no term for the initializer (e.g. a
                    // class-target `cast`, `.pop()`, an attribute read, a
                    // method call, or a heterogeneous container literal).
                    // Track it as opaquely-but-definitely bound so a later
                    // read reports "no term" instead of the misleading
                    // "not bound before this use".
                    env.opaque_bindings.insert(target.clone());
                }
            }
            HirStmt::AnnAssign {
                target,
                value: Some(value),
                annotation,
                is_final: _,
            } => {
                // Issue #359 (Part 2 of #118): an unconditional annotated
                // assignment upgrades a maybe-bound name back to definitely
                // bound, same as a plain `Assign`.
                env.maybe_bindings.remove(target.as_str());
                // Issue #771: this arm always binds `target` into
                // `env.bindings` below regardless of the initializer term
                // (see the comment further down), so `target` is never
                // opaque by the time this arm finishes. Clear any stale
                // opaque marker for hygiene, mirroring the `maybe_bindings`
                // removal above; `HirExpr::Name`'s lookup already prefers
                // `bindings` over `opaque_bindings` either way.
                env.opaque_bindings.remove(target.as_str());
                if let Some(term) = collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )? {
                    // The annotation is a directional bound on the initializer,
                    // not a symmetric equality. Defer it until every hard
                    // call/operator constraint has been collected so later
                    // `bool` evidence is not widened to `int` merely because
                    // this body was visited first.
                    constraints
                        .annotation_defaults
                        .push(AnnotationDefaultConstraint {
                            initializer: term,
                            annotation: annotation.clone(),
                        });
                }
                // A scalar target has the declared type even when the
                // collector cannot produce an initializer term. A non-scalar
                // target is still bound, but deliberately remains unresolved:
                // otherwise a hand-built HIR module could use that annotation
                // to materialize an inferred container signature through a
                // later `return target`. Keep the existing first-binding-wins
                // representation rule; final validation checks initializer
                // compatibility and re-declarations whenever signature
                // inference can otherwise complete.
                if !env.bindings.contains_key(target) {
                    let target_term = if is_private_solver_scalar(annotation) {
                        Ok(annotation.clone())
                    } else {
                        let var = fresh_variable(parents, concrete);
                        constraints.non_scalar_local_terms.push(var);
                        Err(var)
                    };
                    env.bindings.insert(target.clone(), target_term);
                }
            }
            // Deliberately out of scope for issue #245: `ConstraintEnvironment`
            // (unlike the checker's `Environment`) has no declared-but-unbound
            // side-table, and every entry in its `env.bindings` is treated as
            // resolved and readable. Registering the annotation here the same
            // way the `Some(value)` arm above does would make the name
            // readable in this solver, silently accepting `def _f():\n    x:
            // int\n    return x` (should still raise T0021, unbound) instead
            // of only `def _f():\n    x: int\n    x = 1\n    return x`. A
            // parallel declared-side-table for `ConstraintEnvironment` is a
            // separate, independently-testable follow-up if solver-scope
            // coverage of this gap is wanted later.
            HirStmt::AnnAssign { value: None, .. } => {}
            HirStmt::ExprStmt(expr) => {
                // PEP 572 (#774), deep-review follow-up (round 4): bind
                // before unifying -- see `bind_named_expr_targets`'s own
                // doc comment for why this pre-pass is required.
                bind_named_expr_targets(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    expr,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    expr,
                )?;
            }
            HirStmt::If { test, body, orelse } => {
                // PEP 572 (#774), deep-review follow-up (round 4): bind
                // before unifying -- the test always executes, so this
                // binding is unconditional relative to the branch join
                // below (mirroring `crate::collect_named_expr_bindings`'s
                // own `If` arm ordering rationale in the full checker).
                bind_named_expr_targets(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    test,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    test,
                )?;
                // Issue #359 (Part 2 of #118): clone the environment for
                // each branch so bindings from one branch do not leak into
                // the other, then join them back — mirroring the validation
                // pass's `join_if_branches` (D-147). Names introduced by
                // only one branch are tracked as maybe-bound in
                // `maybe_bindings`; names introduced by both branches are
                // definitely bound.
                // Issue #771 join-site follow-up: include `opaque_bindings`
                // alongside `bindings` here. A name definitely-but-opaquely
                // bound before this construct must count as pre-existing
                // too -- otherwise a branch that reassigns it to a real,
                // solver-representable term looks "newly introduced" to the
                // join helper below, and when only one branch performs that
                // reassignment the name is misclassified as bound in both
                // branches (the other, untouched branch still carries the
                // opaque marker), unmasking a term that only reflects one
                // path as if it were unconditionally correct. Confirmed as a
                // real gap by the pinned local reviewer's second pass.
                let pre_existing: HashSet<String> = env
                    .bindings
                    .keys()
                    .chain(env.opaque_bindings.iter())
                    .cloned()
                    .collect();
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                let mut orelse_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut orelse_env,
                    orelse,
                    return_term.clone(),
                )?;
                solver::join_if_branches_solver(env, &body_env, &orelse_env, &pre_existing);
            }
            HirStmt::While { test, body } => {
                // PEP 572 (#774), deep-review follow-up (round 4): bind
                // before unifying -- see the `If` arm's own doc comment
                // just above for why this order is required.
                bind_named_expr_targets(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    test,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    test,
                )?;
                // Issue #359 (Part 2 of #118): clone the environment for
                // the loop body so body-only bindings do not leak into the
                // post-loop environment as definitely bound. A `while` body
                // may execute zero times, so every body-only binding is
                // maybe-bound — mirroring the validation pass's
                // `join_loop_body` (D-147).
                // Issue #771 join-site follow-up: include `opaque_bindings`
                // alongside `bindings` here. A name definitely-but-opaquely
                // bound before this construct must count as pre-existing
                // too -- otherwise a branch that reassigns it to a real,
                // solver-representable term looks "newly introduced" to the
                // join helper below, and when only one branch performs that
                // reassignment the name is misclassified as bound in both
                // branches (the other, untouched branch still carries the
                // opaque marker), unmasking a term that only reflects one
                // path as if it were unconditionally correct. Confirmed as a
                // real gap by the pinned local reviewer's second pass.
                let pre_existing: HashSet<String> = env
                    .bindings
                    .keys()
                    .chain(env.opaque_bindings.iter())
                    .cloned()
                    .collect();
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
            }
            HirStmt::ForRange {
                var,
                start,
                stop,
                step,
                body,
            } => {
                for (position, expr) in [("start", start), ("stop", stop), ("step", step)] {
                    if let Some(term @ Err(_)) = collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        expr,
                    )? {
                        unify_terms(
                            term,
                            Ok(Ty::Int),
                            parents,
                            concrete,
                            "T0021",
                            &format!("range {position}"),
                        )?;
                    }
                }
                // Issue #359 (Part 2 of #118): snapshot the pre-loop
                // binding names so the loop variable and body-only
                // bindings can be tracked as maybe-bound after the loop
                // (a `for` loop may execute zero times).
                // Issue #771 join-site follow-up: include `opaque_bindings`
                // alongside `bindings` here. A name definitely-but-opaquely
                // bound before this construct must count as pre-existing
                // too -- otherwise a branch that reassigns it to a real,
                // solver-representable term looks "newly introduced" to the
                // join helper below, and when only one branch performs that
                // reassignment the name is misclassified as bound in both
                // branches (the other, untouched branch still carries the
                // opaque marker), unmasking a term that only reflects one
                // path as if it were unconditionally correct. Confirmed as a
                // real gap by the pinned local reviewer's second pass.
                let pre_existing: HashSet<String> = env
                    .bindings
                    .keys()
                    .chain(env.opaque_bindings.iter())
                    .cloned()
                    .collect();
                if let Some(existing) = env.bindings.get(var).cloned() {
                    unify_terms(
                        existing,
                        Ok(Ty::Int),
                        parents,
                        concrete,
                        "T0023",
                        &format!("assignment to for-loop target `{var}`"),
                    )?;
                } else {
                    env.bindings.insert(var.clone(), Ok(Ty::Int));
                }
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
                // The loop variable itself is maybe-bound after the loop
                // if it was newly introduced (the loop may not execute).
                if !pre_existing.contains(var) {
                    env.maybe_bindings.insert(var.clone());
                }
            }
            HirStmt::ForList { var, list: _, body } => {
                // Unlike `ForRange`, we have no concrete `Ty::Int` fact to
                // unify the loop variable against -- this solver doesn't
                // track a `list`-typed name's element type at all (see the
                // `HirExpr::ListLiteral`/`Subscript`/`ListAppend` arms
                // above). Give `var` a fresh, unconstrained term so a body
                // reference to it doesn't spuriously fail as "not bound"
                // (it *is* locally bound, just not solver-typed); real
                // element-type checking happens in the second, real check
                // pass (`check_with_signatures`).
                // Issue #359 (Part 2 of #118): snapshot the pre-loop binding
                // names so the loop variable and body-only bindings can be
                // tracked as maybe-bound after the loop (a `for` loop may
                // execute zero times).
                // Issue #771 join-site follow-up: include `opaque_bindings`
                // alongside `bindings` here. A name definitely-but-opaquely
                // bound before this construct must count as pre-existing
                // too -- otherwise a branch that reassigns it to a real,
                // solver-representable term looks "newly introduced" to the
                // join helper below, and when only one branch performs that
                // reassignment the name is misclassified as bound in both
                // branches (the other, untouched branch still carries the
                // opaque marker), unmasking a term that only reflects one
                // path as if it were unconditionally correct. Confirmed as a
                // real gap by the pinned local reviewer's second pass.
                let pre_existing: HashSet<String> = env
                    .bindings
                    .keys()
                    .chain(env.opaque_bindings.iter())
                    .cloned()
                    .collect();
                if !env.bindings.contains_key(var) {
                    let term = fresh_term(parents, concrete);
                    env.bindings.insert(var.clone(), term);
                }
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
                // The loop variable itself is maybe-bound after the loop
                // if it was newly introduced (the loop may not execute).
                if !pre_existing.contains(var) {
                    env.maybe_bindings.insert(var.clone());
                }
            }
            HirStmt::Return(value) => {
                let Some(return_term) = return_term.clone() else {
                    continue;
                };
                let actual = match value {
                    Some(expr) => collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        expr,
                    )?,
                    None => Some(Ok(Ty::None)),
                };
                if let Some(actual) = actual {
                    unify_terms(
                        return_term,
                        actual,
                        parents,
                        concrete,
                        "T0022",
                        "private helper return type",
                    )?;
                }
            }
            // PR-11 Task 3 (D-123): `dict`'s own type isn't tracked by this
            // solver either (same reasoning as `ForList`'s `list` field
            // above) -- recurse into `key`/`value` only to keep propagating
            // genuine errors; real item-assignment type-checking is
            // `check_stmt`/`check_stmt_in_function`'s job.
            HirStmt::DictSet {
                dict: _,
                key,
                value,
            } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    key,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )?;
            }
            // D-154 (Part 1 of #375): `base`'s own type isn't tracked by
            // this solver either (same reasoning as `DictSet` above) --
            // recurse into `base`/`value` only to keep propagating genuine
            // errors; real attribute-slot type-checking is
            // `check_stmt`/`check_stmt_in_function`'s job.
            HirStmt::AttrSet { base, value, .. } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    base,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )?;
            }
            // PR-12 Task 3 (D-117): no unification term is registered for
            // `target` -- per D-116's own correction note ("a
            // container-literal assignment's target never receives a solver
            // binding at all... confirmed empirically for all four container
            // types"), a comprehension's own `target` gets that same
            // treatment, matching `ListLiteral`/`DictLiteral`/`SetLiteral`
            // above. Unlike a bare no-op, though, this arm still binds the
            // loop variable (mirroring `ForRange`/`ForList` above, via the
            // shared `bind_comp_loop_var` helper) and recurses into
            // `cond`/`elt` (or `key`/`value`) to keep propagating genuine
            // errors and, critically, to let a call inside a comprehension's
            // own sub-expressions still participate in this solver's
            // argument<->parameter unification (self-review finding: an
            // earlier no-op-only version of this arm made an unannotated
            // private-helper parameter used only inside a comprehension's
            // `elt` spuriously fail to infer with "cannot infer type of
            // parameter ...; add an annotation", since the call inside `elt`
            // was never visited at all -- see
            // `private_helper_parameter_is_inferred_through_a_comprehension_s_elt`).
            HirStmt::ListCompAssign {
                var,
                iter,
                cond,
                elt,
                ..
            }
            | HirStmt::SetCompAssign {
                var,
                iter,
                cond,
                elt,
                ..
            } => {
                bind_comp_loop_var(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    var,
                    iter,
                )?;
                if let Some(cond) = cond {
                    collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        cond,
                    )?;
                }
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    elt,
                )?;
            }
            HirStmt::DictCompAssign {
                var,
                iter,
                cond,
                key,
                value,
                ..
            } => {
                bind_comp_loop_var(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    var,
                    iter,
                )?;
                if let Some(cond) = cond {
                    collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        cond,
                    )?;
                }
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    key,
                )?;
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    value,
                )?;
            }
            HirStmt::Match { subject, cases } => {
                collect_expr_constraints(
                    signatures,
                    parents,
                    concrete,
                    &mut constraints.binops,
                    env,
                    subject,
                )?;
                for case in cases {
                    // Issue #771 join-site follow-up: include `opaque_bindings`
                // alongside `bindings` here. A name definitely-but-opaquely
                // bound before this construct must count as pre-existing
                // too -- otherwise a branch that reassigns it to a real,
                // solver-representable term looks "newly introduced" to the
                // join helper below, and when only one branch performs that
                // reassignment the name is misclassified as bound in both
                // branches (the other, untouched branch still carries the
                // opaque marker), unmasking a term that only reflects one
                // path as if it were unconditionally correct. Confirmed as a
                // real gap by the pinned local reviewer's second pass.
                let pre_existing: HashSet<String> = env
                    .bindings
                    .keys()
                    .chain(env.opaque_bindings.iter())
                    .cloned()
                    .collect();
                    let mut case_env = env.clone();
                    collect_block_constraints(
                        signatures,
                        parents,
                        concrete,
                        constraints,
                        &mut case_env,
                        &case.body,
                        return_term.clone(),
                    )?;
                    solver::join_loop_body_solver(env, &case_env, &pre_existing);
                }
            }
            HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                // #382 (PR-22 Part 1): collect constraints from the try
                // body, each handler, the else body, and the finally body.
                // The try body's bindings are joined back as `Maybe` (the
                // body may raise before reaching an assignment).
                // Issue #771 join-site follow-up: include `opaque_bindings`
                // alongside `bindings` here. A name definitely-but-opaquely
                // bound before this construct must count as pre-existing
                // too -- otherwise a branch that reassigns it to a real,
                // solver-representable term looks "newly introduced" to the
                // join helper below, and when only one branch performs that
                // reassignment the name is misclassified as bound in both
                // branches (the other, untouched branch still carries the
                // opaque marker), unmasking a term that only reflects one
                // path as if it were unconditionally correct. Confirmed as a
                // real gap by the pinned local reviewer's second pass.
                let pre_existing: HashSet<String> = env
                    .bindings
                    .keys()
                    .chain(env.opaque_bindings.iter())
                    .cloned()
                    .collect();
                let mut body_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut body_env,
                    body,
                    return_term.clone(),
                )?;
                solver::join_loop_body_solver(env, &body_env, &pre_existing);
                for handler in handlers {
                    let mut henv = env.clone();
                    // Bind the `as` name in the handler environment.
                    // Inside the handler body, the binding is definite.
                    if let Some(exc_types) = &handler.exc_type
                        && let Some(name) = &handler.name
                    {
                        let binding_type = pycc_hir::except_handler_binding_type_name(exc_types);
                        henv.bindings
                            .insert(name.clone(), Ok(Ty::Instance(Box::new(binding_type))));
                    }
                    collect_block_constraints(
                        signatures,
                        parents,
                        concrete,
                        constraints,
                        &mut henv,
                        &handler.body,
                        return_term.clone(),
                    )?;
                    solver::join_loop_body_solver(env, &henv, &pre_existing);
                }
                let mut else_env = env.clone();
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    &mut else_env,
                    orelse,
                    return_term.clone(),
                )?;
                solver::join_loop_body_solver(env, &else_env, &pre_existing);
                // The finally body always runs — collect in-place.
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    constraints,
                    env,
                    finalbody,
                    return_term.clone(),
                )?;
            }
            HirStmt::Raise { exc, cause } => {
                // #382 (PR-22 Part 1): A raise expression that is a direct
                // call to a builtin exception class (e.g.
                // `ValueError("msg")`) is classified as C0001 by
                // `collect_expr_constraints`: that collector resolves a
                // callee against the `signatures` map it was handed, which
                // holds only `HirItem::Function` entries, so a class name
                // misses and falls through to the known-callable-builtin
                // arm. Part 1 of #541 seeded the seven builtin exception
                // names into `Environment::classes`, but that table is not
                // what this collector consults, so the classification is
                // unchanged. The actual validation is done by
                // `check_raise_stmt` in the check pass, so errors from
                // constraint collection for raise operands are deliberately
                // ignored here — they would otherwise prevent the solver
                // path from reaching `check_with_signatures`, where the
                // real check succeeds.
                if let Some(exc_expr) = exc {
                    let _ = collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        exc_expr,
                    );
                }
                if let Some(cause_expr) = cause {
                    let _ = collect_expr_constraints(
                        signatures,
                        parents,
                        concrete,
                        &mut constraints.binops,
                        env,
                        cause_expr,
                    );
                }
            }
        }
    }
    Ok(())
}

fn propagate_binop_constraints(
    binops: &[BinOpConstraint],
    parents: &mut [usize],
    concrete: &mut [Option<Ty>],
) -> Result<(), Diagnostic> {
    loop {
        let mut changed = false;
        for &(op, ref left_term, ref right_term, ref result_term) in binops {
            let left = resolved_term(left_term.clone(), parents, concrete);
            let right = resolved_term(right_term.clone(), parents, concrete);
            let result = resolved_term(result_term.clone(), parents, concrete);
            if let (Some(left), Some(right)) = (left, right) {
                let result_ty = numeric_result_type(op, left, right)?;
                changed |= unify_terms(
                    result_term.clone(),
                    Ok(result_ty),
                    parents,
                    concrete,
                    "T0021",
                    "binary expression",
                )?;
                continue;
            }

            // Propagate constraints backward when the result determines a
            // unique operand representation. In particular, an annotated
            // `int` result for a non-division binary expression rules out
            // floats and strings, so unresolved operands are int-like and
            // use the merged `int` representation. This makes
            // `def _inc(x) -> int: return x + 1` infer `x: int` without a
            // call-site constraint (D-045).
            if result == Some(Ty::Int) && op != BinOpKind::Div {
                let left_changed = unify_terms(
                    left_term.clone(),
                    Ok(Ty::Int),
                    parents,
                    concrete,
                    "T0021",
                    "left operand of int binary expression",
                )?;
                let right_changed = unify_terms(
                    right_term.clone(),
                    Ok(Ty::Int),
                    parents,
                    concrete,
                    "T0021",
                    "right operand of int binary expression",
                )?;
                changed |= left_changed || right_changed;
            }
        }
        if !changed {
            return Ok(());
        }
    }
}

fn apply_annotation_defaults(
    constraints: &[AnnotationDefaultConstraint],
    parents: &mut [usize],
    concrete: &mut [Option<Ty>],
) -> Result<(), Diagnostic> {
    let mut bounds_by_root = vec![Vec::new(); parents.len()];
    for constraint in constraints {
        let Err(var) = &constraint.initializer else {
            continue;
        };
        if !is_private_solver_scalar(&constraint.annotation) {
            // Private-helper inference is deliberately scalar-only. A
            // hand-built HIR module must not use an annotated local to leak a
            // container type into an otherwise-unresolved signature.
            continue;
        }
        let root = root(parents, *var);
        if concrete[root].is_none() {
            bounds_by_root[root].push(constraint.annotation.clone());
        }
    }

    for (root, bounds) in bounds_by_root.iter_mut().enumerate() {
        if bounds.is_empty() {
            continue;
        }
        bounds.sort_by_key(Ty::name);
        bounds.dedup();
        let fallback = bounds.iter().find(|candidate| {
            bounds
                .iter()
                .all(|bound| is_assignable((*candidate).clone(), bound.clone()))
        });
        let Some(fallback) = fallback else {
            let bounds = bounds
                .iter()
                .map(|bound| format!("`{}`", bound.name()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Diagnostic::error(
                "T0021",
                format!("annotated initializer has incompatible directional constraints: {bounds}"),
                Span::new(0, 0),
            ));
        };
        // This root was deliberately selected only while unresolved, and
        // defaults never union roots. Install the chosen directional fallback
        // directly instead of routing it through symmetric type merging.
        concrete[root] = Some(fallback.clone());
    }
    Ok(())
}

pub(crate) fn contains_return(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Return(_) => true,
        HirStmt::If { body, orelse, .. } => contains_return(body) || contains_return(orelse),
        HirStmt::While { body, .. }
        | HirStmt::ForRange { body, .. }
        | HirStmt::ForList { body, .. } => contains_return(body),
        HirStmt::Match { cases, .. } => cases.iter().any(|case| contains_return(&case.body)),
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. }
        | HirStmt::Raise { .. } => false,
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            contains_return(body)
                || handlers.iter().any(|h| contains_return(&h.body))
                || contains_return(orelse)
                || contains_return(finalbody)
        }
    })
}

/// Issue #118 Part 1: returns true if any statement in `body` introduces
/// a new binding (assignment, annotated assignment, comprehension assignment,
/// or dict/set item assignment, including nested inside if/while/for). Used
/// to skip the expensive `env.clone()` + `join_if_branches` path when neither
/// branch of an `if` assigns anything -- the common case for guard-only ifs.
pub(crate) fn introduces_bindings(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::AttrSet { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. } => true,
        HirStmt::If { body, orelse, .. } => {
            introduces_bindings(body) || introduces_bindings(orelse)
        }
        HirStmt::While { body, .. } => introduces_bindings(body),
        HirStmt::ForRange { body, .. } => introduces_bindings(body),
        HirStmt::ForList { body, .. } => introduces_bindings(body),
        HirStmt::Match { cases, .. } => cases.iter().any(|case| introduces_bindings(&case.body)),
        HirStmt::Return(_) | HirStmt::ExprStmt(_) => false,
        HirStmt::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            introduces_bindings(body)
                || handlers.iter().any(|h| introduces_bindings(&h.body))
                || introduces_bindings(orelse)
                || introduces_bindings(finalbody)
        }
        HirStmt::Raise { .. } => false,
    })
}

pub(crate) fn concrete_function_signatures(
    hir: &HirModule,
) -> Option<HashMap<String, (Vec<Ty>, Ty)>> {
    let mut signatures = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if *return_ty == Ty::Infer || params.iter().any(|(_, ty)| *ty == Ty::Infer) {
            return None;
        }
        signatures.insert(
            name.clone(),
            (
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_ty.clone(),
            ),
        );
    }
    Some(signatures)
}

/// Builds the function registry for a fully annotated module directly from
/// HIR. Unlike [`concrete_function_signatures`] followed by
/// [`check_with_signatures`], this creates each owned name and parameter vector
/// only once. `check` does not need to materialize a second signature map for a
/// downstream consumer, so its overwhelmingly common concrete, valid path can
/// validate with this registry directly.
pub(crate) fn concrete_function_environment(hir: &HirModule) -> Option<Environment> {
    let mut functions = HashMap::new();
    let mut generics = HashMap::new();
    for item in &hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if *return_ty == Ty::Infer || params.iter().any(|(_, ty)| *ty == Ty::Infer) {
            return None;
        }
        if is_generic_signature(params, return_ty) {
            generics.insert(name.clone(), item.clone());
        }
        functions.insert(
            name.clone(),
            (
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_ty.clone(),
            ),
        );
    }
    let mut env = Environment {
        bindings: HashMap::new(),
        declared: HashMap::new(),
        functions: Arc::new(functions),
        def_rebound: HashSet::new(),
        defined_functions: HashSet::new(),
        generics: Arc::new(generics),
        classes: Arc::new(HashMap::new()),
        synthetic_classes: Arc::new(HashSet::new()),
        own_type_param: None,
        current_class: None,
        finals: HashSet::new(),
        in_except_handler: false,
    };
    // Part 1 of #541: register the class table through `bind_class` (via
    // `bind_classes`) rather than by populating `classes` directly, so this
    // second `Environment` constructor cannot drift from the first on which
    // entries are marked synthetic. `bind_class` and `bind_synthetic_class`
    // are together the sole mutators of both tables precisely so that
    // invariant holds by construction.
    crate::class::bind_classes(&mut env, hir);
    Some(env)
}

pub(crate) fn infer_function_signatures_with_solver(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
    let mut parents = Vec::new();
    let mut concrete = Vec::new();
    let mut signatures = HashMap::new();
    for item in &hir.items {
        if let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        {
            signatures.insert(
                name.clone(),
                (
                    params.iter().map(|(name, _)| name.clone()).collect(),
                    params
                        .iter()
                        .map(|(_, ty)| term_for_type(ty.clone(), &mut parents, &mut concrete))
                        .collect(),
                    term_for_type(return_ty.clone(), &mut parents, &mut concrete),
                ),
            );
        }
    }

    let mut constraints = SolverConstraints::default();
    let mut globals = ConstraintEnvironment {
        bindings: HashMap::new(),
        local_names: &[],
        defs_rebound: HashSet::new(),
        maybe_bindings: HashSet::new(),
        opaque_bindings: HashSet::new(),
    };
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                collect_block_constraints(
                    &signatures,
                    &mut parents,
                    &mut concrete,
                    &mut constraints,
                    &mut globals,
                    std::slice::from_ref(stmt),
                    None,
                )?;
            }
            // Mirror of pass 2's source-order `def` rebinding (D-110): the
            // `def` marks the name def-rebound in the accumulated globals
            // (without erasing its term, which representation tracking may
            // still need), so helper-body environments seeded from them see
            // the net binding, not a stale shadowed primitive.
            HirItem::Function { name, .. } => {
                globals.defs_rebound.insert(name.clone());
            }
        }
    }
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        let HirItem::Function {
            name, body, params, ..
        } = item
        else {
            continue;
        };
        let signature = &signatures[name];
        let mut env = ConstraintEnvironment {
            bindings: globals.bindings.clone(),
            local_names,
            defs_rebound: globals.defs_rebound.clone(),
            maybe_bindings: globals.maybe_bindings.clone(),
            opaque_bindings: globals.opaque_bindings.clone(),
        };
        for local_name in local_names.iter().copied() {
            env.bindings.remove(local_name);
            // A local name (parameter or body-assigned) re-binds within this
            // body, so a stale module-level def-rebound fact must not
            // survive for it (D-110, PR #252's round-6 review): a parameter
            // colliding with a def-rebound module name would otherwise skip
            // the mirror gate and be mislabeled "not bound before this use".
            env.defs_rebound.remove(local_name);
            env.maybe_bindings.remove(local_name);
            // Issue #771: same reasoning as `maybe_bindings` above — a
            // local name re-binds within this function body, so a stale
            // module-level opaque marker must not survive for it either.
            env.opaque_bindings.remove(local_name);
        }
        // Use the current item's own parameter names, not the last-inserted
        // signature's names (#386): a redefined method shares its mangled
        // name but has its own parameter names, and checking its body against
        // the wrong names would report false T0021 "not bound" errors. The
        // type terms (signature.1) and return type (signature.2) come from
        // the last definition, which is correct — compatible redefinitions
        // have the same raw type shape (already validated by
        // check_incompatible_redefinitions), and the last definition is the
        // one bound at call sites.
        for (param_name, param_ty) in params.iter().map(|(n, _)| n).zip(&signature.1) {
            env.bindings.insert(param_name.clone(), param_ty.clone());
        }
        // #380 (PR-20): skip the constraint solver for abstract method
        // bodies. An abstract method's HIR body is just `Return(None)`,
        // but its declared return type may be non-`None` (e.g. `-> int`).
        // Running the solver on it would unify `None` with the declared
        // type and produce a spurious `T0022`. The type checker
        // (`check_and_resolve`) also skips abstract method bodies.
        let is_abstract_method = name
            .split('.')
            .next()
            .filter(|class_name| *class_name != name)
            .and_then(|class_name| {
                hir.class_defs
                    .iter()
                    .find(|(n, _)| n == class_name)
                    .map(|(_, cd)| cd)
            })
            .is_some_and(|class_def| {
                let method_name = name.split('.').nth(1).unwrap_or("");
                class_def.abstract_methods.iter().any(|m| m == method_name)
            });
        if is_abstract_method {
            continue;
        }
        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut constraints,
            &mut env,
            body,
            Some(signature.2.clone()),
        )?;
        if signature.2.is_err() && !contains_return(body) {
            unify_terms(
                signature.2.clone(),
                Ok(Ty::None),
                &mut parents,
                &mut concrete,
                "T0022",
                "private helper implicit return",
            )?;
        }
    }

    // Annotation bounds are directional defaults, not hard equalities. Let
    // every call/operator fact settle first, aggregate all remaining bounds
    // per union-find root, then propagate any selected fallback back through
    // operators. This keeps inference independent of body/declaration order.
    propagate_binop_constraints(&constraints.binops, &mut parents, &mut concrete)?;
    apply_annotation_defaults(
        &constraints.annotation_defaults,
        &mut parents,
        &mut concrete,
    )?;
    propagate_binop_constraints(&constraints.binops, &mut parents, &mut concrete)?;

    let non_scalar_local_roots = constraints
        .non_scalar_local_terms
        .iter()
        .map(|&var| root(&mut parents, var))
        .collect::<HashSet<_>>();

    let mut resolved = HashMap::new();
    for (name, signature) in &signatures {
        let param_tys = signature
            .0
            .iter()
            .zip(signature.1.iter().cloned())
            .map(|(param_name, term)| {
                resolved_private_signature_term(
                    term,
                    &mut parents,
                    &concrete,
                    &non_scalar_local_roots,
                )
                .ok_or_else(|| {
                    Diagnostic::error(
                        "T0021",
                        format!(
                            "cannot infer type of parameter `{param_name}` in private helper `{name}`; add an annotation"
                        ),
                        Span::new(0, 0),
                    ).with_help(format!("add a type annotation to parameter `{param_name}`"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = resolved_private_signature_term(
            signature.2.clone(),
            &mut parents,
            &concrete,
            &non_scalar_local_roots,
        )
        .ok_or_else(|| {
            Diagnostic::error(
                "T0021",
                format!("cannot infer return type of private helper `{name}`; add an annotation"),
                Span::new(0, 0),
            )
            .with_help(format!("add a return type annotation to `{name}`"))
        })?;
        resolved.insert(name.clone(), (param_tys, return_ty));
    }
    Ok(resolved)
}

pub(crate) fn checked_function_signatures(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
    // Issue #22: reject incompatible redefinitions before trying either the
    // concrete or solver path (same rationale as `check`'s own call).
    check_incompatible_redefinitions(hir)?;
    // Fully annotated valid modules have no inference variables to constrain.
    // Validate them once and avoid the preceding constraint-collection walk.
    // If validation fails, deliberately fall back to the historical
    // solver-first sequence so modules with multiple errors retain the same
    // first diagnostic as before this fast path existed.
    if let Some(signatures) = concrete_function_signatures(hir)
        && check_with_signatures(hir, &signatures, function_local_names).is_ok()
    {
        return Ok(signatures);
    }

    let signatures = infer_function_signatures_with_solver(hir, function_local_names)?;
    check_with_signatures(hir, &signatures, function_local_names)?;
    Ok(signatures)
}
