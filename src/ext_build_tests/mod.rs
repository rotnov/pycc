//! Unit tests for the `--ext` build seam.
//!
//! Every test here is non-`#[ignore]`d and needs no CPython installation:
//! `.github/workflows/ci.yml`'s coverage job runs `llvm-cov` without
//! `--include-ignored`, so an ignored test earns zero coverage while its
//! lines stay in `scripts/check_diff_coverage.py`'s denominator. The
//! CPython-driven behaviour of a *built* artifact is verified separately by
//! the ignored oracle tests under `tests/`.
//!
//! No test here asserts a rendered `Path`: `Display`/`to_string_lossy`
//! carries the building host's separator, so `dist/m` renders as `dist\m` on
//! Windows. Paths are compared as `PathBuf`s built with `Path::join`.

use super::*;
use pycc_hir::Ty;

fn func(name: &str, params: &[(&str, Ty)], return_ty: Ty) -> HirItem {
    HirItem::Function {
        name: name.to_string(),
        params: params
            .iter()
            .map(|(n, ty)| ((*n).to_string(), ty.clone()))
            .collect(),
        return_ty,
        body: Vec::new(),
    }
}

fn module(items: Vec<HirItem>) -> HirModule {
    HirModule {
        items,
        type_aliases: Vec::new(),
        imports: Vec::new(),
        class_defs: Vec::new(),
        seeded_builtin_exception_classes: false,
    }
}

fn probe(version: (u32, u32), include: &Path) -> ExtProbe {
    ExtProbe {
        version,
        include: include.to_path_buf(),
        libs: include.join("libs"),
    }
}

mod exports;
mod generated_c;
mod toolchain;
