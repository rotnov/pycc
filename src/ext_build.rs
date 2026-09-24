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
    BUILTIN_EXCEPTION_CLASSES, FIRST_USER_EXCEPTION_TYPE_TAG, HirClassDef, HirItem, HirModule,
    ProtocolMember, Ty, flat_attr_layout, is_builtin_exception_class, is_public_name,
};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
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
        let output = probe_command(&self.interpreter, PROBE_SCRIPT)
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

/// The `<interpreter> -I -c <script>` command both CPython probes (this
/// one and the embedded mode's) run. `-I` is isolated mode: the current
/// directory, the user site directory and every `PYTHON*` variable stay off
/// the probe's module search path, so a `sysconfig.py` planted in the
/// project being built cannot execute when the probe imports `sysconfig`.
pub(crate) fn probe_command(interpreter: &OsStr, script: &str) -> std::process::Command {
    let mut command = std::process::Command::new(interpreter);
    command.arg("-I").arg("-c").arg(script);
    command
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
    /// Whether this export's body returns a **sub-range** of one of its
    /// `memoryview` parameters (Part 2 of #1175, #1179).
    ///
    /// Not a function of [`ExtExport::return_ty`]: a declared
    /// `-> memoryview` is the same signature for a bare `return b`
    /// (Part 1 of #1175), an artifact-owned `return a` (Part 2b of #1142)
    /// and `return b[i:j]`, and only the last of the three carries the
    /// three trailing `long long *` out-pointers
    /// `pycc_codegen::ext_thunk_out_tys` describes. Keying the wrapper on
    /// the declared type instead would move every existing
    /// buffer-returning export onto the thunk path and change generated C
    /// this task does not touch.
    pub(crate) returns_buffer_slice: bool,
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
    /// Per-parameter writability, parallel to [`ExtExport::params`] and
    /// carrying `true` exactly where that parameter is a `memoryview` the
    /// body stores into (Part 1 of #1142, `pycc_hir::body_stores_into`).
    ///
    /// Carried here rather than derived in [`boundary_carrier`], which is a
    /// pure function of a `Ty` and cannot see a body, and rather than
    /// folded into [`ExtExport::params`], which every other consumer reads
    /// as a plain type list. `false` for every non-buffer parameter, where
    /// it is inert.
    pub(crate) param_writable: Vec<bool>,
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
/// instance can be a receiver for -- [`instance_method_reachable`] is the
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
/// * every instance method of a class [`instance_method_reachable`]
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
    // Every compiled name any definition of which returns a buffer
    // sub-range; see the union pass after the loop for why the fact is
    // per-name rather than last-wins.
    let mut slice_widened_names: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut gaps = Vec::new();
    for item in &module.items {
        let HirItem::Function {
            name,
            params,
            return_ty,
            body,
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
            method,
            receiver: ExtReceiver::SelfInstance,
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
            if !instance_method_reachable(module, class, method) {
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
        // such a function really leads with a receiver lives in another
        // crate -- `crates/pycc_hir/src/class.rs` refuses a `@classmethod`
        // that does not take `cls` first, and requires a regular method to
        // declare a receiver as its first parameter, lowering it under the
        // canonical name `self` whatever the source spelled it (#1181;
        // `crates/pycc_hir/src/class/receiver.rs`). The invariant this site
        // rests on is *positional*, so the #1181 relaxation of the source
        // spelling does not weaken it -- this site states that cross-crate
        // invariant instead of slicing on the strength of it.
        let carried_params = if receiver == ExtReceiver::None {
            &params[..]
        } else {
            match params.split_first() {
                Some((_, tail)) => tail,
                None => panic!(
                    "pycc: internal error: `{name}` is spelled as a method with a \
                     receiver but has no parameters -- pycc_hir::class refuses a \
                     `@classmethod` without a leading `cls` and a regular method \
                     without a leading receiver parameter, so this HIR should never \
                     have been built"
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
            // Part 1 of #1142. The walk runs over the *post-split* carried
            // tail, so the flags line up with `params` even for a method
            // whose receiver was dropped above, and it is keyed on the
            // parameter's own source name because that is what
            // `HirStmt::DictSet` carries.
            param_writable: carried_params
                .iter()
                .map(|(param_name, ty)| {
                    *ty == Ty::MemoryView && pycc_hir::body_stores_into(body, param_name)
                })
                .collect(),
            // Part 2 of #1175 (#1179). The driver's half of the per-export
            // "this body carries a buffer sub-range egress" fact; codegen
            // recomputes the same fact from MIR
            // (`pycc_codegen::body_returns_buffer_slice`) because
            // `ExtExport` never crosses the crate boundary, and a parity
            // test pins the two answers together. A divergence is not a
            // wrong diagnostic: it is a generated C call form that does not
            // match the compiled function's own signature.
            //
            // Keyed on the carried parameter's own source name, exactly as
            // `param_writable` above is, and asked only of `memoryview`
            // parameters -- which is the provenance half
            // `pycc_hir::body_returns_slice_of` deliberately leaves to its
            // caller.
            returns_buffer_slice: carried_params.iter().any(|(param_name, ty)| {
                *ty == Ty::MemoryView && pycc_hir::body_returns_slice_of(body, param_name)
            }),
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
        if export.returns_buffer_slice {
            slice_widened_names.insert(export.name.clone());
        }
        let key = export_dedup_key(&export);
        match exports
            .iter_mut()
            .find(|held| export_dedup_key(held) == key)
        {
            Some(held) => *held = export,
            None => exports.push(export),
        }
    }
    // Part 2 of #1175 (#1179), review round 2. The one field that is *not*
    // resolved last-wins: whether the compiled function carries the three
    // buffer-sub-range out-pointers is a property of the shared
    // `fnptr_<name>` slot's single signature, not of the definition
    // currently bound to it, so it unions over every definition of the
    // compiled name exactly as `pycc_codegen::buffer_slice_out_names` does.
    //
    // Keyed on the compiled `name` rather than on `export_dedup_key`
    // deliberately: the name is what codegen groups by, and a class that
    // publishes one `ml_name` from two differently-mangled definitions
    // shares the dedup key without sharing a signature.
    //
    // Taking the last definition's answer instead is what let
    // `def f: return b[1:]` / `def f: return b` declare one arity in the
    // generated C and compile another -- an ill-typed call across the
    // object boundary that no compiler on either side can see. Widening a
    // name whose active definition only does a bare `return b` is harmless:
    // that return stores `has_slice = 0` and the wrapper hands back the
    // whole view.
    for export in &mut exports {
        export.returns_buffer_slice = slice_widened_names.contains(&export.name);
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
/// [`instance_method_reachable`] can hold them while relaxing conditions 3
/// and 4, instead of restating them (`AGENTS.md`'s canonical-statement
/// rule).
fn instance_shape_admissible(class_def: &HirClassDef, class: &str) -> bool {
    !class_def.is_abstract
        && !class_def.is_protocol
        && !class_def.is_enum
        && class_def.exception_type_tag.is_none()
        && !is_builtin_exception_class(class)
}

/// Whether the artifact publishes a type object for `class` at all, as far
/// as the class's *name and kind* decide it -- the two conditions
/// [`collect_class_publications`] applies, factored out so
/// [`instance_method_reachable`] can require them of its witness instead
/// of restating them (`AGENTS.md`'s canonical-statement rule).
///
/// Publication's third condition -- that the class's MRO-resolved method
/// set is non-empty -- is deliberately *not* here, and a witness does not
/// need it: a class that satisfies this predicate and is
/// [`class_constructible`] carries, in its own MRO, the very method whose
/// export it is asked to justify, so its resolved set is non-empty by
/// construction. Stating it here would also be circular, since the
/// resolved set is built out of the export set this predicate helps
/// decide.
/// Where each conjunct bites: the name half is what a *witness* needs --
/// a privately named subclass is constructible and unpublished -- and the
/// tag half is what the *publication* site needs, since `class_constructible`
/// already refuses an exception-tagged class through
/// [`instance_shape_admissible`]. Deleting either one turns a test red, but
/// not the same test: the name half is pinned by
/// `a_privately_named_constructible_subclass_witnesses_nothing_for_its_base`
/// and the tag half by `a_private_or_exception_inheriting_class_is_not_published`.
///
/// [`instance_shape_admissible`]'s third exclusion, `is_builtin_exception_class`,
/// is deliberately absent: the synthetic builtin classes `pycc_hir` seeds carry no
/// public method of their own, so [`collect_class_publications`]'s non-empty
/// resolved-method-set condition already removes every one of them before this
/// predicate's answer could matter.
fn class_publishable(class_def: &HirClassDef, class: &str) -> bool {
    is_public_name(class) && class_def.exception_type_tag.is_none()
}

/// Whether `class`'s own shape admits instance exports **and** the host can
/// actually obtain a receiver for them -- the predicate [`collect_exports`]
/// applies to the bare method spelling, and the canonical statement of
/// D-244 rule 1's #1145 receiver-reachability clause.
///
/// The answer is: [`instance_shape_admissible`] holds of `class` itself,
/// *and* some class the artifact **publishes** ([`class_publishable`])
/// whose MRO contains `class` is [`class_constructible`].
///
/// Wider than [`class_constructible`] alone, and deliberately so. A method
/// is lowered once against its own class's slot layout and is then
/// inherited by every subclass, so `Derived(21).value()` reaches
/// `Base.value`'s compiled body even when `Base` itself can never be built
/// from the host -- an unannotated or `tuple`-carrying `__init__` makes
/// `Base` unconstructible without making its methods unreachable. `mro[0]`
/// is the class itself, so a publishable constructible class answers for
/// its own methods.
///
/// **The witness must also resolve `method` to `class`.** The predicate is
/// per method, not per class, because [`collect_class_publications`] answers
/// a name from the first MRO entry that binds it (see [`namespace_owner`]):
/// a witness whose own body shadows `method` -- with a `@property`, an
/// `@abstractmethod`, or a member of any other kind -- publishes its own
/// binding and never the one compiled here, so this method is as
/// unreachable through that witness as it is through an unpublished one.
/// A witness whose MRO assigns `method` to `self` in any `__init__`
/// disqualifies it for the same reason: the instance answers the name and
/// the compiled body is unreachable through that witness too.
/// Exporting it anyway would emit a `PyMethodDef` row nothing can call and,
/// with an uncarriable signature, fail the whole `--ext` build with a
/// `C0003` for a method no host could ever reach.
///
/// **Both halves of the witness are load-bearing.** Constructibility alone
/// is not enough, because the host names a constructor only through a
/// published type object: a privately named subclass is never published by
/// [`collect_class_publications`], so `mod._Priv(...)` does not exist and
/// no instance reaching `class`'s methods can ever be built. Accepting
/// such a witness would emit a `PyMethodDef` row with no obtainable
/// receiver, or -- with an uncarriable signature -- fail the whole `--ext`
/// build with a `C0003` for a method nothing could ever call.
///
/// [`class_constructible`]'s conditions 3 and 4 are the ones a publishable
/// constructible subclass rescues. Conditions 1 and 2 --
/// [`instance_shape_admissible`] -- are not: an `@abstractmethod`'s stub
/// body returns nothing while its `return_ty` says otherwise, so exporting
/// it from an `is_abstract` base would emit a wrapper over a body that
/// never returns, and an exception class publishes no type object at all.
fn instance_method_reachable(module: &HirModule, class: &str, method: &str) -> bool {
    let Some((_, class_def)) = module.class_defs.iter().find(|(held, _)| held == class) else {
        return false;
    };
    if !instance_shape_admissible(class_def, class) {
        return false;
    }
    module.class_defs.iter().any(|(held, def)| {
        def.mro.iter().any(|entry| entry == class)
            && class_publishable(def, held)
            && class_constructible(module, held)
            && namespace_owner(module, &def.mro, method) == Some(class)
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
    let body = module.items.iter().find_map(|item| match item {
        HirItem::Function {
            name: held, body, ..
        } if held == name => Some(body.as_slice()),
        _ => None,
    });
    // `resolved_init` found `name` in `module.items` to produce the
    // signature above, so the same lookup cannot miss now; `unwrap_or` is
    // the total spelling of that rather than a second panic site.
    let body = body.unwrap_or(&[]);
    Some(ExtCtor {
        class: class.to_string(),
        name: name.to_string(),
        params: carried.iter().map(|(_, ty)| ty.clone()).collect(),
        param_writable: carried
            .iter()
            .map(|(param_name, ty)| {
                *ty == Ty::MemoryView && pycc_hir::body_stores_into(body, param_name)
            })
            .collect(),
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
    /// Per-parameter writability, parallel to [`ExtCtor::params`] and
    /// meaning exactly what [`ExtExport::param_writable`] means.
    ///
    /// A `memoryview` `__init__` parameter is a live ingress path
    /// `collect_exports` never sees -- it refuses `__init__` outright -- so
    /// the flag has to be computed here too, or `Py_tp_init` would acquire
    /// read-only for a constructor body the checker admits a store in.
    pub(crate) param_writable: Vec<bool>,
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
/// the same order [`resolved_init`] walks for `__init__`.
///
/// **The walk resolves the namespace, not the export set.** This is the
/// canonical statement of that rule: a name any `__init__` along the MRO
/// assigns to `self` is answered by the instance and belongs to no class
/// at all ([`mro_binds_slot`]); every other name is answered by the
/// *first* MRO entry that binds it in the class namespace --
/// [`class_member_names`] is what "binds" means -- and that entry alone
/// decides the outcome. If its binding is an
/// export, the method is published; if it is anything the export set does
/// not hold (a `@property` getter, an `@abstractmethod`'s stub, a private
/// or uncarriable member), the name is simply absent from the published
/// class, and the walk never falls through to a base that happens to
/// export the same name. Stopping at the first *exportable* hit instead
/// would publish `Base.value`'s compiled body on a `Derived` whose own
/// `@property value` shadows it -- an artifact that silently disagrees
/// with Python's own attribute lookup (#1146). The rule is kind-blind in
/// both directions, so a derived ordinary method still shadows a base
/// `@property`, and a derived `@staticmethod` still shadows a base
/// instance method, each published under its own receiver kind.
///
/// That direction is the opposite of [`collect_exports`]' `(class, method)`
/// dedup, which keeps the *last* binding because a rebound name is what
/// `Grid.f` means in one class body.
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
/// ([`instance_method_reachable`]).
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
        if !class_publishable(class_def, class) {
            continue;
        }
        let mut methods: Vec<ExtExport> = Vec::new();
        for ancestor in &class_def.mro {
            // [`ExtExport::method`] is `Some` exactly when
            // [`ExtExport::class`] is, so the `?` rejects only the
            // module-level functions this filter drops anyway.
            for (export, method) in exports.iter().filter_map(|export| {
                let method = export.method.as_deref()?;
                (export.class.as_deref() == Some(ancestor.as_str())).then_some((export, method))
            }) {
                // No dedup pass is needed beside this test: exactly one MRO
                // entry owns a given name, and `collect_exports`' own
                // `(class, method)` dedup leaves that entry at most one
                // export under it.
                if namespace_owner(module, &class_def.mro, method) == Some(ancestor.as_str()) {
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

/// Every name `class_def`'s own body binds **in the class namespace**, in
/// a deterministic order, whatever kind of member binds it.
///
/// This is the canonical statement of "does this class define this name"
/// for [`collect_class_publications`]' namespace walk. The kinds are read
/// off the HIR class table rather than recognized by a name pattern:
/// `methods` (an ordinary method, a `@dataclass`-generated one, and an
/// `@abstractmethod`, which `crates/pycc_hir/src/class/body.rs` enters
/// there *and* into `abstract_methods` -- so the latter adds nothing here),
/// `properties` by [`pycc_hir::PropertyDef::name`] (one entry covers a
/// getter and its optional setter: that file refuses a `@<name>.setter`
/// without a preceding `@property` getter, so a setter never binds a name
/// on its own), `static_methods`, `class_methods`, and `class_attrs` -- a
/// `ClassVar` or bare class-level assignment, whose constant is an
/// ordinary entry in the class object's namespace.
///
/// **`attrs` is deliberately not one of them.** An instance-attribute slot
/// is bound on the instance, not on the type, so it is not a namespace
/// entry and has no position in the MRO walk at all; [`mro_binds_slot`]
/// is where it is seen instead.
///
/// `class_attrs` is here for a reason the sibling predicate
/// `crates/pycc_hir/src/class/shadow.rs`'s `declares_name_outside_class_attrs`
/// documents from the other side: that one answers a *class-name-qualified*
/// read (`Derived.LIMIT`) and excludes `class_attrs` because its callers
/// check them separately, while this walk answers an *instance* read and
/// must treat a class attribute as the ordinary namespace entry it is.
/// The two lists differ a second way, which is not an oversight: that
/// predicate also carries `enum_members`, which cannot appear on the MRO
/// this walk is given, because an enum is a terminal leaf --
/// `validate_bases` refuses to extend one with a `C0001` (#941).
///
/// The `ProtocolMember::Method` half of `protocol_members` *is* carried
/// here, for the reason that predicate states: a `Protocol` class's
/// declaration-style `def f(self) -> int: ...` is a real function object in
/// its namespace, and CPython resolves it like any other. A protocol base
/// does reach this walk -- `crates/pycc_hir/src/class.rs` propagates
/// `is_protocol` to an inheritor and gives it the base's
/// `protocol_members`, but such a class is still published, so for
/// `class Q(P, A)` with `P` declaring `f` and `A` exporting a
/// `@staticmethod f`, `P`'s binding is the one CPython answers and `A`'s
/// export must not be published under that name. The binding is not itself
/// exportable, so the name is published by no one -- lossy in the
/// direction this walk is always willing to be wrong in, where resolving
/// the export set instead published a callable Python does not give.
/// Nothing rejects the shape that makes the difference visible:
/// `crates/pycc_hir/src/class/attrs.rs`'s `reject_class_attr_collisions`
/// checks a class's *own* newly declared `class_attrs` against its own MRO
/// and never runs for a class that declares none, so two independent bases
/// -- one binding `f` as a method, the other as a class attribute -- are
/// combined without complaint by a third class that declares neither.
///
/// Slices are walked in table order and never through a hash map: the
/// generated `.inc` must be byte-identical across runs.
fn class_member_names(class_def: &HirClassDef) -> impl Iterator<Item = &str> {
    class_def
        .methods
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(class_def.properties.iter().map(|prop| prop.name.as_str()))
        .chain(
            class_def
                .static_methods
                .iter()
                .map(|(name, _)| name.as_str()),
        )
        .chain(
            class_def
                .class_methods
                .iter()
                .map(|(name, _)| name.as_str()),
        )
        .chain(
            class_def
                .class_attrs
                .iter()
                .map(|(name, _, _)| name.as_str()),
        )
        .chain(class_def.protocol_members.iter().filter_map(|member| {
            match member {
                ProtocolMember::Method { name, .. } => Some(name.as_str()),
                // An annotation-only protocol attribute declares a type,
                // not a binding: `x: int` in a class body leaves the class
                // namespace without an `x`, exactly as it does anywhere
                // else, so it shadows nothing.
                ProtocolMember::Attribute { .. } => None,
            }
        }))
}

/// Whether any class linearized in `mro` assigns `name` to `self` in its
/// `__init__` -- that is, declares it as an instance-attribute slot.
///
/// **Position in the walk is irrelevant, which is the whole point.** An
/// instance slot is not a namespace binding that competes with the class
/// namespace at its own MRO index. CPython consults the instance
/// `__dict__` *before* the type's namespace for everything that is not a
/// data descriptor, so a slot contributed by the *least* derived base
/// still wins over a method defined on the most derived class. Modelling a
/// slot as one more kind inside [`class_member_names`] would answer only
/// the cases where the slot's own class happens to precede the method's
/// (#1146), and would silently publish a callable for
/// `class Base: def __init__(self, n): self.value = n` combined with
/// `class Derived(Base): def value(self): ...`, where CPython answers the
/// integer and raises `TypeError: 'int' object is not callable`.
///
/// **What this predicate actually tests is broader than that, on purpose.**
/// CPython's precedence is per instance and per construction: a name is in
/// the instance `__dict__` only once an `__init__` that assigns it has
/// run. This predicate asks a static question instead -- does any class
/// linearized in `mro` declare the slot at all -- because a compiled
/// instance has no `__dict__`. Every ancestor layout is a name-wise
/// prefix of the derived one: in a single-inheritance chain by
/// construction, since `crates/pycc_hir/src/class/mro.rs`'s
/// `flat_attr_layout` assigns slots most-base-first, and under multiple
/// inheritance because `validate_mro_slot_layout` (#969) rejects every
/// shape where that would not hold, a single base onto an
/// already-validated ancestor inheriting the property transitively. A
/// slot declared anywhere on the MRO
/// therefore occupies a fixed offset in every subclass whether or not the
/// `__init__` that assigns it is the one a given construction reaches.
/// The two conditions differ exactly where an override's `__init__` skips
/// its base's: for `class Base: def __init__(self): self.value = 5` with a
/// `class Derived(Base)` whose `__init__` calls no `super()` and which
/// declares `def value`, CPython answers `Derived().value()` with `99`
/// while the artifact publishes nothing. That is the direction this walk
/// is willing to be wrong in -- lossy, never a callable Python would not
/// give -- and publishing on a guess is what it exists to avoid.
///
/// The same conservatism covers the one class-namespace kind that beats an
/// instance slot, a `@property`: it is a data descriptor, so it would win
/// the name, and suppressing it anyway costs at most a getter+setter
/// property whose class also assigns the name in `__init__`. A read-only
/// property is not that case -- `crates/pycc_types/src/class.rs`'s
/// `check_attr_set` rejects `self.<name> = ...` against it with a `T0044`
/// before such a class compiles at all -- so nothing is lost there.
fn mro_binds_slot(module: &HirModule, mro: &[String], name: &str) -> bool {
    mro.iter().any(|ancestor| {
        module.class_defs.iter().any(|(held, def)| {
            held == ancestor && def.attrs.iter().any(|(held_name, _)| held_name == name)
        })
    })
}

/// The MRO entry that answers `method` for a class linearized as `mro`:
/// the first entry, most derived first, that binds the name in the class
/// namespace ([`class_member_names`]), or `None` when nothing publishable
/// answers it.
///
/// Python's own attribute lookup, stated as a mechanism rather than as a
/// list of kinds. Two rules, in this order:
///
/// 1. A name any `__init__` along the MRO assigns to `self` is answered by
///    the instance, never by the type, so no class owns it and the result
///    is `None` ([`mro_binds_slot`]).
/// 2. Otherwise the first MRO entry binding the name in the class
///    namespace owns it, kind-blind: that entry decides the outcome
///    whether it binds a regular method, a `@property`, an
///    `@abstractmethod`'s stub, a `@staticmethod`, a `@classmethod` or a
///    class attribute.
///
/// Callers ask whether the entry they hold is the winner, never whether an
/// entry further down the walk could also answer.
///
/// `None` from rule 2 alone is unreachable for a `method` some export
/// names: `pycc_hir`'s class lowering records a table entry for every
/// method it mangles, so an export's own class always binds its name. A
/// fixture that pushes a `<Class>.<method>` item without the matching
/// table entry describes a class that lowering could not have produced,
/// and is treated as binding nothing.
fn namespace_owner<'a>(module: &HirModule, mro: &'a [String], method: &str) -> Option<&'a str> {
    if mro_binds_slot(module, mro, method) {
        return None;
    }
    mro.iter().map(String::as_str).find(|ancestor| {
        module.class_defs.iter().any(|(held, def)| {
            held == ancestor && class_member_names(def).any(|name| name == method)
        })
    })
}

mod carrier;
pub(crate) use carrier::*;
mod export_name;
pub(crate) use export_name::*;
mod method_types;
pub(crate) use method_types::*;
mod wrappers;
pub(crate) use wrappers::*;

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
/// trap directly -- the flat seven builtins carry `None` and every builtin
/// past them carries a fixed `Some` below `FIRST_USER_EXCEPTION_TYPE_TAG`.
/// Group-derived classes are
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
