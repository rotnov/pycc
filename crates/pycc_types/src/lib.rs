use pycc_diag::{Diagnostic, Span};
#[cfg(test)]
use pycc_hir::CmpOpKind;
pub use pycc_hir::Ty;
use pycc_hir::{BinOpKind, CompIter, FStringPart, HirExpr, HirItem, HirModule, HirStmt};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub struct Environment {
    bindings: HashMap<String, Ty>,
    functions: Arc<HashMap<String, (Vec<Ty>, Ty)>>,
    /// Names whose *net* source-order top-level binding is currently a
    /// `def` (D-110). Tracked separately from `bindings` on purpose: the
    /// representation record in `bindings` must survive a `def` so that
    /// D-040's sticky-representation rule (`check_assignment`) still rejects
    /// a later incompatible value reassignment -- PR #252's round-4 review
    /// caught a version that cleared `bindings` at a `def` and thereby let
    /// `helper = 1; def helper(): ...; helper = "leaked"` reach codegen with
    /// an `int`-allocated slot stored as `str`.
    def_rebound: HashSet<String>,
}

impl Environment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, name: &str) -> Option<Ty> {
        self.bindings.get(name).cloned()
    }

    pub fn bind(&mut self, name: String, ty: Ty) {
        // A value assignment makes the name's net binding a value again,
        // whatever `def`s came before it (D-110).
        self.def_rebound.remove(name.as_str());
        self.bindings.insert(name, ty);
    }

    pub fn bind_function(&mut self, name: String, param_tys: Vec<Ty>, return_ty: Ty) {
        Arc::make_mut(&mut self.functions).insert(name, (param_tys, return_ty));
    }

    pub fn lookup_function(&self, name: &str) -> Option<&(Vec<Ty>, Ty)> {
        self.functions.get(name)
    }

    fn child_for_function(&self, local_names: &[&str]) -> Self {
        let mut child = self.clone();
        for name in local_names {
            child.bindings.remove(*name);
        }
        child
    }
}

fn unbound_local(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("local name `{name}` is not bound before this use"),
        Span::new(0, 0),
    )
}

fn non_callable_binding(name: &str) -> Diagnostic {
    Diagnostic::error(
        "T0021",
        format!("name `{name}` is bound to a non-callable value"),
        Span::new(0, 0),
    )
}

/// Looks up a bare name's already-bound type, producing the same
/// "unbound local" vs. "not defined" distinction `HirExpr::Name` itself
/// uses in `infer_expr_in` below. `HirStmt::ForList`'s `list` field and
/// `HirExpr::ListAppend`'s `list` field are both plain `String`s rather
/// than `HirExpr::Name` nodes (D-105's HIR shape), so they can't go
/// through `infer_expr_in`'s own `Name` arm and need this helper instead.
fn lookup_bound_name(
    env: &Environment,
    local_names: &[&str],
    name: &str,
) -> Result<Ty, Diagnostic> {
    env.lookup(name).ok_or_else(|| {
        if is_local(local_names, name) {
            unbound_local(name)
        } else {
            Diagnostic::error(
                "T0021",
                format!("name `{name}` is not defined"),
                Span::new(0, 0),
            )
        }
    })
}

fn function_local_names<'a>(params: &'a [(String, Ty)], body: &'a [HirStmt]) -> Vec<&'a str> {
    let mut names = params
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    collect_local_names(body, &mut names);
    names
}

fn collect_local_names<'a>(body: &'a [HirStmt], names: &mut Vec<&'a str>) {
    for stmt in body {
        match stmt {
            HirStmt::Assign { target, .. } => {
                if !is_local(names, target) {
                    names.push(target);
                }
            }
            HirStmt::AnnAssign { target, .. } => {
                if !is_local(names, target) {
                    names.push(target);
                }
            }
            HirStmt::If { body, orelse, .. } => {
                collect_local_names(body, names);
                collect_local_names(orelse, names);
            }
            HirStmt::While { body, .. } => collect_local_names(body, names),
            HirStmt::ForRange { var, body, .. } => {
                if !is_local(names, var) {
                    names.push(var);
                }
                collect_local_names(body, names);
            }
            HirStmt::ForList { var, body, .. } => {
                if !is_local(names, var) {
                    names.push(var);
                }
                collect_local_names(body, names);
            }
            // PR-12 Task 3 (D-117): a comprehension introduces two new local
            // names where a plain `for` loop introduces one -- its own
            // `target` (the comprehension's result) and its synthesized
            // `var` (the loop variable, already collision-proof by
            // construction, see `pycc_hir`'s `synthesize_comp_var_name`).
            // Neither has a body to recurse into (a comprehension is not a
            // nested block of statements).
            HirStmt::ListCompAssign { target, var, .. }
            | HirStmt::SetCompAssign { target, var, .. }
            | HirStmt::DictCompAssign { target, var, .. } => {
                if !is_local(names, target) {
                    names.push(target);
                }
                if !is_local(names, var) {
                    names.push(var);
                }
            }
            // `d[k] = v` (PR-11 Task 3) reassigns an existing binding's
            // contents, not a name -- unlike `Assign`/`AnnAssign`/`ForList`
            // above, it introduces no new local name to collect.
            HirStmt::ExprStmt(_) | HirStmt::Return(_) | HirStmt::DictSet { .. } => {}
        }
    }
}

fn module_function_local_names(hir: &HirModule) -> Vec<Vec<&str>> {
    hir.items
        .iter()
        .map(|item| match item {
            HirItem::Function { params, body, .. } => function_local_names(params, body),
            HirItem::TopLevelStmt(_) => Vec::new(),
        })
        .collect()
}

fn is_local(local_names: &[&str], name: &str) -> bool {
    local_names.contains(&name)
}

type TypeTerm = Result<Ty, usize>;
type SignatureTerms = (Vec<String>, Vec<TypeTerm>, TypeTerm);
type BinOpConstraint = (BinOpKind, TypeTerm, TypeTerm, TypeTerm);

#[derive(Debug)]
struct ConstraintEnvironment<'scope, 'hir> {
    bindings: HashMap<String, TypeTerm>,
    local_names: &'scope [&'hir str],
    /// Mirror of `Environment::def_rebound` (D-110): names whose net
    /// source-order module binding is a `def`, kept apart from the term
    /// bindings for the same reason -- terms must survive a `def` for
    /// representation purposes.
    defs_rebound: HashSet<String>,
}

fn fresh_term(parents: &mut Vec<usize>, concrete: &mut Vec<Option<Ty>>) -> TypeTerm {
    let id = parents.len();
    parents.push(id);
    concrete.push(None);
    Err(id)
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

fn resolved_term(term: TypeTerm, parents: &mut [usize], concrete: &[Option<Ty>]) -> Option<Ty> {
    match term {
        Ok(ty) => Some(ty),
        Err(var) => concrete[root(parents, var)].clone(),
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

fn unify_terms(
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

fn collect_expr_constraints(
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
        HirExpr::Name(name) => match env.bindings.get(name).cloned() {
            Some(term) => Ok(Some(term)),
            None if is_local(env.local_names, name) => Err(unbound_local(name)),
            None => Ok(None),
        },
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
                    ));
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
                    ));
                }
                return Ok(Some(Ok(Ty::Int)));
            }
            let Some(signature) = signatures.get(callee) else {
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
        // `TypeTerm` (`Result<Ty, usize>`) has no unification-friendly
        // representation for `Ty::List` -- this solver only exists to infer
        // scalar `Ty::Infer` parameters/returns of underscore-prefixed
        // private helpers (D-045), and list homogeneity/element-type
        // checking is `infer_expr_in`'s job, not this constraint collector's
        // (see that function's `HirExpr::ListLiteral`/`Subscript`/
        // `ListAppend` arms below). Recurse only to keep propagating
        // genuine errors (e.g. an unbound local used as a list element) and
        // otherwise report "no term produced," exactly like an unresolved
        // `Name` or a call to an unregistered function above -- returning
        // `Err` here for a case this solver can't actually validate would
        // wrongly preempt `checked_function_signatures`' fallback to the
        // real, list-aware check pass (`check_with_signatures`) that runs
        // after this solver.
        HirExpr::ListLiteral(elements) => {
            for element in elements {
                collect_expr_constraints(signatures, parents, concrete, binops, env, element)?;
            }
            Ok(None)
        }
        HirExpr::Subscript { base, index } => {
            collect_expr_constraints(signatures, parents, concrete, binops, env, base)?;
            collect_expr_constraints(signatures, parents, concrete, binops, env, index)?;
            Ok(None)
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
        // recurse into for `ListPop`.
        HirExpr::ListPop { list: _ } => Ok(None),
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

fn collect_block_constraints(
    signatures: &HashMap<String, SignatureTerms>,
    parents: &mut Vec<usize>,
    concrete: &mut Vec<Option<Ty>>,
    binops: &mut Vec<BinOpConstraint>,
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
                if let Some(term) =
                    collect_expr_constraints(signatures, parents, concrete, binops, env, value)?
                {
                    env.bindings.entry(target.clone()).or_insert(term);
                }
            }
            HirStmt::AnnAssign {
                target,
                value: Some(value),
                annotation: _,
            } => {
                // KNOWN GAP (PR-9 Task 4 review, 2026-07-30): binds the solver term to the
                // initializer's own inferred term, discarding `annotation` entirely. A private
                // helper mixing type inference with an annotated local whose annotation diverges
                // from the naturally-inferred term (e.g. `def _h(x): y: int = x; return y` called
                // as `_h(True)`) can get a spurious T0022 rejection of otherwise-legal Python --
                // not a soundness hole (nothing incorrect is silently accepted), and confirmed
                // unreachable by any fixture PR-9 itself ships (this path only runs for
                // underscore-prefixed private helpers with Ty::Infer signatures). Cheap fix if
                // ever needed: unify the term with `Ok(*annotation)` instead of discarding it.
                if let Some(term) =
                    collect_expr_constraints(signatures, parents, concrete, binops, env, value)?
                {
                    env.bindings.entry(target.clone()).or_insert(term);
                }
            }
            HirStmt::AnnAssign { value: None, .. } => {}
            HirStmt::ExprStmt(expr) => {
                collect_expr_constraints(signatures, parents, concrete, binops, env, expr)?;
            }
            HirStmt::If { test, body, orelse } => {
                collect_expr_constraints(signatures, parents, concrete, binops, env, test)?;
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    binops,
                    env,
                    body,
                    return_term.clone(),
                )?;
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    binops,
                    env,
                    orelse,
                    return_term.clone(),
                )?;
            }
            HirStmt::While { test, body } => {
                collect_expr_constraints(signatures, parents, concrete, binops, env, test)?;
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    binops,
                    env,
                    body,
                    return_term.clone(),
                )?;
            }
            HirStmt::ForRange {
                var,
                start,
                stop,
                step,
                body,
            } => {
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
                            &format!("range {position}"),
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
                        &format!("assignment to for-loop target `{var}`"),
                    )?;
                } else {
                    env.bindings.insert(var.clone(), Ok(Ty::Int));
                }
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    binops,
                    env,
                    body,
                    return_term.clone(),
                )?;
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
                if !env.bindings.contains_key(var) {
                    let term = fresh_term(parents, concrete);
                    env.bindings.insert(var.clone(), term);
                }
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    binops,
                    env,
                    body,
                    return_term.clone(),
                )?;
            }
            HirStmt::Return(value) => {
                let Some(return_term) = return_term.clone() else {
                    continue;
                };
                let actual = match value {
                    Some(expr) => {
                        collect_expr_constraints(signatures, parents, concrete, binops, env, expr)?
                    }
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
                collect_expr_constraints(signatures, parents, concrete, binops, env, key)?;
                collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
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
                bind_comp_loop_var(signatures, parents, concrete, binops, env, var, iter)?;
                if let Some(cond) = cond {
                    collect_expr_constraints(signatures, parents, concrete, binops, env, cond)?;
                }
                collect_expr_constraints(signatures, parents, concrete, binops, env, elt)?;
            }
            HirStmt::DictCompAssign {
                var,
                iter,
                cond,
                key,
                value,
                ..
            } => {
                bind_comp_loop_var(signatures, parents, concrete, binops, env, var, iter)?;
                if let Some(cond) = cond {
                    collect_expr_constraints(signatures, parents, concrete, binops, env, cond)?;
                }
                collect_expr_constraints(signatures, parents, concrete, binops, env, key)?;
                collect_expr_constraints(signatures, parents, concrete, binops, env, value)?;
            }
        }
    }
    Ok(())
}

fn contains_return(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Return(_) => true,
        HirStmt::If { body, orelse, .. } => contains_return(body) || contains_return(orelse),
        HirStmt::While { body, .. }
        | HirStmt::ForRange { body, .. }
        | HirStmt::ForList { body, .. } => contains_return(body),
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::DictSet { .. }
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. } => false,
    })
}

fn concrete_function_signatures(hir: &HirModule) -> Option<HashMap<String, (Vec<Ty>, Ty)>> {
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
            (params.iter().map(|(_, ty)| ty.clone()).collect(), return_ty.clone()),
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
fn concrete_function_environment(hir: &HirModule) -> Option<Environment> {
    let mut functions = HashMap::new();
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
        functions.insert(
            name.clone(),
            (params.iter().map(|(_, ty)| ty.clone()).collect(), return_ty.clone()),
        );
    }
    Some(Environment {
        bindings: HashMap::new(),
        functions: Arc::new(functions),
        def_rebound: HashSet::new(),
    })
}

