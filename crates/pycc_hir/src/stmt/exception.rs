//! Lowering for `except` handler clauses.

use super::lower_body;
use crate::class::ClassAnnotationInfo;
use crate::{HirExceptHandler, Ty, unsupported};
use pycc_ast::{ExceptHandler, Expr};
use pycc_diag::Diagnostic;

pub(super) fn lower_except_handler(
    handler: &ExceptHandler,
    aliases: &[(String, Ty)],
    in_loop: bool,
    in_function: bool,
    class_name: Option<&str>,
    type_param: Option<&str>,
    class_defs: &[ClassAnnotationInfo],
) -> Result<HirExceptHandler, Diagnostic> {
    let pycc_ast::ExceptHandler::ExceptHandler(handler) = handler;
    let exc_type = handler
        .type_
        .as_deref()
        .map(|exception_type| {
            let Expr::Name(name) = exception_type else {
                return Err(unsupported(
                    "only a bare-name exception type is supported so far in except handlers",
                    pycc_ast::expr_range(exception_type),
                ));
            };
            Ok(name.id.as_str().to_string())
        })
        .transpose()?;
    let name = handler
        .name
        .as_ref()
        .map(|name| name.id.as_str().to_string());
    let body = lower_body(
        &handler.body,
        aliases,
        in_loop,
        in_function,
        class_name,
        type_param,
        class_defs,
    )?;
    Ok(HirExceptHandler {
        exc_type,
        name,
        body,
    })
}

#[cfg(test)]
mod tests {
    use crate::{HirExpr, HirStmt};

    // -- Exception handling HIR tests (#382, PR-22 Part 1) --

