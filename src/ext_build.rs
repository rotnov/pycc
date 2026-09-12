//! The `--ext` build seam: what a hosted CPython extension artifact needs
//! that a native executable does not (D-244, #1036, Part 1 of #1025).
//!
//! `src/ext_output.rs` already owns *where* the artifact is written and what
//! its module is called. This module owns everything else `try_build`'s ext
//! branch needs:
//!
//! * the CPython toolchain probe (interpreter, header directory, stable-ABI
//!   floor), [`ExtToolchain`];
//! * the per-platform shared-object link argv, [`ExtLinkPlatform`] and
//!   [`ext_link_args`];
//! * the export set and its `C0003` capability gaps, [`collect_exports`];
//! * the generated C companion to the fixed shim, [`generate_exports_inc`]
//!   and [`SHIM_C`].
//!
//! Every item here except [`ExtToolchain::probe`] is a pure function of its
//! arguments. That is deliberate, and it is the same property
//! `src/ext_output.rs` documents for its own contract: the three link arms
//! and both header-resolution failures must be executable from an ordinary
//! unit test on one host, because `.github/workflows/ci.yml`'s coverage job
//! runs `llvm-cov` without `--include-ignored` -- an `#[ignore]`d
//! CPython-driven test contributes exactly zero coverage while its lines
//! stay in `scripts/check_diff_coverage.py`'s denominator. A `cfg!(windows)`
//! branch would be just as unexecutable as a `#[cfg]` one is *coverable*,
//! so the platform arms are selected from the resolved **target triple**,
//! never from the building host -- which is also what makes
//! `pycc build --target x86_64-pc-windows-msvc --ext` emit a Windows argv
//! from a macOS host at all.

use crate::ext_output::ExtPlatform;
use pycc_diag::{Diagnostic, Severity};
use pycc_hir::{HirItem, HirModule, Ty, is_public_name};
#[cfg(test)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The fixed, hand-written half of every `ext` artifact, embedded at compile
/// time. See `src/ext/pycc_ext_module.c`'s own header comment for why it is
/// C at all and why it is embedded rather than located by path.
pub(crate) const SHIM_C: &str = include_str!("ext/pycc_ext_module.c");

/// The shim's file name in the build scratch directory. The generated
/// companion is [`EXPORTS_INC_NAME`], which the shim `#include`s by this
/// exact spelling, so the two must be written side by side.
pub(crate) const SHIM_C_NAME: &str = "pycc_ext_module.c";

/// The generated companion's file name -- `#include`d by [`SHIM_C`], never
/// compiled on its own.
pub(crate) const EXPORTS_INC_NAME: &str = "pycc_ext_exports.inc";

/// The stable-ABI floor: `Py_LIMITED_API 0x030D0000` in the shim pins the
/// artifact to CPython 3.13's limited API, so building against an older
/// interpreter's headers would compile against a smaller stable ABI than the
/// one the shim declares. 3.13 is also the first release whose
/// `EXTENSION_SUFFIXES` the `ext_output` contract's `.abi3.so` spelling is
/// derived from.
pub(crate) const MIN_PYTHON: (u32, u32) = (3, 13);

/// `C0003`: an `ext`-mode capability gap.
///
/// Distinct from `C0001` ("construct not supported yet") because the
/// construct here *is* supported -- `pycc` compiles the function perfectly
/// well for `native` mode. What is missing is the `PyObject*` boundary for
/// its signature: Part 1 of #1025 bridges `int` only. `docs/DIAGNOSTICS.md`
/// carries the registry entry and `crates/pycc_diag/src/explain.rs` the
/// long-form explanation.
pub(crate) const EXT_CAPABILITY_CODE: &str = "C0003";

/// What a CPython installation told us about itself.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtProbe {
    /// `(major, minor)`, checked against [`MIN_PYTHON`].
    pub(crate) version: (u32, u32),
    /// The directory holding `Python.h`, passed to the compiler as `-I`.
    pub(crate) include: PathBuf,
    /// Where an import library lives. Only Windows links against one; every
    /// other platform resolves the interpreter's symbols at load time and
    /// must *not* be given a `-lpython`, which is what would pin the
    /// artifact to one interpreter build.
    pub(crate) libs: PathBuf,
}

