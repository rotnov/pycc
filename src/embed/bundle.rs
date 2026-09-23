//! Effectful assembly of an embedded executable's sidecar (§3.2 and §3.4
//! of the #1028 plan): the marker check, the staged copy, the macOS
//! relocation, and the swap into place. Every failure is an environment
//! failure the caller reports at exit 2.
//!
//! Symlink policy: an existing `OUT.pycc` that is a symlink is refused and
//! never followed or removed. Inside the interpreter's standard library a
//! symlink to a file is copied as the file it resolves to; a symlink to a
//! directory, and a dangling one, is skipped, so the copy cannot cycle.

use super::EmbedProbe;
use super::layout::{self, EmbedPlatform};
use super::macho::{self, MachoDep};
use super::sha256::sha256_hex;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Checks the path the sidecar will occupy. `Ok(false)`: nothing is there.
/// `Ok(true)`: a sidecar with a current marker is there, and the build may
/// replace it. `Err`: something else is there, and it is left untouched.
pub(crate) fn check_existing(sidecar: &Path) -> Result<bool, String> {
    let metadata = match std::fs::symlink_metadata(sidecar) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("could not inspect `{}`: {e}", sidecar.display())),
    };
    let marker = sidecar.join(layout::MARKER_NAME);
    let marked = metadata.is_dir()
        && std::fs::read_to_string(&marker).is_ok_and(|text| layout::marker_is_current(&text));
    if marked {
        return Ok(true);
    }
    Err(format!(
        "`{}` already exists and is not a bundle this pycc wrote (no current `{}` marker); \
         an embedded build puts its interpreter there, so move or remove it first",
        sidecar.display(),
        layout::MARKER_NAME
    ))
}

