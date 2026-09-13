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
use pycc_hir::{
    BUILTIN_EXCEPTION_CLASSES, FIRST_USER_EXCEPTION_TYPE_TAG, HirItem, HirModule, Ty,
    is_public_name,
};
use std::collections::HashMap;
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
/// its signature; `docs/RUNTIME.md`'s admissibility matrix is the canonical
/// statement of which types that boundary carries, in which direction, and
/// what each admitted one narrows. `docs/DIAGNOSTICS.md` carries the
/// registry entry and `crates/pycc_diag/src/explain.rs` the long-form
/// explanation.
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
/// and directly in tests (`ExtToolchain::with_probe`) -- never by a test
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
    ///
    /// The directory existing is not enough: `Python.h` itself must be in it.
    /// An interpreter installed without its development package reports a
    /// header directory that is absent or empty, and letting that through
    /// would surface the failure inside the C compiler instead -- which
    /// `docs/CLI_SPEC.md` classifies as a compile error at exit 1, when a
    /// broken CPython development environment is an environment failure at
    /// exit 2.
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
        if !probe.include.join("Python.h").is_file() {
            return Err(format!(
                "CPython header directory `{}` contains no `Python.h`; --ext compiles \
                 against that header, so an interpreter without its development package \
                 installed cannot serve it -- install that package, or set \
                 PYCC_PYTHON_INCLUDE to the directory that does hold `Python.h`",
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
        // the interpreter already occupies, so `-shared` is correct here and
        // a `-lpython` is actively wrong for the same reason as above.
        //
        // `-Bsymbolic` binds every reference this artifact makes to a symbol
        // it defines itself, at link time. Without it, two pycc extension
        // modules loaded with `RTLD_GLOBAL` interpose on each other: ELF
        // resolves a defined global symbol through the *global* lookup
        // scope, so the second module's `pycc_ext_module_exec` and
        // `fnptr_<name>` references reach the first module's definitions and
        // it executes the wrong module's body. CPython imports without
        // `RTLD_GLOBAL` by default, but `sys.setdlopenflags` is public API
        // and some extensions set it, so the artifact cannot rely on the
        // loader's default. Mach-O and PE need no counterpart: a two-level
        // namespace records the defining library per reference, and a PE
        // exports only what `__declspec(dllexport)` names -- and both `ld64`
        // and `link.exe` reject the flag, so it stays on this arm alone.
        ExtLinkPlatform::Linux => vec![OsString::from("-shared"), OsString::from("-Wl,-Bsymbolic")],
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
    /// The declared parameter types, in order. Their count is the arity the
    /// `METH_FASTCALL` wrapper checks, and each one alone picks that
    /// argument's C local, its `pycc_ext_unpack_*` helper and its slot in
    /// the indirect call's cast.
    pub(crate) params: Vec<Ty>,
    /// The declared return type, which picks the cast's return type and the
    /// egress: a `pycc_ext_pack_*` call, or `Py_RETURN_NONE` for `-> None`.
    pub(crate) return_ty: Ty,
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
/// The boundary carries `int`, `float`, `bool`, `str` and a `tuple` of
/// those scalars in either direction, and `None` as a return type only
/// (#1036, #1048, #1049, #1050). `docs/RUNTIME.md`'s admissibility matrix
/// is the canonical statement of that set, including the narrowings each
/// admitted type carries. Every other public signature is a
/// [`EXT_CAPABILITY_CODE`] capability gap, and *all* of them are collected
/// before returning -- one `--ext` build should not have to be re-run once
/// per unsupported function.
///
/// A `tuple` is admitted by its elements and not by its own name: the
/// boundary carries it by spreading it into one scalar slot per element
/// (see [`boundary_carrier`]), so a nested or container-carrying `tuple`
/// stays a gap. That is a restatement of D-116's model -- a tuple type has
/// a fixed arity of `int`/`bool`/`float` elements -- and not a second
/// admissibility rule.
///
/// The export set is derived here, in the driver, rather than carried as a
/// new field on `HirItem::Function`/`MirItem::Function`: those two patterns
/// are constructed at 693 and 174 sites across this workspace, and a new
/// field would put hundreds of mechanically-updated non-test lines into the
/// 100%-coverage denominator for no behavioural gain.
pub(crate) fn collect_exports(module: &HirModule) -> Result<Vec<ExtExport>, Vec<Diagnostic>> {
    let mut exports: Vec<ExtExport> = Vec::new();
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
        let export = ExtExport {
            name: name.clone(),
            params: params.iter().map(|(_, ty)| ty.clone()).collect(),
            return_ty: return_ty.clone(),
        };
        // A module may rebind a public name -- two `def`s, or a `def` over an
        // imported name. Codegen emits exactly one `fnptr_<name>` global and
        // binds it to the *last* definition, so the wrapper table must carry
        // exactly one entry per name, with that definition's signature: a second
        // entry generates a second `pycc_ext_wrap_<name>` and the C compiler
        // rejects the redefinition outright. Replacing in place rather than
        // appending keeps the table in definition order, which is what the
        // generated `.inc` fixtures assert. Cross-*module* collisions cannot
        // reach here -- `pycc_hir`'s import closure rejects a name defined by
        // two inputs with `C0001` first.
        match exports.iter_mut().find(|held| held.name == export.name) {
            Some(held) => *held = export,
            None => exports.push(export),
        }
    }
    if gaps.is_empty() {
        Ok(exports)
    } else {
        Err(gaps)
    }
}

/// What one `Ty` the boundary admits occupies at a generated wrapper's
/// parameter or result position.
///
/// A scalar occupies one C slot. A `tuple` occupies one slot per element
/// and never a slot of its own (#1050): C cannot spell a pycc aggregate,
/// because pycc's own convention for passing and returning one is not the
/// platform C struct ABI. An enum rather than a `Vec` that happens to hold
/// one entry, so every consumer has to say which case it is handling --
/// this table is the entire seam between the driver's C and codegen's
/// LLVM, and a width or a slot count that disagrees with the callee is a
/// silent miscompile, never a compile error on either side.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BoundaryCarrier {
    /// One C slot: its C type, and the `pycc_ext_*` helper suffix that
    /// unpacks and packs a value of it.
    Scalar(&'static str, &'static str),
    /// A `tuple`, as one `Scalar`'s payload per element in declaration
    /// order. D-116 fixes a tuple type's arity, so this is exactly its
    /// element list and never a run-time length.
    Tuple(Vec<(&'static str, &'static str)>),
}

impl BoundaryCarrier {
    /// The single C slot this carrier occupies, or `None` for a `tuple`,
    /// which occupies several.
    fn into_scalar(self) -> Option<(&'static str, &'static str)> {
        match self {
            BoundaryCarrier::Scalar(c_type, helper) => Some((c_type, helper)),
            BoundaryCarrier::Tuple(_) => None,
        }
    }
}

/// The C slots one type the boundary admits uses inside a generated
/// wrapper, or `None` when this pycc version's boundary cannot carry `ty`
/// in either position.
///
/// Each C type is chosen to match exactly what `pycc_codegen`'s
/// `ty_to_basic_type` gives the compiled function, because the wrapper
/// reaches that function through a seam no compiler can check: `Ty::Int`
/// is `i64`, `Ty::Float` is `f64`, and `Ty::Bool` is a one-byte `i8`
/// holding `0`/`1` at the parameter position as well as the return one --
/// hence `char`, and deliberately not `int` or `_Bool`. A width that
/// disagrees with the callee is a silent miscompile here, never a compile
/// error.
///
/// `Ty::Str` is the one scalar entry that is not a number -- hence this
/// function's name, which #1049 widened from `boundary_scalar`.
/// `ty_to_basic_type` gives it an opaque pointer, so the C type is
/// `void *` in both positions and the wrapper never reads through it:
/// `pycc_ext_unpack_str` produces the `PyStrObj` and `pycc_ext_pack_str`
/// consumes it.
///
/// `Ty::Tuple` (#1050) is the one entry that is not a single slot at all.
/// Its elements are looked up through this same function, so their widths
/// come from the same table rather than a parallel one, but only D-116's
/// three element types are admitted: a tuple carrying anything else -- a
/// nested tuple, `tuple[list[int]]`, or `tuple[str, int]`, none of which
/// a `T0039`-checked program can express -- answers `None` exactly as any
/// other uncarriable type does.
fn boundary_carrier(ty: &Ty) -> Option<BoundaryCarrier> {
    match ty {
        Ty::Int => Some(BoundaryCarrier::Scalar("long long", "int")),
        Ty::Float => Some(BoundaryCarrier::Scalar("double", "float")),
        Ty::Bool => Some(BoundaryCarrier::Scalar("char", "bool")),
        Ty::Str => Some(BoundaryCarrier::Scalar("void *", "str")),
        Ty::Tuple(elements) => elements
            .iter()
            .map(|element| match element {
                // The one type the two admissibility questions answer
                // differently, and so the one arm `into_scalar` cannot
                // decide. `str` occupies a single C slot at a top-level
                // position (#1049), so it would pass `into_scalar`
                // unchanged -- but the element shims are the `_at`
                // variants, which take an element index and exist only
                // for D-116's three element types. Admitting
                // `tuple[str, int]` here would render C naming an
                // undeclared `pycc_ext_unpack_str_at`: a clang error on
                // the generated artifact instead of the `C0003`
                // capability gap every other uncarriable shape gets.
                // Unreachable from source today, exactly as
                // `tuple[list[int]]` is -- `T0039` refuses the
                // annotation -- which is why it is stated rather than
                // left to a recursion that happens to work.
                Ty::Str => None,
                // Every other element goes through this same function,
                // so its width comes from the table above rather than a
                // parallel one, and a `list` element (no carrier at all)
                // or a nested tuple (a carrier, but not a single slot)
                // answers `None` on its own.
                _ => boundary_carrier(element).and_then(BoundaryCarrier::into_scalar),
            })
            .collect::<Option<Vec<_>>>()
            .map(BoundaryCarrier::Tuple),
        _ => None,
    }
}

/// Whether the boundary can carry `ty` at a parameter position.
///
/// `Ty::None` is deliberately not admitted: a `None` parameter stays a
/// capability gap, gated on #1047's call-argument ICE. Everything else the
/// boundary carries at all, it carries in both directions, so this is
/// [`boundary_carrier`] with no further narrowing -- the asymmetry lives
/// entirely in [`return_c_type`].
fn carries_param(ty: &Ty) -> bool {
    boundary_carrier(ty).is_some()
}

/// The C return type of a wrapper's call into the compiled function, or
/// `None` when the boundary cannot carry `ty` as a return type.
///
/// Two types answer `void`, for different reasons. Codegen emits a `None`
/// return as LLVM `void`, so there is nothing to receive at all and the
/// wrapper's egress becomes `Py_RETURN_NONE`. A `tuple` return does carry
/// values, but they leave through `pycc_ext_thunk_<name>`'s trailing
/// out-pointers rather than as a return value (#1050), so the call itself
/// is still `void` and the elements are read out of the wrapper's own
/// locals afterwards.
fn return_c_type(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::None => Some("void"),
        // Still asked of `boundary_carrier`: `tuple[list[int]]` is a tuple
        // whose element the boundary cannot carry, and answering `void`
        // for it unconditionally would admit a signature no wrapper can
        // unpack.
        Ty::Tuple(_) => boundary_carrier(ty).map(|_| "void"),
        _ => boundary_carrier(ty)
            .and_then(BoundaryCarrier::into_scalar)
            .map(|(c_type, _)| c_type),
    }
}

