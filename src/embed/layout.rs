//! Pure layout rules for an embedded executable's sidecar (Part 1 of
//! #1028): where the sidecar lives, what it is called, how the executable
//! finds its library, what the marker says, and which standard-library
//! files are copied. The embedded-executable decision entry under
//! `docs/decisions/` owns the layout; this module only computes it.

use super::EmbedProbe;
use super::stdlib_roots::EXCLUDED_STDLIB_ROOTS;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The object format an embedded build links for. Injected rather than
/// read from `cfg!` so the macOS coverage host also drives the Linux arm's
/// lines (the `ExtLinkPlatform` precedent). Windows never reaches here:
/// `EmbedHost::WindowsHost` refuses the build first (#1226).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbedPlatform {
    MacOs,
    Linux,
}

impl EmbedPlatform {
    /// The build host's own platform.
    pub(crate) const HOST: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else {
        Self::Linux
    };
}

/// The first line of every `PYCC-BUNDLE` marker this build writes. A
/// sidecar whose marker starts with anything else is treated as not
/// pycc's, so it is never deleted (risk R6 of the #1028 plan).
pub(crate) const MARKER_HEADER: &str = "pycc-bundle 1";

/// The marker's file name inside the sidecar.
pub(crate) const MARKER_NAME: &str = "PYCC-BUNDLE";

/// The sidecar directory's file name for the executable `out`: `out`'s own
/// file name plus `.pycc`.
///
/// Refuses a name the loader cannot carry in an rpath: on Linux `ld.so`
/// expands `$` tokens inside `DT_RUNPATH` and splits it on `:`, and `:` is
/// unsafe in the macOS form too. No escaping fixes either, so the build
/// stops instead. A name that is not UTF-8 is refused because it is also
/// baked into the launcher as a C string literal.
pub(crate) fn sidecar_name(out: &Path) -> Result<String, String> {
    let name = out
        .file_name()
        .ok_or_else(|| format!("output path `{}` names no file", out.display()))?
        .to_str()
        .ok_or_else(|| {
            format!(
                "output file name `{}` is not UTF-8; an embedded executable records it \
                 in its library search path",
                out.display()
            )
        })?;
    if name.contains(['$', ':']) {
        return Err(format!(
            "output file name `{name}` contains `$` or `:`, which an embedded executable \
             cannot carry in its library search path; choose another `-o` name"
        ));
    }
    Ok(format!("{name}.pycc"))
}

