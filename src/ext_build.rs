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
use pycc_diag::Diagnostic;
use pycc_hir::{
    BUILTIN_EXCEPTION_CLASSES, FIRST_USER_EXCEPTION_TYPE_TAG, HirClassDef, HirItem, HirModule, Ty,
    flat_attr_layout, is_builtin_exception_class, is_public_name,
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

/// One export: a public module-level function, or a public
/// `@staticmethod`/`@classmethod` of a public class.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtExport {
    /// The compiled program's own name for the function -- a plain
    /// identifier for a module-level `def`, and `pycc_hir::class`'s mangled
    /// `<Class>.<method>.static` / `<Class>.<method>.classmethod` for a
    /// method. Every C identifier built from it goes through
    /// `pycc_codegen::mangle_ext_name` first; it is *not* the host-visible
    /// name for a method.
    pub(crate) name: String,
    /// The owning class, or `None` for a module-level function. A method's
    /// host-visible name is `mod.<class>.<method>` -- a `PyMethodDef` entry
    /// in that class's own table -- so two classes may carry the same
    /// [`ExtExport::method`] without colliding.
    pub(crate) class: Option<String>,
    /// The bare method name, which is the `PyMethodDef` `ml_name` for a
    /// method. `None` exactly when [`ExtExport::class`] is `None`, in which
    /// case [`ExtExport::name`] is itself the `ml_name`.
    pub(crate) method: Option<String>,
    /// Which leading receiver pointer the compiled function takes, and what
    /// the wrapper must supply for it.
    ///
    /// [`ExtReceiver::NullCls`] for a `@classmethod`: `pycc_hir::class`
    /// injects `cls: Ty::Instance(Class)` as its first parameter, and
    /// `MirExpr::NullInstance` records that every native `Class.method(...)`
    /// call site passes a null pointer for it, because a method compiled for
    /// one class resolves `cls.attr` at compile time and never dereferences
    /// it. The wrapper does the same, and so discards the *type object*
    /// CPython hands `METH_CLASS` in `self` -- writing that pointer into a
    /// slot typed `Ty::Instance` would be type confusion even though nothing
    /// dereferences it today.
    ///
    /// [`ExtReceiver::SelfInstance`] for an instance method (#1145), whose
    /// leading `self` *is* dereferenced: the wrapper unwraps the host
    /// carrier object's inner `PyInstanceObj` and passes that. Both spell
    /// the same `void *` in the declaration; only the call argument differs.
    ///
    /// [`ExtReceiver::None`] for a module-level `def` and a
    /// `@staticmethod`, which declare no receiver at all.
    pub(crate) receiver: ExtReceiver,
    /// The declared parameter types, in order, **excluding** a
    /// [`ExtExport::receiver`]. Their count is the arity the
    /// `METH_FASTCALL` wrapper checks, and each one alone picks that
    /// argument's C local, its `pycc_ext_unpack_*` helper and its slot in
    /// the indirect call's cast.
    ///
    /// The receiver is dropped here and reinstated *textually* in
    /// [`wrapper_for`], because every consumer of this field is
    /// arity-shaped or carrier-shaped and neither can represent it:
    /// `boundary_carrier` has no `Ty::Instance` arm, so leaving it in would
    /// panic. It is reinstated rather than simply dropped because
    /// `pycc_codegen`'s thunk builds its own parameter list from the MIR
    /// function's parameters, which *do* include the receiver -- declaring
    /// different arities on the two sides of one symbol is the silent ABI
    /// mismatch the thunk exists to prevent.
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
/// D-244 rule 1's export set also reaches a public `@staticmethod`,
/// `@classmethod` and -- since #1145 -- instance method of a public,
/// non-exception class. Such a method reaches `HirItem::Function` under
/// `pycc_hir::class`'s mangled `<Class>.<method>.static` /
/// `<Class>.<method>.classmethod` / bare `<Class>.<method>` name, and its
/// host-visible name is `mod.<Class>.<method>` -- a `PyMethodDef` entry in
/// that class's own `PyType_FromSpec` type object, never a flat
/// `mod.<Class>.<method>` module attribute. [`classify_export_name`] owns
/// the lexical half of that verdict.
///
/// An instance method is exported only from a class some host-obtainable
/// instance can be a receiver for -- [`instance_methods_reachable`] is the
/// canonical statement of that predicate -- because a method no instance
/// can ever reach would be an unreachable entry. That ordering is also what
/// bounds the new `C0003` set: a class no constructible class inherits
/// contributes no new gaps at all, so an `@abstractmethod`'s stub body and
/// a `@property` getter are **excluded as representation, before
/// [`unsupported_boundary_ty`] is consulted**, exactly as the suffixed
/// spellings that preceded them were.
///
/// "Public" is D-038's predicate, `pycc_hir::is_public_name`, applied to
/// the class name and the method name alike. The exclusions that are not
/// policy but representation:
///
/// * a monomorphized generic specialization carries the `0gen_` prefix and
///   has no `fnptr_` global to call through (codegen dispatches those
///   directly);
/// * `<Class>.<property>.setter` -- a `@property` is attribute syntax on
///   the host side, not a method;
/// * a `@property` **getter**, which shares the bare `<Class>.<method>`
///   spelling with an ordinary instance method and is told apart here by
///   `HirClassDef::properties`, whose `getter` field holds exactly that
///   mangled name;
/// * every instance method of a class [`instance_methods_reachable`]
///   refuses, which is what removes an `@abstractmethod`'s stub body: such
///   a method survives lowering only on an `is_abstract` class (a class
///   carrying its own `@abstractmethod` without an `ABC` base is rejected
///   with `C0001` by `crates/pycc_hir/src/class.rs`'s unoverridden-abstract
///   check), and that predicate refuses an `is_abstract` class outright --
///   unlike an unconstructible `__init__` shape, which a constructible
///   subclass does rescue. The exclusion story is therefore *"excluded
///   because no host instance can ever receive it"*, never *"excluded
///   because the method is abstract"*.
///
/// Routing any of them through the gap collector instead would turn a
/// public ABC into a build failure on a signature the boundary carries
/// perfectly well.
///
/// A class whose HIR carries an `exception_type_tag` publishes no type
/// object and exports no method. [`register_class_c`] already publishes
/// such a class under its **bare class name** as a module attribute, so a
/// second `PyModule_AddObjectRef` under that name would replace a working
/// exception class with a non-instantiable type -- the host then gets
/// `TypeError: catching classes that do not inherit from BaseException`.
/// The exclusion is scoped on the HIR tag and deliberately *not* on
/// [`collect_user_exception_classes`]' selector, which additionally drops
/// group-derived classes: a `class MyGroup(ExceptionGroup)` is never
/// registered, so scoping there would publish `mod.MyGroup` as a
/// non-instantiable non-`BaseException` type standing in for a user
/// exception class -- the same wrong-published-name defect from the other
/// side. This filter is applied in the driver, layered *on top of*
/// [`classify_export_name`]'s lexical verdict rather than inside it, which
/// keeps that verdict mirror-comparable with
/// `pycc_codegen::is_ext_exportable_name` (work item 8's parity test) and
/// makes the mirror a harmless superset.
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
        let Some(spelling) = classify_export_name(name) else {
            continue;
        };
        // The driver-only exception-class filter, layered on top of the
        // lexical verdict rather than inside it -- see this function's doc.
        if let ExportName::Method { class, .. } = &spelling
            && module
                .class_defs
                .iter()
                .any(|(held, def)| held == class && def.exception_type_tag.is_some())
        {
            continue;
        }
        // #1145's two driver filters, in the same position and form as the
        // exception-class one above and, like it, *before*
        // `unsupported_boundary_ty` -- so neither exclusion can become a
        // `C0003`. Both apply only to the bare spelling: a `@staticmethod`
        // and a `@classmethod` are published on a non-constructible class
        // exactly as Part 1 published them.
        if let ExportName::Method {
            class,
            receiver: ExtReceiver::SelfInstance,
            ..
        } = &spelling
        {
            // A `@property` getter shares the bare spelling with an
            // ordinary instance method. `HirClassDef::properties` holds the
            // getter's own mangled name, so this is an identity test rather
            // than a name pattern.
            if module.class_defs.iter().any(|(held, def)| {
                held == class && def.properties.iter().any(|prop| prop.getter == *name)
            }) {
                continue;
            }
            if !instance_methods_reachable(module, class) {
                continue;
            }
        }
        // A `@classmethod`'s leading `cls` never crosses the boundary (the
        // wrapper passes a C `NULL` for it) and an instance method's
        // leading `self` crosses it as an opaque pointer the wrapper
        // unwraps, not as a carried argument. Both are split off here,
        // before `unsupported_boundary_ty` runs, because `Ty::Instance` has
        // no `boundary_carrier` arm -- leaving either in would make every
        // such export a `C0003` instead of an export.
        let receiver = match &spelling {
            ExportName::ModuleLevel => ExtReceiver::None,
            ExportName::Method { receiver, .. } => *receiver,
        };
        // `receiver` is decided lexically, from the mangled suffix alone,
        // because `classify_export_name` cannot see HIR. The guarantee that
        // such a function really leads with `cls`/`self` lives in another
        // crate -- `crates/pycc_hir/src/class.rs` refuses a `@classmethod`
        // that does not take `cls` first and requires a regular method's
        // first parameter to be named `self` -- so this site states that
        // cross-crate invariant instead of slicing on the strength of it.
        let carried_params = if receiver == ExtReceiver::None {
            &params[..]
        } else {
            match params.split_first() {
                Some((_, tail)) => tail,
                None => panic!(
                    "pycc: internal error: `{name}` is spelled as a method with a \
                     receiver but has no parameters -- pycc_hir::class refuses a \
                     `@classmethod` without a leading `cls` and a regular method \
                     without a leading `self`, so this HIR should never have been built"
                ),
            }
        };
        if let Some(offender) = unsupported_boundary_ty(carried_params, return_ty) {
            gaps.push(capability_gap(name, &offender));
            continue;
        }
        let (class, method) = match &spelling {
            ExportName::ModuleLevel => (None, None),
            ExportName::Method { class, method, .. } => (Some(class.clone()), Some(method.clone())),
        };
        let export = ExtExport {
            name: name.clone(),
            class,
            method,
            receiver,
            params: carried_params.iter().map(|(_, ty)| ty.clone()).collect(),
            return_ty: return_ty.clone(),
        };
        // A module may rebind a public name -- two `def`s, a `def` over an
        // imported name, or two `@staticmethod def f` in one class body,
        // which `pycc check` accepts. Codegen emits exactly one
        // `fnptr_<name>` global and binds it to the *last* definition, so
        // the wrapper table must carry exactly one entry per C function
        // definition, with that definition's signature: a second entry
        // generates a second `pycc_ext_wrap_<name>` and the C compiler
        // rejects the redefinition outright. Replacing in place rather than
        // appending keeps the table in definition order, which is what the
        // generated `.inc` fixtures assert. Cross-*module* collisions cannot
        // reach here -- `pycc_hir`'s import closure rejects a name defined by
        // two inputs with `C0001` first.
        //
        // The key is `(class, method)` for a method and the plain name for a
        // module-level function -- **not** the host-visible name alone. On a
        // type object `Grid.scale` and `Other.scale` both publish `ml_name`
        // `"scale"` and belong to different `PyMethodDef` tables, so a
        // host-visible key would collapse two distinct exports into one;
        // while keying on the compiled `name` alone would let one class
        // publish `ml_name` `"f"` twice, once from `f.static` and once from
        // `f.classmethod`, which Python's own class body cannot mean.
        // Replacing keeps the last definition, which is what `Grid.f` binds
        // to in Python and what the shared `fnptr_` slot holds.
        let key = export_dedup_key(&export);
        match exports
            .iter_mut()
            .find(|held| export_dedup_key(held) == key)
        {
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

/// What [`resolved_init`] hands back: the constructor's mangled name, its
/// declared parameter list -- receiver still at index 0 -- and its return
/// type. A named alias only because `clippy::type_complexity` refuses the
/// tuple spelled inline; every caller destructures it immediately.
pub(crate) type ResolvedInit<'a> = (&'a str, &'a [(String, Ty)], &'a Ty);

/// The `__init__` a host-side `mod.<Class>(...)` call would run, resolved
/// through `class`'s MRO exactly as instantiation resolves it.
///
/// Two passes, and the second is mandatory (#966, D-232): a D-225 implicit
/// zero-argument constructor lands in its *own* class's method table, where
/// it would otherwise out-rank a real `__init__` declared by a later base.
/// The first pass therefore skips every `implicit_object_init` class and the
/// second accepts one, mirroring `pycc_types::class`' `super().__init__()`
/// ranking and `pycc_mir`'s instantiation lowering. A class whose only
/// constructor *is* the implicit one -- `class C: pass`, the common case --
/// is found by the second pass alone.
///
/// Returns the constructor's *destructured* mangled name, parameter list
/// and return type rather than the `HirItem` itself. Every caller needs all
/// three, and handing back the enum would make each one re-match a variant
/// whose other arms this function has already excluded -- an `else` arm no
/// test could ever execute, which
/// `scripts/check_diff_coverage.py`'s 100%-changed-lines invariant does not
/// admit (D-242 rule 1).
pub(crate) fn resolved_init<'a>(module: &'a HirModule, class: &str) -> Option<ResolvedInit<'a>> {
    let (_, class_def) = module.class_defs.iter().find(|(held, _)| held == class)?;
    let resolve = |skip_implicit: bool| {
        class_def.mro.iter().find_map(|mro_class| {
            let (_, mro_def) = module
                .class_defs
                .iter()
                .find(|(held, _)| held == mro_class)?;
            if skip_implicit && mro_def.implicit_object_init {
                return None;
            }
            mro_def
                .methods
                .iter()
                .find(|(method, _)| method == "__init__")
                .map(|(_, mangled)| mangled.as_str())
        })
    };
    let mangled = resolve(true).or_else(|| resolve(false))?;
    module.items.iter().find_map(|item| match item {
        HirItem::Function {
            name,
            params,
            return_ty,
            ..
        } if name == mangled => Some((name.as_str(), params.as_slice(), return_ty)),
        _ => None,
    })
}

