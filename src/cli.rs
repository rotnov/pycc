use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ErrorFormat {
    Human,
    Json,
}

/// `pycc explain <code>`'s own output-format flag. Deliberately not
/// `ErrorFormat`/`--error-format`: `explain`'s output documents a code, it
/// never reports an occurred error, so reusing `check`'s error-flavored
/// name and type would be actively misleading about what the command does
/// (see D-151).
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
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
        /// Enable LLVM optimization (O3-equivalent whole-module pipeline).
        /// True cross-file LTO has no effect yet -- pycc compiles exactly
        /// one module per invocation until v0.4's multi-file support lands
        /// (D-094). Omit to use `pycc.toml`'s neighboring `[build] opt =
        /// "release"` default when one is present, or plain debug output
        /// otherwise.
        #[arg(long)]
        release: bool,
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
        /// `pycc explain`'s own output-format flag (see `OutputFormat`'s
        /// doc comment for why this isn't `--error-format`).
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,
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
    use super::{Cli, Command, ErrorFormat, OutputFormat};
    use clap::Parser;

    #[cfg(unix)]
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

    #[test]
    fn explain_accepts_json_format() {
        assert!(matches!(
            Cli::try_parse_from(["pycc", "explain", "T0001", "--format", "json"])
                .unwrap()
                .command,
            Command::Explain {
                code,
                format: OutputFormat::Json,
            } if code == "T0001"
        ));
    }

    #[test]
    fn explain_defaults_to_human_format() {
        assert!(matches!(
            Cli::try_parse_from(["pycc", "explain", "T0001"])
                .unwrap()
                .command,
            Command::Explain {
                format: OutputFormat::Human,
                ..
            }
        ));
    }

    fn parsed_build_release(command: Command) -> Option<bool> {
        match command {
            Command::Build { release, .. } => Some(release),
            _ => None,
        }
    }

    #[test]
    fn build_defaults_to_release_false_when_the_flag_is_omitted() {
        let cli = Cli::try_parse_from(["pycc", "build", "in.py", "-o", "out"]).unwrap();
        assert_eq!(parsed_build_release(cli.command), Some(false));
        assert!(parsed_build_release(Command::Clean).is_none());
    }

    #[test]
    fn build_accepts_an_explicit_release_flag() {
        let cli =
            Cli::try_parse_from(["pycc", "build", "in.py", "-o", "out", "--release"]).unwrap();
        assert_eq!(parsed_build_release(cli.command), Some(true));
    }
}
