//! MIR exception operands, handlers, and HIR-to-MIR lowering (#382).

use super::{HirClassDef, MirExpr, MirStmt, lower_expr};
use pycc_hir::{HirExpr, Ty};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum MirExceptionValue {
    Constructed { type_tag: u8, message: MirExpr },
    Existing(MirExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirExceptHandler {
    pub exc_type_tag: Option<u8>,
    pub binding_name: Option<String>,
    pub binding_ty: Option<Ty>,
    pub body: Vec<MirStmt>,
}

pub(super) fn resolve_exception_tag(name: &str) -> Option<u8> {
    match name {
        "Exception" => Some(0),
        "ValueError" => Some(1),
        "TypeError" => Some(2),
        "KeyError" => Some(3),
        "IndexError" => Some(4),
        "ZeroDivisionError" => Some(5),
        "RuntimeError" => Some(6),
        _ => None,
    }
}

pub(super) fn lower_raise(
    exc: &Option<HirExpr>,
    cause: &Option<HirExpr>,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirStmt {
    let Some(exc) = exc else {
        return MirStmt::Reraise;
    };
    let exception = lower_exception_value(exc, scopes, classes, current_class);
    if let Some(cause) = cause {
        return MirStmt::RaiseFrom {
            exception,
            cause: lower_exception_value(cause, scopes, classes, current_class),
        };
    }
    MirStmt::Raise { exception }
}

pub(super) fn lower_exception_value(
    expr: &HirExpr,
    scopes: &mut [HashMap<String, Ty>],
    classes: &HashMap<String, HirClassDef>,
    current_class: Option<&str>,
) -> MirExceptionValue {
    if let HirExpr::Call { callee, args } = expr
        && let Some(type_tag) = resolve_exception_tag(callee)
    {
        let message = args.first().map_or_else(
            || MirExpr::StringLiteral("unknown".to_string()),
            |message| lower_expr(message, scopes, classes, current_class),
        );
        return MirExceptionValue::Constructed { type_tag, message };
    }
    MirExceptionValue::Existing(lower_expr(expr, scopes, classes, current_class))
}
