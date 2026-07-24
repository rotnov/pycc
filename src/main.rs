mod cli;

use clap::Parser;
use cli::{Cli, Command};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Build { path, out } => match try_build(&path, &out) {
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
/// failure -- `ExitCode` itself isn't comparable, so `run` (which needs to
/// know whether the build step succeeded before executing the result)
/// can't branch on a returned `ExitCode` directly.
fn try_build(path: &str, out: &str) -> Result<(), ExitCode> {
    let source =
        std::fs::read_to_string(path).expect("reading the source file should not fail for this slice");
    let module = pycc_parser::parse(&source).map_err(|diag| {
        eprintln!("error[{}]: {}", diag.code, diag.message);
        ExitCode::from(1)
    })?;
    let hir = pycc_hir::lower(&module);
    pycc_types::check(&hir).expect("v0.1's type checker is a no-op passthrough; it never fails");
    let mir = pycc_mir::build(&hir);

    let obj_path = std::env::temp_dir().join(format!("pycc_obj_{}.o", std::process::id()));
    pycc_codegen::compile_to_object(&mir, &obj_path).map_err(|e| {
        eprintln!("error: codegen failed: {e}");
        ExitCode::from(1)
    })?;

    let rt_lib_dir = find_pycc_rt_lib_dir();
    let status = std::process::Command::new("cc")
        .arg(&obj_path)
        .arg("-L")
        .arg(&rt_lib_dir)
        .arg("-lpycc_rt")
        .arg("-o")
        .arg(out)
        .status()
        .expect("cc should run");
    if status.success() { Ok(()) } else { Err(ExitCode::from(1)) }
}

fn run(path: &str) -> ExitCode {
    let out = std::env::temp_dir().join(format!("pycc_run_{}", std::process::id()));
    if let Err(code) = try_build(path, out.to_str().expect("temp dir path should be valid UTF-8")) {
        return code;
    }
    let status = std::process::Command::new(&out).status().expect("built binary should run");
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

fn find_pycc_rt_lib_dir() -> std::path::PathBuf {
    // v0.1: pycc always runs from within the workspace during development;
    // a packaged pycc distribution embedding pycc_rt's staticlib into the
    // pycc binary itself (so end users don't need this lookup at all) is a
    // v0.2+ packaging concern, not part of this slice.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug")
}
