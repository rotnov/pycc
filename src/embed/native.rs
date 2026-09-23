//! The native libraries a locked closure needs (the pycc.lock decision
//! entry, rule 8; #1243): libraries outside the system directories, the
//! interpreter's prefix and the bundled libpython that a closure image
//! depends on, directly or through another native. One derivation serves
//! both consumers: `pycc lock` records its result as `[[target.native]]`,
//! and an embedded build re-derives it, compares it with the section, and
//! copies what it lists into `OUT.pycc/lib/`.
//!
//! The derivation reads the closure's *source* images, the files under the
//! site directory that `lock::build::payload` lists. On macOS it follows
//! absolute install names only; relative references are left to the
//! build's relocation, which refuses those that miss the payload (#1259).
//! On Linux it is the ELF walk in `native_linux.rs`.

use super::EmbedProbe;
use super::bundle::{io_error, run_tool};
use super::layout::{self, EmbedPlatform};
use super::macho::{self, MachoDep};
use super::native_linux::{self, LinuxEnv};
use super::sha256::sha256_file;
use crate::lock::build::{ClosureFile, LockedClosure};
use crate::lock::schema::LockedNative;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One native library: where it is read from, and its lock entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DerivedNative {
    /// The resolved source path.
    pub(crate) source: PathBuf,
    pub(crate) locked: LockedNative,
}

/// What an embedded build copies into `OUT.pycc/lib/` besides libpython
/// and the standard library.
#[derive(Debug, Default)]
pub(crate) struct NativePlan {
    /// The natives, sorted by name.
    pub(crate) natives: Vec<DerivedNative>,
    /// Linux only: every library the build copies into `lib/` and links
    /// into the executable, prefix-vendored or native, as (the name it is
    /// needed by, its resolved source), sorted by name. macOS vendors
    /// during the relocation instead.
    pub(crate) linux_vendor: Vec<(String, PathBuf)>,
}

impl NativePlan {
    /// The `[[target.native]]` entries, sorted by name.
    pub(crate) fn locked(&self) -> Vec<LockedNative> {
        self.natives
            .iter()
            .map(|native| native.locked.clone())
            .collect()
    }

    /// The lock entry of the native read from `source`, if it is one.
    pub(crate) fn native_at(&self, source: &Path) -> Option<&LockedNative> {
        let native = self.natives.iter().find(|native| native.source == source);
        native.map(|native| &native.locked)
    }
}

/// Derives the natives of `closure` for `platform`. `interpreter` also
/// walks the interpreter's own images on Linux (libpython and
/// `lib-dynload`), which the build needs for their prefix-vendored
/// libraries and refusals; `pycc lock` walks the closure alone, as on
/// macOS, where the relocation walks the interpreter's images.
pub(crate) fn plan_natives(
    platform: EmbedPlatform,
    probe: &EmbedProbe,
    closure: Option<&LockedClosure>,
    env: &LinuxEnv,
    interpreter: bool,
) -> Result<NativePlan, String> {
    match platform {
        EmbedPlatform::MacOs => Ok(NativePlan {
            natives: match closure {
                Some(closure) => derive_macos(probe, closure)?,
                None => Vec::new(),
            },
            linux_vendor: Vec::new(),
        }),
        EmbedPlatform::Linux => native_linux::plan(probe, closure, env, interpreter),
    }
}

/// The natives found so far, and the names they and every other vendored
/// library claim in `lib/`.
pub(crate) struct Natives {
    bundled_name: String,
    /// Source path to (name, sha256, the distributions that need it).
    by_source: BTreeMap<PathBuf, (String, String, BTreeSet<String>)>,
    /// Case-folded name to (name, source).
    names: BTreeMap<String, (String, PathBuf)>,
}

impl Natives {
    pub(crate) fn new(bundled_name: String) -> Self {
        Self {
            bundled_name,
            by_source: BTreeMap::new(),
            names: BTreeMap::new(),
        }
    }

    /// Claims `name` in `lib/` for the library read from `source`, and
    /// returns whether that library already claimed it. Refused when it is
    /// the bundled libpython's name, or when a different source already
    /// claims it; names are compared case-folded, because the macOS default
    /// file system does.
    pub(crate) fn claim_name(&mut self, name: &str, source: &Path) -> Result<bool, String> {
        if name.eq_ignore_ascii_case(&self.bundled_name) {
            return Err(format!(
                "the library `{}` needs the name `{name}` in the bundle's `lib/`, which the \
                 bundled libpython already has; pycc cannot bundle both",
                source.display()
            ));
        }
        let folded = name.to_ascii_lowercase();
        match self.names.get(&folded) {
            Some((other, other_source)) if other_source != source => Err(format!(
                "the libraries `{}` and `{}` both need the name `{name}` in the bundle's \
                 `lib/` (as `{other}` and `{name}`); pycc cannot bundle both",
                other_source.display(),
                source.display()
            )),
            Some(_) => Ok(true),
            None => {
                let entry = (name.to_string(), source.to_path_buf());
                self.names.insert(folded, entry);
                Ok(false)
            }
        }
    }

