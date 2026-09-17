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
    /// order, see `pycc_parser::parse_all`; HIR lowering: per top-level
    /// item in source order, the item's own diagnostic when it fails plus
    /// one enum-call `C0001` per enum-class call inside it that the scan
    /// can attribute (a shadowed or rebound name falls through to the type
    /// checker's span-less guard), with cascades of an earlier skipped item
    /// suppressed, see `pycc_hir::lower_module`, D-219 and D-233; linking:
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
    /// `import_bounds[i]` is the number of `ImportBinding`s contributed by
    /// files `0..=i`, the import-table analogue of `bounds` (Part 1 of
    /// #1026). Read by [`Self::owner_of_import`].
    import_bounds: Vec<usize>,
    /// `class_bounds[i]` is the number of `class_defs` entries contributed
    /// by files `0..=i`, the class-table analogue of `bounds`. Read by
    /// [`Self::owner_of_class`].
    ///
    /// It is *not* each module's own `hir.class_defs.len()`:
    /// `pycc_hir::link` drops a module's seeded builtin exception classes
    /// as it concatenates and appends one set for the whole program at the
    /// back, so a module's contribution is its non-seeded entry count. That
    /// trailing program-wide block is past the last bound and falls to the
    /// entry file, exactly as `bounds` treats `finalize`'s appended items.
    class_bounds: Vec<usize>,
}

impl ProgramSources {
    fn entry(&self) -> usize {
        self.files.len() - 1
    }

    fn owner(&self, key: DiagnosticKey) -> usize {
        match key.item_index() {
            Some(index) => self.owner_of_item(index),
            None => self.entry(),
        }
    }

    /// The file that owns item `index` of the linked program.
    fn owner_of_item(&self, index: usize) -> usize {
        self.bounds
            .partition_point(|end| *end <= index)
            .min(self.entry())
    }

    /// The file that owns import `position` of the linked program's import
    /// table (Part 1 of #1026).
    ///
    /// Same shape as [`Self::owner_of_item`] against the import bounds
    /// instead of the item bounds. An import's own `item_index` cannot
    /// serve here: it is the item count at the moment the `import` lowered,
    /// so a trailing import in one file and a leading import in the next
    /// record the same linked index. The import table has no such boundary
    /// ambiguity -- `pycc_hir::link` concatenates each module's imports in
    /// the same file order as its items.
    ///
    /// The `.min(entry())` clamp is unreachable here (the last import bound
    /// *is* the linked import count, so `partition_point` can never exceed
    /// `entry()`); it is kept for shape parity with `owner_of_item`, where
    /// `finalize`'s appended program-wide items make it load-bearing.
    fn owner_of_import(&self, position: usize) -> usize {
        self.import_bounds
            .partition_point(|end| *end <= position)
            .min(self.entry())
    }

