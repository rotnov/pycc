mod cli;

use clap::Parser;
use cli::{Cli, Command, ErrorFormat};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Build { path, out, target } => match try_build(&path, &out, target.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => code,
        },
        Command::Run { path } => run(&path),
        Command::Version { .. } => {
            println!("pycc 0.1.0 (rustc 1.97.1, LLVM 22.1.1)");
            ExitCode::SUCCESS
        }
        Command::Check { path, error_format } => {
            match try_check(path.as_deref().unwrap_or("."), error_format) {
                Ok(()) => ExitCode::SUCCESS,
                Err(code) => code,
            }
        }
        Command::Test | Command::Explain { .. } | Command::Init { .. } | Command::Clean => {
            eprintln!("pycc: this subcommand is not yet implemented");
            ExitCode::from(2)
        }
    }
}

/// `pycc check`: parse + HIR-lowering + type-checking only, no codegen --
/// CLI_SPEC.md's contract for this subcommand (ruff-fast, no codegen).
/// `error_format`: "human" (default) or "json", matching CLI_SPEC.md's
/// `--error-format` flag. Any diagnostic is printed to stdout and reported
/// as exit code 1; a missing/unreadable file is a clean exit-2 error (same
/// convention as `try_build`).
fn try_check(path: &str, error_format: ErrorFormat) -> Result<(), ExitCode> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("error: could not read `{path}`: {e}");
        ExitCode::from(2)
    })?;
    let report = |diag: pycc_diag::Diagnostic| -> ExitCode {
        match error_format {
            ErrorFormat::Human => print!("{}", pycc_diag::render_human(&diag, path, &source)),
            ErrorFormat::Json => println!("{}", pycc_diag::render_json(&diag, path, &source)),
        }
        ExitCode::from(1)
    };
    let module = match pycc_parser::parse(&source) {
        Ok(m) => m,
        Err(diag) => return Err(report(diag)),
    };
    let hir = match pycc_hir::lower_checked(&module) {
        Ok(h) => h,
        Err(diag) => return Err(report(diag)),
    };
    match pycc_types::check(&hir) {
        Ok(()) => Ok(()),
        Err(diag) => Err(report(diag)),
    }
}

/// `Ok(())` on success, `Err(code)` carrying the exit code to use on
/// failure. `?` inside this function needs a `Result`, not an `ExitCode`
/// directly -- `run` then just propagates whatever `Err` it gets.
///
/// `target`: `None` builds for the host's own default target (`run` always
/// passes this, since running a cross-compiled binary on this host makes
/// no sense). `Some(triple)` cross-compiles -- see `find_pycc_rt_lib_dir`
/// for what that requires to actually be available.
fn try_build(path: &str, out: &str, target: Option<&str>) -> Result<(), ExitCode> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("error: could not read `{path}`: {e}");
        ExitCode::from(2)
    })?;
    let module = pycc_parser::parse(&source).map_err(|diag| {
        eprintln!("error[{}]: {}", diag.code, diag.message);
        ExitCode::from(1)
    })?;
    let hir = pycc_hir::lower_checked(&module).map_err(|diag| {
        eprintln!("error[{}]: {}", diag.code, diag.message);
        ExitCode::from(1)
    })?;
    let typed_hir = pycc_types::check_and_resolve(&hir).map_err(|diag| {
        eprintln!("error[{}]: {}", diag.code, diag.message);
        ExitCode::from(1)
    })?;
    let mir = pycc_mir::build(&typed_hir);

    let obj_path = std::env::temp_dir().join(format!("pycc_obj_{}.o", std::process::id()));
    pycc_codegen::compile_to_object(&mir, &obj_path, target).map_err(|e| {
        eprintln!("error: codegen failed: {e}");
        ExitCode::from(1)
    })?;

    let rt_lib_dir = find_pycc_rt_lib_dir(target).map_err(|e| {
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
    let status = cmd.status().expect("the linker driver should run");
    if status.success() {
        Ok(())
    } else {
        Err(ExitCode::from(1))
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

fn run(path: &str) -> ExitCode {
    let out = std::env::temp_dir().join(format!("pycc_run_{}", std::process::id()));
    if let Err(code) = try_build(
        path,
        out.to_str().expect("temp dir path should be valid UTF-8"),
        None,
    ) {
        return code;
    }
    let status = std::process::Command::new(&out)
        .status()
        .expect("built binary should run");
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

/// `target: None` (the common case) returns this workspace's ordinary
/// `target/debug/` -- always populated once `pycc_rt` has been built
/// (see that crate's own doc comment on the build-order requirement).
///
/// `target: Some(triple)` looks in `target/<triple>/debug/` -- where
/// `cargo build --target <triple> -p pycc_rt` (after `rustup target add
/// <triple>` if needed) puts it. Cross-compiling `pycc_rt` itself for the
/// requested target is not done automatically here: unlike the ordinary
/// case, there's no single always-correct way to trigger it that doesn't
/// either require duplicating `rustup`/`cargo`'s own target-management
/// logic or risk the same build-lock deadlock documented on `pycc_rt`
/// (see that crate's own doc comment). Failing with a clear, actionable
/// message is better than a confusing linker error about a missing
/// `-lpycc_rt`.
fn find_pycc_rt_lib_dir(target: Option<&str>) -> Result<std::path::PathBuf, String> {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    find_pycc_rt_lib_dir_in(workspace_root, target, std::path::Path::exists)
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
    exists: fn(&std::path::Path) -> bool,
) -> Result<std::path::PathBuf, String> {
    let dir = match target {
        Some(triple) => workspace_root.join("target").join(triple).join("debug"),
        None => workspace_root.join("target/debug"),
    };
    if exists(&dir.join(pycc_rt_lib_filename(target))) {
        Ok(dir)
    } else if let Some(triple) = target {
        Err(format!(
            "no pycc_rt build found for target `{triple}` (expected {}). \
             Run `rustup target add {triple}` then `cargo build --target {triple} -p pycc_rt` first.",
            dir.display()
        ))
    } else {
        Err(format!(
            "no pycc_rt build found (expected {}). Run `cargo build -p pycc_rt` first.",
            dir.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_native_lib_dir_when_it_exists() {
        let root = std::path::Path::new("/workspace");
        let result = find_pycc_rt_lib_dir_in(root, None, |_| true);
        assert_eq!(result, Ok(root.join("target/debug")));
    }

    #[test]
    fn reports_a_clean_error_when_the_native_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err = find_pycc_rt_lib_dir_in(root, None, |_| false).unwrap_err();
        assert!(err.contains("cargo build -p pycc_rt"));
    }

    #[test]
    fn finds_the_target_specific_lib_dir_when_it_exists() {
        let root = std::path::Path::new("/workspace");
        let result = find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), |_| true);
        assert_eq!(
            result,
            Ok(root.join("target/x86_64-unknown-linux-gnu/debug"))
        );
    }

    #[test]
    fn reports_a_clean_error_naming_the_target_when_its_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err =
            find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), |_| false).unwrap_err();
        assert!(err.contains("x86_64-unknown-linux-gnu"));
        assert!(err.contains("rustup target add"));
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
