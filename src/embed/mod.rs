//! The embedded-executable artifact mode (Part 1 of #1028): a plain
//! `pycc build` of a program with a standard-library CPython import links
//! a launcher that starts a bundled CPython 3.14 and runs the program as
//! its `__main__` module. The layout, the marker, the relocation rules and
//! the bridge split are recorded in the embedded-executable decision entry
//! under `docs/decisions/`; `docs/RUNTIME.md` describes the behavior.
//!
//! Kept out of `src/ext_build.rs` (already past the AGENTS.md size
//! threshold). It reuses that module's shim and `.inc` generator unchanged:
//! the shim stays the single layer that moves a `PyObject*` in both modes.

mod bundle;
mod closure;
#[cfg(test)]
pub(crate) mod fake_layout;
pub(crate) mod layout;
mod macho;
pub(crate) mod sha256;
pub(crate) mod stdlib_roots;

use crate::ext_build;
use crate::lock::probe::LockProbe;
use layout::EmbedPlatform;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The launcher's C source, compiled into every embedded executable.
pub(crate) const LAUNCHER_C: &str = include_str!("pycc_embed_launcher.c");

/// The launcher's file name in the build's scratch directory.
pub(crate) const LAUNCHER_C_NAME: &str = "pycc_embed_launcher.c";

/// The generated header the launcher includes for its sidecar name.
pub(crate) const EMBED_CONFIG_INC_NAME: &str = "pycc_embed_config.inc";

/// The CPython minor line an embedded build bundles (D-128 pins 3.14).
pub(crate) const EMBED_PYTHON: (u32, u32) = (3, 14);

/// What the embed probe reports about an interpreter, one field per line
/// of [`EMBED_PROBE_SCRIPT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbedProbe {
    pub(crate) version: (u32, u32, u32),
    /// The interpreter's `sys.executable`, recorded in the marker.
    pub(crate) executable: PathBuf,
    pub(crate) include: PathBuf,
    pub(crate) stdlib: PathBuf,
    pub(crate) base_prefix: PathBuf,
    pub(crate) enable_shared: bool,
    pub(crate) framework: String,
    pub(crate) ldlibrary: String,
    pub(crate) libdir: PathBuf,
    pub(crate) instsoname: String,
    pub(crate) gil_disabled: bool,
}

impl EmbedProbe {
    /// The configuration values an error message prints, so a refusal
    /// says what the interpreter reported rather than only that it failed.
    fn describe(&self) -> String {
        let (major, minor, micro) = self.version;
        format!(
            "version {major}.{minor}.{micro}, Py_ENABLE_SHARED={}, PYTHONFRAMEWORK='{}', \
             LDLIBRARY='{}', LIBDIR='{}', INSTSONAME='{}', Py_GIL_DISABLED={}",
            u8::from(self.enable_shared),
            self.framework,
            self.ldlibrary,
            self.libdir.display(),
            self.instsoname,
            u8::from(self.gil_disabled)
        )
    }
}

/// Asks an interpreter what an embedded build needs, one value per line; a
/// config variable that is `None` prints as an empty line. Separate from
/// `ext_build`'s probe, whose contract and tests stay unchanged.
pub(crate) const EMBED_PROBE_SCRIPT: &str = "import sys,sysconfig\n\
     print('%d.%d.%d' % sys.version_info[:3])\n\
     print(sys.executable)\n\
     print(sysconfig.get_path('include'))\n\
     print(sysconfig.get_path('stdlib'))\n\
     print(sys.base_prefix)\n\
     for k in ('Py_ENABLE_SHARED','PYTHONFRAMEWORK','LDLIBRARY','LIBDIR','INSTSONAME','Py_GIL_DISABLED'):\n\
     \x20   v = sysconfig.get_config_var(k)\n\
     \x20   print('' if v is None else v)\n";

/// The interpreter an embedded build bundles, resolved lazily: nothing
/// spawns until [`Self::probe`] runs, which happens only once the program
/// is known to need an interpreter, so a native build never starts Python.
#[derive(Debug, Clone)]
pub(crate) struct EmbedToolchain {
    interpreter: OsString,
    probe_override: Option<EmbedProbe>,
    lock_probe_override: Option<LockProbe>,
}

impl EmbedToolchain {
    /// The production constructor: `PYCC_PYTHON` names the interpreter to
    /// bundle, default `python3.14` (not `--ext`'s `python3`, because an
    /// embedded build pins the 3.14 line). Reads the variable only.
    pub(crate) fn from_env() -> Self {
        Self {
            interpreter: std::env::var_os("PYCC_PYTHON")
                .unwrap_or_else(|| OsString::from("python3.14")),
            probe_override: None,
            lock_probe_override: None,
        }
    }

