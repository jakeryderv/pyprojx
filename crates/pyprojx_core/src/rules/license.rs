//! `project.license`, `project.license-files`, and `License ::` classifiers (PEP 639).

use toml::de::{DeTable, DeValue};

use super::Context;
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::{describe_type, get};
use crate::standards::{LicenseExpression, check_glob_pattern, check_license_expression};

pub(super) fn check(context: &mut Context<'_>, project: &DeTable<'_>) {
    let license = get(project, "license");
    let mut expression_span = None;
    if let Some((_, value)) = license {
        match value.get_ref() {
            DeValue::String(expression) => {
                expression_span = Some(value.span());
                check_expression(context, expression, value.span());
            }
            DeValue::Table(table) => {
                context.report(
                    Diagnostic::new(
                        Rule::DeprecatedMetadata,
                        "the `project.license` table is deprecated",
                        value.span(),
                    )
                    .with_help("use an SPDX license expression such as `license = \"MIT\"`, and list license files in `license-files` (PEP 639)"),
                );
                context.check_keys(table, "`project.license`", &["file", "text"]);
                let file = get(table, "file");
                let text = get(table, "text");
                for (name, entry) in [("file", file), ("text", text)] {
                    if let Some((_, value)) = entry {
                        context.expect_string(&format!("project.license.{name}"), value);
                    }
                }
                if let (Some((file_key, _)), Some((text_key, _))) = (file, text) {
                    context.report(
                        Diagnostic::new(
                            Rule::InvalidValue,
                            "`project.license` cannot set both `file` and `text`",
                            text_key.span(),
                        )
                        .with_label(file_key.span(), "`file` is also set here"),
                    );
                }
            }
            other => context.report(Diagnostic::new(
                Rule::InvalidType,
                format!(
                    "`project.license` must be a string, found {}",
                    describe_type(other)
                ),
                value.span(),
            )),
        }
    }

    if let Some((_, value)) = get(project, "license-files") {
        for (pattern, span) in context.expect_string_array("project.license-files", value) {
            if let Err(reason) = check_glob_pattern(pattern) {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("invalid glob pattern `{pattern}`"),
                        span,
                    )
                    .with_help(reason)
                    .as_warning(),
                );
            }
        }
    }

    if let Some((_, value)) = get(project, "classifiers")
        && let Some(classifiers) = value.get_ref().as_array()
    {
        for classifier in classifiers {
            let Some(text) = classifier.get_ref().as_str() else {
                continue;
            };
            if !text.starts_with("License ::") {
                continue;
            }
            let mut diagnostic = Diagnostic::new(
                Rule::DeprecatedMetadata,
                "`License ::` classifiers are deprecated",
                classifier.span(),
            )
            .with_help("use an SPDX license expression in `project.license` instead (PEP 639)");
            if let Some(expression) = &expression_span {
                // The specification lets backends reject this, which the
                // compatibility checks report.
                diagnostic = diagnostic
                    .with_label(expression.clone(), "license expression set here")
                    .with_help("remove the classifier; the license expression replaces it");
            }
            context.report(diagnostic);
        }
    }
}

fn check_expression(context: &mut Context<'_>, expression: &str, span: std::ops::Range<usize>) {
    match check_license_expression(expression) {
        LicenseExpression::Valid => {}
        LicenseExpression::Case(canonical) => context.report(
            Diagnostic::new(
                Rule::InvalidValue,
                format!("license expression should be written as `{canonical}`"),
                span,
            )
            .with_help("tools normalize the capitalization, but the canonical form is clearer")
            .as_warning(),
        ),
        LicenseExpression::Deprecated(canonical) => context.report(
            Diagnostic::new(
                Rule::DeprecatedMetadata,
                format!("license expression `{expression}` uses deprecated SPDX identifiers"),
                span,
            )
            .with_help(format!("use `{canonical}`")),
        ),
        LicenseExpression::Invalid {
            problem,
            suggestion,
        } => {
            let help = match suggestion {
                Some(suggestion) => format!("did you mean `{suggestion}`?"),
                None => {
                    "use an SPDX license expression such as `MIT` or `Apache-2.0 OR MIT`".to_owned()
                }
            };
            let span = context.problem_span(span, &problem);
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("invalid license expression `{expression}`"),
                    span,
                )
                .with_help(help),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    /// Checks a `[project]` body, returning (rule, severity, spanned text).
    fn project(body: &str) -> Vec<(&'static str, Severity, String)> {
        let text = format!("[project]\nname = \"demo\"\nversion = \"1\"\n{body}");
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .filter(|d| d.rule != crate::Rule::UnsupportedFeature)
            .map(|d| (d.rule.name(), d.severity(), text[d.span].to_owned()))
            .collect()
    }

    #[test]
    fn valid_license_metadata() {
        let body = "license = \"MIT OR Apache-2.0\"\nlicense-files = [\"LICEN[CS]E*\", \"licenses/**\"]\nclassifiers = [\"Programming Language :: Python\"]\n";
        assert_eq!(project(body), []);
    }

    #[test]
    fn license_expressions() {
        assert_eq!(
            project("license = \"mit\"\n"),
            [("invalid-value", Severity::Warning, "\"mit\"".to_owned())]
        );
        assert_eq!(
            project("license = \"GPL-3.0+\"\n"),
            [(
                "deprecated-metadata",
                Severity::Warning,
                "\"GPL-3.0+\"".to_owned()
            )]
        );
        let found = project("license = \"Apache 2.0\"\n");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].0, found[0].1), ("invalid-value", Severity::Error));
    }

    #[test]
    fn license_table_is_deprecated() {
        assert_eq!(
            project("license = { text = \"MIT\" }\n"),
            [(
                "deprecated-metadata",
                Severity::Warning,
                "{ text = \"MIT\" }".to_owned()
            )]
        );
        let found = project("license = { file = \"LICENSE\", text = \"MIT\" }\n");
        assert_eq!(
            found[1],
            ("invalid-value", Severity::Error, "text".to_owned())
        );
    }

    #[test]
    fn license_classifiers() {
        let classifier = "\"License :: OSI Approved :: MIT License\"";
        assert_eq!(
            project(&format!("classifiers = [{classifier}]\n")),
            [(
                "deprecated-metadata",
                Severity::Warning,
                classifier.to_owned()
            )]
        );
        // Backends that reject the combination get a compatibility diagnostic
        // instead, such as setuptools, which builds projects without
        // `[build-system]`.
        let text = format!(
            "[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n[project]\nname = \"demo\"\nversion = \"1\"\nlicense = \"MIT\"\nclassifiers = [{classifier}]\n"
        );
        let found: Vec<_> = crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| (d.rule.name(), d.severity()))
            .collect();
        assert_eq!(found, [("deprecated-metadata", Severity::Warning)]);
        assert_eq!(
            project(&format!(
                "license = \"MIT\"\nclassifiers = [{classifier}]\n"
            )),
            []
        );
    }

    #[test]
    fn license_files_globs_are_warnings() {
        assert_eq!(
            project("license-files = [\"../LICENSE\"]\n"),
            [(
                "invalid-value",
                Severity::Warning,
                "\"../LICENSE\"".to_owned()
            )]
        );
    }
}
