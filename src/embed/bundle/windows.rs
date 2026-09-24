//! The Windows sidecar (D-253, Part 1 of #1226): the interpreter's DLL,
//! `python3.dll` and the VC runtime DLLs at the sidecar root, a filtered
//! `Lib\` and `DLLs\`, and the marker. There is no `lib\`, no closure
//! (a consumed `pycc.lock` is refused until #1287) and no relocation: the
//! stub loads the program DLL with the sidecar as its DLL search root.

use super::{EmbedProbe, bundle_library, copy_stdlib, io_error};
use crate::embed::layout::{self, LibpythonLink};
use std::path::Path;

/// Fills `staging` for a Windows embedded executable. The interpreter
/// DLL's digest is recorded in the marker, as on every platform.
pub(super) fn populate(probe: &EmbedProbe, staging: &Path) -> Result<(), String> {
    let dll = layout::windows_interpreter_dll(probe);
    let digest = bundle_library(&dll, &staging.join(&probe.ldlibrary), None)?;
    // The probe checked `python3.dll`; the VC runtime DLLs are optional.
    let stable = format!("python{}.dll", probe.version.0);
    copy_file(&probe.base_prefix.join(&stable), &staging.join(&stable))?;
    for name in layout::windows_runtime_dlls() {
        let from = probe.base_prefix.join(name);
        if from.is_file() {
            copy_file(&from, &staging.join(name))?;
        }
    }
    let skip = layout::skip_in_windows_stdlib_copy;
    copy_stdlib(&probe.stdlib, &staging.join("Lib"), Path::new(""), skip)?;
    let extensions = probe.base_prefix.join("DLLs");
    copy_stdlib(&extensions, &staging.join("DLLs"), Path::new(""), skip)?;
    let marker = staging.join(layout::MARKER_NAME);
    let text = layout::marker_text(probe, &digest, LibpythonLink::Shared);
    std::fs::write(&marker, text).map_err(|e| io_error("write", &marker, &e))
}

/// Copies one file's bytes from `from` to `to`.
fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    let bytes = std::fs::read(from).map_err(|e| io_error("read", from, &e))?;
    std::fs::write(to, bytes).map_err(|e| io_error("write", to, &e))
}
