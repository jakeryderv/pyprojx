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
use crate::ruff::{self, OptionData, OptionKind};
use crate::standards::{VersionSet, allowed_versions, required_versions};

/// The known Ruff releases the project allows.
struct Versions {
    /// Indexes into [`ruff::releases`], oldest first.
    candidates: Vec<usize>,
    /// Where the allowed versions are set.
    source: Option<Range<usize>>,
}

impl Versions {
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
    let versions = versions(context, root, ruff);
    // Nothing to say about releases pyprojx does not know.
    if versions.candidates.is_empty() {
        return;
    }
    let mut moved = Vec::new();
    check_table(context, &versions, ruff, "", &mut moved);
    report_moved(context, &moved);
}

/// Finds the Ruff releases the project allows, reporting an invalid
/// `required-version`.
fn versions(context: &mut Context<'_>, root: &DeTable<'_>, ruff: &DeTable<'_>) -> Versions {
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
    Versions { candidates, source }
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
    versions: &Versions,
    table: &DeTable<'_>,
    prefix: &str,
    moved: &mut Vec<Range<usize>>,
) {
    for (key, value) in table {
        let name = key.get_ref().as_ref();
        let path = format!("{prefix}{name}");
        let Some(option) = ruff::option(&path) else {
            report_unknown(context, versions, prefix, name, key.span());
            continue;
        };
        let supported = versions
            .candidates
            .iter()
            .filter(|&&index| option.is_present(index))
            .count();
        if supported < versions.candidates.len() {
            report_unsupported(context, versions, option, key.span(), supported == 0);
            if supported == 0 {
                continue;
            }
        }

        let newest = versions.newest();
        if option.is_deprecated(newest) {
            if prefix.is_empty() && ruff::option(&format!("lint.{name}")).is_some() {
                moved.push(key.span());
            } else {
                report_deprecated(context, option, newest, key.span());
            }
        }
        if option.kind == OptionKind::Table
            && let DeValue::Table(table) = value.get_ref()
        {
            check_table(context, versions, table, &format!("{path}."), moved);
        }
    }
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
    versions: &Versions,
    prefix: &str,
    name: &str,
    span: Range<usize>,
) {
    let known = ruff::names_in(prefix, versions.newest());
    let help = match suggest(name, &known) {
        Some(suggestion) => format!("did you mean `{suggestion}`?"),
        None => format!(
            "no Ruff release from {} to {} has this option; see https://docs.astral.sh/ruff/settings/",
            Versions::release(0),
            Versions::release(ruff::releases().len() - 1)
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
    versions: &Versions,
    option: &OptionData,
    span: Range<usize>,
    none_supported: bool,
) {
    let path = option.path;
    let lacking = *versions
        .candidates
        .iter()
        .find(|&&index| !option.is_present(index))
        .expect("some candidate lacks the option");
    let (history, requirement) = match option.added_after(lacking) {
        Some(added) => {
            let added = Versions::release(added);
            (
                format!("Ruff added it in {added}"),
                Some(format!("require `ruff>={added}`")),
            )
        }
        None => {
            let removed = option.removed_before(lacking).map_or("", Versions::release);
            (format!("Ruff removed it in {removed}"), None)
        }
    };
    let (message, label) = if none_supported {
        (
            format!("no Ruff version the project allows supports `tool.ruff.{path}`"),
            "allows only versions without it",
        )
    } else {
        (
            format!("some Ruff versions the project allows do not support `tool.ruff.{path}`"),
            "allows versions without it",
        )
    };
    let help = match requirement {
        Some(requirement) => format!("{history}; {requirement}"),
        None => history,
    };
    let mut diagnostic = Diagnostic::new(Rule::UnsupportedFeature, message, span).with_help(help);
    if let Some(source) = &versions.source {
        diagnostic = diagnostic.with_label(source.clone(), label);
    }
    // Ruff rejects unknown options.
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
    let since = option
        .deprecated_since(newest)
        .map_or("", Versions::release);
    let mut diagnostic = Diagnostic::new(
        Rule::DeprecatedOption,
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
        Rule::DeprecatedOption,
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
            ("deprecated-option", Severity::Warning, "select")
        );
    }

    #[test]
    fn deprecated_options() {
        let found = check("[tool.ruff.lint.flake8-builtins]\nbuiltins-ignorelist = [\"id\"]\n");
        assert_eq!(
            found,
            [(
                "deprecated-option",
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
}
