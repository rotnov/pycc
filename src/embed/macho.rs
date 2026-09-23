//! Pure Mach-O relocation rules for an embedded executable's sidecar on
//! macOS (§3.4 of the #1028 plan): parsing `otool -L`, classifying each
//! dependency, and the `install_name_tool`/`codesign` argument lists.
//! `bundle.rs` runs the tools; nothing here spawns anything.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What the bundle does with one dependency of one copied image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MachoDep {
    /// A system library, the bundled libpython by its `@rpath` name, or the
    /// image's own id: left as it is.
    Keep,
    /// The source interpreter's own libpython, by its original id or its
    /// resolved path: rewritten to the bundled `@rpath` name and never
    /// vendored, since a second copy would load a second interpreter.
    RewriteToBundled,
    /// A library inside the interpreter's prefix: copied into the sidecar
    /// and referenced relative to the loading image. Carries the resolved
    /// source path.
    Vendor(PathBuf),
    /// A native library (the pycc.lock decision entry, rule 8; #1243): an
    /// absolute dependency of a closure image, or of another native, that
    /// lies outside the system directories and the interpreter's prefix.
    /// Copied into the sidecar, like [`MachoDep::Vendor`], but only when
    /// the lock lists it. Carries the resolved source path.
    VendorNative(PathBuf),
    /// Anything else. The build stops rather than ship a bundle that only
    /// runs on the build host (D-128 rule 1).
    Refuse,
}

