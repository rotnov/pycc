use pycc_diag::Diagnostic;
use pycc_hir::HirModule;

pub fn check(_hir: &HirModule) -> Result<(), Diagnostic> {
    // v0.1 slice-0: nothing in this HIR subset is rejectable yet.
    // Real T0001 strictness + local inference land in PR-4.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_1_slice_always_type_checks() {
        let hir = HirModule { items: vec![] };
        assert!(check(&hir).is_ok());
    }
}
