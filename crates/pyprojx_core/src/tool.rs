//! Tool configuration options across releases, such as Ruff's `[tool.ruff]`.
//!
//! Each tool's data is generated from the configuration schema of each of its
//! releases by a script in `scripts/`, which records the releases that accept
//! and deprecate each option and the values each release accepts.

use toml::de::DeValue;

/// A tool whose configuration pyprojx checks.
#[derive(Debug)]
pub struct Tool {
    /// The tool's name in messages, such as "Ruff".
    pub name: &'static str,
    /// The configuration table, such as `tool.ruff`.
    pub table: &'static str,
    /// Where the tool documents its options.
    pub docs: &'static str,
    /// How to require a release, with `{version}` for its version, such as
    /// "require `ruff>={version}`".
    pub upgrade: &'static str,
    /// How to tell pyprojx which version the project uses, when nothing does.
    pub pin: &'static str,
    /// Known releases, oldest first. Other data refers to them by index.
    pub releases: &'static [&'static str],
    /// Options by path, sorted.
    pub options: &'static [OptionData],
    /// Options the tables that hold them require, sorted.
    pub required: &'static [&'static str],
    /// Options a release renamed but still accepts, with their new names, sorted.
    pub renamed: &'static [(&'static str, &'static str)],
    /// What the tool does with an unknown key in each table (`""` for the top
    /// level), where it does not reject it, sorted.
    pub unknown_keys: &'static [(&'static str, Treatment)],
    /// What the tool does with an invalid value for each option, where it does
    /// not reject it, sorted.
    pub invalid_values: &'static [(&'static str, Treatment)],
    /// What else happens when the tool warns about a setting it cannot read,
    /// such as ignoring the other settings.
    pub warning: Option<&'static str>,
}

/// What a tool does with a setting it cannot read, measured by running it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Treatment {
    /// It fails.
    Rejects,
    /// It warns, and may ignore more than the setting; see [`Tool::warning`].
    Warns,
    /// It ignores the setting without a word.
    Ignores,
}

impl Tool {
    /// The release at `index`.
    pub fn release(&self, index: usize) -> &'static str {
        self.releases[index]
    }

    /// The index of the latest known release.
    pub fn latest(&self) -> usize {
        self.releases.len() - 1
    }

    /// The option at `path`, such as `lint.select`, in any known release.
    pub fn option(&self, path: &str) -> Option<&'static OptionData> {
        let options = self.options;
        options
            .binary_search_by(|option| option.path.cmp(path))
            .ok()
            .map(|index| &options[index])
    }

    /// The names of options directly in the table at `prefix`, such as `lint.`,
    /// or `""` for the top level, that `release` accepts.
    pub fn names_in(&self, prefix: &str, release: usize) -> Vec<&'static str> {
        self.options
            .iter()
            .filter(|option| option.is_present(release))
            .filter_map(|option| option.path.strip_prefix(prefix))
            .filter(|name| !name.contains('.'))
            .collect()
    }

    /// What the tool does with an unknown key in the table at `path`, such as
    /// `lint`, or `""` for the top level.
    pub fn unknown_key(&self, path: &str) -> Treatment {
        lookup(self.unknown_keys, path)
    }

    /// What the tool does with an invalid value for the option at `path`.
    pub fn invalid_value(&self, path: &str) -> Treatment {
        lookup(self.invalid_values, path)
    }

    /// The new name of the option at `path`, if a release renamed it.
    pub fn new_name(&self, path: &str) -> Option<&'static str> {
        self.renamed
            .binary_search_by(|(old, _)| (*old).cmp(path))
            .ok()
            .map(|index| self.renamed[index].1)
    }

    /// Whether the table that holds the option at `path` requires it.
    pub fn is_required(&self, path: &str) -> bool {
        self.required.binary_search(&path).is_ok()
    }

    /// How to require `release`, such as "require `ruff>=0.8.0`".
    pub fn upgrade_to(&self, release: usize) -> String {
        self.upgrade.replace("{version}", self.release(release))
    }
}

/// What an option holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    /// A table of options, such as `lint`.
    Table,
    /// A table whose keys the user chooses, such as `lint.per-file-ignores`.
    Map,
    /// An array of tables of options, such as ty's `[[tool.ty.overrides]]`.
    TableArray,
    Value,
}

