//! The `pycc build` pipeline: frontend, codegen, and the one link site
//! every artifact mode shares (#1036).
//!
//! Split out of `src/main.rs` (AGENTS.md "Keep source files decomposable")
//! when the embedded-executable mode (Part 1 of #1028) became the pipeline's
//! third artifact mode; `main.rs` keeps the CLI dispatch and `pycc run`'s
//! own spawn.

use crate::frontend::{self, report_build_failure, resolve_frontend, resolve_frontend_native};
use crate::{ext_build, ext_output, memoryview_mode};
use std::path::Path;
use std::process::ExitCode;

/// `Ok(())` on success, `Err(code)` carrying the exit code to use on
/// failure. `?` inside this function needs a `Result`, not an `ExitCode`
/// directly -- `run` then just propagates whatever `Err` it gets.
///
/// `target`: `None` builds for the host's own default target (`run` always
/// passes this, since running a cross-compiled binary on this host makes
/// no sense). `Some(triple)` cross-compiles -- see
/// `pycc_codegen::artifact_layout::find_pycc_rt_lib_dir_in` for what that
/// requires to actually be available.
///
/// `release`: the final, already-resolved profile -- `true` runs LLVM's
/// `"default<O3>"` pipeline, `false` skips it. This function does *not*
/// consult a neighboring `pycc.toml` itself: that consumption point
/// (`resolve_release_flag` below) is scoped to `Command::Build`'s own match
/// arm in `main()`, resolved *before* `try_build` is ever called, precisely
/// so that `run`'s hardcoded `false` here stays final and unconditional --
/// `run` has no `--release` flag yet (CLI_SPEC.md doesn't document one for
/// it), so a neighboring release-profile `pycc.toml` must not silently
/// change what `pycc run` does with no way for the user to override it.
///
/// `obj_path`: where codegen's temporary object file is emitted before
/// linking. Caller-supplied (#783) rather than computed here, for two
/// reasons: the production callers (`main()`'s `Command::Build` arm and
/// `run`) place it inside a `create_scratch` `ScratchDir` whose `Drop`
/// removes it on every exit path, and
/// `try_build_ignores_a_neighboring_release_pycc_toml_when_given_release_false`
/// needs to read the emitted object back *after* this function returns --
/// so the path's owner must outlive the call, which only injection (the
/// same DI convention as `init`'s `dir` parameter) provides.
pub(crate) fn try_build(
    path: &Path,
    out: &Path,
    target: Option<&str>,
    release: bool,
    obj_path: &Path,
    ext: Option<&ext_build::ExtToolchain>,
) -> Result<(), ExitCode> {
    // A CPython import only means anything inside a CPython interpreter, so
    // a native build refuses it -- before codegen, which is allowed to
    // ignore the item precisely because of this gate. The gate lives inside
    // the frontend seam because that is where the per-file sources are: the
    // `I0403` has to be rendered against whichever file of the program
    // actually wrote the `import`, which for a multi-file program is
    // usually a dependency rather than the entry path.
    // W0 of #882 (#1156): an `--ext` build compiles the entry module under
    // the extension module's own name, so `__name__` inside the artifact
    // reads the name the artifact is importable as. That is a compile-time
    // constant derived from `-o`, where CPython takes the module object's
    // `__name__` from the import spec: the two agree for a top-level import
    // and diverge for a package submodule (`pkg/mod.abi3.so` imported as
    // `pkg.mod`). #1161 closes that by seeding from the live module object
    // in the `Py_mod_exec` slot; `docs/STDLIB_PLAN.md` carries the contract.
    // That makes the output
    // contract an input to the *frontend*, so it is resolved here rather
    // than only inside `plan_ext` below. The resolve runs twice --
    // deliberately: it is pure and cheap (`src/ext_output.rs` touches no
    // filesystem), and keeping `plan_ext` self-contained keeps its own
    // ordered failure sequence, and the tests that pin it, unchanged.
    //
    // The failure is *reported* here rather than carried forward: an
    // earlier shape dropped it to a `None` module name, which withheld the
    // `__name__` seed, so a program that reads `__name__` then failed with
    // an unrelated `T0021` -- propagated before `plan_ext` ever ran, which
    // hid the real `-o` diagnostic entirely. Reporting here does move the
    // output-path message ahead of any type error in the same invocation;
    // no test pins that pairing, and the output path is a property of the
    // command line rather than of the program, so it is the more useful of
    // the two to report first.
    let ext_module_name = match ext {
        Some(_) => Some(resolve_ext_output(out, target)?.module_name),
        None => None,
    };
    let typed_hir = match ext {
        Some(_) => resolve_frontend(path, ext_module_name.as_deref()),
        None => resolve_frontend_native(path),
    }
    .map_err(|failure| ExitCode::from(report_build_failure(failure)))?;
    // Everything `--ext` needs that can fail on the program itself or on
    // the host toolchain is resolved here, before codegen runs: a `C0003`
    // capability gap and a missing `Python.h` are both cheaper to report
    // than to discover after LLVM has emitted an object nobody can link.
    let ext_plan = match ext {
        Some(toolchain) => Some(plan_ext(
            path, out, target, &typed_hir, toolchain, obj_path,
        )?),
        None => None,
    };
    let mir = pycc_mir::build(&typed_hir);

    pycc_codegen::compile_to_object_with_options(
        &mir,
        obj_path,
        &pycc_codegen::CompileOptions {
            target_triple: target.map(str::to_string),
            release,
            ext: ext.is_some(),
        },
    )
    .map_err(|e| {
        eprintln!("error: codegen failed: {e}");
        ExitCode::from(1)
    })?;

    let rt_lib_dir = find_pycc_rt_lib_dir(target, release).map_err(|e| {
        eprintln!("error: {e}");
        ExitCode::from(2)
    })?;
    // One link site for both modes (#1036): `ext` contributes extra
    // arguments and a different output path, but the spawn, the
    // spawn-failure message and the exit-status mapping below stay shared,
    // so neither mode can drift into its own untested tail.
    let link_out: &Path = ext_plan
        .as_ref()
        .map_or(out, |plan| plan.artifact.as_path());
    let mut cmd = linker_command(target);
    if let Some(triple) = effective_link_target(target) {
        cmd.arg("-target").arg(triple);
    }
    if let Some(plan) = &ext_plan {
        cmd.args(&plan.compile_args).args(&plan.link_args);
    }
    cmd.arg(obj_path)
        .arg("-L")
        .arg(&rt_lib_dir)
        .arg("-lpycc_rt")
        .arg("-o")
        .arg(link_out);
    add_windows_system_libs(&mut cmd);
    add_linux_system_libs(&mut cmd);
    // #250: failing to *start* the driver (missing `cc`/`clang`, an
    // unusable toolchain) is an ordinary environment failure, not a pycc
    // invariant -- report it like the `find_pycc_rt_lib_dir` failure above
    // (CLI_SPEC.md's exit-2 invocation/environment class) instead of
    // panicking with a raw backtrace. Every cfg-gated `linker_command`
    // variant funnels into this one spawn site, so the message names the
    // exact driver that failed on this host.
    let status = cmd.status().map_err(|e| {
        // `to_string_lossy` without `pycc_diag::display_path`'s terminal
        // escaping, unlike the file-path diagnostics above: the program is
        // always one of `linker_command`'s compile-time-fixed values
        // (`cc`, or the D-028 bundled-clang path built from
        // `LLVM_SYS_221_PREFIX`), never user-controlled input.
        eprintln!(
            "error: could not run the linker driver `{}`: {e}",
            cmd.get_program().to_string_lossy()
        );
        ExitCode::from(2)
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(ExitCode::from(1))
    }
}

/// Everything `try_build`'s link step needs that is specific to `--ext`.
///
/// `artifact` replaces `OUT` as the linker's `-o`: `ext_output::resolve`
/// may have appended the platform's stable-ABI suffix to it, and CPython's
/// finder will only import a file whose name it recognizes.
#[derive(Debug)]
struct ExtPlan {
    artifact: std::path::PathBuf,
    compile_args: Vec<std::ffi::OsString>,
    link_args: Vec<std::ffi::OsString>,
}

/// Resolves `OUT` against the `--ext` output contract, reporting a rejected
/// path exactly as `plan_ext` does (`src/ext_output.rs` owns the messages).
///
/// Exists because `try_build` needs the resolved module name *before* the
/// frontend runs -- it is the entry module's `__name__` (#1156) -- while
/// `plan_ext` needs the resolved artifact path after it. `ext_output::resolve`
/// is pure, so calling it from both places costs nothing and keeps
/// `plan_ext`'s own failure ordering intact.
fn resolve_ext_output(out: &Path, target: Option<&str>) -> Result<ext_output::ExtOutput, ExitCode> {
    ext_output::resolve(
        out,
        &ext_build::ExtLinkPlatform::resolve(target).suffix_platform(),
    )
    .map_err(|e| {
        eprintln!("error: {}", e.message());
        ExitCode::from(2)
    })
}

/// Resolves the `--ext` output, export set and host toolchain, and writes
/// the two C files the link step compiles alongside the emitted object.
///
/// Ordered failure-cheapest-first, and every failure here happens before
/// codegen: the output contract and the `C0003` export gaps are properties
/// of the invocation and the program, the header probe is a property of the
/// host, and none of them becomes more informative for having run LLVM.
///
/// The C files are written next to `obj_path`, which the caller already
/// owns as a `pycc_scratch::ScratchDir` whose `Drop` removes the whole
/// directory on every exit path (#783). Nothing here writes beside the
/// user's `-o`, which stays the single persistent output.
fn plan_ext(
    source_path: &Path,
    out: &Path,
    target: Option<&str>,
    typed_hir: &pycc_hir::HirModule,
    toolchain: &ext_build::ExtToolchain,
    obj_path: &Path,
) -> Result<ExtPlan, ExitCode> {
    let platform = ext_build::ExtLinkPlatform::resolve(target);
    let output = ext_output::resolve(out, &platform.suffix_platform()).map_err(|e| {
        eprintln!("error: {}", e.message());
        ExitCode::from(2)
    })?;
    let exports = ext_build::collect_exports(typed_hir).map_err(|gaps| {
        // Span-less `C0003`s (a lowered `HirItem::Function` carries no
        // source range), so the empty source text below is never read:
        // `pycc_diag::render_human` renders a span-less diagnostic as
        // exactly `error[C0003]: <message>`.
        ExitCode::from(report_build_failure(frontend::FrontendFailure::compile(
            &source_path.display().to_string(),
            "",
            gaps,
        )))
    })?;
    // Every function whose return type is a buffer except the shapes the
    // `--ext` boundary admits: Part 2b of #1142 (#1164) admitted a public
    // *module-level* export, and #1174 widened that to the whole export
    // set, a public method of a public class included. What is left --
    // a private function, a private method, a method of a private or
    // exception class, a specialization -- would otherwise reach codegen's
    // own panic for a `memoryview`-typed call result;
    // `refuse_in_ext_mode`'s own doc comment carries the argument in full,
    // including why #1174's interception at the four method-resolution
    // exits is what makes the widening safe. It is handed the export set
    // because a buffer return type is carriable now, so `collect_exports`
    // above no longer refuses one on its own.
    //
    // Position: this runs inside `plan_ext`, which `run_build` calls
    // *before* `compile_to_object_with_options`, so a refusal here is what
    // keeps that panic unreachable rather than merely unlikely.
    memoryview_mode::refuse_in_ext_mode(typed_hir, &exports).map_err(|gaps| {
        ExitCode::from(report_build_failure(frontend::FrontendFailure::compile(
            &source_path.display().to_string(),
            "",
            gaps,
        )))
    })?;
    let probe = toolchain.probe().map_err(|e| {
        eprintln!("error: {e}");
        ExitCode::from(2)
    })?;
    let shim = obj_path.with_file_name(ext_build::SHIM_C_NAME);
    let inc = obj_path.with_file_name(ext_build::EXPORTS_INC_NAME);
    // Rendered into a binding first so the call below fits one line: a
    // multi-line `foo(\n  ..\n)?;` puts the `?`'s early-return arm on a
    // line of its own, which no passing build ever executes and which
    // `scripts/check_diff_coverage.py` then reports as an uncovered
    // changed line (D-242 rule 1).
    let classes = ext_build::collect_user_exception_classes(typed_hir);
    let publications = ext_build::collect_class_publications(typed_hir, &exports);
    let ctors = ext_build::collect_constructors(typed_hir, &publications);
    let inc_body = ext_build::generate_exports_inc(
        &output.module_name,
        &exports,
        &classes,
        &publications,
        &ctors,
    );
    write_ext_source(&shim, ext_build::SHIM_C)?;
    write_ext_source(&inc, &inc_body)?;
    Ok(ExtPlan {
        compile_args: ext_build::ext_compile_args(platform, &probe.include, &shim),
        link_args: ext_build::ext_link_args(platform, &probe.libs),
        artifact: output.artifact,
    })
}

/// Writes one generated C file into the caller-owned scratch directory.
/// A failure here is an environment failure (an unwritable scratch), not a
/// pycc invariant -- reported at exit 2 like `create_scratch`'s own.
fn write_ext_source(path: &Path, contents: &str) -> Result<(), ExitCode> {
    std::fs::write(path, contents).map_err(|e| {
        eprintln!(
            "error: could not write the --ext build source `{}`: {e}",
            pycc_diag::display_path(&path.to_string_lossy())
        );
        ExitCode::from(2)
    })
}

/// Windows has no `cc` by default (that's a Unix convention -- MSVC's own
/// tools are `cl.exe`/`link.exe`), so this uses the `clang` bundled with the
/// same LLVM install `LLVM_SYS_221_PREFIX` already points builds at (see
/// D-015/D-027) -- clang's driver translates GCC-style `-l`/`-L`/`-o` flags
/// into the `link.exe` invocation this target needs, verified empirically
/// (`clang -target x86_64-pc-windows-msvc -### ...`) rather than assumed.
/// Elsewhere, the system `cc` already works for the no-`--target` case
/// (verified: native-build-test passes on both Linux architectures and
/// macOS) -- see this function's other two cfg-gated bodies below for
/// what changes when `--target` is given (D-031).
#[cfg(windows)]
fn linker_command(_target: Option<&str>) -> std::process::Command {
    let clang = std::path::Path::new(env!("LLVM_SYS_221_PREFIX"))
        .join("bin")
        .join("clang.exe");
    std::process::Command::new(clang)
}

/// Linux's default `cc` is GCC (confirmed: Ubuntu's `ubuntu-latest`/
/// `ubuntu-24.04-arm` runners), and GCC's driver rejects clang-only
/// `-target <triple>` syntax outright ("unrecognized command-line option
/// '-target'") -- for *any* value, even a triple naming this same host
/// (D-031). Only route through the bundled clang when the caller actually
/// asked for one: `<LLVM_SYS_221_PREFIX>/bin/clang` is the same
/// apt.llvm.org prefix layout `ci.yml`'s "Install LLVM 22 (Linux)" step
/// already installs for `inkwell` itself, so this needs no new install.
/// The plain, no-target case keeps using the system `cc` unchanged --
/// verified working there already (D-028 point 1).
#[cfg(target_os = "linux")]
fn linker_command(target: Option<&str>) -> std::process::Command {
    if target.is_some() {
        let clang = std::path::Path::new(env!("LLVM_SYS_221_PREFIX"))
            .join("bin")
            .join("clang");
        std::process::Command::new(clang)
    } else {
        std::process::Command::new("cc")
    }
}

/// macOS's system `cc` already *is* Apple clang, and D-026 already proved
/// it handles `--target` correctly for the cross-arch pair CI verifies
/// (`cross-compile-build`/`cross-compile-verify`) -- left exactly as-is
/// regardless of `target`, rather than folded into Linux's branch above,
/// so this fix doesn't change a path that's already tested working.
#[cfg(all(not(windows), not(target_os = "linux")))]
fn linker_command(_target: Option<&str>) -> std::process::Command {
    std::process::Command::new("cc")
}

/// A bare `clang.exe` invocation with no `-target` flag was observed
/// (D-028) resolving inconsistently: some invocations correctly select
/// MSVC's `lld-link`, others silently fall back to a MinGW/GCC toolchain
/// discovered on `PATH` (`C:\mingw64`) -- which cannot link `pycc_rt.lib`'s
/// MSVC-ABI symbols (`__imp_closesocket`, `__chkstk`, the MSVC RTTI
/// vtable), producing a wall of "undefined reference" errors from GNU
/// `ld`/`collect2` instead of a normal MSVC link. This is the only Windows
/// target v0.1 supports (the Tier-1 matrix), so there's no reason to let
/// the linker guess: force it explicitly instead of relying on clang's
/// bare-invocation default, which this evidence shows is not reliable.
#[cfg(windows)]
fn effective_link_target(target: Option<&str>) -> Option<&str> {
    Some(target.unwrap_or("x86_64-pc-windows-msvc"))
}

#[cfg(not(windows))]
fn effective_link_target(target: Option<&str>) -> Option<&str> {
    target
}

/// `pycc_rt.lib` is a Rust `staticlib` -- linking it via `cargo`/`rustc`
/// (as happens when building `pycc.exe` itself) automatically adds every
/// Windows system library Rust's std transitively needs; invoking the
/// linker driver directly here (see `linker_command` above) does not. This
/// set is the exact one rustc itself passed when linking `pycc.exe` on
/// this same CI runner (D-028) -- confirmed from that link's own log, not
/// guessed. `#[cfg(not(windows))]`'s no-op keeps the other platforms,
/// where system libs are found automatically, unaffected.
#[cfg(windows)]
fn add_windows_system_libs(cmd: &mut std::process::Command) {
    for lib in [
        "ws2_32",
        "ntdll",
        "userenv",
        "advapi32",
        "shell32",
        "ole32",
        "uuid",
        "psapi",
        "dbghelp",
        "kernel32",
        "legacy_stdio_definitions",
    ] {
        cmd.arg(format!("-l{lib}"));
    }
}

#[cfg(not(windows))]
fn add_windows_system_libs(_cmd: &mut std::process::Command) {}

/// `pycc_rt`'s `f64::powf` (used by `float ** float`, see D-001/RUNTIME.md's
/// float support) lowers to a call to the C library's `pow` -- part of
/// `libm`, not `libc`. macOS folds `libm` into `libSystem`, which every link
/// already pulls in implicitly, and Windows's UCRT bundles it too, so
/// neither platform needs an explicit flag (confirmed: this exact
/// unmodified code already links and runs `native-build-test` on both). On
/// Linux, GCC's and clang's default driver invocation does not add `-lm` on
/// its own: PR-5's own CI run surfaced this directly (`native-build-test
/// (ubuntu-latest, x86_64-unknown-linux-gnu)` and `(ubuntu-24.04-arm,
/// aarch64-unknown-linux-gnu)` both failed link with "undefined reference to
/// `pow'"), so every Linux link needs `-lm` explicitly, both with and
/// without `--target` (see `linker_command`'s two Linux-reachable paths
/// above).
#[cfg(target_os = "linux")]
fn add_linux_system_libs(cmd: &mut std::process::Command) {
    cmd.arg("-lm");
}

#[cfg(not(target_os = "linux"))]
fn add_linux_system_libs(_cmd: &mut std::process::Command) {}

/// Locates `pycc_rt`'s static library for the requested target and
/// profile, honoring Cargo's target-directory environment variables.
///
/// A thin driver-side wrapper: the resolution rules, the precedence
/// `CARGO_TARGET_DIR` participates in, and the error messages all live in
/// [`pycc_codegen::artifact_layout`], which `pycc_codegen`'s own
/// link-and-run tests and `tests/slice0.rs` share so that every artifact
/// lookup in this workspace agrees on where Cargo put things.
///
/// `env!("CARGO_MANIFEST_DIR")` is the `pycc` package directory, which is
/// also the workspace root, so it is the correct fallback anchor when
/// neither `CARGO_TARGET_DIR` nor `CARGO_BUILD_TARGET_DIR` redirects the
/// target directory.
pub(crate) fn find_pycc_rt_lib_dir(
    target: Option<&str>,
    release: bool,
) -> Result<std::path::PathBuf, String> {
    let target_root = pycc_codegen::artifact_layout::resolve_cargo_target_root(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        pycc_codegen::artifact_layout::cargo_target_dir_from_env,
    );
    pycc_codegen::artifact_layout::find_pycc_rt_lib_dir_in(
        &target_root,
        target,
        release,
        std::path::Path::exists,
    )
}

#[cfg(test)]
mod try_build_release_isolation_tests {
    use super::*;
    use pycc_scratch::ScratchDir;

    /// Regression test for a bug caught in review: `try_build` used to call
    /// `resolve_release_flag` internally, so `run`'s hardcoded `false` (see
    /// `run`'s own doc comment) still got silently upgraded to `true` by a
    /// neighboring release-profile `pycc.toml`, with no `--release` flag on
    /// `run` to override it. Proves the object `try_build` actually emits
    /// for `release: false` is identical regardless of a neighboring
    /// `pycc.toml` naming `opt = "release"`, by comparing it against the
    /// same source's MIR compiled directly through `pycc_codegen` with
    /// `release: false`. Object-byte equality is appropriate for this
    /// isolation claim because both paths are intentionally debug codegen;
    /// `pycc_codegen`'s own `release_mode_actually_runs_llvm_optimization_
    /// passes` test proves the separate release claim by observing the exact
    /// pipeline and used/unused declaration state before object emission.
    /// Deliberately not a final-linked-binary-
    /// size comparison: `docs/AGENT_RETROSPECTIVE.md`'s 2026-07-28 entry
    /// found that proxy has no signal at that level (a large statically-
    /// linked runtime plus OS segment-alignment padding absorbs the
    /// relevant code-size delta, and embedded path-string lengths
    /// independently perturb it). Calling `try_build` directly (rather than
    /// spawning `pycc` as a subprocess) is what makes this reliable:
    /// since #783, `try_build` takes `obj_path` by injection, so this test
    /// supplies a path inside its own `ScratchDir` -- which outlives the
    /// call, exactly the ownership contract `try_build`'s doc comment
    /// states -- and reads the emitted object back from a location it
    /// controls, with no shared process-global path to race on.
    #[test]
    fn try_build_ignores_a_neighboring_release_pycc_toml_when_given_release_false() {
        let dir = ScratchDir::new("release_isolation").expect("failed to create scratch dir");
        let src = dir.join("main.py");
        std::fs::write(&src, "def main() -> None:\n    print(42)\n\nmain()\n").unwrap();
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
             [build]\nopt = \"release\"\n",
        )
        .unwrap();
        let out = dir.join("out");
        let obj_path = dir.join("obj.o");

        // Exactly what `run()` does: `release: false` straight through.
        try_build(&src, &out, None, false, &obj_path, None).expect("try_build should succeed");

        let obj_bytes = std::fs::read(&obj_path).expect("try_build's temp object should exist");

        // Independently compiled reference: the same source's MIR, built
        // through the exact same frontend pipeline try_build itself uses,
        // compiled directly with release=false.
        // The same `__name__` value `resolve_frontend_native` gives the
        // native path `try_build` just took, so the two objects stay
        // byte-identical (#1156).
        let typed_hir = resolve_frontend(&src, Some(frontend::NATIVE_MODULE_NAME))
            .ok()
            .expect("fixture source should type-check");
        let mir = pycc_mir::build(&typed_hir);
        let ref_obj_path = dir.join("reference.o");
        pycc_codegen::compile_to_object(&mir, &ref_obj_path, None, false)
            .expect("reference codegen should succeed");
        let ref_obj_bytes = std::fs::read(&ref_obj_path).unwrap();

        assert_eq!(
            obj_bytes, ref_obj_bytes,
            "try_build(release: false) must ignore a neighboring pycc.toml's \
             `opt = \"release\"` entirely -- only main()'s Command::Build arm may \
             consult it, before try_build is ever called"
        );
    }

    /// Exercises `BindingState::Maybe` in `join_match_branches` through the
    /// `pycc` crate's own test binary (not just `pycc_types`'s unit tests).
    /// A match case body that assigns `y` only inside an `if` without `else`
    /// leaves `y` as `Maybe` in that case's env, so the join calls `ty()` on
    /// a `Maybe` binding.
    #[test]
    fn check_and_resolve_match_with_maybe_binding_type_checks() {
        let source = "def f(x: int) -> None:\n    match x:\n        case 0:\n            if x > 0:\n                y = 1\n        case _:\n            pass\n";
        let module = pycc_parser::parse(source).expect("test fixture must parse");
        let hir = pycc_hir::lower_checked(&module).expect("test fixture must lower");
        let result = pycc_types::check_and_resolve(&hir);
        assert!(result.is_ok(), "match with Maybe binding should type-check");
    }
}

