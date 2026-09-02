//! The driver's frontend seam: runs a source file through the parser, checked
//! HIR lowering, and the type checker, and renders whatever diagnostics the
//! first failing pass collected (#864 Part 1, D-217; Part 2's per-item HIR
//! collection, D-219, flows through the same payload).
//!
//! Extracted from `src/main.rs` (AGENTS.md's oversized-file rule) when the
//! failure payload became a `Vec<Diagnostic>`; `main.rs` keeps the command
//! dispatch and calls into here.

use crate::cli::ErrorFormat;
use crate::source;
use pycc_diag::Diagnostic;
use std::path::Path;

/// Why a frontend pass could not produce a typed module for one input.
pub(crate) enum FrontendFailure {
    /// The file could not be read or decoded (CLI_SPEC.md's exit-2
    /// invocation/environment class); never subject to `--error-format`.
    Input(String),
    /// A frontend pass rejected the file. `diagnostics` holds every
    /// diagnostic the *first failing pass* collected for it, in that pass's
    /// own collection order (the parser: ruff's discovery order, see
    /// `pycc_parser::parse_all`; HIR lowering: one per failing top-level
    /// item in source order, with cascades of an earlier skipped item
    /// suppressed, see `pycc_hir::lower_all` and D-219; the type checker:
    /// still one entry until #864 Part 3, #868, lands).
    ///
    /// Invariant: `diagnostics` is never empty. Every constructor below
    /// either wraps one `Diagnostic` in a `vec![...]` or forwards
    /// `pycc_parser::parse_all`'s or `pycc_hir::lower_all`'s `Err`, both
    /// non-empty by construction (proven by those crates' unit tests). No
    /// runtime assertion guards it:
    /// an `assert!` would add an uncoverable in-crate region under D-014's
    /// 100%-region gate. (Should the invariant ever break, `render_all`'s
    /// loops would print nothing and `check` would exit 1 silently -- a
    /// contract violation of CLI_SPEC.md's "exit 1 means at least one
    /// diagnostic", which is exactly why construction, not rendering, is
    /// what guarantees non-emptiness.)
    ///
    /// The payload used to be `Box<Diagnostic>` only to keep this variant
    /// under clippy's `result_large_err` threshold after D-152 grew
    /// `Diagnostic`; a `Vec` is three words, so the box is gone.
    Compile {
        diagnostics: Vec<Diagnostic>,
        source: String,
    },
}

pub(crate) fn lower_frontend(
    path: &Path,
) -> Result<(pycc_hir::HirModule, String), FrontendFailure> {
    let bytes = std::fs::read(path).map_err(|error| FrontendFailure::Input(error.to_string()))?;
    let source = source::decode_python_source(&bytes).map_err(FrontendFailure::Input)?;
    let module = match pycc_parser::parse_all(&source) {
        Ok(module) => module,
        Err(diagnostics) => {
            return Err(FrontendFailure::Compile {
                diagnostics,
                source,
            });
        }
    };
    let hir = match pycc_hir::lower_all(&module) {
        Ok(hir) => hir,
        Err(diagnostics) => {
            return Err(FrontendFailure::Compile {
                diagnostics,
                source,
            });
        }
    };
    Ok((hir, source))
}

pub(crate) fn check_frontend(path: &Path) -> Result<(), FrontendFailure> {
    let (hir, source) = lower_frontend(path)?;
    pycc_types::check(&hir).map_err(|diagnostic| FrontendFailure::Compile {
        diagnostics: vec![diagnostic],
        source,
    })
}

pub(crate) fn resolve_frontend(path: &Path) -> Result<pycc_hir::HirModule, FrontendFailure> {
    let (hir, source) = lower_frontend(path)?;
    pycc_types::check_and_resolve(&hir).map_err(|diagnostic| FrontendFailure::Compile {
        diagnostics: vec![diagnostic],
        source,
    })
}

/// Renders every collected diagnostic for one file, in order, into one
/// string. Human renders are concatenated with no separator (exactly how
/// `check_paths` already concatenates per-file renders); JSON renders are
/// one object per line (JSON Lines), the shape multi-file `check` already
/// produces. The caller decides the stream (stdout for `check`, stderr for
/// `build`/`run`) and the exit code.
fn render_all(
    diagnostics: &[Diagnostic],
    path: &str,
    source: &str,
    error_format: ErrorFormat,
) -> String {
    let mut out = String::new();
    for diagnostic in diagnostics {
        match error_format {
            ErrorFormat::Human => out.push_str(&pycc_diag::render_human(diagnostic, path, source)),
            ErrorFormat::Json => {
                out.push_str(&pycc_diag::render_json(diagnostic, path, source));
                out.push('\n');
            }
        }
    }
    out
}

fn report_input_failure(path: &str, message: &str) -> u8 {
    eprintln!(
        "error: could not read `{}`: {message}",
        pycc_diag::display_path(path)
    );
    2
}

/// `pycc check`'s reporter: diagnostics go to stdout in the selected
/// `--error-format`, every collected one; returns the exit code for this
/// file (`1` for any compile diagnostic, `2` for an unreadable input).
pub(crate) fn report_check_failure(
    path: &Path,
    failure: FrontendFailure,
    error_format: ErrorFormat,
) -> u8 {
    let path = path.to_string_lossy();
    match failure {
        FrontendFailure::Input(message) => report_input_failure(&path, &message),
        FrontendFailure::Compile {
            diagnostics,
            source,
        } => {
            print!("{}", render_all(&diagnostics, &path, &source, error_format));
            1
        }
    }
}

/// `pycc build`/`pycc run`'s reporter: the same human renders as `check`,
/// every collected diagnostic, written to stderr (these commands have no
/// `--error-format`). The build still stops here, before MIR -- only the
/// reporting changed with #864, not the fail-fast semantics.
pub(crate) fn report_build_failure(path: &Path, failure: FrontendFailure) -> u8 {
    let path = path.to_string_lossy();
    match failure {
        FrontendFailure::Input(message) => report_input_failure(&path, &message),
        FrontendFailure::Compile {
            diagnostics,
            source,
        } => {
            eprint!(
                "{}",
                render_all(&diagnostics, &path, &source, ErrorFormat::Human)
            );
            1
        }
    }
}
