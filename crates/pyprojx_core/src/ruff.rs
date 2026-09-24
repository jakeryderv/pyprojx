//! Ruff's configuration options and rule selectors across releases.
//!
//! The data is generated from each release's configuration schema and rule
//! metadata by `scripts/update_ruff_data.py`.

use crate::ruff_data::{
    DEPRECATED_VALUES, OPTIONS, PYTHON_IN_DEVELOPMENT, REDIRECTS, RELEASES, REQUIRED, SELECTORS,
};
use crate::tool::{Tool, added_after, contains, removed_before};

/// Ruff's configuration, `[tool.ruff]`.
pub static RUFF: Tool = Tool {
    name: "Ruff",
    table: "tool.ruff",
    docs: "https://docs.astral.sh/ruff/settings/",
    upgrade: "require `ruff>={version}`",
    releases: RELEASES,
    options: OPTIONS,
    required: REQUIRED,
    unknown_keys: &[],
    invalid_values: &[],
    warning: None,
};

/// A rule's status in the latest known release, with indexes into [`RUFF`]'s releases.
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

/// Whether `release` supports the `target-version` value only in preview,
/// warning that support is under development otherwise. Measured by running
/// each release.
pub fn is_python_in_development(value: &str, release: usize) -> bool {
    PYTHON_IN_DEVELOPMENT.iter().any(|&(python, first, last)| {
        python == value && (usize::from(first)..=usize::from(last)).contains(&release)
    })
}

/// If `release` deprecates `value` for the option at `path`, the first release
/// that does and what to use instead.
pub fn deprecated_value(path: &str, value: &str, release: usize) -> Option<(usize, &'static str)> {
    DEPRECATED_VALUES
        .iter()
        .find(|&&(option, deprecated, first, last, _)| {
            option == path
                && deprecated == value
                && (usize::from(first)..=usize::from(last)).contains(&release)
        })
        .map(|&(_, _, first, _, message)| (usize::from(first), message))
}

/// The rule codes and prefixes `release` accepts.
pub fn codes_in(release: usize) -> Vec<&'static str> {
    SELECTORS
        .iter()
        .filter(|data| !data.is_name() && data.is_present(release))
        .map(|data| data.selector)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_consistent() {
        crate::tool::tests::assert_consistent(&RUFF);
        assert!(SELECTORS.is_sorted_by(|a, b| a.selector < b.selector));
        assert!(REDIRECTS.is_sorted_by(|a, b| a.0 < b.0));
    }

    #[test]
    fn lookups() {
        let select = RUFF.option("select").unwrap();
        let latest = RUFF.latest();
        assert!(select.is_present(0) && select.is_deprecated(latest));
        assert_eq!(
            RUFF.releases[select.deprecated_since(latest).unwrap()],
            "0.2.0"
        );
        assert!(RUFF.option("lint.isort.known-first-party").is_some());
        assert!(RUFF.option("line-lenght").is_none());
        assert!(RUFF.names_in("lint.", latest).contains(&"select"));
        assert!(!RUFF.names_in("", latest).contains(&"known-first-party"));

        let removed = selector("ANN101").unwrap();
        assert!(!removed.is_present(latest));
        assert_eq!(
            RUFF.releases[removed.removed_before(latest).unwrap()],
            "0.8.0"
        );
        assert!(selector("AIR003").unwrap().is_preview(latest));
        let ann101 = selector("ANN101").unwrap();
        let v0_5 = RUFF.releases.iter().position(|r| *r == "0.5.0").unwrap();
        assert!(
            ann101.is_deprecated(v0_5) && !ann101.is_deprecated(0) && !ann101.is_deprecated(latest)
        );
        assert!(!selector("E501").unwrap().is_preview(latest));
        assert_eq!(redirect("PLR1701"), Some("SIM101"));
        assert!(codes_in(latest).contains(&"E5"));
        assert!(selects_only_preview_rules("AIR003", latest));
        assert!(!selects_only_preview_rules("E", latest));
        assert!(is_lenient_with_preview(latest) && !is_lenient_with_preview(0));
        assert!(is_python_in_development("py315", latest));
        assert!(!is_python_in_development("py314", latest));
        let v0_3 = RUFF.releases.iter().position(|r| *r == "0.3.0").unwrap();
        assert!(deprecated_value("output-format", "text", v0_3).is_some());
        assert!(deprecated_value("output-format", "text", latest).is_none());
    }
}
