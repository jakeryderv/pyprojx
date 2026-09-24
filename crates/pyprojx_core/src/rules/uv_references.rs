//! Names that `[tool.uv]` refers to: the indexes, extras, groups, and packages
//! that sources, `default-groups`, `conflicts`, and group settings name, and
//! the shape of each source.
//!
//! Each check follows what uv does, found by running it: uv fails on sources
//! of the wrong shape or naming an undeclared index, extra, or group, on
//! duplicate or invalid index names, and, in `uv sync`, on default groups that
//! do not exist. It silently ignores conflicts naming extras or groups that do
//! not exist and sources for packages the project does not depend on, which
//! pyprojx reports as warnings. Checks of options the newest allowed release
//! lacks are left out, as are errors that only some allowed releases make.

use std::collections::BTreeSet;
use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::tool::Checker;
use super::{Context, quoted_list, suggest};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::standards::{normalize_name, requirement_name};
use crate::uv::{
    DUPLICATE_INDEX_NAMES_REJECTED_SINCE, MULTIPLE_DEFAULT_INDEXES_REJECTED_SINCE, UV, source_kinds,
};

type Spanned<'a> = toml::Spanned<DeValue<'a>>;

/// The names in the project that `[tool.uv]` can refer to, normalized. `None`
/// where the project leaves them to its build backend (`dynamic`).
struct Names {
    dependencies: Option<BTreeSet<String>>,
    extras: Option<BTreeSet<String>>,
    groups: BTreeSet<String>,
    indexes: BTreeSet<String>,
}

pub(super) fn check(
    context: &mut Context<'_>,
    checker: &Checker,
    root: &DeTable<'_>,
    uv: &DeTable<'_>,
) {
    let names = names(root, uv);
    // Options the newest allowed release lacks are reported as such.
    let newest = checker.newest();
    let get = |key: &str| {
        get(uv, key).filter(|_| {
            UV.option(key)
                .is_some_and(|option| option.is_present(newest))
        })
    };
    if let Some((_, sources)) = get("sources")
        && let Some(sources) = sources.get_ref().as_table()
    {
        // A workspace root's sources also apply to its members' dependencies.
        let workspace = crate::document::get(uv, "workspace").is_some();
        for (package, entries) in sources {
            check_source(
                context,
                &names,
                (package.get_ref(), package.span()),
                entries,
                workspace,
            );
        }
    }
    if let Some((_, groups)) = get("default-groups") {
        check_default_groups(context, &names, groups);
    }
    if let Some((_, settings)) = get("dependency-groups")
        && let Some(settings) = settings.get_ref().as_table()
    {
        for group in settings.keys() {
            if !is_known(&names.groups, group.get_ref()) {
                context.report(undefined(
                    "dependency group",
                    group.get_ref(),
                    group.span(),
                    &names.groups,
                    "uv fails when a group it has settings for does not exist",
                ));
            }
        }
    }
    if let Some((_, conflicts)) = get("conflicts") {
        check_conflicts(context, &names, conflicts);
    }
    if let Some((_, indexes)) = get("index") {
        check_indexes(context, checker, indexes);
    }
}

/// Makes `diagnostic` an error if every allowed release rejects what it
/// reports, as releases from `since` do, and otherwise a warning that says so.
fn rejected_since(checker: &Checker, since: &str, diagnostic: Diagnostic) -> Diagnostic {
    let first = checker
        .tool
        .releases
        .iter()
        .position(|release| *release == since);
    let all = first.is_some_and(|first| checker.candidates.iter().all(|&index| index >= first));
    let diagnostic = diagnostic.with_help(format!("uv rejects it from {since}"));
    if all {
        diagnostic
    } else {
        diagnostic.as_warning()
    }
}