/// The CPython installation an `ext` build compiles against.
///
/// Constructed from the environment in production ([`ExtToolchain::from_env`])
/// and directly in tests ([`ExtToolchain::with_probe`]) -- never by a test
/// setting a process-wide environment variable, which races every other test
/// in the same binary.
#[derive(Debug, Clone)]
pub(crate) struct ExtToolchain {
    interpreter: OsString,
    probe_override: Option<ExtProbe>,
}

/// Asks an interpreter the three things an `ext` build needs, one per line.
/// Kept as a single expression-free script so no shell quoting is involved:
/// it is passed as one `-c` argument to the interpreter process directly.
const PROBE_SCRIPT: &str = "import os,sys,sysconfig\n\
                            print('%d.%d' % sys.version_info[:2])\n\
                            print(sysconfig.get_path('include'))\n\
                            print(os.path.join(sys.base_prefix, 'libs'))\n";

impl ExtToolchain {
    /// The production constructor. `PYCC_PYTHON` names the interpreter to
    /// probe (default `python3`); `PYCC_PYTHON_INCLUDE`, when set, supplies
    /// the header directory without probing at all, for a cross build or a
    /// sysroot whose interpreter cannot run on this host.
    ///
    /// `PYCC_PYTHON_INCLUDE` records [`MIN_PYTHON`] as the version, which is
    /// an *assertion by the caller*, not a measurement: no interpreter runs,
    /// so nothing here can read the headers' real version, and
    /// [`Self::probe`]'s floor check is therefore satisfied by construction
    /// on this path. Setting it says "these headers are at least the
    /// stable-ABI floor"; if they are not, the failure surfaces in `cc`
    /// against the real `Python.h` rather than here.
    pub(crate) fn from_env() -> Self {
        let interpreter =
            std::env::var_os("PYCC_PYTHON").unwrap_or_else(|| OsString::from("python3"));
        let probe_override = std::env::var_os("PYCC_PYTHON_INCLUDE").map(|include| {
            let include = PathBuf::from(include);
            ExtProbe {
                version: MIN_PYTHON,
                libs: include.with_file_name("libs"),
                include,
            }
        });
        Self {
            interpreter,
            probe_override,
        }
    }

    /// Builds a toolchain that answers from `probe` instead of running an
    /// interpreter. `interpreter` is still recorded, so a caller can prove
    /// the override is what suppressed the spawn.
    #[cfg(test)]
    pub(crate) fn with_probe(interpreter: impl Into<OsString>, probe: ExtProbe) -> Self {
        Self {
            interpreter: interpreter.into(),
            probe_override: Some(probe),
        }
    }

    /// Builds a toolchain that really probes `interpreter`.
    #[cfg(test)]
    pub(crate) fn with_interpreter(interpreter: impl Into<OsString>) -> Self {
        Self {
            interpreter: interpreter.into(),
            probe_override: None,
        }
    }

    /// Resolves the headers to compile against, or an environment-class
    /// message for `try_build` to report at exit 2.
    ///
    /// The directory check runs on an override exactly as it does on a real
    /// probe, and so does the floor check -- but see [`Self::from_env`]: an
    /// override built from `PYCC_PYTHON_INCLUDE` carries [`MIN_PYTHON`] as
    /// its asserted version, so the floor is a real gate only for a probe
    /// that actually ran (or a test-supplied one).
    pub(crate) fn probe(&self) -> Result<ExtProbe, String> {
        let probe = match &self.probe_override {
            Some(probe) => probe.clone(),
            None => self.run_probe()?,
        };
        check_floor(probe.version)?;
        if !probe.include.is_dir() {
            return Err(format!(
                "CPython header directory `{}` does not exist; --ext compiles against \
                 `Python.h` from the interpreter named by PYCC_PYTHON (default `python3`), \
                 or from PYCC_PYTHON_INCLUDE when that is set",
                probe.include.display()
            ));
        }
        Ok(probe)
    }

