mod cli;
mod frontend;
mod modules;
mod project_config;
mod source;

use clap::Parser;
use cli::{Cli, Command, ErrorFormat, OutputFormat};
use frontend::{check_frontend, report_build_failure, report_check_failure, resolve_frontend};
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
            let release = resolve_release_flag(release, &path);
            // Caller-owned scratch (#783): the temp object `try_build` emits
            // lives inside this `ScratchDir`, so every exit from this arm --
            // success and each error path alike -- removes it on drop. The
            // user's `-o` output is untouched (it is the explicitly
            // requested persistent output #779 carves out). Created before
            // any frontend work runs: an unusable system temp directory is
            // an environment failure that fails fast here (exit 2, like
            // `init`'s unreadable cwd), before the source is even read.
            let scratch = match create_scratch("build") {
                Ok(scratch) => scratch,
                Err(code) => return code,
            };
            match try_build(
                &path,
                &out,
                target.as_deref(),
                release,
                &scratch.join("main.o"),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(code) => code,
            }
        }
        Command::Run { path, args } => run(&path, &args),
        Command::Version { verbose } => {
            // `pycc {version}` comes from the manifest (`CARGO_PKG_VERSION`),
            // so it can't silently rot when the crate bumps. `rustc
            // {version}` used to come from `CARGO_PKG_RUST_VERSION`, but
            // that's the manifest's `rust-version` (MSRV) contract, not the
            // compiler that actually produced this binary -- a newer
            // installed rustc than the declared minimum builds cleanly and
            // silently makes the two diverge (#247). `build.rs` captures the
            // real build-time `rustc --version` into
            // `PYCC_BUILD_RUSTC_VERSION` instead. "LLVM 22.1.1" stays a
            // literal deliberately: it states D-015's pinned contract value,
            // and whether the *installed* LLVM actually matches that pin is
            // #75's open scope, not this line's job.
            println!(
                "pycc {} (rustc {}, LLVM 22.1.1)",
                env!("CARGO_PKG_VERSION"),
                env!("PYCC_BUILD_RUSTC_VERSION"),
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
            // `std::env::current_dir()` is fallible: the process's cwd may
            // have been deleted, unmounted, or become otherwise inaccessible
            // after launch (#251). This is an invocation/environment error
            // controlled outside pycc, not an internal invariant, so it is
            // reported as an exit-2 diagnostic (CLI_SPEC.md's exit-code
            // contract) rather than a panic. No scaffold write is attempted
            // and no fallback directory is used.
            match std::env::current_dir() {
                Ok(cwd) => match init(name.as_deref(), &cwd) {
                    Ok(()) => {
                        println!("Created pycc.toml and src/main.py");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error: pycc init failed: {e}");
                        ExitCode::from(2)
                    }
                },
                Err(e) => {
                    eprintln!("error: pycc init failed: cannot read current directory: {e}");
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
/// `check`'s own out-of-band `frontend::FrontendFailure::Input` class ("could not
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
/// internally -- matching `pycc_codegen::artifact_layout`'s
/// `find_pycc_rt_lib_dir_in` dependency-injection split -- so a test can
/// exercise `project_config::scaffold`'s error path (an existing `pycc.toml`,
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
            exit_code = exit_code.max(report_check_failure(failure, error_format));
        }
    }
    ExitCode::from(exit_code)
}

/// Creates the `pycc_scratch::ScratchDir` that holds a command's temporary
/// build artifacts (#783, Part 3 of #779), mapping a creation failure to
/// CLI_SPEC.md's exit-2 invocation/environment class -- an unusable system
/// temp directory is controlled outside pycc, exactly like the unstartable
/// linker driver below and `init`'s unreadable cwd, and is reported as an
/// actionable `error:` diagnostic rather than a panic. The returned handle
/// is caller-owned: `Drop` removes the directory tree (success, error, and
/// panic-unwind paths alike), so the caller must keep it alive until every
/// consumer of the files placed inside it -- the linker reading the temp
/// object, `run`'s child executing from inside it -- has finished.
///
/// Before creating the new root, this opportunistically sweeps
/// provably-stale pycc scratch roots left in the temp directory by dead
/// pycc processes (#784, Part 4 of #779) -- silent, bounded, and
/// best-effort, so the report is deliberately discarded and the sweep can
/// never change this command's output or exit code.
fn create_scratch(category: &str) -> Result<pycc_scratch::ScratchDir, ExitCode> {
    let _ = pycc_scratch::sweep_stale_roots();
    pycc_scratch::ScratchDir::new(category).map_err(|e| {
        eprintln!(
            "error: could not create a scratch directory under the system temp directory: {e}"
        );
        ExitCode::from(2)
    })
}

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
fn try_build(
    path: &Path,
    out: &Path,
    target: Option<&str>,
    release: bool,
    obj_path: &Path,
) -> Result<(), ExitCode> {
    let typed_hir =
        resolve_frontend(path).map_err(|failure| ExitCode::from(report_build_failure(failure)))?;
    let mir = pycc_mir::build(&typed_hir);

    pycc_codegen::compile_to_object(&mir, obj_path, target, release).map_err(|e| {
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
    cmd.arg(obj_path)
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

/// Builds the `std::process::Command` that launches the freshly built
/// `binary`, forwarding `args` unchanged and in order as its process
/// arguments (CLI_SPEC.md's `pycc run [PATH] [-- args]` contract, #23).
/// `args` is `OsString`, not `String` (#824 item 2): a forwarded value can
/// be a non-UTF-8 byte sequence, and `Command::args` accepts `OsStr`
/// directly, so nothing here needs to re-validate or lossily convert it.
/// Factored out of `run` so the forwarding itself -- not just the full
/// build-and-execute pipeline -- has a direct test against a real child
/// process.
fn run_command(binary: &std::path::Path, args: &[std::ffi::OsString]) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command.args(args);
    command
}

/// `pycc run` has no `--release` flag (undocumented in CLI_SPEC.md) and
/// always builds in the debug profile: the hardcoded `false` below reaches
/// `try_build` directly, which -- unlike `main()`'s `Command::Build` arm --
/// never consults a neighboring `pycc.toml`'s `[build] opt = "release"`
/// default, so this stays unconditional regardless of what any nearby
/// `pycc.toml` names.
///
/// Both temporary artifacts -- the codegen object and the linked
/// executable -- live inside one caller-owned `create_scratch` directory
/// (#783). The `ScratchDir`'s own `pid`/`nanos`/`seq` name already carries
/// all the uniqueness the old `pycc_run_{pid}` file name provided, so plain
/// `out`/`main.o` file names inside it are enough. Binding order is
/// load-bearing: `scratch` stays alive across `try_build` (the linker reads
/// the object and writes `out`) *and* across the call to
/// `run_built_binary` below, which itself calls `run_command(..).status()`
/// (the child executes from inside the scratch directory; `status()` waits
/// for it to exit), so `Drop` removes the directory only after the child
/// has terminated -- do not restructure this to return before that wait.
fn run(path: &Path, args: &[std::ffi::OsString]) -> ExitCode {
    let scratch = match create_scratch("run") {
        Ok(scratch) => scratch,
        Err(code) => return code,
    };
    let out = scratch.join("out");
    if let Err(code) = try_build(path, &out, None, false, &scratch.join("main.o")) {
        return code;
    }
    ExitCode::from(run_built_binary(&out, args))
}

/// Spawns the just-linked binary at `out` and waits for it, mapping the
/// result to `pycc run`'s exit code. Factored out of `run` (#249) so the
/// exec-failure path -- `Command::status()` failing to even start the
/// child, e.g. a permissions error or the binary having vanished between
/// link and spawn -- has a direct seam to test: calling this function with
/// a path that cannot be executed reaches the `Err` arm deterministically,
/// without needing `run`'s own scratch-directory and build machinery to
/// manufacture that failure. Returns a plain `u8` rather than `ExitCode`
/// (mirroring `frontend::report_check_failure`/`report_build_failure`) so a
/// test can assert the exact numeric value instead of only "did not
/// panic" -- `ExitCode` itself has no `PartialEq` (see
/// `create_scratch_tests`'s own note on this).
fn run_built_binary(out: &Path, args: &[std::ffi::OsString]) -> u8 {
    let status = match run_command(out, args).status() {
        Ok(status) => status,
        // Failing to *start* the built binary (permission denied, the file
        // vanishing between link and spawn) is an ordinary environment
        // failure, not a pycc invariant -- report it like the linker's own
        // spawn failure in `try_build` above (CLI_SPEC.md's exit-2
        // invocation/environment class) instead of panicking with a raw
        // backtrace.
        Err(e) => {
            eprintln!(
                "error: could not run the built program `{}`: {e}",
                pycc_diag::display_path(&out.to_string_lossy())
            );
            return 2;
        }
    };
    // Generated programs currently have no user-controlled non-zero exit
    // status. Any unsuccessful termination is therefore a runtime panic,
    // trap, or uncaught failure and maps to CLI_SPEC.md's stable 101 on
    // every platform, including Unix signal termination where `code()` is
    // `None` and Windows abort statuses that do not fit in a u8.
    if status.success() { 0 } else { 101 }
}

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
fn find_pycc_rt_lib_dir(target: Option<&str>, release: bool) -> Result<std::path::PathBuf, String> {
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
mod init_tests {
    use super::*;
    use pycc_scratch::ScratchDir;

    #[test]
    fn succeeds_and_scaffolds_a_project_in_a_writable_directory() {
        let dir = ScratchDir::new("main_init_test").expect("failed to create scratch dir");

        assert_eq!(init(Some("initdirect"), &dir), Ok(()));
        assert!(dir.join("pycc.toml").exists());
        assert!(dir.join("src").join("main.py").exists());
    }

    #[test]
    fn reports_the_scaffold_error_when_pycc_toml_already_exists() {
        // #237: an existing `pycc.toml` is the deterministic cross-platform
        // error case for `init`'s error arm now that a nonexistent target
        // directory scaffolds successfully (the inverted write order's
        // `create_dir_all` creates it -- see `project_config.rs`'s
        // `scaffold_creates_a_missing_target_directory`). The refusal
        // message flows through `init`'s io::Error -> String mapping.
        let dir = ScratchDir::new("main_init_existing_toml").expect("failed to create scratch dir");
        std::fs::write(dir.join("pycc.toml"), "user content").unwrap();

        let err = init(Some("irrelevant"), &dir).unwrap_err();
        assert!(err.contains("`pycc.toml` already exists"));
        assert_eq!(
            std::fs::read_to_string(dir.join("pycc.toml")).unwrap(),
            "user content"
        );
    }
}

#[cfg(test)]
mod release_flag_tests {
    use super::*;
    use pycc_scratch::ScratchDir;

    fn scratch_dir(label: &str) -> ScratchDir {
        ScratchDir::new(&format!("main_release_flag_{label}"))
            .expect("failed to create scratch dir")
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
        let dir = scratch_dir("no_toml");
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));
    }

    #[test]
    fn a_malformed_neighboring_pycc_toml_falls_back_to_false_instead_of_aborting() {
        // A build must not fail merely because an optional default it
        // doesn't even need happens to be unreadable.
        let dir = scratch_dir("malformed_toml");
        std::fs::write(dir.join("pycc.toml"), "this is not [valid toml").unwrap();
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));
    }

    #[test]
    fn a_neighboring_pycc_toml_with_a_non_release_opt_falls_back_to_false() {
        let dir = scratch_dir("debug_opt");
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
             [build]\nopt = \"debug\"\n",
        )
        .unwrap();
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));
    }

    #[test]
    fn a_neighboring_pycc_toml_with_no_build_section_falls_back_to_false() {
        let dir = scratch_dir("no_build_section");
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n",
        )
        .unwrap();
        let source_path = dir.join("main.py");

        assert!(!resolve_release_flag(false, &source_path));
    }

    #[test]
    fn a_neighboring_pycc_toml_with_a_release_opt_becomes_the_default() {
        let dir = scratch_dir("release_opt");
        std::fs::write(
            dir.join("pycc.toml"),
            "[project]\nname = \"t\"\nentry = \"main.py\"\npython = \"3.14\"\n\n\
             [build]\nopt = \"release\"\n",
        )
        .unwrap();
        let source_path = dir.join("main.py");

        assert!(resolve_release_flag(false, &source_path));
    }
}