/// The install names in `otool -L`'s output, in order. The first line is
/// the image's own path and is skipped, as is every per-architecture header
/// of a universal image; for a dylib the first entry is its own id. A
/// universal image lists its names once per slice, and each name is kept
/// once, so a later `install_name_tool -change` never runs twice.
pub(crate) fn parse_otool_l(stdout: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let all = stdout.lines().skip(1).filter_map(|line| {
        let line = line.trim();
        // A universal image repeats a `<path> (architecture <arch>):`
        // header per slice; it names the image, not a dependency.
        let name = line
            .rsplit_once(" (compatibility version")
            .map_or(line, |(name, _)| name)
            .trim();
        (!name.is_empty() && !line.ends_with("):")).then(|| name.to_string())
    });
    for name in all {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Classifies one dependency `dep` of an image whose own id is `own_id`.
///
/// `resolved` is `dep` with symlinks resolved (or `dep` itself when it does
/// not resolve), computed by the caller so this stays pure. `prefix` is the
/// interpreter's resolved `sys.base_prefix`; `bundle_lib` holds the source
/// libpython's original id and resolved path; `bundled_name` is its file
/// name inside the sidecar. The bundled check runs before the prefix check
/// because the source libpython also lies under the prefix.
pub(crate) fn classify_macho_dep(
    dep: &str,
    resolved: &Path,
    own_id: Option<&str>,
    prefix: &Path,
    bundle_lib: &[PathBuf],
    bundled_name: &str,
) -> MachoDep {
    if is_system(dep) || own_id == Some(dep) || dep == format!("@rpath/{bundled_name}") {
        return MachoDep::Keep;
    }
    if is_bundled(dep, resolved, bundle_lib) {
        return MachoDep::RewriteToBundled;
    }
    if resolved.starts_with(prefix) {
        return MachoDep::Vendor(resolved.to_path_buf());
    }
    MachoDep::Refuse
}

fn is_system(dep: &str) -> bool {
    dep.starts_with("/usr/lib/") || dep.starts_with("/System/Library/")
}

fn is_bundled(dep: &str, resolved: &Path, bundle_lib: &[PathBuf]) -> bool {
    bundle_lib
        .iter()
        .any(|lib| lib.as_path() == Path::new(dep) || lib.as_path() == resolved)
}

/// Classifies an absolute dependency `dep` of a closure image or a native
/// library (#1243), in precedence order: a system library is kept; the
/// source libpython is rewritten to the bundled one; a library under the
/// interpreter's `prefix` is vendored; anything else is a native. The
/// caller passes only absolute dependencies, or one that names the bundled
/// library by a relative id. `resolved`, `prefix` and `bundle_lib` are as
/// for [`classify_macho_dep`].
pub(crate) fn classify_absolute(
    dep: &str,
    resolved: &Path,
    prefix: &Path,
    bundle_lib: &[PathBuf],
) -> MachoDep {
    if is_system(dep) {
        return MachoDep::Keep;
    }
    if is_bundled(dep, resolved, bundle_lib) {
        return MachoDep::RewriteToBundled;
    }
    if resolved.starts_with(prefix) {
        return MachoDep::Vendor(resolved.to_path_buf());
    }
    MachoDep::VendorNative(resolved.to_path_buf())
}

/// Classifies one dependency of a vendored native library whose own id is
/// `own_id`: the id is kept, an absolute dependency (or the bundled library
/// by any id) goes through [`classify_absolute`], so a whole chain of
/// natives is vendored, and a relative reference is refused (#1259).
pub(crate) fn classify_native_dep(
    dep: &str,
    resolved: &Path,
    own_id: &str,
    prefix: &Path,
    bundle_lib: &[PathBuf],
) -> MachoDep {
    if dep == own_id {
        return MachoDep::Keep;
    }
    if dep.starts_with('/') || is_bundled(dep, resolved, bundle_lib) {
        return classify_absolute(dep, resolved, prefix, bundle_lib);
    }
    MachoDep::Refuse
}

/// Whether `bytes` starts like a Mach-O image: a thin header in either
/// byte order, or a fat (universal) header whose big-endian `nfat_arch` is
/// between 1 and 29. The bound keeps a Java `.class` file, which shares
/// `0xcafebabe` but carries its version (45 or more) in that field, out of
/// the relocation scan.
pub(crate) fn is_macho_header(bytes: &[u8]) -> bool {
    let Some(magic) = bytes.get(..4) else {
        return false;
    };
    let magic = u32::from_be_bytes([magic[0], magic[1], magic[2], magic[3]]);
    match magic {
        0xfeed_face | 0xfeed_facf | 0xcefa_edfe | 0xcffa_edfe => true,
        0xcafe_babe | 0xcafe_babf | 0xbeba_feca | 0xbfba_feca => {
            let Some(count) = bytes.get(4..8) else {
                return false;
            };
            let count = [count[0], count[1], count[2], count[3]];
            let count = if magic >> 24 == 0xca {
                u32::from_be_bytes(count)
            } else {
                u32::from_le_bytes(count)
            };
            (1..=29).contains(&count)
        }
        _ => false,
    }
}

/// The install name `otool -D` reports for an image, if it has one: a
/// bundle prints only its path header, a dylib its id after it, and a
/// universal image one header and id per slice.
pub(crate) fn parse_otool_d(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.ends_with(':'))
        .map(str::to_string)
}

/// The `LC_RPATH` entries in `otool -l`'s output, in load-command order,
/// each kept once (a universal image lists them per slice).
pub(crate) fn parse_otool_rpaths(stdout: &str) -> Vec<String> {
    let mut rpaths: Vec<String> = Vec::new();
    let mut in_rpath = false;
    for line in stdout.lines().map(str::trim) {
        if let Some(cmd) = line.strip_prefix("cmd ") {
            in_rpath = cmd.trim() == "LC_RPATH";
            continue;
        }
        let Some(path) = line.strip_prefix("path ").filter(|_| in_rpath) else {
            continue;
        };
        let path = path.rsplit_once(" (offset").map_or(path, |(path, _)| path);
        let path = path.trim().to_string();
        if !rpaths.contains(&path) {
            rpaths.push(path);
        }
        in_rpath = false;
    }
    rpaths
}

/// What [`classify_closure_dep`] knows about one closure image, all of it
/// computed by the caller so the classifier stays pure.
pub(crate) struct ClosureImage<'a> {
    /// The image's path under `<sidecar>/closure`, `/`-separated.
    pub(crate) rel: &'a str,
    /// Its install name (`otool -D`), if it is a dylib.
    pub(crate) own_id: Option<&'a str>,
    /// Its `LC_RPATH` entries, in load-command order.
    pub(crate) rpaths: &'a [String],
    /// The payload paths (under `closure/`) of the distribution(s) that
    /// own it.
    pub(crate) own_payload: &'a BTreeSet<String>,
    /// Every file in the staged sidecar, relative to its root
    /// (`closure/...` and `lib/...`).
    pub(crate) sidecar: &'a BTreeSet<String>,
}

