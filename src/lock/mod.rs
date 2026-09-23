//! `pycc lock`: records the CPython dependency closure an embedded build
//! will carry, read offline from the `PYCC_PYTHON` interpreter's installed
//! environment, into a per-entry, per-triple `pycc.lock` (the pycc.lock
//! decision entry under `docs/decisions/`; `docs/CLI_SPEC.md` describes
//! the command).

pub(crate) mod dist;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod marker;
pub(crate) mod probe;
pub(crate) mod requirement;
pub(crate) mod resolve;
pub(crate) mod schema;

use crate::embed::stdlib_roots::{is_embeddable_stdlib_root, is_excluded_stdlib_root};
use crate::embed::{self, EmbedToolchain};
use crate::frontend::{self, FrontendFailure};
use crate::interop_policy::InteropCli;
use pycc_hir::{HirModule, ImportBinding};
use schema::{LOCK_FILE_NAME, LOCK_VERSION, Lock, LockTarget, LockedPackage};
use std::collections::BTreeSet;
use std::path::Path;

/// Why `pycc lock` failed, by exit class (`docs/CLI_SPEC.md`).
pub(crate) enum LockFailure {
    /// The program itself was refused: a parse or lowering diagnostic, an
    /// `I0402` policy gap (exit 1), or an unreadable input (exit 2).
    Frontend(FrontendFailure),
    /// The host, the interpreter, its environment or the lock file cannot
    /// be used (exit 2).
    Env(String),
    /// `--check` found the lock is not what `pycc lock` would write (exit 1).
    Stale(String),
}

impl From<FrontendFailure> for LockFailure {
    fn from(failure: FrontendFailure) -> Self {
        Self::Frontend(failure)
    }
}

/// Prints `failure` to stderr and returns its exit code.
pub(crate) fn report_lock_failure(failure: LockFailure) -> u8 {
    match failure {
        LockFailure::Frontend(failure) => frontend::report_build_failure(failure),
        LockFailure::Env(message) => {
            eprintln!("error: {message}");
            2
        }
        LockFailure::Stale(message) => {
            eprintln!("error: {message}");
            1
        }
    }
}

/// `pycc lock PATH [--check]` on this host (the pycc.lock decision entry,
/// rule 6).
pub(crate) fn run_lock(
    path: &Path,
    check: bool,
    interop: InteropCli,
    toolchain: &EmbedToolchain,
) -> Result<(), LockFailure> {
    let host = (std::env::consts::ARCH, std::env::consts::OS);
    run_lock_on(path, check, interop, toolchain, host)
}

/// [`run_lock`] for an explicit `(arch, os)` host, so the host refusals are
/// testable on every host.
fn run_lock_on(
    path: &Path,
    check: bool,
    interop: InteropCli,
    toolchain: &EmbedToolchain,
    (arch, os): (&str, &str),
) -> Result<(), LockFailure> {
    if os == "windows" {
        return Err(LockFailure::Env(
            "`pycc lock` does not run on a Windows host yet: it locks the closure an embedded \
             build carries, and pycc does not embed CPython into a Windows executable yet (#1226)"
                .to_string(),
        ));
    }
    let triple = schema::host_triple(arch, os).map_err(LockFailure::Env)?;
    let hir = frontend::lock_frontend(path, interop)?;
    let roots = import_roots(&hir);
    let entry = std::fs::canonicalize(path)
        .map_err(|e| LockFailure::Env(format!("cannot resolve `{}`: {e}", path.display())))?;
    let entry_dir = entry.parent().unwrap_or(&entry);
    let lock_dir = crate::modules::nearest_manifest(entry_dir).map_or(entry_dir, |(_, dir)| dir);
    let key =
        entry_key(entry.strip_prefix(lock_dir).unwrap_or(&entry)).map_err(LockFailure::Env)?;
    let lock_path = lock_dir.join(LOCK_FILE_NAME);
    let existing_text = read_existing(&lock_path)?;
    let existing =
        match &existing_text {
            Some(text) => Some(schema::parse(text).map_err(|m| {
                LockFailure::Env(format!("cannot use `{}`: {m}", lock_path.display()))
            })?),
            None => None,
        };
    let old_section = existing
        .as_ref()
        .and_then(|lock| find_section(lock, &key, &triple))
        .cloned();
    let derived = if roots.is_empty() {
        None
    } else {
        let direct: BTreeSet<String> = roots
            .into_iter()
            .filter(|root| !is_embeddable_stdlib_root(root) && !is_excluded_stdlib_root(root))
            .collect();
        // A standard-library-only program needs no lock (rule 7), so
        // `--check` accepts its absence without starting the interpreter.
        if check && direct.is_empty() && old_section.is_none() {
            None
        } else {
            Some(derive(&key, &triple, &direct, toolchain)?)
        }
    };
    let base = existing.unwrap_or(Lock {
        version: LOCK_VERSION,
        target: Vec::new(),
    });
    let sections = base.target.len();
    let pruned = schema::prune(base, lock_dir);
    let orphans = sections - pruned.target.len();
    let new = schema::replace_target(pruned, &key, &triple, derived);
    let rendered = (!new.target.is_empty()).then(|| schema::render(&new));
    if rendered == existing_text {
        return Ok(());
    }
    if check {
        let reason = match &existing_text {
            None => "it does not exist".to_string(),
            Some(_) => stale_reason(
                old_section.as_ref(),
                find_section(&new, &key, &triple),
                orphans,
            ),
        };
        return Err(LockFailure::Stale(format!(
            "`{}` is not current for `{key}` on `{triple}`: {reason}; run `pycc lock {}`",
            lock_path.display(),
            path.display()
        )));
    }
    match rendered {
        Some(text) => write_atomically(lock_dir, LOCK_FILE_NAME, &text),
        None => std::fs::remove_file(&lock_path)
            .map_err(|e| format!("cannot remove `{}`: {e}", lock_path.display())),
    }
    .map_err(LockFailure::Env)
}

