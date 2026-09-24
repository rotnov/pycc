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
use super::closure;
use super::layout::{self, EmbedPlatform};
use super::macho::{self, ClosureImage, MachoDep};
use super::native::{self, NativePlan, Natives};
use super::sha256::sha256_hex;
use crate::lock::build::LockedClosure;
use crate::lock::schema::LockedNative;
use std::ffi::OsString;
use std::io::Write;
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
/// path, which the link step names by file. `locked` is the consumed
/// `pycc.lock` section's closure, when the build has one (#1242), and
/// `natives` what the build copies into `lib/` besides libpython (#1243).
pub(crate) fn assemble(
    probe: &EmbedProbe,
    platform: EmbedPlatform,
    parent: &Path,
    sidecar_name: &str,
    replace_existing: bool,
    locked: Option<&LockedClosure>,
    natives: &NativePlan,
) -> Result<PathBuf, String> {
    let pid = std::process::id();
    let sidecar = parent.join(sidecar_name);
    let staging = parent.join(format!("{sidecar_name}.tmp-{pid}"));
    std::fs::create_dir(&staging).map_err(|e| io_error("create", &staging, &e))?;
    if let Err(e) = populate(probe, platform, &staging, locked, natives) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Err(e) = swap_into_place(&staging, &sidecar, parent, sidecar_name, replace_existing) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(sidecar
        .join("lib")
        .join(layout::bundled_library_name(platform, probe)))
}