#[cfg(test)]
mod create_scratch_tests {
    use super::*;

    #[test]
    fn returns_a_live_directory_that_drop_removes() {
        let scratch =
            create_scratch("main_create_scratch_ok").expect("create_scratch should succeed");
        let path = scratch.to_path_buf();
        assert!(
            path.is_dir(),
            "create_scratch must hand back an already-created directory"
        );
        drop(scratch);
        assert!(
            !path.exists(),
            "dropping the handle must remove the scratch directory"
        );
    }

    #[test]
    fn maps_a_creation_failure_to_an_error() {
        // A NUL byte is invalid in a path component on every platform this
        // repo targets, so `create_dir` fails portably -- mirroring
        // `pycc_scratch`'s own
        // `a_category_that_produces_an_invalid_path_component_propagates_the_create_dir_error`.
        // This exercises the `map_err` closure (its own coverage region)
        // inside this single `--cfg test` instantiation; `ExitCode` has no
        // `PartialEq`, so the *value* 2 is asserted at the e2e level
        // (tests/slice0.rs's bad-temp-dir tests), not here.
        let result = create_scratch("bad\0category");
        assert!(
            result.is_err(),
            "a NUL byte in the category should make create_scratch fail"
        );
    }
}

#[cfg(all(test, unix))]
mod run_command_tests {
    use super::run_command;
    use std::ffi::OsString;