/// Names the first part of a signature the `ext` boundary cannot carry, as
/// the user would write it (`x: str`, `-> list`), or `None` when the whole
/// signature is admissible.
///
/// Parameters and the return type are asked separately because the two
/// admissible sets genuinely differ rather than sharing one widened list:
/// see [`carries_param`] and [`return_c_type`].
fn unsupported_boundary_ty(params: &[(String, Ty)], return_ty: &Ty) -> Option<String> {
    if let Some((name, ty)) = params.iter().find(|(_, ty)| !carries_param(ty)) {
        return Some(format!("parameter `{name}: {}`", render_ty(ty)));
    }
    if return_c_type(return_ty).is_none() {
        return Some(format!("return type `-> {}`", render_ty(return_ty)));
    }
    None
}

/// A short Python-facing spelling of a `Ty`, for the `C0003` message only.
///
/// Deliberately coarse: the reader's fix is to change the signature, and
/// the element type of the container that was refused adds nothing to that.
/// The `Ty::Tuple(_)` arm survives #1050 rather than becoming dead, because
/// a `tuple` is admitted by its elements: `tuple[list[int]]` and a nested
/// `tuple[tuple[int], int]` still reach this function, and both are named
/// `tuple` -- the element that failed is the same one D-116 already forbids
/// spelling in a type annotation (T0039), so naming it would point at a
/// program that cannot be written.
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
            "--ext cannot export the public function `{name}`: its {offender} is not a type \
             this pycc version's CPython boundary can carry -- a parameter must be `int`, \
             `float`, `bool`, `str` or a `tuple` of `int`/`float`/`bool`, and a return type \
             must be one of those or `None` \
             (D-244 rule \
             1 exports every public module-level function, so there is no way to opt one \
             out) -- rename it to `_{name}` to keep it out of the export set, or build \
             without --ext"
        ),
        span: None,
        label: None,
        help: None,
    }
}