    /// A toolchain that answers from `probe` instead of running anything.
    #[cfg(test)]
    pub(crate) fn with_probe(interpreter: impl Into<OsString>, probe: EmbedProbe) -> Self {
        Self {
            interpreter: interpreter.into(),
            probe_override: Some(probe),
            lock_probe_override: None,
        }
    }

    /// A toolchain that answers both the embed probe and the lock probe
    /// from `probe` and `lock_probe` instead of running anything.
    #[cfg(test)]
    pub(crate) fn with_probes(
        interpreter: impl Into<OsString>,
        probe: EmbedProbe,
        lock_probe: LockProbe,
    ) -> Self {
        Self {
            interpreter: interpreter.into(),
            probe_override: Some(probe),
            lock_probe_override: Some(lock_probe),
        }
    }

    /// A toolchain that really runs `interpreter`.
    #[cfg(test)]
    pub(crate) fn with_interpreter(interpreter: impl Into<OsString>) -> Self {
        Self {
            interpreter: interpreter.into(),
            probe_override: None,
            lock_probe_override: None,
        }
    }

    /// The interpreter's site directories, tags and marker environment
    /// (the lock probe), which `pycc lock` records and an embedded build
    /// compares against the lock.
    pub(crate) fn lock_probe(&self) -> Result<LockProbe, String> {
        match &self.lock_probe_override {
            Some(probe) => Ok(probe.clone()),
            None => crate::lock::probe::run_lock_probe(&self.interpreter),
        }
    }

    /// Probes the interpreter and checks it can be bundled, or returns an
    /// environment-failure message for exit 2.
    pub(crate) fn probe(&self) -> Result<EmbedProbe, String> {
        let probe = match &self.probe_override {
            Some(probe) => probe.clone(),
            None => self.run_probe()?,
        };
        check_embed_version(probe.version)?;
        let name = self.interpreter.to_string_lossy();
        if probe.gil_disabled {
            return Err(format!(
                "the embed interpreter `{name}` is a free-threaded build ({}); an embedded \
                 executable bundles a GIL-enabled CPython 3.14",
                probe.describe()
            ));
        }
        if !check_shared(probe.enable_shared, &probe.framework) {
            return Err(format!(
                "the embed interpreter `{name}` has no shared libpython ({}); an embedded \
                 executable links against one -- set PYCC_PYTHON to a CPython 3.14 built \
                 with --enable-shared or as a framework",
                probe.describe()
            ));
        }
        if !probe.include.join("Python.h").is_file() {
            return Err(format!(
                "the embed interpreter `{name}` has no `Python.h` in `{}`; an embedded build \
                 compiles its launcher against that header -- install the interpreter's \
                 development files",
                probe.include.display()
            ));
        }
        let library = layout::source_library(&probe);
        if !library.is_file() {
            return Err(format!(
                "the embed interpreter `{name}` reports a shared library `{}` that does not \
                 exist ({})",
                library.display(),
                probe.describe()
            ));
        }
        if !probe.stdlib.is_dir() {
            return Err(format!(
                "the embed interpreter `{name}` reports a standard library `{}` that does not \
                 exist",
                probe.stdlib.display()
            ));
        }
        Ok(probe)
    }

