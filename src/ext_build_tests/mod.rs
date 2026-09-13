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
//!
//! For the same reason, a test that matches a *multi-line* fragment of
//! [`SHIM_C`] goes through [`shim_c`] rather than reading the constant
//! directly. A single-line match needs no normalization and uses `SHIM_C`.

use super::*;
use pycc_hir::Ty;

/// [`SHIM_C`] with its line endings normalized to `\n`.
///
/// `include_str!` embeds the shim exactly as the checkout holds it, and a
/// Windows checkout holds it with CRLF endings, so an assertion spanning a
/// line break matches on four Tier-1 targets and fails on the fifth. The
/// shipped artifact is deliberately left alone -- a C compiler does not care
/// which ending the source carries, and rewriting it here would make the
/// tests describe something other than what `--ext` writes out.
fn shim_c() -> String {
    SHIM_C.replace("\r\n", "\n")
}

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
mod refusal_completeness;
mod toolchain;