/// Whether a published class can be constructed from the host -- the
/// canonical statement of D-244's #1145 amendment clause (b), and the
/// predicate every instance-method exclusion cites.
///
/// *Publication* is decided elsewhere, by [`collect_class_publications`]: a
/// class gets a type object exactly when its MRO-resolved method set is
/// non-empty, which since #1145's inheritance fix includes a class that
/// declares no exportable member of its own. A class with an empty resolved
/// set gets no type object at all and is not constructible however this
/// answers. What this function adds is which *published* class gets a
/// `Py_tp_init`.
///
/// The four conditions, each a HIR fact rather than a name pattern:
///
/// 1. the class is not abstract, not a `Protocol` and not an `Enum` --
///    `is_abstract` is what removes an `@abstractmethod`'s stub body, whose
///    lowered `HirItem::Function` returns nothing while its `return_ty` says
///    otherwise, and it removes it *totally*: a class carrying its own
///    `@abstractmethod` without an `ABC` base never reaches codegen at all
///    (`crates/pycc_hir/src/class.rs`'s unoverridden-abstract check rejects
///    it with `C0001`), so every surviving abstract stub belongs to an
///    `is_abstract` class;
/// 2. it carries no `exception_type_tag` and is not a seeded synthetic
///    builtin exception class -- [`register_class_c`] already publishes a
///    user exception class under its bare name, and the flat builtins carry
///    no tag of their own;
/// 3. its MRO-resolved `__init__` ([`resolved_init`]) exists and returns
///    `None` -- an unannotated `__init__` infers `Ty::Infer` and is refused
///    here rather than emitting a constructor whose C return type is
///    unspellable;
/// 4. the receiver-free tail of that `__init__`'s parameters is carriable by
///    [`unsupported_boundary_ty`] and contains no `tuple`.
///
/// Condition 4 **reuses `unsupported_boundary_ty`** rather than re-deriving
/// the predicate, so a constructor's admissibility is literally the same
/// statement as every other export's instead of a second one that drifts
/// (`AGENTS.md`'s canonical-statement rule). It also answers correctly, for
/// free, on the case a hand-written predicate would most likely miss: `def
/// __init__(self, other: Grid)` carries a `Ty::Instance`, which has no
/// [`boundary_carrier`] arm, so the class is non-constructible rather than
/// emitting a C declaration no C type can spell. The extra `tuple` refusal
/// is not a second admissibility rule either: `is_ext_exportable_name`
/// answers `false` for `<Class>.__init__` (its second segment starts with
/// `_`), so `ext_thunk_required` emits no `pycc_ext_thunk_` for a
/// constructor and a `tuple` parameter would have no callable C entry point
/// at all.
pub(crate) fn class_constructible(module: &HirModule, class: &str) -> bool {
    ctor_descriptor(module, class).is_some()
}

