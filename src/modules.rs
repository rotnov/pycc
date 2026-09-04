//! The driver's project-module loader (#898, Part 1 of #881, D-222).
//!
//! `pycc_hir` is deliberately filesystem-free: it publishes
//! [`pycc_hir::ProjectImportRequest`]s for the imports its own `pycc_std`
//! registry does not answer, and consumes the driver's answers through
//! [`pycc_hir::ResolvedImports`]. Everything filesystem-shaped -- finding
//! the source root, mapping a dotted module name to a file, reading and
//! decoding it, ordering package `__init__.py` side effects, detecting
//! import cycles -- lives here.
//!
//! The loader is a depth-first walk from the entry file. Each module is
//! parsed, its project imports resolved (loading each dependency first),
//! and then lowered with `pycc_hir::lower_module`; the finished modules
//! come out in dependency order with the entry last, which is exactly the
//! order `pycc_hir::link` wants.
//!
//! Source-root discovery is *lazy*: a file with no project import never
//! touches the filesystem beyond its own read, which is what keeps the
//! `check_frontend_throughput.rb` gate honest.

use crate::frontend::FrontendFailure;
use crate::project_config;
use crate::source;
use pycc_hir::{
    LoweredModule, ProjectImportRequest, ResolvedImport, ResolvedImports, ResolvedModule,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One loaded project module: the path diagnostics render for it, its
/// decoded source (kept so a later pass can render against it), and its
/// lowering.
pub(crate) struct LoadedModule {
    pub(crate) display_path: String,
    pub(crate) source: String,
    pub(crate) module: LoweredModule,
}

/// Every module reachable from the entry file, in dependency order with
/// the entry file last (post-order of the import walk), which is the order
/// `pycc_hir::link` expects and the order top-level statements run in.
pub(crate) struct LoadedProgram {
    pub(crate) modules: Vec<LoadedModule>,
}

/// The source root a dotted absolute module name resolves against, in both
/// spellings: the canonical path filesystem probes use, and the as-typed
/// path diagnostics render (see [`display_root`]).
#[derive(Clone)]
struct RootInfo {
    canonical: PathBuf,
    display: PathBuf,
}

/// What the loader decided about one [`ProjectImportRequest`]. Owned (not
/// borrowing the module store) so the resolution phase can keep mutating
/// that store while the answers accumulate.
enum Resolution {
    /// The dependency loaded; `index` addresses it in the module store.
    Loaded {
        index: usize,
        submodules: Vec<String>,
    },
    /// A bare `import m` naming a real project module: recognized, not
    /// loaded (Part 1 binds no module namespace).
    Found,
    /// The import cannot be satisfied, with the exact diagnostic to report.
    NotFound { code: &'static str, message: String },
    /// Not a project import after all (an absolute module name that
    /// resolves nowhere on disk): left unanswered so `pycc_hir` reports it
    /// exactly as a single-file compilation would.
    Unanswered,
}

/// Loads the whole program reachable from `entry`.
pub(crate) fn load(entry: &Path) -> Result<LoadedProgram, FrontendFailure> {
    let display = entry.to_string_lossy().into_owned();
    let canonical = canonicalize(entry, &display)?;
    let mut entry_dir = canonical.clone();
    entry_dir.pop();
    let mut entry_display_dir = PathBuf::from(&display);
    entry_display_dir.pop();
    let mut loader = Loader {
        modules: Vec::new(),
        memo: HashMap::new(),
        in_progress: Vec::new(),
        entry_dir,
        entry_display_dir,
        root: None,
    };
    loader.load_module(&canonical, display)?;
    Ok(LoadedProgram {
        modules: loader.modules,
    })
}

struct Loader {
    modules: Vec<LoadedModule>,
    /// Canonical path -> index in `modules`, so two spellings of one file
    /// (a symlink, a `./` prefix) are one module.
    memo: HashMap<PathBuf, usize>,
    /// The canonical paths currently being loaded, innermost last, with
    /// their display spellings: the import-cycle chain.
    in_progress: Vec<(PathBuf, String)>,
    entry_dir: PathBuf,
    entry_display_dir: PathBuf,
    root: Option<RootInfo>,
}

impl Loader {
    /// Parses, resolves and lowers one module, loading every dependency it
    /// imports first. Returns its index in `modules`.
    fn load_module(&mut self, canonical: &Path, display: String) -> Result<usize, FrontendFailure> {
        if let Some(index) = self.memo.get(canonical) {
            return Ok(*index);
        }
        let bytes = std::fs::read(canonical)
            .map_err(|error| FrontendFailure::input(display.clone(), error.to_string()))?;
        let source = source::decode_python_source(&bytes)
            .map_err(|message| FrontendFailure::input(display.clone(), message))?;
        let parsed = pycc_parser::parse_all(&source)
            .map_err(|diagnostics| FrontendFailure::compile(&display, &source, diagnostics))?;

        self.in_progress
            .push((canonical.to_path_buf(), display.clone()));
        let requests = pycc_hir::project_import_requests(&parsed);
        let mut answers = Vec::with_capacity(requests.len());
        for request in &requests {
            answers.push((request.span, self.resolve(request, &display, canonical)?));
        }
        self.in_progress.pop();

        let mut resolved = ResolvedImports::default();
        for loaded in &self.modules {
            resolved.add_module(loaded.display_path.clone(), &loaded.module.hir);
        }
        for (span, answer) in answers {
            match answer {
                Resolution::Loaded { index, submodules } => {
                    let loaded = &self.modules[index];
                    resolved.insert(
                        span,
                        ResolvedImport::Module(ResolvedModule {
                            display_path: loaded.display_path.clone(),
                            hir: &loaded.module.hir,
                            submodule_names: submodules,
                        }),
                    );
                }
                Resolution::Found => resolved.insert(span, ResolvedImport::Found),
                Resolution::NotFound { code, message } => {
                    resolved.insert(span, ResolvedImport::NotFound { code, message });
                }
                Resolution::Unanswered => {}
            }
        }
        let module = pycc_hir::lower_module(&parsed, &resolved)
            .map_err(|diagnostics| FrontendFailure::compile(&display, &source, diagnostics))?;
        drop(resolved);

        let index = self.modules.len();
        self.modules.push(LoadedModule {
            display_path: display,
            source,
            module,
        });
        self.memo.insert(canonical.to_path_buf(), index);
        Ok(index)
    }

    /// Answers one import request: finds the target file, reports every
    /// CPython-rejected or not-yet-supported shape, and otherwise loads the
    /// dependency (plus the package `__init__.py`s on the way to it).
    fn resolve(
        &mut self,
        request: &ProjectImportRequest,
        importer_display: &str,
        importer_canonical: &Path,
    ) -> Result<Resolution, FrontendFailure> {
        let base = self.base_dir(request, importer_display, importer_canonical)?;
        let segments: Vec<&str> = match &request.module {
            Some(module) => module.split('.').collect(),
            None => Vec::new(),
        };
        let target = match self.probe(&base, &segments, request) {
            Ok(target) => target,
            Err(resolution) => return Ok(resolution),
        };
        if request.names.is_empty() {
            // A bare `import m`: the module exists, but Part 1 binds no
            // module namespace, so the file is never loaded.
            return Ok(Resolution::Found);
        }
        let canonical = identity_path(&target.path);
        if let Some(position) = self
            .in_progress
            .iter()
            .position(|(path, _)| *path == canonical)
        {
            let mut chain: Vec<String> = self.in_progress[position..]
                .iter()
                .map(|(_, display)| format!("`{}`", pycc_diag::display_path(display)))
                .collect();
            chain.push(format!("`{}`", pycc_diag::display_path(&target.display)));
            return Ok(Resolution::NotFound {
                code: "E0108",
                message: format!("import cycle: {}", chain.join(" -> ")),
            });
        }
        for (init_path, init_display) in target.package_inits {
            let init_canonical = identity_path(&init_path);
            // An `__init__.py` already on the in-progress stack is the
            // importer itself (a package initializer importing its own
            // submodule); loading it again would be a cycle. Re-loading an
            // *already finished* one is harmless -- `load_module` memoizes.
            if self
                .in_progress
                .iter()
                .any(|(path, _)| *path == init_canonical)
            {
                continue;
            }
            self.load_module(&init_canonical, init_display)?;
        }
        let index = self.load_module(&canonical, target.display)?;
        Ok(Resolution::Loaded {
            index,
            submodules: target.submodules,
        })
    }

    /// The directory `request`'s dotted name resolves against: the source
    /// root for an absolute import, the importer's package (climbed
    /// `level - 1` times) for a relative one. A base that cannot be
    /// resolved at all carries its own rejection (see [`Base::rejected`])
    /// rather than being absent.
    fn base_dir(
        &mut self,
        request: &ProjectImportRequest,
        importer_display: &str,
        importer_canonical: &Path,
    ) -> Result<Base, FrontendFailure> {
        if request.level == 0 {
            let root = self.source_root()?;
            return Ok(Base {
                path: root.canonical,
                display: root.display,
                relative: false,
                rejection: None,
            });
        }
        let mut display = PathBuf::from(importer_display);
        display.pop();
        if display.as_os_str().is_empty() {
            // The entry was named by a bare file name, so its directory
            // spelling is empty; `.` is the same directory and is what a
            // diagnostic can actually render.
            display = PathBuf::from(".");
        }
        // The importer's own canonical path is already known, so the base
        // never has to be re-derived from the display spelling: popping the
        // file name off it is exact even when the display path is relative
        // to the process working directory or spelled through a symlink.
        let mut canonical = importer_canonical.to_path_buf();
        canonical.pop();
        for climb in 0..request.level {
            if !canonical.join("__init__.py").is_file() {
                let message = if climb == 0 {
                    format!(
                        "attempted relative import with no known parent package: `{}` has no `__init__.py`",
                        pycc_diag::display_path(&display.to_string_lossy())
                    )
                } else {
                    "attempted relative import beyond the top-level package".to_string()
                };
                return Ok(Base::rejected(message));
            }
            if climb + 1 < request.level {
                canonical.pop();
                display.pop();
            }
        }
        Ok(Base {
            path: canonical,
            display,
            relative: true,
            rejection: None,
        })
    }

    /// Discovers, once per invocation, the directory absolute project
    /// module names resolve against.
    fn source_root(&mut self) -> Result<RootInfo, FrontendFailure> {
        if let Some(root) = &self.root {
            return Ok(root.clone());
        }
        let root = self.discover_root()?;
        self.root = Some(root.clone());
        Ok(root)
    }

    fn discover_root(&self) -> Result<RootInfo, FrontendFailure> {
        for (climbs, ancestor) in self.entry_dir.ancestors().enumerate() {
            let toml = ancestor.join("pycc.toml");
            if !toml.is_file() {
                continue;
            }
            // Rendered as-typed (relative to the entry's own spelling)
            // rather than as the canonical absolute path the walk uses.
            let display = display_root(&self.entry_display_dir, climbs)
                .join("pycc.toml")
                .to_string_lossy()
                .into_owned();
            let contents = std::fs::read_to_string(&toml)
                .map_err(|error| FrontendFailure::input(display.clone(), error.to_string()))?;
            let config = project_config::parse(&contents)
                .map_err(|message| FrontendFailure::input(display, message))?;
            let mut root = ancestor.join(&config.project.entry);
            root.pop();
            let resolved = root
                .canonicalize()
                .ok()
                .and_then(|root| climbs_between(&self.entry_dir, &root).map(|c| (root, c)));
            if let Some((canonical, climbs)) = resolved {
                return Ok(RootInfo {
                    canonical,
                    display: display_root(&self.entry_display_dir, climbs),
                });
            }
            break;
        }
        let mut climbs = 0;
        let mut dir = self.entry_dir.clone();
        while dir.join("__init__.py").is_file() && dir.pop() {
            climbs += 1;
        }
        Ok(RootInfo {
            canonical: dir,
            display: display_root(&self.entry_display_dir, climbs),
        })
    }

    /// Walks `segments` down from `base`, returning the module file they
    /// name plus the package `__init__.py`s passed on the way. `Err`
    /// carries the [`Resolution`] to report instead.
    fn probe(
        &self,
        base: &Base,
        segments: &[&str],
        request: &ProjectImportRequest,
    ) -> Result<Target, Resolution> {
        if let Some(rejection) = &base.rejection {
            return Err(Resolution::NotFound {
                code: "T0021",
                message: rejection.clone(),
            });
        }
        let mut path = base.path.clone();
        let mut display = base.display.clone();
        let mut package_inits = Vec::new();
        for (position, segment) in segments.iter().enumerate() {
            let last = position + 1 == segments.len();
            let file = path.join(format!("{segment}.py"));
            let file_display = display.join(format!("{segment}.py"));
            if last && file.is_file() {
                return Ok(Target::new(file, file_display, package_inits, Vec::new()));
            }
            let init = path.join(segment).join("__init__.py");
            let init_display = display.join(segment).join("__init__.py");
            if !path.join(segment).is_dir() {
                return Err(self.missing(base, request));
            }
            if last {
                if !init.is_file() {
                    return Err(Resolution::NotFound {
                        code: "C0001",
                        message: format!(
                            "namespace package `{}` (a directory without `__init__.py`) is not supported yet",
                            pycc_diag::display_path(&display.join(segment).to_string_lossy())
                        ),
                    });
                }
                let submodules = submodule_names(&path.join(segment));
                return Ok(Target::new(init, init_display, package_inits, submodules));
            }
            if init.is_file() {
                package_inits.push((init, init_display));
            }
            path.push(segment);
            display.push(segment);
        }
        // A relative `from . import x`: the base package itself.
        let submodules = submodule_names(&path);
        Ok(Target::new(
            path.join("__init__.py"),
            display.join("__init__.py"),
            package_inits,
            submodules,
        ))
    }

    /// The diagnostic for a dotted name that resolves to nothing: a
    /// CPython-rejected relative import (`T0021`), or -- for an absolute
    /// name -- no answer at all, so `pycc_hir` keeps its own
    /// "import of module `x` is not supported yet" `C0001`.
    fn missing(&self, base: &Base, request: &ProjectImportRequest) -> Resolution {
        if !base.relative {
            return Resolution::Unanswered;
        }
        let spec = format!(
            "{}{}",
            ".".repeat(request.level as usize),
            request.module.as_deref().unwrap_or_default()
        );
        Resolution::NotFound {
            code: "T0021",
            message: format!(
                "no module named `{spec}` in `{}`",
                pycc_diag::display_path(&base.display.to_string_lossy())
            ),
        }
    }
}

/// The directory a dotted name resolves against, or the reason it cannot
/// be resolved at all (a relative import outside a package).
struct Base {
    path: PathBuf,
    display: PathBuf,
    relative: bool,
    rejection: Option<String>,
}

impl Base {
    fn rejected(message: String) -> Self {
        Self {
            path: PathBuf::new(),
            display: PathBuf::new(),
            relative: true,
            rejection: Some(message),
        }
    }
}

struct Target {
    path: PathBuf,
    display: String,
    package_inits: Vec<(PathBuf, String)>,
    submodules: Vec<String>,
}

impl Target {
    fn new(
        path: PathBuf,
        display: PathBuf,
        package_inits: Vec<(PathBuf, PathBuf)>,
        submodules: Vec<String>,
    ) -> Self {
        Self {
            path,
            display: display.to_string_lossy().into_owned(),
            package_inits: package_inits
                .into_iter()
                .map(|(path, display)| (path, display.to_string_lossy().into_owned()))
                .collect(),
            submodules,
        }
    }
}

/// The submodule names a package directory offers: every `name.py` other
/// than `__init__.py`, and every subdirectory. Sorted so a diagnostic that
/// mentions them is deterministic across filesystems.
fn submodule_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            match name.strip_suffix(".py") {
                Some(stem) => (stem != "__init__").then(|| stem.to_string()),
                None => entry.path().is_dir().then_some(name),
            }
        })
        .collect();
    names.sort();
    names
}

