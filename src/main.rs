mod cli;

use clap::Parser;
use cli::{Cli, Command};
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
        Command::Check { .. }
        | Command::Test
        | Command::Explain { .. }
        | Command::Init { .. }
        | Command::Clean => {
            eprintln!("pycc: this subcommand is not yet implemented");
            ExitCode::from(2)
        }
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
    let hir = pycc_hir::lower(&module);
    pycc_types::check(&hir).expect("v0.1's type checker is a no-op passthrough; it never fails");
    let mir = pycc_mir::build(&hir);

    let obj_path = std::env::temp_dir().join(format!("pycc_obj_{}.o", std::process::id()));
    pycc_codegen::compile_to_object(&mir, &obj_path, target).map_err(|e| {
        eprintln!("error: codegen failed: {e}");
        ExitCode::from(1)
    })?;

    let rt_lib_dir = find_pycc_rt_lib_dir(target).map_err(|e| {
        eprintln!("error: {e}");
        ExitCode::from(2)
    })?;
    let mut cmd = std::process::Command::new("cc");
    if let Some(triple) = target {
        cmd.arg("-target").arg(triple);
    }
    cmd.arg(&obj_path).arg("-L").arg(&rt_lib_dir).arg("-lpycc_rt").arg("-o").arg(out);
    let status = cmd.status().expect("cc should run");
    if status.success() { Ok(()) } else { Err(ExitCode::from(1)) }
}

fn run(path: &str) -> ExitCode {
    let out = std::env::temp_dir().join(format!("pycc_run_{}", std::process::id()));
    if let Err(code) = try_build(path, out.to_str().expect("temp dir path should be valid UTF-8"), None) {
        return code;
    }
    let status = std::process::Command::new(&out).status().expect("built binary should run");
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
    if exists(&dir.join("libpycc_rt.a")) {
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
        assert_eq!(result, Ok(root.join("target/x86_64-unknown-linux-gnu/debug")));
    }

    #[test]
    fn reports_a_clean_error_naming_the_target_when_its_build_is_missing() {
        let root = std::path::Path::new("/workspace");
        let err = find_pycc_rt_lib_dir_in(root, Some("x86_64-unknown-linux-gnu"), |_| false).unwrap_err();
        assert!(err.contains("x86_64-unknown-linux-gnu"));
        assert!(err.contains("rustup target add"));
    }
}