/// Moves `staging` to `sidecar`. With `replace_existing`, the old sidecar
/// is first moved aside and restored when the final move fails, so a
/// failed rebuild leaves the previous artifact runnable (or, if even the
/// restore fails, names where it was left); removing the
/// moved-aside copy afterwards is best-effort, because the build already
/// succeeded by then.
pub(crate) fn swap_into_place(
    staging: &Path,
    sidecar: &Path,
    parent: &Path,
    sidecar_name: &str,
    replace_existing: bool,
) -> Result<(), String> {
    let old = parent.join(format!("{sidecar_name}.old-{}", std::process::id()));
    if replace_existing {
        std::fs::rename(sidecar, &old).map_err(|e| io_error("move aside", sidecar, &e))?;
    }
    if let Err(e) = std::fs::rename(staging, sidecar) {
        let restored = !replace_existing || std::fs::rename(&old, sidecar).is_ok();
        let note = stranded_note(restored, &old);
        return Err(format!(
            "{}{note}",
            io_error("move into place", staging, &e)
        ));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// The suffix a failed swap's error carries: empty when the previous
/// sidecar is back in place (or there was none), otherwise where it was
/// left, so the user can move it back by hand.
pub(crate) fn stranded_note(restored: bool, old: &Path) -> String {
    if restored {
        String::new()
    } else {
        format!("; the previous sidecar is left at `{}`", old.display())
    }
}

pub(crate) fn io_error(action: &str, path: &Path, e: &std::io::Error) -> String {
    format!("could not {action} `{}`: {e}", path.display())
}

/// Fills `staging` with the library, the filtered standard library, the
/// locked closure, the Linux vendored libraries or the macOS relocation,
/// and last the marker. The library's digest is taken once, before
/// relocation, for the marker and for the lock's `libpython-sha256`.
fn populate(
    probe: &EmbedProbe,
    platform: EmbedPlatform,
    staging: &Path,
    locked: Option<&LockedClosure>,
    natives: &NativePlan,
) -> Result<(), String> {
    let lib_dir = staging.join("lib");
    std::fs::create_dir(&lib_dir).map_err(|e| io_error("create", &lib_dir, &e))?;
    let source = layout::source_library(probe);
    let bytes = std::fs::read(&source).map_err(|e| io_error("read", &source, &e))?;
    let digest = sha256_hex(&bytes);
    if let Some(locked) = locked {
        let fields = [(
            "libpython-sha256",
            locked.libpython_sha256.as_str(),
            digest.as_str(),
        )];
        if let Some(difference) = crate::lock::field_difference(&fields) {
            return Err(locked.stale(&difference));
        }
    }
    let bundled_name = layout::bundled_library_name(platform, probe);
    let bundled = lib_dir.join(&bundled_name);
    std::fs::write(&bundled, &bytes).map_err(|e| io_error("write", &bundled, &e))?;
    let stdlib = lib_dir.join(layout::stdlib_dir_name(probe));
    copy_stdlib(&probe.stdlib, &stdlib, Path::new(""))?;
    let images = match locked {
        Some(locked) if !locked.files.is_empty() => closure::copy_closure(locked, staging)?,
        _ => Vec::new(),
    };
    for (name, from) in &natives.linux_vendor {
        copy_library(from, &lib_dir, name, natives.native_at(from), locked)?;
    }
    if platform == EmbedPlatform::MacOs {
        let closure = locked.map(|locked| (locked, images.as_slice()));
        relocate_macho(probe, &lib_dir, &source, &bundled_name, closure, natives)?;
    }
    let marker = staging.join(layout::MARKER_NAME);
    let text = layout::marker_text(probe, &digest);
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
/// and the id it answers to, if it is a dylib; for a closure image, its
/// path relative to `<sidecar>/closure` (#1242); for a native library, its
/// name in `<sidecar>/lib` (#1243).
#[derive(Clone)]
enum Image {
    Lib { rel: PathBuf, id: Option<String> },
    Closure { rel: String },
    Native { name: String },
}

/// Makes the copied Mach-O images load only from the sidecar: rewrites the
/// bundled libpython's id to `@rpath/<name>`, then walks a worklist of
/// libpython, every `lib-dynload` extension, every closure image and every
/// image it vendors, rewriting or vendoring each dependency per
/// [`macho::classify_macho_dep`] (for a closure image,
/// [`macho::classify_closure_dep`]; for a native,
/// [`macho::classify_native_dep`]) and ad-hoc re-signing every image it
/// rewrote. A closure image keeps its own id. A native is vendored only
/// when `natives` lists it, from bytes that still match its lock entry.
fn relocate_macho(
    probe: &EmbedProbe,
    lib_dir: &Path,
    source: &Path,
    bundled_name: &str,
    closure: Option<(&LockedClosure, &[String])>,
    natives: &NativePlan,
) -> Result<(), String> {
    let bundled = lib_dir.join(bundled_name);
    let bundle_lib = native::macho_bundle_lib(&bundled, source)?;
    let prefix = native::canonical_prefix(probe);
    let bundled_id = format!("@rpath/{bundled_name}");
    let set_id = macho::set_id_args(&bundled_id, &bundled);
    run_tool("install_name_tool", &set_id)?;
    let mut worklist = vec![Image::Lib {
        rel: PathBuf::from(bundled_name),
        id: Some(bundled_id),
    }];
    let dynload = PathBuf::from(layout::stdlib_dir_name(probe)).join("lib-dynload");
    // An interpreter without a `lib-dynload` directory contributes no
    // extension images, so a failed listing is an empty one.
    let mut names: Vec<OsString> = std::fs::read_dir(lib_dir.join(&dynload))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    let dynload_images = names.into_iter().map(|name| Image::Lib {
        rel: dynload.join(name),
        id: None,
    });
    worklist.extend(dynload_images);
    let sidecar_root = lib_dir.parent().unwrap_or(lib_dir);
    let closure_dir = sidecar_root.join(closure::CLOSURE_DIR);
    let (locked, mut sidecar) = match closure {
        Some((locked, images)) if !images.is_empty() => {
            worklist.extend(images.iter().map(|rel| Image::Closure { rel: rel.clone() }));
            (Some(locked), closure::sidecar_files(sidecar_root)?)
        }
        _ => (None, Default::default()),
    };
    // Every library vendored into `lib/`, by the name it claims there.
    let mut vendored = Natives::new(bundled_name.to_string());
    let mut next = 0;
    while let Some(image) = worklist.get(next).cloned() {
        next += 1;
        let (path, own_id, rpaths, own_payload) = match &image {
            Image::Lib { rel, id } => (lib_dir.join(rel), id.clone(), Vec::new(), None),
            Image::Native { name } => {
                let id = Some(format!("@rpath/{name}"));
                (lib_dir.join(name), id, Vec::new(), None)
            }
            Image::Closure { rel } => {
                let path = closure_dir.join(rel);
                let id_listing = run_tool("otool", &[OsString::from("-D"), path.clone().into()])?;
                let id = macho::parse_otool_d(&id_listing);
                let load_commands =
                    run_tool("otool", &[OsString::from("-l"), path.clone().into()])?;
                let rpaths = macho::parse_otool_rpaths(&load_commands);
                let payload = locked
                    .map(|locked| locked.payload_of(rel))
                    .unwrap_or_default();
                (path, id, rpaths, Some(payload))
            }
        };
        // libpython and every vendored image had their id rewritten already.
        let mut rewritten = !matches!(&image, Image::Lib { id: None, .. } | Image::Closure { .. });
        for dep in otool_deps(&path)? {
            let resolved = if dep.starts_with('/') {
                native::resolved(Path::new(&dep))
            } else {
                PathBuf::from(&dep)
            };
            let class = match (&image, &own_payload) {
                (Image::Closure { rel }, Some(own_payload)) => {
                    let closure_image = ClosureImage {
                        rel,
                        own_id: own_id.as_deref(),
                        rpaths: &rpaths,
                        own_payload,
                        sidecar: &sidecar,
                    };
                    macho::classify_closure_dep(
                        &dep,
                        &resolved,
                        &closure_image,
                        &prefix,
                        &bundle_lib,
                    )
                }
                (Image::Native { .. }, _) => {
                    let own_id = own_id.as_deref().unwrap_or_default();
                    macho::classify_native_dep(&dep, &resolved, own_id, &prefix, &bundle_lib)
                }
                _ => macho::classify_macho_dep(
                    &dep,
                    &resolved,
                    own_id.as_deref(),
                    &prefix,
                    &bundle_lib,
                    bundled_name,
                ),
            };
            let (from, locked_native) = match class {
                MachoDep::Keep => continue,
                MachoDep::RewriteToBundled => {
                    let new = format!("@rpath/{bundled_name}");
                    run_tool("install_name_tool", &macho::change_args(&dep, &new, &path))?;
                    rewritten = true;
                    continue;
                }
                MachoDep::Vendor(from) => (from, None),
                MachoDep::VendorNative(from) => {
                    let Some(entry) = natives.native_at(&from) else {
                        let why = format!(
                            "{} needs the native library `{}`, which its \
                             `[[target.native]]` entries do not list",
                            describe(&image, locked),
                            from.display()
                        );
                        return Err(stale(locked, &why));
                    };
                    (from, Some(entry))
                }
                MachoDep::Refuse => return Err(refusal(probe, &image, &dep, locked)),
            };
            let name = native::file_name(&from);
            if !vendored.claim_name(&name, &from)? {
                copy_library(&from, lib_dir, &name, locked_native, locked)?;
                let set_id = macho::set_id_args(&format!("@rpath/{name}"), &lib_dir.join(&name));
                run_tool("install_name_tool", &set_id)?;
                // A later closure image's `@rpath` walk must see the
                // vendored file, as dyld will at run time.
                sidecar.insert(format!("lib/{name}"));
                worklist.push(match locked_native {
                    Some(_) => Image::Native { name: name.clone() },
                    None => Image::Lib {
                        rel: PathBuf::from(&name),
                        id: Some(format!("@rpath/{name}")),
                    },
                });
            }
            let new = match &image {
                Image::Lib { rel, .. } => layout::loader_relative(rel, &name),
                Image::Native { name: own } => layout::loader_relative(Path::new(own), &name),
                Image::Closure { rel } => layout::closure_loader_relative(rel, &name),
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

/// How a refusal names `image`.
fn describe(image: &Image, locked: Option<&LockedClosure>) -> String {
    match image {
        Image::Lib { rel, .. } => format!("`{}`", rel.display()),
        Image::Closure { rel } => format!(
            "`closure/{rel}` (distribution `{}`)",
            locked.map_or("", |locked| locked.owner_of(rel))
        ),
        Image::Native { name } => format!("the native library `lib/{name}`"),
    }
}

/// A refusal naming the lock and `pycc lock` when the build has a lock.
fn stale(locked: Option<&LockedClosure>, why: &str) -> String {
    locked.map_or_else(|| why.to_string(), |locked| locked.stale(why))
}

/// Why `image` cannot be bundled because of its dependency `dep`.
fn refusal(probe: &EmbedProbe, image: &Image, dep: &str, locked: Option<&LockedClosure>) -> String {
    let described = describe(image, locked);
    match image {
        Image::Lib { .. } => format!(
            "the embed interpreter `{}` is not relocatable: {described} depends on `{dep}` \
             outside the interpreter and the system library directories; pycc cannot bundle \
             it",
            probe.executable.display()
        ),
        Image::Closure { .. } => format!(
            "the locked closure is not relocatable: {described} depends on `{dep}`, which is \
             a relative reference outside the interpreter and the distribution's own files; \
             pycc cannot bundle it yet (#1259)"
        ),
        Image::Native { .. } => format!(
            "the locked closure is not relocatable: {described} depends on `{dep}`, which is \
             a relative reference; pycc cannot bundle it yet (#1259)"
        ),
    }
}

/// Copies the library `from` into `lib_dir/name`, refusing to overwrite a
/// file already there. A native's bytes must still match its lock entry.
fn copy_library(
    from: &Path,
    lib_dir: &Path,
    name: &str,
    native: Option<&LockedNative>,
    locked: Option<&LockedClosure>,
) -> Result<(), String> {
    let to = lib_dir.join(name);
    let bytes = std::fs::read(from).map_err(|e| io_error("read", from, &e))?;
    if let Some(native) = native.filter(|native| sha256_hex(&bytes) != native.sha256) {
        let why = format!(
            "the native library `{}` (`{}`) changed after it was locked",
            native.name,
            from.display()
        );
        return Err(stale(locked, &why));
    }
    let mut file = std::fs::File::create_new(&to).map_err(|e| io_error("write", &to, &e))?;
    file.write_all(&bytes)
        .map_err(|e| io_error("write", &to, &e))
}
