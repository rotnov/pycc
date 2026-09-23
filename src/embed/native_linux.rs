//! The Linux dependency scan of an embedded build (#1243, lifting the
//! embedded-executable decision entry's NEG-002): every `DT_NEEDED` of the
//! interpreter's images and the locked closure's images is resolved the
//! way `ld.so` would on the build host, then kept, vendored from the
//! prefix, recorded as a native, or refused.
//!
//! Host-independent by construction: the system directories and the
//! `ldconfig` program come from a [`LinuxEnv`] the tests inject, and every
//! image is read through `elf.rs`, so the macOS coverage host drives every
//! line over synthetic ELF files.

use super::EmbedProbe;
use super::bundle::io_error;
use super::elf::{self, ElfImage};
use super::layout::{self, EmbedPlatform};
use super::native::{NativePlan, Natives, read_head, resolved};
use crate::lock::build::LockedClosure;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the scan looks for libraries besides an image's own search path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinuxEnv {
    /// The system library directories: a library resolved under one is
    /// kept, and these are also the loader's default search directories.
    pub(crate) system_dirs: Vec<PathBuf>,
    /// The `ldconfig` programs to try, in order, for the loader cache
    /// (`ldconfig -p`); the first that exits successfully answers.
    pub(crate) ldconfig_programs: Vec<PathBuf>,
}

impl LinuxEnv {
    /// The build host's: glibc's default directories plus the Debian
    /// multiarch ones for the two Tier-1 Linux architectures, and
    /// `/sbin/ldconfig`, else `ldconfig` on `PATH`.
    pub(crate) fn host() -> Self {
        let mut system_dirs: Vec<PathBuf> = ["/lib", "/lib64", "/usr/lib", "/usr/lib64"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        for arch in ["x86_64-linux-gnu", "aarch64-linux-gnu"] {
            for base in ["/lib", "/usr/lib"] {
                system_dirs.push(Path::new(base).join(arch));
            }
        }
        Self {
            system_dirs,
            ldconfig_programs: vec![PathBuf::from("/sbin/ldconfig"), PathBuf::from("ldconfig")],
        }
    }
}

/// The loader cache from the first `programs` entry that runs
/// successfully, or an empty one.
fn run_ldconfig(programs: &[PathBuf]) -> BTreeMap<String, Vec<PathBuf>> {
    for program in programs {
        let output = Command::new(program).arg("-p").output().ok();
        if let Some(output) = output.filter(|output| output.status.success()) {
            return elf::parse_ldconfig(&String::from_utf8_lossy(&output.stdout));
        }
    }
    BTreeMap::new()
}

/// What an image is, which decides what its dependencies may be.
#[derive(Debug, Clone)]
enum Kind {
    /// libpython, a `lib-dynload` extension, or a library vendored from
    /// the prefix, by its path under `<sidecar>/lib`: the interpreter's
    /// rule (the embedded-executable decision entry, rule 5).
    Interpreter(String),
    /// A closure image, by its path under `<sidecar>/closure`.
    Closure(String),
    /// A native library, by its name in `<sidecar>/lib`.
    Native(String),
}

/// One image to scan: the path it was found at (which `$ORIGIN` expands
/// from), what it is, and the distribution it is scanned for.
struct Node {
    path: PathBuf,
    kind: Kind,
    owner: Option<String>,
}

impl Node {
    fn describe(&self) -> String {
        let owner = self.owner.as_deref().unwrap_or_default();
        match &self.kind {
            Kind::Interpreter(rel) => format!("`{rel}`"),
            Kind::Closure(rel) => format!("`closure/{rel}` (distribution `{owner}`)"),
            Kind::Native(name) => {
                format!("the native library `lib/{name}` (for distribution `{owner}`)")
            }
        }
    }
}

/// Where one dependency lands.
enum LinuxDep {
    Keep,
    Vendor,
    Native,
}

struct Walk<'a> {
    env: &'a LinuxEnv,
    executable: &'a Path,
    system: Vec<PathBuf>,
    prefix: PathBuf,
    bundled_name: String,
    source_library: PathBuf,
    /// The resolved sources of the closure's ELF images.
    payload: BTreeSet<PathBuf>,
    ldconfig: Option<BTreeMap<String, Vec<PathBuf>>>,
    natives: Natives,
    /// Every library copied into `lib/`, by the name it is needed by.
    vendor: BTreeMap<String, PathBuf>,
    /// Every dependency kept on the system, by its `DT_NEEDED` name: the
    /// first image that needs it and where it resolved.
    kept: BTreeMap<String, (String, PathBuf)>,
    visited: BTreeSet<(PathBuf, Option<String>)>,
    pending: Vec<Node>,
}