    fn run_probe(&self) -> Result<ExtProbe, String> {
        let output = std::process::Command::new(&self.interpreter)
            .arg("-c")
            .arg(PROBE_SCRIPT)
            .output()
            .map_err(|e| {
                format!(
                    "could not run the CPython interpreter `{}`: {e}; --ext needs one to \
                     locate `Python.h` -- set PYCC_PYTHON to name a different interpreter, \
                     or PYCC_PYTHON_INCLUDE to skip the probe entirely",
                    self.interpreter.to_string_lossy()
                )
            })?;
        if !output.status.success() {
            return Err(format!(
                "the CPython interpreter `{}` failed to report its own configuration \
                 (exit {})",
                self.interpreter.to_string_lossy(),
                output.status.code().unwrap_or(-1)
            ));
        }
        parse_probe_output(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
            format!(
                "the CPython interpreter `{}` reported a configuration this build could \
                 not parse",
                self.interpreter.to_string_lossy()
            )
        })
    }
}

/// Parses [`PROBE_SCRIPT`]'s three lines. Pure, so every malformed shape is
/// reachable from a unit test without an interpreter that produces it.
pub(crate) fn parse_probe_output(stdout: &str) -> Option<ExtProbe> {
    let mut lines = stdout.lines();
    let version = parse_version(lines.next()?)?;
    let include = PathBuf::from(lines.next()?.trim());
    let libs = PathBuf::from(lines.next()?.trim());
    if include.as_os_str().is_empty() {
        return None;
    }
    Some(ExtProbe {
        version,
        include,
        libs,
    })
}

/// Parses a `major.minor` version line. Extra components (`3.13.1`) are
/// ignored rather than rejected: the floor only concerns the first two.
pub(crate) fn parse_version(line: &str) -> Option<(u32, u32)> {
    let line = line.trim();
    let (major, rest) = line.split_once('.')?;
    let minor = rest.split('.').next()?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Enforces [`MIN_PYTHON`].
pub(crate) fn check_floor(version: (u32, u32)) -> Result<(), String> {
    if version >= MIN_PYTHON {
        return Ok(());
    }
    Err(format!(
        "--ext needs CPython {}.{} or newer to build against (found {}.{}): the artifact \
         declares `Py_LIMITED_API 0x030D0000`, so an older interpreter's headers describe a \
         smaller stable ABI than the one it uses",
        MIN_PYTHON.0, MIN_PYTHON.1, version.0, version.1
    ))
}

/// The link-time shape of an `ext` artifact, selected from the resolved
/// target triple.
///
/// Distinct from [`ExtPlatform`], which distinguishes only the two
/// *extension-suffix* families (POSIX vs Windows) because that is all
/// `EXTENSION_SUFFIXES` distinguishes. Linking needs three arms: macOS's
/// two-level namespace makes a `-shared` `.so` with unresolved `Py*` symbols
/// a link error, so it needs a bundle with deferred lookup instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExtLinkPlatform {
    /// macOS: a `-bundle` with `-undefined dynamic_lookup`.
    MacOs,
    /// Linux and every other ELF host: a plain `-shared`.
    Linux,
    /// Windows: a DLL that must resolve `Py*` at link time against
    /// `python3.lib`, the stable-ABI import library.
    Windows,
}

impl ExtLinkPlatform {
    /// This build host, for the no-`--target` case. The only `cfg` in this
    /// module: it selects a *value*, so every arm's own behaviour stays
    /// reachable from a unit test on any host.
    pub(crate) const HOST: Self = if cfg!(windows) {
        Self::Windows
    } else if cfg!(target_os = "macos") {
        Self::MacOs
    } else {
        Self::Linux
    };

