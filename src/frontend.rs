//! The driver's frontend seam: runs a program through the parser, checked
//! HIR lowering, whole-program linking, and the type checker, and renders
//! whatever diagnostics the first failing pass collected (#864 Part 1,
//! D-217; Part 2's per-item HIR collection, D-219, and Part 3's
//! per-function type-checker collection, D-220, flow through the same
//! payload).
//!
//! Since #898 (Part 1 of #881, D-222) a "program" is one or more files:
//! `src/modules.rs` loads and lowers the entry file's whole import
//! closure, `pycc_hir::link` concatenates the modules into one
//! `HirModule`, and the type checker's keyed diagnostics
//! (`pycc_types::DiagnosticKey`) are mapped back to the file that owns
//! the item they came from.
//!
//! Extracted from `src/main.rs` (AGENTS.md's oversized-file rule) when the
//! failure payload became a `Vec<Diagnostic>`; `main.rs` keeps the command
//! dispatch and calls into here.

use crate::cli::ErrorFormat;
use crate::modules::{self, LoadedProgram};
use pycc_diag::Diagnostic;
use pycc_hir::{HirModule, LinkInput};
use pycc_types::DiagnosticKey;
use std::path::Path;

/// Every diagnostic collected for one file of the program, with the source
/// they render against. `diagnostics` is never empty: a file with none is
/// never added to [`FrontendFailure::Compile`].
pub(crate) struct FileDiagnostics {
    pub(crate) path: String,
    pub(crate) source: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// Why a frontend pass could not produce a typed program.
pub(crate) enum FrontendFailure {
    /// A file could not be read or decoded (CLI_SPEC.md's exit-2
    /// invocation/environment class); never subject to `--error-format`.
    /// `path` names the file that failed, which since #898 is not
    /// necessarily the file named on the command line -- an unreadable
    /// *dependency* fails the same way, under its own path.
    Input { path: String, message: String },
    /// A frontend pass rejected the program. Each entry holds every
    /// diagnostic the *first failing pass* collected for that file, in
    /// that pass's own collection order (the parser: ruff's discovery
    /// order, see `pycc_parser::parse_all`; HIR lowering: one per failing
    /// top-level item in source order, with cascades of an earlier skipped
    /// item suppressed, see `pycc_hir::lower_module` and D-219; linking:
    /// the first cross-module conflict; the type checker: one per failing
    /// item, solver-first per function; a pre-check or module-level solver
    /// failure is reported alone, otherwise the checker's entries for
    /// functions the solver did not flag follow the solver's -- see
    /// `pycc_types::check_all_keyed` and D-220). Files appear in the
    /// program's own dependency order, entry last.
    ///
    /// Invariant: `files` is non-empty and every `diagnostics` is
    /// non-empty. Every constructor below either wraps one non-empty
    /// diagnostic list or forwards `pycc_parser::parse_all`'s,
    /// `pycc_hir::lower_module`'s/`link`'s/`finalize`'s, or
    /// `pycc_types::check_all_keyed`/`check_and_resolve_all_keyed`'s `Err`,
    /// all non-empty by construction (proven by those crates' unit tests).
    /// No runtime assertion guards it: an `assert!` would add an
    /// uncoverable in-crate region under D-014's 100%-region gate. (Should
    /// the invariant ever break, `render_all`'s loops would print nothing
    /// and `check` would exit 1 silently -- a contract violation of
    /// CLI_SPEC.md's "exit 1 means at least one diagnostic", which is
    /// exactly why construction, not rendering, is what guarantees
    /// non-emptiness.)
    Compile { files: Vec<FileDiagnostics> },
}

impl FrontendFailure {
    pub(crate) fn input(path: String, message: String) -> Self {
        Self::Input { path, message }
    }

    /// The single-file compile failure: every diagnostic belongs to `path`.
    pub(crate) fn compile(path: &str, source: &str, diagnostics: Vec<Diagnostic>) -> Self {
        Self::Compile {
            files: vec![FileDiagnostics {
                path: path.to_string(),
                source: source.to_string(),
                diagnostics,
            }],
        }
    }
}

/// The per-file sources of a loaded program, kept alongside the linked
/// module so a diagnostic can be rendered against the file that owns it.
struct ProgramSources {
    files: Vec<(String, String)>,
    /// `bounds[i]` is the number of `HirModule` items contributed by files
    /// `0..=i`, so `bounds.partition_point(|end| *end <= index)` names the
    /// file that owns item `index`. An index past the last bound belongs to
    /// an item `link`/`finalize` appended for the whole program (the seeded
    /// builtin exception classes, `Exception.__init__`), which is attributed
    /// to the entry file.
    bounds: Vec<usize>,
}

impl ProgramSources {
    fn entry(&self) -> usize {
        self.files.len() - 1
    }

