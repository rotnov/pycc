mod cli;

use clap::Parser;
use cli::{Cli, Command};
use std::process::ExitCode;

pub fn execute() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Build { path, out } => match try_build(&path, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => code,
        },
        Command::Run { path, args } => run(&path, &args),
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
fn try_build(path: &str, out: &str) -> Result<(), ExitCode> {
    let workspace = report_workspace_result(create_temp_workspace())?;
    try_build_in(path, std::path::Path::new(out), workspace.path())
}

fn try_build_in(
    path: &str,
    out: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<(), ExitCode> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("error: could not read `{path}`: {e}");
        ExitCode::from(2)
    })?;
    let module = pycc_parser::parse(&source).map_err(|diag| {
        eprintln!("{}", diag.render_human(path, &source));
        ExitCode::from(1)
    })?;
    let hir = pycc_hir::lower(&module).map_err(|diag| {
        eprintln!("{}", diag.render_human(path, &source));
        ExitCode::from(1)
    })?;
    pycc_types::check(&hir).expect("v0.1's type checker is a no-op passthrough; it never fails");
    let mir = pycc_mir::build(&hir);

    let obj_path = workspace.join("program.o");
    emit_and_link(
        &mir,
        &obj_path,
        out,
        pycc_codegen::compile_to_object,
        pycc_rt::write_c_runtime,
    )
}

type CodegenFn = fn(&pycc_mir::MirModule, &std::path::Path) -> Result<(), String>;
type RuntimeWriter = fn(&std::path::Path) -> std::io::Result<()>;

fn emit_and_link(
    mir: &pycc_mir::MirModule,
    obj_path: &std::path::Path,
    out: &std::path::Path,
    codegen: CodegenFn,
    write_runtime: RuntimeWriter,
) -> Result<(), ExitCode> {
    report_codegen_result(codegen(mir, obj_path))?;
    let rt_source_path = obj_path.with_extension("pycc_rt.c");
    report_runtime_result(write_runtime(&rt_source_path))?;
    report_link_result(pycc_codegen::link_object_with_runtime(
        obj_path,
        &rt_source_path,
        out,
    ))
}

fn create_temp_workspace() -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("pycc-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

fn report_workspace_result(
    result: std::io::Result<tempfile::TempDir>,
) -> Result<tempfile::TempDir, ExitCode> {
    match result {
        Ok(workspace) => Ok(workspace),
        Err(error) => {
            eprintln!("error: could not create a private temporary workspace: {error}");
            Err(ExitCode::from(1))
        }
    }
}

fn report_codegen_result(result: Result<(), String>) -> Result<(), ExitCode> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("error: codegen failed: {error}");
            Err(ExitCode::from(1))
        }
    }
}

fn report_runtime_result(result: std::io::Result<()>) -> Result<(), ExitCode> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("error: could not materialize the bundled runtime: {error}");
            Err(ExitCode::from(1))
        }
    }
}

fn report_link_result(result: Result<(), String>) -> Result<(), ExitCode> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("error: linking failed: {error}");
            Err(ExitCode::from(1))
        }
    }
}