/// The values an option accepts, as its schema describes them. Tools do not
/// enforce the maximums some schemas give, so they are left out.
#[derive(Debug, PartialEq, Eq)]
pub enum ValueType {
    Any,
    Bool,
    Int {
        min: Option<i64>,
    },
    /// An integer or a float.
    Number,
    Str,
    /// One of these strings.
    Enum(&'static [&'static str]),
    Array(&'static ValueType),
    OneOf(&'static [ValueType]),
    /// A Ruff rule selector, which is checked separately.
    Selector,
}

impl ValueType {
    /// Whether `value` has this type.
    pub fn accepts(&self, value: &DeValue<'_>) -> bool {
        match self {
            Self::Any => true,
            Self::Bool => value.as_bool().is_some(),
            Self::Int { min } => value
                .as_integer()
                .and_then(|integer| i64::from_str_radix(integer.as_str(), integer.radix()).ok())
                .is_some_and(|integer| min.is_none_or(|min| integer >= min)),
            Self::Number => matches!(value, DeValue::Integer(_) | DeValue::Float(_)),
            Self::Str | Self::Selector => value.as_str().is_some(),
            Self::Enum(variants) => value.as_str().is_some_and(|text| variants.contains(&text)),
            Self::Array(item) => value
                .as_array()
                .is_some_and(|items| items.iter().all(|entry| item.accepts(entry.get_ref()))),
            Self::OneOf(types) => types.iter().any(|ty| ty.accepts(value)),
        }
    }

    /// Describes the type in messages, such as "an integer of at least 1".
    pub fn describe(&self) -> String {
        match self {
            Self::Any => "any value".to_owned(),
            Self::Bool => "a boolean".to_owned(),
            Self::Int { min: Some(min) } => format!("an integer of at least {min}"),
            Self::Int { min: None } => "an integer".to_owned(),
            Self::Number => "a number".to_owned(),
            Self::Str => "a string".to_owned(),
            Self::Selector => "a rule selector".to_owned(),
            Self::Enum([only]) => format!("`\"{only}\"`"),
            Self::Enum(variants) => format!(
                "one of {}",
                variants
                    .iter()
                    .map(|v| format!("`\"{v}\"`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Array(item) => format!("an array of {}", item.describe_plural()),
            Self::OneOf(types) => types
                .iter()
                .map(Self::describe)
                .collect::<Vec<_>>()
                .join(" or "),
        }
    }

    fn describe_plural(&self) -> String {
        match self {
            Self::Bool => "booleans".to_owned(),
            Self::Int { .. } => "integers".to_owned(),
            Self::Str => "strings".to_owned(),
            Self::Selector => "rule selectors".to_owned(),
            Self::Enum(_) => format!("strings, each {}", self.describe()),
            other => format!("values, each {}", other.describe()),
        }
    }
}

/// An option, such as `lint.isort.known-first-party`, and the releases that
/// accept and deprecate it.
#[derive(Debug)]
pub struct OptionData {
    pub path: &'static str,
    pub kind: OptionKind,
    /// Whether the values are Ruff rule selectors, in a list or, for a map, in
    /// lists for each key.
    pub selectors: bool,
    /// The type of the option's values, or of each value in a map, in inclusive
    /// ranges of releases.
    pub types: &'static [(u16, u16, &'static ValueType)],
    /// Inclusive ranges of indexes into the tool's releases.
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

    /// The type of the option's values in `release`.
    pub fn value_type(&self, release: usize) -> Option<&'static ValueType> {
        self.types
            .iter()
            .find(|&&(first, last, _)| (usize::from(first)..=usize::from(last)).contains(&release))
            .map(|&(_, _, ty)| ty)
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

fn lookup(treatments: &[(&str, Treatment)], path: &str) -> Treatment {
    treatments
        .binary_search_by(|(other, _)| (*other).cmp(path))
        .map_or(Treatment::Rejects, |index| treatments[index].1)
}

/// Whether inclusive ranges of releases include `release`.
pub(crate) fn contains(ranges: &[(u16, u16)], release: usize) -> bool {
    ranges
        .iter()
        .any(|&(first, last)| (usize::from(first)..=usize::from(last)).contains(&release))
}

/// The first release after `release` in inclusive ranges of releases.
pub(crate) fn added_after(ranges: &[(u16, u16)], release: usize) -> Option<usize> {
    ranges
        .iter()
        .map(|&(first, _)| usize::from(first))
        .find(|&first| first > release)
}

/// The release after the last range that ends before `release`.
pub(crate) fn removed_before(ranges: &[(u16, u16)], release: usize) -> Option<usize> {
    ranges
        .iter()
        .rev()
        .map(|&(_, last)| usize::from(last) + 1)
        .find(|&removed| removed <= release)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::Tool;

    /// Checks that a tool's data is sorted and refers to known releases.
    pub fn assert_consistent(tool: &Tool) {
        assert!(tool.options.is_sorted_by(|a, b| a.path < b.path));
        assert!(tool.required.is_sorted());
        assert!(tool.unknown_keys.is_sorted_by(|a, b| a.0 < b.0));
        assert!(tool.invalid_values.is_sorted_by(|a, b| a.0 < b.0));
        for path in tool
            .required
            .iter()
            .chain(tool.invalid_values.iter().map(|(p, _)| p))
        {
            assert!(tool.option(path).is_some(), "{path}");
        }
        let last = u16::try_from(tool.latest()).unwrap();
        for option in tool.options {
            assert!(!option.present.is_empty(), "{}", option.path);
            for &(first, end) in option.present.iter().chain(option.deprecated) {
                assert!(first <= end && end <= last, "{}", option.path);
            }
            for &(first, end, _) in option.types {
                assert!(first <= end && end <= last, "{}", option.path);
            }
        }
    }
}