#[cfg(test)]
mod ext_build_wiring_tests {
    use super::*;
    use ext_build::{ExtProbe, ExtToolchain};
    use pycc_scratch::ScratchDir;

    /// A toolchain whose headers are `dir` itself, holding a `Python.h` that
    /// is not one: a single `#error` line. Every step up to and including
    /// the compiler spawn then runs for real, and the build fails
    /// deterministically inside `cc` on any host, with no CPython installed
    /// and nothing `#[ignore]`d. That is the only way the ext branch's
    /// effectful tail earns coverage: `.github/workflows/ci.yml`'s coverage
    /// job runs `llvm-cov` without `--include-ignored`.
    ///
    /// The stub file is what keeps that reachable. The probe now rejects a
    /// header directory with no `Python.h` as an environment failure, so an
    /// empty directory would stop the build two steps earlier and leave the
    /// whole tail uncovered -- the failure has to come from the *contents*
    /// of a header, which is a compile error, not from its absence, which is
    /// a broken build environment.
    fn header_less_toolchain(dir: &Path) -> ExtToolchain {
        std::fs::write(
            dir.join("Python.h"),
            "#error pycc test fixture: not a real Python.h\n",
        )
        .expect("write the stub header");
        ExtToolchain::with_probe(
            "pycc-unused-interpreter",
            ExtProbe {
                version: ext_build::MIN_PYTHON,
                include: dir.to_path_buf(),
                libs: dir.join("libs"),
            },
        )
    }