fn names(root: &DeTable<'_>, uv: &DeTable<'_>) -> Names {
    let project = get(root, "project").and_then(|(_, project)| project.get_ref().as_table());
    let dynamic: Vec<&str> = project
        .and_then(|project| get(project, "dynamic"))
        .and_then(|(_, dynamic)| dynamic.get_ref().as_array())
        .map(|fields| fields.iter().filter_map(|f| f.get_ref().as_str()).collect())
        .unwrap_or_default();
    let groups = get(root, "dependency-groups").and_then(|(_, groups)| groups.get_ref().as_table());
    let extras = project
        .and_then(|project| get(project, "optional-dependencies"))
        .and_then(|(_, extras)| extras.get_ref().as_table());
    let dev_dependencies = get(uv, "dev-dependencies");

    let mut lists: Vec<&Spanned<'_>> = Vec::new();
    lists.extend(
        project
            .and_then(|project| get(project, "dependencies"))
            .map(|(_, list)| list),
    );
    lists.extend(extras.into_iter().flat_map(|extras| extras.values()));
    lists.extend(groups.into_iter().flat_map(|groups| groups.values()));
    lists.extend(dev_dependencies.map(|(_, list)| list));
    let dependencies = lists
        .iter()
        .filter_map(|list| list.get_ref().as_array())
        .flatten()
        .filter_map(|entry| entry.get_ref().as_str())
        .filter_map(requirement_name)
        .collect();

    let keys = |table: Option<&DeTable<'_>>| -> BTreeSet<String> {
        table
            .into_iter()
            .flat_map(|table| table.keys())
            .filter_map(|key| normalize_name(key.get_ref()).ok())
            .collect()
    };
    let mut group_names = keys(groups);
    // uv adds `tool.uv.dev-dependencies` to the `dev` group.
    if dev_dependencies.is_some() {
        group_names.insert("dev".to_owned());
    }
    let indexes = get(uv, "index")
        .and_then(|(_, indexes)| indexes.get_ref().as_array())
        .into_iter()
        .flatten()
        .filter_map(|index| index.get_ref().as_table())
        .filter_map(|index| get(index, "name"))
        .filter_map(|(_, name)| name.get_ref().as_str().map(str::to_owned))
        .collect();
    let has_dependencies = project.is_some() && !dynamic.contains(&"dependencies");
    let has_extras = project.is_some() && !dynamic.contains(&"optional-dependencies");
    Names {
        dependencies: (has_dependencies && !dynamic.contains(&"optional-dependencies"))
            .then_some(dependencies),
        extras: has_extras.then(|| keys(extras)),
        groups: group_names,
        indexes,
    }
}

fn is_known(names: &BTreeSet<String>, name: &str) -> bool {
    normalize_name(name).is_ok_and(|name| names.contains(&name))
}

/// A diagnostic for a reference to a `kind`, such as "extra", named `name`,
/// which the project does not define.
fn undefined(
    kind: &str,
    name: &str,
    span: Range<usize>,
    known: &BTreeSet<String>,
    consequence: &str,
) -> Diagnostic {
    let candidates: Vec<&str> = known.iter().map(String::as_str).collect();
    let help = match suggest(name, &candidates) {
        Some(suggestion) => format!("did you mean `{suggestion}`? {consequence}"),
        None => consequence.to_owned(),
    };
    Diagnostic::new(
        Rule::InvalidValue,
        format!("undefined {kind} `{name}`"),
        span,
    )
    .with_help(help)
}

/// Names in backticks joined with "and", such as "`tag` and `branch`".
fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("`{only}`"),
        [first, second] => format!("`{first}` and `{second}`"),
        [rest @ .., last] => format!("{}, and `{last}`", quoted_list(rest)),
    }
}

/// Checks the source, or list of sources, for `package`.
fn check_source(
    context: &mut Context<'_>,
    names: &Names,
    (package, span): (&str, Range<usize>),
    entries: &Spanned<'_>,
    workspace: bool,
) {
    if !workspace
        && let Some(dependencies) = &names.dependencies
        && !is_known(dependencies, package)
    {
        context.report(
            Diagnostic::new(
                Rule::IneffectiveSetting,
                format!("`{package}` is not a dependency of the project, so uv ignores its source"),
                span,
            )
            .with_help("add it to the project's dependencies, or remove the source"),
        );
    }
    let tables: Vec<&Spanned<'_>> = match entries.get_ref() {
        DeValue::Array(entries) => entries.iter().collect(),
        _ => vec![entries],
    };
    for entry in tables {
        let Some(source) = entry.get_ref().as_table() else {
            // The value's type is checked with the other options.
            continue;
        };
        check_source_shape(context, package, source, entry.span());
        for (key, kind, known, consequence) in [
            (
                "index",
                "index",
                Some(&names.indexes),
                "uv fails when a source names an index that `[[tool.uv.index]]` does not declare",
            ),
            (
                "extra",
                "extra",
                names.extras.as_ref(),
                "uv fails when a source is for an extra that does not exist",
            ),
            (
                "group",
                "dependency group",
                Some(&names.groups),
                "uv fails when a source is for a group that does not exist",
            ),
        ] {
            let Some(known) = known else {
                continue;
            };
            if let Some((_, value)) = get(source, key)
                && let Some(name) = value.get_ref().as_str()
            {
                let defined = if key == "index" {
                    known.contains(name)
                } else {
                    is_known(known, name)
                };
                if !defined {
                    context.report(undefined(kind, name, value.span(), known, consequence));
                }
            }
        }
    }
}