    fn owner(&self, key: DiagnosticKey) -> usize {
        match key.item_index() {
            Some(index) => self
                .bounds
                .partition_point(|end| *end <= index)
                .min(self.entry()),
            None => self.entry(),
        }
    }

    /// Groups keyed diagnostics into per-file payloads, in program order.
    fn group(&self, keyed: Vec<(usize, Diagnostic)>) -> FrontendFailure {
        let mut files: Vec<FileDiagnostics> = self
            .files
            .iter()
            .map(|(path, source)| FileDiagnostics {
                path: path.clone(),
                source: source.clone(),
                diagnostics: Vec::new(),
            })
            .collect();
        for (index, diagnostic) in keyed {
            files[index].diagnostics.push(diagnostic);
        }
        files.retain(|file| !file.diagnostics.is_empty());
        FrontendFailure::Compile { files }
    }
}

/// Loads and links the entry file's whole import closure into the single
/// `HirModule` the rest of the pipeline consumes.
fn link_frontend(path: &Path) -> Result<(HirModule, ProgramSources), FrontendFailure> {
    let program: LoadedProgram = modules::load(path)?;
    let mut files = Vec::with_capacity(program.modules.len());
    let mut bounds = Vec::with_capacity(program.modules.len());
    let mut inputs = Vec::with_capacity(program.modules.len());
    let mut total = 0;
    for loaded in program.modules {
        total += loaded.module.hir.items.len();
        bounds.push(total);
        files.push((loaded.display_path.clone(), loaded.source));
        inputs.push(LinkInput {
            display_path: loaded.display_path,
            module: loaded.module,
        });
    }
    let sources = ProgramSources { files, bounds };
    let linked = pycc_hir::link(inputs).map_err(|keyed| sources.group(keyed))?;
    let hir = pycc_hir::finalize(linked).map_err(|diagnostics| {
        let entry = sources.entry();
        sources.group(diagnostics.into_iter().map(|d| (entry, d)).collect())
    })?;
    Ok((hir, sources))
}

pub(crate) fn check_frontend(path: &Path) -> Result<(), FrontendFailure> {
    let (hir, sources) = link_frontend(path)?;
    pycc_types::check_all_keyed(&hir).map_err(|keyed| sources.group(attribute(&sources, keyed)))
}

pub(crate) fn resolve_frontend(path: &Path) -> Result<HirModule, FrontendFailure> {
    let (hir, sources) = link_frontend(path)?;
    pycc_types::check_and_resolve_all_keyed(&hir)
        .map_err(|keyed| sources.group(attribute(&sources, keyed)))
}

fn attribute(
    sources: &ProgramSources,
    keyed: pycc_types::KeyedDiagnostics,
) -> Vec<(usize, Diagnostic)> {
    keyed
        .into_iter()
        .map(|(key, diagnostic)| (sources.owner(key), diagnostic))
        .collect()
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

fn render_files(files: &[FileDiagnostics], error_format: ErrorFormat) -> String {
    let mut out = String::new();
    for file in files {
        out.push_str(&render_all(
            &file.diagnostics,
            &file.path,
            &file.source,
            error_format,
        ));
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
pub(crate) fn report_check_failure(failure: FrontendFailure, error_format: ErrorFormat) -> u8 {
    match failure {
        FrontendFailure::Input { path, message } => report_input_failure(&path, &message),
        FrontendFailure::Compile { files } => {
            print!("{}", render_files(&files, error_format));
            1
        }
    }
}

/// `pycc build`/`pycc run`'s reporter: the same human renders as `check`,
/// every collected diagnostic, written to stderr (these commands have no
/// `--error-format`). The build still stops here, before MIR -- only the
/// reporting changed with #864, not the fail-fast semantics.
pub(crate) fn report_build_failure(failure: FrontendFailure) -> u8 {
    match failure {
        FrontendFailure::Input { path, message } => report_input_failure(&path, &message),
        FrontendFailure::Compile { files } => {
            eprint!("{}", render_files(&files, ErrorFormat::Human));
            1
        }
    }
}
