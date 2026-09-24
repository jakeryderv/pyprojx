//! The `pyprojx check` command.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use pyprojx_core::lock::project_name;
use pyprojx_core::{Lock, LockKind, Severity};

use crate::Status;
use crate::render;

const PYPROJECT: &str = "pyproject.toml";

/// A file to check, with the path to show in output.
struct Target {
    path: PathBuf,
    display: String,
}

pub fn run(path: Option<PathBuf>) -> Status {
    match check(path) {
        Ok(status) => status,
        Err(message) => {
            anstream::eprintln!("error: {message}");
            Status::Error
        }
    }
}

fn check(path: Option<PathBuf>) -> Result<Status, String> {
    let target = match path {
        Some(path) => explicit_target(path),
        None => discover_target()?,
    };
    let bytes = std::fs::read(&target.path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => format!("`{}` does not exist", target.display),
        _ => format!("failed to read `{}`: {error}", target.display),
    })?;
    let lock = find_lock(&target, &String::from_utf8_lossy(&bytes));
    let checked = pyprojx_core::check_with_lock(bytes, lock.as_ref());

    let mut stdout = anstream::stdout().lock();
    for diagnostic in &checked.diagnostics {
        let report = render::render(diagnostic, &checked.text, &target.display);
        writeln!(stdout, "{report}\n").map_err(|error| error.to_string())?;
    }
    let errors = count(&checked.diagnostics, Severity::Error);
    let warnings = count(&checked.diagnostics, Severity::Warning);
    writeln!(stdout, "{}", summary(errors, warnings)).map_err(|error| error.to_string())?;

    Ok(if errors > 0 {
        Status::Failure
    } else {
        Status::Success
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

fn count(diagnostics: &[pyprojx_core::Diagnostic], severity: Severity) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity() == severity)
        .count()
}

fn summary(errors: usize, warnings: usize) -> String {
    let plural =
        |count: usize, noun: &str| format!("{count} {noun}{}", if count == 1 { "" } else { "s" });
    match (errors, warnings) {
        (0, 0) => "All checks passed!".to_owned(),
        (errors, 0) => format!("Found {}.", plural(errors, "error")),
        (0, warnings) => format!("Found {}.", plural(warnings, "warning")),
        (errors, warnings) => format!(
            "Found {} and {}.",
            plural(errors, "error"),
            plural(warnings, "warning")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_counts() {
        assert_eq!(summary(0, 0), "All checks passed!");
        assert_eq!(summary(1, 0), "Found 1 error.");
        assert_eq!(summary(0, 2), "Found 2 warnings.");
        assert_eq!(summary(2, 1), "Found 2 errors and 1 warning.");
    }
}