    /// Test helper: extract a `Try` statement from a top-level HIR item,
    /// panicking if the item is not a `Try`.  The panic arm is covered by
    /// `expect_top_level_try_panics_on_non_try`.
    fn expect_top_level_try(
        item: &crate::HirItem,
    ) -> (
        &[HirStmt],
        &[crate::HirExceptHandler],
        &[HirStmt],
        &[HirStmt],
    ) {
        match item {
            crate::HirItem::TopLevelStmt(HirStmt::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }) => (body, handlers, orelse, finalbody),
            _ => panic!("expected Try"),
        }
    }

    /// Test helper: extract a `Raise` statement from a top-level HIR item,
    /// panicking if the item is not a `Raise`.  The panic arm is covered by
    /// `expect_top_level_raise_panics_on_non_raise`.
    fn expect_top_level_raise(item: &crate::HirItem) -> (&Option<HirExpr>, &Option<HirExpr>) {
        match item {
            crate::HirItem::TopLevelStmt(HirStmt::Raise { exc, cause }) => (exc, cause),
            _ => panic!("expected Raise"),
        }
    }

    /// Test helper: extract a `Raise` statement from a `HirStmt`,
    /// panicking if the statement is not a `Raise`.  The panic arm is
    /// covered by `expect_raise_stmt_panics_on_non_raise`.
    fn expect_raise_stmt(stmt: &HirStmt) -> (&Option<HirExpr>, &Option<HirExpr>) {
        match stmt {
            HirStmt::Raise { exc, cause } => (exc, cause),
            _ => panic!("expected Raise"),
        }
    }

    #[test]
    #[should_panic(expected = "expected Try")]
    fn expect_top_level_try_panics_on_non_try() {
        expect_top_level_try(&crate::HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::IntLiteral(0),
        )));
    }

    #[test]
    #[should_panic(expected = "expected Raise")]
    fn expect_top_level_raise_panics_on_non_raise() {
        expect_top_level_raise(&crate::HirItem::TopLevelStmt(HirStmt::ExprStmt(
            HirExpr::IntLiteral(0),
        )));
    }

    #[test]
    #[should_panic(expected = "expected Raise")]
    fn expect_raise_stmt_panics_on_non_raise() {
        expect_raise_stmt(&HirStmt::ExprStmt(HirExpr::IntLiteral(0)));
    }

    #[test]
    fn lower_try_except_lowers_successfully() {
        let module = pycc_parser::parse("try:\n    x = 1\nexcept ValueError:\n    y = 2\n")
            .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let items = &hir.items;
        assert_eq!(items.len(), 1);
        let (body, handlers, orelse, finalbody) = expect_top_level_try(&items[0]);
        assert_eq!(body.len(), 1);
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].exc_type.as_deref(), Some("ValueError"));
        assert!(handlers[0].name.is_none());
        assert_eq!(handlers[0].body.len(), 1);
        assert!(orelse.is_empty());
        assert!(finalbody.is_empty());
    }

    #[test]
    fn lower_try_except_as_binding_lowers_successfully() {
        let module = pycc_parser::parse("try:\n    x = 1\nexcept ValueError as e:\n    y = 2\n")
            .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (_, handlers, _, _) = expect_top_level_try(&hir.items[0]);
        assert_eq!(handlers[0].exc_type.as_deref(), Some("ValueError"));
        assert_eq!(handlers[0].name.as_deref(), Some("e"));
    }

    #[test]
    fn lower_try_bare_except_lowers_successfully() {
        let module = pycc_parser::parse("try:\n    x = 1\nexcept:\n    y = 2\n")
            .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (_, handlers, _, _) = expect_top_level_try(&hir.items[0]);
        assert!(handlers[0].exc_type.is_none());
        assert!(handlers[0].name.is_none());
    }

    #[test]
    fn lower_try_multiple_handlers_lowers_successfully() {
        let module = pycc_parser::parse(
            "try:\n    x = 1\nexcept ValueError:\n    y = 2\nexcept KeyError:\n    z = 3\n",
        )
        .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (_, handlers, _, _) = expect_top_level_try(&hir.items[0]);
        assert_eq!(handlers.len(), 2);
        assert_eq!(handlers[0].exc_type.as_deref(), Some("ValueError"));
        assert_eq!(handlers[1].exc_type.as_deref(), Some("KeyError"));
    }

    #[test]
    fn lower_try_else_finally_lowers_successfully() {
        let module = pycc_parser::parse(
            "try:\n    x = 1\nexcept ValueError:\n    y = 2\nelse:\n    z = 3\nfinally:\n    w = 4\n",
        ).expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (_, _, orelse, finalbody) = expect_top_level_try(&hir.items[0]);
        assert_eq!(orelse.len(), 1);
        assert_eq!(finalbody.len(), 1);
    }

    #[test]
    fn lower_raise_lowers_successfully() {
        let module =
            pycc_parser::parse("raise ValueError(\"bad\")\n").expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (exc, cause) = expect_top_level_raise(&hir.items[0]);
        assert!(exc.is_some());
        assert!(cause.is_none());
    }

    #[test]
    fn lower_raise_from_lowers_successfully() {
        let module = pycc_parser::parse("raise ValueError(\"bad\") from RuntimeError(\"cause\")\n")
            .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (exc, cause) = expect_top_level_raise(&hir.items[0]);
        assert!(exc.is_some());
        assert!(cause.is_some());
    }

    #[test]
    fn lower_raise_from_none_drops_the_cause() {
        // PEP 409: `from None` suppresses implicit context chaining, whose
        // only observable effect is traceback rendering. pycc emits no
        // traceback, so the suppression marker lowers to "no cause" rather
        // than to a `None`-valued cause expression.
        let module = pycc_parser::parse("raise ValueError(\"bad\") from None\n")
            .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (exc, cause) = expect_top_level_raise(&hir.items[0]);
        assert!(exc.is_some());
        assert!(cause.is_none());
    }

    #[test]
    fn lower_bare_raise_lowers_successfully() {
        let module = pycc_parser::parse("try:\n    x = 1\nexcept ValueError:\n    raise\n")
            .expect("test fixture must parse");
        let hir = crate::lower_checked(&module).expect("lowering must succeed");
        let (_, handlers, _, _) = expect_top_level_try(&hir.items[0]);
        let (exc, cause) = expect_raise_stmt(&handlers[0].body[0]);
        assert!(exc.is_none());
        assert!(cause.is_none());
    }

    #[test]
    fn lower_try_star_rejects_with_c0001() {
        let module = pycc_parser::parse("try:\n    x = 1\nexcept* ValueError:\n    y = 2\n")
            .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("except*"));
    }

    #[test]
    fn lower_except_with_tuple_type_rejects_with_c0001() {
        // `except (ValueError, TypeError):` — a tuple exception type is
        // not a bare name, so it should be rejected with C0001.
        let module =
            pycc_parser::parse("try:\n    x = 1\nexcept (ValueError, TypeError):\n    y = 2\n")
                .expect("test fixture must parse");
        let err = crate::lower_checked(&module).unwrap_err();
        assert_eq!(err.code, "C0001");
        assert!(err.message.contains("bare-name exception type"));
    }

    #[test]
    fn lower_try_with_unsupported_body_expr_rejects() {
        // An unsupported expression in the try body should propagate
        // the lowering error through the `?` operator.
        let module =
            pycc_parser::parse("try:\n    x = lambda y: y\nexcept ValueError:\n    pass\n")
                .expect("test fixture must parse");
        let result = crate::lower_checked(&module);
        assert!(
            result.is_err(),
            "lowering should fail for unsupported expression in try body"
        );
    }

    #[test]
    fn lower_try_with_unsupported_handler_body_rejects() {
        // An unsupported expression in the handler body should propagate
        // the lowering error through the `?` operator.
        let module =
            pycc_parser::parse("try:\n    x = 1\nexcept ValueError:\n    y = lambda z: z\n")
                .expect("test fixture must parse");
        let result = crate::lower_checked(&module);
        assert!(
            result.is_err(),
            "lowering should fail for unsupported expression in handler body"
        );
    }

    #[test]
    fn lower_try_with_unsupported_else_body_rejects() {
        // An unsupported expression in the else body should propagate
        // the lowering error through the `?` operator.
        let module = pycc_parser::parse(
            "try:\n    x = 1\nexcept ValueError:\n    pass\nelse:\n    y = lambda z: z\n",
        )
        .expect("test fixture must parse");
        let result = crate::lower_checked(&module);
        assert!(
            result.is_err(),
            "lowering should fail for unsupported expression in else body"
        );
    }

    #[test]
    fn lower_try_with_unsupported_finally_body_rejects() {
        // An unsupported expression in the finally body should propagate
        // the lowering error through the `?` operator.
        let module = pycc_parser::parse(
            "try:\n    x = 1\nexcept ValueError:\n    pass\nfinally:\n    y = lambda z: z\n",
        )
        .expect("test fixture must parse");
        let result = crate::lower_checked(&module);
        assert!(
            result.is_err(),
            "lowering should fail for unsupported expression in finally body"
        );
    }

    #[test]
    fn lower_raise_with_unsupported_exc_rejects() {
        // An unsupported expression as the raise exc should propagate
        // the lowering error through the `?` operator (line 459).
        let module = pycc_parser::parse("raise lambda: None\n").expect("test fixture must parse");
        let result = crate::lower_checked(&module);
        assert!(
            result.is_err(),
            "lowering should fail for unsupported expression in raise exc"
        );
    }

    #[test]
    fn lower_raise_from_with_unsupported_cause_rejects() {
        // An unsupported expression as the raise cause should propagate
        // the lowering error through the `?` operator (line 464).
        let module = pycc_parser::parse("raise ValueError(\"bad\") from lambda: None\n")
            .expect("test fixture must parse");
        let result = crate::lower_checked(&module);
        assert!(
            result.is_err(),
            "lowering should fail for unsupported expression in raise cause"
        );
    }
}