/// Builds the sidecar for `probe` in a staging directory beside it, then
/// swaps it into `parent/sidecar_name`. Returns the bundled library's final
/// path, which the link step names by file.
pub(crate) fn assemble(
    probe: &EmbedProbe,
    platform: EmbedPlatform,
    parent: &Path,
    sidecar_name: &str,
    replace_existing: bool,
) -> Result<PathBuf, String> {
    let pid = std::process::id();
    let sidecar = parent.join(sidecar_name);
    let staging = parent.join(format!("{sidecar_name}.tmp-{pid}"));
    std::fs::create_dir(&staging).map_err(|e| io_error("create", &staging, &e))?;
    if let Err(e) = populate(probe, platform, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if replace_existing {
        let old = parent.join(format!("{sidecar_name}.old-{pid}"));
        std::fs::rename(&sidecar, &old).map_err(|e| io_error("move aside", &sidecar, &e))?;
        std::fs::rename(&staging, &sidecar)
            .map_err(|e| io_error("move into place", &staging, &e))?;
        std::fs::remove_dir_all(&old).map_err(|e| io_error("remove", &old, &e))?;
    } else {
        std::fs::rename(&staging, &sidecar)
            .map_err(|e| io_error("move into place", &staging, &e))?;
    }
    Ok(sidecar
        .join("lib")
        .join(layout::bundled_library_name(platform, probe)))
}

fn io_error(action: &str, path: &Path, e: &std::io::Error) -> String {
    format!("could not {action} `{}`: {e}", path.display())
}

/// Fills `staging` with the library, the filtered standard library, the
/// macOS relocation, and last the marker.
fn populate(probe: &EmbedProbe, platform: EmbedPlatform, staging: &Path) -> Result<(), String> {
    let lib_dir = staging.join("lib");
    std::fs::create_dir(&lib_dir).map_err(|e| io_error("create", &lib_dir, &e))?;
    let source = layout::source_library(probe);
    let bytes = std::fs::read(&source).map_err(|e| io_error("read", &source, &e))?;
    let bundled_name = layout::bundled_library_name(platform, probe);
    let bundled = lib_dir.join(&bundled_name);
    std::fs::write(&bundled, &bytes).map_err(|e| io_error("write", &bundled, &e))?;
    let stdlib = lib_dir.join(layout::stdlib_dir_name(probe));
    copy_stdlib(&probe.stdlib, &stdlib, Path::new(""))?;
    if platform == EmbedPlatform::MacOs {
        relocate_macho(probe, &lib_dir, &source, &bundled_name)?;
    }
    let marker = staging.join(layout::MARKER_NAME);
    let text = layout::marker_text(probe, &sha256_hex(&bytes));
    std::fs::write(&marker, text).map_err(|e| io_error("write", &marker, &e))
}

/// Copies `from/rel` into `to/rel`, recursively, skipping what
/// [`layout::skip_in_stdlib_copy`] names. Files are copied by reading and
/// writing their bytes, so every copy is writable by the build for the
/// relocation step regardless of the source's mode.
fn copy_stdlib(from: &Path, to: &Path, rel: &Path) -> Result<(), String> {
    let dest = to.join(rel);
    std::fs::create_dir(&dest).map_err(|e| io_error("create", &dest, &e))?;
    let source = from.join(rel);
    let entries = std::fs::read_dir(&source).map_err(|e| io_error("read", &source, &e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_error("read", &source, &e))?;
        let child = rel.join(entry.file_name());
        if layout::skip_in_stdlib_copy(&child) {
            continue;
        }
        let path = entry.path();
        let kind = std::fs::symlink_metadata(&path).map_err(|e| io_error("inspect", &path, &e))?;
        if kind.is_dir() {
            copy_stdlib(from, to, &child)?;
        } else if std::fs::metadata(&path).is_ok_and(|target| target.is_file()) {
            let bytes = std::fs::read(&path).map_err(|e| io_error("read", &path, &e))?;
            let out = to.join(&child);
            std::fs::write(&out, bytes).map_err(|e| io_error("write", &out, &e))?;
        }
    }
    Ok(())
}

/// Runs one tool, mapping a spawn failure or an unsuccessful exit to an
/// environment-failure message. Returns the tool's stdout.
pub(crate) fn run_tool(program: &str, args: &[OsString]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run `{program}`: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(format!(
        "`{program}` failed while bundling the interpreter (exit {}): {}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn otool_deps(image: &Path) -> Result<Vec<String>, String> {
    let stdout = run_tool("otool", &[OsString::from("-L"), image.into()])?;
    Ok(macho::parse_otool_l(&stdout))
}

/// One image the relocation scans: its path relative to `<sidecar>/lib`
/// and the id it answers to, if it is a dylib.
#[derive(Clone)]
struct Image {
    rel: PathBuf,
    id: Option<String>,
}

/// Makes the copied Mach-O images load only from the sidecar: rewrites the
/// bundled libpython's id to `@rpath/<name>`, then walks a worklist of
/// libpython, every `lib-dynload` extension and every image it vendors,
/// rewriting or vendoring each dependency per [`macho::classify_macho_dep`]
/// and ad-hoc re-signing every image it rewrote.
fn relocate_macho(
    probe: &EmbedProbe,
    lib_dir: &Path,
    source: &Path,
    bundled_name: &str,
) -> Result<(), String> {
    let bundled = lib_dir.join(bundled_name);
    let original_id = otool_deps(&bundled)?.into_iter().next().unwrap_or_default();
    let resolved_source = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let bundle_lib = [PathBuf::from(original_id), resolved_source];
    let prefix =
        std::fs::canonicalize(&probe.base_prefix).unwrap_or_else(|_| probe.base_prefix.clone());
    let bundled_id = format!("@rpath/{bundled_name}");
    run_tool(
        "install_name_tool",
        &macho::set_id_args(&bundled_id, &bundled),
    )?;
    let mut worklist = vec![Image {
        rel: PathBuf::from(bundled_name),
        id: Some(bundled_id),
    }];
    let dynload = PathBuf::from(layout::stdlib_dir_name(probe)).join("lib-dynload");
    if let Ok(entries) = std::fs::read_dir(lib_dir.join(&dynload)) {
        let mut names: Vec<OsString> = entries.flatten().map(|entry| entry.file_name()).collect();
        names.sort();
        worklist.extend(names.into_iter().map(|name| Image {
            rel: dynload.join(name),
            id: None,
        }));
    }
    let mut vendored: Vec<String> = Vec::new();
    let mut next = 0;
    while let Some(image) = worklist.get(next).cloned() {
        next += 1;
        let path = lib_dir.join(&image.rel);
        // libpython and every vendored image had their id rewritten already.
        let mut rewritten = image.id.is_some();
        for dep in otool_deps(&path)? {
            let resolved = if dep.starts_with('/') {
                std::fs::canonicalize(&dep).unwrap_or_else(|_| PathBuf::from(&dep))
            } else {
                PathBuf::from(&dep)
            };
            let new = match macho::classify_macho_dep(
                &dep,
                &resolved,
                image.id.as_deref(),
                &prefix,
                &bundle_lib,
                bundled_name,
            ) {
                MachoDep::Keep => continue,
                MachoDep::RewriteToBundled => format!("@rpath/{bundled_name}"),
                MachoDep::Vendor(from) => {
                    let name = from
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if !vendored.contains(&name) {
                        vendor(&from, lib_dir, &name)?;
                        vendored.push(name.clone());
                        worklist.push(Image {
                            rel: PathBuf::from(&name),
                            id: Some(format!("@rpath/{name}")),
                        });
                    }
                    layout::loader_relative(&image.rel, &name)
                }
                MachoDep::Refuse => {
                    return Err(format!(
                        "the embed interpreter `{}` is not relocatable: `{}` depends on `{dep}` \
                         outside the interpreter and the system library directories; pycc \
                         cannot bundle it yet (#1225)",
                        probe.executable.display(),
                        image.rel.display()
                    ));
                }
            };
            run_tool("install_name_tool", &macho::change_args(&dep, &new, &path))?;
            rewritten = true;
        }
        if rewritten {
            run_tool("codesign", &macho::codesign_args(&path))?;
        }
    }
    Ok(())
}

/// Copies a prefix-internal library into the sidecar and gives it an
/// `@rpath` id. Its own signature is renewed when the worklist reaches it.
fn vendor(from: &Path, lib_dir: &Path, name: &str) -> Result<(), String> {
    let to = lib_dir.join(name);
    let bytes = std::fs::read(from).map_err(|e| io_error("read", from, &e))?;
    std::fs::write(&to, bytes).map_err(|e| io_error("write", &to, &e))?;
    run_tool(
        "install_name_tool",
        &macho::set_id_args(&format!("@rpath/{name}"), &to),
    )?;
    Ok(())
}