/// The exact C declaration of the generated tag-to-class lookup.
///
/// Shared on purpose (risk R3 of this change's plan): the *definition* is
/// emitted here while the forward declaration is hand-written in
/// [`SHIM_C`], and nothing links the two until a C compiler runs inside an
/// `#[ignore]`d test the coverage job never executes. Both the shim test
/// and the generated-text test assert this one constant, so a spelling or
/// parameter-type drift fails an ordinary `cargo test` instead.
pub(crate) const USER_EXCEPTION_LOOKUP_DECL: &str =
    "static PyObject *pycc_ext_user_exception_class(unsigned char tag)";

/// The exact C declaration of the generated eager-registration entry point,
/// called from `pycc_ext_exec_module` after the `.inc` is included (so it
/// needs no forward declaration, unlike [`USER_EXCEPTION_LOOKUP_DECL`]).
pub(crate) const USER_EXCEPTION_REGISTER_DECL: &str =
    "static int pycc_ext_register_exception_classes(PyObject *module)";

/// The two PEP 654 group classes. A class whose MRO reaches either one is
/// excluded from the table by [`collect_user_exception_classes`]: the
/// limited C API exposes no `PyExc_ExceptionGroup`, and PEP 654 requires
/// `(msg, exceptions)`, so a synthesized stand-in would be a fake group
/// class rather than CPython's. Such a class keeps today's honest
/// `Exception`; `docs/RUNTIME.md` records the residual.
const GROUP_EXCEPTION_CLASSES: [&str; 2] = ["BaseExceptionGroup", "ExceptionGroup"];

