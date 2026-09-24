//! `[tool.ruff]`, checked against the Ruff releases the project allows.
//!
//! The allowed releases come from `ruff` requirements in the project's
//! dependencies, extras, and dependency groups, narrowed by `required-version`,
//! which Ruff enforces. Beyond the options and values each release accepts,
//! Ruff's rule selectors and preview mode are checked here.

use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::tool::{Checker, Extension, check_python_version};
use super::{Context, suggest};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::ruff::{self, RUFF, RuleStatus, SelectorData};
use crate::tool::OptionData;

/// Ruff's checks beyond options and their values.
struct RuffChecks {
    /// Whether preview is enabled for the linter, which `lint.preview` can
    /// override, and globally.
    preview: bool,
    global_preview: bool,
    /// Top-level linter settings, which belong in `[tool.ruff.lint]`.
    moved: Vec<Range<usize>>,
}

impl Extension for RuffChecks {
    fn deprecated(&mut self, prefix: &str, name: &str, span: Range<usize>) -> bool {
        let moved = prefix.is_empty() && RUFF.option(&format!("lint.{name}")).is_some();
        if moved {
            self.moved.push(span);
        }
        moved
    }

    fn accepted(
        &mut self,
        context: &mut Context<'_>,
        checker: &Checker,
        option: &OptionData,
        value: &toml::Spanned<DeValue<'_>>,
    ) {
        self.check_value_history(context, checker, option, value);
    }

    fn option(
        &mut self,
        context: &mut Context<'_>,
        checker: &Checker,
        option: &OptionData,
        value: &toml::Spanned<DeValue<'_>>,
    ) {
        if option.selectors {
            // `select` and `extend-select` enable rules; the others adjust them.
            let name = option.path.rsplit('.').next().unwrap_or(option.path);
            let enables = matches!(name, "select" | "extend-select");
            for (selector, span) in selectors(value) {
                self.check_selector(context, checker, enables, selector, span);
            }
        }
    }
}

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some(ruff) = get(root, "tool")
        .and_then(|(_, tool)| tool.get_ref().as_table())
        .and_then(|tool| get(tool, "ruff"))
        .and_then(|(_, ruff)| ruff.get_ref().as_table())
    else {
        return;
    };
    let checker = Checker::new(
        context,
        &RUFF,
        root,
        ruff,
        Some("ruff"),
        Some("required-version"),
    );
    // Nothing to say about releases pyprojx does not know.
    if checker.candidates.is_empty() {
        return;
    }
    // `lint.preview` overrides `preview` for the linter.
    let flag = |table: &DeTable<'_>| get(table, "preview").and_then(|(_, v)| v.get_ref().as_bool());
    let lint_preview = get(ruff, "lint")
        .and_then(|(_, lint)| lint.get_ref().as_table())
        .and_then(flag);
    let global_preview = flag(ruff).unwrap_or(false);
    let mut checks = RuffChecks {
        preview: lint_preview.unwrap_or(global_preview),
        global_preview,
        moved: Vec::new(),
    };
    checker.check_table(context, &mut checks, ruff, "");
    report_moved(context, &checks.moved);
    check_target_version(context, root, &checker, ruff);
}

/// Warns when `target-version` is newer than the oldest Python that
/// `requires-python` allows, so that Ruff may suggest code it cannot run.
fn check_target_version(
    context: &mut Context<'_>,
    root: &DeTable<'_>,
    checker: &Checker,
    ruff: &DeTable<'_>,
) {
    let Some((_, target)) = get(ruff, "target-version") else {
        return;
    };
    let Some(minor) = target
        .get_ref()
        .as_str()
        .and_then(|text| text.strip_prefix("py3"))
        .and_then(|minor| minor.parse::<u64>().ok())
    else {
        return;
    };
    check_python_version(
        context,
        root,
        checker,
        ("target-version", target, minor),
        |minor| format!("py3{minor}"),
        "Ruff may suggest code that needs",
    );
}

