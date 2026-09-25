//! The `pyprojx check` command.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use pyprojx_core::{Applicability, Diagnostic, Lock, Severity};

use crate::Status;
use crate::output::{self, OutputFormat};

const PYPROJECT: &str = "pyproject.toml";

/// A file to check, with the path to show in output.
struct Target {
    path: PathBuf,
    display: String,
}

/// What `pyprojx check` does besides checking.
pub struct Options {
    /// Apply fixes at least this applicable, and write the files.
    pub fix: Option<Applicability>,
    /// Count fixes at least this applicable as available.
    pub fixable: Applicability,
    /// Show the changes the fixes would make instead of making them.
    pub diff: bool,
    pub format: OutputFormat,
}

/// A checked file.
pub struct Report {
    /// The path shown in output.
    pub display: String,
    /// The file's text before any fixes.
    pub original: String,
    /// The file's text, after any fixes, which diagnostic spans refer to.
    pub text: String,
    pub diagnostics: Vec<Diagnostic>,
    /// How many problems were fixed.
    pub fixed: usize,
}

pub fn run(paths: Vec<PathBuf>, options: &Options) -> Status {
    let targets = if paths.is_empty() {
        match discover_target() {
            Ok(target) => vec![target],
            Err(message) => {
                anstream::eprintln!("error: {message}");
                return Status::Error;
            }
        }
    } else {
        paths.into_iter().map(explicit_target).collect()
    };
    let mut reports = Vec::new();
    let mut failed = false;
    for target in &targets {
        match check(target, options) {
            Ok(report) => reports.push(report),
            Err(message) => {
                anstream::eprintln!("error: {message}");
                failed = true;
            }
        }
    }
    // Nothing was checked, so there is nothing to report.
    if reports.is_empty() {
        return Status::Error;
    }
    if options.diff {
        return show_diff(&reports, failed);
    }
    let output = match options.format {
        OutputFormat::Full => output::full(&reports, options.fixable),
        OutputFormat::Github => output::github(&reports),
        OutputFormat::Json => output::json(&reports),
    };
    let mut stdout = anstream::stdout().lock();
    if let Err(error) = write!(stdout, "{output}") {
        anstream::eprintln!("error: {error}");
        return Status::Error;
    }
    let errors = reports
        .iter()
        .flat_map(|report| &report.diagnostics)
        .any(|diagnostic| diagnostic.severity() == Severity::Error);
    if failed {
        Status::Error
    } else if errors {
        Status::Failure
    } else {
        Status::Success
    }
}

/// Prints the changes fixes would make to `reports`' files, and how many
/// problems they would fix. Fails if there are any, like `ruff check --diff`.
fn show_diff(reports: &[Report], failed: bool) -> Status {
    let mut stdout = anstream::stdout().lock();
    if let Err(error) = write!(stdout, "{}", output::diff(reports)) {
        anstream::eprintln!("error: {error}");
        return Status::Error;
    }
    let fixed: usize = reports.iter().map(|report| report.fixed).sum();
    if fixed == 0 {
        anstream::eprintln!("No fixes available.");
    } else {
        anstream::eprintln!("Would fix {}.", output::plural(fixed, "problem"));
    }
    if failed {
        Status::Error
    } else if fixed > 0 {
        Status::Failure
    } else {
        Status::Success
    }
}

/// Checks a file, first fixing it if asked to, and writing it unless showing
/// the changes instead.
fn check(target: &Target, options: &Options) -> Result<Report, String> {
    let bytes = std::fs::read(&target.path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => format!("`{}` does not exist", target.display),
        _ => format!("failed to read `{}`: {error}", target.display),
    })?;
    let original = String::from_utf8_lossy(&bytes).into_owned();
    let lock = find_lock(target, &original);
    let fix = if options.diff {
        Some(options.fixable)
    } else {
        options.fix
    };
    let (checked, fixed) = match fix {
        Some(applicability) => {
            let fixed = pyprojx_core::fix::fix(bytes, lock.as_ref(), applicability);
            if fixed.fixed > 0 && !options.diff {
                std::fs::write(&target.path, &fixed.text)
                    .map_err(|error| format!("failed to write `{}`: {error}", target.display))?;
            }
            if fixed.skipped > 0 {
                anstream::eprintln!(
                    "warning: skipped {} that would have made `{}` invalid TOML; please report this",
                    output::plural(fixed.skipped, "fix"),
                    target.display
                );
            }
            (fixed.checked, fixed.fixed)
        }
        None => (pyprojx_core::check_with_lock(bytes, lock.as_ref()), 0),
    };
    Ok(Report {
        display: target.display.clone(),
        original,
        text: checked.text,
        diagnostics: checked.diagnostics,
        fixed,
    })
}

/// Uses `path` as given, or the `pyproject.toml` inside it if it is a directory.
fn explicit_target(path: PathBuf) -> Target {
    let path = if path.is_dir() {
        path.join(PYPROJECT)
    } else {
        path
    };
    let display = path.display().to_string();
    Target { path, display }
}

/// Finds the nearest `pyproject.toml` in the current directory or its parents.
fn discover_target() -> Result<Target, String> {
    let cwd = std::env::current_dir()
        .map_err(|error| format!("failed to read the current directory: {error}"))?;
    nearest_pyproject(&cwd).ok_or_else(|| {
        format!("no `{PYPROJECT}` found in the current directory or any parent directory")
    })
}

fn nearest_pyproject(start: &Path) -> Option<Target> {
    start
        .ancestors()
        .enumerate()
        .map(|(depth, dir)| (depth, dir.join(PYPROJECT)))
        .find(|(_, path)| path.is_file())
        .map(|(depth, path)| Target {
            path,
            display: format!("{}{PYPROJECT}", "../".repeat(depth)),
        })
}

/// Finds the lock file for the project, reporting one that cannot be read.
fn find_lock(target: &Target, pyproject: &str) -> Option<Lock> {
    let prefix = target.display.strip_suffix(PYPROJECT).unwrap_or_default();
    match pyprojx_core::lock::find(&target.path, pyproject, prefix)? {
        Ok(lock) => Some(lock),
        Err(error) => {
            anstream::eprintln!(
                "warning: failed to read `{}`, so tool versions are not taken from it: {}",
                error.name,
                error.message
            );
            None
        }
    }
}
