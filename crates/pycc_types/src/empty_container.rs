//! Empty-container element-type resolution (#1021, D-245).
//!
//! An empty `[]` or `{}` carries no element type of its own, and this
//! compiler has no bidirectional/expected-type inference to hand one down
//! from the surrounding context. Before this pass existed, *every* empty
//! container literal was a hard error -- including the annotated
//! `xs: list[int] = []` form (#927).
//!
//! This module is the single place that resolves one. It is an infallible
//! HIR-to-HIR rewrite that runs **once, at the top of both
//! [`crate::check_all_keyed`] and [`crate::check_and_resolve_all_keyed`]**,
//! replacing a resolvable `HirExpr::ListLiteral(vec![])` with
//! [`HirExpr::EmptyList`] and a resolvable `HirExpr::DictLiteral(vec![])`
//! with [`HirExpr::EmptyDict`]. Running *before* checking, rather than as a
//! post-check resolve phase, is what makes the pass's key safety property
//! structural instead of merely asserted: `pycc check` and `pycc build`
//! consume the identical rewritten module, so `pycc check` cannot accept a
//! program that `pycc build` then panics on in `MirExpr::ty()`. See
//! `docs/decisions/D-245-*.md` for the alternatives this rejected.
//!
//! The element type comes from exactly three sources, in priority order:
//!
//! 1. the annotation on an `AnnAssign` target (`xs: list[int] = []`) --
//!    purely syntactic, needing no environment at all;
//! 2. *any* successfully-inferred binding for the target anywhere in the
//!    enclosing function -- not only one that precedes the literal;
//! 3. a forward scan of the enclosing function body for the first
//!    *producer* use of the target -- `xs.append(v)` or `d[k] = v`.
//!
//! Sources 2 and 3 are best-effort: they use the same
//! `bind_local_types_in_body` pre-pass environment the protocol
//! monomorphization pass already builds, and they swallow inference
//! failures exactly as that function does. A container this pass cannot
//! resolve is left as the empty literal it was, and the check phase then
//! reports `T0003`.
//!
//! **Source 2 is order-insensitive, not a backward scan.** The whole-function
//! environment is built by a single forward `bind_local_types_in_body` pass
//! that completes *before* any rewriting, and is then reused for every
//! occurrence regardless of position, so `xs = [1]; ...; xs = []` and
//! `xs = []; xs = [1]; ...` resolve identically. That is safe because this
//! pass resolves but never accepts: `pycc_hir::check_container_ty` admits
//! only `list[int]` and `dict[str, int]`, so a resolution is either the one
//! element type the program could have compiled with or a `T0034`/`T0036`,
//! and the check phase re-validates the function in true program order
//! (D-040's sticky-representation rule included). Restricting source 2 to
//! strictly-prior bindings could only turn some compilable programs into
//! `T0003`; it could not change which accepted program is produced. The
//! "only a fully concrete `Ty` is ever stored" invariant is likewise enforced
//! at that downstream gate rather than here: this pass performs no
//! `Ty::Infer` check of its own, and a resolved `Ty::Infer` cannot survive
//! `check_container_ty` to reach MIR.
//!
//! The same completed-before-rewriting property is why
//! `bind_local_types_in_stmt`'s `AnnAssign` arm seeds the declared annotation
//! when the value does not infer: `xs: list[int] = []`'s raw literal is not
//! yet rewritten when the environment is built, so without that fallback the
//! environment never learned `xs: list[int]` and a resolution derived from
//! `xs` (`for x in xs: ys.append(x)`) reported a spurious `T0003`.
//!
//! **Producers, not consumers.** `for x in xs`, `xs[0]`, `len(xs)` and
//! `xs.pop()` *read* an element type that is already known; in a single
//! forward synthesis pass with no backward unification they cannot supply
//! one. Only `HirExpr::ListAppend` and `HirStmt::DictSet` can. `HirExpr::
//! SetAdd` is structurally dead as a producer -- `{}` parses as a dict and
//! `set()` is rejected at `pycc_hir` lowering -- so there is deliberately no
//! set path here at all.
//!
//! **Scope.** Function bodies only (class methods included: they lower to
//! ordinary mangled `HirItem::Function` items). Module-level statements keep
//! failing, now with `T0003`. Nested `match`/`try` bodies are not scanned for
//! producers; an empty container that would need one resolves to `T0003`
//! rather than silently to a wrong type.

use super::*;

/// What a resolution produced: the *element* type of a list, or the
/// key/value pair of a dict.
enum Resolution {
    List(Ty),
    Dict(Ty, Ty),
}

impl Resolution {
    /// The rewritten node this resolution produces.
    fn into_expr(self) -> HirExpr {
        match self {
            Resolution::List(element) => HirExpr::EmptyList(element),
            Resolution::Dict(key, value) => HirExpr::EmptyDict(Box::new((key, value))),
        }
    }