    fn write_source(dir: &Path, body: &str) -> std::path::PathBuf {
        let src = dir.join("m.py");
        std::fs::write(&src, body).expect("write source");
        src
    }

    fn typed(src: &Path) -> pycc_hir::HirModule {
        resolve_frontend(src, Some(frontend::NATIVE_MODULE_NAME))
            .unwrap_or_else(|_| panic!("the fixture must type-check"))
    }

    #[test]
    fn a_planned_ext_build_writes_both_c_files_beside_the_object() {
        let dir = ScratchDir::new("ext_plan").expect("scratch");
        let src = write_source(&dir, "def square(x: int) -> int:\n    return x * x\n");
        let obj = dir.join("main.o");
        let plan = plan_ext(
            &src,
            &dir.join("fastmath"),
            None,
            &typed(&src),
            &header_less_toolchain(&dir),
            &obj,
        )
        .expect("an int-only program plans cleanly");

        let inc = std::fs::read_to_string(dir.join(ext_build::EXPORTS_INC_NAME))
            .expect("the generated companion is written next to the object");
        assert!(
            inc.contains("#define PYCC_EXT_MODULE_NAME fastmath\n"),
            "{inc}"
        );
        assert!(inc.contains("fnptr_square"), "{inc}");
        let shim = std::fs::read_to_string(dir.join(ext_build::SHIM_C_NAME))
            .expect("the fixed shim is written next to the object");
        assert_eq!(shim, ext_build::SHIM_C);

        // The artifact is the resolved output, not `OUT` as given: comparing
        // `PathBuf`s built with `Path::join`, never rendered strings. This
        // call passes no target, so the suffix follows the *build host* --
        // two legal values, enumerated rather than branched on, so the
        // assertion holds on every Tier-1 host without a `cfg` arm that only
        // one of them ever executes.
        assert!(
            [dir.join("fastmath.abi3.so"), dir.join("fastmath.pyd")].contains(&plan.artifact),
            "{:?}",
            plan.artifact
        );
        assert!(plan.compile_args.contains(&std::ffi::OsString::from("-I")));
        assert!(!plan.link_args.is_empty());
    }

