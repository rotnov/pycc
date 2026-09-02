//! Expression lowering: `lower_expr` and its comprehension/range helper
//! cluster, the recursive-descent core that turns a `pycc_ast::Expr` into a
//! `HirExpr` (or a capability / context-invalidity diagnostic).
//!
//! Extracted from `lib.rs` per AGENTS.md's file-decomposition rule (issue
//! #361, mirroring #141's `stmt.rs` extraction, D-148): `lower_expr` and
//! every helper it needs formed one cohesive, already-separable unit, called
//! only from `lib.rs`, `stmt.rs` (a sibling module), and two unit tests in the
//! crate root's `mod tests` (`tests.rs`, moved out of `lib.rs` by issue #547)
//! that call `lower_comprehension_header` and `rename_name_in_expr` directly,
//! bypassing the public `lower_checked` entry point -- unlike `stmt.rs`'s own
//! extraction, this one is not fully test-transparent, so those two tests
//! stayed with the crate root's test module, which reaches them through an
//! explicit `use crate::expr::{...}` instead of them moving here.
//!
//! `in_function: bool` (D-149) is this module's new piece of state, mirroring
//! `in_loop`'s existing shape (`stmt.rs`, D-148) but on the expression side:
//! `true` exactly when the expression being lowered is (transitively) inside
//! a real function body reached via `lower_function`'s own dispatch; `false`
//! at module scope. It exists so `Expr::Yield`/`Expr::YieldFrom` can
//! distinguish a context-invalid occurrence (no enclosing function -- CPython
//! raises `SyntaxError`, so this reuses `L0001`, the same D-148 precedent) from
//! a valid-but-unimplemented occurrence (a real enclosing function -- still
//! `C0001`, generator codegen remains out of scope). Every position lexically
//! *inside a comprehension*'s own scope (`if`-filter `cond`, `elt`, `key`,
//! `value`, and -- as a documented, narrower exception, see
//! `lower_comprehension_iter` below -- the comprehension's outermost
//! iterable) instead hardcodes a literal `true`, deliberately preserving
//! today's exact `C0001`-in-both-scopes behavior for a comprehension-internal
//! `yield`/`yield from`: CPython's real rule there is a third,
//! scope-independent classification (`'yield' inside list comprehension`)
//! this issue does not implement (see D-149 for the full rationale). This is
//! why the five comprehension-helper functions below need no new parameter at
//! all -- they never forward the ambient `in_function` value, only the one
//! literal that reproduces current behavior.

use crate::int_boundary::check_boundary_literal;
use crate::{
    BinOpKind, CmpOpKind, CompIter, FStringPart, HirExpr, HirStmt, Ty, UnaryOpKind,
    context_invalid, unsupported,
};
use pycc_ast::{CmpOp, Expr, Int, Number, Operator, UnaryOp};
use pycc_diag::Diagnostic;

/// Resolves a PEP 695 generic-class type argument (the `int` in `C[int]`)
/// to a `Ty`. PEP 695 generic class instantiation is scoped to scalar-only
/// types (D-133/D-134), so only `int`/`float`/`bool`/`str` are recognized —
/// matching `annotation_to_ty`'s own bare-name-to-`Ty` mapping for those
/// four scalars, without needing the full `aliases`/`type_param`/`class_name`
/// context `annotation_to_ty` threads for method annotations.
fn type_arg_name_to_ty(slice: &Expr) -> Result<Ty, Diagnostic> {
    let Expr::Name(name) = slice else {
        return Err(unsupported(
            "a generic class type argument must be a bare type name (int/float/bool/str) \
             so far -- subscript expressions are not supported yet",
            pycc_ast::expr_range(slice),
        ));
    };
    match name.id.as_str() {
        "int" => Ok(Ty::Int),
        "float" => Ok(Ty::Float),
        "bool" => Ok(Ty::Bool),
        "str" => Ok(Ty::Str),
        other => Err(unsupported(
            format!(
                "a generic class type argument `{other}` is not supported yet \
                 -- only int/float/bool/str are (D-133/D-134 scalar-only scope)"
            ),
            pycc_ast::expr_range(slice),
        )),
    }
}

/// #433: Recognizes a zero-arg `super()` call expression — `Expr::Call`
/// whose `func` is `Expr::Name("super")` and whose argument list is empty.
/// Used by `lower_expr`'s `Expr::Call` and `Expr::Attribute` arms to detect
/// `super().method(args)` and `super().attr` respectively, lowering both
/// to a `HirExpr::Super` base instead of letting the `super` name fall
/// through to the ordinary (unsupported-builtin) `Call` path.
pub(crate) fn is_zero_arg_super_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Name(name) = call.func.as_ref() else {
        return false;
    };
    name.id.as_str() == "super"
        && call.arguments.keywords.is_empty()
        && call.arguments.args.is_empty()
}

/// #602: applies a source-level unary sign to an integer literal's own
/// magnitude, in the literal's arbitrary-precision form, *before* the `i64`
/// range check.
///
/// The order matters for exactly one value. `ruff`'s `Int` stores its
/// magnitude as a `u64`, and `-9223372036854775808` parses as `USub` applied
/// to the literal `9223372036854775808` -- a magnitude that does not fit in
/// an `i64` even though its negation is precisely `i64::MIN`. Range-checking
/// the operand first (what `lower_expr`'s own unsigned `Number::Int` arm
/// does, correctly, for an unsigned literal) would reject that source as out
/// of range. Checking after the sign is applied accepts it.
fn fold_int_literal_sign(
    value: &Int,
    negate: bool,
    range: std::ops::Range<u32>,
) -> Result<i64, Diagnostic> {
    let magnitude = value.as_u64();
    let folded = match magnitude {
        // `i64::MIN`'s magnitude is one past `i64::MAX`, so it is
        // representable only when the sign is actually negative.
        Some(m) if negate && m == (i64::MAX as u64) + 1 => Some(i64::MIN),
        Some(m) => i64::try_from(m).ok().map(|v| if negate { -v } else { v }),
        None => None,
    };
    folded.ok_or_else(|| {
        unsupported(
            format!("integer literal does not fit in i64: {value:?}"),
            range,
        )
    })
}