/// Classifies one dependency `dep` of a closure image (the pycc.lock
/// decision entry, rule 8), in precedence order: the image's own id and a
/// system library are kept; the source libpython is rewritten to the
/// bundled one; an `@loader_path` reference into the image's own payload
/// is kept; an `@rpath` reference is resolved in dyld's order over the
/// image's `LC_RPATH` entries and kept only when its first match is in the
/// image's own payload; any other absolute dependency is vendored, from
/// the prefix or as a native, per [`classify_absolute`] (#1243); a relative
/// reference that misses the payload is refused (#1259). `resolved`,
/// `prefix` and `bundle_lib` are as for [`classify_macho_dep`].
pub(crate) fn classify_closure_dep(
    dep: &str,
    resolved: &Path,
    image: &ClosureImage<'_>,
    prefix: &Path,
    bundle_lib: &[PathBuf],
) -> MachoDep {
    if image.own_id == Some(dep) {
        return MachoDep::Keep;
    }
    if dep.starts_with('/') || is_bundled(dep, resolved, bundle_lib) {
        return classify_absolute(dep, resolved, prefix, bundle_lib);
    }
    let image_dir = format!("closure/{}/..", image.rel);
    if let Some(rest) = dep.strip_prefix("@loader_path/") {
        let target = normalize_under_root(&format!("{image_dir}/{rest}"));
        return match target.as_deref().and_then(|t| t.strip_prefix("closure/")) {
            Some(rel) if image.own_payload.contains(rel) => MachoDep::Keep,
            _ => MachoDep::Refuse,
        };
    }
    if let Some(rest) = dep.strip_prefix("@rpath/") {
        return resolve_rpath(rest, &image_dir, image);
    }
    MachoDep::Refuse
}

/// dyld's `@rpath/<rest>` search over `image.rpaths`: the first candidate
/// that is not relative to the image (an absolute or `@executable_path`
/// rpath, or one that climbs out of the sidecar) is refused, because on the
/// machine that runs the program it may exist and win; the first relative
/// candidate that names a sidecar file decides.
fn resolve_rpath(rest: &str, image_dir: &str, image: &ClosureImage<'_>) -> MachoDep {
    for rpath in image.rpaths {
        let Some(relative) = rpath
            .strip_prefix("@loader_path")
            .filter(|tail| tail.is_empty() || tail.starts_with('/'))
        else {
            return MachoDep::Refuse;
        };
        let Some(candidate) = normalize_under_root(&format!("{image_dir}{relative}/{rest}")) else {
            return MachoDep::Refuse;
        };
        if let Some(rel) = candidate.strip_prefix("closure/")
            && image.own_payload.contains(rel)
        {
            return MachoDep::Keep;
        }
        if image.sidecar.contains(&candidate) {
            return MachoDep::Refuse;
        }
    }
    MachoDep::Refuse
}

/// Normalizes a `/`-separated path relative to the sidecar root, or `None`
/// when it climbs above that root.
fn normalize_under_root(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// `install_name_tool -id <id> <image>`.
pub(crate) fn set_id_args(id: &str, image: &Path) -> Vec<OsString> {
    vec!["-id".into(), id.into(), image.as_os_str().to_os_string()]
}

/// `install_name_tool -change <old> <new> <image>`.
pub(crate) fn change_args(old: &str, new: &str, image: &Path) -> Vec<OsString> {
    vec![
        "-change".into(),
        old.into(),
        new.into(),
        image.as_os_str().to_os_string(),
    ]
}

/// `codesign -f -s - <image>`: an ad-hoc signature, which every rewritten
/// image needs because `install_name_tool` invalidates the old one.
pub(crate) fn codesign_args(image: &Path) -> Vec<OsString> {
    vec![
        "-f".into(),
        "-s".into(),
        "-".into(),
        image.as_os_str().to_os_string(),
    ]
}

#[cfg(test)]
#[path = "macho_tests.rs"]
mod tests;
