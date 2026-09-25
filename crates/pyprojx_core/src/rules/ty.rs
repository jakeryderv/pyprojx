//! `[tool.ty]`, checked against the ty releases the project allows.
//!
//! The allowed releases come from `ty` requirements in the project's
//! dependencies, extras, and dependency groups. Beyond the options and values
//! each release accepts, rule names in `rules` tables are checked here.

use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::tool::{Checker, Extension, check_python_version};
use super::{Context, suggest};
use crate::diagnostic::{Diagnostic, Fix, Rule};
use crate::document::get;
use crate::fix::rename;
use crate::tool::{OptionData, Treatment};
use crate::ty::{self, TY};

/// ty's checks beyond options and their values.
struct TyChecks;

impl Extension for TyChecks {
    fn option(
        &mut self,
        context: &mut Context<'_>,
        checker: &Checker,
        option: &OptionData,
        value: &toml::Spanned<DeValue<'_>>,
    ) {
        if matches!(option.path, "rules" | "overrides.rules")
            && let Some(rules) = value.get_ref().as_table()
        {
            for key in rules.keys() {
                check_rule(context, checker, rules, key.get_ref(), key.span());
            }
        }
    }
}

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((ty, span, key)) = get(root, "tool")
        .and_then(|(_, tool)| tool.get_ref().as_table())
        .and_then(|tool| get(tool, "ty"))
        .and_then(|(key, ty)| Some((ty.get_ref().as_table()?, ty.span(), key.span())))
    else {
        return;
    };
    let checker = Checker::new(context, &TY, root, ty, Some("ty"), None);
    if checker.candidates.is_empty() {
        checker.report_unchecked(context, key);
        return;
    }
    let start = context.diagnostics.len();
    checker.check_table(context, &mut TyChecks, (ty, span), "");
    check_python_version_setting(context, root, &checker, ty);
    checker.annotate(context, start);
}

/// Checks a rule name, which ty only warns about if it does not know it.
fn check_rule(
    context: &mut Context<'_>,
    checker: &Checker,
    rules: &DeTable<'_>,
    name: &str,
    span: Range<usize>,
) {
    let data = ty::rule(name);
    let lacking: Vec<usize> = checker
        .candidates
        .iter()
        .copied()
        .filter(|&index| !data.is_some_and(|data| data.is_present(index)))
        .collect();
    let Some(&first_lacking) = lacking.first() else {
        return;
    };
    let Some(data) = data else {
        let known = ty::rules_in(checker.newest());
        let suggestion = suggest(name, &known);
        let help = match suggestion {
            Some(suggestion) => format!("did you mean `{suggestion}`?"),
            None => {
                "ty ignores rules it does not know; see https://docs.astral.sh/ty/reference/rules/"
                    .to_owned()
            }
        };
        let mut diagnostic = Diagnostic::new(
            Rule::InvalidValue,
            format!("unknown rule `{name}`"),
            span.clone(),
        )
        .with_help(help)
        .as_warning();
        // The suggestion is a guess, and enabling a rule changes what ty reports.
        if let Some(suggestion) = suggestion
            && get(rules, suggestion).is_none()
        {
            let edit = rename(context.text, span, name, suggestion);
            diagnostic = diagnostic.with_fix(Fix::unsafe_(vec![edit]));
        }
        context.report(diagnostic);
        return;
    };
    checker.report_missing(
        context,
        &format!("the rule `{name}`"),
        (
            data.added_after(first_lacking),
            data.removed_before(first_lacking),
        ),
        span,
        lacking.len() == checker.candidates.len(),
        // ty warns about rules it does not know and ignores them.
        Treatment::Warns,
    );
}

