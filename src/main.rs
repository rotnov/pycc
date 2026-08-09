mod cli;
mod project_config;
mod source;

use clap::Parser;
use cli::{Cli, Command, ErrorFormat, OutputFormat};
use pycc_diag::Diagnostic;
use std::path::Path;
use std::process::ExitCode;

/// The five Tier-1 targets, in ARCHITECTURE.md's own "Cross-platform (hard
/// requirement)" table order (Linux, macOS, Windows; x86_64 before aarch64
/// within each OS pair). `pycc version --verbose` reports this list per
/// CLI_SPEC.md's command table; nothing else in the codebase enumerates
/// Tier-1 at runtime, so this constant is that table's only code mirror and
/// changes only when the table does.
const TIER1_TARGETS: [&str; 5] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            path,
            out,
            target,
            release,
        } => {
            // Resolved here, not inside `try_build`: this consumption point
            // (a neighboring `pycc.toml`'s `[build] opt = "release"` as a
            // default profile) is scoped to `pycc build` specifically --
            // CLI_SPEC.md/ROADMAP.md both document it that way, and `run`
            // has no `--release` flag of its own to override it with if a
            // user doesn't want that default applied. Resolving it here,
            // before `try_build` ever runs, is what keeps `run`'s own
            // hardcoded `false` (below) actually final.
            let release = resolve_release_flag(release, Path::new(&path));
            match try_build(&path, &out, target.as_deref(), release) {
                Ok(()) => ExitCode::SUCCESS,
                Err(code) => code,
            }
        }
        Command::Run { path } => run(&path),
        Command::Version { verbose } => {
            // `pycc {version}` and `rustc {rust-version}` come from the
            // manifest (`CARGO_PKG_VERSION`, `CARGO_PKG_RUST_VERSION`) so the
            // line can't silently rot when either bumps; the repository keeps
            // `rust-version` in lockstep with `rust-toolchain.toml`. "LLVM
            // 22.1.1" stays a literal deliberately: it states D-015's pinned
            // contract value, and whether the *installed* LLVM actually
            // matches that pin is #75's open scope, not this line's job.
            println!(
                "pycc {} (rustc {}, LLVM 22.1.1)",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_RUST_VERSION"),
            );
            if verbose {
                // CLI_SPEC.md's `pycc version --verbose` row promises the
                // target list; the set and order mirror the one authoritative
                // Tier-1 table in ARCHITECTURE.md ("Cross-platform (hard
                // requirement)"), which has no runtime representation
                // anywhere else in the codebase.
                println!("tier-1 targets:");
                for target in TIER1_TARGETS {
                    println!("  {target}");
                }
            }
            ExitCode::SUCCESS
        }
        Command::Check {
            paths,
            error_format,
        } => check_paths(&paths, error_format),
        Command::Init { name } => {
            let cwd = std::env::current_dir().expect("current directory must be readable");
            match init(name.as_deref(), &cwd) {
                Ok(()) => {
                    println!("Created pycc.toml and src/main.py");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: pycc init failed: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Command::Explain { code, format } => explain(&code, format),
        Command::Test | Command::Clean => {
            eprintln!("pycc: this subcommand is not yet implemented");
            ExitCode::from(2)
        }
    }
}

/// `pycc explain <code> [--format human|json]`: prints the code, severity,
/// a one-line summary, and a longer explanation with a worked example for
/// a recognized diagnostic code (D-150, `pycc_diag::explain`). Exits `0`
/// for a recognized code in either format. Exits `2` for an unrecognized
/// code, with a plain stderr message regardless of `--format` -- an
/// unrecognized code is an out-of-band invocation failure, not a
/// diagnostic occurrence, so it is never subject to `--format` the way
/// `check`'s own out-of-band `FrontendFailure::Input` class ("could not
/// read ...") is never subject to `--error-format` either.
fn explain(code: &str, format: OutputFormat) -> ExitCode {
    match pycc_diag::explain::find(code) {
        Some(entry) => {
            match format {
                OutputFormat::Human => {
                    print!("{}", pycc_diag::explain::render_explanation_human(entry));
                }
                OutputFormat::Json => {
                    println!("{}", pycc_diag::explain::render_explanation_json(entry));
                }
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("error: unknown diagnostic code `{code}`");
            ExitCode::from(2)
        }
    }
}

/// `pycc init [NAME]`: testable core of the `Command::Init` arm. Takes
/// `dir` as a parameter instead of calling `std::env::current_dir()`
/// internally -- matching `find_pycc_rt_lib_dir`/`find_pycc_rt_lib_dir_in`'s
/// existing dependency-injection split below -- so a test can exercise
/// `project_config::scaffold`'s error path (an existing `pycc.toml`,
/// #237's refusal contract) without mutating this test process's real,
/// shared working directory.
fn init(name: Option<&str>, dir: &Path) -> Result<(), String> {
    project_config::scaffold(name, dir).map_err(|e| e.to_string())
}

/// `pycc check`: parse + HIR-lowering + type-checking only, no codegen --
/// CLI_SPEC.md's contract for this subcommand (ruff-fast, no codegen).
/// `error_format`: "human" (default) or "json", matching CLI_SPEC.md's
/// `--error-format` flag. Compile diagnostics are printed to stdout; input
/// errors are printed to stderr. Every supplied file is checked before the
/// highest-precedence exit code is returned.
fn check_paths(paths: &[std::path::PathBuf], error_format: ErrorFormat) -> ExitCode {
    if paths.is_empty() {
        eprintln!("error: `pycc check` requires at least one Python file in v0.1");
        return ExitCode::from(2);
    }

    let mut exit_code = 0;
    for path in paths {
        if let Err(failure) = check_frontend(path) {
            exit_code = exit_code.max(report_check_failure(path, failure, error_format));
        }
    }
    ExitCode::from(exit_code)
}

/// `Ok(())` on success, `Err(code)` carrying the exit code to use on
/// failure. `?` inside this function needs a `Result`, not an `ExitCode`
/// directly -- `run` then just propagates whatever `Err` it gets.
///
/// `target`: `None` builds for the host's own default target (`run` always
/// passes this, since running a cross-compiled binary on this host makes
/// no sense). `Some(triple)` cross-compiles -- see `find_pycc_rt_lib_dir`
/// for what that requires to actually be available.
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
fn try_build(path: &str, out: &str, target: Option<&str>, release: bool) -> Result<(), ExitCode> {
    let path = Path::new(path);
    let typed_hir = resolve_frontend(path)
        .map_err(|failure| ExitCode::from(report_build_failure(path, failure)))?;
    let mir = pycc_mir::build(&typed_hir);

    let obj_path = std::env::temp_dir().join(format!("pycc_obj_{}.o", std::process::id()));
    pycc_codegen::compile_to_object(&mir, &obj_path, target, release).map_err(|e| {
        eprintln!("error: codegen failed: {e}");
        ExitCode::from(1)
    })?;

    let rt_lib_dir = find_pycc_rt_lib_dir(target, release).map_err(|e| {
        eprintln!("error: {e}");
        ExitCode::from(2)
    })?;
    let mut cmd = linker_command(target);
    if let Some(triple) = effective_link_target(target) {
        cmd.arg("-target").arg(triple);
    }
    cmd.arg(&obj_path)
        .arg("-L")
        .arg(&rt_lib_dir)
        .arg("-lpycc_rt")
        .arg("-o")
        .arg(out);
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

/// Resolves `pycc build`'s effective release/debug profile -- called only
/// from `main()`'s `Command::Build` arm, before `try_build` runs (`run` has
/// no `--release` flag and never calls this, so it's never subject to a
/// neighboring `pycc.toml`'s default; see `try_build`'s own doc comment).
/// An explicit `--release` (`explicit_release: true`) always wins outright
/// -- this is not project-mode directory resolution (`docs/CLI_SPEC.md`'s
/// deferred "PATH = ... project directory" case), just a narrow
/// default-profile consumption point (this PR's plan, Task 3 Step 6a,
/// moved here from Task 2's original Step 8 once this task's own `release`
/// field existed for it to combine with).
///
/// Otherwise, this looks for a `pycc.toml` next to `source_path` and uses
/// its `[build] opt == "release"` as the default. Every other outcome --
/// no `pycc.toml` there, one that fails to parse (`project_config::parse`
/// already rejects malformed TOML and any non-"3.14" `python`), or a
/// `[build] opt` value other than `"release"` (including no `[build]`
/// section at all, whose `opt` defaults to `None`) -- falls back to
/// `false`. A malformed neighboring `pycc.toml` must not abort an
/// otherwise-valid build over an optional default it doesn't even need.
fn resolve_release_flag(explicit_release: bool, source_path: &Path) -> bool {
    if explicit_release {
        return true;
    }
    let Some(dir) = source_path.parent() else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(dir.join("pycc.toml")) else {
        return false;
    };
    match project_config::parse(&contents) {
        Ok(config) => config.build.opt.as_deref() == Some("release"),
        Err(_) => false,
    }
}

enum FrontendFailure {
    Input(String),
    Compile {
        // Boxed (D-152): `Diagnostic` grew a `help: Option<String>` field,
        // which pushed this variant's inline size past clippy's
        // `result_large_err` threshold. Boxing keeps `Result<_,
        // FrontendFailure>` small regardless of how large `Diagnostic`
        // itself grows in the future.
        diagnostic: Box<Diagnostic>,
        source: String,
    },
}

fn lower_frontend(path: &Path) -> Result<(pycc_hir::HirModule, String), FrontendFailure> {
    let bytes = std::fs::read(path).map_err(|error| FrontendFailure::Input(error.to_string()))?;
    let source = source::decode_python_source(&bytes).map_err(FrontendFailure::Input)?;
    let module = match pycc_parser::parse(&source) {
        Ok(module) => module,
        Err(diagnostic) => {
            return Err(FrontendFailure::Compile {
                diagnostic: Box::new(diagnostic),
                source,
            });
        }
    };
    let hir = match pycc_hir::lower_checked(&module) {
        Ok(hir) => hir,
        Err(diagnostic) => {
            return Err(FrontendFailure::Compile {
                diagnostic: Box::new(diagnostic),
                source,
            });
        }
    };
    Ok((hir, source))
}

fn check_frontend(path: &Path) -> Result<(), FrontendFailure> {
    let (hir, source) = lower_frontend(path)?;
    pycc_types::check(&hir).map_err(|diagnostic| FrontendFailure::Compile {
        diagnostic: Box::new(diagnostic),
        source,
    })
}

fn resolve_frontend(path: &Path) -> Result<pycc_hir::HirModule, FrontendFailure> {
    let (hir, source) = lower_frontend(path)?;
    pycc_types::check_and_resolve(&hir).map_err(|diagnostic| FrontendFailure::Compile {
        diagnostic: Box::new(diagnostic),
        source,
    })
}

fn report_check_failure(path: &Path, failure: FrontendFailure, error_format: ErrorFormat) -> u8 {
    let path = path.to_string_lossy();
    match failure {
        FrontendFailure::Input(message) => {
            eprintln!(
                "error: could not read `{}`: {message}",
                pycc_diag::display_path(&path)
            );
            2
        }
        FrontendFailure::Compile { diagnostic, source } => {
            match error_format {
                ErrorFormat::Human => {
                    print!("{}", pycc_diag::render_human(&diagnostic, &path, &source));
                }
                ErrorFormat::Json => {
                    println!("{}", pycc_diag::render_json(&diagnostic, &path, &source));
                }
            }
            1
        }
    }
}

fn report_build_failure(path: &Path, failure: FrontendFailure) -> u8 {
    let path = path.to_string_lossy();
    match failure {
        FrontendFailure::Input(message) => {
            eprintln!(
                "error: could not read `{}`: {message}",
                pycc_diag::display_path(&path)
            );
            2
        }
        FrontendFailure::Compile { diagnostic, source } => {
            eprint!("{}", pycc_diag::render_human(&diagnostic, &path, &source));
            1
        }
    }
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

/// `pycc run` has no `--release` flag (undocumented in CLI_SPEC.md) and
/// always builds in the debug profile: the hardcoded `false` below reaches
/// `try_build` directly, which -- unlike `main()`'s `Command::Build` arm --
/// never consults a neighboring `pycc.toml`'s `[build] opt = "release"`
/// default, so this stays unconditional regardless of what any nearby
/// `pycc.toml` names.
fn run(path: &str) -> ExitCode {
    let out = std::env::temp_dir().join(format!("pycc_run_{}", std::process::id()));
    if let Err(code) = try_build(
        path,
        out.to_str().expect("temp dir path should be valid UTF-8"),
        None,
        false,
    ) {
        return code;
    }
    let status = std::process::Command::new(&out)
        .status()
        .expect("built binary should run");
    // Generated programs currently have no user-controlled non-zero exit
    // status. Any unsuccessful termination is therefore a runtime panic,
    // trap, or uncaught failure and maps to CLI_SPEC.md's stable 101 on
    // every platform, including Unix signal termination where `code()` is
    // `None` and Windows abort statuses that do not fit in a u8.
    ExitCode::from(if status.success() { 0 } else { 101 })
}

/// `target: None` (the common case) returns this workspace's ordinary
/// `target/debug/` or `target/release/` -- always populated once `pycc_rt`
/// has been built for the requested profile (see that crate's own doc
/// comment on the build-order requirement).
///
/// `target: Some(triple)` looks in `target/<triple>/debug/` or
/// `target/<triple>/release/` -- where `cargo build [--release] --target
/// <triple> -p pycc_rt` (after `rustup target add <triple>` if needed) puts
/// it. Cross-compiling `pycc_rt` itself for the requested target is not
/// done automatically here: unlike the ordinary case, there's no single
/// always-correct way to trigger it that doesn't either require
/// duplicating `rustup`/`cargo`'s own target-management logic or risk the
/// same build-lock deadlock documented on `pycc_rt` (see that crate's own
/// doc comment). Failing with a clear, actionable message is better than a
/// confusing linker error about a missing `-lpycc_rt`.
///
/// `release`: selects which of `pycc_rt`'s own two builds to link against.
/// Before this parameter existed, this function unconditionally linked the
/// debug build regardless of `pycc build --release` -- a real, unambiguous
/// bug (not a design choice) caught while investigating why a `--release`
/// nbody benchmark's speedup ratio fell far short of expectations
/// (`tests/nbody_bench.rs`): `--release` was optimizing the compiled
/// module's own LLVM IR but every runtime call (`pycc_rt_float_pow`, string
/// helpers, etc.) still ran through an unoptimized debug `pycc_rt`. Callers
/// pass `try_build`'s own already-resolved `release` bool through here
/// unchanged, so an optimized build is linked exactly when `--release`
/// (explicit or via a neighboring `pycc.toml` default) is actually in
/// effect.
fn find_pycc_rt_lib_dir(
    target: Option<&str>,
    release: bool,
) -> Result<std::path::PathBuf, String> {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    find_pycc_rt_lib_dir_in(workspace_root, target, release, std::path::Path::exists)
}

/// Rust's `staticlib` output naming is platform-specific: `lib<name>.a` on
/// Unix-like targets, but `<name>.lib` (no `lib` prefix, COFF format) on
/// `-msvc` targets -- verified empirically by cross-building `pycc_rt` for
/// `x86_64-pc-windows-msvc` and inspecting `target/x86_64-pc-windows-msvc/
/// debug/` directly, not assumed from Unix convention. Keyed on the
/// *requested* `target` triple, not the host `#[cfg(windows)]` -- an
/// earlier version used a host-keyed constant, which silently checked for
/// the wrong filename whenever `--target` crossed OS families (e.g.
/// requesting an `-msvc` triple from a non-Windows host, or vice versa),
/// reporting a misleading "no build found" even when the correctly-named
/// file was right there (caught in PR review before merge).
fn pycc_rt_lib_filename(target: Option<&str>) -> &'static str {
    let targets_msvc = match target {
        Some(triple) => triple.contains("windows-msvc"),
        None => cfg!(windows),
    };
    if targets_msvc {
        "pycc_rt.lib"
    } else {
        "libpycc_rt.a"
    }
}

/// Testable core of `find_pycc_rt_lib_dir`: takes the filesystem-existence
/// check as a parameter instead of calling `Path::exists` directly, so
/// tests can simulate "no build found" without mutating this workspace's
/// real, shared `target/` directory (which every other test also depends
/// on -- deleting the real `libpycc_rt.a`, even temporarily, would be
/// flaky under parallel test execution).
///
/// A plain `fn` pointer, not `impl Fn(..)`: every caller here (production
/// and all four tests) passes a non-capturing closure, so a concrete
/// function pointer covers all of them with a single compiled body.
/// Genericity over `impl Fn` would monomorphize a separate copy per
/// closure type -- each one only ever exercising the branches *that
/// caller* takes, which under `cargo llvm-cov` reads as a real gap in
/// coverage even though every branch collectively runs somewhere.
fn find_pycc_rt_lib_dir_in(
    workspace_root: &std::path::Path,
    target: Option<&str>,
    release: bool,
    exists: fn(&std::path::Path) -> bool,
) -> Result<std::path::PathBuf, String> {
    let profile = if release { "release" } else { "debug" };
    let release_flag = if release { " --release" } else { "" };
    let dir = match target {
        Some(triple) => workspace_root.join("target").join(triple).join(profile),
        None => workspace_root.join("target").join(profile),
    };
    if exists(&dir.join(pycc_rt_lib_filename(target))) {
        Ok(dir)
    } else if let Some(triple) = target {
        Err(format!(
            "no pycc_rt build found for target `{triple}` (expected {}). \
             Run `rustup target add {triple}` then `cargo build{release_flag} --target {triple} -p pycc_rt` first.",
            dir.display()
        ))
    } else {
        Err(format!(
            "no pycc_rt build found (expected {}). Run `cargo build{release_flag} -p pycc_rt` first.",
            dir.display()
        ))
    }
}

#[cfg(test)]
mod linker_tests {
    use super::*;

    #[test]
    fn finds_the_native_lib_dir_when_it_exists() {
        let root = std::path::Path::new("/workspace");
        let result = find_pycc_rt_lib_dir_in(root, None, false, |_| true);
        assert_eq!(result, Ok(root.join("target/debug")));
    }

    #[test]
    fn reports_a_clean_error_when_the_native_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err = find_pycc_rt_lib_dir_in(root, None, false, |_| false).unwrap_err();
        assert!(err.contains("cargo build -p pycc_rt"));
        // Must not suggest --release for a debug-profile lookup.
        assert!(!err.contains("--release"));
    }

    #[test]
    fn finds_the_target_specific_lib_dir_when_it_exists() {
        let root = std::path::Path::new("/workspace");
        let result =
            find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), false, |_| true);
        assert_eq!(
            result,
            Ok(root.join("target/x86_64-unknown-linux-gnu/debug"))
        );
    }

    #[test]
    fn reports_a_clean_error_naming_the_target_when_its_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err = find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), false, |_| {
            false
        })
        .unwrap_err();
        assert!(err.contains("x86_64-unknown-linux-gnu"));
        assert!(err.contains("rustup target add"));
    }

    #[test]
    fn finds_the_native_release_lib_dir_when_it_exists() {
        let root = std::path::Path::new("/workspace");
        let result = find_pycc_rt_lib_dir_in(root, None, true, |_| true);
        assert_eq!(result, Ok(root.join("target/release")));
    }

    #[test]
    fn reports_a_clean_error_naming_release_when_the_native_release_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err = find_pycc_rt_lib_dir_in(root, None, true, |_| false).unwrap_err();
        assert!(err.contains("cargo build --release -p pycc_rt"));
        // Not a literal "target/release" substring check: `dir.display()`
        // renders with the platform's native separator, so this would be
        // "target\\release" on Windows -- a real bug caught by the pinned
        // reviewer in this same fix's own review round. `assert_eq!` against
        // a `PathBuf` join (like the sibling `Ok(...)` tests above) compares
        // components instead of literal separator bytes.
        let expected_dir = root.join("target").join("release");
        assert!(err.contains(&expected_dir.display().to_string()));
    }

    #[test]
    fn finds_the_target_specific_release_lib_dir_when_it_exists() {
        let root = std::path::Path::new("/workspace");
        let result =
            find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), true, |_| true);
        assert_eq!(
            result,
            Ok(root.join("target/x86_64-unknown-linux-gnu/release"))
        );
    }

    #[test]
    fn reports_a_clean_error_naming_the_target_and_release_when_its_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err = find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), true, |_| {
            false
        })
        .unwrap_err();
        assert!(err.contains("x86_64-unknown-linux-gnu"));
        assert!(err.contains("rustup target add"));
        assert!(err.contains("cargo build --release --target x86_64-unknown-linux-gnu -p pycc_rt"));
    }

    #[test]
    fn an_msvc_target_uses_the_dot_lib_filename_regardless_of_host() {
        // The regression this guards: pycc_rt_lib_filename used to be a
        // host-#[cfg(windows)] constant, so requesting an -msvc target from
        // this (non-Windows) test runner silently checked for the wrong
        // filename (libpycc_rt.a instead of pycc_rt.lib).
        assert_eq!(
            pycc_rt_lib_filename(Some("x86_64-pc-windows-msvc")),
            "pycc_rt.lib"
        );
    }

    #[test]
    fn a_non_msvc_target_uses_the_lib_prefix_dot_a_filename() {
        assert_eq!(
            pycc_rt_lib_filename(Some("x86_64-unknown-linux-gnu")),
            "libpycc_rt.a"
        );
    }
}