pub(crate) fn lower_expr(
    expr: &Expr,
    in_function: bool,
    class_name: Option<&str>,
) -> Result<HirExpr, Diagnostic> {
    let lowered = match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(i) => {
                let Some(value) = i.as_i64() else {
                    return Err(unsupported(
                        format!("integer literal does not fit in i64: {i:?}"),
                        lit.range,
                    ));
                };
                HirExpr::IntLiteral(value)
            }
            Number::Float(f) => HirExpr::FloatLiteral(*f),
            other => {
                return Err(unsupported(
                    format!("numeric literal kind not supported yet: {other:?}"),
                    lit.range,
                ));
            }
        },
        // #602 (Part 1 of #573): a source-level negative number is not a
        // negative literal in the AST -- `-5` parses as `UnaryOp { op: USub,
        // operand: NumberLiteral(5) }`. `HirExpr::IntLiteral` is already an
        // `i64` and `FloatLiteral` an `f64`, and every downstream crate
        // already handles negative values in them, so folding the sign into
        // the literal here needs no new HIR variant and no change to
        // `pycc_types`, `pycc_mir`, or `pycc_codegen`. Only a *literal*
        // operand folds: `-x` for a variable needs a real `HirExpr` variant
        // and downstream arms, which the next arm supplies (#603, Part 2);
        // `not x` and `~x` (#604, Part 3) get no such fold either, since
        // neither is part of Python's numeric-literal grammar.
        Expr::UnaryOp(unary) => match (unary.op, unary.operand.as_ref()) {
            (UnaryOp::USub | UnaryOp::UAdd, Expr::NumberLiteral(lit)) => {
                let negate = matches!(unary.op, UnaryOp::USub);
                match &lit.value {
                    Number::Int(i) => HirExpr::IntLiteral(fold_int_literal_sign(
                        i,
                        negate,
                        pycc_ast::expr_range(expr),
                    )?),
                    Number::Float(f) => HirExpr::FloatLiteral(if negate { -*f } else { *f }),
                    other => {
                        return Err(unsupported(
                            format!("numeric literal kind not supported yet: {other:?}"),
                            lit.range,
                        ));
                    }
                }
            }
            // #603 (Part 2 of #573): every `-`/`+` whose operand is not a
            // numeric literal. The literal arm above still runs first, so
            // `-1` keeps folding into a signed `IntLiteral` with no node at
            // all; this arm covers only what folding cannot reach (`-x`,
            // `-f(y)`, `-(a + b)`).
            (UnaryOp::USub | UnaryOp::UAdd, operand) => HirExpr::UnaryOp {
                op: if matches!(unary.op, UnaryOp::USub) {
                    UnaryOpKind::USub
                } else {
                    UnaryOpKind::UAdd
                },
                operand: Box::new(lower_expr(operand, in_function, class_name)?),
            },
            // #604 (Part 3 of #573): `not x` and `~x`. Neither operator has
            // a literal-folding arm the way `USub`/`UAdd` do above --
            // `not 5` and `~5` are not part of Python's numeric-literal
            // grammar the way a source-level `-5` is, so every operand
            // (literal or not) lowers into the same `HirExpr::UnaryOp` node
            // and is typed/rewritten downstream.
            (UnaryOp::Not, operand) => HirExpr::UnaryOp {
                op: UnaryOpKind::Not,
                operand: Box::new(lower_expr(operand, in_function, class_name)?),
            },
            (UnaryOp::Invert, operand) => HirExpr::UnaryOp {
                op: UnaryOpKind::Invert,
                operand: Box::new(lower_expr(operand, in_function, class_name)?),
            },
        },
        Expr::Name(name) => HirExpr::Name(name.id.as_str().to_string()),
        Expr::List(list) => HirExpr::ListLiteral(
            list.elts
                .iter()
                .map(|e| {
                    let lowered = lower_expr(e, in_function, class_name)?;
                    check_boundary_literal(
                        &lowered,
                        pycc_ast::expr_range(e),
                        "list-literal element",
                    )?;
                    Ok(lowered)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Dict(dict) => HirExpr::DictLiteral(
            dict.items
                .iter()
                .map(|item| {
                    let Some(key) = &item.key else {
                        return Err(unsupported(
                            "dict-unpacking (`**expr`) inside a dict literal is not supported yet",
                            pycc_ast::expr_range(&item.value),
                        ));
                    };
                    let key = lower_expr(key, in_function, class_name)?;
                    let value = lower_expr(&item.value, in_function, class_name)?;
                    check_boundary_literal(
                        &value,
                        pycc_ast::expr_range(&item.value),
                        "dict-literal value",
                    )?;
                    Ok((key, value))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Set(set) => HirExpr::SetLiteral(
            set.elts
                .iter()
                .map(|e| {
                    let lowered = lower_expr(e, in_function, class_name)?;
                    check_boundary_literal(
                        &lowered,
                        pycc_ast::expr_range(e),
                        "set-literal element",
                    )?;
                    Ok(lowered)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Tuple(tuple) => HirExpr::TupleLiteral(
            tuple
                .elts
                .iter()
                .map(|e| lower_expr(e, in_function, class_name))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Subscript(sub) => match sub.slice.as_ref() {
            // A colon-containing subscript (`xs[a:b:c]`) parses its `slice`
            // field as `Expr::Slice`, distinct from the plain single-
            // expression `slice` an ordinary index (`xs[0]`) produces
            // (PR-12, D-118). Each bound is independently optional in real
            // Python's own grammar, so each is lowered through
            // `Option::map`/`.transpose()` rather than assumed present.
            Expr::Slice(slice) => {
                let lower_bound = |e: &Expr| -> Result<HirExpr, Diagnostic> {
                    let lowered = lower_expr(e, in_function, class_name)?;
                    check_boundary_literal(&lowered, pycc_ast::expr_range(e), "slice bound")?;
                    Ok(lowered)
                };
                HirExpr::Slice {
                    base: Box::new(lower_expr(&sub.value, in_function, class_name)?),
                    start: slice
                        .lower
                        .as_deref()
                        .map(lower_bound)
                        .transpose()?
                        .map(Box::new),
                    stop: slice
                        .upper
                        .as_deref()
                        .map(lower_bound)
                        .transpose()?
                        .map(Box::new),
                    step: slice
                        .step
                        .as_deref()
                        .map(lower_bound)
                        .transpose()?
                        .map(Box::new),
                }
            }
            _ => {
                let base = Box::new(lower_expr(&sub.value, in_function, class_name)?);
                let index = lower_expr(&sub.slice, in_function, class_name)?;
                // #618/D-207 (finding from PR #827 review): a tuple base has
                // no D-141 runtime `int`-boundary position at all -- tuple
                // indexing is resolved entirely at compile time in
                // `pycc_types::check_expr`'s own `Ty::Tuple` arm, which
                // already rejects an out-of-range literal index with T0040
                // ("non-negative literal within range"). Emitting T0051
                // unconditionally here, before the base's type is known,
                // would preempt that existing T0040 check and mislabel the
                // position as a "list index" for a tuple. HIR lowering can
                // only recognize a tuple base syntactically when it is
                // itself a tuple *literal* (`(1, 2)[huge]`); a tuple value
                // held in a variable is indistinguishable from a list at
                // this stage without type information `pycc_hir` does not
                // have, so that case is an accepted, documented gap
                // mirroring the `str * int` repeat-count narrowing
                // elsewhere in this module -- see `crate::int_boundary`'s
                // doc comment.
                if !matches!(sub.value.as_ref(), Expr::Tuple(_)) {
                    check_boundary_literal(&index, pycc_ast::expr_range(&sub.slice), "list index")?;
                }
                HirExpr::Subscript {
                    base,
                    index: Box::new(index),
                }
            }
        },
        Expr::Call(call) => {
            if !call.arguments.keywords.is_empty() {
                return Err(unsupported(
                    "keyword call arguments are not supported yet",
                    call.range,
                ));
            }
            if let Expr::Attribute(attr) = call.func.as_ref() {
                // #433: `super().method(args)` — recognize a zero-arg
                // `super()` call as the receiver of a method call, before
                // any container-method or stdlib fast path. Lower to
                // `HirExpr::MethodCall { base: Super, method, args }` so
                // the type checker and MIR lowering can resolve `method`
                // starting from the next class in the MRO (D-006 static
                // dispatch, per the #433 ADR). A `super()` outside a
                // method body (`class_name` is `None`) is rejected here
                // with C0001, matching CPython's own `RuntimeError: super()
                // no arguments` / `NameError: __class__` for the same shape.
                if is_zero_arg_super_call(&attr.value) {
                    if class_name.is_none() {
                        return Err(unsupported(
                            "`super()` outside a method body is not supported",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    }
                    let args = call
                        .arguments
                        .args
                        .iter()
                        .map(|e| lower_expr(e, in_function, class_name))
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(HirExpr::MethodCall {
                        base: Box::new(HirExpr::Super),
                        method: attr.attr.to_string(),
                        args,
                    });
                }
                if attr.attr.as_str() == "append" {
                    let Expr::Name(list_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.append()` is only supported on a bare-name list so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [value] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "list.append() takes exactly one argument, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    let value_span = pycc_ast::expr_range(value);
                    let value = lower_expr(value, in_function, class_name)?;
                    check_boundary_literal(&value, value_span, "`list.append()` value")?;
                    return Ok(HirExpr::ListAppend {
                        list: list_name.id.as_str().to_string(),
                        value: Box::new(value),
                    });
                }
                if attr.attr.as_str() == "pop" {
                    let Expr::Name(list_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.pop()` is only supported on a bare-name list so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "list.pop() takes no arguments, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    return Ok(HirExpr::ListPop {
                        list: list_name.id.as_str().to_string(),
                    });
                }
                if attr.attr.as_str() == "get" {
                    let Expr::Name(dict_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.get()` is only supported on a bare-name dict so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [key, default] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "dict.get() takes exactly two arguments (key, default), got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    let default_span = pycc_ast::expr_range(default);
                    let key = lower_expr(key, in_function, class_name)?;
                    let default = lower_expr(default, in_function, class_name)?;
                    check_boundary_literal(&default, default_span, "`dict.get()` default")?;
                    return Ok(HirExpr::DictGetOrDefault {
                        dict: dict_name.id.as_str().to_string(),
                        key: Box::new(key),
                        default: Box::new(default),
                    });
                }
                if attr.attr.as_str() == "add" {
                    let Expr::Name(set_name) = attr.value.as_ref() else {
                        return Err(unsupported(
                            "`.add()` is only supported on a bare-name set so far",
                            pycc_ast::expr_range(&attr.value),
                        ));
                    };
                    let [value] = &*call.arguments.args else {
                        return Err(unsupported(
                            format!(
                                "set.add() takes exactly one argument, got {}",
                                call.arguments.args.len()
                            ),
                            call.range,
                        ));
                    };
                    let value_span = pycc_ast::expr_range(value);
                    let value = lower_expr(value, in_function, class_name)?;
                    check_boundary_literal(&value, value_span, "`set.add()` value")?;
                    return Ok(HirExpr::SetAdd {
                        set: set_name.id.as_str().to_string(),
                        value: Box::new(value),
                    });
                }
                // `math.sqrt(x)`-shaped stdlib intrinsic call (D-136/D-137).
                // Resolved textually against `pycc_std`'s registry (receiver
                // name, then attribute name), the same precedent this file
                // already uses for `X: TypeAlias` (see
                // `lower_legacy_type_alias_ann_assign`'s doc comment): real
                // flow-sensitive "was `math` actually imported before this
                // use" verification is not attempted here, because
                // `lower_expr` has no access to the module-level import
                // side-table `module::lower_all` builds (threading it through
                // every recursive `lower_expr` call site is a materially
                // larger change than this thin v0.2 slice needs). `math` is
                // not a valid bare Python identifier binding to anything
                // else in this compiler's current name-resolution model
                // (no ordinary variable/import can produce a receiver whose
                // name doubles as a registered stdlib module and *isn't*
                // that module), so this narrowing does not accept any
                // program CPython itself would reject as a `NameError` in
                // practice for the fixtures this PR ships -- but it is a
                // real, deliberate scope trim from a fully import-gated
                // design, recorded here rather than silently.
                if let Expr::Name(receiver) = attr.value.as_ref()
                    && let Some(module) = pycc_std::resolve_module(receiver.id.as_str())
                {
                    // Unlike the generic `MethodCall` fallback below, a
                    // receiver that *is* a resolvable stdlib module keeps
                    // its existing "not registered" rejection even when the
                    // called symbol itself doesn't resolve -- falling
                    // through to `MethodCall` here would silently turn
                    // `math.tan(1.0)` (a real module, an unregistered
                    // symbol) into "call method `tan` on `math`", losing
                    // the far more precise stdlib diagnostic.
                    let Some(symbol) = pycc_std::resolve_symbol(module, attr.attr.as_str()) else {
                        return Err(unsupported(
                            format!(
                                "module `{}` has no importable symbol named `{}`",
                                receiver.id.as_str(),
                                attr.attr
                            ),
                            call.range,
                        ));
                    };
                    let args = call
                        .arguments
                        .args
                        .iter()
                        .map(|e| lower_expr(e, in_function, class_name))
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(HirExpr::Call {
                        callee: format!("{}.{}", receiver.id.as_str(), symbol.name),
                        args,
                    });
                }
                // `base.method(args)` (D-154, Part 1 of #375): the generic
                // instance-method-call fallback, tried only after every
                // hand-recognized container method and the stdlib-module
                // call above -- both must keep winning first, or e.g.
                // `xs.append(1)` would start lowering as a `MethodCall`
                // instead of the dedicated `ListAppend` node, silently
                // breaking every existing container-method conformance
                // fixture. `base` is lowered generically (mirroring
                // `Expr::Attribute`'s own instance-attribute-read fallback
                // below): this lowering step has no type information to
                // narrow it further, so `pycc_types` is the one that
                // rejects a method call on a non-instance-typed receiver or
                // an unknown method name.
                let args = call
                    .arguments
                    .args
                    .iter()
                    .map(|e| lower_expr(e, in_function, class_name))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(HirExpr::MethodCall {
                    base: Box::new(lower_expr(&attr.value, in_function, class_name)?),
                    method: attr.attr.to_string(),
                    args,
                });
            }
            // PEP 695 (#387): `C[int](args)` — a generic class instantiation.
            // The call's func is a `Subscript` with a bare-name base (the
            // class name) and a bare-name slice (the type argument). The
            // type argument is resolved to a `Ty` here, at HIR-lowering
            // time, using the same bare-name-to-`Ty` mapping
            // `annotation_to_ty` uses for scalar types (int/float/bool/str)
            // — no `aliases` context is needed since PEP 695 generic class
            // instantiation is scoped to scalar-only types (D-133/D-134).
            if let Expr::Subscript(sub) = call.func.as_ref() {
                let Expr::Name(gen_class_name) = sub.value.as_ref() else {
                    return Err(unsupported(
                        "calling a subscript expression is not supported yet \
                         (only a generic class instantiation `C[type](args)` is)",
                        pycc_ast::expr_range(&call.func),
                    ));
                };
                let type_arg = type_arg_name_to_ty(&sub.slice)?;
                let args = call
                    .arguments
                    .args
                    .iter()
                    .map(|e| lower_expr(e, in_function, class_name))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(HirExpr::GenericClassInstantiate {
                    class: gen_class_name.id.as_str().to_string(),
                    type_arg,
                    args,
                });
            }
            // #433: a bare `super()` not used as a method-call or
            // attribute-access base (e.g. `x = super()`) has no useful
            // static-dispatch lowering on its own — reject it here with
            // C0001 rather than letting `super` fall through to the
            // known-but-unsupported-builtin path, which would produce a
            // less precise diagnostic. `super().method()` and `super().attr`
            // are already handled above (in the `Expr::Attribute` arm of
            // this `Expr::Call` match), so a bare `super()` reaching here
            // is genuinely a standalone use.
            if is_zero_arg_super_call(expr) {
                return Err(unsupported(
                    "a bare `super()` expression is not supported — use `super().method()` or `super().attr`",
                    call.range,
                ));
            }
            let Expr::Name(callee) = call.func.as_ref() else {
                return Err(unsupported(
                    format!(
                        "only calling a bare name is supported so far: {:?}",
                        call.func
                    ),
                    pycc_ast::expr_range(&call.func),
                ));
            };
            let args = call
                .arguments
                .args
                .iter()
                .map(|e| lower_expr(e, in_function, class_name))
                .collect::<Result<Vec<_>, _>>()?;
            HirExpr::Call {
                callee: callee.id.as_str().to_string(),
                args,
            }
        }
        Expr::BinOp(bin_op) => {
            let op = match bin_op.op {
                Operator::Add => BinOpKind::Add,
                Operator::Sub => BinOpKind::Sub,
                Operator::Mult => BinOpKind::Mul,
                Operator::Div => BinOpKind::Div,
                Operator::FloorDiv => BinOpKind::FloorDiv,
                Operator::Mod => BinOpKind::Mod,
                Operator::Pow => BinOpKind::Pow,
                other => {
                    return Err(unsupported(
                        format!("binary operator not supported yet: {other:?}"),
                        bin_op.range,
                    ));
                }
            };
            let left = lower_expr(&bin_op.left, in_function, class_name)?;
            let right = lower_expr(&bin_op.right, in_function, class_name)?;
            // #618: `str` repeat count. Only the case where the *string*
            // side is itself a string literal is recognized here -- see
            // `crate::int_boundary`'s doc comment for why a `str`-typed
            // variable multiplied by an oversized literal is a documented,
            // narrower out-of-scope gap rather than a missed case.
            if op == BinOpKind::Mul {
                if matches!(bin_op.left.as_ref(), Expr::StringLiteral(_)) {
                    check_boundary_literal(
                        &right,
                        pycc_ast::expr_range(&bin_op.right),
                        "`str` repeat count",
                    )?;
                } else if matches!(bin_op.right.as_ref(), Expr::StringLiteral(_)) {
                    check_boundary_literal(
                        &left,
                        pycc_ast::expr_range(&bin_op.left),
                        "`str` repeat count",
                    )?;
                }
            }
            HirExpr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        Expr::BooleanLiteral(lit) => HirExpr::BoolLiteral(lit.value),
        Expr::StringLiteral(lit) => HirExpr::StringLiteral(lit.value.to_str().to_string()),
        Expr::NoneLiteral(_) => HirExpr::NoneLiteral,
        Expr::FString(fstring) => {
            let parts = fstring
                .value
                .elements()
                .map(|element| -> Result<FStringPart, Diagnostic> {
                    Ok(match element {
                        pycc_ast::InterpolatedStringElement::Literal(lit) => {
                            FStringPart::Literal(lit.value.to_string())
                        }
                        pycc_ast::InterpolatedStringElement::Interpolation(interp) => {
                            if interp.debug_text.is_some() {
                                // #720: the `=` debug specifier (`f"{n=}"`) renders the
                                // source text plus `repr(value)`, which this crate does
                                // not implement. Silently discarding `debug_text` would
                                // compile cleanly and print the wrong value, so reject
                                // it explicitly instead.
                                return Err(unsupported(
                                    "f-string debug specifier (=) is not supported yet",
                                    interp.range,
                                ));
                            }
                            if interp.conversion != pycc_ast::ConversionFlag::None {
                                return Err(unsupported(
                                    "f-string conversion flags (!r/!s/!a) are not supported yet",
                                    interp.range,
                                ));
                            }
                            if interp.format_spec.is_some() {
                                return Err(unsupported(
                                    "f-string format spec ({x:...}) is not supported yet",
                                    interp.range,
                                ));
                            }
                            FStringPart::Interpolation(Box::new(lower_expr(
                                &interp.expression,
                                in_function,
                                class_name,
                            )?))
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            HirExpr::FString(parts)
        }
        Expr::Compare(cmp) => {
            if cmp.ops.len() != 1 {
                return Err(unsupported(
                    format!("chained comparisons are not supported yet: {:?}", cmp.ops),
                    cmp.range,
                ));
            }
            // `is`/`is not` (D-197, #763, Part 1 of #747): this compiler's
            // first support of any kind for either operator, deliberately
            // scoped at this syntactic gate to exactly the case #763 needs
            // -- one operand is literally `Expr::NoneLiteral`. Every other
            // `is`/`is not` use (`x is y` for two arbitrary non-`None`
            // operands, or `x is None` where the check below finds neither
            // side is `NoneLiteral` -- unreachable today since `None` is
            // the only way to spell that side, kept as a real check rather
            // than an `assert!` so a future second `None`-shaped literal
            // does not silently widen acceptance) keeps falling through to
            // the pre-existing `other =>` rejection below unchanged. The
            // *type* of the non-`None` operand (must be `Ty::Optional(_)`
            // or `Ty::None`) is `pycc_types`' job, not this lowering step's
            // -- HIR only records the syntactic shape, matching every other
            // shape-vs-type division of labor in this module (D-105).
            let is_none_operand_shape = matches!(cmp.left.as_ref(), Expr::NoneLiteral(_))
                || matches!(cmp.comparators[0], Expr::NoneLiteral(_));
            let op = match cmp.ops[0] {
                CmpOp::Eq => CmpOpKind::Eq,
                CmpOp::NotEq => CmpOpKind::NotEq,
                CmpOp::Lt => CmpOpKind::Lt,
                CmpOp::LtE => CmpOpKind::LtE,
                CmpOp::Gt => CmpOpKind::Gt,
                CmpOp::GtE => CmpOpKind::GtE,
                CmpOp::Is if is_none_operand_shape => CmpOpKind::Is,
                CmpOp::IsNot if is_none_operand_shape => CmpOpKind::IsNot,
                other => {
                    return Err(unsupported(
                        format!("comparison operator not supported yet: {other:?}"),
                        cmp.range,
                    ));
                }
            };
            HirExpr::Compare {
                op,
                left: Box::new(lower_expr(&cmp.left, in_function, class_name)?),
                right: Box::new(lower_expr(&cmp.comparators[0], in_function, class_name)?),
            }
        }
        // `math.pi`-shaped bare stdlib constant reference (D-136/D-137),
        // e.g. `print(math.pi)`. A call-shaped `math.sqrt(x)` is handled
        // separately inside the `Expr::Call` arm above (it needs the call
        // arguments, which this bare-attribute position never has). Resolved
        // with the same textual, non-flow-sensitive precedent documented on
        // that arm. Encoded as `HirExpr::Name("math.pi")`: real Python
        // identifiers can never contain `.`, so this qualified spelling is
        // an unambiguous marker `pycc_types`' ordinary name lookup can
        // special-case without any risk of colliding with a real variable
        // named `pi`.
        Expr::Attribute(attr) => {
            if let Expr::Name(receiver) = attr.value.as_ref()
                && let Some(module) = pycc_std::resolve_module(receiver.id.as_str())
            {
                let Some(symbol) = pycc_std::resolve_symbol(module, attr.attr.as_str()) else {
                    return Err(unsupported(
                        format!(
                            "module `{}` has no attribute `{}`",
                            receiver.id.as_str(),
                            attr.attr
                        ),
                        pycc_ast::expr_range(expr),
                    ));
                };
                return Ok(HirExpr::Name(format!(
                    "{}.{}",
                    receiver.id.as_str(),
                    symbol.name
                )));
            }
            // #433: `super().attr` — recognize a zero-arg `super()` call as
            // the base of an attribute access, before the generic fallback
            // below. Lower to `HirExpr::AttrGet { base: Super, attr }` so
            // the type checker and MIR lowering can resolve `attr` starting
            // from the next class in the MRO. Same `class_name.is_none()`
            // rejection as the `super().method()` arm above.
            if is_zero_arg_super_call(&attr.value) {
                if class_name.is_none() {
                    return Err(unsupported(
                        "`super()` outside a method body is not supported",
                        pycc_ast::expr_range(&attr.value),
                    ));
                }
                return Ok(HirExpr::AttrGet {
                    base: Box::new(HirExpr::Super),
                    attr: attr.attr.to_string(),
                });
            }
            // `base.attr` (D-154, Part 1 of #375): the generic
            // instance-attribute-read fallback, tried only after the
            // stdlib-module case above -- a receiver that *is* a resolvable
            // module keeps its existing "no attribute named ..." rejection
            // unchanged rather than falling through here (that error names
            // the exact reason far more precisely than a generic
            // "not a declared attribute" `pycc_types` diagnostic could).
            // Every other receiver shape -- `self`, any other bare name,
            // or an arbitrary nested expression -- lowers `base` generically
            // and defers to `pycc_types` to reject a non-instance base or an
            // attribute name the base's class never declares.
            HirExpr::AttrGet {
                base: Box::new(lower_expr(&attr.value, in_function, class_name)?),
                attr: attr.attr.to_string(),
            }
        }
        // `yield`/`yield from` outside any function body is a CPython
        // `SyntaxError`, not "valid but unimplemented" (D-149, the
        // expression-lowering sequel to D-148's `break`/`continue`/`async
        // for` precedent) -- reused as `L0001`, matching `context_invalid`'s
        // existing convention. The match guard is deliberately `!in_function`
        // so the valid-context case (`in_function == true`, a real enclosing
        // function) falls through unchanged to the generic `other =>`
        // fallback below, preserving today's `C0001` "expression kind not
        // supported yet" classification there byte-for-byte (generator
        // codegen itself remains out of scope, D-149).
        Expr::Yield(y) if !in_function => {
            return Err(context_invalid("'yield' outside function", y.range));
        }
        Expr::YieldFrom(yf) if !in_function => {
            return Err(context_invalid("'yield from' outside function", yf.range));
        }
        // PEP 572 (#774): `target := value`. CPython's own grammar only ever
        // parses a bare identifier as a walrus target -- there is no
        // tuple/attribute/subscript walrus target to reject here in
        // practice, but the check is kept explicit (rather than an
        // unchecked `Expr::Name` pattern) so a future `ruff_python_parser`
        // upgrade that somehow relaxed the grammar would still surface a
        // clean diagnostic instead of an `unreachable!()`/panic.
        Expr::Named(named) => {
            let Expr::Name(target) = named.target.as_ref() else {
                return Err(unsupported(
                    "a walrus assignment target must be a bare name",
                    pycc_ast::expr_range(&named.target),
                ));
            };
            HirExpr::NamedExpr {
                name: target.id.as_str().to_string(),
                value: Box::new(lower_expr(&named.value, in_function, class_name)?),
            }
        }
        other => {
            return Err(unsupported(
                "expression kind not supported yet",
                pycc_ast::expr_range(other),
            ));
        }
    };
    Ok(lowered)
}

/// Rewrites every occurrence of the bare name `from` inside `expr` to `to`
/// (PR-12, D-117) -- used to give a comprehension's own loop variable a
/// synthesized, collision-proof internal name (see `synthesize_comp_var_name`
/// below) without inventing real lexical scoping. Exhaustive over `HirExpr`
/// on purpose: a future variant added to this enum must add its own arm here
/// too, the same "let the compiler enumerate every site" discipline this
/// project's own `Scalar::List` precedent (D-107) already established for
/// `pycc_codegen`. Safe to apply blindly (no risk of renaming an unrelated
/// same-named binding from some other nested scope) because v0.2's
/// comprehension grammar has no nested comprehensions, no lambda, and no
/// nested function defs inside a comprehension's own `elt`/`cond`/`key`/
/// `value` -- none of those are expressible here at all yet.
pub(crate) fn rename_name_in_expr(expr: HirExpr, from: &str, to: &str) -> HirExpr {
    let recurse = |e: HirExpr| rename_name_in_expr(e, from, to);
    match expr {
        HirExpr::Name(n) => HirExpr::Name(if n == from { to.to_string() } else { n }),
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral => expr,
        // `callee` (a bare `String`, never an `HirExpr::Name`) is
        // deliberately left untouched even if it equals `from`: this HIR
        // subset has no first-class functions, so `callee` always names a
        // module-level function definition, never a local variable this
        // rename could plausibly shadow -- unlike `args`, which are
        // recursed into normally.
        HirExpr::Call { callee, args } => HirExpr::Call {
            callee,
            args: args.into_iter().map(recurse).collect(),
        },
        HirExpr::UnaryOp { op, operand } => HirExpr::UnaryOp {
            op,
            operand: Box::new(recurse(*operand)),
        },
        HirExpr::BinOp { op, left, right } => HirExpr::BinOp {
            op,
            left: Box::new(recurse(*left)),
            right: Box::new(recurse(*right)),
        },
        HirExpr::Compare { op, left, right } => HirExpr::Compare {
            op,
            left: Box::new(recurse(*left)),
            right: Box::new(recurse(*right)),
        },
        HirExpr::FString(parts) => HirExpr::FString(
            parts
                .into_iter()
                .map(|part| match part {
                    FStringPart::Literal(s) => FStringPart::Literal(s),
                    FStringPart::Interpolation(e) => {
                        FStringPart::Interpolation(Box::new(recurse(*e)))
                    }
                })
                .collect(),
        ),
        HirExpr::ListLiteral(es) => HirExpr::ListLiteral(es.into_iter().map(recurse).collect()),
        HirExpr::Subscript { base, index } => HirExpr::Subscript {
            base: Box::new(recurse(*base)),
            index: Box::new(recurse(*index)),
        },
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => HirExpr::Slice {
            base: Box::new(recurse(*base)),
            start: start.map(|s| Box::new(recurse(*s))),
            stop: stop.map(|s| Box::new(recurse(*s))),
            step: step.map(|s| Box::new(recurse(*s))),
        },
        HirExpr::ListAppend { list, value } => HirExpr::ListAppend {
            list: if list == from { to.to_string() } else { list },
            value: Box::new(recurse(*value)),
        },
        HirExpr::DictLiteral(pairs) => HirExpr::DictLiteral(
            pairs
                .into_iter()
                .map(|(k, v)| (recurse(k), recurse(v)))
                .collect(),
        ),
        HirExpr::SetLiteral(es) => HirExpr::SetLiteral(es.into_iter().map(recurse).collect()),
        HirExpr::TupleLiteral(es) => HirExpr::TupleLiteral(es.into_iter().map(recurse).collect()),
        // `list`/`dict`/`set` base-name fields are plain `String`s, mirroring
        // `ListAppend`'s own arm exactly: renamed only when they equal
        // `from`, otherwise left untouched. This matters for a
        // comprehension's own `elt`/`cond` referencing e.g. `xs.pop()` where
        // `xs` is the loop variable being synthesized-renamed -- the common
        // case (some other, non-loop-variable base) must not be touched.
        HirExpr::ListPop { list } => HirExpr::ListPop {
            list: if list == from { to.to_string() } else { list },
        },
        HirExpr::DictGetOrDefault { dict, key, default } => HirExpr::DictGetOrDefault {
            dict: if dict == from { to.to_string() } else { dict },
            key: Box::new(recurse(*key)),
            default: Box::new(recurse(*default)),
        },
        HirExpr::SetAdd { set, value } => HirExpr::SetAdd {
            set: if set == from { to.to_string() } else { set },
            value: Box::new(recurse(*value)),
        },
        HirExpr::AttrGet { base, attr } => HirExpr::AttrGet {
            base: Box::new(recurse(*base)),
            attr,
        },
        HirExpr::MethodCall { base, method, args } => HirExpr::MethodCall {
            base: Box::new(recurse(*base)),
            method,
            args: args.into_iter().map(recurse).collect(),
        },
        HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args,
        } => HirExpr::GenericClassInstantiate {
            class,
            type_arg,
            args: args.into_iter().map(recurse).collect(),
        },
        // #433: `Super` carries no names to rename — it is a compile-time
        // marker, not a value with sub-expressions.
        HirExpr::Super => expr,
        // PEP 572 (#774): a walrus target is renamed exactly like a bound
        // `Name` would be (mirroring `HirExpr::Name`'s own arm above) if it
        // happens to collide with the comprehension loop variable being
        // synthesized-renamed; `value` is recursed into normally. In
        // practice a walrus embedded in a comprehension's `elt`/`cond` is
        // out of scope for #774 (comprehension-scope walrus semantics are
        // not implemented -- see that issue's scope-cut note) and is
        // rejected upstream before lowering ever reaches a real
        // comprehension body, but this arm still needs to exist so this
        // exhaustive match compiles, and it does the structurally correct
        // thing on its own terms regardless.
        HirExpr::NamedExpr { name, value } => HirExpr::NamedExpr {
            name: if name == from { to.to_string() } else { name },
            value: Box::new(recurse(*value)),
        },
    }
}

/// PEP 572 (#774): whether `expr` contains a `HirExpr::NamedExpr` anywhere
/// within it, at any nesting depth. `crate::stmt::lower_stmt` calls this on
/// every expression field of a statement kind other than `Stmt::If`'s/
/// `Stmt::While`'s own `test` and a bare `Stmt::Expr`'s own value -- the
/// three placements a walrus is permitted in (#774's own explicit
/// permitted-scope-cut) -- to reject a walrus lowered anywhere else with a
/// clean diagnostic instead of leaving `pycc_types`/`pycc_mir` to either
/// silently mishandle it or panic downstream on an unbound name. An
/// exhaustive match over every `HirExpr` variant, mirroring
/// `rename_name_in_expr`'s own exhaustive structure just above, so a future
/// variant is a compile error here rather than a silently-permitted new
/// placement.
pub(crate) fn contains_named_expr(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::NamedExpr { .. } => true,
        HirExpr::IntLiteral(_)
        | HirExpr::FloatLiteral(_)
        | HirExpr::BoolLiteral(_)
        | HirExpr::StringLiteral(_)
        | HirExpr::NoneLiteral
        | HirExpr::Name(_)
        | HirExpr::Super => false,
        HirExpr::Call { args, .. } => args.iter().any(contains_named_expr),
        HirExpr::BinOp { left, right, .. } | HirExpr::Compare { left, right, .. } => {
            contains_named_expr(left) || contains_named_expr(right)
        }
        HirExpr::UnaryOp { operand, .. } => contains_named_expr(operand),
        HirExpr::FString(parts) => parts.iter().any(|part| match part {
            FStringPart::Literal(_) => false,
            FStringPart::Interpolation(e) => contains_named_expr(e),
        }),
        HirExpr::ListLiteral(es) | HirExpr::SetLiteral(es) | HirExpr::TupleLiteral(es) => {
            es.iter().any(contains_named_expr)
        }
        HirExpr::Subscript { base, index } => {
            contains_named_expr(base) || contains_named_expr(index)
        }
        HirExpr::Slice {
            base,
            start,
            stop,
            step,
        } => {
            contains_named_expr(base)
                || [start, stop, step]
                    .into_iter()
                    .flatten()
                    .any(|b| contains_named_expr(b))
        }
        HirExpr::ListAppend { value, .. } | HirExpr::SetAdd { value, .. } => {
            contains_named_expr(value)
        }
        HirExpr::DictLiteral(pairs) => pairs
            .iter()
            .any(|(k, v)| contains_named_expr(k) || contains_named_expr(v)),
        HirExpr::ListPop { .. } => false,
        HirExpr::DictGetOrDefault { key, default, .. } => {
            contains_named_expr(key) || contains_named_expr(default)
        }
        HirExpr::AttrGet { base, .. } => contains_named_expr(base),
        HirExpr::MethodCall { base, args, .. } => {
            contains_named_expr(base) || args.iter().any(contains_named_expr)
        }
        HirExpr::GenericClassInstantiate { args, .. } => args.iter().any(contains_named_expr),
    }
}

/// Synthesizes a collision-proof internal name for a comprehension's loop
/// variable (D-117): a leading digit can never begin a valid Python
/// identifier (confirmed against the vendored `ruff_python_parser`'s own
/// tokenizer -- a `NAME` token cannot start with a decimal digit), so this
/// string can never be produced by lowering real Python source, no matter
/// what the user names their own variables -- no new lexical-scoping
/// machinery is needed; this is just another ordinary entry in the existing
/// flat, name-keyed slot model. Seeded by the loop target's own byte offset,
/// not a mutable counter: two distinct comprehensions in one file can never
/// share a target's start offset, so this needs no threaded lowering state
/// and stays fully deterministic across repeated compiles of the same
/// source.
///
/// Takes a plain `u32` byte offset (from `pycc_ast::expr_range`) rather than
/// naming `ruff_text_size::TextSize` directly -- `pycc_hir` depends only on
/// `pycc_ast`, never on `ruff_text_size` (Step 0's own re-export widening is
/// this crate's one and only upstream-crate seam), and `pycc_ast`'s own
/// `expr_range`/`stmt_range` exist specifically to keep that boundary from
/// leaking (see their doc comments).
fn synthesize_comp_var_name(target_start: u32, source_name: &str) -> String {
    format!("0comp_{target_start}_{source_name}")
}

/// Parses `range(...)`'s argument list into `(start, stop, step)` `HirExpr`s,
/// defaulting `start`/`step` per Python's own `range()` overloads. Shared by
/// `Stmt::For`'s own lowering and `lower_comprehension_iter` below (PR-12) --
/// factored out rather than duplicated a second time. Callers are
/// responsible for checking the callee is actually `range` and carries no
/// keyword arguments first (their own diagnostics differ in wording between
/// a plain `for` loop and a comprehension's `for` clause), so this helper
/// only ever inspects `call.arguments.args`.
pub(crate) fn lower_range_call(
    call: &pycc_ast::ExprCall,
    in_function: bool,
    class_name: Option<&str>,
) -> Result<(HirExpr, HirExpr, HirExpr), Diagnostic> {
    // Issue #618 (T0051) deliberately does NOT check a `range()` argument:
    // D-179 already removed `range` from D-141's runtime `int`-boundary
    // inventory. `range()` is fully bigint-capable (bounds, step, and a
    // mid-loop-promoting induction variable all work via
    // `pycc_rt_range_normalize_operand`/`pycc_rt_range_continue`), so an
    // out-of-range literal here is not a capability gap at all -- it is
    // ordinary, supported behavior, not a candidate for a boundary
    // diagnostic. See D-207 for why this position was wrongly included in
    // #618's own filed inventory (copied from D-178's pre-D-179 fourteen).
    let lower_arg =
        |e: &Expr| -> Result<HirExpr, Diagnostic> { lower_expr(e, in_function, class_name) };
    match &*call.arguments.args {
        [stop] => Ok((
            HirExpr::IntLiteral(0),
            lower_arg(stop)?,
            HirExpr::IntLiteral(1),
        )),
        [start, stop] => Ok((lower_arg(start)?, lower_arg(stop)?, HirExpr::IntLiteral(1))),
        [start, stop, step] => Ok((lower_arg(start)?, lower_arg(stop)?, lower_arg(step)?)),
        other => Err(unsupported(
            format!("range() with {} arguments is not supported", other.len()),
            call.range,
        )),
    }
}

/// Resolves a comprehension's `for var in <iter>` clause into a `CompIter`,
/// reusing `Stmt::For`'s own iterable-shape acceptance verbatim (D-117):
/// `range(...)` or a bare name (resolved to `Ty::List`/`Ty::Dict`/`Ty::Set`
/// downstream by `pycc_types`/`pycc_mir`, exactly like a plain `for` loop).
/// Any other shape is rejected with the existing generic `C0001` path,
/// mirroring `Stmt::For`'s own "only `for x in range(...)` or `for x in
/// <list>` is supported so far" message.
fn lower_comprehension_iter(
    iter_expr: &Expr,
    class_name: Option<&str>,
) -> Result<CompIter, Diagnostic> {
    if let Expr::Name(name) = iter_expr {
        return Ok(CompIter::Name(name.id.as_str().to_string()));
    }
    let Expr::Call(call) = iter_expr else {
        return Err(unsupported(
            format!(
                "only `range(...)` or a bare-name iterable is supported so far in a comprehension: {iter_expr:?}"
            ),
            pycc_ast::expr_range(iter_expr),
        ));
    };
    let Expr::Name(callee) = call.func.as_ref() else {
        return Err(unsupported(
            "only calling `range(...)` is supported so far in a comprehension",
            pycc_ast::expr_range(&call.func),
        ));
    };
    if callee.id.as_str() != "range" {
        return Err(unsupported(
            format!(
                "only iterating over `range(...)` is supported so far in a comprehension, got `{}`",
                callee.id
            ),
            call.range,
        ));
    }
    if !call.arguments.keywords.is_empty() {
        return Err(unsupported(
            "keyword arguments to range() are not supported yet",
            call.range,
        ));
    }
    // Literal `true`, not comprehension-internal, not the threaded ambient
    // value (D-149 correction 6): a comprehension's outermost iterable --
    // including a `range(...)` call and its arguments -- evaluates in the
    // *enclosing* scope per real CPython grammar, confirmed against the
    // oracle (`[x for x in range((yield 3))]` is `'yield' outside function`
    // at module scope, and valid inside a `def` -- the ordinary
    // scope-dependent rule, not the comprehension-internal one). The literal
    // `true` here is not the theoretically correct value; it reproduces
    // today's unconditional `C0001` behavior for this narrow sub-position
    // with zero regression risk, and getting the enclosing-scope split fully
    // right for it is deliberately deferred (see D-149 and its own "out of
    // scope" section).
    let (start, stop, step) = lower_range_call(call, true, class_name)?;
    Ok(CompIter::Range { start, stop, step })
}

/// Validates and lowers a comprehension's shared shape (D-117): exactly one
/// generator clause, no `async for`, a bare-name loop target, at most one
/// `if` filter. Returns the loop target's *source* name, its synthesized
/// internal replacement, the resolved `CompIter`, and the (not-yet-renamed)
/// lowered `if`-filter expression, if present -- renaming is the caller's
/// job (`lower_list_comp_assign`/`lower_set_comp_assign`/
/// `lower_dict_comp_assign` below), since `elt`/`key`/`value` also need the
/// identical rename and this helper has no visibility into which of those
/// the caller is building.
///
/// `iter` (the resolved `CompIter` returned above) is deliberately **never**
/// passed through `rename_name_in_expr` -- neither here nor by any caller --
/// unlike `cond`/`elt`/`key`/`value`, which all are. This is not an
/// oversight: it matches real CPython scoping. A comprehension's outermost
/// iterable expression evaluates in the *enclosing* scope, before the
/// comprehension's own scope exists at all -- `[i for i in range(i)]`'s
/// `range(i)` reads the *enclosing* `i`, not the comprehension's own loop
/// variable (confirmed directly against CPython). Renaming `iter`'s
/// occurrences of the source loop-variable name would therefore be actively
/// wrong, not merely redundant: it would make `range(i)` read the
/// comprehension's own (not-yet-bound) synthesized variable instead of
/// whatever `i` means in the enclosing scope. See
/// `a_comprehension_range_iterable_referencing_the_loop_variables_own_source_name_is_not_renamed`
/// and
/// `a_comprehension_bare_name_iterable_sharing_the_loop_variables_own_source_name_is_not_renamed`
/// below, which pin this behavior directly -- without them, a future change
/// that "fixed" this asymmetry by renaming `iter` too would silently break
/// correct scoping with every existing test still green.
pub(crate) fn lower_comprehension_header(
    generators: &[pycc_ast::Comprehension],
    class_name: Option<&str>,
) -> Result<(String, String, CompIter, Option<HirExpr>), Diagnostic> {
    // Named `generator`, not `gen` -- `gen` is a reserved keyword as of the
    // 2024 edition (this workspace's own edition, reserved for a future
    // generator-block feature), so the brief's own `gen` binding does not
    // compile here.
    let [generator] = generators else {
        return Err(unsupported(
            "a comprehension with more than one `for` clause is not supported yet",
            generators.first().map(|g| g.range).unwrap_or_default(),
        ));
    };
    if generator.is_async {
        return Err(unsupported(
            "async comprehensions are not supported yet",
            generator.range,
        ));
    }
    let Expr::Name(var) = &generator.target else {
        return Err(unsupported(
            "only a bare name comprehension target is supported so far",
            pycc_ast::expr_range(&generator.target),
        ));
    };
    let cond = match generator.ifs.as_slice() {
        [] => None,
        // Literal `true`, not the threaded ambient value: a comprehension's
        // `if`-filter is lexically inside the comprehension's own scope
        // (D-149 correction 5), so a `yield` there is governed by a third,
        // scope-independent CPython rule (`'yield' inside list
        // comprehension`, unconditionally invalid regardless of what
        // encloses the comprehension) that this issue deliberately does not
        // implement -- hardcoding `true` here preserves today's exact
        // `C0001`-in-both-scopes behavior byte-for-byte instead of emitting
        // the wrong classification.
        [single] => Some(lower_expr(single, true, class_name)?),
        _ => {
            return Err(unsupported(
                "a comprehension with more than one `if` filter is not supported yet",
                generator.range,
            ));
        }
    };
    let iter = lower_comprehension_iter(&generator.iter, class_name)?;
    let source_name = var.id.as_str().to_string();
    let synth_var =
        synthesize_comp_var_name(pycc_ast::expr_range(&generator.target).start, &source_name);
    Ok((source_name, synth_var, iter, cond))
}

pub(crate) fn lower_list_comp_assign(
    target: &str,
    comp: &pycc_ast::ExprListComp,
    class_name: Option<&str>,
) -> Result<HirStmt, Diagnostic> {
    let (source_name, synth_var, iter, cond) =
        lower_comprehension_header(&comp.generators, class_name)?;
    // Literal `true`: `elt` is lexically inside the comprehension's own
    // scope, same reasoning as `lower_comprehension_header`'s `cond` arm
    // above (D-149 correction 5) -- preserves today's `C0001` classification
    // for a comprehension-internal `yield`/`yield from` in both enclosing
    // scopes.
    let elt_hir = lower_expr(&comp.elt, true, class_name)?;
    check_boundary_literal(
        &elt_hir,
        pycc_ast::expr_range(&comp.elt),
        "listcomp element",
    )?;
    let elt = rename_name_in_expr(elt_hir, &source_name, &synth_var);
    let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
    Ok(HirStmt::ListCompAssign {
        target: target.to_string(),
        var: synth_var,
        iter,
        cond: cond.map(Box::new),
        elt: Box::new(elt),
    })
}

pub(crate) fn lower_set_comp_assign(
    target: &str,
    comp: &pycc_ast::ExprSetComp,
    class_name: Option<&str>,
) -> Result<HirStmt, Diagnostic> {
    let (source_name, synth_var, iter, cond) =
        lower_comprehension_header(&comp.generators, class_name)?;
    // Literal `true`: same reasoning as `lower_list_comp_assign`'s `elt`
    // above (D-149 correction 5).
    let elt_hir = lower_expr(&comp.elt, true, class_name)?;
    check_boundary_literal(&elt_hir, pycc_ast::expr_range(&comp.elt), "setcomp element")?;
    let elt = rename_name_in_expr(elt_hir, &source_name, &synth_var);
    let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
    Ok(HirStmt::SetCompAssign {
        target: target.to_string(),
        var: synth_var,
        iter,
        cond: cond.map(Box::new),
        elt: Box::new(elt),
    })
}

pub(crate) fn lower_dict_comp_assign(
    target: &str,
    comp: &pycc_ast::ExprDictComp,
    class_name: Option<&str>,
) -> Result<HirStmt, Diagnostic> {
    // Real Python's dict-comprehension grammar (`{k: v for ...}`) has no
    // `**`-unpacking form the way a plain `Expr::Dict` literal does -- but
    // unlike that literal case, the parser does *not* reject
    // `{**x for k in y}`-shaped source at parse time: confirmed directly
    // against the vendored `ruff_python_parser` (0.0.6), which parses it
    // successfully as `ExprDictComp { key: None, value: Name("x"), .. }`,
    // silently dropping the `**` token rather than erroring. The brief this
    // task followed assumed `key: None` was unreachable from real parsed
    // source and modeled it with an `unreachable!()`/`.expect()` internal
    // panic; that assumption is false, so this is a real (if unusual)
    // C0001 capability diagnostic, mirroring `Expr::Dict`'s own analogous
    // `**`-unpacking rejection, not an internal-error panic.
    let Some(key_expr) = comp.key.as_deref() else {
        return Err(unsupported(
            "dict-unpacking (`**expr`) inside a dict comprehension is not supported yet",
            pycc_ast::expr_range(&comp.value),
        ));
    };
    let (source_name, synth_var, iter, cond) =
        lower_comprehension_header(&comp.generators, class_name)?;
    // Literal `true` for both `key` and `value`: same reasoning as
    // `lower_list_comp_assign`'s `elt` above (D-149 correction 5) -- `key`
    // and `value` are both lexically inside the comprehension's own scope.
    let key = rename_name_in_expr(
        lower_expr(key_expr, true, class_name)?,
        &source_name,
        &synth_var,
    );
    let value_hir = lower_expr(&comp.value, true, class_name)?;
    check_boundary_literal(
        &value_hir,
        pycc_ast::expr_range(&comp.value),
        "dictcomp value",
    )?;
    let value = rename_name_in_expr(value_hir, &source_name, &synth_var);
    let cond = cond.map(|c| rename_name_in_expr(c, &source_name, &synth_var));
    Ok(HirStmt::DictCompAssign {
        target: target.to_string(),
        var: synth_var,
        iter,
        cond: cond.map(Box::new),
        key: Box::new(key),
        value: Box::new(value),
    })
}

#[cfg(test)]
mod tests;