    fn run_probe(&self) -> Result<EmbedProbe, String> {
        let name = self.interpreter.to_string_lossy();
        let output = ext_build::probe_command(&self.interpreter, EMBED_PROBE_SCRIPT)
            .output()
            .map_err(|e| {
                format!(
                    "could not run the embed interpreter `{name}`: {e}; a build that imports \
                     a CPython standard-library module bundles CPython 3.14 -- set \
                     PYCC_PYTHON to name one (default `python3.14`)"
                )
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = output.status.success().then(|| parse_embed_probe(&stdout));
        parsed.flatten().ok_or_else(|| {
            format!(
                "the embed interpreter `{name}` did not report a configuration this build \
                 could parse (exit {})",
                output.status.code().unwrap_or(-1)
            )
        })
    }
}

/// Parses [`EMBED_PROBE_SCRIPT`]'s eleven lines. Pure.
pub(crate) fn parse_embed_probe(stdout: &str) -> Option<EmbedProbe> {
    let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
    let [
        version,
        executable,
        include,
        stdlib,
        base_prefix,
        shared,
        framework,
        ldlibrary,
        libdir,
        instsoname,
        gil,
    ]: [&str; 11] = lines.get(..11)?.try_into().ok()?;
    let mut parts = version.split('.').map(str::parse::<u32>);
    let version = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    Some(EmbedProbe {
        version,
        executable: PathBuf::from(executable),
        include: PathBuf::from(include),
        stdlib: PathBuf::from(stdlib),
        base_prefix: PathBuf::from(base_prefix),
        enable_shared: shared == "1",
        framework: framework.to_string(),
        ldlibrary: ldlibrary.to_string(),
        libdir: PathBuf::from(libdir),
        instsoname: instsoname.to_string(),
        gil_disabled: gil == "1",
    })
}

/// Accepts any `3.14.x`.
pub(crate) fn check_embed_version(version: (u32, u32, u32)) -> Result<(), String> {
    let (major, minor, micro) = version;
    if (major, minor) == EMBED_PYTHON {
        return Ok(());
    }
    Err(format!(
        "the embed interpreter is CPython {major}.{minor}.{micro}; an embedded executable \
         bundles CPython {}.{} -- set PYCC_PYTHON to a {}.{} interpreter",
        EMBED_PYTHON.0, EMBED_PYTHON.1, EMBED_PYTHON.0, EMBED_PYTHON.1
    ))
}

/// Whether the interpreter has a shared libpython. Measured rule: a
/// framework build is shared even though it reports `Py_ENABLE_SHARED=0`.
pub(crate) fn check_shared(enable_shared: bool, framework: &str) -> bool {
    enable_shared || !framework.is_empty()
}

/// What `try_build`'s single link site adds for an embedded executable.
#[derive(Debug)]
pub(crate) struct EmbedPlan {
    /// `-I <include> -fPIC <shim> <launcher>`, before the pycc object.
    pub(crate) compile_args: Vec<OsString>,
    /// The bundled library by path, then the rpath, after the runtime.
    pub(crate) link_args: Vec<OsString>,
}

/// Prepares everything an embedded build links, in order: the sidecar
/// name, the check of an existing sidecar (before anything is probed), the
/// `pycc.lock` checks that need no interpreter (#1242), the probe, the
/// lock's interpreter comparison and payload plan, the C sources in the
/// scratch directory beside `obj_path`, and the sidecar itself (with the
/// locked closure copied into it), swapped into place before the link so
/// the executable links against the final bundled library. `host` is the
/// `(arch, os)` pair the lock section is selected by.
pub(crate) fn plan_embed(
    out: &Path,
    entry: &Path,
    typed_hir: &pycc_hir::HirModule,
    toolchain: &EmbedToolchain,
    platform: EmbedPlatform,
    host: (&str, &str),
    obj_path: &Path,
) -> Result<EmbedPlan, String> {
    let sidecar_name = layout::sidecar_name(out)?;
    let parent = layout::sidecar_parent(out);
    let replace_existing = bundle::check_existing(&parent.join(&sidecar_name))?;
    let check = crate::lock::build::plan_closure(entry, typed_hir, host)?;
    let probe = toolchain.probe()?;
    let locked = match &check {
        Some(check) => {
            let lock_probe = toolchain.lock_probe()?;
            crate::lock::build::verify_interpreter(check, &probe, &lock_probe)?;
            Some(crate::lock::build::payload(check, &lock_probe)?)
        }
        None => None,
    };
    let has_closure = locked
        .as_ref()
        .is_some_and(|locked| !locked.files.is_empty());
    let shim = obj_path.with_file_name(ext_build::SHIM_C_NAME);
    let launcher = obj_path.with_file_name(LAUNCHER_C_NAME);
    let classes = ext_build::collect_user_exception_classes(typed_hir);
    let exports_inc = ext_build::generate_exports_inc("__main__", &[], &classes, &[], &[]);
    write_source(&shim, ext_build::SHIM_C)?;
    let exports_path = obj_path.with_file_name(ext_build::EXPORTS_INC_NAME);
    write_source(&exports_path, &exports_inc)?;
    write_source(&launcher, LAUNCHER_C)?;
    let config_path = obj_path.with_file_name(EMBED_CONFIG_INC_NAME);
    let config = layout::embed_config_inc(&sidecar_name, has_closure);
    write_source(&config_path, &config)?;
    let library = bundle::assemble(
        &probe,
        platform,
        parent,
        &sidecar_name,
        replace_existing,
        locked.as_ref(),
    )?;
    let mut compile_args = vec![OsString::from("-I"), probe.include.into_os_string()];
    compile_args.extend([OsString::from("-fPIC"), shim.into(), launcher.into()]);
    let mut link_args = vec![library.into_os_string()];
    link_args.extend(layout::rpath_args(platform, &sidecar_name));
    Ok(EmbedPlan {
        compile_args,
        link_args,
    })
}

fn write_source(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| {
        format!(
            "could not write the embedded build source `{}`: {e}",
            pycc_diag::display_path(&path.to_string_lossy())
        )
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