/// One base of a synthesized user exception class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExceptionBase {
    /// A seeded builtin exception class, spelled `PyExc_<name>` host-side.
    /// The `&'static str` is the [`BUILTIN_EXCEPTION_CLASSES`] entry itself,
    /// so the rendered spelling cannot drift from the array.
    Builtin(&'static str),
    /// Another entry of the same table, by its slot index. Always a lower
    /// slot than the class that names it: `resolve_mro` requires a base to
    /// be defined before its subclass, and `finalize` assigns tags in that
    /// same program order.
    User(usize),
}

/// One user-defined exception class the artifact synthesizes on the host
/// side, in the order the generated registration function creates them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserExceptionClass {
    /// The runtime tag `pycc_ext_raise_pending` receives for an instance of
    /// this class. Assigned by `pycc_hir::program::finalize` in program
    /// order, so it shifts on any source edit: it is regenerated with every
    /// artifact and never persisted in a fixture, a checked-in file, or the
    /// hand-written shim.
    pub(crate) tag: u8,
    /// The class's own (unqualified) Python name.
    pub(crate) name: String,
    /// The exception bases, in declaration order. Provably non-empty: a
    /// class earns a tag only when its MRO reaches a builtin exception
    /// class, which requires at least one exception base.
    pub(crate) bases: Vec<ExceptionBase>,
}

/// The per-program table of user exception classes the artifact
/// synthesizes, in ascending tag order.
///
/// Membership is decided here and nowhere else (correction C3 of this
/// change's plan): the C shim performs no tag arithmetic of its own, so a
/// class this function omits simply misses the generated lookup and keeps
/// `PyExc_Exception`.
///
/// Selection is `exception_type_tag == Some(t)` with
/// `t >= FIRST_USER_EXCEPTION_TYPE_TAG`. Neither `None` nor a smaller
/// `Some` is a user class: `crates/pycc_hir/src/class.rs` documents that
/// trap directly -- the flat seven builtins carry `None` and the PEP 3151
/// `OSError` family carries a fixed `Some`. Group-derived classes are
/// excluded by [`GROUP_EXCEPTION_CLASSES`], and a monomorphized generic
/// specialization is untagged by construction and never appears here.
///
/// Bases come from `def.bases`, never from the linearized `def.mro`:
/// `class E(MyBase, ValueError)` is legal and is caught natively by
/// `except ValueError:`, so taking the first exception ancestor out of the
/// MRO would pick `MyBase` alone and drop `ValueError` -- exactly the
/// host-side mismatch this table exists to remove. A non-exception base (a
/// method-only mixin) is dropped, which `docs/RUNTIME.md` records as a
/// residual.
pub(crate) fn collect_user_exception_classes(module: &HirModule) -> Vec<UserExceptionClass> {
    let selected: Vec<(&str, &[String], u8)> = module
        .class_defs
        .iter()
        .filter_map(|(name, def)| {
            let tag = def.exception_type_tag?;
            let derives_from_group = def
                .mro
                .iter()
                .any(|ancestor| GROUP_EXCEPTION_CLASSES.contains(&ancestor.as_str()));
            (tag >= FIRST_USER_EXCEPTION_TYPE_TAG && !derives_from_group).then_some((
                name.as_str(),
                def.bases.as_slice(),
                tag,
            ))
        })
        .collect();
    let slots: HashMap<&str, usize> = selected
        .iter()
        .enumerate()
        .map(|(slot, (name, _, _))| (*name, slot))
        .collect();
    selected
        .iter()
        .map(|(name, bases, tag)| UserExceptionClass {
            tag: *tag,
            name: (*name).to_string(),
            bases: bases
                .iter()
                .filter_map(|base| {
                    BUILTIN_EXCEPTION_CLASSES
                        .iter()
                        .copied()
                        .find(|builtin| *builtin == base.as_str())
                        .map(ExceptionBase::Builtin)
                        .or_else(|| slots.get(base.as_str()).copied().map(ExceptionBase::User))
                })
                .collect(),
        })
        .collect()
}

/// The C expression naming one base class at registration time.
fn base_expression(base: &ExceptionBase) -> String {
    match base {
        ExceptionBase::Builtin(name) => format!("PyExc_{name}"),
        ExceptionBase::User(slot) => format!("pycc_ext_user_exception_classes[{slot}]"),
    }
}

