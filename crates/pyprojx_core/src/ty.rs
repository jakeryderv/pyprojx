//! ty's configuration options and rules across releases.
//!
//! The data is generated from each release's configuration schema by
//! `scripts/update_ty_data.py`.

use crate::tool::{Tool, added_after, contains, removed_before};
use crate::ty_data::{OPTIONS, RELEASES, REQUIRED, RULES};

/// ty's configuration, `[tool.ty]`.
pub static TY: Tool = Tool {
    name: "ty",
    table: "tool.ty",
    docs: "https://docs.astral.sh/ty/reference/configuration/",
    upgrade: "require `ty>={version}`",
    pin: "add `ty` to a dependency group, or lock it, to check the version you use",
    releases: RELEASES,
    options: OPTIONS,
    required: REQUIRED,
    unknown_keys: &[],
    invalid_values: &[],
    warning: None,
};

/// A rule, such as `unresolved-import`, and the releases that know it.
#[derive(Clone, Copy, Debug)]
pub struct RuleData {
    pub name: &'static str,
    /// Inclusive ranges of indexes into [`TY`]'s releases.
    pub present: &'static [(u16, u16)],
}

impl RuleData {
    pub fn is_present(&self, release: usize) -> bool {
        contains(self.present, release)
    }

    pub fn added_after(&self, release: usize) -> Option<usize> {
        added_after(self.present, release)
    }

    pub fn removed_before(&self, release: usize) -> Option<usize> {
        removed_before(self.present, release)
    }
}

/// The rule `name` in any known release.
pub fn rule(name: &str) -> Option<RuleData> {
    RULES
        .binary_search_by(|(rule, _)| rule.cmp(&name))
        .ok()
        .map(|index| RuleData {
            name: RULES[index].0,
            present: RULES[index].1,
        })
}

/// The rules `release` knows.
pub fn rules_in(release: usize) -> Vec<&'static str> {
    RULES
        .iter()
        .filter(|(_, present)| contains(present, release))
        .map(|(name, _)| *name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_consistent() {
        crate::tool::tests::assert_consistent(&TY);
        assert!(RULES.is_sorted_by(|a, b| a.0 < b.0));
        let last = u16::try_from(TY.latest()).unwrap();
        for (name, present) in RULES {
            assert!(!present.is_empty(), "{name}");
            assert!(present.iter().all(|&(a, b)| a <= b && b <= last), "{name}");
        }
    }

    #[test]
    fn lookups() {
        let latest = TY.latest();
        assert!(rule("unresolved-import").unwrap().is_present(latest));
        assert!(rule("unresolved-imports").is_none());
        assert!(rules_in(latest).contains(&"possibly-unresolved-reference"));
        assert!(TY.option("overrides.rules").is_some());
        assert!(TY.option("environment.python-version").is_some());
    }
}
