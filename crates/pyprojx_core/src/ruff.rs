//! Ruff's configuration options and rule selectors across releases.
//!
//! The data is generated from each release's configuration schema by
//! `scripts/update_ruff_data.py`.

use crate::ruff_data::{OPTIONS, REDIRECTS, RELEASES, SELECTORS};

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
    /// Whether the values are rule selectors, in a list or, for a map, in
    /// lists for each key.
    pub selectors: bool,
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
        added_after(self.present, release)
    }

    /// The release that removed the option, if it was accepted before `release`.
    pub fn removed_before(&self, release: usize) -> Option<usize> {
        removed_before(self.present, release)
    }

    /// The first release of the deprecation that includes `release`.
    pub fn deprecated_since(&self, release: usize) -> Option<usize> {
        self.deprecated
            .iter()
            .find(|&&(first, last)| (usize::from(first)..=usize::from(last)).contains(&release))
            .map(|&(first, _)| usize::from(first))
    }
}

/// A rule's status in the latest known release, with indexes into [`releases`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleStatus {
    /// Stable since the release; a preview rule before it.
    Stable(u16),
    Preview,
    /// Removed in `removed`, and deprecated from `deprecated` until then.
    Removed {
        deprecated: Option<u16>,
        removed: u16,
    },
}

/// A rule selector, such as `E`, `E5`, `E501`, or, with preview, a rule name,
/// and the releases that accept it.
#[derive(Debug)]
pub struct SelectorData {
    pub selector: &'static str,
    pub present: &'static [(u16, u16)],
    /// For a single rule's code.
    pub status: Option<RuleStatus>,
}

impl SelectorData {
    pub fn is_present(&self, release: usize) -> bool {
        contains(self.present, release)
    }

    /// Whether the selector is a rule name, which needs preview.
    pub fn is_name(&self) -> bool {
        self.selector.starts_with(|c: char| c.is_ascii_lowercase())
    }

    /// Whether the selector is a single rule that is in preview in `release`.
    pub fn is_preview(&self, release: usize) -> bool {
        match self.status {
            Some(RuleStatus::Preview) => true,
            Some(RuleStatus::Stable(since)) => release < usize::from(since),
            Some(RuleStatus::Removed { .. }) | None => false,
        }
    }

    /// Whether the selector is a single rule that `release` deprecates.
    pub fn is_deprecated(&self, release: usize) -> bool {
        match self.status {
            Some(RuleStatus::Removed {
                deprecated: Some(deprecated),
                removed,
            }) => (usize::from(deprecated)..usize::from(removed)).contains(&release),
            _ => false,
        }
    }

    pub fn added_after(&self, release: usize) -> Option<usize> {
        added_after(self.present, release)
    }

    pub fn removed_before(&self, release: usize) -> Option<usize> {
        removed_before(self.present, release)
    }
}

fn contains(ranges: &[(u16, u16)], release: usize) -> bool {
    ranges
        .iter()
        .any(|&(first, last)| (usize::from(first)..=usize::from(last)).contains(&release))
}

fn added_after(ranges: &[(u16, u16)], release: usize) -> Option<usize> {
    ranges
        .iter()
        .map(|&(first, _)| usize::from(first))
        .find(|&first| first > release)
}

fn removed_before(ranges: &[(u16, u16)], release: usize) -> Option<usize> {
    ranges
        .iter()
        .rev()
        .map(|&(_, last)| usize::from(last) + 1)
        .find(|&removed| removed <= release)
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

/// The selector `selector` in any known release.
pub fn selector(selector: &str) -> Option<&'static SelectorData> {
    SELECTORS
        .binary_search_by(|data| data.selector.cmp(selector))
        .ok()
        .map(|index| &SELECTORS[index])
}

/// The code Ruff redirects `code` to, if any.
pub fn redirect(code: &str) -> Option<&'static str> {
    REDIRECTS
        .binary_search_by(|(old, _)| old.cmp(&code))
        .ok()
        .map(|index| REDIRECTS[index].1)
}

/// The first release that, with preview, warns about unknown rule selectors
/// instead of rejecting them. Found by comparing releases.
pub const LENIENT_PREVIEW_SINCE: &str = "0.16.0";

/// Whether `release` warns about unknown rule selectors when preview is enabled.
pub fn is_lenient_with_preview(release: usize) -> bool {
    let since = RELEASES
        .iter()
        .position(|release| *release == LENIENT_PREVIEW_SINCE)
        .expect("the release is known");
    release >= since
}

/// Whether every rule `selector` matches in `release` is in preview, so that
/// selecting it has no effect without preview.
///
/// Rules are matched by prefix, which can include rules of other linters (`E`
/// matches `ERA001`) but never misses one, so this errs toward `false`.
pub fn selects_only_preview_rules(selector: &str, release: usize) -> bool {
    let mut rules = SELECTORS
        .iter()
        .filter(|data| data.status.is_some() && data.selector.starts_with(selector))
        .filter(|data| data.is_present(release))
        .peekable();
    rules.peek().is_some() && rules.all(|data| data.is_preview(release))
}

/// The rule codes and prefixes `release` accepts.
pub fn codes_in(release: usize) -> Vec<&'static str> {
    SELECTORS
        .iter()
        .filter(|data| !data.is_name() && data.is_present(release))
        .map(|data| data.selector)
        .collect()
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
        assert!(SELECTORS.is_sorted_by(|a, b| a.selector < b.selector));
        assert!(REDIRECTS.is_sorted_by(|a, b| a.0 < b.0));
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

        let removed = selector("ANN101").unwrap();
        assert!(!removed.is_present(latest));
        assert_eq!(releases()[removed.removed_before(latest).unwrap()], "0.8.0");
        assert!(selector("AIR003").unwrap().is_preview(latest));
        let ann101 = selector("ANN101").unwrap();
        let v0_5 = releases().iter().position(|r| *r == "0.5.0").unwrap();
        assert!(
            ann101.is_deprecated(v0_5) && !ann101.is_deprecated(0) && !ann101.is_deprecated(latest)
        );
        assert!(!selector("E501").unwrap().is_preview(latest));
        assert_eq!(redirect("PLR1701"), Some("SIM101"));
        assert!(codes_in(latest).contains(&"E5"));
        assert!(selects_only_preview_rules("AIR003", latest));
        assert!(!selects_only_preview_rules("E", latest));
        assert!(is_lenient_with_preview(latest) && !is_lenient_with_preview(0));
    }
}
