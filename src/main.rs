mod cli;
mod source;

use clap::Parser;
use cli::{Cli, Command};
use pycc_diag::{Diagnostic, Span};
use std::path::Path;
use std::process::ExitCode;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
        Command::Check { paths } => check_paths(&paths),
        Command::Test | Command::Explain { .. } | Command::Init { .. } | Command::Clean => {
            eprintln!("pycc: this subcommand is not yet implemented");
            ExitCode::from(2)
        }
    }
}

/// `Ok(())` on success, `Err(code)` carrying the exit code to use on
/// failure. `?` inside this function needs a `Result`, not an `ExitCode`
/// directly -- `run` then just propagates whatever `Err` it gets.
fn try_build(path: &str, out: &str) -> Result<(), ExitCode> {
    let path = Path::new(path);
    let hir = check_frontend(path)
        .map_err(|failure| ExitCode::from(report_frontend_failure(path, failure)))?;
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
    if status.success() {
        Ok(())
    } else {
        Err(ExitCode::from(1))
    }
}

enum FrontendFailure {
    Input(String),
    Compile {
        diagnostic: Diagnostic,
        source: String,
    },
}

fn check_frontend(path: &Path) -> Result<pycc_hir::HirModule, FrontendFailure> {
    let bytes = std::fs::read(path).map_err(|error| FrontendFailure::Input(error.to_string()))?;
    let source = source::decode_python_source(&bytes).map_err(FrontendFailure::Input)?;
    let module = match pycc_parser::parse(&source) {
        Ok(module) => module,
        Err(diagnostic) => return Err(FrontendFailure::Compile { diagnostic, source }),
    };
    let hir = match pycc_hir::lower(&module) {
        Ok(hir) => hir,
        Err(diagnostic) => return Err(FrontendFailure::Compile { diagnostic, source }),
    };
    pycc_types::check(&hir).expect("v0.1's type checker is a no-op passthrough; it never fails");
    Ok(hir)
}

fn check_paths(paths: &[std::path::PathBuf]) -> ExitCode {
    if paths.is_empty() {
        eprintln!("error: `pycc check` requires at least one Python file in v0.1");
        return ExitCode::from(2);
    }

    let mut exit_code = 0;
    for path in paths {
        if let Err(failure) = check_frontend(path) {
            exit_code = exit_code.max(report_frontend_failure(path, failure));
        }
    }
    ExitCode::from(exit_code)
}

fn report_frontend_failure(path: &Path, failure: FrontendFailure) -> u8 {
    let path = normalize_diagnostic_path(&path.to_string_lossy());
    match failure {
        FrontendFailure::Input(message) => {
            eprintln!("error: could not read `{path}`: {message}");
            2
        }
        FrontendFailure::Compile { diagnostic, source } => {
            let span = diagnostic
                .span
                .expect("compile errors must carry a primary source span");
            let label = diagnostic
                .label
                .as_deref()
                .expect("compile errors must carry a primary source label");
            eprintln!(
                "error[{}]: {}\n{}",
                diagnostic.code,
                diagnostic.message,
                render_source_span(&path, &source, span, label),
            );
            1
        }
    }
}

fn normalize_diagnostic_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let root = if path.starts_with("//") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let joined = path
        .trim_start_matches('/')
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>()
        .join("/");

    if joined.is_empty() {
        if root.is_empty() {
            ".".to_string()
        } else {
            root.to_string()
        }
    } else {
        format!("{root}{joined}")
    }
}

fn render_source_span(path: &str, source: &str, span: Span, label: &str) -> String {
    let start = usize::try_from(span.start).expect("source offsets must fit usize");
    let end = usize::try_from(span.end).expect("source offsets must fit usize");
    let prefix = &source[..start];
    let line_number = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = match prefix.rfind('\n') {
        Some(newline) => newline + 1,
        None => 0,
    };
    let line_end = match source[start..].find('\n') {
        Some(relative_newline) => start + relative_newline,
        None => source.len(),
    };
    let source_line = &source[line_start..line_end];
    let source_prefix = &source[line_start..start];
    let column = source_prefix.chars().count() + 1;
    let highlight_end = end.max(start).min(line_end);
    let highlight_width = UnicodeWidthStr::width(&source[start..highlight_end]).max(1);
    let gutter_width = line_number.to_string().len();
    let empty_gutter = " ".repeat(gutter_width);
    let mut caret_padding = String::new();
    for character in source_prefix.chars() {
        if character == '\t' {
            caret_padding.push('\t');
        } else {
            caret_padding.extend(std::iter::repeat_n(
                ' ',
                UnicodeWidthChar::width(character).unwrap_or(0),
            ));
        }
    }

    format!(
        " --> {path}:{line_number}:{column}\n{empty_gutter} |\n\
         {line_number:>gutter_width$} | {source_line}\n\
         {empty_gutter} | {caret_padding}{} {label}",
        "^".repeat(highlight_width),
    )
}

fn run(path: &str) -> ExitCode {
    let out = std::env::temp_dir().join(format!("pycc_run_{}", std::process::id()));
    if let Err(code) = try_build(
        path,
        out.to_str().expect("temp dir path should be valid UTF-8"),
    ) {
        return code;
    }
    let status = std::process::Command::new(&out)
        .status()
        .expect("built binary should run");
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

fn find_pycc_rt_lib_dir() -> std::path::PathBuf {
    // v0.1: pycc always runs from within the workspace during development;
    // a packaged pycc distribution embedding pycc_rt's staticlib into the
    // pycc binary itself (so end users don't need this lookup at all) is a
    // v0.2+ packaging concern, not part of this slice.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug")
}

#[cfg(test)]
mod tests {
    use super::normalize_diagnostic_path;

    #[test]
    fn diagnostic_paths_are_lexically_normalized() {
        assert_eq!(
            normalize_diagnostic_path(r".\src\.\package\\module.py"),
            "src/package/module.py"
        );
        assert_eq!(
            normalize_diagnostic_path("/tmp//./package/module.py"),
            "/tmp/package/module.py"
        );
        assert_eq!(
            normalize_diagnostic_path(r"\\server\share\.\module.py"),
            "//server/share/module.py"
        );
        assert_eq!(normalize_diagnostic_path("."), ".");
        assert_eq!(normalize_diagnostic_path("/"), "/");
    }
}