/// One table entry's registration: create the class if the cache slot is
/// empty, then publish it as a module attribute.
///
/// The cache owns the strong reference `PyErr_NewException` returns and
/// `PyModule_AddObjectRef` takes its own, so neither needs a compensating
/// decref. The slot is assigned *before* `AddObjectRef` is checked, so a
/// failing `AddObjectRef` cannot leak the class. A non-`NULL` slot is
/// reused rather than re-minted: deleting the `sys.modules` entry and
/// re-importing is the one path that re-runs `Py_mod_exec`
/// (`docs/RUNTIME.md`), and reuse keeps class identity stable across it.
/// A mid-registration failure leaves the already-created classes cached, so
/// a later import completes the remaining slots.
fn register_class_c(slot: usize, class: &UserExceptionClass) -> String {
    let name = &class.name;
    let mut out = format!("    if (pycc_ext_user_exception_classes[{slot}] == NULL) {{\n");
    // A single base is passed directly; two or more need a `PyTuple`, which
    // `PyErr_NewException` accepts and which is what gives the synthesized
    // class the same `__mro__` the native side matches on.
    let (base, release) = match class.bases.as_slice() {
        [single] => (base_expression(single), ""),
        several => {
            let packed: Vec<String> = several.iter().map(base_expression).collect();
            out.push_str(&format!(
                "        PyObject *bases = PyTuple_Pack({}, {});\n",
                several.len(),
                packed.join(", ")
            ));
            out.push_str("        if (bases == NULL) {\n            return -1;\n        }\n");
            ("bases".to_string(), "        Py_DECREF(bases);\n")
        }
    };
    out.push_str(&format!(
        "        pycc_ext_user_exception_classes[{slot}] = PyErr_NewException(\n            \
         PYCC_EXT_MODULE_NAME_STR \".{name}\", {base}, NULL);\n"
    ));
    out.push_str(release);
    out.push_str(&format!(
        "        if (pycc_ext_user_exception_classes[{slot}] == NULL) {{\n            \
         return -1;\n        }}\n    }}\n"
    ));
    out.push_str(&format!(
        "    if (PyModule_AddObjectRef(module, \"{name}\", \
         pycc_ext_user_exception_classes[{slot}]) < 0) {{\n        return -1;\n    }}\n"
    ));
    out
}

/// The user-exception-class half of the generated companion: the cache, the
/// tag lookup `pycc_ext_raise_pending` calls, and the eager registration
/// `pycc_ext_exec_module` calls.
///
/// Both functions are emitted unconditionally -- with empty bodies when the
/// program declares no user exception class -- so every artifact links.
///
/// Entries are keyed by **explicit tag** in a `switch`, not by an offset
/// into the cache: the tag space is sparse relative to the table, because
/// `finalize` consumes a tag for a group-derived class that
/// [`collect_user_exception_classes`] excludes. An array indexed by
/// `tag - FIRST_USER_EXCEPTION_TYPE_TAG` and sized by entry count would
/// then send a later class past its bounds and silently flatten it to
/// `Exception` with no compile or link error; a `switch` makes that
/// unrepresentable rather than merely tested against.
fn exception_classes_c(classes: &[UserExceptionClass]) -> String {
    let mut out = String::new();
    if classes.is_empty() {
        out.push_str(&format!(
            "{USER_EXCEPTION_LOOKUP_DECL}\n{{\n    (void)tag;\n    return NULL;\n}}\n\n"
        ));
        out.push_str(&format!(
            "{USER_EXCEPTION_REGISTER_DECL}\n{{\n    (void)module;\n    return 0;\n}}\n\n"
        ));
        return out;
    }
    out.push_str(&format!(
        "/* Synthesized user exception classes, created once during \
         `Py_mod_exec`.\n * File-scope statics are licensed by the shim's \
         refusal of both\n * subinterpreters and free-threaded hosts. */\n\
         static PyObject *pycc_ext_user_exception_classes[{}];\n\n",
        classes.len()
    ));
    out.push_str(&format!(
        "{USER_EXCEPTION_LOOKUP_DECL}\n{{\n    switch (tag) {{\n"
    ));
    for (slot, class) in classes.iter().enumerate() {
        out.push_str(&format!(
            "    case {}:\n        return pycc_ext_user_exception_classes[{slot}];\n",
            class.tag
        ));
    }
    out.push_str("    default:\n        return NULL;\n    }\n}\n\n");
    out.push_str(&format!("{USER_EXCEPTION_REGISTER_DECL}\n{{\n"));
    for (slot, class) in classes.iter().enumerate() {
        out.push_str(&register_class_c(slot, class));
    }
    out.push_str("    return 0;\n}\n\n");
    out
}

