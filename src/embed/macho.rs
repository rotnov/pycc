//! Pure Mach-O relocation rules for an embedded executable's sidecar on
//! macOS (§3.4 of the #1028 plan): parsing `otool -L`, classifying each
//! dependency, and the `install_name_tool`/`codesign` argument lists.
//! `bundle.rs` runs the tools; nothing here spawns anything.

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
    /// Anything else. The build stops rather than ship a bundle that only
    /// runs on the build host (D-128 rule 1).
    Refuse,
}

/// The install names in `otool -L`'s output, in order. The first line is
/// the image's own path and is skipped; for a dylib the first entry is its
/// own id.
pub(crate) fn parse_otool_l(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            let name = line
                .rsplit_once(" (compatibility version")
                .map_or(line, |(name, _)| name)
                .trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
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
    if dep.starts_with("/usr/lib/") || dep.starts_with("/System/Library/") {
        return MachoDep::Keep;
    }
    if own_id == Some(dep) || dep == format!("@rpath/{bundled_name}") {
        return MachoDep::Keep;
    }
    if bundle_lib
        .iter()
        .any(|lib| lib.as_path() == Path::new(dep) || lib.as_path() == resolved)
    {
        return MachoDep::RewriteToBundled;
    }
    if resolved.starts_with(prefix) {
        return MachoDep::Vendor(resolved.to_path_buf());
    }
    MachoDep::Refuse
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