    /// Classifies a target triple. Unknown triples fall to [`Self::Linux`],
    /// the ELF default -- the same POSIX-default convention
    /// [`crate::ext_output`]'s suffix classification uses, so a triple this
    /// function has never seen gets a plain `-shared` link and a `.so`
    /// rather than a build that refuses to start.
    pub(crate) fn from_target_triple(triple: &str) -> Self {
        if triple.contains("windows") {
            Self::Windows
        } else if triple.contains("apple") || triple.contains("darwin") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// Resolves the platform for `target`, honouring the same
    /// "target, never host" rule `ext_output` states.
    pub(crate) fn resolve(target: Option<&str>) -> Self {
        target.map_or(Self::HOST, Self::from_target_triple)
    }

    /// The suffix family this link platform writes into.
    pub(crate) fn suffix_platform(self) -> ExtPlatform {
        match self {
            Self::Windows => ExtPlatform::Windows,
            Self::MacOs | Self::Linux => ExtPlatform::Unix,
        }
    }
}

/// The platform-specific arguments that turn a link into an importable
/// extension module, in the order the driver receives them.
///
/// Deliberately returns `OsString`s built from `Path::join`, never a
/// formatted string: a rendered `Path` carries the *building host's*
/// separator, so a `-L` argument assembled by `format!` would spell a
/// Windows target's library directory with `/` when cross-building from
/// macOS. (PR 1a's only CI failure was a test asserting a rendered path.)
///
/// No position-independent-code work is needed on the *Rust* side to pull
/// `libpycc_rt.a` into a shared object: none of the five Tier-1 target
/// specs sets `relocation-model`, so all five take `rustc`'s own default,
/// `pic` (checked with `rustc --print target-spec-json --target <triple>`
/// for each triple in [`ARCHITECTURE.md`]'s Tier-1 table). `pycc_rt`
/// therefore keeps `crate-type = ["staticlib", "rlib"]` and gains no
/// `cdylib`: the artifact is one shared object holding the compiled module
/// *and* the runtime, not two linked against each other. The C shim is the
/// one piece that does need an explicit flag -- see [`ext_compile_args`].
///
/// [`ARCHITECTURE.md`]: https://github.com/rotnov/pycc/blob/main/docs/ARCHITECTURE.md
pub(crate) fn ext_link_args(platform: ExtLinkPlatform, libs: &Path) -> Vec<OsString> {
    match platform {
        // A macOS extension module is a Mach-O *bundle*, and its `Py*`
        // references are resolved by the already-loaded interpreter at
        // `dlopen` time. Without `-undefined dynamic_lookup` the static
        // linker rejects them outright; with a `-lpython` instead, the
        // artifact would load a *second* copy of libpython beside the
        // running one.
        ExtLinkPlatform::MacOs => vec![
            OsString::from("-bundle"),
            OsString::from("-undefined"),
            OsString::from("dynamic_lookup"),
        ],
        // ELF resolves undefined symbols lazily against the global scope
        // the interpreter already occupies, so `-shared` alone is correct
        // and a `-lpython` is actively wrong for the same reason as above.
        ExtLinkPlatform::Linux => vec![OsString::from("-shared")],
        // Windows has no lazy global scope: every import must be bound at
        // link time through an import library. `python3.lib` is the
        // stable-ABI one, deliberately not the version-tagged
        // `python313.lib` -- the whole point of the artifact is that it
        // outlives one interpreter minor version.
        ExtLinkPlatform::Windows => {
            let mut args = vec![OsString::from("-shared"), OsString::from("-L")];
            args.push(libs.as_os_str().to_os_string());
            args.push(OsString::from("-lpython3"));
            args
        }
    }
}

/// One exported module-level function.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtExport {
    /// The Python name, which is also the `PyMethodDef` name and the suffix
    /// of the `fnptr_<name>` global codegen emits for its binding.
    pub(crate) name: String,
    /// How many `int` parameters it takes.
    pub(crate) arity: usize,
}

/// Derives the export set from the typed program, per D-244 rule 1: every
/// public module-level function is exported. "The compiled module" there is
/// the linked program D-222 produces -- the entry file plus its whole import
/// closure -- so a public function defined in an imported project module is
/// exported too, and renaming it private is how a project keeps it off the
/// artifact's CPython surface.
///
/// "Public" is D-038's predicate, `pycc_hir::is_public_name`. Two further
/// exclusions are not policy but representation: a method reaches
/// `HirItem::Function` under its `Class.method` name, which is not a
/// module-level function at all, and a monomorphized generic specialization
/// carries the `0gen_` prefix and has no `fnptr_` global to call through
/// (codegen dispatches those directly).
///
/// Part 1 of #1025 bridges `int` only, in both directions. Every other
/// public signature is a [`EXT_CAPABILITY_CODE`] capability gap, and *all*
/// of them are collected before returning -- one `--ext` build should not
/// have to be re-run once per unsupported function.
///
/// The export set is derived here, in the driver, rather than carried as a
/// new field on `HirItem::Function`/`MirItem::Function`: those two patterns
/// are constructed at 693 and 174 sites across this workspace, and a new
/// field would put hundreds of mechanically-updated non-test lines into the
/// 100%-coverage denominator for no behavioural gain.
pub(crate) fn collect_exports(module: &HirModule) -> Result<Vec<ExtExport>, Vec<Diagnostic>> {
    let mut exports = Vec::new();
    let mut gaps = Vec::new();
    for item in &module.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } = item
        else {
            continue;
        };
        if !is_public_name(name) || name.contains('.') || name.starts_with("0gen_") {
            continue;
        }
        if let Some(offender) = unsupported_boundary_ty(params, return_ty) {
            gaps.push(capability_gap(name, &offender));
            continue;
        }
        exports.push(ExtExport {
            name: name.clone(),
            arity: params.len(),
        });
    }
    if gaps.is_empty() {
        Ok(exports)
    } else {
        Err(gaps)
    }
}