/// Scans the closure's images (and, with `interpreter`, libpython and the
/// `lib-dynload` extensions the copy keeps) and returns what the build
/// copies into `lib/`.
pub(crate) fn plan(
    probe: &EmbedProbe,
    closure: Option<&LockedClosure>,
    env: &LinuxEnv,
    interpreter: bool,
) -> Result<NativePlan, String> {
    let bundled_name = layout::bundled_library_name(EmbedPlatform::Linux, probe);
    let source_library = layout::source_library(probe);
    let mut walk = Walk {
        env,
        executable: &probe.executable,
        system: env.system_dirs.iter().map(|dir| resolved(dir)).collect(),
        prefix: resolved(&probe.base_prefix),
        bundled_name: bundled_name.clone(),
        source_library: resolved(&source_library),
        payload: BTreeSet::new(),
        ldconfig: None,
        natives: Natives::new(bundled_name.clone()),
        vendor: BTreeMap::new(),
        kept: BTreeMap::new(),
        visited: BTreeSet::new(),
        pending: Vec::new(),
    };
    if interpreter {
        walk.pending.push(Node {
            path: source_library,
            kind: Kind::Interpreter(bundled_name),
            owner: None,
        });
        let dynload = probe.stdlib.join("lib-dynload");
        // An interpreter without a `lib-dynload` directory contributes no
        // extension images, so a failed listing is an empty one.
        let mut names: Vec<String> = std::fs::read_dir(&dynload)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !layout::skip_in_stdlib_copy(&Path::new("lib-dynload").join(name)))
            .collect();
        names.sort();
        let stdlib = layout::stdlib_dir_name(probe);
        for name in names {
            walk.pending.push(Node {
                path: dynload.join(&name),
                kind: Kind::Interpreter(format!("{stdlib}/lib-dynload/{name}")),
                owner: None,
            });
        }
    }
    for file in closure.map_or(&[][..], |closure| closure.files.as_slice()) {
        if elf::is_elf_magic(&read_head(&file.source)?) {
            walk.payload.insert(resolved(&file.source));
            walk.pending.push(Node {
                path: file.source.clone(),
                kind: Kind::Closure(file.rel.clone()),
                owner: Some(file.package.clone()),
            });
        }
    }
    // Depth-first from the end, so reverse to scan in the order pushed.
    walk.pending.reverse();
    while let Some(node) = walk.pending.pop() {
        walk.visit(&node)?;
    }
    walk.check_shadowing()?;
    Ok(NativePlan {
        natives: walk.natives.finish(),
        linux_vendor: walk.vendor.into_iter().collect(),
    })
}

