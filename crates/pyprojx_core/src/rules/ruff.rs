//! `[tool.ruff]`, checked against the Ruff releases the project allows.
//!
//! The allowed releases come from `ruff` requirements in the project's
//! dependencies, extras, and dependency groups, any of which might be used,
//! narrowed by `required-version`, which Ruff enforces. Without either, the
//! latest release is assumed.

use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::{Context, suggest};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::ruff::{self, OptionData, OptionKind, RuleStatus, SelectorData};
use crate::standards::{VersionSet, allowed_versions, required_versions};

/// The known Ruff releases the project allows, and whether it enables preview.
struct Target {
    /// Indexes into [`ruff::releases`], oldest first.
    candidates: Vec<usize>,
    /// Where the allowed versions are set.
    source: Option<Range<usize>>,
    preview: bool,
}

impl Target {
    fn newest(&self) -> usize {
        *self
            .candidates
            .last()
            .expect("checked only with candidates")
    }

    fn release(index: usize) -> &'static str {
        ruff::releases()[index]
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
    let target = target(context, root, ruff);
    // Nothing to say about releases pyprojx does not know.
    if target.candidates.is_empty() {
        return;
    }
    let mut moved = Vec::new();
    check_table(context, &target, ruff, "", &mut moved);
    report_moved(context, &moved);
}

/// Finds the Ruff releases the project allows, reporting an invalid
/// `required-version`, and whether it enables preview.
fn target(context: &mut Context<'_>, root: &DeTable<'_>, ruff: &DeTable<'_>) -> Target {
    let releases = ruff::releases();
    let latest = releases.len() - 1;
    let mut candidates: Option<Vec<usize>> = None;
    let mut source = None;
    for (versions, span) in ruff_requirements(root) {
        source.get_or_insert(span);
        let candidates = candidates.get_or_insert_default();
        match versions {
            // Installers pick the newest release.
            None => candidates.push(latest),
            Some(versions) => candidates
                .extend((0..releases.len()).filter(|&index| versions.contains(releases[index]))),
        }
    }

    if let Some((_, required)) = get(ruff, "required-version")
        && let Some(text) = required.get_ref().as_str()
    {
        match allowed_versions(text) {
            Ok(versions) => {
                // Without requirements, any release might run, but Ruff refuses
                // to run unless it matches.
                let allowed = candidates.get_or_insert_with(|| (0..releases.len()).collect());
                allowed.retain(|&index| versions.contains(releases[index]));
                source = Some(required.span());
            }
            Err(problem) => {
                let message = format!("invalid `tool.ruff.required-version`: {}", problem.message);
                context.invalid_string(message, required.span(), &problem);
            }
        }
    }
    let mut candidates = candidates.unwrap_or_else(|| vec![latest]);
    candidates.sort_unstable();
    candidates.dedup();
    // `lint.preview` overrides `preview` for the linter.
    let flag = |table: &DeTable<'_>| get(table, "preview").and_then(|(_, v)| v.get_ref().as_bool());
    let lint_preview = get(ruff, "lint")
        .and_then(|(_, lint)| lint.get_ref().as_table())
        .and_then(flag);
    let preview = lint_preview.or_else(|| flag(ruff)).unwrap_or(false);
    Target {
        candidates,
        source,
        preview,
    }
}

/// The versions allowed by each `ruff` requirement of the project (`None` for
/// no version specifiers), with the requirement's span.
fn ruff_requirements(root: &DeTable<'_>) -> Vec<(Option<VersionSet>, Range<usize>)> {
    let mut lists = Vec::new();
    if let Some((_, project)) = get(root, "project")
        && let Some(project) = project.get_ref().as_table()
    {
        if let Some((_, dependencies)) = get(project, "dependencies") {
            lists.push(dependencies);
        }
        if let Some((_, extras)) = get(project, "optional-dependencies")
            && let Some(extras) = extras.get_ref().as_table()
        {
            lists.extend(extras.values());
        }
    }
    if let Some((_, groups)) = get(root, "dependency-groups")
        && let Some(groups) = groups.get_ref().as_table()
    {
        lists.extend(groups.values());
    }

    let mut found = Vec::new();
    for list in lists {
        let Some(entries) = list.get_ref().as_array() else {
            continue;
        };
        for entry in entries {
            if let Some(text) = entry.get_ref().as_str()
                && let Some((name, versions)) = required_versions(text)
                && name == "ruff"
            {
                found.push((versions, entry.span()));
            }
        }
    }
    found
}