/// The first segment of every CPython-backed import in the linked program.
fn import_roots(hir: &HirModule) -> BTreeSet<String> {
    hir.imports
        .iter()
        .filter_map(|binding| match binding {
            ImportBinding::Foreign { module_path, .. } => {
                module_path.split('.').next().map(str::to_string)
            }
            _ => None,
        })
        .collect()
}

/// The lock's `entry` for the entry's path relative to the lock
/// directory: its components joined by `/`.
fn entry_key(relative: &Path) -> Result<String, String> {
    let parts: Option<Vec<&str>> = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect();
    parts.map(|parts| parts.join("/")).ok_or_else(|| {
        format!(
            "the entry path `{}` is not valid UTF-8; `pycc.lock` records its entry as UTF-8 \
             text",
            relative.display()
        )
    })
}

fn read_existing(lock_path: &Path) -> Result<Option<String>, LockFailure> {
    match std::fs::read_to_string(lock_path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LockFailure::Env(format!(
            "cannot read `{}`: {e}",
            lock_path.display()
        ))),
    }
}

fn find_section<'a>(lock: &'a Lock, entry: &str, triple: &str) -> Option<&'a LockTarget> {
    lock.target
        .iter()
        .find(|target| target.entry == entry && target.triple == triple)
}

/// Derives the (entry, triple) section for a program with at least one
/// CPython-backed import; `direct` is empty for a standard-library-only
/// program, whose section carries only the interpreter fields.
fn derive(
    entry: &str,
    triple: &str,
    direct: &BTreeSet<String>,
    toolchain: &EmbedToolchain,
) -> Result<LockTarget, LockFailure> {
    let probe = toolchain.probe().map_err(LockFailure::Env)?;
    let env = probe::run_lock_probe(toolchain.interpreter()).map_err(LockFailure::Env)?;
    let library = embed::layout::source_library(&probe);
    let libpython_sha256 = embed::sha256::sha256_file(&library)
        .map_err(|e| LockFailure::Env(format!("cannot read `{}`: {e}", library.display())))?;
    let packages = if direct.is_empty() {
        Vec::new()
    } else {
        let sites = resolve::scanned_sites(&env.purelib, &env.platlib).map_err(LockFailure::Env)?;
        resolve::resolve(&sites, direct, &env.markers).map_err(LockFailure::Env)?
    };
    let (major, minor, micro) = probe.version;
    Ok(LockTarget {
        entry: entry.to_string(),
        triple: triple.to_string(),
        python: format!("{major}.{minor}.{micro}"),
        cache_tag: env.cache_tag,
        platform: env.platform,
        libpython_sha256,
        roots: direct.iter().cloned().collect(),
        package: packages
            .into_iter()
            .map(|package| LockedPackage {
                name: package.name,
                version: package.version,
                site: package.site.as_str().to_string(),
                files: package.files as u64,
                tree_sha256: package.tree_sha256,
                requires: package.requires,
            })
            .collect(),
        native: Vec::new(),
    })
}

/// Why `--check` found the lock stale, given the locked and the derived
/// (entry, triple) sections and how many orphaned sections `prune` drops.
fn stale_reason(old: Option<&LockTarget>, new: Option<&LockTarget>, orphans: usize) -> String {
    match (old, new) {
        (None, Some(_)) => "it has no section for this entry and host".to_string(),
        (Some(_), None) => {
            "its section for this entry is not needed: the program has no CPython-backed import"
                .to_string()
        }
        (Some(old), Some(new)) if old != new => first_difference(old, new),
        _ if orphans > 0 => {
            format!("{orphans} section(s) name an entry script that no longer exists")
        }
        _ => "it is not in the form `pycc lock` writes".to_string(),
    }
}

/// The first field in which a locked section differs from the derived one.
fn first_difference(old: &LockTarget, new: &LockTarget) -> String {
    let scalars = [
        ("python", &old.python, &new.python),
        ("cache-tag", &old.cache_tag, &new.cache_tag),
        ("platform", &old.platform, &new.platform),
        (
            "libpython-sha256",
            &old.libpython_sha256,
            &new.libpython_sha256,
        ),
    ];
    if let Some((field, locked, now)) = scalars.into_iter().find(|(_, a, b)| a != b) {
        return format!("`{field}` is `{locked}` in the lock but `{now}` in the environment");
    }
    if old.roots != new.roots {
        return format!(
            "`roots` is {:?} in the lock but {:?} for the program",
            old.roots, new.roots
        );
    }
    let names: BTreeSet<&str> = old
        .package
        .iter()
        .chain(&new.package)
        .map(|package| package.name.as_str())
        .collect();
    for name in names {
        let locked = old.package.iter().find(|package| package.name == name);
        let now = new.package.iter().find(|package| package.name == name);
        match (locked, now) {
            (None, _) => return format!("package `{name}` is in the closure but not locked"),
            (_, None) => return format!("package `{name}` is locked but not in the closure"),
            (Some(a), Some(b)) if a != b => {
                return format!("package `{name}` differs from the installed distribution");
            }
            _ => {}
        }
    }
    "its `[[target.native]]` entries differ".to_string()
}

/// Writes `text` to `dir/name` through `name.tmp-<pid>` and a rename,
/// removing the temporary file on any failure.
fn write_atomically(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let tmp = dir.join(format!("{name}.tmp-{}", std::process::id()));
    let target = dir.join(name);
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &target))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write `{}`: {e}", target.display())
        })
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod command_tests;