    #[test]
    fn a_windows_target_plans_a_pyd_from_this_host() {
        let dir = ScratchDir::new("ext_plan_win").expect("scratch");
        let src = write_source(&dir, "def f() -> int:\n    return 1\n");
        let plan = plan_ext(
            &src,
            &dir.join("m"),
            Some("x86_64-pc-windows-msvc"),
            &typed(&src),
            &header_less_toolchain(&dir),
            &dir.join("main.o"),
        )
        .expect("a cross-target plan needs nothing from this host");
        assert_eq!(plan.artifact, dir.join("m.pyd"));
        assert!(
            plan.link_args
                .contains(&std::ffi::OsString::from("-lpython3"))
        );
    }

    #[test]
    fn an_output_path_the_ext_contract_rejects_fails_before_the_export_scan() {
        let dir = ScratchDir::new("ext_plan_out").expect("scratch");
        let src = write_source(&dir, "def f() -> int:\n    return 1\n");
        let code = plan_ext(
            &src,
            Path::new("/"),
            None,
            &typed(&src),
            &header_less_toolchain(&dir),
            &dir.join("main.o"),
        )
        .expect_err("`/` names no module");
        assert_eq!(code, ExitCode::from(2));
    }

    #[test]
    fn a_public_function_the_boundary_cannot_carry_fails_the_build_with_a_diagnostic() {
        let dir = ScratchDir::new("ext_plan_gap").expect("scratch");
        // `list[int]`, not `str`: #1049 made `str` carriable in both
        // positions, so the old fixture no longer reaches a gap.
        let src = write_source(&dir, "def greet(x: list[int]) -> int:\n    return 1\n");
        let code = plan_ext(
            &src,
            &dir.join("m"),
            None,
            &typed(&src),
            &header_less_toolchain(&dir),
            &dir.join("main.o"),
        )
        .expect_err("list is not bridged");
        assert_eq!(code, ExitCode::from(1));
    }

