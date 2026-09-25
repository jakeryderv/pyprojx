//! Package versions from a lock file: `uv.lock`, or `pylock.toml` (PEP 751).
//!
//! A lock records the versions a project installs, which are more precise than
//! the ranges its requirements allow. Only names and versions are read.
//!
//! Checking takes a parsed lock, so that it needs no file access; [`find`]
//! finds a project's lock file on disk for front ends such as the CLI and the
//! language server.

use std::collections::BTreeMap;
use std::path::Path;

use toml::de::DeTable;

use crate::document::get;
use crate::standards::normalize_name;

/// The kind of lock file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    /// uv's `uv.lock`, whose packages are `[[package]]` tables.
    Uv,
    /// PEP 751's `pylock.toml`, whose packages are `[[packages]]` tables.
    Pylock,
}

/// The versions of each package in a lock file.
#[derive(Clone, Debug)]
pub struct Lock {
    /// The file in messages, such as `uv.lock` or `../uv.lock`.
    pub name: String,
    /// Versions by normalized package name. A lock can hold several versions
    /// of a package, for different platforms or Python versions.
    packages: BTreeMap<String, Vec<String>>,
    /// Normalized names of the packages the lock builds from local sources,
    /// such as the project itself and other workspace members.
    local: Vec<String>,
}

impl Lock {
    /// Parses a lock file, shown as `name` in messages.
    pub fn parse(kind: LockKind, name: impl Into<String>, text: &str) -> Result<Self, String> {
        let root = DeTable::parse(text).map_err(|error| error.to_string())?;
        let root = root.get_ref();
        let key = match kind {
            LockKind::Uv => "package",
            LockKind::Pylock => "packages",
        };
        let mut packages: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut local = Vec::new();
        let entries = get(root, key)
            .and_then(|(_, entries)| entries.get_ref().as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get_ref().as_table());
        for package in entries {
            let text =
                |key: &str| get(package, key).and_then(|(_, value)| value.get_ref().as_str());
            let Some(name) = text("name").and_then(|name| normalize_name(name).ok()) else {
                continue;
            };
            if kind == LockKind::Uv && is_local(package) {
                local.push(name.clone());
            }
            if let Some(version) = text("version") {
                let versions = packages.entry(name).or_default();
                if !versions.iter().any(|known| known == version) {
                    versions.push(version.to_owned());
                }
            }
        }
        Ok(Self {
            name: name.into(),
            packages,
            local,
        })
    }

    /// The locked versions of the package `name`, if any.
    pub fn versions(&self, name: &str) -> &[String] {
        normalize_name(name)
            .ok()
            .and_then(|name| self.packages.get(&name))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether the lock builds the package `name` from a local source, as
    /// `uv.lock` does for each member of its workspace.
    pub fn has_local_package(&self, name: &str) -> bool {
        normalize_name(name).is_ok_and(|name| self.local.contains(&name))
    }
}

/// A lock file that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockError {
    /// The file in messages, as [`Lock::name`] would be.
    pub name: String,
    pub message: String,
}

/// Finds the lock file for the project whose `pyproject.toml`, with text
/// `pyproject`, is at `path`: a `uv.lock` or `pylock.toml` next to it, or the
/// `uv.lock` of a workspace in a parent directory that includes the project.
/// The lock is named in messages by its path relative to the project's
/// directory, after `prefix`, such as `packages/demo/../../uv.lock`.
///
/// Returns `None` if there is no lock file, or if the nearest `uv.lock` in a
/// parent directory is for another project.
pub fn find(path: &Path, pyproject: &str, prefix: &str) -> Option<Result<Lock, LockError>> {
    let dir = std::path::absolute(path).ok()?.parent()?.to_path_buf();
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
            return match lock {
                // A parent's lock is for another project unless it includes this one.
                Ok(lock)
                    if depth == 0 || name.as_deref().is_some_and(|n| lock.has_local_package(n)) =>
                {
                    Some(Ok(lock))
                }
                Ok(_) => None,
                Err(message) => Some(Err(LockError {
                    name: display,
                    message,
                })),
            };
        }
    }
    None
}

/// The `[project]` name in the text of a `pyproject.toml` file, if it has one,
/// for finding the workspace lock file that includes the project.
pub fn project_name(pyproject: &str) -> Option<String> {
    let root = DeTable::parse(pyproject).ok()?;
    let (_, project) = get(root.get_ref(), "project")?;
    let (_, name) = get(project.get_ref().as_table()?, "name")?;
    name.get_ref().as_str().map(str::to_owned)
}

/// Whether a `uv.lock` package comes from the workspace rather than an index,
/// a URL, or Git.
fn is_local(package: &DeTable<'_>) -> bool {
    get(package, "source")
        .and_then(|(_, source)| source.get_ref().as_table())
        .is_some_and(|source| {
            source
                .keys()
                .any(|key| matches!(key.get_ref().as_ref(), "editable" | "virtual" | "directory"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const UV_LOCK: &str = r#"version = 1
revision = 3
requires-python = ">=3.10"

[[package]]
name = "demo"
version = "0.1.0"
source = { editable = "." }

[[package]]
name = "Ruff"
version = "0.16.8"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "ty"
version = "0.0.80"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "ty"
version = "0.0.83"
source = { registry = "https://pypi.org/simple" }
"#;

    #[test]
    fn reads_uv_lock() {
        let lock = Lock::parse(LockKind::Uv, "uv.lock", UV_LOCK).unwrap();
        assert_eq!(lock.versions("ruff"), ["0.16.8"]);
        assert_eq!(lock.versions("ty"), ["0.0.80", "0.0.83"]);
        assert!(lock.versions("uv").is_empty());
        assert!(lock.has_local_package("Demo"));
        assert!(!lock.has_local_package("ruff"));
    }

    #[test]
    fn reads_pylock() {
        let text = "lock-version = \"1.0\"\ncreated-by = \"uv\"\n[[packages]]\nname = \"ruff\"\nversion = \"0.16.8\"\n";
        let lock = Lock::parse(LockKind::Pylock, "pylock.toml", text).unwrap();
        assert_eq!(lock.versions("ruff"), ["0.16.8"]);
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(Lock::parse(LockKind::Uv, "uv.lock", "[[package]\n").is_err());
    }
}