/// Warns when `environment.python-version` is newer than the oldest Python
/// that `requires-python` allows.
fn check_python_version_setting(
    context: &mut Context<'_>,
    root: &DeTable<'_>,
    checker: &Checker,
    ty: &DeTable<'_>,
) {
    let Some((_, version)) = get(ty, "environment")
        .and_then(|(_, environment)| environment.get_ref().as_table())
        .and_then(|environment| get(environment, "python-version"))
    else {
        return;
    };
    let Some(minor) = version
        .get_ref()
        .as_str()
        .and_then(|text| text.strip_prefix("3."))
        .and_then(|minor| minor.parse::<u64>().ok())
    else {
        return;
    };
    check_python_version(
        context,
        root,
        checker,
        ("environment.python-version", version, minor),
        |minor| format!("3.{minor}"),
        "ty may accept code that needs",
    );
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    /// Checks `[tool.ty]` settings with ty pinned to `version` (or unpinned),
    /// returning (rule, severity, message, spanned text).
    fn ty(version: Option<&str>, body: &str) -> Vec<(&'static str, Severity, String, String)> {
        let requirement = version.map_or_else(|| "ty".to_owned(), |v| format!("ty=={v}"));
        let text = format!(
            "[project]\nname = \"demo\"\nversion = \"1\"\nrequires-python = \">=3.10\"\n[dependency-groups]\ndev = [\"{requirement}\"]\n{body}"
        );
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| {
                (
                    d.rule.name(),
                    d.severity(),
                    d.message,
                    text[d.span].to_owned(),
                )
            })
            .collect()
    }

    fn short<'a>(
        found: &'a [(&'static str, Severity, String, String)],
    ) -> Vec<(&'static str, Severity, &'a str)> {
        found
            .iter()
            .map(|(rule, severity, _, span)| (*rule, *severity, span.as_str()))
            .collect()
    }

    #[test]
    fn valid_configuration() {
        let body = "[tool.ty.environment]\npython-version = \"3.10\"\n[tool.ty.rules]\nunresolved-import = \"error\"\n[[tool.ty.overrides]]\ninclude = [\"tests/**\"]\n[tool.ty.overrides.rules]\nunresolved-reference = \"ignore\"\n";
        assert_eq!(ty(None, body), []);
    }

    #[test]
    fn unknown_keys_and_values() {
        let found = ty(None, "[tool.ty.environment]\npython-verison = \"3.10\"\n");
        assert_eq!(
            short(&found),
            [("unknown-key", Severity::Error, "python-verison")]
        );
        assert_eq!(
            found[0].2,
            "unknown key `python-verison` in `[tool.ty.environment]`"
        );
        let found = ty(
            None,
            "[[tool.ty.overrides]]\ninclude = [\"x\"]\nbogus = 1\n",
        );
        assert_eq!(found[0].2, "unknown key `bogus` in `[[tool.ty.overrides]]`");
        assert_eq!(
            short(&ty(
                None,
                "[tool.ty.rules]\nunresolved-import = \"fatal\"\n"
            )),
            [("invalid-value", Severity::Error, "\"fatal\"")]
        );
        assert_eq!(
            short(&ty(None, "[tool.ty]\noverrides = 1\n")),
            [("invalid-type", Severity::Error, "1")]
        );
    }

    #[test]
    fn unknown_rules_are_warnings() {
        let found = ty(None, "[tool.ty.rules]\nunresolved-imprt = \"error\"\n");
        assert_eq!(
            short(&found),
            [("invalid-value", Severity::Warning, "unresolved-imprt")]
        );
        assert_eq!(found[0].2, "unknown rule `unresolved-imprt`");
        let found = ty(
            None,
            "[[tool.ty.overrides]]\ninclude = [\"x\"]\nrules = { unresolved-imprt = \"error\" }\n",
        );
        assert_eq!(
            short(&found),
            [("invalid-value", Severity::Warning, "unresolved-imprt")]
        );
    }

    #[test]
    fn options_and_rules_the_allowed_versions_lack() {
        // `analysis.strict-equality-semantics` was added in 0.0.63.
        let body = "[tool.ty.analysis]\nstrict-equality-semantics = true\n";
        assert_eq!(
            short(&ty(Some("0.0.40"), body)),
            [(
                "unsupported-feature",
                Severity::Error,
                "strict-equality-semantics"
            )]
        );
        assert_eq!(ty(Some("0.0.83"), body), []);
        // `src.root` was deprecated, then removed.
        let body = "[tool.ty.src]\nroot = \".\"\n";
        assert_eq!(
            short(&ty(Some("0.0.30"), body)),
            [("deprecated-setting", Severity::Warning, "root")]
        );
        assert_eq!(
            short(&ty(Some("0.0.83"), body)),
            [("unsupported-feature", Severity::Error, "root")]
        );
    }

    #[test]
    fn python_version_values_and_requires_python() {
        let setting =
            |version: &str| format!("[tool.ty.environment]\npython-version = \"{version}\"\n");
        assert_eq!(
            short(&ty(None, &setting("3.16"))),
            [("invalid-value", Severity::Error, "\"3.16\"")]
        );
        let found = ty(None, &setting("3.12"));
        assert_eq!(
            short(&found),
            [("invalid-value", Severity::Warning, "\"3.12\"")]
        );
        assert_eq!(
            found[0].2,
            "`python-version` is Python 3.12, but `requires-python` allows Python 3.10"
        );
    }
}