#[cfg(test)]
mod init_tests {
    use super::*;

    #[test]
    fn succeeds_and_scaffolds_a_project_in_a_writable_directory() {
        let dir = std::env::temp_dir().join(format!("pycc_main_init_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(init(Some("initdirect"), &dir), Ok(()));
        assert!(dir.join("pycc.toml").exists());
        assert!(dir.join("src").join("main.py").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reports_the_scaffold_error_when_pycc_toml_already_exists() {
        // #237: an existing `pycc.toml` is the deterministic cross-platform
        // error case for `init`'s error arm now that a nonexistent target
        // directory scaffolds successfully (the inverted write order's
        // `create_dir_all` creates it -- see `project_config.rs`'s
        // `scaffold_creates_a_missing_target_directory`). The refusal
        // message flows through `init`'s io::Error -> String mapping.
        let dir = std::env::temp_dir().join(format!(
            "pycc_main_init_existing_toml_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pycc.toml"), "user content").unwrap();

        let err = init(Some("irrelevant"), &dir).unwrap_err();
        assert!(err.contains("`pycc.toml` already exists"));
        assert_eq!(
            std::fs::read_to_string(dir.join("pycc.toml")).unwrap(),
            "user content"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod release_flag_tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pycc_main_release_flag_{label}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_explicit_release_flag_always_wins() {
        // True regardless of the source path or any neighboring file --
        // an explicit `--release` never needs a `pycc.toml` to exist at all.
        assert!(resolve_release_flag(true, Path::new("/does/not/exist.py")));
    }

    #[test]
    fn a_source_path_with_no_parent_falls_back_to_false() {
        // `Path::new("").parent()` is documented to return `None` on every
        // platform (an empty path has no components at all) -- the one
        // input that reliably exercises this function's `None` branch
        // without relying on any real filesystem root.
        assert!(!resolve_release_flag(false, Path::new("")));
    }

    #[test]
    fn no_neighboring_pycc_toml_falls_back_to_false() {
        let dir = temp_dir("no_toml");
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_neighboring_pycc_toml_falls_back_to_false_instead_of_aborting() {
        // A build must not fail merely because an optional default it
        // doesn't even need happens to be unreadable.
        let dir = temp_dir("malformed_toml");
        std::fs::write(dir.join("pycc.toml"), "this is not [valid toml").unwrap();
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_neighboring_pycc_toml_with_a_non_release_opt_falls_back_to_false() {
        let dir = temp_dir("debug_opt");
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
             [build]\nopt = \"debug\"\n",
        )
        .unwrap();
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_neighboring_pycc_toml_with_no_build_section_falls_back_to_false() {
        let dir = temp_dir("no_build_section");
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n",
        )
        .unwrap();
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_neighboring_pycc_toml_with_a_release_opt_becomes_the_default() {
        let dir = temp_dir("release_opt");
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
             [build]\nopt = \"release\"\n",
        )
        .unwrap();
        let source_path = dir.join("main.py");

        assert!(resolve_release_flag(false, &source_path));

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod try_build_release_isolation_tests {
    use super::*;

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
    /// `try_build`'s own `std::process::id()`-keyed temp object path is
    /// this same test process's id, not an unpredictable child's, so it's
    /// locatable here without reaching into any uncontracted external
    /// path -- and safe from cross-test races since no other test in this
    /// crate writes to that same path.
    #[test]
    fn try_build_ignores_a_neighboring_release_pycc_toml_when_given_release_false() {
        let dir =
            std::env::temp_dir().join(format!("pycc_release_isolation_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("main.py");
        std::fs::write(&src, "def main() -> None:\n    print(42)\n\nmain()\n").unwrap();
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
             [build]\nopt = \"release\"\n",
        )
        .unwrap();
        let out = dir.join("out");

        // Exactly what `run()` does: `release: false` straight through.
        try_build(src.to_str().unwrap(), out.to_str().unwrap(), None, false)
            .expect("try_build should succeed");

        let obj_path = std::env::temp_dir().join(format!("pycc_obj_{}.o", std::process::id()));
        let obj_bytes = std::fs::read(&obj_path).expect("try_build's temp object should exist");

        // Independently compiled reference: the same source's MIR, built
        // through the exact same frontend pipeline try_build itself uses,
        // compiled directly with release=false.
        let typed_hir = resolve_frontend(&src)
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

        std::fs::remove_dir_all(&dir).ok();
    }
}