/// Renders the generated C companion to [`SHIM_C`]: the module name macros,
/// the user-exception-class table ([`exception_classes_c`]), one
/// `METH_FASTCALL` wrapper per export, and the `PyMethodDef` table.
///
/// `module_name` is already known to be a valid ASCII Python identifier
/// (`ext_output::resolve` rejects everything else before this runs), and an
/// export name is a Python identifier by construction, so neither can carry
/// a character that would escape the C source it is pasted into.
pub(crate) fn generate_exports_inc(
    module_name: &str,
    exports: &[ExtExport],
    classes: &[UserExceptionClass],
) -> String {
    let mut out = String::new();
    out.push_str("/* Generated by pycc --ext. Do not edit: see src/ext_build.rs. */\n");
    out.push_str(&format!("#define PYCC_EXT_MODULE_NAME {module_name}\n"));
    out.push_str(&format!(
        "#define PYCC_EXT_MODULE_NAME_STR \"{module_name}\"\n\n"
    ));
    out.push_str(&exception_classes_c(classes));
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
/// A scalar-only signature's call goes through `fnptr_<name>`, the
/// module-level function-pointer global codegen emits for each `def`'s
/// binding, and *not* through the compiled function's own symbol. That is
/// what makes a rebound name (`f = g` at module level) call what Python
/// says it calls.
///
/// A signature carrying a `tuple` calls `pycc_ext_thunk_<name>` instead
/// (#1050), which dispatches through that same global one LLVM frame
/// further in and so keeps the rebinding property. The indirection exists
/// because C cannot spell a pycc aggregate: pycc's own convention for
/// passing and returning one is not the platform C struct ABI, so the thunk
/// presents the signature as scalars and out-pointers and every aggregate
/// stays on the LLVM side. That declaration is a real `extern` *function*
/// declaration and the call is direct -- deliberately not the scalar path's
/// `void *` global cast to a function-pointer type. The cast form was
/// measured to fault (SIGBUS) on aarch64-apple-darwin for an out-pointer
/// signature, and it is also the only one of the two that no C compiler can
/// type-check at all.
fn wrapper_for(export: &ExtExport) -> String {
    let name = &export.name;
    let arity = export.params.len();
    // Every local, unpack helper and call slot below is a function of the
    // declared type alone and never of the object that arrives: `def
    // f(x: int)` gets `long long a0` and `pycc_ext_unpack_int` even though
    // that helper also accepts `True`, because widening the accepted object
    // set is the helper's business and never narrows the local.
    // `collect_exports` refused every type these two lookups cannot name, so
    // an export in hand always has both.
    let slots: Vec<BoundaryCarrier> = export
        .params
        .iter()
        .map(|ty| boundary_carrier(ty).expect("collect_exports admits only carriable parameters"))
        .collect();
    let return_c = return_c_type(&export.return_ty).expect("a carriable return type");
    let returns_none = export.return_ty == Ty::None;
    // One entry per element of a returned `tuple`, and empty for every
    // other return type: these become trailing out-pointer arguments, not
    // a return value.
    let out_slots: Vec<(&'static str, &'static str)> = match boundary_carrier(&export.return_ty) {
        Some(BoundaryCarrier::Tuple(elements)) => elements,
        _ => Vec::new(),
    };
    // #1050 keeps two index spaces apart on purpose. `arity` and every
    // `a{index}` below are per *Python argument* -- what `nargs` counts,
    // what the arity message names, and what an unpack failure has to clean
    // up after -- while the C call's own argument list is the flattened
    // one, built only at the call expression further down. Renumbering
    // `a{index}` to follow the flattened list would make
    // `def f(t: tuple[int, int])` report "takes exactly 2 arguments" for a
    // one-argument function.
    let use_thunk = pycc_codegen::ext_thunk_required(name, &export.params, &export.return_ty);
    let thunk = pycc_codegen::ext_thunk_symbol(name);
    let params = c_param_list(&slots, &out_slots);
    let mut out = String::new();
    if use_thunk {
        out.push_str(&format!("extern {return_c} {thunk}({params});\n"));
    } else {
        out.push_str(&format!("extern void *fnptr_{name};\n"));
    }
    out.push_str(&format!(
        "static PyObject *pycc_ext_wrap_{name}(PyObject *self, PyObject *const *args, \
         Py_ssize_t nargs)\n{{\n"
    ));
    out.push_str("    (void)self;\n");
    out.push_str("    (void)args;\n");
    // A `-> None` export has no result to hold: codegen emits its return as
    // LLVM `void`, so a result local would be a C type error, not a waste.
    // A `tuple` return has no single result either -- its elements arrive
    // in the `r{index}` locals below, through the thunk's out-pointers.
    if !returns_none && out_slots.is_empty() {
        out.push_str(&format!("    {return_c} result;\n"));
    }
    for (index, (c_type, _)) in out_slots.iter().enumerate() {
        out.push_str(&format!("    {c_type} r{index};\n"));
        out.push_str(&format!("    PyObject *e{index};\n"));
    }
    if !out_slots.is_empty() {
        out.push_str("    PyObject *packed;\n");
    }
    for (index, slot) in slots.iter().enumerate() {
        match slot {
            BoundaryCarrier::Scalar(c_type, _) => {
                out.push_str(&format!("    {c_type} a{index};\n"));
            }
            BoundaryCarrier::Tuple(elements) => {
                for (element, (c_type, _)) in elements.iter().enumerate() {
                    out.push_str(&format!("    {c_type} a{index}_{element};\n"));
                }
            }
        }
    }
    out.push_str(&format!(
        "    if (nargs != {arity}) {{\n        PyErr_Format(PyExc_TypeError, \
         \"{name}() takes exactly {arity} argument{plural} (%zd given)\", nargs);\n        \
         return NULL;\n    }}\n",
        plural = if arity == 1 { "" } else { "s" },
    ));
    for (index, slot) in slots.iter().enumerate() {
        // Each `str` argument already unpacked holds a fresh reference that
        // only the compiled function's own parameter slot ever consumes, and
        // this branch bails before the call -- so release them here, or a
        // `TypeError` on argument 2 would leak argument 1's `PyStrObj` on
        // every raising call. Emitted inline rather than behind a shared
        // `goto` label: the cleanup differs per argument index, and the
        // wrapper has no other exit that owes anything. A `tuple` argument
        // owes nothing: its elements are copied out by value.
        let cleanup: String = slots[..index]
            .iter()
            .enumerate()
            .filter(|(_, earlier)| matches!(earlier, BoundaryCarrier::Scalar(_, "str")))
            .map(|(earlier, _)| format!("        pycc_rt_str_decref(a{earlier});\n"))
            .collect();
        match slot {
            BoundaryCarrier::Scalar(_, helper) => out.push_str(&format!(
                "    if (pycc_ext_unpack_{helper}(args[{index}], \"{name}\", {index}, &a{index}) \
                 != 0) {{\n{cleanup}        return NULL;\n    }}\n"
            )),
            BoundaryCarrier::Tuple(elements) => {
                let elements_len = elements.len();
                out.push_str(&format!(
                    "    if (pycc_ext_unpack_tuple(args[{index}], \"{name}\", {index}, \
                     {elements_len}) != 0) {{\n{cleanup}        return NULL;\n    }}\n"
                ));
                for (element, (_, helper)) in elements.iter().enumerate() {
                    // `PyTuple_GetItem` cannot fail at this call: the check
                    // just emitted refused every non-tuple and every length
                    // but this one, so the index is always in range.
                    out.push_str(&format!(
                        "    if (pycc_ext_unpack_{helper}_at(PyTuple_GetItem(args[{index}], \
                         {element}), \"{name}\", {index}, {element}, &a{index}_{element}) != 0) \
                         {{\n{cleanup}        return NULL;\n    }}\n"
                    ));
                }
            }
        }
    }
    let mut call_args: Vec<String> = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        match slot {
            BoundaryCarrier::Scalar(..) => call_args.push(format!("a{index}")),
            BoundaryCarrier::Tuple(elements) => {
                call_args.extend((0..elements.len()).map(|element| format!("a{index}_{element}")))
            }
        }
    }
    call_args.extend((0..out_slots.len()).map(|index| format!("&r{index}")));
    let call_args = call_args.join(", ");
    // Nothing is assigned on the `-> None` arm (a `void` call has no value)
    // nor on the `tuple` arm (its elements arrive through the out-pointers).
    let assign = if returns_none || !out_slots.is_empty() {
        ""
    } else {
        "result = "
    };
    if use_thunk {
        out.push_str(&format!("    {assign}{thunk}({call_args});\n"));
    } else {
        out.push_str(&format!(
            "    {assign}(({return_c} (*)({params}))fnptr_{name})({call_args});\n"
        ));
    }
    // A compiled function that raised returns a neutral carrier and leaves
    // the runtime's thread-local flag set (see `pycc_codegen`'s
    // `exception_exit` block), so the carrier must never be packed: the
    // pending exception is checked first and translated into a CPython one.
    // This reads no part of the call's return value, so it stands unchanged
    // on the `-> None` arm -- where it is the only thing between a raised
    // exception and a fabricated `None`.
    //
    // #1050: its *position*, before the pack below, is load-bearing for a
    // `tuple` return in a way it is not for a scalar one. A raising call
    // leaves every `r{index}` out-pointer local exactly as uninitialized as
    // it found it, so a pack that ran first would read indeterminate
    // storage -- undefined behaviour, not merely a wrong value.
    out.push_str(
        "    if (pycc_rt_ext_pending_type() >= 0) {\n        pycc_ext_raise_pending();\n        \
         return NULL;\n    }\n",
    );
    match &export.return_ty {
        Ty::None => out.push_str("    Py_RETURN_NONE;\n}\n\n"),
        Ty::Tuple(_) => out.push_str(&pack_tuple_return(name, &out_slots)),
        // `pack_int` is the one packer whose failure is a property of the
        // *value*, and the only one whose message therefore names the
        // function: D-141's bigint egress (#1040). `PyFloat_FromDouble` and
        // `PyBool_FromLong` cannot fail at all, and `pack_str` can only fail
        // the way any allocation can -- it refuses no `str` -- so none of
        // the three take a name. Arity is uniform across them, so every
        // packer but `int` shares the generic arm below.
        Ty::Int => out.push_str(&format!(
            "    return pycc_ext_pack_int(\"{name}\", result);\n}}\n\n"
        )),
        ty => {
            let (_, helper) = boundary_carrier(ty)
                .and_then(BoundaryCarrier::into_scalar)
                .expect("a carriable scalar return type");
            out.push_str(&format!(
                "    return pycc_ext_pack_{helper}(result);\n}}\n\n"
            ));
        }
    }
    out
}