/// How many directory components separate `dir` from its ancestor `root`,
/// or `None` when `root` is not an ancestor of `dir` at all.
fn climbs_between(dir: &Path, root: &Path) -> Option<usize> {
    dir.strip_prefix(root)
        .ok()
        .map(|rest| rest.components().count())
}

/// The as-typed spelling of a source root reached by climbing `climbs`
/// directories out of `dir`. A total function: when `dir` has fewer
/// components than `climbs` (the entry was named by a bare file name, so
/// the root is above the working directory), the remainder is padded with
/// `..` so the rendered path still points at the right place.
fn display_root(dir: &Path, climbs: usize) -> PathBuf {
    let components: Vec<_> = dir.components().collect();
    let keep = components.len().saturating_sub(climbs);
    let mut root = PathBuf::new();
    for component in &components[..keep] {
        root.push(component);
    }
    for _ in 0..climbs.saturating_sub(components.len()) {
        root.push("..");
    }
    root
}

fn canonicalize(path: &Path, display: &str) -> Result<PathBuf, FrontendFailure> {
    path.canonicalize()
        .map_err(|error| FrontendFailure::input(display.to_string(), error.to_string()))
}

/// The identity a module is memoized under: its canonical form when the
/// filesystem can produce one, and the path as given otherwise. Both
/// callers have already found the file with `is_file`, and each is about to
/// read it, so a canonicalization failure here is not the right place to
/// report: the read that follows raises the real diagnostic, and the only
/// cost of the fallback is that two spellings of one unreadable file
/// memoize separately.
fn identity_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests;