/// Checks a source's keys against the kinds of sources uv knows.
fn check_source_shape(
    context: &mut Context<'_>,
    package: &str,
    source: &DeTable<'_>,
    span: Range<usize>,
) {
    let keys: Vec<(&str, Range<usize>)> = source
        .keys()
        .map(|key| (key.get_ref().as_ref(), key.span()))
        .collect();
    let kinds: Vec<&(&str, &[&str])> = source_kinds()
        .iter()
        .filter(|(required, _)| keys.iter().any(|(key, _)| key == required))
        .collect();
    let fits = |allowed: &[&str]| keys.iter().all(|(key, _)| allowed.contains(key));
    if kinds.iter().any(|(_, allowed)| fits(allowed)) {
        let chosen: Vec<&str> = ["rev", "tag", "branch"]
            .into_iter()
            .filter(|name| get(source, name).is_some())
            .collect();
        if chosen.len() > 1 {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("the Git source for `{package}` sets {}", and_list(&chosen)),
                    span,
                )
                .with_help("set only one of `rev`, `tag`, and `branch`"),
            );
        }
        return;
    }
    let diagnostic = match kinds.as_slice() {
        [] => {
            let required: Vec<&str> = source_kinds().iter().map(|(key, _)| *key).collect();
            Diagnostic::new(
                Rule::InvalidValue,
                format!("the source for `{package}` does not say where the package comes from"),
                span,
            )
            .with_help(format!("set one of {}", quoted_list(&required)))
        }
        [(kind, allowed)] => {
            for (key, key_span) in keys.iter().filter(|(key, _)| !allowed.contains(key)) {
                let help = match suggest(key, allowed) {
                    Some(suggestion) => format!("did you mean `{suggestion}`?"),
                    None => format!("a `{kind}` source allows {}", quoted_list(allowed)),
                };
                context.report(
                    Diagnostic::new(
                        Rule::UnknownKey,
                        format!("unknown key `{key}` in the source for `{package}`"),
                        key_span.clone(),
                    )
                    .with_help(help),
                );
            }
            return;
        }
        several => {
            let found: Vec<&str> = several.iter().map(|(key, _)| *key).collect();
            Diagnostic::new(
                Rule::InvalidValue,
                format!("the source for `{package}` combines {}", and_list(&found)),
                span,
            )
            .with_help("a source is one of a Git repository, a URL, a path, an index, or a workspace member")
        }
    };
    context.report(diagnostic);
}

/// Checks that `default-groups` names groups that exist, which `uv sync`
/// requires.
fn check_default_groups(context: &mut Context<'_>, names: &Names, groups: &Spanned<'_>) {
    let Some(groups) = groups.get_ref().as_array() else {
        return;
    };
    for group in groups {
        if let Some(name) = group.get_ref().as_str()
            && !is_known(&names.groups, name)
        {
            context.report(undefined(
                "dependency group",
                name,
                group.span(),
                &names.groups,
                "`uv sync` fails when a default group does not exist",
            ));
        }
    }
}

/// Checks that each set of conflicts has at least two items, which uv
/// requires, and names extras and groups that exist.
fn check_conflicts(context: &mut Context<'_>, names: &Names, conflicts: &Spanned<'_>) {
    let Some(sets) = conflicts.get_ref().as_array() else {
        return;
    };
    for set in sets {
        let Some(items) = set.get_ref().as_array() else {
            continue;
        };
        if items.len() < 2 {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    "a set of conflicts needs at least two items",
                    set.span(),
                )
                .with_help("list the extras or groups that conflict with each other"),
            );
        }
        for item in items {
            let Some(item) = item.get_ref().as_table() else {
                continue;
            };
            // Items for other workspace members name their extras and groups.
            if get(item, "package").is_some() {
                continue;
            }
            for (key, kind, known) in [
                ("extra", "extra", names.extras.as_ref()),
                ("group", "dependency group", Some(&names.groups)),
            ] {
                if let Some(known) = known
                    && let Some((_, value)) = get(item, key)
                    && let Some(name) = value.get_ref().as_str()
                    && !is_known(known, name)
                {
                    context.report(
                        undefined(kind, name, value.span(), known, "uv ignores it").as_warning(),
                    );
                }
            }
        }
    }
}

