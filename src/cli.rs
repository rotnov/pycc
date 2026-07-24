use clap::{Parser, Subcommand};
use std::ffi::OsString;

#[derive(Parser)]
#[command(name = "pycc")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Subcommand)]
pub enum Command {
    Build {
        path: String,
        #[arg(short = 'o')]
        out: String,
    },
    Run {
        path: String,
        #[arg(last = true)]
        args: Vec<OsString>,
    },
    Check {
        path: Option<String>,
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
    use super::*;

    #[test]
    fn run_accepts_no_program_arguments() {
        let cli = Cli::try_parse_from(["pycc", "run", "app.py"]).expect("CLI should parse");
        assert_eq!(
            cli.command,
            Command::Run {
                path: "app.py".to_string(),
                args: vec![],
            }
        );
    }

    #[test]
    fn run_captures_every_value_after_double_dash_verbatim() {
        let cli = Cli::try_parse_from(["pycc", "run", "app.py", "--", "one", "olá", "--flag"])
            .expect("CLI should parse trailing program arguments");
        assert_eq!(
            cli.command,
            Command::Run {
                path: "app.py".to_string(),
                args: vec![
                    OsString::from("one"),
                    OsString::from("olá"),
                    OsString::from("--flag"),
                ],
            }
        );
    }
}