/// The directory `out` is written into. `-o app` has an empty parent,
/// which means the current directory.
pub(crate) fn sidecar_parent(out: &Path) -> &Path {
    match out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// The linker arguments that make the executable find `<sidecar>/lib`
/// relative to itself. `-Xlinker` rather than `-Wl,`, which splits on
/// commas; `$ORIGIN` is a literal argument because no shell is involved.
pub(crate) fn rpath_args(platform: EmbedPlatform, sidecar: &str) -> Vec<OsString> {
    let origin = match platform {
        EmbedPlatform::MacOs => "@executable_path",
        EmbedPlatform::Linux => "$ORIGIN",
    };
    ["-Xlinker", "-rpath", "-Xlinker"]
        .into_iter()
        .map(OsString::from)
        .chain([OsString::from(format!("{origin}/{sidecar}/lib"))])
        .collect()
}

/// The interpreter's shared library as the probe describes it: on a
/// framework build the framework binary beside `LIBDIR`
/// (`.../Versions/3.14/Python`), otherwise `LIBDIR/LDLIBRARY` with symlinks
/// resolved. An unresolvable path is returned as is, for the caller's
/// existence check to report.
pub(crate) fn source_library(probe: &EmbedProbe) -> PathBuf {
    if !probe.framework.is_empty() {
        let root = probe.libdir.parent().unwrap_or(&probe.libdir);
        return root.join(&probe.framework);
    }
    let path = probe.libdir.join(&probe.ldlibrary);
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// The library's file name inside `<sidecar>/lib`. macOS renames it to a
/// plain dylib whose id the build rewrites to `@rpath/<name>`; Linux keeps
/// the SONAME the executable will record as `DT_NEEDED`.
pub(crate) fn bundled_library_name(platform: EmbedPlatform, probe: &EmbedProbe) -> String {
    match platform {
        EmbedPlatform::MacOs => {
            format!("libpython{}.{}.dylib", probe.version.0, probe.version.1)
        }
        EmbedPlatform::Linux if probe.instsoname.is_empty() => probe.ldlibrary.clone(),
        EmbedPlatform::Linux => probe.instsoname.clone(),
    }
}

/// The standard library's directory name under `<sidecar>/lib`, which is
/// where CPython looks for it once `home` is the sidecar.
pub(crate) fn stdlib_dir_name(probe: &EmbedProbe) -> String {
    format!("python{}.{}", probe.version.0, probe.version.1)
}

/// The `PYCC-BUNDLE` marker's text.
pub(crate) fn marker_text(probe: &EmbedProbe, library_sha256: &str) -> String {
    let (major, minor, micro) = probe.version;
    format!(
        "{MARKER_HEADER}\npython {major}.{minor}.{micro}\nexecutable {}\nlibpython-sha256 {library_sha256}\n",
        probe.executable.display()
    )
}

/// Whether `text` is a marker this build knows how to replace.
pub(crate) fn marker_is_current(text: &str) -> bool {
    text.lines().next() == Some(MARKER_HEADER)
}

/// Whether the standard-library copy skips `rel` (a path relative to the
/// interpreter's stdlib directory): `site-packages`, every `__pycache__`,
/// the `test` package, and each excluded root as a package, as a top-level
/// module, and as a `lib-dynload` extension.
pub(crate) fn skip_in_stdlib_copy(rel: &Path) -> bool {
    let parts: Vec<&str> = rel
        .components()
        .map(|part| part.as_os_str().to_str().unwrap_or(""))
        .collect();
    if parts.contains(&"__pycache__") {
        return true;
    }
    let Some(first) = parts.first() else {
        return false;
    };
    if matches!(*first, "site-packages" | "test") {
        return true;
    }
    EXCLUDED_STDLIB_ROOTS.iter().any(|root| {
        *first == *root
            || *first == format!("{root}.py")
            || (*first == "lib-dynload"
                && parts
                    .get(1)
                    .is_some_and(|file| file.starts_with(&format!("{root}."))))
    })
}

/// `text` as a C string literal. Every byte outside printable ASCII, and
/// the quote and backslash, is written as a three-digit octal escape, which
/// (unlike `\x`) cannot run into a following hex digit.
pub(crate) fn c_string_literal(text: &str) -> String {
    let mut out = String::from("\"");
    for byte in text.bytes() {
        if byte.is_ascii_graphic() && byte != b'"' && byte != b'\\' || byte == b' ' {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("\\{byte:03o}"));
        }
    }
    out.push('"');
    out
}

/// The generated `pycc_embed_config.inc` the launcher includes. With
/// `has_closure` it also defines `PYCC_EMBED_CLOSURE`, which makes the
/// launcher append `<sidecar>/closure` to `sys.path` (#1242); a build with
/// no locked package emits exactly the pre-#1242 text.
pub(crate) fn embed_config_inc(sidecar: &str, has_closure: bool) -> String {
    let closure = if has_closure {
        "#define PYCC_EMBED_CLOSURE 1\n"
    } else {
        ""
    };
    format!(
        "/* Generated by pycc for an embedded executable. Do not edit: see src/embed/layout.rs. */\n\
         #define PYCC_EMBED_SIDECAR {}\n{closure}",
        c_string_literal(sidecar)
    )
}

/// The `@loader_path` reference from a closure image at `rel` (relative to
/// `<sidecar>/closure`, `/`-separated) to a vendored library `name` in
/// `<sidecar>/lib`: one `../` per component of `rel`, which climbs from the
/// image's directory to the sidecar root.
pub(crate) fn closure_loader_relative(rel: &str, name: &str) -> String {
    let mut path = String::from("@loader_path/");
    for _ in rel.split('/') {
        path.push_str("../");
    }
    path.push_str("lib/");
    path.push_str(name);
    path
}

/// The `@loader_path` reference from an image at `image` (relative to
/// `<sidecar>/lib`) to a vendored library `name` in `<sidecar>/lib`.
pub(crate) fn loader_relative(image: &Path, name: &str) -> String {
    let depth = image.components().count().saturating_sub(1);
    let mut path = String::from("@loader_path/");
    for _ in 0..depth {
        path.push_str("../");
    }
    path.push_str(name);
    path
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