    /// Proves `run_command` actually forwards its `args` unchanged and in
    /// order to the child process (#23), not merely that `pycc run` parses
    /// them: the Python language surface has no `sys.argv` yet, so a
    /// compiled program can't itself observe its own arguments, and this is
    /// the only way to verify forwarding against a real process rather than
    /// only the CLI parser. `/bin/echo` is POSIX-guaranteed, hence
    /// `#[cfg(unix)]`; the multi-value, dash-prefixed, and Unicode cases are
    /// already covered without a real process at the parser level in
    /// `cli.rs`, so this single case only needs to prove forwarding, not
    /// re-cover every value shape.
    #[test]
    fn forwards_args_unchanged_and_in_order_to_the_child_process() {
        let args: Vec<OsString> = vec!["first".into(), "-x".into(), "héllo".into()];

        let output = run_command(std::path::Path::new("/bin/echo"), &args)
            .output()
            .expect("/bin/echo should run");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"first -x h\xc3\xa9llo\n");
    }

    #[test]
    fn forwards_no_args_when_the_slice_is_empty() {
        let output = run_command(std::path::Path::new("/bin/echo"), &[])
            .output()
            .expect("/bin/echo should run");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"\n");
    }

    /// #824 item 2: a forwarded value that is not valid UTF-8 must reach
    /// the child process as the same opaque bytes, not get lossily
    /// replaced or rejected. `/bin/echo`'s own stdout is a byte stream, so
    /// this proves the forwarding survives all the way through a real
    /// child process, matching `cli.rs`'s parser-level
    /// `run_captures_non_utf8_trailing_args_as_opaque_bytes` proving the
    /// parse side alone.
    #[test]
    fn forwards_non_utf8_args_as_opaque_bytes_to_the_child_process() {
        use std::os::unix::ffi::OsStringExt;

        let args: Vec<OsString> = vec![OsString::from_vec(b"arg_\xff".to_vec())];

        let output = run_command(std::path::Path::new("/bin/echo"), &args)
            .output()
            .expect("/bin/echo should run");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"arg_\xff\n");
    }
}