/// [`class_constructible`]'s conditions 1 and 2 -- the ones about the
/// class's own *shape* rather than about its `__init__` -- factored out so
/// [`instance_methods_reachable`] can hold them while relaxing conditions 3
/// and 4, instead of restating them (`AGENTS.md`'s canonical-statement
/// rule).
fn instance_shape_admissible(class_def: &HirClassDef, class: &str) -> bool {
    !class_def.is_abstract
        && !class_def.is_protocol
        && !class_def.is_enum
        && class_def.exception_type_tag.is_none()
        && !is_builtin_exception_class(class)
}

/// Whether an instance method *declared by* `class` can ever reach the host
/// -- the predicate [`collect_exports`] applies to the bare method spelling,
/// and the canonical statement of D-244 rule 1's #1145 receiver-reachability
/// clause.
///
/// Strictly wider than [`class_constructible`], and deliberately so. A
/// method is lowered once against its own class's slot layout and is then
/// inherited by every subclass, so `Derived(21).value()` reaches
/// `Base.value`'s compiled body even when `Base` itself can never be built
/// from the host -- an unannotated or `tuple`-carrying `__init__` makes
/// `Base` unconstructible without making its methods unreachable. The
/// answer is therefore "*some* class whose MRO contains `class` is
/// constructible", and `mro[0]` is the class itself, so a constructible
/// class answers for its own methods.
///
/// [`class_constructible`]'s conditions 3 and 4 are the ones a constructible
/// subclass rescues. Conditions 1 and 2 -- [`instance_shape_admissible`] --
/// are not: an `@abstractmethod`'s stub body returns nothing while its
/// `return_ty` says otherwise, so exporting it from an `is_abstract` base
/// would emit a wrapper over a body that never returns, and an exception
/// class publishes no type object at all.
fn instance_methods_reachable(module: &HirModule, class: &str) -> bool {
    let Some((_, class_def)) = module.class_defs.iter().find(|(held, _)| held == class) else {
        return false;
    };
    if !instance_shape_admissible(class_def, class) {
        return false;
    }
    module.class_defs.iter().any(|(held, def)| {
        def.mro.iter().any(|entry| entry == class) && class_constructible(module, held)
    })
}

