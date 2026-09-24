//! `project.classifiers`: known trove classifiers, and Python version
//! classifiers consistent with `requires-python`.
//!
//! `License ::` classifiers are checked with the license metadata.

use toml::de::DeTable;

use super::Context;
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::standards::allows_python_minor;
use crate::trove::{self, Classifier};

const PYTHON_VERSION_PREFIX: &str = "Programming Language :: Python :: ";

pub(super) fn check(context: &mut Context<'_>, project: &DeTable<'_>) {
    let Some((_, value)) = get(project, "classifiers") else {
        return;
    };
    let Some(classifiers) = value.get_ref().as_array() else {
        // The type is reported with the rest of `[project]`.
        return;
    };
    let requires_python = get(project, "requires-python").and_then(|(_, value)| {
        value
            .get_ref()
            .as_str()
            .map(|specifiers| (specifiers, value.span()))
    });

    for classifier in classifiers {
        let Some(name) = classifier.get_ref().as_str() else {
            continue;
        };
        match trove::lookup(name) {
            Classifier::Valid | Classifier::Private => {}
            Classifier::Deprecated(replacements) => {
                let help = match replacements {
                    [] => "remove it; it has no replacement".to_owned(),
                    [replacement] => format!("use `{replacement}`"),
                    _ => format!(
                        "use one of {}",
                        replacements
                            .iter()
                            .map(|item| format!("`{item}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("classifier `{name}` is deprecated"),
                        classifier.span(),
                    )
                    .with_help(format!("{help}; PyPI rejects deprecated classifiers")),
                );
            }
            Classifier::Unknown => {
                let help = match trove::suggest(name) {
                    Some(suggestion) => format!("did you mean `{suggestion}`?"),
                    None => format!(
                        "see https://pypi.org/classifiers/ (known classifiers from trove-classifiers {})",
                        trove::VERSION
                    ),
                };
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("unknown classifier `{name}`"),
                        classifier.span(),
                    )
                    .with_help(help),
                );
            }
        }

        if let Some((specifiers, specifiers_span)) = &requires_python
            && let Some(version) = name.strip_prefix(PYTHON_VERSION_PREFIX)
            && let Some((major, minor)) = parse_minor_version(version)
            && allows_python_minor(specifiers, major, minor) == Some(false)
        {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("classifier says Python {major}.{minor} is supported, but `requires-python` excludes it"),
                    classifier.span(),
                )
                .with_label(specifiers_span.clone(), "`requires-python` set here")
                .with_help("remove the classifier or widen `requires-python`")
                .as_warning(),
            );
        }
    }
}

/// Parses `3.12` into `(3, 12)`; other forms, such as `3` or `3 :: Only`, are skipped.
fn parse_minor_version(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    fn project(body: &str) -> Vec<(Severity, String, String)> {
        let text = format!("[project]\nname = \"demo\"\nversion = \"1\"\n{body}");
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| (d.severity(), d.message, d.help.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn valid_and_private_classifiers() {
        let body = "requires-python = \">=3.11\"\nclassifiers = [\"Programming Language :: Python :: 3\", \"Programming Language :: Python :: 3.11\", \"Programming Language :: Python :: 3 :: Only\", \"Private :: Do Not Upload\"]\n";
        assert_eq!(project(body), []);
    }

    #[test]
    fn unknown_and_deprecated_classifiers_are_errors() {
        let found = project(
            "classifiers = [\"Programming Language :: Pythn :: 3\", \"Natural Language :: Ukranian\"]\n",
        );
        assert_eq!(
            found,
            [
                (
                    Severity::Error,
                    "unknown classifier `Programming Language :: Pythn :: 3`".to_owned(),
                    "did you mean `Programming Language :: Python :: 3`?".to_owned()
                ),
                (
                    Severity::Error,
                    "classifier `Natural Language :: Ukranian` is deprecated".to_owned(),
                    "use `Natural Language :: Ukrainian`; PyPI rejects deprecated classifiers"
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn python_versions_excluded_by_requires_python() {
        let found = project(
            "requires-python = \">=3.10\"\nclassifiers = [\"Programming Language :: Python :: 3.9\", \"Programming Language :: Python :: 3.10\"]\n",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, Severity::Warning);
        assert!(found[0].1.contains("Python 3.9"));
        // A patch-level lower bound still allows the minor version.
        assert_eq!(
            project(
                "requires-python = \">=3.8.1\"\nclassifiers = [\"Programming Language :: Python :: 3.8\"]\n"
            ),
            []
        );
    }
}