    #[test]
    fn an_unusable_cpython_toolchain_fails_before_codegen() {
        let dir = ScratchDir::new("ext_plan_probe").expect("scratch");
        let src = write_source(&dir, "def f() -> int:\n    return 1\n");
        let toolchain = ExtToolchain::with_probe(
            "pycc-unused-interpreter",
            ExtProbe {
                version: ext_build::MIN_PYTHON,
                include: dir.join("no-such-include"),
                libs: dir.join("libs"),
            },
        );
        let code = plan_ext(
            &src,
            &dir.join("m"),
            None,
            &typed(&src),
            &toolchain,
            &dir.join("main.o"),
        )
        .expect_err("a missing header directory is an environment failure");
        assert_eq!(code, ExitCode::from(2));
    }

    #[test]
    fn a_scratch_directory_that_cannot_be_written_is_an_environment_failure() {
        let dir = ScratchDir::new("ext_plan_write").expect("scratch");
        let src = write_source(&dir, "def f() -> int:\n    return 1\n");
        // An object path inside a directory that does not exist: the C
        // files land beside it, so writing them is what fails.
        let code = plan_ext(
            &src,
            &dir.join("m"),
            None,
            &typed(&src),
            &header_less_toolchain(&dir),
            &dir.join("no-such-dir").join("main.o"),
        )
        .expect_err("an unwritable scratch is an environment failure");
        assert_eq!(code, ExitCode::from(2));
    }

