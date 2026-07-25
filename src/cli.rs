use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ErrorFormat {
    Human,
    Json,
}

#[derive(Parser)]
#[command(name = "pycc")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Build {
        path: String,
        #[arg(short = 'o')]
        out: String,
        /// Cross-compile for a different Tier-1 target triple (e.g.
        /// x86_64-apple-darwin). Omit to build for the host's own default
        /// target -- the common case.
        #[arg(long)]
        target: Option<String>,
    },
    Run {
        path: String,
    },
    Check {
        paths: Vec<PathBuf>,
        /// CLI_SPEC.md's diagnostic-output contract: "human" (default) or "json".
        #[arg(long, value_enum, default_value = "human")]
        error_format: ErrorFormat,
    },
    Test,
    Explain {
        code: String,
    },
    Init {
        name: Option<String>,
    },
    Clean,
    Version {
        #[arg(long)]
        verbose: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, ErrorFormat};
    use clap::Parser;

    fn parsed_check_paths(command: Command) -> Option<Vec<std::path::PathBuf>> {
        match command {
            Command::Check { paths, .. } => Some(paths),
            _ => None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn check_paths_preserve_non_utf8_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let path = std::ffi::OsString::from_vec(b"staged_\xff.py".to_vec());
        let cli = Cli::try_parse_from([
            std::ffi::OsString::from("pycc"),
            std::ffi::OsString::from("check"),
            std::ffi::OsString::from("--"),
            path,
        ])
        .unwrap();
        let paths = parsed_check_paths(cli.command).unwrap();

        assert_eq!(paths[0].as_os_str().as_bytes(), b"staged_\xff.py");
        assert!(parsed_check_paths(Command::Clean).is_none());
    }

    #[test]
    fn check_accepts_json_error_format_with_multiple_paths() {
        assert!(matches!(
            Cli::try_parse_from([
                "pycc",
                "check",
                "--error-format",
                "json",
                "first.py",
                "second.py",
            ])
            .unwrap()
            .command,
            Command::Check {
                error_format: ErrorFormat::Json,
                paths,
            } if paths.len() == 2
        ));
    }
}