fn run(path: &str, args: &[std::ffi::OsString]) -> ExitCode {
    let workspace = match report_workspace_result(create_temp_workspace()) {
        Ok(workspace) => workspace,
        Err(code) => return code,
    };
    let out = run_output_path(workspace.path());
    if let Err(code) = try_build_in(path, &out, workspace.path()) {
        return code;
    }
    let status = program_command(&out, args)
        .status()
        .expect("built binary should run");
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

fn run_output_path(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join(format!("program{}", std::env::consts::EXE_SUFFIX))
}

fn program_command(path: &std::path::Path, args: &[std::ffi::OsString]) -> std::process::Command {
    let mut command = std::process::Command::new(path);
    command.args(args);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_command_preserves_argument_values_and_order() {
        let args = [
            std::ffi::OsString::from("one"),
            std::ffi::OsString::from("olá"),
            std::ffi::OsString::from("--flag"),
        ];
        let command = program_command(std::path::Path::new("program"), &args);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args.iter()
                .map(std::ffi::OsString::as_os_str)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn run_output_stays_inside_the_private_workspace() {
        let workspace = std::path::Path::new("private-workspace");
        let path = run_output_path(workspace);
        assert_eq!(path.parent(), Some(workspace));
        assert_eq!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some(format!("program{}", std::env::consts::EXE_SUFFIX).as_str())
        );
    }

    #[test]
    fn workspace_creation_failures_map_to_compile_error_exit_code() {
        assert_eq!(
            report_workspace_result(Err(std::io::Error::other("temp unavailable"))).err(),
            Some(ExitCode::from(1))
        );
    }

    #[test]
    fn temporary_workspaces_are_unique_and_private() {
        let first = create_temp_workspace().unwrap();
        let second = create_temp_workspace().unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().starts_with(std::env::temp_dir()));
        assert!(second.path().starts_with(std::env::temp_dir()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(first.path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn codegen_failures_map_to_compile_error_exit_code() {
        fn fail_codegen(_mir: &pycc_mir::MirModule, _path: &std::path::Path) -> Result<(), String> {
            Err("bad IR".to_string())
        }

        let mir = pycc_mir::MirModule { items: vec![] };
        assert_eq!(
            emit_and_link(
                &mir,
                std::path::Path::new("unused.o"),
                std::path::Path::new("unused"),
                fail_codegen,
                pycc_rt::write_c_runtime,
            ),
            Err(ExitCode::from(1))
        );
    }

    #[test]
    fn runtime_materialization_failures_map_to_compile_error_exit_code() {
        fn succeed_codegen(
            _mir: &pycc_mir::MirModule,
            _path: &std::path::Path,
        ) -> Result<(), String> {
            Ok(())
        }

        fn fail_runtime(_path: &std::path::Path) -> std::io::Result<()> {
            Err(std::io::Error::other("read-only destination"))
        }

        let mir = pycc_mir::MirModule { items: vec![] };
        assert_eq!(
            emit_and_link(
                &mir,
                std::path::Path::new("unused.o"),
                std::path::Path::new("unused"),
                succeed_codegen,
                fail_runtime,
            ),
            Err(ExitCode::from(1))
        );
    }

    #[test]
    fn emit_and_link_succeeds_with_the_real_codegen_and_runtime() {
        let dir = create_temp_workspace().unwrap();
        let obj_path = dir.path().join("program.o");
        let bin_path = dir
            .path()
            .join(format!("program{}", std::env::consts::EXE_SUFFIX));
        let mir = pycc_mir::MirModule {
            items: vec![pycc_mir::MirItem::TopLevelStmt(
                pycc_mir::MirInstr::CallPrint { arg: 42 },
            )],
        };

        assert_eq!(
            emit_and_link(
                &mir,
                &obj_path,
                &bin_path,
                pycc_codegen::compile_to_object,
                pycc_rt::write_c_runtime,
            ),
            Ok(())
        );
        let output = std::process::Command::new(bin_path).output().unwrap();
        assert_eq!(output.stdout, b"42\n");
    }

    #[test]
    fn emit_and_link_reports_a_linker_failure() {
        let dir = create_temp_workspace().unwrap();
        let obj_path = dir.path().join("program.o");
        let bad_bin_path = dir.path().join("missing").join("program");
        let mir = pycc_mir::MirModule {
            items: vec![pycc_mir::MirItem::TopLevelStmt(
                pycc_mir::MirInstr::CallPrint { arg: 42 },
            )],
        };

        assert_eq!(
            emit_and_link(
                &mir,
                &obj_path,
                &bad_bin_path,
                pycc_codegen::compile_to_object,
                pycc_rt::write_c_runtime,
            ),
            Err(ExitCode::from(1))
        );
    }
}