impl RuffChecks {
    /// Reports a valid value that allowed releases deprecate, or, for
    /// `target-version`, support only in preview.
    fn check_value_history(
        &self,
        context: &mut Context<'_>,
        checker: &Checker,
        option: &OptionData,
        value: &toml::Spanned<DeValue<'_>>,
    ) {
        let Some(text) = value.get_ref().as_str() else {
            return;
        };
        let path = option.path;
        let newest = checker.newest();
        if let Some((since, instead)) = ruff::deprecated_value(path, text, newest) {
            context.report(
                Diagnostic::new(
                    Rule::DeprecatedSetting,
                    format!(
                        "`{path} = \"{text}\"` is deprecated since Ruff {}",
                        checker.release(since)
                    ),
                    value.span(),
                )
                .with_help(instead),
            );
        }
        if path == "target-version"
            && !self.global_preview
            && let Some(&release) = checker
                .candidates
                .iter()
                .rev()
                .find(|&&index| ruff::is_python_in_development(text, index))
        {
            context.report(
                Diagnostic::new(
                    Rule::UnsupportedFeature,
                    format!(
                        "Ruff {} supports `{text}` only in preview",
                        checker.release(release)
                    ),
                    value.span(),
                )
                .with_help("Ruff warns that support is under development; set `preview = true` in `[tool.ruff]`, or target an earlier version")
                .as_warning(),
            );
        }
    }

    /// Checks a rule selector in an option that enables rules if `enables`.
    fn check_selector(
        &self,
        context: &mut Context<'_>,
        checker: &Checker,
        enables: bool,
        text: &str,
        span: Range<usize>,
    ) {
        let newest = checker.newest();
        let data = ruff::selector(text);
        let present = |index: &usize| data.is_some_and(|data| data.is_present(*index));
        let lacking: Vec<usize> = checker
            .candidates
            .iter()
            .copied()
            .filter(|index| !present(index))
            .collect();
        if lacking.is_empty() {
            let data = data.expect("present selectors have data");
            let deprecated = checker
                .candidates
                .iter()
                .filter(|&&index| data.is_deprecated(index))
                .count();
            if enables && deprecated > 0 {
                self.report_deprecated_rule(context, checker, data, span, deprecated);
            } else if data.is_name() && !self.preview {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("selecting rules by name, like `{text}`, needs preview"),
                        span,
                    )
                    .with_help(
                        "use the rule's code, or set `preview = true` in `[tool.ruff.lint]`",
                    ),
                );
            } else if enables && !self.preview && ruff::selects_only_preview_rules(text, newest) {
                context.report(
                    Diagnostic::new(
                        Rule::IneffectiveSetting,
                        format!(
                            "`{text}` selects only preview rules, so it has no effect without preview"
                        ),
                        span,
                    )
                    .with_help("set `preview = true` in `[tool.ruff.lint]` to enable them"),
                );
            }
            return;
        }

        // Ruff accepts old codes it redirects, with a warning.
        if let Some(new) = ruff::redirect(text) {
            context.report(
                Diagnostic::new(
                    Rule::DeprecatedSetting,
                    format!("Ruff remaps the rule code `{text}` to `{new}`"),
                    span,
                )
                .with_help(format!("use `{new}`")),
            );
            return;
        }
        // With preview, recent releases only warn about selectors they do not know.
        let rejected = !(self.preview
            && lacking
                .iter()
                .all(|&index| ruff::is_lenient_with_preview(index)));
        let Some(data) = data.filter(|data| !data.present.is_empty()) else {
            let known = ruff::codes_in(newest);
            let mut diagnostic = Diagnostic::new(
                Rule::InvalidValue,
                format!("unknown rule selector `{text}`"),
                span,
            );
            if let Some(suggestion) = suggest_code(text, &known) {
                diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
            } else if let Some(removed) = data.and_then(|data| data.removed_before(newest)) {
                diagnostic = diagnostic
                    .with_help(format!("Ruff removed it in {}", checker.release(removed)));
            }
            if !rejected {
                diagnostic = diagnostic.as_warning();
            }
            context.report(diagnostic);
            return;
        };

        let first_lacking = lacking[0];
        // Ruff rejects removed rules even with preview.
        let none_rejecting = lacking.len() == checker.candidates.len()
            && (rejected || matches!(data.status, Some(RuleStatus::Removed { .. })));
        // Ruff ignores removed rules in options that do not enable rules.
        if !enables && data.added_after(first_lacking).is_none() {
            let removed = data
                .removed_before(first_lacking)
                .map_or("", |index| checker.release(index));
            context.report(
                Diagnostic::new(
                    Rule::IneffectiveSetting,
                    format!("Ruff removed the rule `{text}` in {removed}, so this has no effect"),
                    span,
                )
                .with_help("remove it"),
            );
            return;
        }
        checker.report_missing(
            context,
            &format!("the rule selector `{text}`"),
            (
                data.added_after(first_lacking),
                data.removed_before(first_lacking),
            ),
            span,
            none_rejecting,
            none_rejecting,
        );
    }

    /// Reports selecting a rule that some allowed releases deprecate, which Ruff
    /// rejects with preview.
    fn report_deprecated_rule(
        &self,
        context: &mut Context<'_>,
        checker: &Checker,
        data: &SelectorData,
        span: Range<usize>,
        deprecated: usize,
    ) {
        let code = data.selector;
        let removal = match data.status {
            Some(RuleStatus::Removed { removed, .. }) => {
                format!("Ruff removed it in {}", checker.release(removed.into()))
            }
            _ => "Ruff will remove it".to_owned(),
        };
        let diagnostic = if self.preview && deprecated == checker.candidates.len() {
            Diagnostic::new(
                Rule::InvalidValue,
                format!("Ruff does not allow selecting the deprecated rule `{code}` with preview"),
                span,
            )
        } else {
            Diagnostic::new(
                Rule::DeprecatedSetting,
                format!("Ruff deprecates the rule `{code}`"),
                span,
            )
        };
        context.report(diagnostic.with_help(format!("remove it; {removal}")));
    }
}

