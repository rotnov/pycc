//! Issue #1095: in a module that can hold a CPython object -- an `ext`
//! module (D-258), or one that has bound a foreign import (D-244 rule 3) --
//! a call to `append`, `pop`, `get` or `add` keeps both readings in a
//! `HirExpr::ReceiverDispatchedCall`, so an object receiver can take the
//! foreign method-call reading. Every other module lowers these calls
//! exactly as before.

use super::*;
use crate::{ContainerFallback, HirExpr, HirItem, HirStmt, Ty, receiver_takes_method_path};

/// Lowers `source` as an `ext` module (`ext == true`) or a `native` one,
/// with every import request answered `Foreign`.
fn lower(source: &str, ext: bool) -> LoweredModule {
    let parsed = parse(source);
    let mut resolved = ResolvedImports::default();
    resolved.set_ext_module(ext);
    for request in project_import_requests(&parsed) {
        resolved.insert(request.span, ResolvedImport::Foreign);
    }
    lower_module(&parsed, &resolved, None)
        .unwrap_or_else(|diagnostics| panic!("{source:?} must lower: {diagnostics:#?}"))
}

/// Every top-level expression statement of `module`, in source order.
fn top_level_exprs(module: &LoweredModule) -> Vec<&HirExpr> {
    module
        .hir
        .items
        .iter()
        .filter_map(|item| match item {
            HirItem::TopLevelStmt(HirStmt::ExprStmt(expr)) => Some(expr),
            _ => None,
        })
        .collect()
}

/// The expression statements of the module-level function `name`'s body.
fn function_exprs<'m>(module: &'m LoweredModule, name: &str) -> Vec<&'m HirExpr> {
    let body = module
        .hir
        .items
        .iter()
        .find_map(|item| match item {
            HirItem::Function { name: n, body, .. } if n == name => Some(body),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no function `{name}`"));
    body.iter()
        .filter_map(|stmt| match stmt {
            HirStmt::ExprStmt(expr) => Some(expr),
            _ => None,
        })
        .collect()
}

/// The container fallback of a receiver-dispatched call, after checking its
/// method reading names `method`.
fn dispatched(expr: &HirExpr, method: &str) -> ContainerFallback {
    let HirExpr::ReceiverDispatchedCall { call, container } = expr else {
        panic!("expected a receiver-dispatched call, got {expr:?}");
    };
    assert_eq!(call.method_receiver().map(|(_, m)| m), Some(method));
    container.clone()
}

#[test]
fn an_object_receiver_takes_the_method_reading() {
    assert!(receiver_takes_method_path(&Ty::Object));
    for native in [
        Ty::List(Box::new(Ty::Int)),
        Ty::Dict(Box::new((Ty::Str, Ty::Int))),
        Ty::Set(Box::new(Ty::Int)),
        Ty::Int,
    ] {
        assert!(!receiver_takes_method_path(&native), "{native:?}");
    }
}

#[test]
fn an_ext_module_dispatches_all_four_names_on_their_receiver() {
    let module = lower(
        "from typing import Any\n\n\ndef f(o: Any) -> None:\n    o.append(1)\n    o.pop()\n    \
         o.get(1, 2)\n    o.add(1)\n    o.get(1)\n    o.pop(0)\n    o.extend(o)\n",
        true,
    );
    let exprs = function_exprs(&module, "f");
    for (expr, method) in exprs.iter().zip(["append", "pop", "get", "add"]) {
        assert_eq!(dispatched(expr, method), ContainerFallback::Admitted);
    }
    // The arities the container fast path refuses keep that refusal beside
    // the method reading, which an object receiver takes instead.
    for (expr, method) in exprs[4..6].iter().zip(["get", "pop"]) {
        assert!(matches!(
            dispatched(expr, method),
            ContainerFallback::Refused(_)
        ));
    }
    // A name the container fast paths never claim is an ordinary call.
    assert!(matches!(exprs[6], HirExpr::MethodCall { method, .. } if method == "extend"));
}

#[test]
fn a_foreign_import_turns_the_gate_on_from_that_statement() {
    let module = lower(
        "xs = [1]\nxs.append(2)\nimport gc\ngc.get(1)\nxs.append(3)\ngc.pop()\n",
        false,
    );
    let exprs = top_level_exprs(&module);
    // Before the import, the call lowers to the container node as always.
    assert!(matches!(exprs[0], HirExpr::ListAppend { .. }), "{exprs:?}");
    assert!(matches!(
        dispatched(exprs[1], "get"),
        ContainerFallback::Refused(_)
    ));
    assert_eq!(dispatched(exprs[2], "append"), ContainerFallback::Admitted);
    assert_eq!(dispatched(exprs[3], "pop"), ContainerFallback::Admitted);
}

#[test]
fn a_native_module_without_a_foreign_import_is_unchanged() {
    let module = lower(
        "xs = [1]\nxs.append(2)\nxs.pop()\nd = {'a': 1}\nd.get('a', 2)\ns = {1}\ns.add(2)\n",
        false,
    );
    let exprs = top_level_exprs(&module);
    assert!(matches!(exprs[0], HirExpr::ListAppend { .. }), "{exprs:?}");
    assert!(matches!(exprs[1], HirExpr::ListPop { .. }), "{exprs:?}");
    assert!(
        matches!(exprs[2], HirExpr::DictGetOrDefault { .. }),
        "{exprs:?}"
    );
    assert!(matches!(exprs[3], HirExpr::SetAdd { .. }), "{exprs:?}");
}