    /// A rejected `-o` is reported as the output-contract failure it is,
    /// even when the source reads `__name__` (#1156).
    ///
    /// The regression this pins: while the resolve's error was dropped to a
    /// `None` module name, `dunder_name::seed_item` withheld the seed, the
    /// type checker then rejected the program with `T0021` (exit 1), and
    /// that unrelated diagnostic reached the user instead of the `-o` one.
    /// Exit 2 is `resolve_ext_output`'s own code, so it distinguishes the
    /// two outcomes without matching on rendered message text.
    #[test]
    fn a_rejected_ext_output_path_is_reported_even_when_the_source_reads_dunder_name() {
        let dir = ScratchDir::new("ext_out_reject").expect("scratch");
        let src = write_source(&dir, "def f() -> str:\n    return __name__\n");
        let code = try_build(
            &src,
            // `/` has no file-name component, so no module name can be
            // derived from it (`ext_output::ExtOutputError::NoFileName`).
            Path::new("/"),
            None,
            false,
            &dir.join("main.o"),
            Some(&header_less_toolchain(&dir)),
        )
        .expect_err("`/` names no module");
        assert_eq!(code, ExitCode::from(2));
    }

    /// The whole `--ext` tail, end to end: output resolution, export scan,
    /// toolchain probe, both C writes, codegen through
    /// `CompileOptions { ext: true, .. }`, the runtime-library lookup, the
    /// platform link argv, and the shared spawn. It fails inside `cc`,
    /// because the include directory this test supplies holds no `Python.h`
    /// -- which is exactly the point: every one of those steps ran.
    #[test]
    fn an_ext_build_runs_the_whole_tail_and_fails_in_the_compiler_without_python_headers() {
        let dir = ScratchDir::new("ext_try_build").expect("scratch");
        let src = write_source(&dir, "def square(x: int) -> int:\n    return x * x\n");
        let obj = dir.join("main.o");
        let code = try_build(
            &src,
            &dir.join("fastmath"),
            None,
            false,
            &obj,
            Some(&header_less_toolchain(&dir)),
        )
        .expect_err("no Python.h means the compiler rejects the shim");
        assert_eq!(code, ExitCode::from(1));
        // Codegen really ran under `ext: true`, and really emitted the
        // module-body symbol the shim calls instead of `main`.
        let object = std::fs::read(&obj).expect("the object was emitted before the link");
        assert!(
            object
                .windows(pycc_codegen::EXT_MODULE_EXEC_SYMBOL.len())
                .any(|window| window == pycc_codegen::EXT_MODULE_EXEC_SYMBOL.as_bytes()),
            "the ext object must export the module-body symbol"
        );
    }
}
