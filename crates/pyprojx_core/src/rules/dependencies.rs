//! `project.dependencies` and `project.optional-dependencies` (extras).

use std::collections::HashMap;
use std::ops::Range;

use toml::de::DeTable;

use super::Context;
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::{Value, get};
use crate::standards::{check_requirement, normalize_name};

pub(super) fn check(context: &mut Context<'_>, project: &DeTable<'_>) {
    if let Some((_, value)) = get(project, "dependencies") {
        check_requirements(context, "project.dependencies", value);
    }
    let Some((_, value)) = get(project, "optional-dependencies") else {
        return;
    };
    let Some(extras) = context.expect_table("project.optional-dependencies", value) else {
        return;
    };
    let mut seen: HashMap<String, Range<usize>> = HashMap::new();
    for (extra, requirements) in extras {
        let name = extra.get_ref().as_ref();
        match normalize_name(name) {
            Ok(normalized) => {
                if let Some(first) = seen.get(&normalized) {
                    context.report(
                        Diagnostic::new(
                            Rule::DuplicateName,
                            format!("extra `{name}` is the same as another extra after normalization"),
                            extra.span(),
                        )
                        .with_label(first.clone(), "first defined here")
                        .with_help(format!("extra names are compared in normalized form, `{normalized}`")),
                    );
                } else {
                    seen.insert(normalized, extra.span());
                }
            }
            Err(_) => context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("invalid extra name `{name}`"),
                    extra.span(),
                )
                .with_help("names must start and end with a letter or digit and contain only letters, digits, `-`, `_`, and `.`"),
            ),
        }
        check_requirements(
            context,
            &format!("project.optional-dependencies.{name}"),
            requirements,
        );
    }
}

/// Checks an array of dependency specifiers.
pub(super) fn check_requirements(context: &mut Context<'_>, location: &str, value: &Value<'_>) {
    for (requirement, span) in context.expect_string_array(location, value) {
        if let Err(problem) = check_requirement(requirement) {
            let message = format!("invalid requirement: {}", problem.message);
            context.invalid_string(message, span, &problem);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::diagnostics;

    fn project(body: &str) -> Vec<(&'static str, String)> {
        let text = format!("[project]\nname = \"demo\"\nversion = \"1\"\n{body}");
        diagnostics(&text)
            .into_iter()
            .map(|(rule, _, span)| (rule, span.to_owned()))
            .collect()
    }

    #[test]
    fn valid_dependencies_and_extras() {
        let body = r#"dependencies = ["requests>=2.31", 'tomli; python_version < "3.11"']
[project.optional-dependencies]
cli = ["click"]
"all" = ["demo[cli]"]
"#;
        assert_eq!(project(body), []);
    }

    #[test]
    fn invalid_requirements() {
        assert_eq!(
            project("dependencies = [\"requests >= 2.3.*\", 1]\n"),
            [
                ("invalid-value", ">= 2.3.*".to_owned()),
                ("invalid-type", "1".to_owned()),
            ]
        );
        assert_eq!(
            project("dependencies = \"requests\"\n"),
            [("invalid-type", "\"requests\"".to_owned())]
        );
    }

    #[test]
    fn extras() {
        let body = "[project.optional-dependencies]\ndev = [\"pytest\"]\nDev = [\"ruff\"]\n\"foo bar\" = []\ndocs = [\"sphinx >\"]\n";
        assert_eq!(
            project(body),
            [
                ("duplicate-name", "Dev".to_owned()),
                ("invalid-value", "\"foo bar\"".to_owned()),
                ("invalid-value", ">".to_owned()),
            ]
        );
    }
}