/// [`class_constructible`]'s answer with the generated `Py_tp_init`'s inputs
/// attached: one function, so the predicate and the descriptor can never
/// disagree about which classes are constructible.
fn ctor_descriptor(module: &HirModule, class: &str) -> Option<ExtCtor> {
    let (_, class_def) = module.class_defs.iter().find(|(held, _)| held == class)?;
    if !instance_shape_admissible(class_def, class) {
        return None;
    }
    let (name, params, return_ty) = resolved_init(module, class)?;
    if *return_ty != Ty::None {
        return None;
    }
    // The constructor descriptor does not come through `collect_exports`,
    // so the receiver is still at index 0 here and has to be split off
    // exactly as the classmethod path splits `cls`: `Ty::Instance` has no
    // carrier arm, so leaving it in would refuse every constructor.
    let (_, carried) = params.split_first()?;
    if unsupported_boundary_ty(carried, &Ty::None).is_some()
        || carried.iter().any(|(_, ty)| matches!(ty, Ty::Tuple(_)))
    {
        return None;
    }
    Some(ExtCtor {
        class: class.to_string(),
        name: name.to_string(),
        params: carried.iter().map(|(_, ty)| ty.clone()).collect(),
        slot_count: instance_slot_count(module, class_def),
    })
}

/// The number of `pycc_rt_instance_new` slots an instance of `class_def`
/// occupies.
///
/// `pycc_hir::flat_attr_layout` is the single definition of what counts as
/// a slot -- merged `@dataclass` fields, an exception class's empty
/// attribute list -- and `pycc_mir` delegates to it too, so the count the
/// generated `tp_init` allocates is by construction the one
/// `MirExpr::Instantiate` passes for the same class.
///
/// That parity is what forbids skipping an MRO entry this program does not
/// define. The counterpart on the MIR side is `pycc_mir::class`'s
/// `mro_attrs`/`mro_class_def`, which does not skip such an entry either --
/// it panics with an internal error, pinned by
/// `mro_attrs_with_a_ghost_class_in_the_mro_panics_with_an_internal_error`
/// (`crates/pycc_mir/src/tests/class_mro.rs`). Skipping here while
/// `mro_attrs` panics there would mean under-allocating an instance whose
/// inherited compiled methods then index past its own storage, so this path
/// fails the same way instead. It is unreachable on any program `pycc check`
/// accepts: `validate_bases` rejects a base this module does not define, and
/// the one MRO entry that can be absent from `HirModule::class_defs` -- a
/// synthetic builtin exception base dropped as `pycc_hir::link` concatenates
/// -- only ever appears in the MRO of an exception class, which
/// [`instance_shape_admissible`] already refused above.
fn instance_slot_count(module: &HirModule, class_def: &HirClassDef) -> usize {
    let mro_defs: Vec<&HirClassDef> = class_def
        .mro
        .iter()
        .map(|mro_class| {
            match module
                .class_defs
                .iter()
                .find(|(held, _)| held == mro_class)
                .map(|(_, def)| def)
            {
                Some(def) => def,
                None => panic!(
                    "pycc: internal error: class `{}` lists `{mro_class}` in its \
                     method resolution order but this program defines no such \
                     class -- pycc_hir::class refuses a base the module does not \
                     define, so this HIR should never have been built",
                    class_def.name
                ),
            }
        })
        .collect();
    flat_attr_layout(&mro_defs).len()
}