    /// Records the native read from `source`, named `name`, as needed by
    /// the distribution `owner`. Returns whether the pair is new, so the
    /// caller walks each native once per distribution. The source is
    /// hashed when it is first seen, so a missing or unreadable one is
    /// refused by name.
    pub(crate) fn add(&mut self, source: &Path, name: &str, owner: &str) -> Result<bool, String> {
        if let Some((_, _, owners)) = self.by_source.get_mut(source) {
            return Ok(owners.insert(owner.to_string()));
        }
        self.claim_name(name, source)?;
        let sha256 = sha256_file(source).map_err(|e| {
            format!(
                "the locked closure needs the native library `{}` (for distribution \
                 `{owner}`), which cannot be read: {e}",
                source.display()
            )
        })?;
        let owners = BTreeSet::from([owner.to_string()]);
        let entry = (name.to_string(), sha256, owners);
        self.by_source.insert(source.to_path_buf(), entry);
        Ok(true)
    }

    /// The natives, sorted by name.
    pub(crate) fn finish(self) -> Vec<DerivedNative> {
        let mut natives: Vec<DerivedNative> = self
            .by_source
            .into_iter()
            .map(|(source, (name, sha256, owners))| DerivedNative {
                source,
                locked: LockedNative {
                    name,
                    sha256,
                    required_by: owners.into_iter().collect(),
                },
            })
            .collect();
        natives.sort_by(|a, b| a.locked.name.cmp(&b.locked.name));
        natives
    }
}

/// The first bytes of `path`, enough to recognize an image's header.
pub(crate) fn read_head(path: &Path) -> Result<Vec<u8>, String> {
    let mut head = Vec::with_capacity(8);
    let file = std::fs::File::open(path).map_err(|e| io_error("read", path, &e))?;
    file.take(8)
        .read_to_end(&mut head)
        .map_err(|e| io_error("read", path, &e))?;
    Ok(head)
}

/// `path` with symlinks resolved, or `path` itself when it does not
/// resolve.
pub(crate) fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The names a dependency can reach the source libpython by: the original
/// id of `library` (the source or its bundled copy, whose bytes are the
/// same) and the source's resolved path. Shared by the derivation and the
/// relocation, so the two never disagree about which dependency is the
/// bundled library.
pub(crate) fn macho_bundle_lib(library: &Path, source: &Path) -> Result<[PathBuf; 2], String> {
    let listing = run_tool("otool", &[OsString::from("-L"), library.into()])?;
    let original_id = macho::parse_otool_l(&listing)
        .into_iter()
        .next()
        .unwrap_or_default();
    Ok([PathBuf::from(original_id), resolved(source)])
}

/// The interpreter's resolved `sys.base_prefix`.
pub(crate) fn canonical_prefix(probe: &EmbedProbe) -> PathBuf {
    resolved(&probe.base_prefix)
}

/// The macOS derivation: every Mach-O image in the closure, and every
/// native it reaches through absolute install names.
fn derive_macos(probe: &EmbedProbe, closure: &LockedClosure) -> Result<Vec<DerivedNative>, String> {
    let mut images: Vec<&ClosureFile> = Vec::new();
    for file in &closure.files {
        if macho::is_macho_header(&read_head(&file.source)?) {
            images.push(file);
        }
    }
    if images.is_empty() {
        return Ok(Vec::new());
    }
    let source = layout::source_library(probe);
    let bundle_lib = macho_bundle_lib(&source, &source)?;
    let prefix = canonical_prefix(probe);
    let bundled_name = layout::bundled_library_name(EmbedPlatform::MacOs, probe);
    let mut natives = Natives::new(bundled_name);
    let mut pending: Vec<(PathBuf, String)> = Vec::new();
    for file in images {
        for native in absolute_natives(&file.source, &prefix, &bundle_lib)? {
            pending.push((native, file.package.clone()));
        }
    }
    while let Some((source, owner)) = pending.pop() {
        let name = file_name(&source);
        if natives.add(&source, &name, &owner)? {
            for native in absolute_natives(&source, &prefix, &bundle_lib)? {
                pending.push((native, owner.clone()));
            }
        }
    }
    Ok(natives.finish())
}

/// The natives among `image`'s absolute dependencies, its own id aside.
fn absolute_natives(
    image: &Path,
    prefix: &Path,
    bundle_lib: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let id_listing = run_tool("otool", &[OsString::from("-D"), image.into()])?;
    let own_id = macho::parse_otool_d(&id_listing);
    let listing = run_tool("otool", &[OsString::from("-L"), image.into()])?;
    let natives = macho::parse_otool_l(&listing)
        .into_iter()
        .filter(|dep| dep.starts_with('/') && own_id.as_deref() != Some(dep.as_str()))
        .filter_map(|dep| {
            let path = resolved(Path::new(&dep));
            match macho::classify_absolute(&dep, &path, prefix, bundle_lib) {
                MachoDep::VendorNative(source) => Some(source),
                _ => None,
            }
        })
        .collect();
    Ok(natives)
}

/// A library's file name, which is its name in `lib/` on macOS.
pub(crate) fn file_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default();
    name.to_string_lossy().into_owned()
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