/// Names the first part of a signature the `ext` boundary cannot carry, as
/// the user would write it (`x: float`, `-> str`), or `None` when the whole
/// signature is `int`-only.
fn unsupported_boundary_ty(params: &[(String, Ty)], return_ty: &Ty) -> Option<String> {
    if let Some((name, ty)) = params.iter().find(|(_, ty)| *ty != Ty::Int) {
        return Some(format!("parameter `{name}: {}`", render_ty(ty)));
    }
    if *return_ty != Ty::Int {
        return Some(format!("return type `-> {}`", render_ty(return_ty)));
    }
    None
}

/// A short Python-facing spelling of a `Ty`, for the `C0003` message only.
/// Deliberately coarse: a container's element type adds nothing to "this
/// boundary carries `int` only".
fn render_ty(ty: &Ty) -> &'static str {
    match ty {
        Ty::Int => "int",
        Ty::Float => "float",
        Ty::Bool => "bool",
        Ty::Str => "str",
        Ty::None => "None",
        Ty::List(_) => "list",
        Ty::Dict(_) => "dict",
        Ty::Set(_) => "set",
        Ty::Tuple(_) => "tuple",
        _ => "that type",
    }
}

/// Builds the `C0003` diagnostic for one unexportable public function.
///
/// Span-less: `HirItem::Function` carries no source range (the whole point
/// of `pycc_hir`'s lowered form), and `pycc_diag::render_human` renders a
/// span-less diagnostic as exactly `error[C0003]: <message>`, which is what
/// `report_build_failure` needs.
fn capability_gap(name: &str, offender: &str) -> Diagnostic {
    Diagnostic {
        code: EXT_CAPABILITY_CODE,
        severity: Severity::Error,
        message: format!(
            "--ext cannot export the public function `{name}`: its {offender} is not an \
             `int`, and this pycc version's CPython boundary carries `int` only (D-244 rule \
             1 exports every public module-level function, so there is no way to opt one \
             out) -- rename it to `_{name}` to keep it out of the export set, or build \
             without --ext"
        ),
        span: None,
        label: None,
        help: None,
    }
}

/// Renders the generated C companion to [`SHIM_C`]: the module name macros,
/// one `METH_FASTCALL` wrapper per export, and the `PyMethodDef` table.
///
/// `module_name` is already known to be a valid ASCII Python identifier
/// (`ext_output::resolve` rejects everything else before this runs), and an
/// export name is a Python identifier by construction, so neither can carry
/// a character that would escape the C source it is pasted into.
pub(crate) fn generate_exports_inc(module_name: &str, exports: &[ExtExport]) -> String {
    let mut out = String::new();
    out.push_str("/* Generated by pycc --ext. Do not edit: see src/ext_build.rs. */\n");
    out.push_str(&format!("#define PYCC_EXT_MODULE_NAME {module_name}\n"));
    out.push_str(&format!(
        "#define PYCC_EXT_MODULE_NAME_STR \"{module_name}\"\n\n"
    ));
    for export in exports {
        out.push_str(&wrapper_for(export));
    }
    out.push_str("static PyMethodDef pycc_ext_methods[] = {\n");
    for export in exports {
        out.push_str(&format!(
            "    {{\"{name}\", (PyCFunction)(void (*)(void))pycc_ext_wrap_{name}, \
             METH_FASTCALL, NULL}},\n",
            name = export.name
        ));
    }
    out.push_str("    {NULL, NULL, 0, NULL},\n};\n");
    out
}

