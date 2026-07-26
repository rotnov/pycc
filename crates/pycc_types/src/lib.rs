use pycc_diag::{Diagnostic, Span};
#[cfg(test)]
use pycc_hir::CmpOpKind;
pub use pycc_hir::Ty;
use pycc_hir::{BinOpKind, FStringPart, HirExpr, HirItem, HirModule, HirStmt};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub struct Environment {
    bindings: HashMap<String, Ty>,
    functions: Arc<HashMap<String, (Vec<Ty>, Ty)>>,
}

impl Environment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, name: &str) -> Option<Ty> {
        self.bindings.get(name).copied()
    }

    pub fn bind(&mut self, name: String, ty: Ty) {
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
            HirStmt::ExprStmt(_) | HirStmt::Return(_) => {}
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
        Err(var) => concrete[root(parents, var)],
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
        (Ok(left), Ok(right)) => merge_inferred_types(left, right)
            .map(|_| false)
            .ok_or_else(|| inference_conflict(code, context, left, right)),
        (Err(var), Ok(ty)) | (Ok(ty), Err(var)) => {
            let root = root(parents, var);
            let merged = match concrete[root] {
                Some(current) => merge_inferred_types(current, ty)
                    .ok_or_else(|| inference_conflict(code, context, current, ty))?,
                None => ty,
            };
            let changed = concrete[root] != Some(merged);
            concrete[root] = Some(merged);
            Ok(changed)
        }
        (Err(left), Err(right)) => {
            let left_root = root(parents, left);
            let right_root = root(parents, right);
            if left_root == right_root {
                return Ok(false);
            }
            let merged = match (concrete[left_root], concrete[right_root]) {
                (Some(left), Some(right)) => Some(
                    merge_inferred_types(left, right)
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
        HirExpr::Name(name) => match env.bindings.get(name).copied() {
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
                    binops.push((*op, left, right, result));
                    Ok(Some(result))
                }
                _ => Ok(None),
            }
        }
        HirExpr::Call { callee, args } => {
            if is_local(env.local_names, callee) {
                return if env.bindings.contains_key(callee) {
                    Err(non_callable_binding(callee))
                } else {
                    Err(unbound_local(callee))
                };
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
                    && matches!((arg, parameter), (Err(_), _) | (_, Err(_)))
                {
                    unify_terms(
                        *parameter,
                        arg,
                        parents,
                        concrete,
                        "T0021",
                        &format!("argument {} of private helper `{callee}`", index + 1),
                    )?;
                }
            }
            Ok(Some(signature.2))
        }
    }
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
                if let Some(term) =
                    collect_expr_constraints(signatures, parents, concrete, binops, env, value)?
                {
                    env.bindings.entry(target.clone()).or_insert(term);
                }
            }
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
                    return_term,
                )?;
                collect_block_constraints(
                    signatures,
                    parents,
                    concrete,
                    binops,
                    env,
                    orelse,
                    return_term,
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
                    return_term,
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
                if let Some(existing) = env.bindings.get(var).copied() {
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
                    return_term,
                )?;
            }
            HirStmt::Return(value) => {
                let Some(return_term) = return_term else {
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
        }
    }
    Ok(())
}

fn contains_return(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        HirStmt::Return(_) => true,
        HirStmt::If { body, orelse, .. } => contains_return(body) || contains_return(orelse),
        HirStmt::While { body, .. } | HirStmt::ForRange { body, .. } => contains_return(body),
        HirStmt::ExprStmt(_) | HirStmt::Assign { .. } => false,
    })
}