/// Checks the options in the table at `prefix`, such as `lint.`, collecting
/// top-level linter settings that belong in `[tool.ruff.lint]`.
fn check_table(
    context: &mut Context<'_>,
    target: &Target,
    table: &DeTable<'_>,
    prefix: &str,
    moved: &mut Vec<Range<usize>>,
) {
    for (key, value) in table {
        let name = key.get_ref().as_ref();
        let path = format!("{prefix}{name}");
        let Some(option) = ruff::option(&path) else {
            report_unknown(context, target, prefix, name, key.span());
            continue;
        };
        let supported = target
            .candidates
            .iter()
            .filter(|&&index| option.is_present(index))
            .count();
        if supported < target.candidates.len() {
            report_unsupported(context, target, option, key.span(), supported == 0);
            if supported == 0 {
                continue;
            }
        }

        let newest = target.newest();
        if option.is_deprecated(newest) {
            if prefix.is_empty() && ruff::option(&format!("lint.{name}")).is_some() {
                moved.push(key.span());
            } else {
                report_deprecated(context, option, newest, key.span());
            }
        }
        if option.selectors {
            // `select` and `extend-select` enable rules; the others adjust them.
            let enables = matches!(name, "select" | "extend-select");
            for (selector, span) in selectors(value) {
                check_selector(context, target, enables, selector, span);
            }
        }
        if option.kind == OptionKind::Table
            && let DeValue::Table(table) = value.get_ref()
        {
            check_table(context, target, table, &format!("{path}."), moved);
        }
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

/// Checks a rule selector in an option that enables rules if `enables`.
fn check_selector(
    context: &mut Context<'_>,
    target: &Target,
    enables: bool,
    text: &str,
    span: Range<usize>,
) {
    let newest = target.newest();
    let data = ruff::selector(text);
    let present = |index: &usize| data.is_some_and(|data| data.is_present(*index));
    let lacking: Vec<usize> = target
        .candidates
        .iter()
        .copied()
        .filter(|index| !present(index))
        .collect();
    if lacking.is_empty() {
        let data = data.expect("present selectors have data");
        let deprecated = target
            .candidates
            .iter()
            .filter(|&&index| data.is_deprecated(index))
            .count();
        if enables && deprecated > 0 {
            report_deprecated_rule(context, target, data, span, deprecated);
        } else if data.is_name() && !target.preview {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("selecting rules by name, like `{text}`, needs preview"),
                    span,
                )
                .with_help("use the rule's code, or set `preview = true` in `[tool.ruff.lint]`"),
            );
        } else if enables && !target.preview && ruff::selects_only_preview_rules(text, newest) {
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
    let rejected = !(target.preview
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
            diagnostic =
                diagnostic.with_help(format!("Ruff removed it in {}", Target::release(removed)));
        }
        if !rejected {
            diagnostic = diagnostic.as_warning();
        }
        context.report(diagnostic);
        return;
    };

    let first_lacking = lacking[0];
    // Ruff ignores removed rules in options that do not enable rules.
    if !enables && data.added_after(first_lacking).is_none() {
        let removed = data
            .removed_before(first_lacking)
            .map_or("", Target::release);
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
    report_missing(
        context,
        target,
        &format!("the rule selector `{text}`"),
        (
            data.added_after(first_lacking),
            data.removed_before(first_lacking),
        ),
        span,
        // Ruff rejects removed rules even with preview.
        lacking.len() == target.candidates.len()
            && (rejected || matches!(data.status, Some(RuleStatus::Removed { .. }))),
    );
}

/// Reports selecting a rule that some allowed releases deprecate, which Ruff
/// rejects with preview.
fn report_deprecated_rule(
    context: &mut Context<'_>,
    target: &Target,
    data: &SelectorData,
    span: Range<usize>,
    deprecated: usize,
) {
    let code = data.selector;
    let removal = match data.status {
        Some(RuleStatus::Removed { removed, .. }) => {
            format!("Ruff removed it in {}", Target::release(removed.into()))
        }
        _ => "Ruff will remove it".to_owned(),
    };
    let diagnostic = if target.preview && deprecated == target.candidates.len() {
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

fn table_name(prefix: &str) -> String {
    match prefix.strip_suffix('.') {
        Some(table) => format!("`[tool.ruff.{table}]`"),
        None => "`[tool.ruff]`".to_owned(),
    }
}

/// Reports a key that no known release accepts.
fn report_unknown(
    context: &mut Context<'_>,
    target: &Target,
    prefix: &str,
    name: &str,
    span: Range<usize>,
) {
    let known = ruff::names_in(prefix, target.newest());
    let help = match suggest(name, &known) {
        Some(suggestion) => format!("did you mean `{suggestion}`?"),
        None => format!(
            "no Ruff release from {} to {} has this option; see https://docs.astral.sh/ruff/settings/",
            Target::release(0),
            Target::release(ruff::releases().len() - 1)
        ),
    };
    context.report(
        Diagnostic::new(
            Rule::UnknownKey,
            format!("unknown key `{name}` in {}", table_name(prefix)),
            span,
        )
        .with_help(help),
    );
}

/// Reports an option that some or all allowed releases lack.
fn report_unsupported(
    context: &mut Context<'_>,
    target: &Target,
    option: &OptionData,
    span: Range<usize>,
    none_supported: bool,
) {
    let lacking = *target
        .candidates
        .iter()
        .find(|&&index| !option.is_present(index))
        .expect("some candidate lacks the option");
    report_missing(
        context,
        target,
        &format!("`tool.ruff.{}`", option.path),
        (option.added_after(lacking), option.removed_before(lacking)),
        span,
        none_supported,
    );
}

/// Reports something that some or all allowed releases lack, given when Ruff
/// added or removed it relative to one of those releases.
fn report_missing(
    context: &mut Context<'_>,
    target: &Target,
    subject: &str,
    (added, removed): (Option<usize>, Option<usize>),
    span: Range<usize>,
    none_supported: bool,
) {
    let help = match (added, removed) {
        (Some(added), _) => {
            let added = Target::release(added);
            format!("Ruff added it in {added}; require `ruff>={added}`")
        }
        (None, removed) => format!("Ruff removed it in {}", removed.map_or("", Target::release)),
    };
    let (message, label) = if none_supported {
        (
            format!("no Ruff version the project allows supports {subject}"),
            "allows only versions without it",
        )
    } else {
        (
            format!("some Ruff versions the project allows do not support {subject}"),
            "allows versions without it",
        )
    };
    let mut diagnostic = Diagnostic::new(Rule::UnsupportedFeature, message, span).with_help(help);
    if let Some(source) = &target.source {
        diagnostic = diagnostic.with_label(source.clone(), label);
    }
    // Ruff rejects what it does not know.
    if !none_supported {
        diagnostic = diagnostic.as_warning();
    }
    context.report(diagnostic);
}

fn report_deprecated(
    context: &mut Context<'_>,
    option: &OptionData,
    newest: usize,
    span: Range<usize>,
) {
    let since = option.deprecated_since(newest).map_or("", Target::release);
    let mut diagnostic = Diagnostic::new(
        Rule::DeprecatedSetting,
        format!(
            "`tool.ruff.{}` is deprecated since Ruff {since}",
            option.path
        ),
        span,
    );
    if let Some(message) = option.message {
        diagnostic = diagnostic.with_help(message);
    }
    context.report(diagnostic);
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
}