/// One export's `METH_FASTCALL` wrapper.
///
/// `METH_FASTCALL` rather than `METH_VARARGS` for two reasons. It is the
/// only limited-API calling convention that receives the argument count
/// directly, so arity and type checking cost no tuple; and CPython itself
/// raises the keyword `TypeError` before the wrapper is entered, which is
/// exactly D-244 rule 7's closed boundary for free -- a `METH_VARARGS`
/// wrapper would silently accept `f(x=1)` at the C level.
///
/// The call goes through `fnptr_<name>`, the module-level function-pointer
/// global codegen emits for each `def`'s binding, and *not* through the
/// compiled function's own symbol. That is what makes a rebound name
/// (`f = g` at module level) call what Python says it calls.
fn wrapper_for(export: &ExtExport) -> String {
    let name = &export.name;
    let mut out = String::new();
    out.push_str(&format!("extern void *fnptr_{name};\n"));
    out.push_str(&format!(
        "static PyObject *pycc_ext_wrap_{name}(PyObject *self, PyObject *const *args, \
         Py_ssize_t nargs)\n{{\n"
    ));
    out.push_str("    (void)self;\n");
    out.push_str("    (void)args;\n");
    out.push_str("    long long result;\n");
    for index in 0..export.arity {
        out.push_str(&format!("    long long a{index};\n"));
    }
    out.push_str(&format!(
        "    if (nargs != {arity}) {{\n        PyErr_Format(PyExc_TypeError, \
         \"{name}() takes exactly {arity} argument{plural} (%zd given)\", nargs);\n        \
         return NULL;\n    }}\n",
        arity = export.arity,
        plural = if export.arity == 1 { "" } else { "s" },
    ));
    for index in 0..export.arity {
        out.push_str(&format!(
            "    if (pycc_ext_unpack_int(args[{index}], \"{name}\", {index}, &a{index}) != 0) \
             {{\n        return NULL;\n    }}\n"
        ));
    }
    let params = if export.arity == 0 {
        "void".to_string()
    } else {
        vec!["long long"; export.arity].join(", ")
    };
    let call_args = (0..export.arity)
        .map(|index| format!("a{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!(
        "    result = ((long long (*)({params}))fnptr_{name})({call_args});\n"
    ));
    // A compiled function that raised returns a neutral carrier and leaves
    // the runtime's thread-local flag set (see `pycc_codegen`'s
    // `exception_exit` block), so the carrier must never be packed: the
    // pending exception is checked first and translated into a CPython one.
    out.push_str(
        "    if (pycc_rt_ext_pending_type() >= 0) {\n        pycc_ext_raise_pending();\n        \
         return NULL;\n    }\n",
    );
    out.push_str(&format!(
        "    return pycc_ext_pack_int(\"{name}\", result);\n}}\n\n"
    ));
    out
}

/// The `-I`, code-model and source arguments for the shim, in driver order.
/// Split out from [`ext_link_args`] so the platform arms there stay purely
/// about what makes the output loadable.
///
/// The ELF arm must pass `-fPIC` explicitly. GCC on Linux compiles to
/// position-dependent code by default, and the shim's references to
/// CPython's exception objects (`PyExc_ValueError`, `PyExc_ImportError`)
/// are undefined symbols resolved by the host at load time, so a
/// position-dependent `R_X86_64_PC32` relocation against them makes the
/// `-shared` link fail outright. It is emitted on the Mach-O arm too,
/// where clang already defaults to PIC, so the flag is a stated property
/// of the artifact rather than an inherited host default -- the same
/// reason this module takes its platform from the target triple and never
/// from `cfg!`. The Windows arm omits it: a PE/COFF target is position
/// independent by construction and its drivers warn the flag is ignored.
pub(crate) fn ext_compile_args(
    platform: ExtLinkPlatform,
    include: &Path,
    shim: &Path,
) -> Vec<OsString> {
    let mut args = vec![OsString::from("-I"), include.as_os_str().to_os_string()];
    if matches!(platform, ExtLinkPlatform::Linux | ExtLinkPlatform::MacOs) {
        args.push(OsString::from("-fPIC"));
    }
    args.push(shim.as_os_str().to_os_string());
    args
}

/// Borrowed view used only to keep [`ExtToolchain::interpreter`] observable
/// from a test without making the field public.
#[cfg(test)]
impl ExtToolchain {
    pub(crate) fn interpreter(&self) -> &OsStr {
        &self.interpreter
    }
}

#[cfg(test)]
#[path = "ext_build_tests.rs"]
mod ext_build_tests;