#[cfg(all(test, unix))]
mod run_built_binary_tests {
    use super::run_built_binary;

    /// #249: `run_built_binary` (factored out of `run`) used to
    /// `.status().expect("built binary should run")`, so a spawn failure --
    /// the built binary missing or unexecutable between link and spawn --
    /// panicked with a raw backtrace instead of reporting CLI_SPEC.md's
    /// exit-2 invocation/environment class, the same class
    /// `try_build`'s own linker-spawn-failure arm already uses. A
    /// nonexistent path makes `Command::status()` fail with `ENOENT`
    /// portably, reaching the `Err` arm directly -- no scratch directory or
    /// real build needed to manufacture the failure. `run_built_binary`
    /// returns a plain `u8` (not `ExitCode`, which has no `PartialEq` --
    /// see `create_scratch_tests`'s own note on this) specifically so this
    /// exact exit-2 value can be asserted directly.
    #[test]
    fn a_binary_that_cannot_be_spawned_reports_exit_code_2() {
        let out = std::path::Path::new("/nonexistent/pycc_249_test_binary");
        assert_eq!(run_built_binary(out, &[]), 2);
    }

    /// A binary that spawns and exits `0` reports exit code `0`. Exercises
    /// the `Ok` arm's success branch, which the spawn-failure test above
    /// never reaches.
    #[test]
    fn a_successfully_run_binary_reports_exit_code_0() {
        let out = std::path::Path::new("/usr/bin/true");
        assert_eq!(run_built_binary(out, &[]), 0);
    }

    /// A binary that spawns but exits non-zero reports the stable `101`
    /// (CLI_SPEC.md's "compiled program panicked/uncaught exception" exit
    /// code, reused here for any unsuccessful child termination). Exercises
    /// the `Ok` arm's failure branch.
    #[test]
    fn a_binary_that_exits_non_zero_reports_exit_code_101() {
        let out = std::path::Path::new("/usr/bin/false");
        assert_eq!(run_built_binary(out, &[]), 101);
    }
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
        try_build(&src, &out, None, false, &obj_path).expect("try_build should succeed");

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