/// Checks index names, which uv requires to be unique and made of ASCII
/// letters, digits, `-`, `_`, and `.`, and that at most one index is the
/// default.
fn check_indexes(context: &mut Context<'_>, checker: &Checker, indexes: &Spanned<'_>) {
    let Some(indexes) = indexes.get_ref().as_array() else {
        return;
    };
    let mut seen: Vec<(&str, Range<usize>)> = Vec::new();
    let mut default: Option<Range<usize>> = None;
    for index in indexes
        .iter()
        .filter_map(|index| index.get_ref().as_table())
    {
        if let Some((_, name)) = get(index, "name")
            && let Some(text) = name.get_ref().as_str()
        {
            let valid = !text.is_empty()
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if !valid {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("invalid index name `{text}`"),
                        name.span(),
                    )
                    .with_help("use ASCII letters, digits, `-`, `_`, and `.`"),
                );
            } else if let Some((_, first)) = seen.iter().find(|(seen, _)| *seen == text) {
                let diagnostic = Diagnostic::new(
                    Rule::DuplicateName,
                    format!("duplicate index name `{text}`"),
                    name.span(),
                )
                .with_label(first.clone(), "first used here");
                context.report(rejected_since(
                    checker,
                    DUPLICATE_INDEX_NAMES_REJECTED_SINCE,
                    diagnostic,
                ));
            } else {
                seen.push((text, name.span()));
            }
        }
        if let Some((_, flag)) = get(index, "default")
            && flag.get_ref().as_bool() == Some(true)
        {
            match &default {
                Some(first) => {
                    let diagnostic = Diagnostic::new(
                        Rule::InvalidValue,
                        "more than one index is the default",
                        flag.span(),
                    )
                    .with_label(first.clone(), "the first default");
                    context.report(rejected_since(
                        checker,
                        MULTIPLE_DEFAULT_INDEXES_REJECTED_SINCE,
                        diagnostic,
                    ));
                }
                None => default = Some(flag.span()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    const PROJECT: &str = "[project]\nname = \"demo\"\nversion = \"1\"\ndependencies = [\"idna\", \"flask @ https://example.com/flask.whl\"]\n[project.optional-dependencies]\nweb = [\"httpx\"]\n[dependency-groups]\ndev = [\"pytest\"]\n";

    /// Checks `body` after a project with dependencies, an extra `web`, and a
    /// group `dev`, returning (rule, severity, spanned text).
    fn check(body: &str) -> Vec<(&'static str, Severity, String)> {
        let text = format!("{PROJECT}{body}");
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
    fn valid_references() {
        let body = "[tool.uv]\ndefault-groups = [\"dev\"]\nconflicts = [[{ extra = \"web\" }, { group = \"dev\" }]]\n[tool.uv.sources]\nidna = { index = \"internal\" }\nhttpx = [{ git = \"https://github.com/encode/httpx\", tag = \"1.0\", marker = \"sys_platform == 'linux'\" }, { path = \"../httpx\", extra = \"web\" }]\npytest = { workspace = true, group = \"dev\" }\n[tool.uv.dependency-groups]\ndev = { requires-python = \">=3.12\" }\n[[tool.uv.index]]\nname = \"internal\"\nurl = \"https://example.com/simple\"\nexplicit = true\n";
        assert_eq!(check(body), []);
        assert_eq!(check("[tool.uv]\ndefault-groups = \"all\"\n"), []);
    }

    #[test]
    fn undefined_names_that_uv_rejects() {
        assert_eq!(
            check("[tool.uv.sources]\nidna = { index = \"internl\" }\n"),
            one("invalid-value", Severity::Error, "\"internl\"")
        );
        assert_eq!(
            check("[tool.uv.sources]\nhttpx = { path = \"../httpx\", extra = \"webb\" }\n"),
            one("invalid-value", Severity::Error, "\"webb\"")
        );
        assert_eq!(
            check("[tool.uv]\ndefault-groups = [\"test\"]\n"),
            one("invalid-value", Severity::Error, "\"test\"")
        );
        assert_eq!(
            check("[tool.uv.dependency-groups]\ntest = { requires-python = \">=3.12\" }\n"),
            one("invalid-value", Severity::Error, "test")
        );
        // `tool.uv.dev-dependencies` makes up the `dev` group.
        let text = "[project]\nname = \"demo\"\nversion = \"1\"\n[tool.uv]\ndev-dependencies = [\"pytest\"]\ndefault-groups = [\"dev\"]\n";
        let found = crate::check(text.as_bytes().to_vec()).diagnostics;
        assert!(
            found.iter().all(|d| d.rule.name() == "deprecated-setting"),
            "{found:?}"
        );
    }

    #[test]
    fn names_that_uv_ignores() {
        assert_eq!(
            check("[tool.uv]\nconflicts = [[{ extra = \"web\" }, { extra = \"cli\" }]]\n"),
            one("invalid-value", Severity::Warning, "\"cli\"")
        );
        assert_eq!(
            check("[tool.uv.sources]\nrequests = { git = \"https://github.com/psf/requests\" }\n"),
            one("ineffective-setting", Severity::Warning, "requests")
        );
        // A workspace root's sources may be for its members' dependencies.
        let body = "[tool.uv.workspace]\nmembers = [\"packages/*\"]\n[tool.uv.sources]\nrequests = { workspace = true }\n";
        assert_eq!(check(body), []);
    }

    #[test]
    fn source_shapes() {
        assert_eq!(
            check(
                "[tool.uv.sources]\nidna = { git = \"https://github.com/kjd/idna\", brnch = \"main\" }\n"
            ),
            one("unknown-key", Severity::Error, "brnch")
        );
        assert_eq!(
            check(
                "[tool.uv.sources]\nidna = { git = \"https://github.com/kjd/idna\", tag = \"v1\", branch = \"main\" }\n"
            ),
            one(
                "invalid-value",
                Severity::Error,
                "{ git = \"https://github.com/kjd/idna\", tag = \"v1\", branch = \"main\" }"
            )
        );
        assert_eq!(
            check(
                "[tool.uv.sources]\nidna = { url = \"https://example.com/idna.whl\", path = \"../idna\" }\n"
            ),
            one(
                "invalid-value",
                Severity::Error,
                "{ url = \"https://example.com/idna.whl\", path = \"../idna\" }"
            )
        );
        assert_eq!(
            check("[tool.uv.sources]\nidna = { marker = \"sys_platform == 'linux'\" }\n"),
            one(
                "invalid-value",
                Severity::Error,
                "{ marker = \"sys_platform == 'linux'\" }"
            )
        );
    }

    #[test]
    fn conflicts_and_indexes() {
        assert_eq!(
            check("[tool.uv]\nconflicts = [[{ extra = \"web\" }]]\n"),
            one("invalid-value", Severity::Error, "[{ extra = \"web\" }]")
        );
        let index = |name: &str, default: bool| {
            format!(
                "[[tool.uv.index]]\nname = \"{name}\"\nurl = \"https://example.com/{name}\"\ndefault = {default}\n"
            )
        };
        assert_eq!(
            check(&format!("{}{}", index("a", false), index("a", false))),
            one("duplicate-name", Severity::Error, "\"a\"")
        );
        assert_eq!(
            check(&format!("{}{}", index("a", true), index("b", true))),
            one("invalid-value", Severity::Error, "true")
        );
        // uv rejects a second default from 0.10.0 and duplicate names from 0.6.4.
        let pinned = |version: &str, body: &str| {
            check(&format!(
                "[tool.uv]\nrequired-version = \">={version}\"\n{body}"
            ))
        };
        let defaults = format!("{}{}", index("a", true), index("b", true));
        assert_eq!(
            pinned("0.9", &defaults),
            one("invalid-value", Severity::Warning, "true")
        );
        let duplicates = format!("{}{}", index("a", false), index("a", false));
        assert_eq!(
            pinned("0.9", &duplicates),
            one("duplicate-name", Severity::Error, "\"a\"")
        );
        // `[tool.uv.dependency-groups]` is from 0.8.0; before, it is unknown.
        let text =
            "[tool.uv]\nrequired-version = \"==0.7.0\"\n[tool.uv.dependency-groups]\ntest = {}\n";
        assert_eq!(
            check(text),
            one(
                "unsupported-feature",
                Severity::Warning,
                "dependency-groups"
            )
        );
        assert_eq!(
            check(&index("my index", false)),
            one("invalid-value", Severity::Error, "\"my index\"")
        );
    }
}
