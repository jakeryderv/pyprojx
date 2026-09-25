use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use pyprojx_core::Applicability;

mod check;
mod output;
mod render;

use output::OutputFormat;

/// Version-aware intelligence for pyproject.toml.
#[derive(Debug, Parser)]
#[command(name = "pyprojx", version, about, long_about = None, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check pyproject.toml files for problems.
    Check(CheckArgs),
}

#[derive(Debug, Args)]
struct CheckArgs {
    /// The files to check, or directories containing a pyproject.toml. Defaults
    /// to the nearest pyproject.toml in the current directory or its parents.
    paths: Vec<PathBuf>,
    /// Apply fixes that keep the configuration's meaning, and write the file.
    #[arg(long)]
    fix: bool,
    /// Include fixes that may change the configuration's meaning, such as
    /// renaming a misspelled key to a guess. Review them before committing.
    #[arg(long)]
    unsafe_fixes: bool,
    /// Show the changes the fixes would make, without making them. Exits with 1
    /// if there are any.
    #[arg(long)]
    diff: bool,
    /// How to write diagnostics.
    #[arg(long, value_enum, default_value_t)]
    output_format: OutputFormat,
}

/// Process exit statuses, following Ruff's conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// No errors were found.
    Success,
    /// Errors were found.
    Failure,
    /// The command could not run, for example because the file is missing.
    Error,
}

impl From<Status> for ExitCode {
    fn from(status: Status) -> Self {
        match status {
            Status::Success => Self::SUCCESS,
            Status::Failure => Self::from(1),
            Status::Error => Self::from(2),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let status = match cli.command {
        Command::Check(args) => {
            let fixable = if args.unsafe_fixes {
                Applicability::Unsafe
            } else {
                Applicability::Safe
            };
            let options = check::Options {
                fix: args.fix.then_some(fixable),
                fixable,
                diff: args.diff,
                format: args.output_format,
            };
            check::run(args.paths, &options)
        }
    };
    status.into()
}