/// The C parameter-type list of the call a wrapper makes into the compiled
/// program: every declared parameter flattened to the slots it occupies,
/// then one out-pointer per element of a returned `tuple`.
///
/// `"void"` and not `""` for the empty list, because an empty C parameter
/// list means "unspecified", not "none". The rule applies to the *combined*
/// list: only a nullary export with no `tuple` return has one.
fn c_param_list(slots: &[BoundaryCarrier], out_slots: &[(&'static str, &'static str)]) -> String {
    let mut types: Vec<String> = Vec::new();
    for slot in slots {
        match slot {
            BoundaryCarrier::Scalar(c_type, _) => types.push((*c_type).to_string()),
            BoundaryCarrier::Tuple(elements) => {
                types.extend(elements.iter().map(|(c_type, _)| (*c_type).to_string()));
            }
        }
    }
    types.extend(out_slots.iter().map(|(c_type, _)| format!("{c_type} *")));
    if types.is_empty() {
        "void".to_string()
    } else {
        types.join(", ")
    }
}

/// The egress of a `tuple`-returning wrapper (#1050): pack every element,
/// then build the tuple.
///
/// Each `r{index}` arrives **borrowed**, not retained. D-180 rule 6 retains
/// at a `return` only where the returned value is a scalar: codegen's
/// `MirStmt::Return` routes the value through `retain_if_int_duplicate`,
/// which acts on a `Scalar::Int` and does nothing for an aggregate, and the
/// `pycc_ext_thunk_` emitter then `extractvalue`s each field straight into
/// its out-pointer. So returning a *stored* tuple (`saved = (2**62,)`;
/// `return saved`) hands this function a word the module global still owns.
/// `pycc_ext_pack_int` discharges one reference on its `OverflowError` path,
/// which without a matching retain here decrements a count this wrapper
/// never took -- a refcount underflow, and on the next call a use-after-free
/// in the host interpreter.
///
/// So each `int` element takes its own reference with `pycc_rt_bigint_retain`
/// immediately before the packer that discharges it. Retain and release share
/// one predicate (`classify_encoded_int(word) == BigInt`), so the pairing is
/// exactly balanced on a smallint, a bool marker, the word `0`, and an
/// unclassifiable word alike -- and the retain is emitted from the same
/// `helper == "int"` arm as the packer, so the two can never drift apart.
///
/// Every element is packed *unconditionally*, before any failure is acted
/// on, and that ordering is the refcount discipline rather than a style
/// choice. Bailing out at the first failing element would leave every later
/// element's word undischarged, leaking one `BigIntObj` per call on exactly
/// the path that already raises.
///
/// The cost is that when two `int` elements both overflow, the second
/// `PyErr_Format` replaces the first. Both carry the same message text and
/// the same exception type, so the observable difference is nil, and
/// replacing a pending exception is well-defined in CPython -- unlike
/// dropping an owned word.
///
/// `PyTuple_New` is reached only once every element is packed, so its own
/// failure path has a fixed, fully-owned set to release.
fn pack_tuple_return(name: &str, out_slots: &[(&'static str, &'static str)]) -> String {
    let mut out = String::new();
    for (index, (_, helper)) in out_slots.iter().enumerate() {
        let argument = if *helper == "int" {
            out.push_str(&format!("    pycc_rt_bigint_retain(r{index});\n"));
            format!("\"{name}\", r{index}")
        } else {
            format!("r{index}")
        };
        out.push_str(&format!(
            "    e{index} = pycc_ext_pack_{helper}({argument});\n"
        ));
    }
    let arity = out_slots.len();
    let any_null = (0..arity)
        .map(|index| format!("e{index} == NULL"))
        .collect::<Vec<_>>()
        .join(" || ");
    let x_release: String = (0..arity)
        .map(|index| format!("        Py_XDECREF(e{index});\n"))
        .collect();
    out.push_str(&format!(
        "    if ({any_null}) {{\n{x_release}        return NULL;\n    }}\n"
    ));
    let release: String = (0..arity)
        .map(|index| format!("        Py_DECREF(e{index});\n"))
        .collect();
    out.push_str(&format!(
        "    packed = PyTuple_New({arity});\n    if (packed == NULL) {{\n{release}        \
         return NULL;\n    }}\n"
    ));
    out.push_str(
        "    /* Every index is in range and `packed` is a fresh tuple, so each\n       \
         PyTuple_SetItem succeeds and steals its element reference. */\n",
    );
    for index in 0..arity {
        out.push_str(&format!(
            "    PyTuple_SetItem(packed, {index}, e{index});\n"
        ));
    }
    out.push_str("    return packed;\n}\n\n");
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
#[path = "ext_build_tests/mod.rs"]
mod ext_build_tests;