fn infer_function_signatures(
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
                        .map(|(_, ty)| term_for_type(*ty, &mut parents, &mut concrete))
                        .collect(),
                    term_for_type(*return_ty, &mut parents, &mut concrete),
                ),
            );
        }
    }

    let mut binops = Vec::new();
    let mut globals = ConstraintEnvironment {
        bindings: HashMap::new(),
        local_names: &[],
    };
    for item in &hir.items {
        if let HirItem::TopLevelStmt(stmt) = item {
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
    }
    for (item, local_names) in hir.items.iter().zip(function_local_names) {
        let HirItem::Function { name, body, .. } = item else {
            continue;
        };
        let signature = &signatures[name];
        let mut env = ConstraintEnvironment {
            bindings: globals.bindings.clone(),
            local_names,
        };
        for local_name in local_names.iter().copied() {
            env.bindings.remove(local_name);
        }
        for (param_name, param_ty) in signature.0.iter().zip(&signature.1) {
            env.bindings.insert(param_name.clone(), *param_ty);
        }
        collect_block_constraints(
            &signatures,
            &mut parents,
            &mut concrete,
            &mut binops,
            &mut env,
            body,
            Some(signature.2),
        )?;
        if signature.2.is_err() && !contains_return(body) {
            unify_terms(
                signature.2,
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
        for &(op, left_term, right_term, result_term) in &binops {
            let left = resolved_term(left_term, &mut parents, &concrete);
            let right = resolved_term(right_term, &mut parents, &concrete);
            let result = resolved_term(result_term, &mut parents, &concrete);
            if let (Some(left), Some(right)) = (left, right) {
                let result_ty = numeric_result_type(op, left, right)?;
                changed |= unify_terms(
                    result_term,
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
                    left_term,
                    Ok(Ty::Int),
                    &mut parents,
                    &mut concrete,
                    "T0021",
                    "left operand of int binary expression",
                )?;
                let right_changed = unify_terms(
                    right_term,
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
            .zip(signature.1.iter().copied())
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
        let return_ty = resolved_term(signature.2, &mut parents, &concrete).ok_or_else(|| {
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
            if numeric_or_bool_compatible(left_ty, right_ty) {
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
            if is_local(local_names, callee) {
                return if env.lookup(callee).is_some() {
                    Err(non_callable_binding(callee))
                } else {
                    Err(unbound_local(callee))
                };
            }
            let arg_tys = args
                .iter()
                .map(|a| infer_expr_in(env, local_names, a))
                .collect::<Result<Vec<_>, _>>()?;
            if callee == "print" {
                return Ok(Ty::None); // print's own signature isn't user-declarable in v0.1
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
                if !is_assignable(*arg_ty, *param_ty) {
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
            Ok(*return_ty)
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
    let as_numeric = |t: Ty| match t {
        Ty::Bool | Ty::Int => Some(Ty::Int),
        Ty::Float => Some(Ty::Float),
        _ => None,
    };
    match (as_numeric(left), as_numeric(right)) {
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
    let is_numeric_like = |t: Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool);
    (is_numeric_like(a) && is_numeric_like(b)) || (a == Ty::Str && b == Ty::Str)
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
    if is_assignable(actual, Ty::Int) {
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
    if let Some(previous) = env.lookup(target) {
        if !is_assignable(ty, previous) {
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

pub fn check_stmt(env: &mut Environment, stmt: &HirStmt) -> Result<(), Diagnostic> {
    match stmt {
        HirStmt::Assign { target, value } => {
            let ty = infer_expr(env, value)?;
            check_assignment(env, target, ty)
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
        HirStmt::Return(_) => Err(Diagnostic::error(
            "T0024",
            "'return' outside a function is not allowed".to_string(),
            Span::new(0, 0),
        )),
    }
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
            (param_tys.as_slice(), *return_ty, true)
        } else {
            standalone_params = params.iter().map(|(_, ty)| *ty).collect::<Vec<_>>();
            (standalone_params.as_slice(), *return_ty, false)
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
        env.bind_function(name.clone(), resolved_params.to_vec(), resolved_return);
    }
    for ((param_name, _), param_ty) in params.iter().zip(resolved_params.iter().copied()) {
        env.bind(param_name.clone(), param_ty);
    }
    for stmt in body {
        check_stmt_in_function(&mut env, local_names, stmt, resolved_return)?;
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
        | HirStmt::While { .. }
        | HirStmt::ForRange { .. } => false,
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
            if !is_assignable(actual, return_ty) {
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
                check_stmt_in_function(env, local_names, s, return_ty)?;
            }
            for s in orelse {
                check_stmt_in_function(env, local_names, s, return_ty)?;
            }
            Ok(())
        }
        HirStmt::While { test, body } => {
            infer_expr_in(env, local_names, test)?;
            for s in body {
                check_stmt_in_function(env, local_names, s, return_ty)?;
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
                check_stmt_in_function(env, local_names, s, return_ty)?;
            }
            Ok(())
        }
        HirStmt::Assign { target, value } => {
            let ty = infer_expr_in(env, local_names, value)?;
            check_assignment(env, target, ty)
        }
        HirStmt::ExprStmt(expr) => infer_expr_in(env, local_names, expr).map(|_| ()),
    }
}

/// Type-checks a module and returns a cloned HIR whose function signatures
/// contain only the concrete types resolved by private-helper inference.
/// Consumers after the type boundary must use this module rather than the
/// unresolved lowering result so `Ty::Infer` can never leak into MIR or code
/// generation.
pub fn check_and_resolve(hir: &HirModule) -> Result<HirModule, Diagnostic> {
    let function_local_names = module_function_local_names(hir);
    let signatures = infer_function_signatures(hir, &function_local_names)?;
    check_with_signatures(hir, &signatures, &function_local_names)?;

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
            *param_ty = *resolved_ty;
        }
        *return_ty = *resolved_return;
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
            env.bind_function(name.clone(), param_tys.clone(), *return_ty);
        }
    }
    // Pass 2: check every top-level statement in source order, growing
    // `env`'s bindings as module-level assignments are encountered --
    // ordinary top-level code is still checked top-to-bottom (a top-level
    // forward reference to a not-yet-assigned name is a genuine error).
    for item in &hir.items {
        if let HirItem::TopLevelStmt(stmt) = item {
            check_stmt(&mut env, stmt)?;
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
    let signatures = infer_function_signatures(hir, &function_local_names)?;
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
                empty_left,
                empty_right,
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
            typed_left,
            Ok(Ty::Bool),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed_right,
            Ok(Ty::Int),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            typed_left,
            typed_right,
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
            typed,
            Ok(Ty::Str),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(typed, empty, &mut parents, &mut concrete, "T0021", "test").unwrap();
        assert_eq!(resolved_term(empty, &mut parents, &concrete), Some(Ty::Str));

        let conflicting_left = fresh_term(&mut parents, &mut concrete);
        let conflicting_right = fresh_term(&mut parents, &mut concrete);
        unify_terms(
            conflicting_left,
            Ok(Ty::Int),
            &mut parents,
            &mut concrete,
            "T0021",
            "test",
        )
        .unwrap();
        unify_terms(
            conflicting_right,
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
                reversed,
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