/// The rule selectors in a list, or in each list of a map, with their spans.
fn selectors<'v>(value: &'v toml::Spanned<DeValue<'_>>) -> Vec<(&'v str, Range<usize>)> {
    let lists: Vec<&toml::Spanned<DeValue<'_>>> = match value.get_ref() {
        DeValue::Table(map) => map.values().collect(),
        _ => vec![value],
    };
    lists
        .into_iter()
        .filter_map(|list| list.get_ref().as_array())
        .flatten()
        .filter_map(|entry| entry.get_ref().as_str().map(|text| (text, entry.span())))
        .collect()
}

/// A known rule code or prefix one edit from `text`, preferring the same
/// length, such as `E501` for `E5O1`.
fn suggest_code<'a>(text: &str, known: &[&'a str]) -> Option<&'a str> {
    let upper = text.to_ascii_uppercase();
    known
        .iter()
        .filter(|code| strsim::levenshtein(&upper, code) <= 1)
        .min_by_key(|code| (code.len() != upper.len(), **code))
        .copied()
        .or_else(|| suggest(text, known))
}

/// Reports top-level linter settings, which Ruff deprecated in favor of
/// `[tool.ruff.lint]`, together.
fn report_moved(context: &mut Context<'_>, moved: &[Range<usize>]) {
    let Some((first, others)) = moved.split_first() else {
        return;
    };
    let mut diagnostic = Diagnostic::new(
        Rule::DeprecatedSetting,
        "linter settings at the top level of `[tool.ruff]` are deprecated",
        first.clone(),
    )
    .with_help("move them to `[tool.ruff.lint]`, such as `[tool.ruff.lint] select = [...]`");
    for span in others {
        diagnostic = diagnostic.with_label(span.clone(), "also a linter setting");
    }
    context.report(diagnostic);
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    /// Checks a project, returning (rule, severity, message, spanned text).
    fn check(text: &str) -> Vec<(&'static str, Severity, String, String)> {
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

    #[test]
    fn valid_configuration() {
        let text = "[tool.ruff]\nline-length = 100\n[tool.ruff.lint]\nselect = [\"E\", \"F\"]\n[tool.ruff.lint.per-file-ignores]\n\"tests/*\" = [\"S101\"]\n[tool.ruff.lint.isort]\nknown-first-party = [\"demo\"]\n[tool.ruff.format]\nquote-style = \"single\"\n";
        assert_eq!(check(text), []);
    }

    #[test]
    fn unknown_keys_suggest_options() {
        let found = check(
            "[tool.ruff]\nline-lenght = 88\n[tool.ruff.lint.isort]\nknown-first-parties = []\n",
        );
        assert_eq!(
            found
                .iter()
                .map(|(rule, severity, message, span)| (
                    *rule,
                    *severity,
                    message.as_str(),
                    span.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "unknown-key",
                    Severity::Error,
                    "unknown key `line-lenght` in `[tool.ruff]`",
                    "line-lenght"
                ),
                (
                    "unknown-key",
                    Severity::Error,
                    "unknown key `known-first-parties` in `[tool.ruff.lint.isort]`",
                    "known-first-parties"
                ),
            ]
        );
    }

    #[test]
    fn top_level_linter_settings_are_reported_together() {
        let found = check("[tool.ruff]\nline-length = 88\nselect = [\"E\"]\nignore = [\"E501\"]\n");
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].0, found[0].1, found[0].3.as_str()),
            ("deprecated-setting", Severity::Warning, "select")
        );
    }

    #[test]
    fn deprecated_options() {
        let found = check("[tool.ruff.lint.flake8-builtins]\nbuiltins-ignorelist = [\"id\"]\n");
        assert_eq!(
            found,
            [(
                "deprecated-setting",
                Severity::Warning,
                "`tool.ruff.lint.flake8-builtins.builtins-ignorelist` is deprecated since Ruff 0.10.0".to_owned(),
                "builtins-ignorelist".to_owned()
            )]
        );
    }

    #[test]
    fn options_the_allowed_versions_lack() {
        // `analyze` was added after 0.5.
        let project = |requirement: &str| {
            format!(
                "[dependency-groups]\ndev = [\"{requirement}\"]\n[tool.ruff.analyze]\ndetect-string-imports = true\n"
            )
        };
        let found = check(&project("ruff==0.5.0"));
        assert_eq!(
            (found[0].0, found[0].1),
            ("unsupported-feature", Severity::Error)
        );
        assert_eq!(found[0].3, "analyze");
        let found = check(&project("ruff>=0.5"));
        assert_eq!(
            (found[0].0, found[0].1),
            ("unsupported-feature", Severity::Warning)
        );
        assert!(
            check(&project("ruff>=0.9"))
                .iter()
                .all(|d| d.0 != "unsupported-feature")
        );
        // An unpinned requirement resolves to the newest release.
        assert_eq!(check(&project("ruff")), []);
    }

    #[test]
    fn required_version_narrows_the_versions() {
        let text = "[dependency-groups]\ndev = [\"ruff>=0.5\"]\n[tool.ruff]\nrequired-version = \">=0.9\"\n[tool.ruff.analyze]\ndetect-string-imports = true\n";
        assert_eq!(check(text), []);
        let found = check("[tool.ruff]\nrequired-version = \"0.5.0\"\n[tool.ruff.analyze]\n");
        assert_eq!(found[0].1, Severity::Error);
        let found = check("[tool.ruff]\nrequired-version = \">=0.5,\"\n");
        assert_eq!(found[0].0, "invalid-value");
        // Versions pyprojx does not know are not judged.
        assert_eq!(
            check("[tool.ruff]\nrequired-version = \">=99\"\nline-lenght = 1\n"),
            []
        );
    }

    /// Checks `[tool.ruff.lint]` settings with Ruff pinned to `version`,
    /// returning (rule, severity, spanned text).
    fn lint(version: &str, body: &str) -> Vec<(&'static str, Severity, String)> {
        let text =
            format!("[tool.ruff]\nrequired-version = \"=={version}\"\n[tool.ruff.lint]\n{body}");
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| (d.rule.name(), d.severity(), text[d.span].to_owned()))
            .collect()
    }

    fn one(
        rule: &'static str,
        severity: Severity,
        span: &str,
    ) -> Vec<(&'static str, Severity, String)> {
        vec![(rule, severity, span.to_owned())]
    }

    #[test]
    fn known_and_unknown_selectors() {
        let body = "select = [\"E\", \"E5\", \"E501\", \"ALL\"]\nper-file-ignores = { \"tests/*\" = [\"S101\"] }\n";
        assert_eq!(lint("0.16.8", body), []);
        assert_eq!(
            lint("0.16.8", "select = [\"XYZ123\"]\n"),
            one("invalid-value", Severity::Error, "\"XYZ123\"")
        );
        assert_eq!(
            lint("0.16.8", "per-file-ignores = { \"x.py\" = [\"XYZ1\"] }\n"),
            one("invalid-value", Severity::Error, "\"XYZ1\"")
        );
        // With preview, recent releases only warn.
        assert_eq!(
            lint("0.16.8", "select = [\"XYZ123\"]\npreview = true\n"),
            one("invalid-value", Severity::Warning, "\"XYZ123\"")
        );
        assert_eq!(
            lint("0.15.0", "select = [\"XYZ123\"]\npreview = true\n"),
            one("invalid-value", Severity::Error, "\"XYZ123\"")
        );
    }

    #[test]
    fn removed_deprecated_and_redirected_rules() {
        let select = "select = [\"ANN101\"]\n";
        assert_eq!(
            lint("0.16.8", select),
            one("unsupported-feature", Severity::Error, "\"ANN101\"")
        );
        assert_eq!(
            lint("0.16.8", "ignore = [\"ANN101\"]\n"),
            one("ineffective-setting", Severity::Warning, "\"ANN101\"")
        );
        assert_eq!(
            lint("0.5.0", select),
            one("deprecated-setting", Severity::Warning, "\"ANN101\"")
        );
        assert_eq!(
            lint("0.5.0", "select = [\"ANN101\"]\npreview = true\n"),
            one("invalid-value", Severity::Error, "\"ANN101\"")
        );
        assert_eq!(lint("0.1.0", select), []);
        assert_eq!(
            lint("0.16.8", "select = [\"PLR1701\"]\n"),
            one("deprecated-setting", Severity::Warning, "\"PLR1701\"")
        );
    }

    #[test]
    fn preview_rules_and_rule_names() {
        assert_eq!(
            lint("0.16.8", "select = [\"AIR003\"]\n"),
            one("ineffective-setting", Severity::Warning, "\"AIR003\"")
        );
        assert_eq!(
            lint("0.16.8", "select = [\"AIR003\"]\npreview = true\n"),
            []
        );
        // Ignoring a preview rule is fine.
        assert_eq!(lint("0.16.8", "ignore = [\"AIR003\"]\n"), []);
        let name = "select = [\"too-many-positional-arguments\"]\n";
        assert_eq!(
            lint("0.16.8", name),
            one(
                "invalid-value",
                Severity::Error,
                "\"too-many-positional-arguments\""
            )
        );
        assert_eq!(lint("0.16.8", &format!("{name}preview = true\n")), []);
        // Top-level `preview` applies to the linter too.
        let text = "[tool.ruff]\nrequired-version = \"==0.16.8\"\npreview = true\n[tool.ruff.lint]\nselect = [\"AIR003\"]\n";
        assert_eq!(check(text), []);
    }

    #[test]
    fn selectors_the_allowed_versions_lack() {
        // FAST was added in 0.8.
        let found = lint("0.5.0", "select = [\"FAST\"]\n");
        assert_eq!(
            found,
            one("unsupported-feature", Severity::Error, "\"FAST\"")
        );
    }

    /// Checks `[tool.ruff]` settings with Ruff pinned to `version`.
    fn ruff(version: &str, body: &str) -> Vec<(&'static str, Severity, String)> {
        let text = format!("[tool.ruff]\nrequired-version = \"=={version}\"\n{body}");
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| (d.rule.name(), d.severity(), text[d.span].to_owned()))
            .collect()
    }

    #[test]
    fn value_types() {
        let valid = "line-length = 400\nfix = true\noutput-format = \"concise\"\n[tool.ruff.format]\nquote-style = \"preserve\"\n[tool.ruff.lint.isort]\nsections = { testing = [\"pytest\"] }\n";
        assert_eq!(ruff("0.16.8", valid), []);
        for (body, span) in [
            ("line-length = 0\n", "0"),
            ("line-length = \"88\"\n", "\"88\""),
            ("fix = \"yes\"\n", "\"yes\""),
            ("extend-exclude = \"x\"\n", "\"x\""),
            ("target-version = \"py399\"\n", "\"py399\""),
            ("[tool.ruff.lint.pylint]\nmax-args = -1\n", "-1"),
            (
                "[tool.ruff.format]\nquote-style = \"Single\"\n",
                "\"Single\"",
            ),
        ] {
            assert_eq!(
                ruff("0.16.8", body),
                one("invalid-value", Severity::Error, span),
                "{body}"
            );
        }
        assert_eq!(
            ruff("0.16.8", "lint = 1\n"),
            one("invalid-type", Severity::Error, "1")
        );
    }

    #[test]
    fn values_that_change_between_releases() {
        let py314 = "target-version = \"py314\"\n";
        assert_eq!(
            ruff("0.11.0", py314),
            one("invalid-value", Severity::Error, "\"py314\"")
        );
        // Ruff 0.12 supports Python 3.14 only in preview.
        assert_eq!(
            ruff("0.12.0", py314),
            one("unsupported-feature", Severity::Warning, "\"py314\"")
        );
        assert_eq!(ruff("0.12.0", &format!("preview = true\n{py314}")), []);
        assert_eq!(ruff("0.16.8", py314), []);

        let text = "output-format = \"text\"\n";
        assert_eq!(ruff("0.1.0", text), []);
        assert_eq!(
            ruff("0.3.0", text),
            one("deprecated-setting", Severity::Warning, "\"text\"")
        );
        assert_eq!(
            ruff("0.5.0", text),
            one("invalid-value", Severity::Error, "\"text\"")
        );

        // Old spellings stay accepted after a change of case.
        assert_eq!(
            ruff(
                "0.16.8",
                "[tool.ruff.analyze]\ndirection = \"Dependencies\"\n"
            ),
            []
        );
    }

    #[test]
    fn target_version_newer_than_requires_python() {
        let project = |target: &str| {
            format!(
                "[project]\nname = \"demo\"\nversion = \"1\"\nrequires-python = \">=3.10\"\n[tool.ruff]\ntarget-version = \"{target}\"\n"
            )
        };
        assert_eq!(check(&project("py310")), []);
        assert_eq!(check(&project("py39")), []);
        let found = check(&project("py312"));
        assert_eq!(
            (found[0].0, found[0].1, found[0].2.as_str()),
            (
                "invalid-value",
                Severity::Warning,
                "`target-version` is Python 3.12, but `requires-python` allows Python 3.10"
            )
        );
    }
}
