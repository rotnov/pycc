pub use ruff_python_ast::{
    Arguments, Expr, ExprCall, ExprName, ExprNumberLiteral, Identifier, ModModule, Number,
    Parameters, Stmt, StmtExpr, StmtFunctionDef, StmtReturn,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_exports_are_the_same_type_as_upstream() {
        fn assert_same_type<T>(_: T) {}
        let m = ModModule {
            node_index: Default::default(),
            range: Default::default(),
            body: Default::default(),
        };
        assert_same_type::<ruff_python_ast::ModModule>(m);
    }
}
