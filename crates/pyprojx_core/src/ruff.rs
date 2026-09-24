//! Ruff's configuration options across releases.
//!
//! The data is generated from each release's configuration schema by
//! `scripts/update_ruff_data.py`.

use crate::ruff_data::{OPTIONS, RELEASES};

/// What an option holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    /// A table of options, such as `lint`.
    Table,
    /// A table whose keys the user chooses, such as `lint.per-file-ignores`.
    Map,
    Value,
}

/// An option, such as `lint.isort.known-first-party`, and the releases that
/// accept and deprecate it.
#[derive(Debug)]
pub struct OptionData {
    pub path: &'static str,
    pub kind: OptionKind,
    /// Inclusive ranges of indexes into [`releases`].
    pub present: &'static [(u16, u16)],
    pub deprecated: &'static [(u16, u16)],
    /// The latest release's deprecation message, if it deprecates the option.
    pub message: Option<&'static str>,
}

impl OptionData {
    pub fn is_present(&self, release: usize) -> bool {
        contains(self.present, release)
    }

    pub fn is_deprecated(&self, release: usize) -> bool {
        contains(self.deprecated, release)
    }

    /// The first release after `release` that accepts the option.
    pub fn added_after(&self, release: usize) -> Option<usize> {
        self.present
            .iter()
            .map(|&(first, _)| usize::from(first))
            .find(|&first| first > release)
    }

    /// The first release after the last one before `release` that accepts the option.
    pub fn removed_before(&self, release: usize) -> Option<usize> {
        self.present
            .iter()
            .rev()
            .map(|&(_, last)| usize::from(last) + 1)
            .find(|&removed| removed <= release)
    }

    /// The first release of the deprecation that includes `release`.
    pub fn deprecated_since(&self, release: usize) -> Option<usize> {
        self.deprecated
            .iter()
            .find(|&&(first, last)| (usize::from(first)..=usize::from(last)).contains(&release))
            .map(|&(first, _)| usize::from(first))
    }
}

fn contains(ranges: &[(u16, u16)], release: usize) -> bool {
    ranges
        .iter()
        .any(|&(first, last)| (usize::from(first)..=usize::from(last)).contains(&release))
}

/// Known Ruff releases, oldest first.
pub fn releases() -> &'static [&'static str] {
    RELEASES
}

/// The option at `path`, such as `lint.select`, in any known release.
pub fn option(path: &str) -> Option<&'static OptionData> {
    OPTIONS
        .binary_search_by(|option| option.path.cmp(path))
        .ok()
        .map(|index| &OPTIONS[index])
}

/// The names of options directly in the table at `prefix`, such as `lint.`, or
/// `""` for the top level, that `release` accepts.
pub fn names_in(prefix: &str, release: usize) -> Vec<&'static str> {
    OPTIONS
        .iter()
        .filter(|option| option.is_present(release))
        .filter_map(|option| option.path.strip_prefix(prefix))
        .filter(|name| !name.contains('.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_consistent() {
        assert!(OPTIONS.is_sorted_by(|a, b| a.path < b.path));
        let last = u16::try_from(RELEASES.len() - 1).unwrap();
        for option in OPTIONS {
            assert!(!option.present.is_empty(), "{}", option.path);
            for &(first, end) in option.present.iter().chain(option.deprecated) {
                assert!(first <= end && end <= last, "{}", option.path);
            }
        }
    }

    #[test]
    fn lookups() {
        let select = option("select").unwrap();
        let latest = releases().len() - 1;
        assert!(select.is_present(0) && select.is_deprecated(latest));
        assert_eq!(
            releases()[select.deprecated_since(latest).unwrap()],
            "0.2.0"
        );
        assert!(option("lint.isort.known-first-party").is_some());
        assert!(option("line-lenght").is_none());
        assert!(names_in("lint.", latest).contains(&"select"));
        assert!(!names_in("", latest).contains(&"known-first-party"));
    }
}
