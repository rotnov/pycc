use clap::{Parser, Subcommand, ValueEnum};

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
        path: Option<String>,
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