    /// The file that owns class `index` of the linked program's class table
    /// (Part 1 of #1027).
    ///
    /// Same shape as [`Self::owner_of_item`] against the class bounds. The
    /// `.min(entry())` clamp is load-bearing here for the same reason it is
    /// there: `pycc_hir::link` appends the seeded builtin exception classes
    /// once, after every module's own contribution, so their indices sit
    /// past the last bound.
    fn owner_of_class(&self, index: usize) -> usize {
        self.class_bounds
            .partition_point(|end| *end <= index)
            .min(self.entry())
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
    let mut import_bounds = Vec::with_capacity(program.modules.len());
    let mut class_bounds = Vec::with_capacity(program.modules.len());
    let mut inputs = Vec::with_capacity(program.modules.len());
    let mut total = 0;
    let mut imports_total = 0;
    let mut classes_total = 0;
    for loaded in program.modules {
        total += loaded.module.hir.items.len();
        bounds.push(total);
        imports_total += loaded.module.hir.imports.len();
        import_bounds.push(imports_total);
        // Mirrors `pycc_hir::link`'s own filter exactly: a seeded module's
        // builtin exception classes are dropped as it concatenates, so
        // counting this module's raw `class_defs` would misalign every
        // later file's bound.
        let seeded = loaded.module.hir.seeded_builtin_exception_classes;
        classes_total += loaded
            .module
            .hir
            .class_defs
            .iter()
            .filter(|(name, _)| !(seeded && pycc_hir::is_builtin_exception_class(name)))
            .count();
        class_bounds.push(classes_total);
        files.push((loaded.display_path.clone(), loaded.source));
        inputs.push(LinkInput {
            display_path: loaded.display_path,
            module: loaded.module,
        });
    }
    let sources = ProgramSources {
        files,
        bounds,
        import_bounds,
        class_bounds,
    };
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

/// [`resolve_frontend`] plus the native-mode artifact gates, for a
/// `pycc build` without `--ext`: the foreign-import gate (Part 1 of #1026)
/// and the `memoryview`-annotation gate (Part 1 of #1027).
///
/// The gate runs here rather than in `main.rs` for one reason: only this
/// module holds the `ProgramSources` that says which *file* an import
/// belongs to, and a foreign import in a dependency must be reported
/// against that dependency, not against the entry path (PR 1c of #1080
/// review finding 2).
///
/// Order matters twice, and the two gates resolve it differently.
///
/// Both are *computed* before the type check, against the linked HIR whose
/// item indices still line up with the per-file bounds --
/// `check_and_resolve_all_keyed` runs monomorphization and enum lowering,
/// which rewrite the item list and recompute those positions.
///
/// The foreign-import gate is *reported* after, so a program with both a
/// type error and a foreign import still reports the type error first,
/// exactly as the former `main.rs` call site did -- when no `memoryview`
/// gap fires in the same program. When one does, the early return below
/// reports the collected gaps and the type check never runs at all, so
/// there is no type error to order against. The `memoryview` gate
/// cannot be: `crates/pycc_types`'s `reject_memoryview_read` refuses every
/// *use* of a `memoryview`-typed name with `C0001` except the `b[i]`
/// element load Part 2 of #1027 admits, so a body that so much as reads
/// its own parameter any other way fails the type check first, and the
/// signature-level `I0405` that `docs/RUNTIME.md` and D-244 promise for
/// "a `memoryview` in a signature in a build without `--ext`" would never
/// be emitted (#1115 review round 5). It is therefore reported *before*
/// the type check -- together with any foreign-import gap found in the
/// same program, so prioritizing it never swallows an `I0403` that would
/// otherwise have been reported.
///
/// `pycc check` selects no artifact mode and so runs neither gate:
/// [`check_frontend`] reports the `C0001` read refusal there, which is
/// correct, because `I0405`'s contract is scoped to a *build* without
/// `--ext`.
pub(crate) fn resolve_frontend_native(path: &Path) -> Result<HirModule, FrontendFailure> {
    let (hir, sources) = link_frontend(path)?;
    let import_gaps = crate::foreign_import::refuse_in_native_mode(&hir);
    // Keyed by *item* index rather than import position: a `memoryview`
    // annotation lives on an `HirItem::Function`, not in the import table,
    // so it resolves to its owning file through the item bounds.
    let memoryview_gaps = crate::memoryview_mode::refuse_in_native_mode(&hir);
    // Keyed by *class* index: a protocol method's signature is a
    // `ProtocolMember::Method` in `hir.class_defs`, never an
    // `HirItem::Function`, so it resolves through the class bounds instead.
    let protocol_gaps = crate::memoryview_mode::refuse_protocol_methods_in_native_mode(&hir);
    let mut keyed: Vec<(usize, Diagnostic)> = Vec::new();
    if let Err(gaps) = import_gaps {
        keyed.extend(
            gaps.into_iter()
                .map(|(position, diagnostic)| (sources.owner_of_import(position), diagnostic)),
        );
    }
    let refused_a_memoryview_signature = memoryview_gaps.is_err() || protocol_gaps.is_err();
    if let Err(gaps) = memoryview_gaps {
        keyed.extend(
            gaps.into_iter()
                .map(|(index, diagnostic)| (sources.owner_of_item(index), diagnostic)),
        );
    }
    if let Err(gaps) = protocol_gaps {
        keyed.extend(
            gaps.into_iter()
                .map(|(index, diagnostic)| (sources.owner_of_class(index), diagnostic)),
        );
    }
    if refused_a_memoryview_signature {
        return Err(sources.group(keyed));
    }
    let resolved = pycc_types::check_and_resolve_all_keyed(&hir)
        .map_err(|keyed| sources.group(attribute(&sources, keyed)))?;
    if keyed.is_empty() {
        return Ok(resolved);
    }
    Err(sources.group(keyed))
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
