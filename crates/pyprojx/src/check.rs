//! The `pyprojx check` command.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use pyprojx_core::lock::project_name;
use pyprojx_core::{Applicability, Diagnostic, Lock, LockKind, Severity};

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
    pub format: OutputFormat,
}

/// A checked file.
pub struct Report {
    /// The path shown in output.
    pub display: String,
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
        match check(target, options.fix) {
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

/// Checks a file, first fixing it with fixes at least as applicable as `fix`,
/// if given, and writing it.
fn check(target: &Target, fix: Option<Applicability>) -> Result<Report, String> {
    let bytes = std::fs::read(&target.path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => format!("`{}` does not exist", target.display),
        _ => format!("failed to read `{}`: {error}", target.display),
    })?;
    let lock = find_lock(target, &String::from_utf8_lossy(&bytes));
    let (checked, fixed) = match fix {
        Some(applicability) => {
            let fixed = pyprojx_core::fix::fix(bytes, lock.as_ref(), applicability);
            if fixed.fixed > 0 {
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

/// Finds the lock file for the project: a `uv.lock` or `pylock.toml` next to
/// its `pyproject.toml`, or the `uv.lock` of a workspace in a parent directory
/// that includes the project. A lock that cannot be read is skipped with a
/// warning.
fn find_lock(target: &Target, pyproject: &str) -> Option<Lock> {
    let dir = std::path::absolute(&target.path)
        .ok()?
        .parent()?
        .to_path_buf();
    let prefix = target
        .display
        .strip_suffix(PYPROJECT)
        .unwrap_or_default()
        .to_owned();
    let name = project_name(pyproject);
    for (depth, dir) in dir.ancestors().enumerate() {
        let candidates: &[(&str, LockKind)] = if depth == 0 {
            &[("uv.lock", LockKind::Uv), ("pylock.toml", LockKind::Pylock)]
        } else {
            &[("uv.lock", LockKind::Uv)]
        };
        for &(file, kind) in candidates {
            let path = dir.join(file);
            if !path.is_file() {
                continue;
            }
            let display = format!("{prefix}{}{file}", "../".repeat(depth));
            let lock = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| Lock::parse(kind, display.clone(), &text));
            match lock {
                // A parent's lock is for another project unless it includes this one.
                Ok(lock)
                    if depth == 0 || name.as_deref().is_some_and(|n| lock.has_local_package(n)) =>
                {
                    return Some(lock);
                }
                Ok(_) => return None,
                Err(error) => {
                    anstream::eprintln!(
                        "warning: failed to read `{display}`, so tool versions are not taken from it: {error}"
                    );
                    return None;
                }
            }
        }
    }
    None
}