    /// Whether this resolution's shape matches the literal being rewritten.
    /// An `xs: list[int] = {}` annotation resolves to a list while the
    /// literal is a dict. Rewriting `{}` into a typed empty *list* would
    /// silently repair a program the user got wrong, so the literal is left
    /// alone and the check phase reports `T0003` against it.
    fn matches(&self, literal: EmptyLiteral) -> bool {
        matches!(
            (self, literal),
            (Resolution::List(_), EmptyLiteral::List) | (Resolution::Dict(..), EmptyLiteral::Dict)
        )
    }
}

/// Which empty literal shape a node is, when it is one at all.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EmptyLiteral {
    List,
    Dict,
}

fn empty_literal(expr: &HirExpr) -> Option<EmptyLiteral> {
    match expr {
        HirExpr::ListLiteral(elements) if elements.is_empty() => Some(EmptyLiteral::List),
        HirExpr::DictLiteral(pairs) if pairs.is_empty() => Some(EmptyLiteral::Dict),
        _ => None,
    }
}

/// Rewrites every resolvable empty container in `hir`, returning `None` when
/// the module contains no empty container literal in a function body at all
/// -- which is the overwhelmingly common case, and which keeps this pass
/// from adding a whole-module clone to `check_all_keyed`, a path that does
/// not otherwise clone.
pub(crate) fn resolve_empty_containers(hir: &HirModule) -> Option<HirModule> {
    if !hir.items.iter().any(|item| match item {
        HirItem::Function { body, .. } => body_has_empty_literal(body),
        HirItem::TopLevelStmt(_) => false,
    }) {
        return None;
    }
    let module_env = concrete_function_environment(hir).unwrap_or_default();
    let local_names = crate::module::module_function_local_names(hir);
    let mut resolved = hir.clone();
    for (index, item) in resolved.items.iter_mut().enumerate() {
        let HirItem::Function { params, body, .. } = item else {
            continue;
        };
        let names = &local_names[index];
        let mut env = module_env.child_for_function(names);
        for (param_name, param_ty) in params.iter() {
            env.bind(param_name.clone(), param_ty.clone());
        }
        crate::bind_local_types_in_body(&mut env, names, body);
        let producers = body.clone();
        rewrite_body(body, &producers, &env, names);
    }
    Some(resolved)
}

fn body_has_empty_literal(body: &[HirStmt]) -> bool {
    body.iter().any(|stmt| {
        let own = match stmt {
            HirStmt::Assign { value, .. } => empty_literal(value).is_some(),
            HirStmt::AnnAssign { value, .. } => {
                value.as_ref().is_some_and(|v| empty_literal(v).is_some())
            }
            _ => false,
        };
        own || nested_bodies(stmt)
            .iter()
            .any(|b| body_has_empty_literal(b))
    })
}

/// The nested statement sequences of a block statement. `match`/`try` bodies
/// are deliberately absent -- see this module's own doc comment.
fn nested_bodies(stmt: &HirStmt) -> Vec<&[HirStmt]> {
    match stmt {
        HirStmt::If { body, orelse, .. } => vec![body, orelse],
        HirStmt::While { body, .. } => vec![body],
        HirStmt::ForRange { body, .. } => vec![body],
        HirStmt::ForList { body, .. } => vec![body],
        _ => Vec::new(),
    }
}

fn rewrite_body(
    body: &mut [HirStmt],
    producers: &[HirStmt],
    env: &Environment,
    local_names: &[&str],
) {
    for stmt in body.iter_mut() {
        match stmt {
            HirStmt::Assign { target, value } => {
                rewrite_value(value, target, None, producers, env, local_names);
            }
            HirStmt::AnnAssign {
                target,
                annotation,
                value: Some(value),
                ..
            } => {
                rewrite_value(value, target, Some(annotation), producers, env, local_names);
            }
            HirStmt::If { body, orelse, .. } => {
                rewrite_body(body, producers, env, local_names);
                rewrite_body(orelse, producers, env, local_names);
            }
            HirStmt::While { body, .. } => rewrite_body(body, producers, env, local_names),
            HirStmt::ForRange { body, .. } => rewrite_body(body, producers, env, local_names),
            HirStmt::ForList { body, .. } => rewrite_body(body, producers, env, local_names),
            _ => {}
        }
    }
}

fn rewrite_value(
    value: &mut HirExpr,
    target: &str,
    annotation: Option<&Ty>,
    producers: &[HirStmt],
    env: &Environment,
    local_names: &[&str],
) {
    let Some(literal) = empty_literal(value) else {
        return;
    };
    let Some(resolution) = resolve(target, annotation, producers, env, local_names) else {
        return;
    };
    if resolution.matches(literal) {
        *value = resolution.into_expr();
    }
}

