//! Trove classifiers, as accepted by PyPI.
//!
//! The data is generated from the `trove-classifiers` package by
//! `scripts/update_classifiers.py`.

use crate::trove_data::{CLASSIFIERS, DEPRECATED};

pub use crate::trove_data::VERSION;

/// The status of a classifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classifier {
    Valid,
    /// Classifiers starting with `Private ::`, which PyPI rejects on upload by
    /// design, to prevent accidental publishing.
    Private,
    /// Deprecated, with its replacements (possibly none).
    Deprecated(&'static [&'static str]),
    Unknown,
}

pub fn lookup(classifier: &str) -> Classifier {
    if CLASSIFIERS.binary_search(&classifier).is_ok() {
        Classifier::Valid
    } else if classifier.starts_with("Private ::") {
        Classifier::Private
    } else if let Ok(index) = DEPRECATED.binary_search_by(|(name, _)| name.cmp(&classifier)) {
        Classifier::Deprecated(DEPRECATED[index].1)
    } else {
        Classifier::Unknown
    }
}

/// The valid classifier closest to `classifier`, if one is close enough to be
/// a likely typo.
pub fn suggest(classifier: &str) -> Option<&'static str> {
    CLASSIFIERS
        .iter()
        .map(|candidate| (*candidate, strsim::jaro_winkler(classifier, candidate)))
        .filter(|(_, score)| *score >= 0.9)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(candidate, _)| candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_sorted_for_binary_search() {
        assert!(CLASSIFIERS.is_sorted());
        assert!(DEPRECATED.is_sorted_by(|a, b| a.0 < b.0));
    }

    #[test]
    fn lookups() {
        assert_eq!(
            lookup("Programming Language :: Python :: 3.14"),
            Classifier::Valid
        );
        assert_eq!(lookup("Private :: Do Not Upload"), Classifier::Private);
        assert_eq!(
            lookup("Natural Language :: Ukranian"),
            Classifier::Deprecated(&["Natural Language :: Ukrainian"])
        );
        assert_eq!(
            lookup("Programming Language :: Pythn :: 3"),
            Classifier::Unknown
        );
        assert_eq!(
            suggest("Programming Language :: Pythn :: 3"),
            Some("Programming Language :: Python :: 3")
        );
        assert_eq!(suggest("Something else entirely"), None);
    }
}