/// One constructible class's generated `Py_tp_init` descriptor.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtCtor {
    /// The class's own (unqualified) Python name, which is also the
    /// host-visible type name and the suffix of every C identifier the
    /// generated shim builds for it.
    pub(crate) class: String,
    /// The compiled program's name for the constructor,
    /// `pycc_hir::class`'s mangled `<Owner>.__init__`. `<Owner>` is the
    /// MRO-resolved owner and not necessarily [`ExtCtor::class`].
    pub(crate) name: String,
    /// The constructor's declared parameter types **excluding** the leading
    /// `self`, which the generated shim supplies itself from
    /// `pycc_rt_instance_new`. Their count is the arity `tp_init` checks
    /// `PyTuple_Size(args)` against.
    pub(crate) params: Vec<Ty>,
    /// The slot count `pycc_rt_instance_new` is called with.
    pub(crate) slot_count: usize,
}

/// The constructible classes among the published ones, in publication order.
///
/// Driven by [`collect_class_publications`] rather than by the export set
/// directly, so a class that declares no exportable member of its own but
/// inherits one -- published since #1145's inheritance fix -- gets its
/// `Py_tp_init` too. Ordered off that list and never off a hash map,
/// because the generated `.inc` must be byte-identical across runs.
pub(crate) fn collect_constructors(
    module: &HirModule,
    publications: &[ExtPublishedClass],
) -> Vec<ExtCtor> {
    publications
        .iter()
        .filter_map(|published| ctor_descriptor(module, &published.class))
        .collect()
}

/// One published class: the host-visible type object's name and the exact
/// `PyMethodDef` rows it carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtPublishedClass {
    /// The class's own (unqualified) Python name.
    pub(crate) class: String,
    /// The methods published on this class's type object, MRO-resolved:
    /// every exported member of the class and of its bases, first MRO hit
    /// winning, in the order [`collect_class_publications`] resolves them.
    /// Each entry is an [`ExtExport`] the export set already holds, so every
    /// row names a `pycc_ext_wrap_` that [`generate_exports_inc`] really
    /// emits.
    pub(crate) methods: Vec<ExtExport>,
}

/// The published classes and, for each, its MRO-resolved method set.
///
/// **MRO-resolved, not own-declared.** A method is lowered once against its
/// own class's slot layout and inherited unchanged, so `mod.Derived(21)`
/// must answer `value()` as well as `twice()`. Resolving the set here is
/// what publishes it: the walk is `class_def.mro`, most derived first --
/// the same order [`resolved_init`] walks for `__init__` -- and the first
/// hit on a given method name wins, so a derived override shadows its base's
/// definition exactly as Python's own attribute lookup does. That direction
/// is the opposite of [`collect_exports`]' `(class, method)` dedup, which
/// keeps the *last* binding because a rebound name is what `Grid.f` means in
/// one class body.
///
/// **Why an inherited method may be published at all.** A base method's
/// compiled body addresses its own class's slot indices, and
/// `crates/pycc_hir/src/class/mro.rs`'s `validate_mro_slot_layout` (#969)
/// rejects with
/// `C0001`, during HIR lowering, every multiple-inheritance shape whose
/// ancestor layout is not a name-wise prefix of the derived one -- see that
/// function's own doc for why. So a base method invoked with a derived
/// instance addresses the same attributes, and this path inherits that
/// invariant rather than restating it.
///
/// The class list is the export set's classes in first-export order, then
/// every remaining class that resolves something, in `HirModule::class_defs`
/// order: a class with no export of its own has no first-export position,
/// and appending is the only deterministic slot for it. Deterministic is the
/// requirement -- the generated `.inc` must be byte-identical across runs,
/// so neither list is ever built from a hash map.
///
/// A class is publishable when [`is_public_name`] accepts its name and it is
/// not an exception class: [`register_class_c`] already publishes a user
/// exception class under its bare name, and a second `PyModule_AddObjectRef`
/// under that name would replace it. Both are already true of every class in
/// the export set -- [`classify_export_name`] and [`collect_exports`]'
/// exception filter see to that -- so the test bites only on an inheriting
/// class that exports nothing itself. Abstractness is deliberately *not*
/// tested: Part 1 published a `@staticmethod` on an abstract class, and an
/// abstract class exports no instance method to begin with
/// ([`instance_methods_reachable`]).
pub(crate) fn collect_class_publications(
    module: &HirModule,
    exports: &[ExtExport],
) -> Vec<ExtPublishedClass> {
    let mut order: Vec<&str> = Vec::new();
    for export in exports {
        if let Some(class) = &export.class
            && !order.contains(&class.as_str())
        {
            order.push(class.as_str());
        }
    }
    for (class, _) in &module.class_defs {
        if !order.contains(&class.as_str()) {
            order.push(class.as_str());
        }
    }
    let mut published: Vec<ExtPublishedClass> = Vec::new();
    for class in order {
        let Some((_, class_def)) = module.class_defs.iter().find(|(held, _)| held == class) else {
            continue;
        };
        if !is_public_name(class) || class_def.exception_type_tag.is_some() {
            continue;
        }
        let mut methods: Vec<ExtExport> = Vec::new();
        for ancestor in &class_def.mro {
            for export in exports
                .iter()
                .filter(|export| export.class.as_deref() == Some(ancestor.as_str()))
            {
                if !methods.iter().any(|held| held.method == export.method) {
                    methods.push(export.clone());
                }
            }
        }
        if !methods.is_empty() {
            published.push(ExtPublishedClass {
                class: class.to_string(),
                methods,
            });
        }
    }
    published
}