/// The three-source priority order. Note that a resolved type is *not*
/// validated here: an inferred `list[str]` is stored and then rejected by
/// `pycc_hir::check_container_ty` (`T0034`) in the check phase, exactly as a
/// written `xs: list[str] = [1]` annotation is -- D-228's rule that a
/// written annotation and an inferred literal share one gate.
fn resolve(
    target: &str,
    annotation: Option<&Ty>,
    producers: &[HirStmt],
    env: &Environment,
    local_names: &[&str],
) -> Option<Resolution> {
    if let Some(from_annotation) = annotation.and_then(from_container_ty) {
        return Some(from_annotation);
    }
    if let Some(from_binding) = env
        .binding_state(target)
        .map(BindingState::ty)
        .and_then(from_container_ty)
    {
        return Some(from_binding);
    }
    find_producer(producers, target, env, local_names)
}

fn from_container_ty(ty: &Ty) -> Option<Resolution> {
    match ty {
        Ty::List(element) => Some(Resolution::List((**element).clone())),
        Ty::Dict(pair) => Some(Resolution::Dict(pair.0.clone(), pair.1.clone())),
        _ => None,
    }
}

/// The first producer use of `target` anywhere in `body`, in source order.
///
/// Scanning the whole function body from its start -- rather than only
/// forward from the assignment being resolved -- is what implements the
/// **first-wins-within-a-scope** rule for a name assigned `[]` in more than
/// one branch: `if c: xs = []; xs.append(1)` / `else: xs = [];
/// xs.append("a")` resolves *both* nodes from the first producer, and the
/// second branch's `append` then fails with the ordinary element-type
/// mismatch. A name-keyed resolution cannot represent two different types
/// for one binding, and neither can the binding itself.
fn find_producer(
    body: &[HirStmt],
    target: &str,
    env: &Environment,
    local_names: &[&str],
) -> Option<Resolution> {
    for stmt in body {
        let found = match stmt {
            HirStmt::ExprStmt(HirExpr::ListAppend { list, value }) if list == target => {
                crate::infer_expr_in(env, local_names, value)
                    .ok()
                    .map(Resolution::List)
            }
            HirStmt::DictSet { dict, key, value } if dict == target => {
                match (
                    crate::infer_expr_in(env, local_names, key),
                    crate::infer_expr_in(env, local_names, value),
                ) {
                    (Ok(key_ty), Ok(value_ty)) => Some(Resolution::Dict(key_ty, value_ty)),
                    _ => None,
                }
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        for nested in nested_bodies(stmt) {
            if let Some(resolution) = find_producer(nested, target, env, local_names) {
                return Some(resolution);
            }
        }
    }
    None
}

/// `T0003` for an empty list literal no source of evidence could type.
pub(crate) fn unresolved_list() -> Diagnostic {
    Diagnostic::error(
        "T0003",
        "an empty list literal has no inferable element type here".to_string(),
        Span::new(0, 0),
    )
    .with_help(
        "annotate the assignment target (`xs: list[int] = []`) or append a value to it".to_string(),
    )
}

/// `T0003` for an empty dict literal no source of evidence could type.
pub(crate) fn unresolved_dict() -> Diagnostic {
    Diagnostic::error(
        "T0003",
        "an empty dict literal has no inferable key/value types here".to_string(),
        Span::new(0, 0),
    )
    .with_help(
        "annotate the assignment target (`d: dict[str, int] = {}`) or assign an entry into it"
            .to_string(),
    )
}

/// Names the binding in a `T0003` raised while checking `target`'s assigned
/// `value`. `T0003`'s span is the `Span::new(0, 0)` every container diagnostic
/// in this crate uses (`HirStmt::Assign` carries no span at all), so the
/// binding's name in the message is the only locator a user gets.
///
/// The substitution is gated on `value` *itself* being the empty literal that
/// failed (#1021 review round 1). A `T0003` also propagates out of a nested
/// element position -- `xs: list[int] = [[]]` fails on the inner `[]` while
/// `xs` is validly annotated -- and naming `xs` there points the user at the
/// wrong node. Such a diagnostic keeps `unresolved_list`/`unresolved_dict`'s
/// generic `" here"` wording, which is the only honest locator available when
/// the failing node is not the assigned value.
pub(crate) fn name_binding(
    mut diagnostic: Diagnostic,
    target: &str,
    value: &HirExpr,
) -> Diagnostic {
    if diagnostic.code == "T0003" && empty_literal(value).is_some() {
        diagnostic.message = diagnostic
            .message
            .replace(" here", &format!(" for `{target}`"));
        diagnostic.label = Some(diagnostic.message.clone());
    }
    diagnostic
}
