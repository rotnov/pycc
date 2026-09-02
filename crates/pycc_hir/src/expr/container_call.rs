//! The container-method call fast paths (`.append()`/`.pop()`/`.get()`/
//! `.add()`) of `lower_expr`'s `Expr::Call`-over-`Expr::Attribute` branch,
//! extracted from `expr.rs` per AGENTS.md's file-decomposition rule (issue
//! #890; tracking issue #552). Each body is the original `if` block's,
//! unchanged; only the dispatch on the attribute name moved into
//! `lower_container_method_call`.
//!
//! These run with no type information available (HIR lowering precedes
//! `pycc_types`): they recognize the *syntactic* shape `name.method(...)`
//! and cannot tell a real `list`/`dict`/`set` receiver from a class
//! instance whose own method shares one of the four names -- which is why
//! `class.rs`'s `CONTAINER_METHOD_NAMES` rejects such a method at
//! class-definition time.

use super::lower_expr;
use crate::int_boundary::check_boundary_literal;
use crate::{HirExpr, unsupported};
use pycc_ast::Expr;
use pycc_diag::Diagnostic;

/// Lowers `call` when `attr` names one of the four hand-recognized
/// container methods; `None` means "not a container method", and the
/// caller falls through to the stdlib-intrinsic / instance-method paths
/// unchanged.
pub(super) fn lower_container_method_call(
    call: &pycc_ast::ExprCall,
    attr: &pycc_ast::ExprAttribute,
    in_function: bool,
    class_name: Option<&str>,
) -> Option<Result<HirExpr, Diagnostic>> {
    match attr.attr.as_str() {
        "append" => Some(lower_list_append(call, attr, in_function, class_name)),
        "pop" => Some(lower_list_pop(call, attr)),
        "get" => Some(lower_dict_get(call, attr, in_function, class_name)),
        "add" => Some(lower_set_add(call, attr, in_function, class_name)),
        _ => None,
    }
}

fn lower_list_append(
    call: &pycc_ast::ExprCall,
    attr: &pycc_ast::ExprAttribute,
    in_function: bool,
    class_name: Option<&str>,
) -> Result<HirExpr, Diagnostic> {
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
    Ok(HirExpr::ListAppend {
        list: list_name.id.as_str().to_string(),
        value: Box::new(value),
    })
}

fn lower_list_pop(
    call: &pycc_ast::ExprCall,
    attr: &pycc_ast::ExprAttribute,
) -> Result<HirExpr, Diagnostic> {
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
    Ok(HirExpr::ListPop {
        list: list_name.id.as_str().to_string(),
    })
}

fn lower_dict_get(
    call: &pycc_ast::ExprCall,
    attr: &pycc_ast::ExprAttribute,
    in_function: bool,
    class_name: Option<&str>,
) -> Result<HirExpr, Diagnostic> {
    let Expr::Name(dict_name) = attr.value.as_ref() else {
        return Err(unsupported(
            "`.get()` is only supported on a bare-name dict so far",
            pycc_ast::expr_range(&attr.value),
        ));
    };
    let [key, default] = &*call.arguments.args else {
        // Issue #890: this fast path cannot see the receiver's type, so
        // the message must not assert one. The receiver may be a real
        // dict (`x = {"a": 1}; x.get("a")`) or something else entirely
        // (`v.get()` on an `int`, a `ContextVar`); a non-dict receiver's
        // own `.get()` is unsupported and is reported by `pycc_types`'s
        // `T0033` only for the two-argument shape, because every other
        // arity is rejected here first. The wording is receiver-neutral
        // on purpose.
        return Err(unsupported(
            format!(
                "`.get()` is only supported as `dict.get(key, default)` with exactly two arguments so far, got {}",
                call.arguments.args.len()
            ),
            call.range,
        ));
    };
    let default_span = pycc_ast::expr_range(default);
    let key = lower_expr(key, in_function, class_name)?;
    let default = lower_expr(default, in_function, class_name)?;
    check_boundary_literal(&default, default_span, "`dict.get()` default")?;
    Ok(HirExpr::DictGetOrDefault {
        dict: dict_name.id.as_str().to_string(),
        key: Box::new(key),
        default: Box::new(default),
    })
}

fn lower_set_add(
    call: &pycc_ast::ExprCall,
    attr: &pycc_ast::ExprAttribute,
    in_function: bool,
    class_name: Option<&str>,
) -> Result<HirExpr, Diagnostic> {
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
    Ok(HirExpr::SetAdd {
        set: set_name.id.as_str().to_string(),
        value: Box::new(value),
    })
}