fn infer_function_signatures_with_solver(
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

    let mut binops = Vec::new();
    let mut globals = ConstraintEnvironment {
        bindings: HashMap::new(),
        local_names: &[],
        defs_rebound: HashSet::new(),
    };
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => {
                collect_block_constraints(
                    &signatures,
                    &mut parents,
                    &mut concrete,
                    &mut binops,
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
        let HirItem::Function { name, body, .. } = item else {
            continue;
        };
        let signature = &signatures[name];
        let mut env = ConstraintEnvironment {
            bindings: globals.bindings.clone(),
            local_names,
            defs_rebound: globals.defs_rebound.clone(),
        };
        for local_name in local_names.iter().copied() {
            env.bindings.remove(local_name);
            // A local name (parameter or body-assigned) re-binds within this
            // body, so a stale module-level def-rebound fact must not
            // survive for it (D-110, PR #252's round-6 review): a parameter
            // colliding with a def-rebound module name would otherwise skip
            // the mirror gate and be mislabeled "not bound before this use".
            env.defs_rebound.remove(local_name);
        }
        for (param_name, param_ty) in signature.0.iter().zip(&signature.1) {
            env.bindings.insert(param_name.clone(), param_ty.clone());
        }
        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
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

    loop {
        let mut changed = false;
        for &(op, ref left_term, ref right_term, ref result_term) in &binops {
            let left = resolved_term(left_term.clone(), &mut parents, &concrete);
            let right = resolved_term(right_term.clone(), &mut parents, &concrete);
            let result = resolved_term(result_term.clone(), &mut parents, &concrete);
            if let (Some(left), Some(right)) = (left, right) {
                let result_ty = numeric_result_type(op, left, right)?;
                changed |= unify_terms(
                    result_term.clone(),
                    Ok(result_ty),
                    &mut parents,
                    &mut concrete,
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
                    &mut parents,
                    &mut concrete,
                    "T0021",
                    "left operand of int binary expression",
                )?;
                let right_changed = unify_terms(
                    right_term.clone(),
                    Ok(Ty::Int),
                    &mut parents,
                    &mut concrete,
                    "T0021",
                    "right operand of int binary expression",
                )?;
                changed |= left_changed || right_changed;
            }
        }
        if !changed {
            break;
        }
    }

    let mut resolved = HashMap::new();
    for (name, signature) in &signatures {
        let param_tys = signature
            .0
            .iter()
            .zip(signature.1.iter().cloned())
            .map(|(param_name, term)| {
                resolved_term(term, &mut parents, &concrete).ok_or_else(|| {
                    Diagnostic::error(
                        "T0021",
                        format!(
                            "cannot infer type of parameter `{param_name}` in private helper `{name}`; add an annotation"
                        ),
                        Span::new(0, 0),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = resolved_term(signature.2.clone(), &mut parents, &concrete).ok_or_else(|| {
            Diagnostic::error(
                "T0021",
                format!("cannot infer return type of private helper `{name}`; add an annotation"),
                Span::new(0, 0),
            )
        })?;
        resolved.insert(name.clone(), (param_tys, return_ty));
    }
    Ok(resolved)
}

fn checked_function_signatures(
    hir: &HirModule,
    function_local_names: &[Vec<&str>],
) -> Result<HashMap<String, (Vec<Ty>, Ty)>, Diagnostic> {
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

pub fn infer_expr(env: &Environment, expr: &HirExpr) -> Result<Ty, Diagnostic> {
    infer_expr_in(env, &[], expr)
}

fn infer_expr_in(
    env: &Environment,
    local_names: &[&str],
    expr: &HirExpr,
) -> Result<Ty, Diagnostic> {
    match expr {
        HirExpr::IntLiteral(_) => Ok(Ty::Int),
        HirExpr::FloatLiteral(_) => Ok(Ty::Float),
        HirExpr::BoolLiteral(_) => Ok(Ty::Bool),
        HirExpr::StringLiteral(_) => Ok(Ty::Str),
        HirExpr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation(expr) = part {
                    infer_expr_in(env, local_names, expr)?; // any interpolatable type is allowed; Python str()-coerces at runtime
                }
            }
            Ok(Ty::Str)
        }
        HirExpr::Name(name) => env.lookup(name).ok_or_else(|| {
            if is_local(local_names, name) {
                unbound_local(name)
            } else {
                Diagnostic::error(
                    "T0021",
                    format!("name `{name}` is not defined"),
                    Span::new(0, 0), // real span threading through HIR is out of scope for this task -- see Task 15's follow-up note
                )
            }
        }),
        HirExpr::BinOp { op, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            numeric_result_type(*op, left_ty, right_ty)
        }
        HirExpr::Compare { op: _, left, right } => {
            let left_ty = infer_expr_in(env, local_names, left)?;
            let right_ty = infer_expr_in(env, local_names, right)?;
            if numeric_or_bool_compatible(left_ty.clone(), right_ty.clone()) {
                Ok(Ty::Bool)
            } else {
                Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot compare `{}` and `{}`",
                        left_ty.name(),
                        right_ty.name()
                    ),
                    Span::new(0, 0),
                ))
            }
        }
        HirExpr::Call { callee, args } => {
            // D-110 (#133): a call target resolves through the active value
            // binding before any builtin or function-registry fallback -- a
            // module `helper = 1` shadows both a same-named `def` and a
            // builtin at every later call site, and every value binding in
            // the current subset is a primitive, so a shadowed target is
            // always non-callable. The gate is deliberately callee-first
            // (before argument inference), uniform with how the local gate
            // below always behaved. Local diagnostics are preserved exactly:
            // a value-bound local reported `non_callable_binding` before this
            // reordering too, and a local without a binding still falls
            // through to `unbound_local`. In pass 3 the environment is the
            // final module environment (D-041), so a body call is rejected
            // when the callee is value-bound anywhere at top level -- D-110
            // records that consequence (it rejects some later-rebind programs
            // CPython's dynamic order would run) as deliberate; source-order
            // visibility questions stay #22's scope.
            if env.lookup(callee).is_some() && !env.def_rebound.contains(callee) {
                return Err(non_callable_binding(callee));
            }
            if is_local(local_names, callee) {
                return Err(unbound_local(callee));
            }
            // For a callee that survives D-110's binding gate above, preserve
            // the established diagnostic order by inferring every
            // argument before validating arity or compatibility. Most Python
            // calls are small, so keep up to four inferred types on the stack
            // and reserve a heap vector only for wider calls.
            const INLINE_ARG_TYPES: usize = 4;
            let mut inline_arg_tys = [const { Ty::Infer }; INLINE_ARG_TYPES];
            let heap_arg_tys;
            let arg_tys: &[Ty] = if args.len() <= INLINE_ARG_TYPES {
                for (slot, arg) in inline_arg_tys.iter_mut().zip(args) {
                    *slot = infer_expr_in(env, local_names, arg)?;
                }
                &inline_arg_tys[..args.len()]
            } else {
                heap_arg_tys = args
                    .iter()
                    .map(|arg| infer_expr_in(env, local_names, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                &heap_arg_tys
            };
            if callee == "print" {
                return Ok(Ty::None); // print's own signature isn't user-declarable in v0.1
            }
            if callee == "len" {
                // D-105 point 3: `len(lst)` is a hand-recognized builtin
                // call, same as `print` above -- not a user-declarable
                // signature. Generic over any scalar element type (`T0034`
                // already gates non-`int` lists further upstream, at the
                // point a list literal is constructed), reusing T0033 for
                // both failure shapes (wrong arity, non-list argument) --
                // the same "value does not support list operations" shape
                // already established for `ForList`/`Subscript`/`ListAppend`.
                // PR-11 Task 3 (D-123): also accepts `Ty::Dict` (gated the
                // same way by `T0036`), so `len(d)` type-checks too. PR-11
                // Task 7 (D-123): also accepts `Ty::Set` (gated the same way
                // by `T0038`), so `len(s)` type-checks too.
                if arg_tys.len() != 1 {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!("`len` expects exactly 1 argument, got {}", arg_tys.len()),
                        Span::new(0, 0),
                    ));
                }
                if !matches!(arg_tys[0], Ty::List(_) | Ty::Dict(_) | Ty::Set(_)) {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `{}`",
                            arg_tys[0].name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                return Ok(Ty::Int);
            }
            let Some((param_tys, return_ty)) = env.lookup_function(callee) else {
                return Err(Diagnostic::error(
                    "T0021",
                    format!("call to undefined function `{callee}`"),
                    Span::new(0, 0),
                ));
            };
            if arg_tys.len() != param_tys.len() {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "`{callee}` expects {} argument(s), got {}",
                        param_tys.len(),
                        arg_tys.len()
                    ),
                    Span::new(0, 0),
                ));
            }
            for (i, (arg_ty, param_ty)) in arg_tys.iter().zip(param_tys.iter()).enumerate() {
                if !is_assignable(arg_ty.clone(), param_ty.clone()) {
                    return Err(Diagnostic::error(
                        "T0021",
                        format!(
                            "argument {} of `{callee}` expects `{}`, got `{}`",
                            i + 1,
                            param_ty.name(),
                            arg_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
            Ok(return_ty.clone())
        }
        HirExpr::ListLiteral(elements) => {
            // D-105: an empty list literal has no element to infer a type
            // from, and v0.2 has no `list[T]` annotation syntax to recover
            // it from instead -- reject plainly rather than letting
            // `Ty::Infer` leak into codegen. Reuses T0021 (an unconstrained
            // variable with no way to determine its type), not a new code
            // -- this is that same failure shape, not a distinct one.
            if elements.is_empty() {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty list literal's element type cannot be inferred without an annotation (list[T] annotations are not supported yet, D-105)".to_string(),
                    Span::new(0, 0),
                ));
            }
            let mut elem_ty: Option<Ty> = None;
            for element in elements {
                let this_ty = infer_expr_in(env, local_names, element)?;
                match &elem_ty {
                    None => elem_ty = Some(this_ty),
                    Some(expected) if *expected == this_ty => {}
                    Some(expected) => {
                        // Exact `Ty` equality, not `is_assignable`'s
                        // bool-is-an-int-subtype rule used elsewhere in this
                        // file -- D-105 requires every element to share the
                        // *exact same* `Ty`, so `[1, True]` is T0032 even
                        // though a bare `bool` is assignable to `int`.
                        return Err(Diagnostic::error(
                            "T0032",
                            format!(
                                "list element type mismatch: expected {} (from the first element), found {}",
                                expected.name(),
                                this_ty.name()
                            ),
                            Span::new(0, 0),
                        ));
                    }
                }
            }
            let elem_ty = elem_ty.expect("checked non-empty above");
            // This is the one place a list literal's element type becomes
            // known with a real source construct behind it (D-105's
            // Consequences) -- deliberately placed here rather than as a
            // separate pre-codegen pass. Everything above this gate (the
            // homogeneity check) is fully generic over any scalar `Ty`; a
            // future PR widening codegen to e.g. `list[str]` only has to
            // relax this one check.
            if elem_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list[{}] is not compiled yet (D-105) -- only list[int] is",
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::List(Box::new(elem_ty)))
        }
        // PR-11 Task 3 (D-123): mirrors `ListLiteral`'s own homogeneity
        // check above, extended to a key/value pair, plus a `dict[str,
        // int]`-only gate mirroring `ListLiteral`'s own `T0034` gate
        // (D-122: "exactly one combination gets real codegen").
        HirExpr::DictLiteral(pairs) => {
            let Some((first_key, first_value)) = pairs.first() else {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty dict literal's key/value types cannot be inferred without an annotation (dict[K, V] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            };
            let key_ty = infer_expr_in(env, local_names, first_key)?;
            let val_ty = infer_expr_in(env, local_names, first_value)?;
            for (key, value) in &pairs[1..] {
                let this_key_ty = infer_expr_in(env, local_names, key)?;
                let this_val_ty = infer_expr_in(env, local_names, value)?;
                // Exact `Ty` equality on both key and value, not
                // `is_assignable`'s bool-is-an-int-subtype rule -- same
                // reasoning as `ListLiteral`'s own homogeneity check.
                if this_key_ty != key_ty || this_val_ty != val_ty {
                    return Err(Diagnostic::error(
                        "T0035",
                        format!(
                            "dict entry type mismatch: expected {}: {} (from the first pair), found {}: {}",
                            key_ty.name(),
                            val_ty.name(),
                            this_key_ty.name(),
                            this_val_ty.name(),
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
            let dict_ty = Ty::Dict(Box::new((key_ty, val_ty)));
            if dict_ty != Ty::Dict(Box::new((Ty::Str, Ty::Int))) {
                return Err(Diagnostic::error(
                    "T0036",
                    format!(
                        "{} is not compiled yet (D-122) -- only dict[str, int] is",
                        dict_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(dict_ty)
        }
        // PR-11 Task 7 (D-123): mirrors `ListLiteral`'s own homogeneity
        // check above, for a single-element-type container (no key/value
        // pair), plus a `set[int]`-only gate mirroring `ListLiteral`'s own
        // `T0034` gate (D-122: "exactly one combination gets real codegen").
        // Unlike `DictLiteral` above, the empty-literal branch below is
        // unreachable from any real Python source: `{}` always parses as an
        // empty *dict* (Python has no empty-set literal spelling at all --
        // `set()` is a call, not a literal), so this only fires for a
        // hand-built `HirExpr::SetLiteral(vec![])` (e.g. this file's own
        // unit tests).
        HirExpr::SetLiteral(elements) => {
            let Some(first) = elements.first() else {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty set literal's element type cannot be inferred without an annotation (set[T] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            };
            let elem_ty = infer_expr_in(env, local_names, first)?;
            for element in &elements[1..] {
                let this_ty = infer_expr_in(env, local_names, element)?;
                // Exact `Ty` equality, not `is_assignable`'s
                // bool-is-an-int-subtype rule -- same reasoning as
                // `ListLiteral`'s own homogeneity check.
                if this_ty != elem_ty {
                    return Err(Diagnostic::error(
                        "T0037",
                        format!(
                            "set element type mismatch: expected {} (from the first element), found {}",
                            elem_ty.name(),
                            this_ty.name(),
                        ),
                        Span::new(0, 0),
                    ));
                }
            }
            let set_ty = Ty::Set(Box::new(elem_ty));
            if set_ty != Ty::Set(Box::new(Ty::Int)) {
                return Err(Diagnostic::error(
                    "T0038",
                    format!(
                        "{} is not compiled yet (D-122) -- only set[int] is",
                        set_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(set_ty)
        }
        // PR-11b Task 3 (D-116): unlike `ListLiteral`/`DictLiteral`/
        // `SetLiteral`'s homogeneity checks, this arm allows *any* mix of
        // accepted element types -- heterogeneity is tuple's own defining
        // feature. The gate is per-element type membership (int/bool/float
        // only), not agreement with a first element's type.
        HirExpr::TupleLiteral(elements) => {
            if elements.is_empty() {
                return Err(Diagnostic::error(
                    "T0021",
                    "an empty tuple literal's element types cannot be inferred without an annotation (tuple[...] annotations are not supported yet)".to_string(),
                    Span::new(0, 0),
                ));
            }
            let mut elem_tys = Vec::with_capacity(elements.len());
            for element in elements {
                let this_ty = infer_expr_in(env, local_names, element)?;
                if !matches!(this_ty, Ty::Int | Ty::Bool | Ty::Float) {
                    return Err(Diagnostic::error(
                        "T0039",
                        format!(
                            "tuple element type `{}` is not compiled yet (D-116) -- only int/bool/float elements are",
                            this_ty.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                elem_tys.push(this_ty);
            }
            Ok(Ty::Tuple(Box::new(elem_tys)))
        }
        HirExpr::Subscript { base, index } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            let index_ty = infer_expr_in(env, local_names, index)?;
            match base_ty {
                Ty::List(elem_ty) => {
                    // Reuses T0021 (an unconstrained/conflicting-constraint
                    // shape), not a new code -- a non-int-compatible index is
                    // that same "operand type mismatch" failure, not a
                    // distinct one.
                    //
                    // Uses `is_assignable`, not exact `Ty` equality: D-086
                    // already established that `bool` is accepted wherever
                    // `int` is expected at an operand boundary (mirroring
                    // `is_assignable`'s own existing param/assignment rule),
                    // and indexing is exactly that kind of boundary --
                    // `xs[True]` is ordinary, CPython-valid Python (`bool` is
                    // an `int` subtype, PEP 285), not a type error.
                    // `pycc_codegen`'s `to_tagged_int` already has a
                    // `Scalar::Bool` arm reached unconditionally by every
                    // subscript index, so this was a pure over-rejection in
                    // the type checker, not a missing codegen capability.
                    if !is_assignable(index_ty.clone(), Ty::Int) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!("list index must be `int`, found `{}`", index_ty.name()),
                            Span::new(0, 0),
                        ));
                    }
                    Ok(*elem_ty)
                }
                // PR-11 Task 3 (D-123): `d[k]` read. Uses exact `Ty`
                // equality on the key, not `is_assignable` -- every
                // `Ty::Dict` value that survives `DictLiteral`'s own
                // `T0036` gate above has key type exactly `Ty::Str` (no
                // other combination reaches codegen, and no other source
                // construct can produce a `Ty::Dict` value at all), so
                // there is no bool/int-style widening question here, unlike
                // the list-index case above.
                Ty::Dict(kv) => {
                    let (key_ty, val_ty) = *kv;
                    if index_ty != key_ty {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!(
                                "dict key type mismatch: expected `{}`, found `{}`",
                                key_ty.name(),
                                index_ty.name()
                            ),
                            Span::new(0, 0),
                        ));
                    }
                    Ok(val_ty)
                }
                // PR-11b Task 3 (D-116): `t[k]` requires `k` to be a
                // literal, non-negative, in-bounds integer -- not merely
                // `int`-typed, unlike `List`'s case above. A heterogeneous
                // tuple's element type at position `k` is only knowable
                // when `k` is known at compile time, so every failure shape
                // (non-literal, negative, out-of-range) shares one code.
                Ty::Tuple(elems) => {
                    let HirExpr::IntLiteral(literal_index) = index.as_ref() else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ));
                    };
                    let Ok(literal_index) = usize::try_from(*literal_index) else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ));
                    };
                    let Some(elem_ty) = elems.get(literal_index) else {
                        return Err(Diagnostic::error(
                            "T0040",
                            "tuple index must be a non-negative literal integer within range"
                                .to_string(),
                            Span::new(0, 0),
                        ));
                    };
                    Ok(elem_ty.clone())
                }
                // PR-11 Task 7 (D-123): `Ty::Set` deliberately has no
                // explicit arm here and falls through to this rejection --
                // real Python sets are not subscriptable either (`s[0]`
                // raises `TypeError` in CPython too), so this is not a v0.2
                // scope cut needing its own arm, it is the semantically
                // correct behavior for every other base type as well.
                other => Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support indexing", other.name()),
                    Span::new(0, 0),
                )),
            }
        }
        // PR-12 Task 7 (D-118): `base[start:stop:step]` read. Only
        // `list[int]` ships slicing in v0.2 -- every other base (including
        // `Ty::Dict`/`Ty::Set`, which real CPython also rejects for `[i:j]`,
        // and `Ty::Tuple`, an explicit deferral rather than a "never
        // supported" case) reuses the same `T0033` code `Subscript`'s own
        // `other` fallthrough above already established for non-indexable
        // bases, not a new one. A `list[T]` with `T != Ty::Int` reuses
        // `ListLiteral`'s own `T0034` ("only list[int] is compiled") gate.
        // Diagnostic order is deliberately base-type (`T0033`) before
        // element-type (`T0034`) before any bound's own type (`T0021`),
        // mirroring this file's existing "callee/base-type errors before
        // argument errors" convention (D-110's callee-first precedent,
        // applied here to base-type-before-bound-type) -- pinned by
        // `slicing_reports_the_base_type_error_before_any_bound_error` and
        // `slicing_reports_the_element_type_error_before_any_bound_error`
        // below.
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            let base_ty = infer_expr_in(env, local_names, base)?;
            let Ty::List(elem_ty) = &base_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!(
                        "`{}` does not support slicing (only list[int] does)",
                        base_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            };
            if **elem_ty != Ty::Int {
                return Err(Diagnostic::error(
                    "T0034",
                    format!(
                        "list codegen only supports `list[int]` in v0.2, cannot slice `list[{}]`",
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            // Each bound is independently optional (`xs[:]`, `xs[1:]`,
            // `xs[:3]`, `xs[::2]` all parse) and, when present, an
            // arbitrary runtime `int`-typed expression -- not a literal-only
            // check -- so `is_assignable` (not exact `Ty` equality) applies
            // here, same as `Subscript`'s own index check above: `xs[True:]`
            // is ordinary, CPython-valid Python (`bool` is an `int`
            // subtype, PEP 285), not a type error. `step`'s runtime
            // positivity is deliberately not validated here -- it can't be,
            // for a non-literal runtime expression, at compile time; that
            // check is a later task's job (D-118).
            for (label, bound) in [("start", start), ("stop", stop), ("step", step)] {
                if let Some(bound) = bound {
                    let bound_ty = infer_expr_in(env, local_names, bound)?;
                    if !is_assignable(bound_ty.clone(), Ty::Int) {
                        return Err(Diagnostic::error(
                            "T0021",
                            format!("slice {label} must be `int`, got `{}`", bound_ty.name()),
                            Span::new(0, 0),
                        ));
                    }
                }
            }
            Ok(base_ty.clone())
        }
        HirExpr::ListAppend { list, value } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.append()`", list_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let value_ty = infer_expr_in(env, local_names, value)?;
            // Uses `is_assignable`, not exact `Ty` equality -- matching the
            // `Subscript` index check above (D-086): `x = [1]; x.append(True)`
            // is ordinary, CPython-valid Python (`bool` is an `int` subtype),
            // and `pycc_codegen`'s `MirExpr::ListAppend` arm already routes
            // the appended value through `to_tagged_int` (the same
            // `Scalar::Bool`-handling conversion the index path uses)
            // unconditionally, so there is no missing codegen capability
            // here either. This is NOT the same question as `ListLiteral`'s
            // own homogeneity check above (`[1, True]`'s element type is
            // genuinely ambiguous to infer -- there is no already-known
            // `elem_ty` to check against); `.append()` on an *already-typed*
            // `list[int]` has no such ambiguity, so the looser
            // `is_assignable` rule applies here, not there. Reuses T0021 (a
            // call-site/assignment constraint mismatch), not a new code.
            if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot append `{}` to a list of `{}`",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::None)
        }
        // PR-12 Task 10 (D-119): `list.pop()`. Unlike `ListAppend`, this
        // arm's result is the list's own element type, not `Ty::None` --
        // `.pop()` is meant to be used for its value.
        HirExpr::ListPop { list } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            let Ty::List(elem_ty) = &list_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.pop()`", list_ty.name()),
                    Span::new(0, 0),
                ));
            };
            Ok((**elem_ty).clone())
        }
        // PR-12 Task 10 (D-119): `dict.get(key, default)`. The key check
        // uses exact `Ty` equality, not `is_assignable` -- same reasoning as
        // `Subscript`'s own `Ty::Dict` read arm and `check_dict_set` above
        // (every `Ty::Dict` value that survives `DictLiteral`'s own `T0036`
        // gate has key type exactly `Ty::Str`, so there is no bool/int-style
        // widening question for a dict *key*, unlike a list index or a
        // dict *value*). The `default` check does use `is_assignable`,
        // mirroring `ListAppend`'s/`check_dict_set`'s own value-position
        // leniency (D-086 `bool`-subtypes-`int`). The overall result is the
        // dict's *value* type, never `Ty::None`, since a missing key still
        // yields the (same-typed) default rather than `None`.
        HirExpr::DictGetOrDefault { dict, key, default } => {
            let dict_ty = lookup_bound_name(env, local_names, dict)?;
            let Ty::Dict(kv) = &dict_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.get()`", dict_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let key_ty = infer_expr_in(env, local_names, key)?;
            if key_ty != kv.0 {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot look up a `{}` key in a dict of `{}` keys",
                        key_ty.name(),
                        kv.0.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            let default_ty = infer_expr_in(env, local_names, default)?;
            if !is_assignable(default_ty.clone(), kv.1.clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot use a `{}` default for a dict of `{}` values",
                        default_ty.name(),
                        kv.1.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(kv.1.clone())
        }
        // PR-12 Task 10 (D-119): `set.add(value)`. Mirrors `ListAppend`
        // exactly -- always `Ty::None`, with D-131's ordinary assignment
        // storage available when that result is bound to a name.
        HirExpr::SetAdd { set, value } => {
            let set_ty = lookup_bound_name(env, local_names, set)?;
            let Ty::Set(elem_ty) = &set_ty else {
                return Err(Diagnostic::error(
                    "T0033",
                    format!("`{}` does not support `.add()`", set_ty.name()),
                    Span::new(0, 0),
                ));
            };
            let value_ty = infer_expr_in(env, local_names, value)?;
            if !is_assignable(value_ty.clone(), (**elem_ty).clone()) {
                return Err(Diagnostic::error(
                    "T0021",
                    format!(
                        "cannot add `{}` to a set of `{}`",
                        value_ty.name(),
                        elem_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(Ty::None)
        }
    }
}

fn is_assignable(from: Ty, to: Ty) -> bool {
    from == to || (from == Ty::Bool && to == Ty::Int) // bool is a subtype of int, TYPE_SYSTEM.md's representation table
}

fn numeric_result_type(op: BinOpKind, left: Ty, right: Ty) -> Result<Ty, Diagnostic> {
    if left == Ty::Str && right == Ty::Str {
        return if op == BinOpKind::Add {
            Ok(Ty::Str)
        } else {
            Err(Diagnostic::error(
                "T0021",
                format!("operator {op:?} is not defined for `str` and `str`"),
                Span::new(0, 0),
            ))
        };
    }
    let as_numeric = |t: &Ty| match t {
        Ty::Bool | Ty::Int => Some(Ty::Int),
        Ty::Float => Some(Ty::Float),
        _ => None,
    };
    match (as_numeric(&left), as_numeric(&right)) {
        (Some(_), Some(_)) if op == BinOpKind::Div => Ok(Ty::Float),
        (Some(Ty::Int), Some(Ty::Int)) => Ok(Ty::Int),
        (Some(_), Some(_)) => Ok(Ty::Float),
        _ => Err(Diagnostic::error(
            "T0021",
            format!(
                "operator {op:?} is not defined for `{}` and `{}`",
                left.name(),
                right.name()
            ),
            Span::new(0, 0),
        )),
    }
}

fn numeric_or_bool_compatible(a: Ty, b: Ty) -> bool {
    let is_numeric_like = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool);
    (is_numeric_like(&a) && is_numeric_like(&b)) || (a == Ty::Str && b == Ty::Str)
}

fn check_range_operand(
    env: &Environment,
    position: &str,
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    check_range_operand_in(env, &[], position, expr)
}

fn check_range_operand_in(
    env: &Environment,
    local_names: &[&str],
    position: &str,
    expr: &HirExpr,
) -> Result<(), Diagnostic> {
    let actual = infer_expr_in(env, local_names, expr)?;
    if is_assignable(actual.clone(), Ty::Int) {
        Ok(())
    } else {
        Err(Diagnostic::error(
            "T0021",
            format!("range {position} expects `int`, got `{}`", actual.name()),
            Span::new(0, 0),
        ))
    }
}

fn check_assignment(env: &mut Environment, target: &str, ty: Ty) -> Result<(), Diagnostic> {
    // Every value assignment re-shadows a same-named `def` (D-110),
    // including a compatible-type reassignment of a name that already has a
    // representation record -- that branch below returns without calling
    // `bind()`, so clearing only inside `bind()` left the def-rebound flag
    // permanently stuck for any name with a pre-`def` binding (PR #252's
    // round-5 review caught `helper = 1; def helper() -> int: ...;
    // helper = 2; helper()` resolving the function where CPython raises
    // `TypeError`). Cleared here unconditionally, matching the solver's own
    // unconditional clearing in its Assign arm. Function bodies operate on
    // `child_for_function` clones, so a body-local assignment clears only
    // that body's view, never the module-level fact.
    env.def_rebound.remove(target);
    if let Some(previous) = env.lookup(target) {
        if !is_assignable(ty.clone(), previous.clone()) {
            return Err(Diagnostic::error(
                "T0023",
                format!(
                    "cannot assign `{}` to `{target}`, previously inferred as `{}`",
                    ty.name(),
                    previous.name()
                ),
                Span::new(0, 0),
            ));
        }
        return Ok(());
    }
    env.bind(target.to_string(), ty);
    Ok(())
}

/// Resolves a comprehension's iterable (`pycc_hir::CompIter`) to the loop
/// variable's type, without binding it -- mirrors `HirStmt::ForList`'s own
/// resolution exactly (`check_stmt`/`check_stmt_in_function`'s existing
/// `ForList` arms), reused rather than duplicated a third time (PR-12,
/// D-117). Range/list/dict/set element-type resolution is identical to
/// `ForList`'s; a comprehension adds nothing new here.
fn resolve_comp_iter(
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

pub fn check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Assign { target, value } => {
            let ty = infer_expr(env, value)?;
            check_assignment(env, target, ty)
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
        } => {
            if let Some(value) = value {
                let inferred = infer_expr(env, value)?;
                if !is_assignable(inferred.clone(), annotation.clone()) {
                    return Err(Diagnostic::error(
                        "T0025",
                        format!(
                            "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                            inferred.name(),
                            annotation.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                // Route through `check_assignment` (not a raw `env.bind`) so a
                // name's first-established representation stays sticky across
                // an annotated re-declaration, exactly as it already does for
                // plain `Assign` -- `pycc_mir`'s own `bind_variable` (D-040's
                // "first assignment fixes a binding's representation"
                // invariant) keeps the *first* recorded MIR type regardless of
                // a later compatible reassignment, so the checker's `env` must
                // agree or a later annotated reassignment (e.g. `x = 1` then
                // `x: str = "s"`, where `is_assignable(Str, Str)` alone would
                // wrongly accept it) could diverge from what codegen actually
                // stores.
                check_assignment(env, target, annotation.clone())?;
            }
            // No value: register no binding, matching CPython's own "declared, not yet
            // assigned" semantics -- collect_local_names (Step 1) already marked
            // `target` local, so a premature read still raises the existing T0021.
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr(env, expr).map(|_| ()),
        HirStmt::If { test, body, orelse } => {
            infer_expr(env, test)?; // any type is accepted as truthy for v0.1 -- Python's own truthiness has no static type restriction
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            for stmt in orelse {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            infer_expr(env, test)?;
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            check_range_operand(env, "start", start)?;
            check_range_operand(env, "stop", stop)?;
            check_range_operand(env, "step", step)?;
            check_assignment(env, var, Ty::Int)?;
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            // Module (top-level) scope has no "local before assignment"
            // concept the way a function body does -- every other arm here
            // (e.g. `ExprStmt` via `infer_expr`) resolves names with an
            // empty `local_names` slice too, so an unresolved `list` is
            // simply "not defined," never `unbound_local`.
            let list_ty = lookup_bound_name(env, &[], list)?;
            // PR-11 Task 3 (D-123): `for k in d:` iterates a dict's own keys
            // in insertion order, mirroring `len()`'s own relaxation to
            // accept `Ty::Dict` alongside `Ty::List` at this crate's other
            // hand-recognized dispatch points. `HirStmt::ForList` itself is
            // reused unconditionally for any bare-name iterable, dict or
            // list alike (`pycc_hir`'s own lowering has no type information
            // to pick a different node) -- this is the point where the real
            // type is resolved. PR-11 Task 7 (D-123): `for x in s:` iterates
            // a set's own elements (order is this implementation's own
            // insertion order, not a CPython-matching guarantee -- see
            // D-123's own iteration-order caveat), so `Ty::Set` is accepted
            // here too, binding the loop variable as the set's element type.
            let var_ty = match list_ty {
                Ty::List(elem_ty) => *elem_ty,
                Ty::Dict(kv) => kv.0,
                Ty::Set(elem_ty) => *elem_ty,
                other => {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                            other.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            };
            check_assignment(env, var, var_ty)?;
            for stmt in body {
                check_stmt(env, stmt)?;
            }
            Ok(())
        }
        // PR-12 Task 3 (D-117): `target = [elt for var in iter [if cond]]`
        // at module scope. `var` is resolved and bound exactly like
        // `ForList`'s own loop variable above (via the shared
        // `resolve_comp_iter` helper) *before* `cond`/`elt` are checked, so
        // a reference to the loop variable inside either sub-expression
        // resolves correctly. The produced element type is gated to
        // `Ty::Int` -- identical rule to `ListLiteral`'s own `T0034` gate
        // (D-105/D-122), just reached via a new code path (D-119); no new
        // diagnostic code is minted.
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, &[], iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr(env, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let elt_ty = infer_expr(env, elt)?;
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
            check_assignment(env, target, Ty::List(Box::new(Ty::Int)))
        }
        // PR-12 Task 3 (D-117): `target = {elt for var in iter [if cond]}`
        // at module scope. Mirrors `ListCompAssign` above exactly except for
        // the produced type and diagnostic code (D-119: reuses `T0038`,
        // identical to `SetLiteral`'s own gate).
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, &[], iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr(env, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let elt_ty = infer_expr(env, elt)?;
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
            check_assignment(env, target, Ty::Set(Box::new(Ty::Int)))
        }
        // PR-12 Task 3 (D-117): `target = {key: value for var in iter [if
        // cond]}` at module scope. Mirrors `ListCompAssign` above except for
        // the key/value split (D-119: reuses `T0036`, identical to
        // `DictLiteral`'s own gate).
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let var_ty = resolve_comp_iter(env, &[], iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr(env, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
            let key_ty = infer_expr(env, key)?;
            let value_ty = infer_expr(env, value)?;
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
            check_assignment(env, target, Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        }
        HirStmt::Return(_) => Err(Diagnostic::error(
            "T0024",
            "'return' outside a function is not allowed".to_string(),
            Span::new(0, 0),
        )),
        HirStmt::DictSet { dict, key, value } => check_dict_set(env, &[], dict, key, value),
    }
}

/// `d[k] = v` (PR-11 Task 3, D-123): insert-or-update. Shared between
/// module (`check_stmt`, `local_names = &[]`) and function-body
/// (`check_stmt_in_function`) scope, mirroring how `check_range_operand`/
/// `check_range_operand_in` are already split for `ForRange`. Reuses
/// `T0033` for a non-`dict` base (matching `ListAppend`'s own "does not
/// support X" shape) and `T0021` for a key or value type mismatch
/// (matching `Subscript`'s and `ListAppend`'s own reuse of `T0021` for an
/// operand/assignment constraint mismatch).
fn check_dict_set(
    env: &Environment,
    local_names: &[&str],
    dict: &str,
    key: &HirExpr,
    value: &HirExpr,
) -> Result<(), Diagnostic> {
    let dict_ty = lookup_bound_name(env, local_names, dict)?;
    let Ty::Dict(kv) = &dict_ty else {
        return Err(Diagnostic::error(
            "T0033",
            format!("`{}` does not support item assignment", dict_ty.name()),
            Span::new(0, 0),
        ));
    };
    let (key_ty, val_ty) = kv.as_ref();
    // Exact `Ty` equality on the key, not `is_assignable` -- same reasoning
    // as `Subscript`'s own `Ty::Dict` read arm.
    let key_expr_ty = infer_expr_in(env, local_names, key)?;
    if key_expr_ty != *key_ty {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "dict key type mismatch: expected `{}`, found `{}`",
                key_ty.name(),
                key_expr_ty.name()
            ),
            Span::new(0, 0),
        ));
    }
    let value_ty = infer_expr_in(env, local_names, value)?;
    if !is_assignable(value_ty.clone(), val_ty.clone()) {
        return Err(Diagnostic::error(
            "T0021",
            format!(
                "cannot assign `{}` to a dict value of `{}`",
                value_ty.name(),
                val_ty.name()
            ),
            Span::new(0, 0),
        ));
    }
    Ok(())
}

pub fn check_function(function: &HirItem) -> Result<(), Diagnostic> {
    let local_names = match function {
        HirItem::Function { params, body, .. } => function_local_names(params, body),
        HirItem::TopLevelStmt(_) => Vec::new(),
    };
    check_function_in(&Environment::new(), function, &local_names)
}

/// Checks one function's body, resolving sibling calls and module-level
/// global reads against a clone of `module_env` (see D-040/D-041/D-055) instead
/// of an isolated, self-only scope. Lexically local binding targets are removed
/// from that clone before the body is checked. The clone owns independent value
/// bindings while sharing the immutable function registry through copy-on-write
/// storage, so a function's parameters and local assignments never leak back
/// into the module scope or into any other function's check.
fn check_function_in(
    module_env: &Environment,
    function: &HirItem,
    local_names: &[&str],
) -> Result<(), Diagnostic> {
    let HirItem::Function {
        name,
        params,
        return_ty,
        body,
    } = function
    else {
        panic!("check_function called with a non-Function HirItem");
    };
    let standalone_params;
    let (resolved_params, resolved_return, signature_was_registered) =
        if let Some((param_tys, return_ty)) = module_env.lookup_function(name) {
            (param_tys.as_slice(), return_ty.clone(), true)
        } else {
            standalone_params = params.iter().map(|(_, ty)| ty.clone()).collect::<Vec<_>>();
            (standalone_params.as_slice(), return_ty.clone(), false)
        };
    if resolved_params.contains(&Ty::Infer) || resolved_return == Ty::Infer {
        return Err(Diagnostic::error(
            "T0021",
            format!("cannot check private helper `{name}` before its signature is inferred"),
            Span::new(0, 0),
        ));
    }
    let mut env = module_env.child_for_function(local_names);
    if !signature_was_registered {
        env.bind_function(name.clone(), resolved_params.to_vec(), resolved_return.clone());
    }
    for ((param_name, _), param_ty) in params.iter().zip(resolved_params.iter().cloned()) {
        env.bind(param_name.clone(), param_ty);
    }
    for stmt in body {
        check_stmt_in_function(&mut env, local_names, stmt, resolved_return.clone())?;
    }
    if resolved_return != Ty::None && !block_always_returns(body) {
        return Err(Diagnostic::error(
            "T0022",
            format!(
                "function `{name}` can exit without returning `{}`",
                resolved_return.name()
            ),
            Span::new(0, 0),
        ));
    }
    Ok(())
}

fn block_always_returns(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Return(_) => true,
        HirStmt::If { body, orelse, .. } => {
            !orelse.is_empty() && block_always_returns(body) && block_always_returns(orelse)
        }
        HirStmt::ExprStmt(_)
        | HirStmt::Assign { .. }
        | HirStmt::AnnAssign { .. }
        | HirStmt::While { .. }
        | HirStmt::ForRange { .. }
        | HirStmt::ForList { .. }
        | HirStmt::DictSet { .. }
        // PR-12 Task 3 (D-117): a comprehension statement never contains a
        // `return` (its `elt`/`cond`/`key`/`value` are expressions, not
        // statements), so it can never make a block always return, exactly
        // like `Assign`/`ForList`/`DictSet` above.
        | HirStmt::ListCompAssign { .. }
        | HirStmt::SetCompAssign { .. }
        | HirStmt::DictCompAssign { .. } => false,
    })
}

fn check_stmt_in_function(
    env: &mut Environment,
    local_names: &[&str],
    stmt: &HirStmt,
    return_ty: Ty,
) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Return(None) => {
            if return_ty != Ty::None {
                return Err(Diagnostic::error(
                    "T0022",
                    format!(
                        "expected a return value of type `{}`, got none",
                        return_ty.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(())
        }
        HirStmt::Return(Some(expr)) => {
            let actual = infer_expr_in(env, local_names, expr)?;
            if !is_assignable(actual.clone(), return_ty.clone()) {
                return Err(Diagnostic::error(
                    "T0022",
                    format!(
                        "expected return type `{}`, got `{}`",
                        return_ty.name(),
                        actual.name()
                    ),
                    Span::new(0, 0),
                ));
            }
            Ok(())
        }
        HirStmt::If { test, body, orelse } => {
            infer_expr_in(env, local_names, test)?;
            for s in body {
                check_stmt_in_function(env, local_names, s, return_ty.clone())?;
            }
            for s in orelse {
                check_stmt_in_function(env, local_names, s, return_ty.clone())?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            infer_expr_in(env, local_names, test)?;
            for s in body {
                check_stmt_in_function(env, local_names, s, return_ty.clone())?;
            }
            Ok(())
        }
        HirStmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            check_range_operand_in(env, local_names, "start", start)?;
            check_range_operand_in(env, local_names, "stop", stop)?;
            check_range_operand_in(env, local_names, "step", step)?;
            check_assignment(env, var, Ty::Int)?;
            for s in body {
                check_stmt_in_function(env, local_names, s, return_ty.clone())?;
            }
            Ok(())
        }
        HirStmt::ForList { var, list, body } => {
            let list_ty = lookup_bound_name(env, local_names, list)?;
            // See the module-scope `check_stmt` arm's own comment (PR-11
            // Task 3, D-123): `for k in d:` iterates a dict's keys. PR-11
            // Task 7 (D-123): `for x in s:` iterates a set's elements.
            let var_ty = match list_ty {
                Ty::List(elem_ty) => *elem_ty,
                Ty::Dict(kv) => kv.0,
                Ty::Set(elem_ty) => *elem_ty,
                other => {
                    return Err(Diagnostic::error(
                        "T0033",
                        format!(
                            "`{}` cannot be iterated with `for ... in ...` (only list[T]/dict[K, V]/set[T] supports this)",
                            other.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
            };
            check_assignment(env, var, var_ty)?;
            for s in body {
                check_stmt_in_function(env, local_names, s, return_ty.clone())?;
            }
            Ok(())
        }
        // PR-12 Task 3 (D-117): function-scope counterparts of the
        // module-scope `check_stmt` arms above, `local_names`-aware
        // (`resolve_comp_iter`/`infer_expr_in` in place of the module-scope
        // helpers/`&[]`) -- otherwise identical, mirroring exactly how
        // `ForList`'s own two arms (module vs. function scope) already
        // differ only in that respect.
        HirStmt::ListCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, local_names, iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr_in(env, local_names, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
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
            check_assignment(env, target, Ty::List(Box::new(Ty::Int)))
        }
        HirStmt::SetCompAssign {
            target,
            var,
            iter,
            cond,
            elt,
        } => {
            let var_ty = resolve_comp_iter(env, local_names, iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr_in(env, local_names, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
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
            check_assignment(env, target, Ty::Set(Box::new(Ty::Int)))
        }
        HirStmt::DictCompAssign {
            target,
            var,
            iter,
            cond,
            key,
            value,
        } => {
            let var_ty = resolve_comp_iter(env, local_names, iter)?;
            check_assignment(env, var, var_ty)?;
            if let Some(cond) = cond {
                infer_expr_in(env, local_names, cond)?; // any type is accepted as truthy, mirroring `If`/`While`
            }
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
            check_assignment(env, target, Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        }
        HirStmt::Assign { target, value } => {
            let ty = infer_expr_in(env, local_names, value)?;
            check_assignment(env, target, ty)
        }
        HirStmt::AnnAssign {
            target,
            annotation,
            value,
        } => {
            if let Some(value) = value {
                let inferred = infer_expr_in(env, local_names, value)?;
                if !is_assignable(inferred.clone(), annotation.clone()) {
                    return Err(Diagnostic::error(
                        "T0025",
                        format!(
                            "cannot assign `{}` to `{target}: {}`, initializer does not match the declared annotation",
                            inferred.name(),
                            annotation.name()
                        ),
                        Span::new(0, 0),
                    ));
                }
                // See the module-scope `check_stmt` arm's comment: route
                // through `check_assignment` so a name's first-established
                // representation stays sticky, matching `pycc_mir`'s own
                // `bind_variable` invariant.
                check_assignment(env, target, annotation.clone())?;
            }
            Ok(())
        }
        HirStmt::ExprStmt(expr) => infer_expr_in(env, local_names, expr).map(|_| ()),
        HirStmt::DictSet { dict, key, value } => check_dict_set(env, local_names, dict, key, value),
    }
}

/// Type-checks a module and returns a cloned HIR whose function signatures
/// contain only the concrete types resolved by private-helper inference.
/// Consumers after the type boundary must use this module rather than the
/// unresolved lowering result so `Ty::Infer` can never leak into MIR or code
/// generation.
pub fn check_and_resolve(hir: &HirModule) -> Result<HirModule, Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    let signatures = checked_function_signatures(hir, &function_local_names)?;

    let mut resolved_hir = hir.clone();
    for item in &mut resolved_hir.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        let (resolved_params, resolved_return) = signatures
            .get(name)
            .expect("every HIR function received an inferred signature");
        for ((_, param_ty), resolved_ty) in params.iter_mut().zip(resolved_params) {
            *param_ty = resolved_ty.clone();
        }
        *return_ty = resolved_return.clone();
    }

    Ok(resolved_hir)
}

fn check_with_signatures(
    hir: &HirModule,
    signatures: &HashMap<String, (Vec<Ty>, Ty)>,
    function_local_names: &[Vec<&str>],
) -> Result<(), Diagnostic> {
    let mut env = Environment::new();
    // Pass 1: register every function's signature before checking any
    // statement body, matching Python's own "a module runs top to bottom,
    // but any def already executed is callable" semantics -- top-level
    // code and other function bodies (D-040) both need to see every
    // function regardless of its position in the file.
    for item in &hir.items {
        if let HirItem::Function { name, .. } = item {
            let (param_tys, return_ty) = signatures
                .get(name)
                .expect("every HIR function received an inferred signature");
            env.bind_function(name.clone(), param_tys.clone(), return_ty.clone());
        }
    }
    check_with_environment(hir, env, function_local_names)
}

fn check_with_environment(
    hir: &HirModule,
    mut env: Environment,
    function_local_names: &[Vec<&str>],
) -> Result<(), Diagnostic> {
    // Pass 2: check every top-level statement in source order, growing
    // `env`'s bindings as module-level assignments are encountered --
    // ordinary top-level code is still checked top-to-bottom (a top-level
    // forward reference to a not-yet-assigned name is a genuine error).
    // A `def` executes at its own position in that order and rebinds its
    // name to the function (D-110, refined by PR #252's review): later
    // `helper()` calls resolve the function exactly as CPython does, while
    // a value assignment *after* the `def` shadows it again. The `def` only
    // marks the name def-rebound -- it must NOT erase the representation
    // record in `bindings`, which D-040's sticky-representation rule keeps
    // consulting so an incompatible later reassignment still fails T0023.
    // The gate therefore tests the net source-order binding, in this pass
    // and in pass 3's final environment alike.
    for item in &hir.items {
        match item {
            HirItem::TopLevelStmt(stmt) => check_stmt(&mut env, stmt)?,
            HirItem::Function { name, .. } => {
                env.def_rebound.insert(name.clone());
            }
        }
    }
    // Pass 3: check every function body against a clone of `env` as it
    // stands once the whole module's top-level code has been processed
    // (D-041) -- a function can read any module-level global regardless of
    // whether its own `def` appears before or after that global's
    // assignment in the file, since real Python only evaluates a function
    // body when it's *called*, typically after the module has finished
    // running top to bottom.
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        if let HirItem::Function { .. } = item {
            check_function_in(&env, item, local_names)?;
        }
    }
    Ok(())
}

/// Type-checks a module without materializing a resolved HIR clone.
///
/// Use [`check_and_resolve`] when a downstream compiler stage needs concrete
/// private-helper signatures in the returned HIR.
pub fn check(hir: &HirModule) -> Result<(), Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    // The public validation-only API has no resolved-signature result to
    // return. Avoid building a temporary concrete signature map and then
    // cloning it into an `Environment`: construct that environment directly.
    // On validation failure, preserve the historical solver-first diagnostic
    // selection exactly as `checked_function_signatures` does.
    if let Some(env) = concrete_function_environment(hir)
        && check_with_environment(hir, env, &function_local_names).is_ok()
    {
        return Ok(());
    }
    let signatures = infer_function_signatures_with_solver(hir, &function_local_names)?;
    check_with_signatures(hir, &signatures, &function_local_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_1_slice_always_type_checks() {
        let hir = HirModule { items: vec![] };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn concrete_signatures_take_the_validation_only_fast_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "identity".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }],
        };

        assert_eq!(
            concrete_function_signatures(&hir).unwrap()["identity"],
            (vec![Ty::Int], Ty::Int)
        );
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn inferred_signatures_keep_the_constraint_solver_path() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_identity".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }],
        };

        assert!(concrete_function_signatures(&hir).is_none());
    }

    #[test]
    fn concrete_fast_path_preserves_solver_first_diagnostic_selection() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "takes_int".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "broken".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "takes_int".to_string(),
                            args: vec![HirExpr::StringLiteral("wrong".to_string())],
                        }),
                        HirStmt::Return(Some(HirExpr::StringLiteral("wrong".to_string()))),
                    ],
                },
            ],
        };
        let local_names = module_function_local_names(&hir);
        let concrete = concrete_function_signatures(&hir).unwrap();
        let validation_first = check_with_signatures(&hir, &concrete, &local_names).unwrap_err();
        let solver_first = infer_function_signatures_with_solver(&hir, &local_names).unwrap_err();
        let fast_path = checked_function_signatures(&hir, &local_names).unwrap_err();
        let public_check = check(&hir).unwrap_err();

        assert_eq!(validation_first.code, "T0021");
        assert_eq!(solver_first.code, "T0022");
        assert_eq!(fast_path.code, solver_first.code);
        assert_eq!(fast_path.message, solver_first.message);
        assert_eq!(public_check.code, solver_first.code);
        assert_eq!(public_check.message, solver_first.message);
    }

    #[test]
    fn constraint_collection_rejects_bound_and_unbound_local_call_targets() {
        for (body, expected_message) in [
            (
                vec![
                    HirStmt::Assign {
                        target: "helper".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::ExprStmt(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }),
                ],
                "name `helper` is bound to a non-callable value",
            ),
            (
                vec![
                    HirStmt::ExprStmt(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }),
                    HirStmt::Assign {
                        target: "helper".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                ],
                "local name `helper` is not bound before this use",
            ),
        ] {
            let hir = HirModule {
                items: vec![HirItem::Function {
                    name: "_caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body,
                }],
            };

            assert_eq!(check(&hir).unwrap_err().message, expected_message);
        }
    }

    #[test]
    fn constraint_collection_skips_already_concrete_call_arguments() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "takes_int".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "_caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "takes_int".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
            ],
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn constraint_collection_reuses_a_top_level_for_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "item".to_string(),
                    value: HirExpr::IntLiteral(0),
                }),
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn constraint_collection_rejects_a_non_integer_top_level_for_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "item".to_string(),
                    value: HirExpr::StringLiteral("not an integer".to_string()),
                }),
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };

        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_list_literal_still_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path()
     {
        // Any `Ty::Infer` signature anywhere in the module routes `check`
        // through `infer_function_signatures_with_solver` first (see
        // `checked_function_signatures`), which runs `collect_expr_constraints`
        // over every expression in the module, including this list literal.
        // That solver-side pass must stay lenient (`Ok(None)`, recurse only)
        // for list forms -- confirms it doesn't wrongly reject a valid list.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                    ]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_heterogeneous_list_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The load-bearing counterpart to the test above: the solver's
        // leniency must not swallow a genuine list error -- it has to fall
        // through to the real, list-aware check pass (`check_with_signatures`)
        // that runs after the solver, which is what actually raises T0032.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::StringLiteral("two".to_string()),
                    ]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0032");
    }

    #[test]
    fn a_for_list_loop_still_type_checks_correctly_when_the_solver_path_runs_first() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::ForList {
                    var: "i".to_string(),
                    list: "xs".to_string(),
                    body: vec![],
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    // PR-12 Task 7 (D-118): whole-module `check()` tests for `HirExpr::Slice`,
    // exercising the real statement walkers (`check_with_environment`'s
    // `check_stmt`, and `collect_block_constraints`'s `Assign` arm under the
    // solver path) rather than calling `infer_expr`/`collect_expr_constraints`
    // directly on a hand-built expression. Mirrors the list-literal pair
    // immediately above: Task 3's own ledger records a real regression of
    // exactly this shape (a solver arm that looked correct in isolation but
    // was never actually reached by the block walker) -- these confirm the
    // `Slice` arm added in this task is genuinely wired into both paths, not
    // just correct when invoked directly.

    #[test]
    fn slicing_type_checks_through_the_full_check_pipeline_on_the_fast_path() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: Some(Box::new(HirExpr::IntLiteral(1))),
                        stop: None,
                        step: None,
                    },
                }),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn slicing_a_list_of_str_is_rejected_through_the_full_check_pipeline() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: None,
                        stop: None,
                        step: None,
                    },
                }),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0034");
    }

    #[test]
    fn slicing_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path() {
        // Companion to `a_list_literal_still_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path`
        // above: proves `collect_block_constraints`'s ordinary `Assign` arm
        // reaches this task's new `HirExpr::Slice` constraint-collection arm
        // (which must stay lenient, `Ok(None)`, recursing only) without
        // wrongly rejecting a valid slice, and that the real check pass
        // (`check_with_signatures`) run afterward still type-checks it.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::IntLiteral(2),
                        HirExpr::IntLiteral(3),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: Some(Box::new(HirExpr::IntLiteral(1))),
                        stop: Some(Box::new(HirExpr::IntLiteral(3))),
                        step: None,
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn slicing_a_list_of_str_is_still_rejected_when_the_solver_path_runs_first() {
        // Load-bearing counterpart: the solver's leniency for `Slice` must
        // not swallow a genuine `T0034` -- it has to fall through to the
        // real, list-aware check pass that runs after the solver.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ys".to_string(),
                    value: HirExpr::Slice {
                        base: Box::new(HirExpr::Name("xs".to_string())),
                        start: None,
                        stop: None,
                        step: None,
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0034");
    }

    #[test]
    fn collect_block_constraints_binds_the_initializer_term_for_an_annotated_assignment() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &[],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(5)),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert_eq!(env.bindings.get("y"), Some(&Ok(Ty::Int)));
    }

    #[test]
    fn collect_block_constraints_skips_binding_when_the_initializer_term_is_unresolved() {
        // `unresolved_global` is neither already bound nor a declared local of
        // this scope, so `collect_expr_constraints` returns `Ok(None)` for it
        // (the same "punt, nothing to unify yet" case a plain `Assign` to an
        // unresolved global would hit) -- the new `AnnAssign` arm must skip
        // the `env.bindings` insert in that case rather than recording a
        // binding for an unresolved term.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &[],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::Name("unresolved_global".to_string())),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(!env.bindings.contains_key("y"));
    }

    #[test]
    fn collect_block_constraints_ignores_a_value_less_annotated_assignment() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &["y"],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: None,
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(!env.bindings.contains_key("y"));
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_the_initializer_expression() {
        // `z` is declared local to this scope but never bound, so
        // `collect_expr_constraints` returns `Err(unbound_local)` for it --
        // the `?` inside the new arm must propagate that error rather than
        // being reachable only via the `Some`/`None` term paths above.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            defs_rebound: HashSet::new(),
            bindings: HashMap::new(),
            local_names: &["z"],
        };
        let body = vec![HirStmt::AnnAssign {
            target: "y".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::Name("z".to_string())),
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_list_literal_as_unconstrained_but_recurses_into_elements() {
        // `TypeTerm` has no unification-friendly representation for
        // `Ty::List` -- this solver only infers scalar `Ty::Infer`
        // parameters/returns (see the comment on this arm in
        // `collect_expr_constraints`). It must still recurse into elements
        // to propagate genuine errors (see the next test) rather than
        // ignoring them outright.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_list_literal_element() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::ListLiteral(vec![HirExpr::Name("missing".to_string())]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_subscript_as_unconstrained_but_recurses_into_base_and_index()
    {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::IntLiteral(1)),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_subscript_base() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("missing".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_subscript_index() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::IntLiteral(1)),
            index: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_slice_as_unconstrained_but_recurses_into_base_and_bounds() {
        // PR-12 Task 7 (D-118): mirrors `Subscript`'s own solver arm above --
        // structurally identical recursion, no term produced for the slice
        // itself.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: Some(Box::new(HirExpr::IntLiteral(0))),
            stop: Some(Box::new(HirExpr::IntLiteral(2))),
            step: Some(Box::new(HirExpr::IntLiteral(1))),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_treats_a_slice_with_every_bound_omitted_as_unconstrained() {
        // Proves the `Option` loop's `None` branch is exercised too, not
        // just the `Some` branch above (`xs[:]`).
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: None,
            stop: None,
            step: None,
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_slice_base() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("missing".to_string())),
            start: None,
            stop: None,
            step: None,
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_slice_bound() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: Some(Box::new(HirExpr::Name("missing".to_string()))),
            stop: None,
            step: None,
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_list_append_as_unconstrained_but_recurses_into_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::ListAppend {
            list: "lst".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_list_append_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::ListAppend {
            list: "lst".to_string(),
            value: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    // -- PR-12 Task 10 (D-119): remaining container methods depth --------
    // `collect_expr_constraints` coverage, mirroring `ListAppend`'s own
    // direct-call test shape exactly.

    #[test]
    fn constraint_collection_treats_a_list_pop_as_unconstrained() {
        // `list` is a plain name, not a sub-expression -- unlike
        // `ListAppend`, there is no `value` to recurse into here.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::ListPop {
            list: "lst".to_string(),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_treats_a_dict_get_or_default_as_unconstrained_but_recurses_into_key_and_default()
     {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_get_or_default_key() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::Name("missing".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_get_or_default_default() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_treats_a_set_add_as_unconstrained_but_recurses_into_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_set_add_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::Name("missing".to_string())),
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_len_call_returns_int_for_a_concretely_bound_list() {
        // `lst`'s binding is a directly concrete `TypeTerm` (`Ok(Ty::List(_))`)
        // rather than one produced by `ListLiteral` (which always returns
        // `Ok(None)` in this solver, per its own comment above) -- this is
        // the only way to get a concrete `Ty::List` term to validate against
        // at constraint-collection time.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("lst".to_string(), Ok(Ty::List(Box::new(Ty::Int))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("lst".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_len_call_defers_an_unresolved_argument_to_the_real_check_pass() {
        // `lst`'s binding is a genuinely unresolved inference variable (a
        // fresh term, not yet unified with anything) -- this solver can't
        // tell yet whether it's a list, so it must not reject it here; the
        // real check pass (`infer_expr_in`) validates it once its type is
        // actually known.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let unresolved = fresh_term(&mut parents, &mut concrete);
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("lst".to_string(), unresolved)]),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("lst".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_len_call_rejects_the_wrong_arity() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![],
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`len` expects exactly 1 argument, got 0");
    }

    #[test]
    fn constraint_collection_len_call_rejects_a_concretely_known_non_list_argument() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::IntLiteral(5)],
        };

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `int`"
        );
    }

    #[test]
    fn collect_block_constraints_gives_a_for_list_loop_variable_a_fresh_term_when_unbound() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["i"],
            defs_rebound: HashSet::new(),
        };
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "lst".to_string(),
            body: vec![],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert!(env.bindings.contains_key("i"));
    }

    #[test]
    fn collect_block_constraints_keeps_a_for_list_loop_variable_s_existing_term() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::from([("i".to_string(), Ok(Ty::Int))]),
            local_names: &["i"],
            defs_rebound: HashSet::new(),
        };
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "lst".to_string(),
            body: vec![],
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap();

        assert_eq!(env.bindings.get("i"), Some(&Ok(Ty::Int)));
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_for_list_loop_body() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["i", "z"],
            defs_rebound: HashSet::new(),
        };
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "lst".to_string(),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("z".to_string()))],
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_private_helper_can_use_an_annotated_assignment_during_signature_inference() {
        // End-to-end: an annotated assignment inside a private helper whose
        // own signature is still `Ty::Infer` exercises the solver path
        // (`collect_block_constraints`) as well as the later concrete
        // re-check (`check_stmt_in_function`), not just the direct unit
        // tests above.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_helper".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::AnnAssign {
                    target: "x".to_string(),
                    annotation: Ty::Int,
                    value: Some(HirExpr::IntLiteral(1)),
                }],
            }],
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn constraint_collection_leaves_top_level_return_to_validation() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Return(Some(HirExpr::IntLiteral(1)))),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };

        assert_eq!(check(&hir).unwrap_err().code, "T0024");
    }

    #[test]
    fn a_cloned_environment_keeps_later_function_bindings_isolated() {
        let mut original = Environment::new();
        original.bind_function("original".to_string(), vec![Ty::Int], Ty::Int);

        let mut cloned = original.clone();
        cloned.bind_function("cloned".to_string(), vec![], Ty::None);

        assert!(original.lookup_function("cloned").is_none());
        assert_eq!(
            cloned.lookup_function("cloned"),
            Some(&(Vec::new(), Ty::None))
        );
        assert_eq!(
            original.lookup_function("original"),
            Some(&(vec![Ty::Int], Ty::Int))
        );
    }

    #[test]
    fn infers_an_int_literal_as_int() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::IntLiteral(1)), Ok(Ty::Int));
    }

    #[test]
    fn infers_a_float_literal_as_float() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::FloatLiteral(1.5)), Ok(Ty::Float));
    }

    #[test]
    fn infers_a_bool_literal_as_bool() {
        let env = Environment::new();
        assert_eq!(infer_expr(&env, &HirExpr::BoolLiteral(true)), Ok(Ty::Bool));
    }

    #[test]
    fn infers_a_string_literal_as_str() {
        let env = Environment::new();
        assert_eq!(
            infer_expr(&env, &HirExpr::StringLiteral("hi".to_string())),
            Ok(Ty::Str)
        );
    }

    #[test]
    fn adding_an_int_and_a_str_is_a_clean_type_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn adding_two_strings_infers_str() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::StringLiteral("b".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn subtracting_two_strings_is_a_clean_type_error() {
        // Python allows `"a" + "b"` but no other arithmetic operator between
        // two strings -- `"a" - "b"` is a `TypeError` at runtime in CPython.
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Sub,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::StringLiteral("b".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_two_strings_infers_bool() {
        // `"a" == "b"`, `"a" < "b"`, etc. are ordinary, valid Python
        // (lexicographic ordering) -- not covered by numeric_or_bool_compatible
        // before `Ty::Str` became constructible via literals.
        let env = Environment::new();
        for op in [
            CmpOpKind::Eq,
            CmpOpKind::NotEq,
            CmpOpKind::Lt,
            CmpOpKind::LtE,
            CmpOpKind::Gt,
            CmpOpKind::GtE,
        ] {
            let expr = HirExpr::Compare {
                op,
                left: Box::new(HirExpr::StringLiteral("a".to_string())),
                right: Box::new(HirExpr::StringLiteral("b".to_string())),
            };
            assert_eq!(
                infer_expr(&env, &expr),
                Ok(Ty::Bool),
                "comparison {op:?} should type-check"
            );
        }
    }

    #[test]
    fn an_f_string_always_infers_str_regardless_of_interpolated_types() {
        let env = Environment::new();
        let expr = HirExpr::FString(vec![
            FStringPart::Literal("n=".to_string()),
            FStringPart::Interpolation(Box::new(HirExpr::IntLiteral(1))),
        ]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn an_f_string_still_type_checks_its_interpolated_expressions() {
        let env = Environment::new();
        let expr = HirExpr::FString(vec![FStringPart::Interpolation(Box::new(HirExpr::Name(
            "undefined".to_string(),
        )))]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_a_string_and_an_int_is_a_clean_type_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::StringLiteral("a".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_two_ints_infers_bool() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Lt,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn comparing_a_bool_and_an_int_succeeds_since_bool_is_a_subtype_of_int() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Bool));
    }

    #[test]
    fn comparing_an_undefined_left_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::Name("undefined".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_an_undefined_right_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Name("undefined".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn comparing_incompatible_types_is_a_clean_type_error() {
        let mut env = Environment::new();
        // A call to a properly declared, zero-arg, `None`-returning function
        // legitimately infers `Ty::None`, which isn't numeric-like --
        // comparing an int against it is a genuine, both-sides-defined
        // incompatibility.
        env.bind_function("f".to_string(), vec![], Ty::None);
        let expr = HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Call {
                callee: "f".to_string(),
                args: vec![],
            }),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("int") && err.message.contains("None"));
    }

    #[test]
    fn a_binop_treats_bool_as_int() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::BoolLiteral(true)),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_binop_treats_bool_and_float_as_float() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::BoolLiteral(true)),
            right: Box::new(HirExpr::FloatLiteral(1.5)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn a_top_level_return_is_a_clean_diagnostic_not_a_panic() {
        // Regression test (self-review finding, pre-merge): this used to be
        // `panic!(...)`, so a bare `return` at module scope crashed the
        // compiler (exit code 101) instead of producing a diagnostic through
        // the documented exit-1 contract every other error path uses.
        // `ruff_python_parser` does not reject `return` outside a function at
        // the grammar level (CPython itself only rejects it in a later
        // compile pass), so this is reachable from ordinary CLI input.
        let mut env = Environment::new();
        let err = check_stmt(&mut env, &HirStmt::Return(None)).unwrap_err();
        assert_eq!(err.code, "T0024");
    }

    #[test]
    fn check_and_resolve_also_rejects_a_top_level_return_with_t0024() {
        // Regression test: `collect_block_constraints` (the private-helper
        // solver) is invoked over top-level statements with `return_term:
        // None` (no enclosing function), so a top-level `Return` hits its
        // own defensive `let Some(return_term) = return_term else {
        // continue }` arm and is silently skipped by the solver -- it's
        // `check_and_resolve`'s later, ordinary `check_stmt` pass (Pass 2)
        // that actually rejects it with T0024, exactly like the
        // `pycc_types::check` entry point already does.
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Return(None))],
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0024");
    }

    #[test]
    fn a_return_nested_in_a_top_level_if_is_also_a_clean_diagnostic() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Return(None)],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0024");
    }

    #[test]
    fn an_assignment_binds_the_inferred_type_in_the_environment() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_assignment_whose_value_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Name("undefined".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_incompatible_reassignment_is_t0023_and_preserves_the_inferred_type() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::StringLiteral("changed".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_annotated_assignment_with_a_matching_value_binds_the_annotation_type() {
        // x: int = True -- bool is assignable to int (matches is_assignable's
        // existing widening rule), and the environment should record Ty::Int
        // (the annotation), not Ty::Bool (the initializer's own inferred type).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::BoolLiteral(true)),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn an_annotated_assignment_with_a_mismatched_value_is_t0025() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::StringLiteral("nope".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0025");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_annotated_assignment_propagates_an_error_from_the_initializer_expression() {
        // The initializer itself can fail to type-check (here, referencing an
        // undefined name) before `is_assignable` is ever consulted -- the `?`
        // on `infer_expr` inside the new arm must propagate that error rather
        // than being reachable only via the is_assignable mismatch path.
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Name("undefined".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_annotation_only_declaration_does_not_bind_a_value_at_module_scope() {
        // x: int alone must only declare x, not bind it -- collect_local_names
        // registers `x` as local independently of this arm, so a premature
        // read still falls through to the existing T0021 mechanism (see the
        // function-scope regression test below for the end-to-end proof).
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn function_scope_annotated_assignment_with_a_matching_value_binds_the_annotation_type() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::BoolLiteral(true)),
            },
            Ty::None,
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn function_scope_annotated_assignment_with_a_mismatched_value_is_t0025() {
        let mut env = Environment::new();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::StringLiteral("nope".to_string())),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0025");
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn function_scope_annotated_assignment_propagates_an_error_from_the_initializer_expression() {
        // Same as the module-scope sibling above, but through
        // `infer_expr_in`'s local-vs-global unbound distinction: `x` is a
        // declared local that was never assigned, so referencing it in `y`'s
        // initializer must propagate T0021 via the new arm's `?` rather than
        // silently falling through to the is_assignable check.
        let mut env = Environment::new();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "y".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::Name("x".to_string())),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(env.lookup("y"), None);
    }

    #[test]
    fn function_scope_annotation_only_declaration_does_not_bind_a_value() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
            Ty::None,
        )
        .unwrap();
        assert_eq!(env.lookup("x"), None);
    }

    #[test]
    fn an_annotation_only_declaration_does_not_bind_a_value() {
        // x: int alone must not make a later use of x succeed -- it only
        // declares x local, it does not bind it (matching CPython's own
        // UnboundLocalError for this exact shape, verified during planning;
        // this end-to-end module mirrors
        // tests/diagnostics/d0026_annotation_only_unbound.py exactly).
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::AnnAssign {
                            target: "x".to_string(),
                            annotation: Ty::Int,
                            value: None,
                        },
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "print".to_string(),
                            args: vec![HirExpr::Name("x".to_string())],
                        }),
                    ],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "f".to_string(),
                    args: vec![],
                })),
            ],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        // Pin the exact message, not just the shared "T0021" code: this test's
        // entire purpose is proving the value-less form reaches the
        // *local-unbound* T0021 path (`unbound_local`), not the differently
        // worded *undefined-name* T0021 path -- both share the same code, so
        // only the message distinguishes them.
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn an_annotated_re_declaration_that_conflicts_with_an_existing_binding_is_t0023() {
        // `x`'s representation is fixed by its first assignment (Int) --
        // `pycc_mir`'s own `bind_variable` keeps that first type regardless of
        // a later compatible reassignment, so the checker must reject a later
        // annotated re-declaration that would otherwise silently repoint `x`
        // at an incompatible representation (here, `str`) via `is_assignable`
        // trivially comparing the initializer to its own annotation alone.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        let err = check_stmt(
            &mut env,
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: Some(HirExpr::StringLiteral("s".to_string())),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn function_scope_annotated_re_declaration_that_conflicts_with_an_existing_binding_is_t0023() {
        let mut env = Environment::new();
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Str,
                value: Some(HirExpr::StringLiteral("s".to_string())),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn assigning_bool_to_an_int_binding_keeps_the_declared_representation() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::BoolLiteral(true),
            },
        )
        .unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_for_target_cannot_change_an_existing_binding_representation() {
        let mut env = Environment::new();
        env.bind("value".to_string(), Ty::Str);
        let err = check_stmt(
            &mut env,
            &HirStmt::ForRange {
                var: "value".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(env.lookup("value"), Some(Ty::Str));
    }

    #[test]
    fn a_for_target_cannot_change_a_parameter_representation() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "loop_over".to_string(),
                params: vec![("value".to_string(), Ty::Str)],
                return_ty: Ty::None,
                body: vec![HirStmt::ForRange {
                    var: "value".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn direct_function_check_rejects_a_for_target_representation_change() {
        let function = HirItem::Function {
            name: "loop_over".to_string(),
            params: vec![("value".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "value".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_private_for_target_infers_an_unannotated_parameter_as_int() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_loop".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::None,
                body: vec![HirStmt::ForRange {
                    var: "value".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }],
            }],
        };
        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_loop".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::None,
                body: vec![HirStmt::ForRange {
                    var: "value".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }],
            }
        );
    }

    #[test]
    fn an_if_s_test_must_be_bool_like_and_both_branches_are_checked() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
            orelse: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::IntLiteral(2),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        // Both branches ran in the same (single, unscoped-per-branch)
        // environment for v0.1's simplified model -- neither branch's
        // bindings are undone; real flow-sensitive narrowing is out of scope.
        assert_eq!(env.lookup("x"), Some(Ty::Int));
        assert_eq!(env.lookup("y"), Some(Ty::Int));
    }

    #[test]
    fn an_if_whose_test_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::Name("undefined".to_string()),
            body: vec![],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            orelse: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_whose_orelse_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![],
            orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_loop_s_test_and_body_are_checked() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_while_loop_whose_test_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::Name("undefined".to_string()),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_binds_its_variable_as_int_and_checks_its_body() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::Name("i".to_string()),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_for_range_loop_whose_start_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::Name("undefined".to_string()),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_stop_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::Name("undefined".to_string()),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_step_is_undefined_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::Name("undefined".to_string()),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_loop_rejects_a_non_int_operand() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::StringLiteral("three".to_string()),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range stop"));
        assert_eq!(env.lookup("i"), None);
    }

    #[test]
    fn a_for_range_loop_accepts_bool_as_an_int_subtype() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForRange {
            var: "i".to_string(),
            start: HirExpr::BoolLiteral(false),
            stop: HirExpr::IntLiteral(3),
            step: HirExpr::BoolLiteral(true),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_loop_binds_its_variable_as_the_list_element_type_and_checks_its_body() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("i".to_string()),
            }],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
        assert_eq!(env.lookup("y"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_loop_binds_its_variable_as_str_for_a_list_of_str() {
        // Proves the `ForList` inference isn't hardcoded to `Ty::Int` --
        // it's generic over whatever scalar element type the list actually
        // has (see the `HirExpr::Subscript`/`ListAppend` genericity notes).
        // Bound directly rather than via a `str` list literal: T0034 gates
        // every non-`int` list literal at creation time, so no source-level
        // program can ever produce a live `Ty::List(Ty::Str)` binding --
        // this is the only way to reach this arm with one.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Str)));
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Str));
    }

    #[test]
    fn a_for_dict_loop_binds_its_variable_as_the_key_type() {
        // PR-11 Task 3 (D-123): `for k in d:` iterates a dict's own keys, so
        // `var` binds as the dict's key type (`Ty::Str`), not its value type.
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let stmt = HirStmt::ForList {
            var: "k".to_string(),
            list: "d".to_string(),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("k"), Some(Ty::Str));
    }

    #[test]
    fn a_for_set_loop_binds_its_variable_as_the_element_type() {
        // PR-11 Task 7 (D-123): `for x in s:` iterates a set's own elements,
        // so `var` binds as the set's element type (`Ty::Int` for
        // `set[int]`).
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let stmt = HirStmt::ForList {
            var: "x".to_string(),
            list: "s".to_string(),
            body: vec![],
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_loop_over_an_undefined_list_is_rejected() {
        let mut env = Environment::new();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "undefined".to_string(),
            body: vec![],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn a_for_list_loop_over_a_non_list_value_is_rejected() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(5),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "x".to_string(),
            body: vec![],
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_for_list_loop_whose_body_statement_is_ill_typed_propagates_the_error() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_list_loop_rejects_a_conflicting_reassignment_of_its_variable() {
        // `i` is already bound to `str` before the loop -- `check_assignment`
        // itself (not the list/element-type logic above it) is what rejects
        // rebinding it to the list's `int` element type.
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "i".to_string(),
                value: HirExpr::StringLiteral("not an int".to_string()),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "xs".to_string(),
                value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
            },
        )
        .unwrap();
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![],
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    // -- PR-12 Task 3 (D-117): comprehension type-checking, module scope --

    #[test]
    fn a_list_comprehension_over_range_type_checks_and_binds_target_as_list_int() {
        // Also pins that `var` is bound (via `check_assignment`) *before*
        // `elt` is checked: `elt` references the loop variable, so this
        // would fail as an unbound local if the ordering were wrong.
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::List(Box::new(Ty::Int))));
        assert_eq!(env.lookup("0comp_11_i"), Some(Ty::Int));
    }

    #[test]
    fn check_accepts_a_module_level_comprehension_whose_target_is_read_afterward() {
        // End-to-end pipeline pin, distinct from the hand-held-`Environment`
        // test above: every other comprehension test in this file either
        // drives `check_stmt`/`check_function` directly against a throwaway
        // `Environment`, or runs the full `check`/`check_and_resolve`
        // pipeline over a module expected to *fail*. This is the one test
        // confirming the full `check` pipeline (`module_function_names` ->
        // `concrete_function_environment` -> `check_with_environment` ->
        // `check_stmt`) accepts a *valid* module-level comprehension and
        // that its `target` binding genuinely survives into the next
        // top-level statement's environment, exactly like an ordinary
        // `Assign`'s binding already does.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "0comp_11_i".to_string(),
                    iter: CompIter::Range {
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "len".to_string(),
                    args: vec![HirExpr::Name("xs".to_string())],
                })),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_list_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks() {
        // Mirrors `If`/`While`: any type is accepted as truthy, no static
        // bool-convertibility restriction.
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn a_list_comprehension_producing_str_is_rejected_as_t0034() {
        // Mirrors `ListLiteral`'s own existing genericity tests
        // (`a_homogeneous_non_int_list_literal_is_rejected_as_t0034`).
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[str]"));
    }

    #[test]
    fn a_list_comprehension_producing_float_is_rejected_as_t0034() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::FloatLiteral(1.5)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[float]"));
    }

    #[test]
    fn a_list_comprehension_producing_bool_is_rejected_as_t0034() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::BoolLiteral(true)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[bool]"));
    }

    #[test]
    fn a_list_comprehension_over_a_bare_list_name_type_checks_and_binds_target_as_list_int() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::List` (the `ListCompAssign` tests above all use
        // `CompIter::Range`, never a bare-name list iterable).
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_20_x".to_string(),
            iter: CompIter::Name("xs".to_string()),
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_20_x".to_string())),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::List(Box::new(Ty::Int))));
        assert_eq!(env.lookup("0comp_20_x"), Some(Ty::Int));
    }

    #[test]
    fn a_set_comprehension_over_a_bare_set_name_type_checks_and_binds_target_as_set_int() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Set`.
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_20_x".to_string(),
            iter: CompIter::Name("s".to_string()),
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_20_x".to_string())),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::Set(Box::new(Ty::Int))));
        assert_eq!(env.lookup("0comp_20_x"), Some(Ty::Int));
    }

    #[test]
    fn a_set_comprehension_producing_a_non_int_element_is_rejected_as_t0038() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::StringLiteral("x".to_string())),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0038");
        assert!(err.message.contains("set[str]"));
    }

    #[test]
    fn a_set_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(env.lookup("y"), Some(Ty::Set(Box::new(Ty::Int))));
    }

    #[test]
    fn a_dict_comprehension_over_a_bare_dict_name_type_checks_and_binds_target_as_dict_str_int() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Dict`, binding `var` as the dict's *key* type (mirroring
        // `ForList`'s own `for k in d:` behavior, D-123).
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_20_k".to_string(),
            iter: CompIter::Name("d".to_string()),
            cond: None,
            key: Box::new(HirExpr::Name("0comp_20_k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(
            env.lookup("y"),
            Some(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
        assert_eq!(env.lookup("0comp_20_k"), Some(Ty::Str));
    }

    #[test]
    fn a_dict_comprehension_producing_a_non_str_int_pair_is_rejected_as_t0036() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::IntLiteral(1)),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0036");
        assert!(err.message.contains("dict[int, int]"));
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        check_stmt(&mut env, &stmt).unwrap();
        assert_eq!(
            env.lookup("y"),
            Some(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
    }

    #[test]
    fn a_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033() {
        // Reuses `ForList`'s own existing message
        // (`a_for_list_loop_over_a_non_list_value_is_rejected`).
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("x".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_comprehension_over_an_undefined_iterable_name_is_rejected_as_not_defined() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("undefined".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn an_entirely_unannotated_private_helper_containing_a_comprehension_fails_with_t0021() {
        // Per D-116's own correction note: a container-literal assignment's
        // target never receives a solver binding at all (the solver only
        // unifies scalar `Ty::Infer` parameters/returns) -- a comprehension's
        // own `target` gets the identical treatment (Step 5's deliberate
        // no-op in `collect_block_constraints`). `def _h(): xs = (10, 20);
        // return xs` already fails T0021 "local name `t` is not bound before
        // this use" for a bare tuple/list assignment reaching a `Return` --
        // this reproduces the same shape with a comprehension in place of a
        // literal, confirming it is the same pre-existing gap, not a new one
        // introduced by this statement.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "xs".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                    },
                    HirStmt::Return(Some(HirExpr::Name("xs".to_string()))),
                ],
            }],
        };
        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    // `resolve_comp_iter`'s own `CompIter::Range` operand checks (shared by
    // every comprehension kind and both scopes) -- exercised once here via
    // a list comprehension, mirroring `a_for_range_loop_whose_start_is_
    // undefined_propagates_the_error` and its stop/step siblings.

    #[test]
    fn a_comprehension_over_a_range_with_a_non_int_start_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::StringLiteral("x".to_string()),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range start expects"));
    }

    #[test]
    fn a_comprehension_over_a_range_with_a_non_int_stop_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::StringLiteral("x".to_string()),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range stop expects"));
    }

    #[test]
    fn a_comprehension_over_a_range_with_a_non_int_step_propagates_the_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::StringLiteral("x".to_string()),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("range step expects"));
    }

    // Per-arm error-propagation pins (module scope): each of `resolve_comp_iter`'s
    // call, the loop-variable `check_assignment`, `cond`, and `elt`/`key`/`value`
    // is its own `?`-guarded call site in each of the three comprehension arms,
    // so each needs its own error-propagating test for D-014's coverage gate,
    // distinct from the "rejected with a specific diagnostic" tests above.

    #[test]
    fn a_set_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("x".to_string()),
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_dict_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Name("x".to_string()),
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = check_stmt(&mut env, &stmt).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_list_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable() {
        let mut env = Environment::new();
        env.bind("0comp_11_i".to_string(), Ty::Str);
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_set_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable() {
        let mut env = Environment::new();
        env.bind("0comp_11_i".to_string(), Ty::Str);
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_dict_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable() {
        let mut env = Environment::new();
        env.bind("0comp_11_i".to_string(), Ty::Str);
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_list_comprehension_s_if_filter_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_if_filter_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            elt: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_list_comprehension_s_elt_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::ListCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_elt_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::SetCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_key_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::Name("undefined".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_value_propagates_an_ill_typed_error() {
        let mut env = Environment::new();
        let stmt = HirStmt::DictCompAssign {
            target: "y".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::StringLiteral("k".to_string())),
            value: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(check_stmt(&mut env, &stmt).unwrap_err().code, "T0021");
    }

    #[test]
    fn referencing_an_assigned_name_infers_its_bound_type() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap();
        assert_eq!(
            infer_expr(&env, &HirExpr::Name("x".to_string())),
            Ok(Ty::Int)
        );
    }

    #[test]
    fn adding_two_ints_infers_int() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_binop_with_an_undefined_left_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::Name("undefined".to_string())),
            right: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_binop_with_an_undefined_right_operand_propagates_the_error() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::Name("undefined".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn numeric_result_type_covers_every_int_float_combination() {
        assert_eq!(
            numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Float),
            Ok(Ty::Float)
        );
        assert_eq!(
            numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Int),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn true_division_of_two_ints_infers_float() {
        assert_eq!(
            numeric_result_type(BinOpKind::Div, Ty::Int, Ty::Int),
            Ok(Ty::Float)
        );
        assert_eq!(
            numeric_result_type(BinOpKind::Div, Ty::Bool, Ty::Bool),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn floor_division_of_two_ints_still_infers_int() {
        assert_eq!(
            numeric_result_type(BinOpKind::FloorDiv, Ty::Int, Ty::Int),
            Ok(Ty::Int)
        );
    }

    #[test]
    fn referencing_an_undefined_name_is_a_clean_error_not_a_panic() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::Name("undefined".to_string())).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined"));
    }

    #[test]
    fn numeric_result_type_rejects_a_hypothetical_incompatible_pair() {
        let err = numeric_result_type(BinOpKind::Add, Ty::Int, Ty::None).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn adding_an_int_and_a_float_promotes_to_float() {
        let env = Environment::new();
        let expr = HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(HirExpr::FloatLiteral(2.5)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn numeric_result_type_accepts_float_and_bool_since_bool_is_numeric_like() {
        // Task 7 makes `bool` numeric-like everywhere (`True + 1.5 == 2.5` is
        // legal Python), so this pair is no longer an error -- see
        // `a_binop_treats_bool_and_float_as_float` for the `infer_expr`-level
        // version of this same rule.
        assert_eq!(
            numeric_result_type(BinOpKind::Add, Ty::Float, Ty::Bool),
            Ok(Ty::Float)
        );
    }

    #[test]
    fn numeric_result_type_rejects_a_float_and_a_hypothetical_none() {
        // Exercises `.name()` for `Float` in the error arm now that
        // `Float`+`Bool` no longer takes that path.
        let err = numeric_result_type(BinOpKind::Add, Ty::Float, Ty::None).unwrap_err();
        assert!(err.message.contains("float") && err.message.contains("None"));
    }

    #[test]
    fn numeric_result_type_rejects_a_hypothetical_str_operand() {
        let err = numeric_result_type(BinOpKind::Add, Ty::Bool, Ty::Str).unwrap_err();
        assert!(err.message.contains("str"));
    }

    #[test]
    fn an_empty_list_literal_cannot_be_inferred() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::ListLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty list literal"));
    }

    #[test]
    fn a_homogeneous_int_list_literal_infers_list_of_int() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn a_heterogeneous_list_literal_is_rejected_as_t0032() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::StringLiteral("two".to_string()),
            HirExpr::IntLiteral(3),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0032");
        // Exact match, not a substring check: this is the same message text
        // baked into `tests/diagnostics/d0032_heterogeneous_list_literal
        // .expected.txt`, which can't currently run end-to-end (`pycc_mir`
        // is still non-exhaustive against Task 7's HIR forms) -- this unit
        // test is what actually guards that fixture's wording today.
        assert_eq!(
            err.message,
            "list element type mismatch: expected int (from the first element), found str"
        );
    }

    #[test]
    fn a_list_literal_of_bool_and_int_is_rejected_since_homogeneity_uses_exact_ty_equality() {
        // D-105 requires the *exact same* `Ty` for every element -- unlike
        // `is_assignable`'s bool-is-an-int-subtype rule used elsewhere in
        // this file, `[1, True]` is still T0032.
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1), HirExpr::BoolLiteral(true)]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0032");
    }

    #[test]
    fn a_homogeneous_non_int_list_literal_is_rejected_as_t0034() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::StringLiteral("b".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        // Exact match: same wording as
        // `tests/diagnostics/d0034_list_element_type_not_int.expected.txt`
        // (currently unrunnable end-to-end -- see the T0032 test above).
        assert_eq!(
            err.message,
            "list[str] is not compiled yet (D-105) -- only list[int] is"
        );
    }

    #[test]
    fn a_list_literal_propagates_an_ill_typed_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::ListLiteral(vec![HirExpr::Name("undefined".to_string())]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn subscripting_a_list_infers_its_element_type() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn subscripting_a_list_of_str_infers_str_proving_subscript_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn subscripting_with_a_bool_index_is_accepted_since_bool_is_an_int_subtype() {
        // D-086: `bool` is accepted wherever `int` is expected at an operand
        // boundary (mirroring `is_assignable`'s existing param/assignment
        // rule) -- `xs[True]` is ordinary, CPython-valid Python (PEP 285),
        // not a type error. Found by an automated PR review (PR #236) and
        // confirmed against a real `pycc build` before this fix.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn subscripting_with_a_non_int_index_is_rejected() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::StringLiteral("zero".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("list index must be"));
    }

    #[test]
    fn subscripting_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        // Exact match: same wording as
        // `tests/diagnostics/d0033_subscript_on_non_list.expected.txt`
        // (currently unrunnable end-to-end -- see the T0032 test above).
        assert_eq!(err.message, "`int` does not support indexing");
    }

    #[test]
    fn subscripting_a_non_list_value_with_a_non_int_index_reports_t0033_not_t0021() {
        // Both operands are ill-typed at once: the base doesn't support
        // indexing at all, AND the index isn't int-compatible. T0033
        // ("does this value support the operation at all") must fire
        // before T0021 ("is this operand's type wrong"). The `match
        // base_ty { Ty::List(..) => ..., Ty::Dict(..) => ..., Ty::Tuple(..)
        // => ..., other => T0033 }` dispatch above already gives this
        // precedence structurally -- a non-container base falls straight
        // to the `other` arm without ever reaching a container's own
        // index-type check -- but PR-10's own final whole-branch review
        // found and pinned this exact compound case as a regression test
        // before container types existed, so it is kept here too.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::StringLiteral("zero".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`int` does not support indexing");
    }

    #[test]
    fn subscripting_propagates_an_ill_typed_base_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("undefined".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn subscripting_propagates_an_ill_typed_index_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // PR-12 Task 7 (D-118): `HirExpr::Slice` type-checking. Only
    // `list[int]` ships slicing in v0.2 -- see the arm's own doc comment in
    // `infer_expr_in` for the full base-type/element-type/bound-type
    // diagnostic-order rationale.

    #[test]
    fn slicing_a_list_of_int_with_both_bounds_infers_list_of_int() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_start_omitted_type_checks() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: Some(Box::new(HirExpr::IntLiteral(1))),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_stop_omitted_type_checks() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: None,
            step: Some(Box::new(HirExpr::IntLiteral(1))),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_step_omitted_type_checks() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::IntLiteral(1))),
            stop: Some(Box::new(HirExpr::IntLiteral(3))),
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_with_every_bound_omitted_type_checks_as_xs_colon() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_a_dict_is_rejected_as_t0033() {
        // Mirrors real CPython: `d[1:2]` raises `TypeError` there too (D-118
        // -- dict/set reuse `Subscript`'s own `T0033` code, not a new one).
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("d".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`dict[str, int]` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_set_is_rejected_as_t0033() {
        // Mirrors real CPython: `s[1:2]` raises `TypeError` there too.
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("s".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`set[int]` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_tuple_is_rejected_as_t0033_as_an_explicit_deferral() {
        // D-118: unlike dict/set (never supported for slicing), `tuple[...]`
        // slicing is a real, explicit v0.2 scope cut -- but it still reuses
        // the same `T0033` code, not a distinct one.
        let mut env = Environment::new();
        env.bind("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Int])));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("t".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`tuple[int, int]` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_scalar_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("x".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`int` does not support slicing (only list[int] does)"
        );
    }

    #[test]
    fn slicing_a_list_of_str_is_rejected_as_t0034() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert_eq!(
            err.message,
            "list codegen only supports `list[int]` in v0.2, cannot slice `list[str]`"
        );
    }

    #[test]
    fn slicing_a_list_of_float_is_rejected_as_t0034() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Float)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert_eq!(
            err.message,
            "list codegen only supports `list[int]` in v0.2, cannot slice `list[float]`"
        );
    }

    #[test]
    fn slicing_a_list_of_bool_is_rejected_as_t0034() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Bool)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert_eq!(
            err.message,
            "list codegen only supports `list[int]` in v0.2, cannot slice `list[bool]`"
        );
    }

    #[test]
    fn slicing_with_a_non_int_start_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::StringLiteral("a".to_string()))),
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "slice start must be `int`, got `str`");
    }

    #[test]
    fn slicing_with_a_non_int_stop_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: Some(Box::new(HirExpr::FloatLiteral(1.0))),
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "slice stop must be `int`, got `float`");
    }

    #[test]
    fn slicing_with_a_non_int_step_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: None,
            stop: None,
            step: Some(Box::new(HirExpr::StringLiteral("a".to_string()))),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "slice step must be `int`, got `str`");
    }

    #[test]
    fn slicing_with_a_bool_bound_is_accepted_since_bool_is_an_int_subtype() {
        // D-086, mirroring `Subscript`'s own index check: `xs[True:]` is
        // ordinary, CPython-valid Python (`bool` is an `int` subtype, PEP
        // 285), not a type error.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::BoolLiteral(true))),
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::List(Box::new(Ty::Int))));
    }

    #[test]
    fn slicing_reports_the_base_type_error_before_any_bound_error() {
        // D-118 diagnostic-order pin: the base-type gate (T0033) fires
        // before a bound's own type is ever inspected.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("x".to_string())),
            start: Some(Box::new(HirExpr::StringLiteral("bad".to_string()))),
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
    }

    #[test]
    fn slicing_reports_the_element_type_error_before_any_bound_error() {
        // D-118 diagnostic-order pin: the element-type gate (T0034) fires
        // before a bound's own type is ever inspected.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::StringLiteral("bad".to_string()))),
            stop: None,
            step: None,
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0034");
    }

    #[test]
    fn slicing_propagates_an_ill_typed_base_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("undefined".to_string())),
            start: None,
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn slicing_propagates_an_ill_typed_bound_s_error() {
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Slice {
            base: Box::new(HirExpr::Name("xs".to_string())),
            start: Some(Box::new(HirExpr::Name("undefined".to_string()))),
            stop: None,
            step: None,
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn appending_a_matching_value_to_a_list_infers_none() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn appending_a_str_to_a_list_of_str_infers_none_proving_append_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::StringLiteral("a".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn appending_to_an_undefined_name_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::ListAppend {
            list: "undefined".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn appending_to_a_local_name_read_before_assignment_is_unbound_local() {
        // `ListAppend`'s `list` field is a plain `String`, not an
        // `HirExpr::Name`, so it must replicate `HirExpr::Name`'s own
        // "unbound local" vs. "not defined" distinction by hand
        // (`lookup_bound_name`) rather than going through `infer_expr_in`'s
        // `Name` arm.
        let env = Environment::new();
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr_in(&env, &["x"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn appending_to_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.append()`"));
    }

    #[test]
    fn appending_a_mismatched_value_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::StringLiteral("nope".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot append"));
    }

    #[test]
    fn appending_a_bool_to_a_list_of_int_is_accepted_since_bool_is_an_int_subtype() {
        // D-086, matching Subscript's own index check above: `x.append(True)`
        // on an already-typed `list[int]` is ordinary, CPython-valid Python
        // (`bool` is an `int` subtype) -- `is_assignable` applies here, not
        // ListLiteral's stricter exact-equality homogeneity rule (which
        // answers a different question: inferring an as-yet-unknown element
        // type, not checking a value against an already-known one). Found
        // by an automated whole-branch review (PR #236) and confirmed
        // against a real `pycc build` before this fix.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn appending_propagates_an_ill_typed_value_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListAppend {
            list: "x".to_string(),
            value: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // -- PR-12 Task 10 (D-119): remaining container methods depth --------
    // `list.pop()`, `dict.get(key, default)`, `set.add(value)` -- each
    // mirrors `ListAppend`'s own `infer_expr_in` test coverage exactly
    // (base-type gate, value/key/default-type gate, `lookup_bound_name`'s
    // own "not defined"/"unbound local" distinction, propagation, D-086
    // bool-subtypes-int leniency, genericity).

    #[test]
    fn popping_the_last_element_of_a_list_of_int_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn popping_from_a_list_of_str_infers_str_proving_pop_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Str));
    }

    #[test]
    fn popping_from_an_undefined_name_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::ListPop {
            list: "undefined".to_string(),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn popping_from_a_local_name_read_before_assignment_is_unbound_local() {
        let env = Environment::new();
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        let err = infer_expr_in(&env, &["x"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn popping_from_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::ListPop {
            list: "x".to_string(),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.pop()`"));
    }

    #[test]
    fn getting_a_str_key_with_a_matching_int_default_infers_int() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn getting_a_bool_default_for_a_dict_of_int_values_is_accepted_since_bool_is_an_int_subtype() {
        // D-086, matching `ListAppend`'s own leniency: `bool` is an `int`
        // subtype, so a `bool` default for a `dict[str, int]` is ordinary,
        // CPython-valid Python, not a type error.
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn getting_from_an_undefined_dict_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::DictGetOrDefault {
            dict: "undefined".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn getting_from_a_local_dict_read_before_assignment_is_unbound_local() {
        let env = Environment::new();
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr_in(&env, &["d"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn getting_from_a_non_dict_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Int);
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.get()`"));
    }

    #[test]
    fn getting_with_a_mismatched_key_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::IntLiteral(1)),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot look up"));
    }

    #[test]
    fn getting_with_a_mismatched_default_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::StringLiteral("nope".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot use"));
    }

    #[test]
    fn getting_propagates_an_ill_typed_key_s_error() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::Name("undefined".to_string())),
            default: Box::new(HirExpr::IntLiteral(0)),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn getting_propagates_an_ill_typed_default_s_error() {
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::DictGetOrDefault {
            dict: "d".to_string(),
            key: Box::new(HirExpr::StringLiteral("a".to_string())),
            default: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn adding_an_int_value_to_a_set_of_int_infers_none() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn adding_a_bool_value_to_a_set_of_int_is_accepted_since_bool_is_an_int_subtype() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::BoolLiteral(true)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn adding_a_value_to_a_set_of_str_infers_none_proving_add_is_not_int_specific() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Str)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::StringLiteral("a".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn adding_to_an_undefined_set_is_rejected_as_not_defined() {
        let env = Environment::new();
        let expr = HirExpr::SetAdd {
            set: "undefined".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn adding_to_a_local_set_read_before_assignment_is_unbound_local() {
        let env = Environment::new();
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr_in(&env, &["s"], &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn adding_to_a_non_set_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Int);
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("does not support `.add()`"));
    }

    #[test]
    fn adding_a_mismatched_value_type_is_rejected_as_t0021() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::StringLiteral("nope".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("cannot add"));
    }

    #[test]
    fn adding_propagates_an_ill_typed_value_s_error() {
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::SetAdd {
            set: "s".to_string(),
            value: Box::new(HirExpr::Name("undefined".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // Whole-module `check()` tests, mirroring `HirExpr::Slice`'s own pair
    // (PR-12 Task 7, D-118) exactly: Task 3's own ledger records a real
    // regression of exactly this shape (a solver arm that looked correct in
    // isolation but was never actually reached by the block walker) -- these
    // confirm each new arm is genuinely wired into both the fast
    // (`check_with_environment`) and solver (`collect_block_constraints`)
    // paths, not just correct when `infer_expr`/`collect_expr_constraints`
    // are called directly on a hand-built expression.

    #[test]
    fn list_pop_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::ListPop {
                        list: "xs".to_string(),
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn dict_get_or_default_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path()
     {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "d".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::IntLiteral(1),
                    )]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::DictGetOrDefault {
                        dict: "d".to_string(),
                        key: Box::new(HirExpr::StringLiteral("a".to_string())),
                        default: Box::new(HirExpr::IntLiteral(0)),
                    },
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn set_add_type_checks_correctly_when_an_unrelated_private_helper_forces_the_solver_path() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "s".to_string(),
                    value: HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::SetAdd {
                    set: "s".to_string(),
                    value: Box::new(HirExpr::IntLiteral(2)),
                })),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn list_pop_assigned_inside_an_unannotated_private_helper_hits_the_pre_existing_solver_binding_gap_today()
     {
        // Pins a pre-existing, tracked gap (D-116's correction, "Three
        // concrete reproductions") -- NOT introduced or fixed by this task.
        // `collect_expr_constraints`'s `ListPop` arm returns `Ok(None)` (no
        // unification term), mirroring `ListAppend`/every container-literal
        // arm exactly (D-116's own stated reasoning: this solver only
        // infers scalar `Ty::Infer` parameters/returns, and a container-
        // producing expression has no unification-friendly term to give).
        // Once any function in the module is unannotated, EVERY function
        // (including this one) is checked via the solver path, whose
        // `collect_block_constraints`' `Assign` arm never registers a
        // binding for a target assigned from an `Ok(None)`-producing
        // expression -- so a *later* read of that target inside the same
        // solver-path function fails with the actively misleading
        // "not bound before this use", even though the assignment is
        // textually right above it. D-116's own three reproductions were
        // all container-*typed* (`tuple`/`list` literals); `.pop()` is
        // NOT the first *scalar*-typed expression to reach this same gap,
        // though -- `HirExpr::Subscript`'s own `collect_expr_constraints`
        // arm (predates D-116/D-119 entirely, commit `0930903`) already
        // returns `Ok(None)` the same way, so plain `xs[0]` indexing already
        // hit this exact failure mode first. This test just pins that
        // `.pop()` is another, independently confirmed instance of the same
        // pre-existing class, not a new or different one. Fixing this is
        // out of Task 10's scope (frontend+types only) -- tracked as
        // `docs/ROADMAP.md`'s existing D-116 follow-up, unaffected by this
        // task.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::Assign {
                        target: "y".to_string(),
                        value: HirExpr::ListPop {
                            list: "xs".to_string(),
                        },
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn dict_get_or_default_assigned_inside_an_unannotated_private_helper_hits_the_pre_existing_solver_binding_gap_today()
     {
        // Same pre-existing D-116 gap as the `ListPop` test immediately
        // above, exercised through `DictGetOrDefault`'s own
        // `collect_expr_constraints` arm (also `Ok(None)`) instead.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))))],
                return_ty: Ty::Infer,
                body: vec![
                    HirStmt::Assign {
                        target: "y".to_string(),
                        value: HirExpr::DictGetOrDefault {
                            dict: "d".to_string(),
                            key: Box::new(HirExpr::StringLiteral("a".to_string())),
                            default: Box::new(HirExpr::IntLiteral(0)),
                        },
                    },
                    HirStmt::Return(Some(HirExpr::Name("y".to_string()))),
                ],
            }],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn len_of_a_list_of_int_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn len_of_a_list_of_str_infers_int_proving_len_is_not_int_specific() {
        // Proves `len`'s own check isn't hardcoded to `Ty::Int` -- generic
        // over any scalar element type, same discipline already applied to
        // `Subscript`/`ListAppend`/`ForList` (D-105's own genericity claim).
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Str)));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn len_with_no_arguments_is_rejected_as_t0033() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`len` expects exactly 1 argument, got 0");
    }

    #[test]
    fn len_with_two_arguments_is_rejected_as_t0033() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`len` expects exactly 1 argument, got 2");
    }

    #[test]
    fn len_of_a_non_list_value_is_rejected_as_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(
            err.message,
            "`len` expects a `list[T]`, `dict[K, V]`, or `set[T]` argument, got `int`"
        );
    }

    #[test]
    fn len_propagates_an_ill_typed_argument_s_error() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    // -- PR-11 Task 3 (D-123): dict[str, int] type-checking --------------

    #[test]
    fn an_empty_dict_literal_cannot_be_inferred() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::DictLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty dict literal"));
    }

    #[test]
    fn a_homogeneous_dict_literal_infers_dict_of_str_int() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::StringLiteral("b".to_string()),
                HirExpr::IntLiteral(2),
            ),
        ]);
        assert_eq!(
            infer_expr(&env, &expr),
            Ok(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
    }

    #[test]
    fn a_dict_literal_with_mismatched_value_types_is_rejected_as_t0035() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::StringLiteral("b".to_string()),
                HirExpr::StringLiteral("oops".to_string()),
            ),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0035");
        assert_eq!(
            err.message,
            "dict entry type mismatch: expected str: int (from the first pair), found str: str"
        );
    }

    #[test]
    fn a_dict_literal_with_mismatched_key_types_is_rejected_as_t0035() {
        // Same code as the value-mismatch case above, but exercises the
        // `this_key_ty != key_ty` half of the homogeneity check's `||`
        // independently of the value half.
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (HirExpr::IntLiteral(2), HirExpr::IntLiteral(2)),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0035");
        assert_eq!(
            err.message,
            "dict entry type mismatch: expected str: int (from the first pair), found int: int"
        );
    }

    #[test]
    fn a_homogeneous_non_str_int_dict_literal_is_rejected_as_t0036() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::StringLiteral("b".to_string()),
        )]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0036");
        assert_eq!(
            err.message,
            "dict[str, str] is not compiled yet (D-122) -- only dict[str, int] is"
        );
    }

    #[test]
    fn a_bad_dict_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The dict-shaped counterpart to
        // `a_heterogeneous_list_literal_is_still_rejected_when_the_solver_path_runs_first`
        // above: `collect_expr_constraints`'s own `DictLiteral` arm always returns
        // `Ok(None)` (it has no unification-friendly representation for `Ty::Dict`
        // either), so an unrelated private helper with an unresolved (`Ty::Infer`)
        // signature must not let the solver's own leniency swallow a genuine T0036 --
        // it has to fall through to the real, dict-aware check pass.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::DictLiteral(vec![(
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::StringLiteral("b".to_string()),
                    )]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0036");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_first_key_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::Name("undefined".to_string()),
            HirExpr::IntLiteral(1),
        )]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_first_value_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::Name("undefined".to_string()),
        )]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_later_key_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::Name("undefined".to_string()),
                HirExpr::IntLiteral(2),
            ),
        ]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_literal_propagates_an_ill_typed_later_value_s_error() {
        let env = Environment::new();
        let expr = HirExpr::DictLiteral(vec![
            (
                HirExpr::StringLiteral("a".to_string()),
                HirExpr::IntLiteral(1),
            ),
            (
                HirExpr::StringLiteral("b".to_string()),
                HirExpr::Name("undefined".to_string()),
            ),
        ]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn subscripting_a_dict_infers_its_value_type() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::StringLiteral("a".to_string())),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn subscripting_a_dict_with_a_mismatched_key_type_is_rejected() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "dict key type mismatch: expected `str`, found `int`"
        );
    }

    #[test]
    fn len_of_a_dict_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn constraint_collection_len_call_returns_int_for_a_concretely_bound_dict() {
        // Mirrors `constraint_collection_len_call_returns_int_for_a_concretely_bound_list`
        // above, proving the solver's own relaxed `len()` arm (PR-11 Task 3)
        // also accepts a concretely-bound `Ty::Dict` term.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([(
                "d".to_string(),
                Ok(Ty::Dict(Box::new((Ty::Str, Ty::Int)))),
            )]),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("d".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_treats_a_dict_literal_as_unconstrained_but_recurses_into_pairs() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::IntLiteral(1),
        )]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_literal_key() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::Name("missing".to_string()),
            HirExpr::IntLiteral(1),
        )]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_dict_literal_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::DictLiteral(vec![(
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::Name("missing".to_string()),
        )]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn collect_block_constraints_recurses_into_a_dict_set_s_key_and_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];

        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap();
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_set_key() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::Name("missing".to_string()),
            value: HirExpr::IntLiteral(1),
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_set_value() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let mut env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::Name("missing".to_string()),
        }];

        let err = collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            &body,
            None,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn local_name_collection_ignores_a_dict_set_target() {
        // `d[k] = v` mutates an existing binding's contents, not a name --
        // unlike `Assign`/`AnnAssign`/`ForList`, it must not be collected as
        // a new local.
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];
        assert_eq!(function_local_names(&params, &body), Vec::<&str>::new());
    }

    #[test]
    fn contains_return_treats_a_dict_set_as_not_a_return() {
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];
        assert!(!contains_return(&body));
    }

    #[test]
    fn block_always_returns_treats_a_dict_set_as_not_always_returning() {
        let body = vec![HirStmt::DictSet {
            dict: "x".to_string(),
            key: HirExpr::StringLiteral("a".to_string()),
            value: HirExpr::IntLiteral(1),
        }];
        assert!(!block_always_returns(&body));
    }

    fn a_list_comp_assign_stmt() -> HirStmt {
        HirStmt::ListCompAssign {
            target: "xs".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        }
    }

    fn a_set_comp_assign_stmt() -> HirStmt {
        HirStmt::SetCompAssign {
            target: "xs".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
        }
    }

    fn a_dict_comp_assign_stmt() -> HirStmt {
        HirStmt::DictCompAssign {
            target: "xs".to_string(),
            var: "0comp_11_i".to_string(),
            iter: CompIter::Range {
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
            },
            cond: None,
            key: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            value: Box::new(HirExpr::IntLiteral(1)),
        }
    }

    #[test]
    fn local_name_collection_includes_a_list_comp_assign_s_target_and_synthesized_var() {
        // PR-12 Task 3 (D-117): unlike `DictSet` above, a comprehension
        // introduces two brand-new local names (`target` and the
        // synthesized `var`), not zero.
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![a_list_comp_assign_stmt()];
        let mut names = function_local_names(&params, &body);
        names.sort_unstable();
        assert_eq!(names, vec!["0comp_11_i", "xs"]);
    }

    #[test]
    fn local_name_collection_skips_a_list_comp_assign_s_target_and_var_when_already_local() {
        // Exercises the `!is_local(...)` guard's "already local" branch: a
        // parameter sharing the comprehension's `target`/`var` names must
        // not be pushed a second time.
        let params: Vec<(String, Ty)> = vec![
            ("xs".to_string(), Ty::Infer),
            ("0comp_11_i".to_string(), Ty::Infer),
        ];
        let body = vec![a_list_comp_assign_stmt()];
        let names = function_local_names(&params, &body);
        assert_eq!(names, vec!["xs", "0comp_11_i"]);
    }

    #[test]
    fn local_name_collection_includes_a_set_comp_assign_s_target_and_var() {
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![a_set_comp_assign_stmt()];
        let mut names = function_local_names(&params, &body);
        names.sort_unstable();
        assert_eq!(names, vec!["0comp_11_i", "xs"]);
    }

    #[test]
    fn local_name_collection_includes_a_dict_comp_assign_s_target_and_var() {
        let params: Vec<(String, Ty)> = vec![];
        let body = vec![a_dict_comp_assign_stmt()];
        let mut names = function_local_names(&params, &body);
        names.sort_unstable();
        assert_eq!(names, vec!["0comp_11_i", "xs"]);
    }

    #[test]
    fn contains_return_treats_a_list_comp_assign_as_not_a_return() {
        assert!(!contains_return(&[a_list_comp_assign_stmt()]));
    }

    #[test]
    fn contains_return_treats_a_set_comp_assign_as_not_a_return() {
        assert!(!contains_return(&[a_set_comp_assign_stmt()]));
    }

    #[test]
    fn contains_return_treats_a_dict_comp_assign_as_not_a_return() {
        assert!(!contains_return(&[a_dict_comp_assign_stmt()]));
    }

    #[test]
    fn block_always_returns_treats_a_list_comp_assign_as_not_always_returning() {
        assert!(!block_always_returns(&[a_list_comp_assign_stmt()]));
    }

    #[test]
    fn block_always_returns_treats_a_set_comp_assign_as_not_always_returning() {
        assert!(!block_always_returns(&[a_set_comp_assign_stmt()]));
    }

    #[test]
    fn block_always_returns_treats_a_dict_comp_assign_as_not_always_returning() {
        assert!(!block_always_returns(&[a_dict_comp_assign_stmt()]));
    }

    #[test]
    fn collect_block_constraints_treats_a_list_comp_assign_target_as_a_no_op() {
        // PR-12 Task 3 (D-117), Step 5: mirrors the dict/set/tuple-literal
        // solver-gap precedent directly -- `collect_block_constraints`'s
        // `ListCompAssign`/`SetCompAssign`/`DictCompAssign` arm registers no
        // term for `target` at all (though it does, unlike a bare no-op,
        // still recurse into `elt`/`cond` -- see
        // `private_helper_parameter_is_inferred_through_a_comprehension_s_elt`
        // below for that half). An unrelated private helper with an
        // unresolved (`Ty::Infer`) signature elsewhere in the same module
        // must not let the solver's own leniency swallow a genuine T0034 --
        // it has to fall through to the real, comprehension-aware check pass
        // (mirrors
        // `a_bad_dict_literal_is_still_rejected_when_the_solver_path_runs_first`).
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::ListCompAssign {
                    target: "xs".to_string(),
                    var: "0comp_11_i".to_string(),
                    iter: CompIter::Range {
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(3),
                        step: HirExpr::IntLiteral(1),
                    },
                    cond: None,
                    elt: Box::new(HirExpr::StringLiteral("x".to_string())),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0034");
    }

    #[test]
    fn private_helper_parameter_is_inferred_through_a_comprehension_s_elt() {
        // Regression test (pinned-reviewer finding, pre-merge, P1): an
        // earlier version of `collect_block_constraints`'s comprehension arm
        // was a bare no-op -- it never recursed into `iter`/`cond`/`elt`
        // at all, unlike every sibling arm in this function (`ForRange`,
        // `ForList`, `DictSet` above all still recurse into their own
        // sub-expressions even though they don't register a term for their
        // own container). That meant a call expression appearing only
        // inside a comprehension's `elt` never participated in this
        // solver's argument<->parameter unification (see
        // `private_parameter_is_inferred_by_forwarding_into_an_annotated_callee`
        // for the plain, non-comprehension version of this exact mechanism):
        // `_forward`'s unannotated `x` parameter, used only as an argument
        // to the fully-annotated `_sink`, used to spuriously fail with
        // "cannot infer type of parameter `x`; add an annotation" even
        // though `x: int` is the only type consistent with this program.
        // `bind_comp_loop_var` + recursing into `elt` (Step 5's actual fix)
        // makes this resolve correctly and reach the real check pass, which
        // accepts the program (`_sink` returns `int`, satisfying the list
        // comprehension's own `T0034` gate).
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(1),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::Call {
                                callee: "_sink".to_string(),
                                args: vec![HirExpr::Name("x".to_string())],
                            }),
                        },
                        HirStmt::Return(None),
                    ],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn bind_comp_loop_var_unifies_a_range_comprehension_s_loop_variable_with_an_existing_term() {
        // Exercises `bind_comp_loop_var`'s `unify_terms(existing, Ok(Ty::Int), ...)`
        // branch (mirrors `ForRange`'s own analogous branch, exercised by
        // `collect_block_constraints_unifies_a_for_range_loop_variable_s_existing_term`-
        // style coverage elsewhere in this file): the loop variable is
        // already bound to an inferred term (via forwarding into another
        // annotated helper) before the comprehension re-binds it to `Ty::Int`.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_sink".to_string(),
                            args: vec![HirExpr::Name("x".to_string())],
                        }),
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "x".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(1),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_unifies_a_range_comprehension_s_stop_forwarded_from_an_unannotated_parameter()
     {
        // Exercises `bind_comp_loop_var`'s `CompIter::Range` operand-check
        // branch (mirrors `ForRange`'s own analogous `start`/`stop`/`step`
        // checks): the comprehension's own `stop` operand is itself an
        // unresolved solver term (an unannotated parameter forwarded
        // directly as the range bound), which this arm must still visit and
        // unify against `Ty::Int` -- exactly like a plain `for i in
        // range(n):` loop's own `ForRange` arm already does.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![("n".to_string(), Ty::Infer)],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::Name("n".to_string()),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_rejects_a_range_comprehension_whose_stop_was_already_resolved_to_an_incompatible_type()
     {
        // The failure half of the operand-check branch above: `n` is first
        // forwarded into a `str`-typed parameter (resolving its solver term
        // to `Ty::Str`), then reused as the comprehension's own `stop`
        // operand -- `bind_comp_loop_var`'s `unify_terms(term, Ok(Ty::Int),
        // ...)` call must propagate the resulting conflict rather than
        // silently accepting it.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink_str".to_string(),
                    params: vec![("value".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("n".to_string(), Ty::Infer)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "_sink_str".to_string(),
                            args: vec![HirExpr::Name("n".to_string())],
                        }),
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::Name("n".to_string()),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_rejects_a_range_comprehension_whose_loop_variable_conflicts_with_an_existing_binding()
     {
        // Exercises `bind_comp_loop_var`'s `CompIter::Range` "existing
        // binding" branch's own *failure* path (mirrors `ForRange`'s own
        // `T0023` branch): the comprehension's loop variable happens, in
        // this hand-built test only, to share a name with the enclosing
        // function's own `str`-typed parameter (real lowering's
        // `synthesize_comp_var_name` never produces such a collision), so
        // `Range`'s `Ty::Int` fact genuinely conflicts with the existing
        // binding.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("0comp_11_i".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            elt: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn collect_block_constraints_gives_a_name_iterable_comprehension_s_loop_variable_a_fresh_term()
    {
        // Exercises `bind_comp_loop_var`'s `CompIter::Name` branch (mirrors
        // `ForList`'s own analogous branch): this solver doesn't track a
        // list-typed name's element type, so the loop variable just gets an
        // unconstrained fresh term here -- real element-type checking is
        // the subsequent `check_with_signatures` pass's job. Every other
        // solver-path comprehension test above uses `CompIter::Range`.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ListCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Name("xs".to_string()),
                            cond: None,
                            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_visits_a_set_comp_assign_s_cond_and_elt() {
        // The combined `HirStmt::ListCompAssign | HirStmt::SetCompAssign`
        // solver arm is only exercised with `ListCompAssign` and a `None`
        // `cond` by the tests above -- this pins the `SetCompAssign` half of
        // that pattern and the `cond.is_some()` branch, both distinct
        // coverage regions from the `ListCompAssign`/`cond: None` case.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::SetCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: Some(Box::new(HirExpr::IntLiteral(1))),
                            elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_visits_a_dict_comp_assign_s_cond_key_and_value() {
        // Pins the `HirStmt::DictCompAssign` solver arm (its own separate
        // key/value split, not shared with the list/set arm above), with a
        // `cond` present so its own `cond.is_some()` branch is covered too.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::DictCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: Some(Box::new(HirExpr::IntLiteral(1))),
                            key: Box::new(HirExpr::StringLiteral("k".to_string())),
                            value: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_visits_a_dict_comp_assign_s_key_and_value_with_no_cond() {
        // Companion to the test above: pins the `DictCompAssign` solver
        // arm's `cond.is_none()` branch, which that test's `cond: Some(...)`
        // shape never reaches.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::DictCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            key: Box::new(HirExpr::StringLiteral("k".to_string())),
                            value: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_range_comprehension_s_stop_operand() {
        // Pins `bind_comp_loop_var`'s own `collect_expr_constraints(...)?`
        // call for a `CompIter::Range` operand failing outright, distinct
        // from the operand-resolves-but-conflicts tests above. Uses a
        // forward reference to a name assigned *later* in the same body
        // (mirrors `private_helper_inference_rejects_a_read_before_local_assignment`)
        // rather than a plain undefined name: this solver's own
        // `collect_expr_constraints` is deliberately lenient about a
        // non-local undefined name (`None => Ok(None)`, no error -- real
        // "not defined" checking is `infer_expr_in`'s job in the second
        // pass), so only a genuine *local-but-not-yet-bound* read actually
        // triggers this solver's own `unbound_local` error.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::Name("later".to_string()),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(3),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_keeps_a_name_iterable_comprehension_s_loop_variable_s_existing_term()
     {
        // Companion to the "fresh term" test above: pins the
        // `CompIter::Name` branch's `contains_key(var)` guard's *true*
        // branch (variable already bound), mirroring
        // `collect_block_constraints_keeps_a_for_list_loop_variable_s_existing_term`'s
        // own analogous `ForList` coverage. The loop variable is given the
        // same name as a real parameter (`0comp_11_i`), whose own term is
        // seeded into the solver's environment before any statement in the
        // body runs -- unlike a plain `ExprStmt` reference to an
        // undeclared name (which this solver's own local-name tracking
        // would instead reject as unbound, never reaching the
        // comprehension at all).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![
                    ("0comp_11_i".to_string(), Ty::List(Box::new(Ty::Int))),
                    ("ys".to_string(), Ty::List(Box::new(Ty::Int))),
                ],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Name("ys".to_string()),
                        cond: None,
                        elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        // The real check pass still rejects this program overall (`0comp_11_i`
        // is a `list[int]`-typed parameter, and the comprehension tries to
        // rebind it to `int`) -- that is not what this test pins. What
        // matters is that the *solver's* own `bind_comp_loop_var` reaches
        // its `CompIter::Name` branch with `var` already a key in
        // `env.bindings` (from the parameter seeding) and takes the
        // "already bound, do nothing" path without itself erroring, which
        // is what lets the module-wide solver pass complete and fall
        // through to the real, comprehension-aware check pass that then
        // reports the actual `T0023` conflict.
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_list_comp_assign_s_cond() {
        // See the range-stop-operand test above for why a forward reference
        // to a same-body local, not a plain undefined name, is required to
        // exercise this solver's own error path.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: Some(Box::new(HirExpr::Name("later".to_string()))),
                        elt: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_list_comp_assign_s_elt() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ListCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        elt: Box::new(HirExpr::Name("later".to_string())),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_loop_variable_binding()
     {
        // Pins `DictCompAssign`'s own `bind_comp_loop_var(...)?` call site
        // (textually distinct from the list/set arm's own call site above)
        // failing outright, using the same loop-variable-collision shape as
        // the list-comprehension version above.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("0comp_11_i".to_string(), Ty::Str)],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::DictCompAssign {
                            target: "y".to_string(),
                            var: "0comp_11_i".to_string(),
                            iter: CompIter::Range {
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(3),
                                step: HirExpr::IntLiteral(1),
                            },
                            cond: None,
                            key: Box::new(HirExpr::StringLiteral("k".to_string())),
                            value: Box::new(HirExpr::IntLiteral(1)),
                        },
                        HirStmt::Return(None),
                    ],
                },
                HirItem::Function {
                    name: "_trigger".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0023");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_cond() {
        // See `collect_block_constraints_propagates_an_error_from_a_range_comprehension_s_stop_operand`
        // above for why a forward reference to a same-body local, not a
        // plain undefined name, is required to exercise this solver's own
        // error path (rather than being leniently ignored as `Ok(None)`).
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::DictCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: Some(Box::new(HirExpr::Name("later".to_string()))),
                        key: Box::new(HirExpr::StringLiteral("k".to_string())),
                        value: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_key() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::DictCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        key: Box::new(HirExpr::Name("later".to_string())),
                        value: Box::new(HirExpr::IntLiteral(1)),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::StringLiteral("k".to_string()),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn collect_block_constraints_propagates_an_error_from_a_dict_comp_assign_s_value() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_h".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::DictCompAssign {
                        target: "y".to_string(),
                        var: "0comp_11_i".to_string(),
                        iter: CompIter::Range {
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(3),
                            step: HirExpr::IntLiteral(1),
                        },
                        cond: None,
                        key: Box::new(HirExpr::StringLiteral("k".to_string())),
                        value: Box::new(HirExpr::Name("later".to_string())),
                    },
                    HirStmt::Assign {
                        target: "later".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                    HirStmt::Return(None),
                ],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_set_item_with_matching_value_type_checks_cleanly() {
        let mut env = Environment::new();
        check_stmt(
            &mut env,
            &HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::DictLiteral(vec![(
                    HirExpr::StringLiteral("a".to_string()),
                    HirExpr::IntLiteral(1),
                )]),
            },
        )
        .unwrap();
        check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("b".to_string()),
                value: HirExpr::IntLiteral(2),
            },
        )
        .unwrap();
        // `d[k] = v` never rebinds `x`'s own environment type, unlike an
        // ordinary `Assign`.
        assert_eq!(
            env.lookup("x"),
            Some(Ty::Dict(Box::new((Ty::Str, Ty::Int))))
        );
    }

    #[test]
    fn a_dict_set_item_accepts_a_bool_value_since_bool_is_an_int_subtype() {
        // Mirrors `appending_a_bool_to_a_list_of_int_is_accepted_since_bool_is_an_int_subtype`
        // (D-086): `d[k] = True` is ordinary, CPython-valid Python.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::BoolLiteral(true),
            },
        )
        .unwrap();
    }

    #[test]
    fn a_dict_set_item_with_wrong_key_type_is_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::IntLiteral(1),
                value: HirExpr::IntLiteral(2),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "dict key type mismatch: expected `str`, found `int`"
        );
    }

    #[test]
    fn a_dict_set_item_with_wrong_value_type_is_t0021() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::StringLiteral("oops".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "cannot assign `str` to a dict value of `int`");
    }

    #[test]
    fn a_dict_set_item_on_a_non_dict_value_is_t0033() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Int);
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`int` does not support item assignment");
    }

    #[test]
    fn a_dict_set_item_on_a_list_value_is_still_t0033() {
        // D-123 supersedes D-105's "no subscript assignment target anywhere
        // in this file" HIR-level invariant (see `pycc_hir`'s own
        // `subscript_assignment_to_a_bare_name_base_lowers_to_dict_set`
        // test), but `list[int]` itself is still read-only-indexed -- this
        // is where that invariant is actually enforced now.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::List(Box::new(Ty::Int)));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::IntLiteral(0),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`list[int]` does not support item assignment");
    }

    #[test]
    fn a_dict_set_item_over_an_undefined_name_is_rejected_as_not_defined() {
        let mut env = Environment::new();
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("is not defined"));
    }

    #[test]
    fn a_dict_set_item_propagates_an_ill_typed_key_expression_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::Name("undefined".to_string()),
                value: HirExpr::IntLiteral(1),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_dict_set_item_propagates_an_ill_typed_value_expression_s_error() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let err = check_stmt(
            &mut env,
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::Name("undefined".to_string()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn function_scope_dict_set_item_with_matching_value_type_checks_cleanly() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap();
    }

    #[test]
    fn function_scope_dict_set_item_over_an_unbound_local_is_unbound_local() {
        // Mirrors the local-vs-global unbound distinction `lookup_bound_name`
        // already gives every other function-scope name lookup (e.g.
        // `ForList`'s own `list` field) -- a declared-but-unassigned local
        // reports `unbound_local`, not "not defined".
        let mut env = Environment::new();
        let err = check_stmt_in_function(
            &mut env,
            &["x"],
            &HirStmt::DictSet {
                dict: "x".to_string(),
                key: HirExpr::StringLiteral("a".to_string()),
                value: HirExpr::IntLiteral(1),
            },
            Ty::None,
        )
        .unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("is not bound before this use"));
    }

    // -- PR-11 Task 7 (D-123): set[int] type-checking ---------------------

    #[test]
    fn an_empty_set_literal_cannot_be_inferred() {
        // Unlike `DictLiteral(vec![])`, which is reachable from real source
        // (`{}` parses as an empty dict), an empty `SetLiteral` can only
        // ever be hand-built HIR -- Python has no empty-set literal spelling
        // at all.
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::SetLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty set literal"));
    }

    #[test]
    fn a_homogeneous_set_literal_infers_set_of_int() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::IntLiteral(2),
            HirExpr::IntLiteral(3),
        ]);
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Set(Box::new(Ty::Int))));
    }

    #[test]
    fn a_set_literal_with_mismatched_element_types_is_rejected_as_t0037() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::StringLiteral("oops".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0037");
        assert_eq!(
            err.message,
            "set element type mismatch: expected int (from the first element), found str"
        );
    }

    #[test]
    fn a_homogeneous_non_int_set_literal_is_rejected_as_t0038() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::StringLiteral("a".to_string()),
            HirExpr::StringLiteral("b".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0038");
        assert_eq!(
            err.message,
            "set[str] is not compiled yet (D-122) -- only set[int] is"
        );
        assert!(err.message.contains("set[str]"));
    }

    #[test]
    fn a_bad_set_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The set-shaped counterpart to
        // `a_bad_dict_literal_is_still_rejected_when_the_solver_path_runs_first`:
        // `collect_expr_constraints`'s own `SetLiteral` arm always returns
        // `Ok(None)` (it has no unification-friendly representation for
        // `Ty::Set` either), so an unrelated private helper with an
        // unresolved (`Ty::Infer`) signature must not let the solver's own
        // leniency swallow a genuine T0038 -- it has to fall through to the
        // real, set-aware check pass.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::SetLiteral(vec![
                        HirExpr::StringLiteral("a".to_string()),
                        HirExpr::StringLiteral("b".to_string()),
                    ]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0038");
    }

    #[test]
    fn a_set_literal_propagates_an_ill_typed_first_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![HirExpr::Name("undefined".to_string())]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_literal_propagates_an_ill_typed_later_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::SetLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::Name("undefined".to_string()),
        ]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn indexing_a_set_reports_t0033() {
        // Mirrors real CPython: sets are not subscriptable either (D-123) --
        // no explicit `Ty::Set` arm exists in `Subscript`'s own match, and
        // the generic `other` fallthrough already covers it.
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("x".to_string())),
            index: Box::new(HirExpr::IntLiteral(0)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert_eq!(err.message, "`set[int]` does not support indexing");
    }

    #[test]
    fn len_of_a_set_infers_int() {
        let mut env = Environment::new();
        env.bind("x".to_string(), Ty::Set(Box::new(Ty::Int)));
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("x".to_string())],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn constraint_collection_len_call_returns_int_for_a_concretely_bound_set() {
        // Mirrors `constraint_collection_len_call_returns_int_for_a_concretely_bound_dict`
        // above, proving the solver's own relaxed `len()` arm (PR-11 Task 7)
        // also accepts a concretely-bound `Ty::Set` term.
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::from([("s".to_string(), Ok(Ty::Set(Box::new(Ty::Int))))]),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::Call {
            callee: "len".to_string(),
            args: vec![HirExpr::Name("s".to_string())],
        };

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert_eq!(term, Some(Ok(Ty::Int)));
    }

    #[test]
    fn constraint_collection_treats_a_set_literal_as_unconstrained_but_recurses_into_elements() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::SetLiteral(vec![HirExpr::IntLiteral(1)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_set_literal_element() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::SetLiteral(vec![HirExpr::Name("missing".to_string())]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    // -- PR-11b Task 3 (D-116): tuple[...] construction + indexing --------

    #[test]
    fn a_tuple_literal_of_int_bool_float_infers_ty_tuple() {
        let env = Environment::new();
        let expr = HirExpr::TupleLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::BoolLiteral(true),
            HirExpr::FloatLiteral(2.5),
        ]);
        assert_eq!(
            infer_expr(&env, &expr),
            Ok(Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float])))
        );
    }

    #[test]
    fn an_empty_tuple_literal_is_rejected_as_t0021() {
        let env = Environment::new();
        let err = infer_expr(&env, &HirExpr::TupleLiteral(vec![])).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("empty tuple literal"));
    }

    #[test]
    fn a_tuple_literal_with_a_string_element_is_rejected_as_t0039() {
        let env = Environment::new();
        let expr = HirExpr::TupleLiteral(vec![
            HirExpr::IntLiteral(1),
            HirExpr::StringLiteral("a".to_string()),
        ]);
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0039");
        assert_eq!(
            err.message,
            "tuple element type `str` is not compiled yet (D-116) -- only int/bool/float elements are"
        );
    }

    #[test]
    fn a_tuple_literal_propagates_an_ill_typed_element_s_error() {
        let env = Environment::new();
        let expr = HirExpr::TupleLiteral(vec![HirExpr::Name("undefined".to_string())]);
        assert_eq!(infer_expr(&env, &expr).unwrap_err().code, "T0021");
    }

    #[test]
    fn tuple_index_with_a_literal_in_range_int_infers_the_positional_element_type() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "t".to_string(),
                    value: HirExpr::TupleLiteral(vec![
                        HirExpr::IntLiteral(1),
                        HirExpr::BoolLiteral(true),
                    ]),
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "y".to_string(),
                    value: HirExpr::Subscript {
                        base: Box::new(HirExpr::Name("t".to_string())),
                        index: Box::new(HirExpr::IntLiteral(1)),
                    },
                }),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn subscripting_a_tuple_with_a_literal_index_infers_the_positional_element_type() {
        let mut env = Environment::new();
        env.bind(
            "t".to_string(),
            Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool, Ty::Float])),
        );
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::IntLiteral(2)),
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Float));
    }

    #[test]
    fn tuple_index_with_a_non_literal_expression_is_rejected_as_t0040() {
        let mut env = Environment::new();
        env.bind("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])));
        env.bind("i".to_string(), Ty::Int);
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::Name("i".to_string())),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0040");
        assert_eq!(
            err.message,
            "tuple index must be a non-negative literal integer within range"
        );
    }

    #[test]
    fn tuple_index_out_of_range_is_rejected_as_t0040() {
        let mut env = Environment::new();
        env.bind("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::IntLiteral(5)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0040");
    }

    #[test]
    fn a_negative_literal_tuple_index_is_rejected_as_t0040() {
        let mut env = Environment::new();
        env.bind("t".to_string(), Ty::Tuple(Box::new(vec![Ty::Int, Ty::Bool])));
        let expr = HirExpr::Subscript {
            base: Box::new(HirExpr::Name("t".to_string())),
            index: Box::new(HirExpr::IntLiteral(-1)),
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0040");
    }

    #[test]
    fn a_bad_tuple_literal_is_still_rejected_when_the_solver_path_runs_first() {
        // The tuple-shaped counterpart to
        // `a_bad_set_literal_is_still_rejected_when_the_solver_path_runs_first`:
        // `collect_expr_constraints`'s own `TupleLiteral` arm always returns
        // `Ok(None)` (it has no unification-friendly representation for
        // `Ty::Tuple` either), so an unrelated private helper with an
        // unresolved (`Ty::Infer`) signature must not let the solver's own
        // leniency swallow a genuine T0039 -- it has to fall through to the
        // real, tuple-aware check pass.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::TupleLiteral(vec![HirExpr::StringLiteral("a".to_string())]),
                }),
                HirItem::Function {
                    name: "_constant".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0039");
    }

    #[test]
    fn constraint_collection_treats_a_tuple_literal_as_unconstrained_but_recurses_into_elements()
    {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &[],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::TupleLiteral(vec![HirExpr::IntLiteral(1)]);

        let term = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap();

        assert!(term.is_none());
    }

    #[test]
    fn constraint_collection_propagates_an_error_from_a_tuple_literal_element() {
        let signatures = HashMap::new();
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        let mut binops = Vec::new();
        let env = ConstraintEnvironment {
            bindings: HashMap::new(),
            local_names: &["missing"],
            defs_rebound: HashSet::new(),
        };
        let expr = HirExpr::TupleLiteral(vec![HirExpr::Name("missing".to_string())]);

        let err = collect_expr_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &env,
            &expr,
        )
        .unwrap_err();

        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_top_level_binary_addition_type_checks() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::IntLiteral(1)),
                right: Box::new(HirExpr::IntLiteral(2)),
            }))],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_top_level_reference_to_an_undefined_name_is_a_clean_error() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Name(
                "undefined".to_string(),
            )))],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_top_level_call_to_a_previously_defined_function_type_checks() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "main".to_string(),
                    args: vec![],
                })),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_call_a_sibling_function_defined_before_it() {
        // Regression test for D-040: `check_function`'s own env used to be
        // seeded empty, so `main` couldn't see `helper` even though both are
        // ordinary module-level functions -- a valid, non-recursive call
        // between two sibling functions was wrongly rejected with T0021.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![HirExpr::IntLiteral(5)],
                        }],
                    })],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_call_a_sibling_function_defined_after_it() {
        // Same gap as above, but exercising the pre-registration pass (D-039)
        // from the *other* direction: `main` is checked first (it's first in
        // the module) yet still must see `helper`, which is defined later.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![HirExpr::IntLiteral(5)],
                        }],
                    })],
                },
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("x".to_string())),
                        right: Box::new(HirExpr::IntLiteral(1)),
                    }))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_read_a_module_level_global_defined_before_it() {
        // Regression test for D-041: reading a module global from a function
        // body needs no `global` declaration in real Python (that's only
        // required to *rebind* one) -- child_for_function used to reset
        // bindings to empty, so `f`'s body couldn't see `x` even though it's
        // an ordinary module-level constant, not some caller's local.
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_can_read_a_module_level_global_defined_after_it() {
        // Same gap, other direction: a function is only ever *called* after
        // the module has (typically) finished running top to bottom, so a
        // global defined later in the file is still visible inside an
        // earlier function's body.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(5),
                }),
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_function_parameter_shadows_a_module_level_global_of_the_same_name() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("global".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![("x".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("x".to_string())))],
                },
            ],
        };
        // If the global (Ty::Str) leaked through instead of the parameter
        // (Ty::Int), this would fail with a T0022 return-type mismatch.
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_later_local_assignment_blocks_fallback_to_a_same_named_global() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn a_read_before_local_assignment_is_local_even_without_a_global() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::Int,
                body: vec![
                    HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                    HirStmt::Assign {
                        target: "x".to_string(),
                        value: HirExpr::IntLiteral(2),
                    },
                    HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                ],
            }],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn local_name_collection_deduplicates_assignment_and_for_targets() {
        let body = vec![
            HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            },
            HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(2),
            },
            HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
            HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(1),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            },
        ];

        assert_eq!(function_local_names(&[], &body), vec!["x", "i"]);
    }

    #[test]
    fn local_name_collection_deduplicates_for_list_targets() {
        let body = vec![
            HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![],
            },
            HirStmt::ForList {
                var: "i".to_string(),
                list: "ys".to_string(),
                body: vec![],
            },
        ];

        assert_eq!(function_local_names(&[], &body), vec!["i"]);
    }

    #[test]
    fn local_name_collection_deduplicates_annotated_assignment_targets() {
        let body = vec![
            HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: Some(HirExpr::IntLiteral(1)),
            },
            HirStmt::AnnAssign {
                target: "x".to_string(),
                annotation: Ty::Int,
                value: None,
            },
        ];

        assert_eq!(function_local_names(&[], &body), vec!["x"]);
    }

    #[test]
    fn contains_return_treats_an_annotated_assignment_as_not_a_return() {
        let body = vec![HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: Some(HirExpr::IntLiteral(1)),
        }];
        assert!(!contains_return(&body));
    }

    #[test]
    fn block_always_returns_treats_an_annotated_assignment_as_not_a_return() {
        let body = vec![HirStmt::AnnAssign {
            target: "x".to_string(),
            annotation: Ty::Int,
            value: None,
        }];
        assert!(!block_always_returns(&body));
    }

    #[test]
    fn contains_return_finds_a_return_inside_a_for_list_loop_body() {
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Return(Some(HirExpr::Name("i".to_string())))],
        }];
        assert!(contains_return(&body));
    }

    #[test]
    fn block_always_returns_treats_a_for_list_loop_as_not_always_returning() {
        // A `for` loop's body might never execute (an empty list), so it can
        // never on its own guarantee a function returns -- same treatment as
        // `ForRange` gets in this same match.
        let body = vec![HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Return(Some(HirExpr::Name("i".to_string())))],
        }];
        assert!(!block_always_returns(&body));
    }

    #[test]
    fn a_call_before_local_assignment_cannot_fall_back_to_a_global_function() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![],
                        }),
                        HirStmt::Assign {
                            target: "helper".to_string(),
                            value: HirExpr::IntLiteral(1),
                        },
                    ],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "local name `helper` is not bound before this use"
        );
    }

    #[test]
    fn a_call_before_local_assignment_cannot_fall_back_to_print() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![
                    HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![],
                    }),
                    HirStmt::Assign {
                        target: "print".to_string(),
                        value: HirExpr::IntLiteral(1),
                    },
                ],
            }],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "local name `print` is not bound before this use"
        );
    }

    #[test]
    fn a_bound_local_value_cannot_fall_back_to_a_function_registry_entry() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![],
                },
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::Assign {
                            target: "helper".to_string(),
                            value: HirExpr::IntLiteral(1),
                        },
                        HirStmt::ExprStmt(HirExpr::Call {
                            callee: "helper".to_string(),
                            args: vec![],
                        }),
                    ],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    #[test]
    fn a_parameter_cannot_fall_back_to_a_same_named_builtin() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![("print".to_string(), Ty::Int)],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![],
                })],
            }],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `print` is bound to a non-callable value");
    }

    #[test]
    fn direct_function_check_uses_the_same_lexical_local_classification() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(2),
                },
                HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
            ],
        };

        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn direct_function_check_treats_an_unbound_call_target_as_a_local_read() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                }),
                HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
            ],
        };

        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "local name `helper` is not bound before this use"
        );
    }

    #[test]
    fn direct_function_check_rejects_calling_a_bound_local_value() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                },
                HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                }),
            ],
        };

        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// #133: a module-level value binding must shadow a same-named user
    /// function at every later call site (CPython raises `TypeError: 'int'
    /// object is not callable`; the pre-fix checker resolved the call
    /// through the function registry and accepted it).
    #[test]
    fn a_module_value_binding_shadows_a_same_named_function_call() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// #133: the same shadowing rule covers builtins -- `print = 1` makes a
    /// later `print()` a call on an `int`, which CPython rejects at runtime.
    #[test]
    fn a_module_value_binding_shadows_a_builtin_call() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "print".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![],
                })),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `print` is bound to a non-callable value");
    }

    /// #133: pass 3 checks annotated function bodies against the final
    /// module environment (D-041), so a body call whose target is
    /// module-bound to a value is rejected through `infer_expr_in`'s gate.
    #[test]
    fn an_annotated_body_call_sees_the_module_value_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::Function {
                    name: "caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// #133: the solver path (an underscore private helper with an inferred
    /// signature) seeds its body environment from the module globals, so
    /// `collect_expr_constraints`'s mirrored gate rejects the same shape.
    /// Mirrors the issue's own third reproduction end to end through
    /// `check_and_resolve`.
    #[test]
    fn a_private_helper_call_sees_the_module_value_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::Function {
                    name: "_call_helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// D-110's first recorded ordering consequence, pinned as deliberate: a
    /// body call is rejected whenever the callee is value-bound anywhere at
    /// module top level, even when the program's only call executes *before*
    /// the rebinding in CPython's dynamic order (this exact module runs fine
    /// under CPython: `caller()` returns 1, then `helper = 2` rebinds). Pass
    /// 3 checks bodies against the final module environment (D-041), and
    /// D-110 accepts the stricter-than-one-dynamic-trace rejection rather
    /// than importing #22's execution-order model.
    #[test]
    fn a_rebinding_after_the_only_call_still_rejects_the_body_call() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                },
                HirItem::Function {
                    name: "caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::Call {
                        callee: "caller".to_string(),
                        args: vec![],
                    },
                }),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// D-110's second recorded ordering consequence: the binding gate is
    /// callee-first, so a shadowed callee with an additionally-invalid
    /// argument reports the non-callable callee, not the argument's own
    /// error -- uniform with the pre-existing local gate, though CPython's
    /// evaluation order would surface the argument's `NameError` first.
    #[test]
    fn a_shadowed_callee_reports_before_its_argument_errors() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![HirExpr::Name("undefined_name".to_string())],
                })),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// D-110's third recorded consequence: a bound value that shadows
    /// nothing at all now reports the accurate non-callable diagnostic
    /// instead of final validation's misleading "call to undefined function".
    #[test]
    fn calling_a_bound_value_that_shadows_nothing_reports_non_callable() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "x".to_string(),
                    args: vec![],
                })),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `x` is bound to a non-callable value");
    }

    /// The discriminating case for `collect_expr_constraints`' mirror gate
    /// (D-110): the bound callee is neither `print` nor any `def`, inside an
    /// inference-signature helper. Without the mirror, `signatures.get`
    /// misses, the call stays unresolved, and the solver dead-ends in
    /// "cannot infer return type of private helper `_h`; add an annotation"
    /// *before* pass 3 ever runs -- a misleading message, since no
    /// annotation fixes calling an `int`. This is the one shape where the
    /// mirror's presence changes the observable diagnostic; a shadowed
    /// `print` would instead be resolved by the special case and caught by
    /// pass 3's own gate later (see the mirror-gate comment).
    #[test]
    fn a_private_helper_calling_a_shadowed_non_function_needs_the_mirror_gate() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "x".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `x` is bound to a non-callable value");
    }

    /// Behavior pin for the shadowed-builtin shape inside an
    /// inference-signature helper: the mirror gate fires before the `print`
    /// special case, so the accurate non-callable diagnostic wins. (Pass 3
    /// would independently reject this shape even without the mirror; the
    /// discriminating case for the mirror itself is the test above.)
    #[test]
    fn a_private_helper_calling_a_shadowed_builtin_is_rejected() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "print".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `print` is bound to a non-callable value");
    }

    /// The round-5 blocker's pin (PR #252's review): a *compatible-type*
    /// value reassignment after the `def` must re-shadow it, even though
    /// `check_assignment`'s already-bound branch never calls `bind()` --
    /// clearing the def-rebound flag only inside `bind()` left it
    /// permanently stuck for any name with a pre-`def` binding, so this
    /// exact module resolved the function where CPython raises `TypeError`.
    #[test]
    fn a_compatible_reassignment_after_the_def_shadows_it_again() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// The round-6 pin (PR #252's review): a private helper's *parameter*
    /// that collides with a module-level def-rebound name re-binds it for
    /// that body, so calling the parameter reports the accurate
    /// non-callable diagnostic -- before the fix the per-function env setup
    /// stripped the name from `bindings` but left the stale def-rebound
    /// flag, skipping the mirror gate and mislabeling a bound parameter as
    /// "not bound before this use".
    #[test]
    fn a_parameter_colliding_with_a_def_rebound_name_reports_non_callable() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![("helper".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// The solver-path twin of the test above, proving the two environment
    /// walks agree: the solver's Assign arm already cleared its def-rebound
    /// flag unconditionally, and after the round-5 fix the `Environment`
    /// walk does too.
    #[test]
    fn a_compatible_reassignment_after_the_def_shadows_it_in_the_solver_too() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(2),
                }),
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(
            err.message,
            "name `helper` is bound to a non-callable value"
        );
    }

    /// The round-4 blocker's pin (PR #252's review): the `def`-rebinding
    /// fact must NOT erase the representation record, so D-040's sticky
    /// rule still rejects `helper = 1; def helper(): ...; helper = "leaked"`
    /// with T0023 -- a version that cleared `bindings` at the `def` let this
    /// program reach codegen with an `int`-allocated slot stored as `str`.
    #[test]
    fn a_value_reassignment_after_the_def_still_enforces_representation() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::StringLiteral("leaked".to_string()),
                }),
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0023");
        assert_eq!(
            err.message,
            "cannot assign `str` to `helper`, previously inferred as `int`"
        );
    }

    /// D-110's source-order `def` rebinding (PR #252's review caught the
    /// initial gate rejecting this): a later `def` rebinds the name over an
    /// earlier value binding, so the call resolves the function exactly
    /// as CPython does (`helper = 1; def helper(): ...; helper()` prints
    /// the function's result there).
    #[test]
    fn a_later_def_rebinds_over_an_earlier_value_binding() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }],
                })),
            ],
        };

        assert!(check(&hir).is_ok());
    }

    /// The same source-order `def` rebinding seen from a function body:
    /// pass 3's final module environment reflects the net binding, so a
    /// body call to a name whose value binding was cleared by a later
    /// `def` resolves the function.
    #[test]
    fn a_body_call_resolves_when_a_later_def_rebinds_the_value() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::Function {
                    name: "caller".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        assert!(check(&hir).is_ok());
    }

    /// And from the solver path: the accumulated globals clear a value
    /// binding when the same-named `def` follows it, so an
    /// inference-signature helper's body resolves the function.
    #[test]
    fn a_private_helper_resolves_when_a_later_def_rebinds_the_value() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
                },
                HirItem::Function {
                    name: "_h".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Call {
                        callee: "helper".to_string(),
                        args: vec![],
                    }))],
                },
            ],
        };

        assert!(check_and_resolve(&hir).is_ok());
    }

    /// #133's deliberate boundary with #22: pass 2 checks top-level code in
    /// source order, so a call *before* the shadowing assignment still
    /// resolves the function -- the binding simply does not exist yet at
    /// that point. Only the already-active binding shadows.
    #[test]
    fn a_top_level_call_before_the_shadowing_assignment_still_resolves() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "helper".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "print".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    })],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "helper".to_string(),
                    args: vec![],
                })),
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "helper".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
            ],
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_local_first_assignment_does_not_inherit_the_globals_type() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::StringLiteral("global".to_string()),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
        };

        assert!(check(&hir).is_ok());
    }

    #[test]
    fn a_self_referential_first_local_assignment_cannot_read_the_global() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::Int,
                    body: vec![
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::BinOp {
                                op: BinOpKind::Add,
                                left: Box::new(HirExpr::Name("x".to_string())),
                                right: Box::new(HirExpr::IntLiteral(1)),
                            },
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn an_assignment_nested_in_if_classifies_the_name_as_function_local() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                        HirStmt::If {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::Assign {
                                target: "x".to_string(),
                                value: HirExpr::IntLiteral(2),
                            }],
                            orelse: vec![],
                        },
                    ],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn a_nested_for_target_classifies_the_name_as_function_local() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "i".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("i".to_string())),
                        HirStmt::While {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::ForRange {
                                var: "i".to_string(),
                                start: HirExpr::IntLiteral(0),
                                stop: HirExpr::IntLiteral(1),
                                step: HirExpr::IntLiteral(1),
                                body: vec![],
                            }],
                        },
                    ],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.message, "local name `i` is not bound before this use");
    }

    #[test]
    fn a_for_target_is_local_while_its_range_operands_are_evaluated() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "i".to_string(),
                    value: HirExpr::IntLiteral(3),
                }),
                HirItem::Function {
                    name: "f".to_string(),
                    params: vec![],
                    return_ty: Ty::None,
                    body: vec![HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::Name("i".to_string()),
                        step: HirExpr::IntLiteral(1),
                        body: vec![],
                    }],
                },
            ],
        };

        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `i` is not bound before this use");
    }

    #[test]
    fn private_helper_inference_rejects_a_read_before_local_assignment() {
        let hir = HirModule {
            items: vec![
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "x".to_string(),
                    value: HirExpr::IntLiteral(1),
                }),
                HirItem::Function {
                    name: "_f".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![
                        HirStmt::ExprStmt(HirExpr::Name("x".to_string())),
                        HirStmt::Assign {
                            target: "x".to_string(),
                            value: HirExpr::IntLiteral(2),
                        },
                        HirStmt::Return(Some(HirExpr::Name("x".to_string()))),
                    ],
                },
            ],
        };

        let err = check_and_resolve(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "local name `x` is not bound before this use");
    }

    #[test]
    fn check_function_the_public_api_still_has_no_sibling_visibility() {
        // `check_function` is a standalone entry point with no module
        // context, so it must keep working exactly as before: it only ever
        // sees its own signature (needed for recursion), never a sibling's.
        let function = HirItem::Function {
            name: "main".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "helper".to_string(),
                args: vec![],
            })],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_function_body_is_now_checked() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "f".to_string(),
                params: vec![],
                return_ty: Ty::None,
                body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_bare_call_infers_none() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "print".to_string(),
            args: vec![],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn calling_an_undefined_function_is_a_clean_error() {
        let env = Environment::new();
        let expr = HirExpr::Call {
            callee: "undefined".to_string(),
            args: vec![],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("undefined"));
    }

    #[test]
    fn calling_a_defined_function_infers_its_declared_return_type() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn a_wide_call_uses_the_heap_argument_type_buffer() {
        let mut env = Environment::new();
        env.bind_function("wide".to_string(), vec![Ty::Int; 5], Ty::Int);
        let expr = HirExpr::Call {
            callee: "wide".to_string(),
            args: vec![HirExpr::IntLiteral(1); 5],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::Int));
    }

    #[test]
    fn calling_a_function_with_a_bool_argument_for_an_int_parameter_succeeds() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int], Ty::None);
        let expr = HirExpr::Call {
            callee: "f".to_string(),
            args: vec![HirExpr::BoolLiteral(true)],
        };
        assert_eq!(infer_expr(&env, &expr), Ok(Ty::None));
    }

    #[test]
    fn calling_a_function_with_the_wrong_number_of_arguments_is_a_clean_error() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("expects 2 argument"));
    }

    #[test]
    fn an_int_argument_for_a_float_parameter_is_a_clean_error() {
        // D-086: pycc requires an explicit `float(...)` conversion at typed
        // boundaries (parameters, returns, assignments) rather than
        // following the Python typing spec's numeric-tower rule that `int`
        // is accepted wherever `float` is annotated. This is a deliberate
        // deviation from `mypy --strict` (which accepts this call), not an
        // oversight -- see D-086's rationale in docs/DECISIONS.md.
        let mut env = Environment::new();
        env.bind_function("identity".to_string(), vec![Ty::Float], Ty::Float);
        let expr = HirExpr::Call {
            callee: "identity".to_string(),
            args: vec![HirExpr::IntLiteral(1)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("argument 1 of `identity` expects `float`, got `int`"));
    }

    #[test]
    fn calling_a_function_with_a_wrong_typed_argument_is_a_clean_error() {
        let mut env = Environment::new();
        env.bind_function("add".to_string(), vec![Ty::Int, Ty::Int], Ty::Int);
        let expr = HirExpr::Call {
            callee: "add".to_string(),
            args: vec![HirExpr::IntLiteral(1), HirExpr::FloatLiteral(2.5)],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(
            err.message.contains("argument 2")
                && err.message.contains("int")
                && err.message.contains("float")
        );
    }

    #[test]
    fn a_wide_calls_later_inference_error_precedes_an_earlier_type_mismatch() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int; 5], Ty::None);
        let expr = HirExpr::Call {
            callee: "f".to_string(),
            args: vec![
                HirExpr::StringLiteral("wrong".to_string()),
                HirExpr::IntLiteral(2),
                HirExpr::IntLiteral(3),
                HirExpr::IntLiteral(4),
                HirExpr::Name("undefined".to_string()),
            ],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert_eq!(err.message, "name `undefined` is not defined");
    }

    #[test]
    fn calling_a_function_with_an_undefined_argument_propagates_the_error() {
        let mut env = Environment::new();
        env.bind_function("f".to_string(), vec![Ty::Int], Ty::None);
        let expr = HirExpr::Call {
            callee: "f".to_string(),
            args: vec![HirExpr::Name("undefined".to_string())],
        };
        let err = infer_expr(&env, &expr).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn a_function_s_body_is_checked_against_its_declared_param_types() {
        let function = HirItem::Function {
            name: "add".to_string(),
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(HirExpr::Name("a".to_string())),
                right: Box::new(HirExpr::Name("b".to_string())),
            }))],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_return_with_no_value_when_none_is_expected_succeeds() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::Return(None)],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_return_with_no_value_when_a_value_is_expected_is_a_clean_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(None)],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
    }

    #[test]
    fn a_return_type_mismatch_is_a_clean_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
    }

    #[test]
    fn a_value_returning_function_must_return_on_every_path() {
        let function = HirItem::Function {
            name: "answer".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![HirExpr::IntLiteral(42)],
            })],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
        assert!(err.message.contains("can exit without returning"));
    }

    #[test]
    fn an_if_with_returns_in_both_branches_satisfies_the_return_contract() {
        let function = HirItem::Function {
            name: "choose".to_string(),
            params: vec![("condition".to_string(), Ty::Bool)],
            return_ty: Ty::Int,
            body: vec![HirStmt::If {
                test: HirExpr::Name("condition".to_string()),
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                orelse: vec![HirStmt::Return(Some(HirExpr::IntLiteral(2)))],
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn an_if_with_only_one_returning_branch_is_t0022() {
        let function = HirItem::Function {
            name: "choose".to_string(),
            params: vec![("condition".to_string(), Ty::Bool)],
            return_ty: Ty::Int,
            body: vec![HirStmt::If {
                test: HirExpr::Name("condition".to_string()),
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                orelse: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0022");
    }

    #[test]
    fn a_return_whose_value_is_undefined_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Name(
                "undefined".to_string(),
            )))],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
    }

    #[test]
    fn recursion_is_supported_since_the_function_s_own_signature_is_in_scope() {
        let function = HirItem::Function {
            name: "count".to_string(),
            params: vec![("n".to_string(), Ty::Int)],
            return_ty: Ty::Int,
            body: vec![HirStmt::Return(Some(HirExpr::Call {
                callee: "count".to_string(),
                args: vec![HirExpr::Name("n".to_string())],
            }))],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_function_s_if_while_and_for_bodies_are_checked_against_its_return_type() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Int,
            body: vec![
                HirStmt::If {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::While {
                        test: HirExpr::BoolLiteral(true),
                        body: vec![HirStmt::ForRange {
                            var: "i".to_string(),
                            start: HirExpr::IntLiteral(0),
                            stop: HirExpr::IntLiteral(1),
                            step: HirExpr::IntLiteral(1),
                            body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                        }],
                    }],
                    orelse: vec![HirStmt::Return(Some(HirExpr::IntLiteral(0)))],
                },
                HirStmt::Return(Some(HirExpr::IntLiteral(2))),
            ],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_bad_return_nested_in_if_while_and_for_is_still_caught() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::Str,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![HirStmt::While {
                    test: HirExpr::BoolLiteral(true),
                    body: vec![HirStmt::ForRange {
                        var: "i".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::IntLiteral(1),
                        step: HirExpr::IntLiteral(1),
                        body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                    }],
                }],
                orelse: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0022");
    }

    #[test]
    #[should_panic(expected = "check_function called with a non-Function HirItem")]
    fn check_function_panics_on_a_non_function_item() {
        let _ = check_function(&HirItem::TopLevelStmt(HirStmt::Return(None)));
    }

    #[test]
    fn an_if_s_test_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::Name("undefined".to_string()),
                body: vec![],
                orelse: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn an_if_s_orelse_ill_typed_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::If {
                test: HirExpr::BoolLiteral(true),
                body: vec![],
                orelse: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_while_s_test_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::While {
                test: HirExpr::Name("undefined".to_string()),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_start_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::Name("undefined".to_string()),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_stop_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::Name("undefined".to_string()),
                step: HirExpr::IntLiteral(1),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_range_s_step_undefined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForRange {
                var: "i".to_string(),
                start: HirExpr::IntLiteral(0),
                stop: HirExpr::IntLiteral(3),
                step: HirExpr::Name("undefined".to_string()),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_list_loop_binds_its_variable_as_the_list_element_type_in_a_function_body() {
        // Direct `check_stmt_in_function` call (matching this file's own
        // established pattern, e.g. the `AnnAssign` tests just above) rather
        // than `check_function`: a for-loop body might never execute (an
        // empty list), so `block_always_returns` always treats it as "does
        // not always return" (same as `ForRange`) -- a function whose only
        // statement is a `for` loop can never satisfy a non-`None` return
        // type, regardless of what's inside the loop body.
        let mut env = Environment::new();
        env.bind("xs".to_string(), Ty::List(Box::new(Ty::Int)));
        let stmt = HirStmt::ForList {
            var: "i".to_string(),
            list: "xs".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("i".to_string()),
            }],
        };
        check_stmt_in_function(&mut env, &["xs", "i", "y"], &stmt, Ty::None).unwrap();
        assert_eq!(env.lookup("i"), Some(Ty::Int));
        assert_eq!(env.lookup("y"), Some(Ty::Int));
    }

    #[test]
    fn a_for_dict_loop_binds_its_variable_as_the_key_type_in_a_function_body() {
        // Mirrors the list-shaped test above (PR-11 Task 3, D-123): `for k in
        // d:` binds `k` as the dict's key type, not its value type.
        let mut env = Environment::new();
        env.bind("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))));
        let stmt = HirStmt::ForList {
            var: "k".to_string(),
            list: "d".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("k".to_string()),
            }],
        };
        check_stmt_in_function(&mut env, &["d", "k", "y"], &stmt, Ty::None).unwrap();
        assert_eq!(env.lookup("k"), Some(Ty::Str));
        assert_eq!(env.lookup("y"), Some(Ty::Str));
    }

    #[test]
    fn a_for_set_loop_binds_its_variable_as_the_element_type_in_a_function_body() {
        // Mirrors the dict-shaped test above (PR-11 Task 7, D-123): `for x in
        // s:` binds `x` as the set's element type.
        let mut env = Environment::new();
        env.bind("s".to_string(), Ty::Set(Box::new(Ty::Int)));
        let stmt = HirStmt::ForList {
            var: "x".to_string(),
            list: "s".to_string(),
            body: vec![HirStmt::Assign {
                target: "y".to_string(),
                value: HirExpr::Name("x".to_string()),
            }],
        };
        check_stmt_in_function(&mut env, &["s", "x", "y"], &stmt, Ty::None).unwrap();
        assert_eq!(env.lookup("x"), Some(Ty::Int));
        assert_eq!(env.lookup("y"), Some(Ty::Int));
    }

    #[test]
    fn a_for_list_s_list_not_defined_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "undefined".to_string(),
                body: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not defined"));
    }

    #[test]
    fn a_for_list_s_list_read_before_a_later_local_assignment_is_unbound_local() {
        // `xs` is a declared local of this function (it's assigned further
        // down), so reading it in the `for` clause before that assignment
        // must be `unbound_local`, not "not defined" -- the same
        // is_local-aware distinction `HirExpr::Name` itself makes.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ForList {
                    var: "i".to_string(),
                    list: "xs".to_string(),
                    body: vec![],
                },
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
            ],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    #[test]
    fn a_for_list_loop_over_a_non_list_value_in_a_function_body_is_rejected() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "x".to_string(),
                body: vec![],
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_for_list_loop_whose_body_statement_is_ill_typed_in_a_function_body_propagates_the_error() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("xs".to_string(), Ty::List(Box::new(Ty::Int)))],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![HirStmt::ExprStmt(HirExpr::Name("undefined".to_string()))],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_for_list_loop_rejects_a_conflicting_reassignment_of_its_variable_in_a_function_body() {
        // Same as the module-scope sibling above: `check_assignment` itself
        // is what rejects rebinding an already-`str` parameter to the list's
        // `int` element type.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![
                ("i".to_string(), Ty::Str),
                ("xs".to_string(), Ty::List(Box::new(Ty::Int))),
            ],
            return_ty: Ty::None,
            body: vec![HirStmt::ForList {
                var: "i".to_string(),
                list: "xs".to_string(),
                body: vec![],
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    // -- PR-12 Task 3 (D-117): comprehension type-checking, function scope --

    #[test]
    fn a_list_comprehension_over_range_type_checks_in_a_function_body() {
        // Also pins that `var` is bound before `elt` is checked at function
        // scope (mirrors the module-scope sibling above): if the ordering
        // were wrong, `elt`'s reference to the loop variable would fail as
        // an unbound local.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_11_i".to_string())),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_list_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_list_comprehension_producing_a_non_int_element_is_rejected_as_t0034_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::StringLiteral("x".to_string())),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0034");
        assert!(err.message.contains("list[str]"));
    }

    #[test]
    fn a_set_comprehension_over_a_bare_set_parameter_type_checks_in_a_function_body() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Set` at function scope.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("s".to_string(), Ty::Set(Box::new(Ty::Int)))],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_20_x".to_string(),
                iter: CompIter::Name("s".to_string()),
                cond: None,
                elt: Box::new(HirExpr::Name("0comp_20_x".to_string())),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_set_comprehension_producing_a_non_int_element_is_rejected_as_t0038_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::StringLiteral("x".to_string())),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0038");
        assert!(err.message.contains("set[str]"));
    }

    #[test]
    fn a_set_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_dict_comprehension_over_a_bare_dict_parameter_type_checks_in_a_function_body() {
        // Exercises `resolve_comp_iter`'s `CompIter::Name` branch resolving
        // to `Ty::Dict` at function scope, binding `var` as the dict's key
        // type.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("d".to_string(), Ty::Dict(Box::new((Ty::Str, Ty::Int))))],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_20_k".to_string(),
                iter: CompIter::Name("d".to_string()),
                cond: None,
                key: Box::new(HirExpr::Name("0comp_20_k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_dict_comprehension_producing_a_non_str_int_pair_is_rejected_as_t0036_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::IntLiteral(1)),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0036");
        assert!(err.message.contains("dict[int, int]"));
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_of_a_non_bool_type_still_type_checks_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::StringLiteral("truthy".to_string()))),
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        check_function(&function).unwrap();
    }

    #[test]
    fn a_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_comprehension_over_a_forward_referenced_iterable_is_rejected_as_unbound_local() {
        // Mirrors `a_for_list_loop_over_a_forward_referenced_list...`-style
        // behavior: `xs` is a genuine function-local name (assigned later in
        // the same body), so referencing it before that assignment is
        // `unbound_local`, not `not defined`.
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![
                HirStmt::ListCompAssign {
                    target: "y".to_string(),
                    var: "0comp_11_i".to_string(),
                    iter: CompIter::Name("xs".to_string()),
                    cond: None,
                    elt: Box::new(HirExpr::IntLiteral(1)),
                },
                HirStmt::Assign {
                    target: "xs".to_string(),
                    value: HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)]),
                },
            ],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("not bound before this use"));
    }

    // Per-arm error-propagation pins (function scope): the same set as the
    // module-scope block above, `check_stmt_in_function`'s own three arms.

    #[test]
    fn a_set_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_dict_comprehension_over_a_non_list_dict_set_iterable_is_rejected_as_t0033_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Ty::Int)],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Name("x".to_string()),
                cond: None,
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        let err = check_function(&function).unwrap_err();
        assert_eq!(err.code, "T0033");
        assert!(err.message.contains("cannot be iterated"));
    }

    #[test]
    fn a_list_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("0comp_11_i".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_set_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("0comp_11_i".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_dict_comprehension_rejects_a_conflicting_reassignment_of_its_loop_variable_in_a_function_body()
     {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![("0comp_11_i".to_string(), Ty::Str)],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0023");
    }

    #[test]
    fn a_list_comprehension_s_if_filter_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_if_filter_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
                elt: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_if_filter_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: Some(Box::new(HirExpr::Name("undefined".to_string()))),
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_list_comprehension_s_elt_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::ListCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("undefined".to_string())),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_set_comprehension_s_elt_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::SetCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                elt: Box::new(HirExpr::Name("undefined".to_string())),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_key_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::Name("undefined".to_string())),
                value: Box::new(HirExpr::IntLiteral(1)),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn a_dict_comprehension_s_value_propagates_an_ill_typed_error_in_a_function_body() {
        let function = HirItem::Function {
            name: "f".to_string(),
            params: vec![],
            return_ty: Ty::None,
            body: vec![HirStmt::DictCompAssign {
                target: "y".to_string(),
                var: "0comp_11_i".to_string(),
                iter: CompIter::Range {
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::IntLiteral(3),
                    step: HirExpr::IntLiteral(1),
                },
                cond: None,
                key: Box::new(HirExpr::StringLiteral("k".to_string())),
                value: Box::new(HirExpr::Name("undefined".to_string())),
            }],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_identity_signature_is_inferred_from_its_call_site_and_return() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn check_and_resolve_takes_the_fast_path_for_an_already_concrete_valid_module() {
        // Pre-existing gap noticed while chasing this task's own 100%
        // coverage requirement: every other `check_and_resolve` test in this
        // file uses a `Ty::Infer` signature, so `checked_function_signatures`'s
        // fast-path *success* return (`concrete_function_signatures` is
        // `Some` and `check_with_signatures` succeeds) was never exercised.
        // Unrelated to this task's list-typing work, but cheap and in-scope
        // to close here rather than leave dangling.
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "identity".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }],
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(resolved, hir);
    }

    #[test]
    fn check_and_resolve_materializes_private_signatures_without_mutating_input() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_identity".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }
        );
        assert_eq!(
            hir.items[0],
            HirItem::Function {
                name: "_identity".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
            }
        );
    }

    #[test]
    fn annotated_int_result_propagates_back_to_a_private_binary_parameter() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("value".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            }],
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("value".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            }
        );
    }

    #[test]
    fn annotated_int_result_propagates_back_to_a_right_binary_parameter() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::Name("value".to_string())),
                }))],
            }],
        };

        let resolved = check_and_resolve(&hir).unwrap();
        assert_eq!(
            resolved.items[0],
            HirItem::Function {
                name: "_inc".to_string(),
                params: vec![("value".to_string(), Ty::Int)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::IntLiteral(1)),
                    right: Box::new(HirExpr::Name("value".to_string())),
                }))],
            }
        );
    }

    #[test]
    fn annotated_int_result_rejects_a_known_string_left_operand() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::StringLiteral("wrong".to_string())),
                    right: Box::new(HirExpr::Name("value".to_string())),
                }))],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn annotated_int_result_rejects_a_known_string_right_operand() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_bad".to_string(),
                params: vec![("value".to_string(), Ty::Infer)],
                return_ty: Ty::Int,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("value".to_string())),
                    right: Box::new(HirExpr::StringLiteral("wrong".to_string())),
                }))],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_parameter_is_inferred_by_forwarding_into_an_annotated_callee() {
        // Regression test (self-review finding, pre-merge): the solver used
        // to only unify a call argument against a callee's parameter when
        // the callee's own parameter term was itself unresolved. When the
        // callee is fully annotated (its parameter term is already
        // `Ok(Ty::Int)`), an unresolved *caller* argument variable never got
        // constrained in that direction, even though `unify_terms` itself
        // already supports it symmetrically -- so `_forward` below used to
        // fail with a spurious "add an annotation" T0021 instead of
        // correctly inferring `x: int` from forwarding into `_sink`.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::ExprStmt(HirExpr::Call {
                        callee: "_sink".to_string(),
                        args: vec![HirExpr::Name("x".to_string())],
                    })],
                },
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_binary_helper_signature_is_inferred_across_operator_constraints() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_add".to_string(),
                    params: vec![
                        ("left".to_string(), Ty::Infer),
                        ("right".to_string(), Ty::Infer),
                    ],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("left".to_string())),
                        right: Box::new(HirExpr::Name("right".to_string())),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_add".to_string(),
                    args: vec![HirExpr::IntLiteral(1), HirExpr::IntLiteral(2)],
                })),
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_true_division_helper_infers_a_float_return() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_ratio".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Div,
                        left: Box::new(HirExpr::Name("value".to_string())),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::Assign {
                    target: "ratio".to_string(),
                    value: HirExpr::Call {
                        callee: "_ratio".to_string(),
                        args: vec![HirExpr::IntLiteral(1)],
                    },
                }),
            ],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_helper_without_a_return_infers_none() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_log".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::ExprStmt(HirExpr::Call {
                    callee: "print".to_string(),
                    args: vec![HirExpr::FString(vec![
                        FStringPart::Literal("equal=".to_string()),
                        FStringPart::Interpolation(Box::new(HirExpr::Compare {
                            op: CmpOpKind::Eq,
                            left: Box::new(HirExpr::IntLiteral(1)),
                            right: Box::new(HirExpr::IntLiteral(1)),
                        })),
                    ])],
                })],
            }],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_helper_with_a_bare_return_infers_none() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_stop".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(None)],
            }],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_constant_helper_infers_a_float_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_constant".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::FloatLiteral(1.5)))],
            }],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn private_range_helper_infers_its_parameter_as_int() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_loop".to_string(),
                params: vec![("limit".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Name("limit".to_string()),
                    step: HirExpr::IntLiteral(1),
                    body: vec![HirStmt::While {
                        test: HirExpr::BoolLiteral(false),
                        body: vec![HirStmt::If {
                            test: HirExpr::BoolLiteral(true),
                            body: vec![HirStmt::ExprStmt(HirExpr::Call {
                                callee: "print".to_string(),
                                args: vec![HirExpr::Name("item".to_string())],
                            })],
                            orelse: vec![],
                        }],
                    }],
                }],
            }],
        };
        check(&hir).unwrap();
    }

    #[test]
    fn unresolved_private_parameter_requests_an_annotation() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_constant".to_string(),
                params: vec![("unused".to_string(), Ty::Infer)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
            }],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("parameter `unused`"));
    }

    #[test]
    fn unresolved_private_return_requests_an_annotation() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_unknown".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Name("missing".to_string())))],
            }],
        };
        let err = check(&hir).unwrap_err();
        assert_eq!(err.code, "T0021");
        assert!(err.message.contains("return type"));
    }

    #[test]
    fn undefined_call_cannot_silently_resolve_a_private_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_unknown".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::Call {
                    callee: "missing".to_string(),
                    args: vec![],
                }))],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn unresolved_binary_operand_cannot_silently_resolve_a_private_return() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_unknown".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("missing".to_string())),
                    right: Box::new(HirExpr::IntLiteral(1)),
                }))],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn unresolved_private_binary_parameters_request_annotations() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_add".to_string(),
                params: vec![
                    ("left".to_string(), Ty::Infer),
                    ("right".to_string(), Ty::Infer),
                ],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(HirExpr::Name("left".to_string())),
                    right: Box::new(HirExpr::Name("right".to_string())),
                }))],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn unresolved_call_argument_does_not_invent_a_private_parameter_type() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::Name("missing".to_string())],
                })),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_parameter_inference_rejects_conflicting_call_sites() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::StringLiteral("one".to_string())],
                })),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_return_inference_rejects_conflicting_return_types() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_choose".to_string(),
                params: vec![("condition".to_string(), Ty::Bool)],
                return_ty: Ty::Infer,
                body: vec![HirStmt::If {
                    test: HirExpr::Name("condition".to_string()),
                    body: vec![HirStmt::Return(Some(HirExpr::IntLiteral(1)))],
                    orelse: vec![HirStmt::Return(Some(HirExpr::StringLiteral(
                        "one".to_string(),
                    )))],
                }],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0022");
    }

    fn nested_private_call_conflict() -> HirExpr {
        HirExpr::Call {
            callee: "_sink".to_string(),
            args: vec![HirExpr::Call {
                callee: "_identity".to_string(),
                args: vec![HirExpr::StringLiteral("wrong".to_string())],
            }],
        }
    }

    fn private_constraint_error_fixture(stmt: HirStmt) -> HirModule {
        HirModule {
            items: vec![
                HirItem::Function {
                    name: "_identity".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
                },
                HirItem::Function {
                    name: "_sink".to_string(),
                    params: vec![("value".to_string(), Ty::Int)],
                    return_ty: Ty::None,
                    body: vec![HirStmt::Return(None)],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_identity".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
                HirItem::Function {
                    name: "_probe".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![stmt],
                },
            ],
        }
    }

    #[test]
    fn private_f_string_and_return_propagate_nested_constraint_errors() {
        let stmt = HirStmt::Return(Some(HirExpr::FString(vec![FStringPart::Interpolation(
            Box::new(nested_private_call_conflict()),
        )])));
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_slice_base_wrapping_a_conflict_is_rejected_end_to_end() {
        // PR-12 Task 7 (D-118). NOTE what this test does and does NOT pin:
        // it proves `check()`'s real, end-to-end pipeline still rejects a
        // nested private-helper call conflict when it sits inside a
        // `Slice`'s `base`, *inside a private helper's own body* (not just
        // at module top level, which the pair above this whole group
        // already covers) -- a real and valuable thing to test on its own.
        // It does NOT, on its own, prove that `collect_expr_constraints`'s
        // `Slice` arm itself recurses into `base`: `check_with_environment`'s
        // Pass 3 independently re-type-checks every function body via
        // `infer_expr_in` regardless of what the solver's own
        // constraint-collection pass did or didn't visit, so this exact
        // conflict is still caught by Pass 3 even with the solver arm's
        // recursion mutated away to a bare `Ok(None)` no-op (confirmed by
        // temporarily making that mutation and rerunning this test: it kept
        // passing). `private_slice_base_recursion_pins_an_otherwise_unconstrained_parameter`
        // below is the test that actually is sensitive to the solver arm's
        // own recursion, using a construction where Pass 3 cannot mask a
        // missing visit (a `Ty::Infer` parameter whose *only* constraining
        // use anywhere in the module is inside a `Slice`'s `base`, so
        // signature resolution itself -- which gates whether Pass 3 ever
        // runs at all -- depends on the solver reaching it).
        let stmt = HirStmt::Return(Some(HirExpr::Slice {
            base: Box::new(nested_private_call_conflict()),
            start: None,
            stop: None,
            step: None,
        }));
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_slice_bound_wrapping_a_conflict_is_rejected_end_to_end() {
        // Same as above, but the conflict is nested inside a bound
        // (`start`), not `base`. Same caveat applies: this alone doesn't
        // pin the solver arm's own recursion (Pass 3 masks it identically),
        // see `private_slice_start_bound_recursion_pins_an_otherwise_unconstrained_parameter`
        // below for the test that actually does.
        let stmt = HirStmt::Return(Some(HirExpr::Slice {
            base: Box::new(HirExpr::IntLiteral(1)),
            start: Some(Box::new(nested_private_call_conflict())),
            stop: None,
            step: None,
        }));
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_slice_base_recursion_pins_an_otherwise_unconstrained_parameter() {
        // Load-bearing counterpart to the pair above (added after review:
        // those two tests' original doc comments overclaimed what they
        // pinned -- Pass 3 independently re-type-checks every function body
        // regardless of solver recursion, so a genuine conflict is caught
        // either way; verified by temporarily gutting the `Slice` arm in
        // `collect_expr_constraints` to a bare `Ok(None)` and confirming
        // those two tests still passed).
        //
        // This test's construction is different in a way that matters:
        // `_forward`'s own parameter `xs` is `Ty::Infer`, and its *only*
        // appearance anywhere in this module is wrapped in a call
        // (`_id_list(xs)`) embedded in a `Slice`'s `base`. Nothing else ever
        // constrains `xs`'s type. `collect_expr_constraints`'s `Call` arm is
        // the one place that performs real unification (`unify_terms`)
        // between an argument's term and the callee's own (here: concretely
        // annotated) parameter term -- so `xs`'s term becomes concretely
        // `list[int]` if and only if the solver actually visits
        // `_id_list(xs)`, which only happens if the outer `Slice` arm
        // recurses into `base`.
        //
        // If that recursion is missing, `xs`'s term never resolves, and
        // `infer_function_signatures_with_solver`'s own final resolution
        // loop raises "cannot infer type of parameter `xs`... add an
        // annotation" *before `check_with_signatures`/Pass 3 ever runs for
        // this function* -- Pass 3 cannot mask a failure that happens
        // before Pass 3 is even reached. Confirmed empirically: gutting the
        // `Slice` arm to a bare `Ok(None)` flips this test from `Ok(())` to
        // exactly that `Err`; restoring the real arm makes it pass again.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_id_list".to_string(),
                    params: vec![("v".to_string(), Ty::List(Box::new(Ty::Int)))],
                    return_ty: Ty::List(Box::new(Ty::Int)),
                    body: vec![HirStmt::Return(Some(HirExpr::Name("v".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("xs".to_string(), Ty::Infer)],
                    return_ty: Ty::List(Box::new(Ty::Int)),
                    body: vec![HirStmt::Return(Some(HirExpr::Slice {
                        base: Box::new(HirExpr::Call {
                            callee: "_id_list".to_string(),
                            args: vec![HirExpr::Name("xs".to_string())],
                        }),
                        start: Some(Box::new(HirExpr::IntLiteral(1))),
                        stop: None,
                        step: None,
                    }))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn private_slice_start_bound_recursion_pins_an_otherwise_unconstrained_parameter() {
        // Same load-bearing shape as the test above, but the embedded call
        // sits in a bound (`start`) rather than `base`: `_forward`'s own
        // parameter `x` is `Ty::Infer` and is used *only* wrapped in
        // `_id(x)` inside the slice's `start` bound. A bare `Name("x")`
        // bound would NOT be load-bearing here -- looking up a name in
        // `collect_expr_constraints` performs no unification of its own,
        // only the `Call` arm does, which is why this test wraps `x` in a
        // call rather than mirroring the brief-review's own bare `xs[x:]`
        // sketch literally.
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_id".to_string(),
                    params: vec![("v".to_string(), Ty::Int)],
                    return_ty: Ty::Int,
                    body: vec![HirStmt::Return(Some(HirExpr::Name("v".to_string())))],
                },
                HirItem::Function {
                    name: "_forward".to_string(),
                    params: vec![("x".to_string(), Ty::Infer)],
                    return_ty: Ty::List(Box::new(Ty::Int)),
                    body: vec![HirStmt::Return(Some(HirExpr::Slice {
                        base: Box::new(HirExpr::ListLiteral(vec![HirExpr::IntLiteral(1)])),
                        start: Some(Box::new(HirExpr::Call {
                            callee: "_id".to_string(),
                            args: vec![HirExpr::Name("x".to_string())],
                        })),
                        stop: None,
                        step: None,
                    }))],
                },
            ],
        };
        assert!(check(&hir).is_ok());
    }

    #[test]
    fn private_if_body_propagates_nested_constraint_errors() {
        let stmt = HirStmt::If {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(nested_private_call_conflict())],
            orelse: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_while_body_propagates_nested_constraint_errors() {
        let stmt = HirStmt::While {
            test: HirExpr::BoolLiteral(true),
            body: vec![HirStmt::ExprStmt(nested_private_call_conflict())],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_range_operand_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ForRange {
            var: "item".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: nested_private_call_conflict(),
            step: HirExpr::IntLiteral(1),
            body: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_range_body_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ForRange {
            var: "item".to_string(),
            start: HirExpr::IntLiteral(0),
            stop: HirExpr::IntLiteral(1),
            step: HirExpr::IntLiteral(1),
            body: vec![HirStmt::ExprStmt(nested_private_call_conflict())],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_compare_left_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(nested_private_call_conflict()),
            right: Box::new(HirExpr::IntLiteral(1)),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_compare_right_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::Compare {
            op: CmpOpKind::Eq,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(nested_private_call_conflict()),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_binary_left_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(nested_private_call_conflict()),
            right: Box::new(HirExpr::IntLiteral(1)),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_binary_right_propagates_nested_constraint_errors() {
        let stmt = HirStmt::ExprStmt(HirExpr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(HirExpr::IntLiteral(1)),
            right: Box::new(nested_private_call_conflict()),
        });
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_assignment_propagates_nested_constraint_errors() {
        let stmt = HirStmt::Assign {
            target: "value".to_string(),
            value: nested_private_call_conflict(),
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_assignment_with_an_unresolved_value_is_checked_after_inference() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "_assign".to_string(),
                params: vec![],
                return_ty: Ty::Infer,
                body: vec![HirStmt::Assign {
                    target: "value".to_string(),
                    value: HirExpr::Name("missing".to_string()),
                }],
            }],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_if_test_propagates_nested_constraint_errors() {
        let stmt = HirStmt::If {
            test: nested_private_call_conflict(),
            body: vec![],
            orelse: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_while_test_propagates_nested_constraint_errors() {
        let stmt = HirStmt::While {
            test: nested_private_call_conflict(),
            body: vec![],
        };
        assert_eq!(
            check(&private_constraint_error_fixture(stmt))
                .unwrap_err()
                .code,
            "T0021"
        );
    }

    #[test]
    fn private_range_parameter_rejects_a_conflicting_call_site_type() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_loop".to_string(),
                    params: vec![("limit".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::ForRange {
                        var: "item".to_string(),
                        start: HirExpr::IntLiteral(0),
                        stop: HirExpr::Name("limit".to_string()),
                        step: HirExpr::IntLiteral(1),
                        body: vec![],
                    }],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_loop".to_string(),
                    args: vec![HirExpr::StringLiteral("wrong".to_string())],
                })),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_implicit_none_return_rejects_an_int_constrained_call_site() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_noop".to_string(),
                    params: vec![],
                    return_ty: Ty::Infer,
                    body: vec![],
                },
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Call {
                        callee: "_noop".to_string(),
                        args: vec![],
                    },
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0022");
    }

    #[test]
    fn private_division_return_rejects_an_int_constrained_call_site() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_ratio".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Div,
                        left: Box::new(HirExpr::Name("value".to_string())),
                        right: Box::new(HirExpr::IntLiteral(2)),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::ForRange {
                    var: "item".to_string(),
                    start: HirExpr::IntLiteral(0),
                    stop: HirExpr::Call {
                        callee: "_ratio".to_string(),
                        args: vec![HirExpr::IntLiteral(4)],
                    },
                    step: HirExpr::IntLiteral(1),
                    body: vec![],
                }),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn private_binary_constraint_rejects_incompatible_resolved_operands() {
        let hir = HirModule {
            items: vec![
                HirItem::Function {
                    name: "_bad_add".to_string(),
                    params: vec![("value".to_string(), Ty::Infer)],
                    return_ty: Ty::Infer,
                    body: vec![HirStmt::Return(Some(HirExpr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(HirExpr::Name("value".to_string())),
                        right: Box::new(HirExpr::StringLiteral("wrong".to_string())),
                    }))],
                },
                HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                    callee: "_bad_add".to_string(),
                    args: vec![HirExpr::IntLiteral(1)],
                })),
            ],
        };
        assert_eq!(check(&hir).unwrap_err().code, "T0021");
    }

    #[test]
    fn direct_check_function_rejects_an_unresolved_private_signature() {
        let function = HirItem::Function {
            name: "_identity".to_string(),
            params: vec![("value".to_string(), Ty::Infer)],
            return_ty: Ty::Infer,
            body: vec![HirStmt::Return(Some(HirExpr::Name("value".to_string())))],
        };
        assert_eq!(check_function(&function).unwrap_err().code, "T0021");
    }

    #[test]
    fn type_solver_covers_concrete_and_union_merge_paths() {
        let mut parents = Vec::new();
        let mut concrete = Vec::new();
        assert!(
            !unify_terms(
                Ok(Ty::Int),
                Ok(Ty::Bool),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );
        assert!(
            unify_terms(
                Ok(Ty::Int),
                Ok(Ty::Str),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .is_err()
        );

        let empty_left = fresh_term(&mut parents, &mut concrete);
        let empty_right = fresh_term(&mut parents, &mut concrete);
        assert!(
            unify_terms(
                empty_left.clone(),
                empty_right.clone(),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );
        assert!(
            !unify_terms(
                empty_left,
                empty_right,
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );

        let typed_left = fresh_term(&mut parents, &mut concrete);
        let typed_right = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            typed_left.clone(),
            Ok(Ty::Bool),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed_right.clone(),
            Ok(Ty::Int),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed_left,
            typed_right.clone(),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        assert_eq!(
            resolved_term(typed_right, &mut parents, &concrete),
            Some(Ty::Int)
        );

        let typed = fresh_term(&mut parents, &mut concrete);
        let empty = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            typed.clone(),
            Ok(Ty::Str),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(typed, empty.clone(), &mut parents, &mut concrete, "T0021", "test").unwrap();
        assert_eq!(resolved_term(empty, &mut parents, &concrete), Some(Ty::Str));

        let conflicting_left = fresh_term(&mut parents, &mut concrete);
        let conflicting_right = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            conflicting_left.clone(),
            Ok(Ty::Int),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            conflicting_right.clone(),
            Ok(Ty::Str),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        assert!(
            unify_terms(
                conflicting_left,
                conflicting_right,
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .is_err()
        );

        let reversed = fresh_term(&mut parents, &mut concrete);
        assert!(
            unify_terms(
                Ok(Ty::Float),
                reversed.clone(),
                &mut parents,
                &mut concrete,
                "T0021",
                "test",
            )
            .unwrap()
        );
        assert_eq!(
            resolved_term(reversed, &mut parents, &concrete),
            Some(Ty::Float)
        );
    }
}