impl Walk<'_> {
    fn visit(&mut self, node: &Node) -> Result<(), String> {
        let bytes = std::fs::read(&node.path).map_err(|e| io_error("read", &node.path, &e))?;
        let image = elf::parse_elf(&bytes)
            .map_err(|e| format!("cannot scan `{}`: {e}", node.path.display()))?;
        let Some(image) = image else {
            return Ok(());
        };
        for needed in &image.needed {
            let Some((found, via_origin, dep)) = self.resolve(needed, &image, &node.path) else {
                // Left to the loader, which fails on the target exactly as
                // it would here.
                continue;
            };
            let canonical = resolved(&found);
            match self.classify(needed, &canonical, via_origin, node)? {
                LinuxDep::Keep => {
                    let first = (node.describe(), canonical);
                    self.kept.entry(needed.clone()).or_insert(first);
                }
                class => self.copy(node, &image, needed, &canonical, &dep, class)?,
            }
        }
        Ok(())
    }

    /// Resolves one `DT_NEEDED` in `ld.so`'s order: `DT_RPATH` (only
    /// without `DT_RUNPATH`), `DT_RUNPATH`, the loader cache, then the
    /// default directories. Returns the path it was found at, whether that
    /// came through `$ORIGIN`, and the library's own dynamic section.
    fn resolve(
        &mut self,
        needed: &str,
        image: &ElfImage,
        found_at: &Path,
    ) -> Option<(PathBuf, bool, ElfImage)> {
        let machine = image.machine;
        if needed.contains('/') {
            let path = PathBuf::from(needed);
            let dep = path.is_absolute().then(|| candidate(&path, machine))??;
            return Some((path, false, dep));
        }
        let origin = found_at.parent().unwrap_or(Path::new("/"));
        let search = image.runpath.as_ref().or(image.rpath.as_ref());
        let dirs = search.map(|value| elf::search_dirs(value, origin));
        for (dir, via_origin) in dirs.unwrap_or_default() {
            let path = dir.join(needed);
            if let Some(dep) = candidate(&path, machine) {
                return Some((path, via_origin, dep));
            }
        }
        let programs = &self.env.ldconfig_programs;
        let cache = self.ldconfig.get_or_insert_with(|| run_ldconfig(programs));
        let cached = cache.get(needed).cloned().unwrap_or_default();
        let defaults = self.env.system_dirs.iter().map(|dir| dir.join(needed));
        for path in cached.into_iter().chain(defaults) {
            if let Some(dep) = candidate(&path, machine) {
                return Some((path, false, dep));
            }
        }
        None
    }

    /// Where the dependency `needed`, resolved to `canonical`, lands.
    fn classify(
        &self,
        needed: &str,
        canonical: &Path,
        via_origin: bool,
        node: &Node,
    ) -> Result<LinuxDep, String> {
        if needed == self.bundled_name {
            return Ok(LinuxDep::Keep);
        }
        if canonical == self.source_library {
            return Err(format!(
                "{} needs `{needed}`, which is the interpreter's own library under a name the \
                 bundle does not provide (it provides `{}`); pycc cannot bundle it",
                node.describe(),
                self.bundled_name
            ));
        }
        if self.system.iter().any(|dir| canonical.starts_with(dir)) {
            return Ok(LinuxDep::Keep);
        }
        let closure_image = matches!(node.kind, Kind::Closure(_));
        if closure_image && via_origin && self.payload.contains(canonical) {
            return Ok(LinuxDep::Keep);
        }
        if canonical.starts_with(&self.prefix) {
            return Ok(LinuxDep::Vendor);
        }
        match node.kind {
            Kind::Interpreter(_) => Err(format!(
                "the embed interpreter `{}` is not relocatable: {} depends on `{needed}` \
                 (`{}`) outside the interpreter and the system library directories; pycc \
                 cannot bundle it",
                self.executable.display(),
                node.describe(),
                canonical.display()
            )),
            _ => Ok(LinuxDep::Native),
        }
    }

    /// Plans the copy of `needed` (resolved to `canonical`, with its own
    /// dynamic section `dep`) into `lib/` and queues it for scanning.
    fn copy(
        &mut self,
        node: &Node,
        image: &ElfImage,
        needed: &str,
        canonical: &Path,
        dep: &ElfImage,
        class: LinuxDep,
    ) -> Result<(), String> {
        if image.program && matches!(node.kind, Kind::Closure(_)) {
            return Err(format!(
                "the locked closure is not relocatable: {} is a program that needs `{needed}` \
                 (`{}`), which pycc copies into the bundle's `lib/`, where only the embedded \
                 executable's own process loads it from",
                node.describe(),
                canonical.display()
            ));
        }
        if dep.soname.as_deref() != Some(needed) {
            let soname = dep.soname.as_deref().map_or_else(
                || "missing".to_string(),
                |soname| format!("`{soname}`"),
            );
            return Err(format!(
                "{} needs `{needed}`, found at `{}`, whose `DT_SONAME` is {soname}; pycc \
                 copies it into the bundle's `lib/` as `{needed}` and the loader matches it \
                 by its `DT_SONAME`, so the two must agree",
                node.describe(),
                canonical.display()
            ));
        }
        let (kind, owner) = match class {
            LinuxDep::Native => {
                let owner = node.owner.clone().unwrap_or_default();
                if !self.natives.add(canonical, needed, &owner)? {
                    return Ok(());
                }
                (Kind::Native(needed.to_string()), Some(owner))
            }
            _ => {
                self.natives.claim_name(needed, canonical)?;
                (Kind::Interpreter(needed.to_string()), None)
            }
        };
        self.vendor.insert(needed.to_string(), canonical.to_path_buf());
        if self.visited.insert((canonical.to_path_buf(), owner.clone())) {
            self.pending.push(Node {
                path: canonical.to_path_buf(),
                kind,
                owner,
            });
        }
        Ok(())
    }

    /// Refuses a vendored library whose name is also the `DT_NEEDED` name
    /// of a dependency kept on the system: the executable loads the copy at
    /// start-up, and glibc would then satisfy the kept dependency from it.
    fn check_shadowing(&self) -> Result<(), String> {
        for (name, (requester, kept)) in &self.kept {
            let Some(source) = self.vendor.get(name).filter(|source| *source != kept) else {
                continue;
            };
            return Err(format!(
                "the bundle copies `{name}` from `{}` into its `lib/` and loads it at \
                 start-up, so it would also answer {requester}'s dependency on `{}`; pycc \
                 cannot bundle both",
                source.display(),
                kept.display()
            ));
        }
        Ok(())
    }
}

/// The dynamic section of `path` when it is an ELF64 little-endian image
/// for `machine`, as the loader would accept it.
fn candidate(path: &Path, machine: u16) -> Option<ElfImage> {
    let bytes = std::fs::read(path).ok()?;
    let image = elf::parse_elf(&bytes).ok()??;
    (image.machine == machine).then_some(image)
}

#[cfg(test)]
#[path = "native_linux_tests.rs"]
mod tests;