mod carrier;
pub(crate) use carrier::*;
mod export_name;
pub(crate) use export_name::*;
mod method_types;
pub(crate) use method_types::*;

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
/// `METH_FASTCALL` wrapper per export, the module-level `PyMethodDef`
/// table, and one type object per published class (`method_types_c`).
///
/// `module_name` is already known to be a valid ASCII Python identifier
/// (`ext_output::resolve` rejects everything else before this runs). An
/// export name is **no longer** a Python identifier by construction: a
/// method's compiled name is `pycc_hir::class`'s dotted mangling, so every
/// C identifier built from it goes through `pycc_codegen::mangle_ext_name`
/// first. What survives of the old invariant is what it was for -- the
/// pieces pasted into C source are a module name, a class name, a method
/// name and a mangled derivation of the three, and none can carry a
/// character that would escape it.
///
/// The export list is **partitioned** on [`ExtExport::class`]. A
/// module-level function keeps its `pycc_ext_methods[]` row unchanged; a
/// method goes into its own class's table instead, because a row in
/// `pycc_ext_methods[]` would publish exactly the flat `mod."Class.method"`
/// attribute the type object exists to avoid.
pub(crate) fn generate_exports_inc(
    module_name: &str,
    exports: &[ExtExport],
    classes: &[UserExceptionClass],
    publications: &[ExtPublishedClass],
    ctors: &[ExtCtor],
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
    for export in exports.iter().filter(|export| export.class.is_none()) {
        // A module-level function's bare name, its mangled name and its
        // wrapper suffix are all the same string -- the mangling is the
        // identity for a dot-free name -- so this row keeps its single-`name`
        // format. The divergence between `ml_name` and the wrapper symbol
        // belongs to the per-class tables below.
        out.push_str(&format!(
            "    {{\"{name}\", (PyCFunction)(void (*)(void))pycc_ext_wrap_{name}, \
             METH_FASTCALL, NULL}},\n",
            name = export.name
        ));
    }
    out.push_str("    {NULL, NULL, 0, NULL},\n};\n\n");
    out.push_str(&method_types_c(publications, ctors));
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
    // `ext_thunk_required` asks only whether a *declared* type is a tuple,
    // and a receiver is never one, so the receiver-free tail gives the same
    // verdict the codegen side reaches from the MIR function's full
    // parameter list. The two therefore stay in agreement about whether a
    // thunk exists at all.
    let use_thunk = pycc_codegen::ext_thunk_required(name, &export.params, &export.return_ty);
    let thunk = pycc_codegen::ext_thunk_symbol(name);
    // A method's `name` is dotted, and these four sites paste it into C
    // identifiers: the thunk `extern` (through `ext_thunk_symbol`, which
    // mangles for itself), the `fnptr_` `extern`, the `fnptr_` cast at the
    // call, and this wrapper's own definition -- plus the `PyMethodDef` row
    // `generate_exports_inc` emits for it. The mangling is the identity for
    // a dot-free name, so every module-level function's wrapper is
    // byte-identical to what it was.
    let symbol = pycc_codegen::mangle_ext_name(name);
    // The `PyErr_Format` arity message is the one interpolation of `name`
    // that is *not* a C identifier: the host reads it, so it renders the
    // source-level spelling. Left alone it would say
    // `Grid.scale.static() takes exactly 1 argument` next to CPython's own
    // `Grid.scale() takes no keyword arguments` on the same object. The
    // unpack helpers below take the same spelling for the same reason.
    let source_name = source_level_name(name);
    // A `@classmethod`'s compiled signature leads with `cls` and an
    // instance method's with `self` (`ExtExport::receiver`); neither is a
    // carried argument. The leading `void *` is reinstated textually here,
    // so this declaration and `pycc_codegen`'s thunk -- which builds its own
    // list from the MIR function's parameters -- declare the same arity for
    // the same symbol. Only the *call* argument differs between the two:
    // `NULL` for `cls`, the unwrapped instance pointer for `self`.
    let params = {
        let carried = c_param_list(&slots, &out_slots);
        if export.receiver == ExtReceiver::None {
            carried
        } else if carried == "void" {
            // `c_param_list` answers `"void"` for an empty list, because an
            // empty C parameter list means "unspecified". With a receiver
            // the list is not empty.
            "void *".to_string()
        } else {
            format!("void *, {carried}")
        }
    };
    let mut out = String::new();
    if use_thunk {
        out.push_str(&format!("extern {return_c} {thunk}({params});\n"));
    } else {
        out.push_str(&format!("extern void *fnptr_{symbol};\n"));
    }
    out.push_str(&format!(
        "static PyObject *pycc_ext_wrap_{symbol}(PyObject *self, PyObject *const *args, \
         Py_ssize_t nargs)\n{{\n"
    ));
    // A `METH_STATIC` wrapper is handed `NULL` in `self` and a `METH_CLASS`
    // one the *type object*; both discard it, exactly as
    // `MirExpr::NullInstance` discards `cls` at a native call site -- the
    // method was compiled for one class, so nothing in its body reads the
    // receiver, and forwarding a CPython type pointer into a slot typed
    // `Ty::Instance` would be type confusion even though nothing
    // dereferences it.
    //
    // A plain `METH_FASTCALL` instance-method wrapper (#1145) is handed the
    // carrier object itself and *does* read it. The NULL guard is not
    // defence in depth: `mod.Grid.__new__(mod.Grid)` runs
    // `PyType_GenericNew`, which zeroes the carrier and never runs
    // `tp_init`, so `inst` really is NULL at the wrapper's entry and the
    // guard is what makes that a `TypeError` instead of a segfault.
    // CPython's own method-descriptor machinery has already refused a
    // `self` of the wrong type before this point, so no type check is
    // needed here.
    if export.receiver == ExtReceiver::SelfInstance {
        out.push_str(&format!(
            "    void *self_inst = ((PyccExtInstance *)self)->inst;\n    \
             if (self_inst == NULL) {{\n        PyErr_SetString(PyExc_TypeError, \
             \"{source_name}() called on an uninitialized instance\");\n        \
             return NULL;\n    }}\n"
        ));
    } else {
        out.push_str("    (void)self;\n");
    }
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
    out.push_str(&arg_slot_locals(&slots));
    out.push_str(&format!(
        "    if (nargs != {arity}) {{\n        PyErr_Format(PyExc_TypeError, \
         \"{source_name}() takes exactly {arity} argument{plural} (%zd given)\", nargs);\n        \
         return NULL;\n    }}\n",
        plural = if arity == 1 { "" } else { "s" },
    ));
    out.push_str(&unpack_args(
        &slots,
        source_name,
        &|index| format!("args[{index}]"),
        "        return NULL;\n",
    ));
    let mut call_args: Vec<String> = Vec::new();
    match export.receiver {
        ExtReceiver::None => {}
        ExtReceiver::NullCls => call_args.push("NULL".to_string()),
        ExtReceiver::SelfInstance => call_args.push("self_inst".to_string()),
    }
    for (index, slot) in slots.iter().enumerate() {
        match slot {
            BoundaryCarrier::Scalar(..) => call_args.push(format!("a{index}")),
            BoundaryCarrier::Tuple(elements) => {
                call_args.extend((0..elements.len()).map(|element| format!("a{index}_{element}")))
            }
            BoundaryCarrier::Buffer => call_args.push(format!("&a{index}")),
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
            "    {assign}(({return_c} (*)({params}))fnptr_{symbol})({call_args});\n"
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
    // The success-path half of the cleanup discipline, and the half nothing
    // else in the tree would notice was missing: a `Py_buffer` acquired
    // before the call is still held after it returns. Emitted once, at the
    // single point every remaining exit passes through -- the pending-
    // exception bail and the pack below both sit after it -- rather than
    // duplicated at each `return`. Releasing before the pack is safe and
    // deliberate: the pack reads only `result`/`r{index}`, machine words the
    // compiled function already produced, never the buffer's storage.
    //
    // Empty for an export with no `memoryview` parameter, so every wrapper
    // generated before Part 1 of #1027 is byte-identical to what it was.
    let release: String = buffer_releases(&slots, "    ");
    out.push_str(&format!(
        "    if (pycc_rt_ext_pending_type() >= 0) {{\n{}        pycc_ext_raise_pending();\n        \
         return NULL;\n    }}\n{release}",
        buffer_releases(&slots, "        ")
    ));
    match &export.return_ty {
        Ty::None => out.push_str("    Py_RETURN_NONE;\n}\n\n"),
        Ty::Tuple(_) => out.push_str(&pack_tuple_return(source_name, &out_slots)),
        // `pack_int` is the one packer whose failure is a property of the
        // *value*, and the only one whose message therefore names the
        // function: D-141's bigint egress (#1040). `PyFloat_FromDouble` and
        // `PyBool_FromLong` cannot fail at all, and `pack_str` can only fail
        // the way any allocation can -- it refuses no `str` -- so none of
        // the three take a name. Arity is uniform across them, so every
        // packer but `int` shares the generic arm below.
        Ty::Int => out.push_str(&format!(
            "    return pycc_ext_pack_int(\"{source_name}\", result);\n}}\n\n"
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

/// The C local declarations one generated function needs for its argument
/// slots: `a{index}` per scalar, one `a{index}_{element}` per `tuple`
/// element, and a `Py_buffer b{index}` beside the view pair for a
/// `memoryview`.
///
/// Shared by [`wrapper_for`] and the generated `Py_tp_init`
/// (`method_types_c`) so a constructor and an ordinary export declare the
/// same locals for the same declared type.
fn arg_slot_locals(slots: &[BoundaryCarrier]) -> String {
    let mut out = String::new();
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
            // Two locals, both owned by the generated function for the
            // whole call: the `Py_buffer` the shim acquires (and the
            // function releases on every exit past that point), and the
            // `{ptr, len}` pair that is all the compiled body ever sees of
            // it.
            BoundaryCarrier::Buffer => {
                out.push_str(&format!("    Py_buffer b{index};\n"));
                out.push_str(&format!("    {BUFFER_VIEW_C_TYPE} a{index};\n"));
            }
        }
    }
    out
}

/// The per-argument ingress: one `pycc_ext_unpack_*` call per slot, each
/// bailing with `fail` after releasing whatever the earlier slots hold.
///
/// `arg_expr` renders the `PyObject *` for argument `index`, because the two
/// callers receive their arguments differently: a `METH_FASTCALL` wrapper
/// gets a `PyObject *const *` vector and indexes it, while the generated
/// `Py_tp_init` gets a real tuple and has to bridge through
/// `PyTuple_GetItem`. Everything else -- which helper, which local, what the
/// bail path owes -- is shared, which is the point: the unpack helper is a
/// function of the declared type alone, so `mod.Grid(True, 4)` and
/// `mod.Grid(3, 4).scale(True)` must admit exactly the same object set for
/// the same declared `int`. `docs/RUNTIME.md` claims one admissibility
/// matrix, not two.
///
/// Emitted inline rather than behind a shared `goto` label: the cleanup
/// differs per argument index, and neither caller has another exit that owes
/// anything at this point. A `tuple` argument owes nothing -- its elements
/// are copied out by value.
fn unpack_args(
    slots: &[BoundaryCarrier],
    source_name: &str,
    arg_expr: &dyn Fn(usize) -> String,
    fail: &str,
) -> String {
    let mut out = String::new();
    for (index, slot) in slots.iter().enumerate() {
        // Each `str` argument already unpacked holds a fresh reference that
        // only the compiled function's own parameter slot ever consumes, and
        // this branch bails before the call -- so release them here, or a
        // `TypeError` on argument 2 would leak argument 1's `PyStrObj` on
        // every raising call.
        let cleanup: String = slots[..index]
            .iter()
            .enumerate()
            .filter_map(|(earlier, carrier)| Some((earlier, carrier.cleanup()?)))
            .map(|(earlier, owed)| match owed {
                SlotCleanup::StrDecref => format!("        pycc_rt_str_decref(a{earlier});\n"),
                SlotCleanup::BufferRelease => format!("        PyBuffer_Release(&b{earlier});\n"),
            })
            .collect();
        let arg = arg_expr(index);
        match slot {
            BoundaryCarrier::Scalar(_, helper) => out.push_str(&format!(
                "    if (pycc_ext_unpack_{helper}({arg}, \"{source_name}\", {index}, &a{index}) \
                 != 0) {{\n{cleanup}{fail}    }}\n"
            )),
            BoundaryCarrier::Tuple(elements) => {
                let elements_len = elements.len();
                out.push_str(&format!(
                    "    if (pycc_ext_unpack_tuple({arg}, \"{source_name}\", {index}, \
                     {elements_len}) != 0) {{\n{cleanup}{fail}    }}\n"
                ));
                for (element, (_, helper)) in elements.iter().enumerate() {
                    // `PyTuple_GetItem` cannot fail at this call: the check
                    // just emitted refused every non-tuple and every length
                    // but this one, so the index is always in range.
                    out.push_str(&format!(
                        "    if (pycc_ext_unpack_{helper}_at(PyTuple_GetItem({arg}, \
                         {element}), \"{source_name}\", {index}, {element}, &a{index}_{element}) != 0) \
                         {{\n{cleanup}{fail}    }}\n"
                    ));
                }
            }
            // The shim refuses everything that is not an exact,
            // C-contiguous, one-dimensional `float` buffer and leaves
            // nothing acquired when it does, so this arm owes no cleanup of
            // its own -- only the earlier slots'. On success the caller
            // takes the two words it is allowed to keep: the data pointer,
            // and a *copy* of `shape[0]`. `b{index}.shape` itself is
            // exporter-owned storage that dies at `PyBuffer_Release`, so it
            // is never carried across the boundary.
            BoundaryCarrier::Buffer => {
                out.push_str(&format!(
                    "    if (pycc_ext_unpack_memoryview({arg}, \"{source_name}\", {index}, \
                     &b{index}) != 0) {{\n{cleanup}{fail}    }}\n"
                ));
                out.push_str(&format!("    a{index}.ptr = b{index}.buf;\n"));
                out.push_str(&format!(
                    "    a{index}.len = (long long)b{index}.shape[0];\n"
                ));
            }
        }
    }
    out
}

/// One `PyBuffer_Release` line per `memoryview` slot, indented with
/// `indent`, or the empty string when the export has none.
///
/// Shared by the two success-path emission points (inside the pending-
/// exception block, and just before the egress) so the two can never
/// release different sets.
fn buffer_releases(slots: &[BoundaryCarrier], indent: &str) -> String {
    slots
        .iter()
        .enumerate()
        .filter(|(_, carrier)| carrier.cleanup() == Some(SlotCleanup::BufferRelease))
        .map(|(index, _)| format!("{indent}PyBuffer_Release(&b{index});\n"))
        .collect()
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
            BoundaryCarrier::Buffer => types.push(format!("{BUFFER_VIEW_C_TYPE} *")),
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
