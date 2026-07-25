use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt};

#[derive(Debug, Clone, PartialEq)]
pub enum MirExpr {
    IntLiteral(i64),
}

#[derive(Debug, PartialEq)]
pub enum MirInstr {
    CallPrint { arg: MirExpr },
    CallUserFunction { name: String },
}

#[derive(Debug, PartialEq)]
pub enum MirItem {
    Function { name: String, body: Vec<MirInstr> },
    TopLevelStmt(MirInstr),
}

pub struct MirModule {
    pub items: Vec<MirItem>,
}

pub fn build(hir: &HirModule) -> MirModule {
    let items = hir
        .items
        .iter()
        .map(|item| match item {
            HirItem::Function { name, body } => MirItem::Function {
                name: name.clone(),
                body: body.iter().map(lower_instr).collect(),
            },
            HirItem::TopLevelStmt(stmt) => MirItem::TopLevelStmt(lower_instr(stmt)),
        })
        .collect();
    MirModule { items }
}

/// Only the two constructs `pycc_codegen` can already emit LLVM IR for are
/// lowered here -- everything HIR's wider grammar (PR-4's frontend depth)
/// can now represent but this crate can't yet compile panics with a message
/// naming PR-5 explicitly (D-034): a deliberate, temporary boundary, not
/// new codegen work landing in this PR.
fn lower_instr(stmt: &HirStmt) -> MirInstr {
    let HirStmt::ExprStmt(expr) = stmt else {
        panic!("pycc_mir: this statement kind's codegen lands in PR-5: {stmt:?}");
    };
    match expr {
        HirExpr::Call { callee, args } if callee == "print" => match args.as_slice() {
            [HirExpr::IntLiteral(n)] => MirInstr::CallPrint { arg: MirExpr::IntLiteral(*n) },
            _ => panic!("pycc_mir: print() with this argument shape lands in PR-5"),
        },
        HirExpr::Call { callee, args } if args.is_empty() => {
            MirInstr::CallUserFunction { name: callee.clone() }
        }
        _ => panic!("pycc_mir: this construct's codegen lands in PR-5"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_hir::{HirExpr, HirItem, HirModule, HirStmt};

    fn call_print(arg: i64) -> HirStmt {
        HirStmt::ExprStmt(HirExpr::Call { callee: "print".to_string(), args: vec![HirExpr::IntLiteral(arg)] })
    }

    fn call_user_fn(name: &str) -> HirStmt {
        HirStmt::ExprStmt(HirExpr::Call { callee: name.to_string(), args: vec![] })
    }

    #[test]
    fn builds_one_call_print_instr_per_top_level_hir_stmt() {
        let hir = HirModule { items: vec![HirItem::TopLevelStmt(call_print(42))] };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: MirExpr::IntLiteral(42) })]
        );
    }

    #[test]
    fn builds_a_call_user_function_instr() {
        let hir = HirModule { items: vec![HirItem::TopLevelStmt(call_user_fn("main"))] };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::TopLevelStmt(MirInstr::CallUserFunction { name: "main".to_string() })]
        );
    }

    #[test]
    fn builds_a_function_item_with_its_body_lowered() {
        let hir = HirModule {
            items: vec![HirItem::Function { name: "main".to_string(), body: vec![call_print(7)] }],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::Function {
                name: "main".to_string(),
                body: vec![MirInstr::CallPrint { arg: MirExpr::IntLiteral(7) }],
            }]
        );
    }

    #[test]
    #[should_panic(expected = "print() with this argument shape lands in PR-5")]
    fn print_with_zero_arguments_is_not_yet_lowerable() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::Call {
                callee: "print".to_string(),
                args: vec![],
            }))],
        };
        build(&hir);
    }

    #[test]
    #[should_panic(expected = "this construct's codegen lands in PR-5")]
    fn a_bare_literal_statement_is_not_yet_lowerable() {
        let hir = HirModule { items: vec![HirItem::TopLevelStmt(HirStmt::ExprStmt(HirExpr::IntLiteral(1)))] };
        build(&hir);
    }

    #[test]
    #[should_panic(expected = "this statement kind's codegen lands in PR-5")]
    fn an_assignment_statement_is_not_yet_lowerable() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::Assign {
                target: "x".to_string(),
                value: HirExpr::IntLiteral(1),
            })],
        };
        build(&hir);
    }
}
