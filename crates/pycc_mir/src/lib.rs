use pycc_hir::{HirItem, HirModule, HirStmt};

#[derive(Debug, PartialEq)]
pub enum MirInstr {
    CallPrint { arg: i64 },
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

fn lower_instr(stmt: &HirStmt) -> MirInstr {
    let HirStmt::CallPrint { arg } = stmt;
    MirInstr::CallPrint { arg: *arg }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycc_hir::{HirItem, HirModule, HirStmt};

    #[test]
    fn builds_one_call_print_instr_per_top_level_hir_stmt() {
        let hir = HirModule {
            items: vec![HirItem::TopLevelStmt(HirStmt::CallPrint { arg: 42 })],
        };
        let mir = build(&hir);
        assert_eq!(mir.items, vec![MirItem::TopLevelStmt(MirInstr::CallPrint { arg: 42 })]);
    }

    #[test]
    fn builds_a_function_item_with_its_body_lowered() {
        let hir = HirModule {
            items: vec![HirItem::Function {
                name: "main".to_string(),
                body: vec![HirStmt::CallPrint { arg: 7 }],
            }],
        };
        let mir = build(&hir);
        assert_eq!(
            mir.items,
            vec![MirItem::Function {
                name: "main".to_string(),
                body: vec![MirInstr::CallPrint { arg: 7 }],
            }]
        );
    }
}
