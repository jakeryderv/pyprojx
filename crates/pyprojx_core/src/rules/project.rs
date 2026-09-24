//! The structure of `[project]`, from the pyproject.toml specification.
//!
//! Dependencies, extras, and license fields are checked separately.

use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::{Context, is_dotted_identifier, is_object_reference, suggest};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::{Value, describe_type, get};
use crate::standards::{check_specifiers, check_version, normalize_name};

/// Every key the specification allows in `[project]`.
const KEYS: &[&str] = &[
    "name",
    "version",
    "description",
    "readme",
    "requires-python",
    "license",
    "license-files",
    "authors",
    "maintainers",
    "keywords",
    "classifiers",
    "urls",
    "scripts",
    "gui-scripts",
    "entry-points",
    "dependencies",
    "optional-dependencies",
    "import-names",
    "import-namespaces",
    "dynamic",
];

/// Keys that may be set statically and also listed in `dynamic` (PEP 808), in
/// which case build backends may only append entries.
const EXTENDABLE_KEYS: &[&str] = &[
    "authors",
    "classifiers",
    "dependencies",
    "entry-points",
    "gui-scripts",
    "import-names",
    "import-namespaces",
    "keywords",
    "license-files",
    "maintainers",
    "optional-dependencies",
    "scripts",
    "urls",
];

const README_CONTENT_TYPES: &[&str] = &["text/markdown", "text/x-rst", "text/plain"];
const MAX_URL_LABEL_CHARS: usize = 32;

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((project_key, value)) = get(root, "project") else {
        return;
    };
    let Some(table) = context.expect_table("project", value) else {
        return;
    };
    context.check_keys(table, "`[project]`", KEYS);
    let dynamic = check_dynamic(context, table);
    let is_dynamic = |key: &str| dynamic.iter().any(|(name, _)| *name == key);

    match get(table, "name") {
        Some((_, value)) => check_name(context, value),
        None => context.report(
            Diagnostic::new(
                Rule::MissingKey,
                "`[project]` is missing the required key `name`",
                project_key.span(),
            )
            .with_help(
                "the project name must be set statically, for example `name = \"my-project\"`",
            ),
        ),
    }

    match get(table, "version") {
        Some((_, value)) => {
            if let Some(version) = context.expect_string("project.version", value)
                && let Err(problem) = check_version(version)
            {
                let message = format!("invalid version `{version}`: {}", problem.message);
                context.invalid_string(message, value.span(), &problem);
            }
        }
        None if !is_dynamic("version") => context.report(
            Diagnostic::new(
                Rule::MissingKey,
                "`[project]` must set `version` or list it in `dynamic`",
                project_key.span(),
            )
            .with_help("add `version = \"0.1.0\"`, or `dynamic = [\"version\"]` if your build backend provides it"),
        ),
        None => {}
    }

    if let Some((_, value)) = get(table, "description")
        && let Some(description) = context.expect_string("project.description", value)
        && description.contains(['\n', '\r'])
    {
        context.report(
            Diagnostic::new(
                Rule::InvalidValue,
                "`project.description` should be a single line",
                value.span(),
            )
            .with_help("core metadata allows a one-line summary; put the full description in the file referenced by `readme`")
            .as_warning(),
        );
    }

    if let Some((_, value)) = get(table, "readme") {
        check_readme(context, value);
    }

    if let Some((_, value)) = get(table, "requires-python")
        && let Some(specifiers) = context.expect_string("project.requires-python", value)
        && let Err(problem) = check_specifiers(specifiers)
    {
        let message = format!("invalid `requires-python`: {}", problem.message);
        context.invalid_string(message, value.span(), &problem);
    }

    for key in ["authors", "maintainers"] {
        if let Some((_, value)) = get(table, key) {
            check_people(context, key, value);
        }
    }

    for key in ["keywords", "classifiers"] {
        if let Some((_, value)) = get(table, key) {
            context.expect_string_array(&format!("project.{key}"), value);
        }
    }

    if let Some((_, value)) = get(table, "urls") {
        check_urls(context, value);
    }

    for key in ["scripts", "gui-scripts"] {
        if let Some((_, value)) = get(table, key)
            && let Some(scripts) = context.expect_table(&format!("project.{key}"), value)
        {
            check_entry_points(context, &format!("project.{key}"), scripts);
        }
    }
    if let Some((_, value)) = get(table, "entry-points") {
        check_entry_point_groups(context, value);
    }

    check_import_names(context, table);
    super::dependencies::check(context, table);
    super::license::check(context, table);
}

/// Checks `project.dynamic` and returns the listed keys with their spans.
fn check_dynamic<'v>(
    context: &mut Context<'_>,
    table: &'v DeTable<'_>,
) -> Vec<(&'v str, Range<usize>)> {
    let Some((_, value)) = get(table, "dynamic") else {
        return Vec::new();
    };
    let entries = context.expect_string_array("project.dynamic", value);
    for (name, span) in &entries {
        if *name == "name" {
            context.report(
                Diagnostic::new(
                    Rule::InvalidDynamic,
                    "`name` cannot be listed in `project.dynamic`",
                    span.clone(),
                )
                .with_help("the project name must be set statically"),
            );
        } else if !KEYS.contains(name) || *name == "dynamic" {
            let mut diagnostic = Diagnostic::new(
                Rule::InvalidDynamic,
                format!("`{name}` in `project.dynamic` is not a `[project]` key"),
                span.clone(),
            );
            if let Some(suggestion) = suggest(name, KEYS) {
                diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
            }
            context.report(diagnostic);
        } else if let Some((key, _)) = get(table, name) {
            let diagnostic = Diagnostic::new(
                Rule::InvalidDynamic,
                format!("`{name}` is set statically and also listed in `project.dynamic`"),
                key.span(),
            )
            .with_label(span.clone(), "listed in `dynamic` here");
            context.report(if EXTENDABLE_KEYS.contains(name) {
                // Allowed since PEP 808, but backends have been slow to adopt it.
                diagnostic
                    .with_help(format!("PEP 808 (2026) allows a build backend to extend a static `{name}`, but many backends, including hatchling and setuptools as of September 2026, still reject it"))
                    .as_warning()
            } else {
                diagnostic.with_help(format!(
                    "remove `{name}` from `dynamic` or remove its static value"
                ))
            });
        }
    }
    entries
}

fn check_name(context: &mut Context<'_>, value: &Value<'_>) {
    if let Some(name) = context.expect_string("project.name", value)
        && normalize_name(name).is_err()
    {
        context.report(
            Diagnostic::new(
                Rule::InvalidValue,
                format!("invalid project name `{name}`"),
                value.span(),
            )
            .with_help("names must start and end with a letter or digit and contain only letters, digits, `-`, `_`, and `.`"),
        );
    }
}

fn check_readme(context: &mut Context<'_>, value: &Value<'_>) {
    match value.get_ref() {
        DeValue::String(path) => {
            let lower = path.to_ascii_lowercase();
            if ![".md", ".rst", ".txt"]
                .iter()
                .any(|suffix| lower.ends_with(suffix))
            {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("cannot infer the content type of readme `{path}`"),
                        value.span(),
                    )
                    .with_help(format!("rename it with a `.md` or `.rst` suffix, or use `readme = {{ file = \"{path}\", content-type = \"text/plain\" }}`")),
                );
            }
        }
        DeValue::Table(readme) => {
            context.check_keys(
                readme,
                "`project.readme`",
                &["file", "text", "content-type"],
            );
            let file = get(readme, "file");
            let text = get(readme, "text");
            for (name, entry) in [("file", file), ("text", text)] {
                if let Some((_, value)) = entry {
                    context.expect_string(&format!("project.readme.{name}"), value);
                }
            }
            match (file, text) {
                (Some((file_key, _)), Some((text_key, _))) => context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        "`project.readme` cannot set both `file` and `text`",
                        text_key.span(),
                    )
                    .with_label(file_key.span(), "`file` is also set here"),
                ),
                (None, None) => context.report(Diagnostic::new(
                    Rule::MissingKey,
                    "`project.readme` must set `file` or `text`",
                    value.span(),
                )),
                _ => {}
            }
            match get(readme, "content-type") {
                Some((_, content_type)) => {
                    if let Some(content_type_value) =
                        context.expect_string("project.readme.content-type", content_type)
                    {
                        let media_type = content_type_value
                            .split(';')
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .to_ascii_lowercase();
                        if !README_CONTENT_TYPES.contains(&media_type.as_str()) {
                            context.report(
                                Diagnostic::new(
                                    Rule::InvalidValue,
                                    format!(
                                        "unsupported readme content type `{content_type_value}`"
                                    ),
                                    content_type.span(),
                                )
                                .with_help("use `text/markdown`, `text/x-rst`, or `text/plain`"),
                            );
                        }
                    }
                }
                None => context.report(Diagnostic::new(
                    Rule::MissingKey,
                    "`project.readme` must set `content-type` when it is a table",
                    value.span(),
                )),
            }
        }
        other => context.report(Diagnostic::new(
            Rule::InvalidType,
            format!(
                "`project.readme` must be a string or a table, found {}",
                describe_type(other)
            ),
            value.span(),
        )),
    }
}

/// Checks `authors` or `maintainers`: an array of tables with `name` and/or `email`.
fn check_people(context: &mut Context<'_>, key: &str, value: &Value<'_>) {
    let name = format!("project.{key}");
    let Some(people) = value.get_ref().as_array() else {
        context.type_mismatch(&name, "an array of tables", value);
        return;
    };
    for person in people {
        let Some(table) = person.get_ref().as_table() else {
            context.report(
                Diagnostic::new(
                    Rule::InvalidType,
                    format!(
                        "entries of `{name}` must be tables, found {}",
                        describe_type(person.get_ref())
                    ),
                    person.span(),
                )
                .with_help("use `{ name = \"...\", email = \"...\" }`"),
            );
            continue;
        };
        context.check_keys(table, &format!("a `{name}` entry"), &["name", "email"]);
        let person_name = get(table, "name");
        let email = get(table, "email");
        if person_name.is_none() && email.is_none() {
            context.report(Diagnostic::new(
                Rule::MissingKey,
                format!("entries of `{name}` must set `name` or `email`"),
                person.span(),
            ));
        }
        if let Some((_, value)) = person_name
            && let Some(person_name) = context.expect_string(&format!("{name}.name"), value)
            && person_name.contains(',')
        {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("`{name}` names should not contain commas"),
                    value.span(),
                )
                .with_help("core metadata separates people with commas, so this reads as several names; list each person or organization as a separate entry")
                .as_warning(),
            );
        }
        if let Some((_, value)) = email
            && let Some(address) = context.expect_string(&format!("{name}.email"), value)
            && !is_email_address(address)
        {
            context.report(Diagnostic::new(
                Rule::InvalidValue,
                format!("invalid email address `{address}`"),
                value.span(),
            ));
        }
    }
}

/// A deliberately permissive check that catches obvious mistakes, such as a
/// name in the email field, without rejecting unusual valid addresses.
fn is_email_address(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !address.contains(|char: char| char.is_whitespace() || matches!(char, ',' | '<' | '>'))
}

fn check_urls(context: &mut Context<'_>, value: &Value<'_>) {
    let Some(urls) = context.expect_table("project.urls", value) else {
        return;
    };
    for (label, url) in urls {
        let label_name = label.get_ref().as_ref();
        if label_name.chars().count() > MAX_URL_LABEL_CHARS {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!(
                        "URL label `{label_name}` is longer than {MAX_URL_LABEL_CHARS} characters"
                    ),
                    label.span(),
                )
                .with_help("core metadata limits labels to 32 characters")
                .as_warning(),
            );
        }
        if let Some(address) = context.expect_string(&format!("project.urls.{label_name}"), url)
            && let Err(error) = url::Url::parse(address)
        {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("invalid URL for `{label_name}`: {error}"),
                    url.span(),
                )
                .as_warning(),
            );
        }
    }
}

/// Checks entries of `scripts`, `gui-scripts`, or an entry point group.
fn check_entry_points(context: &mut Context<'_>, location: &str, entries: &DeTable<'_>) {
    for (name, value) in entries {
        let entry_name = name.get_ref().as_ref();
        if entry_name.is_empty() || entry_name.contains('=') || entry_name.starts_with('[') {
            context.report(Diagnostic::new(
                Rule::InvalidValue,
                format!("invalid entry point name `{entry_name}`"),
                name.span(),
            ));
        }
        let Some(reference) = context.expect_string(&format!("{location}.{entry_name}"), value)
        else {
            continue;
        };
        // Entry point values may end with deprecated `[extra, ...]` markers.
        let object = reference.split('[').next().unwrap_or_default().trim();
        if !is_object_reference(object) {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("invalid object reference `{reference}`"),
                    value.span(),
                )
                .with_help("use `package.module:function`"),
            );
        }
    }
}

fn check_entry_point_groups(context: &mut Context<'_>, value: &Value<'_>) {
    let Some(groups) = context.expect_table("project.entry-points", value) else {
        return;
    };
    for (group, entries) in groups {
        let group_name = group.get_ref().as_ref();
        let replacement = match group_name {
            "console_scripts" => Some("scripts"),
            "gui_scripts" => Some("gui-scripts"),
            _ => None,
        };
        if let Some(replacement) = replacement {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!(
                        "`{group_name}` entry points must be declared in `[project.{replacement}]`"
                    ),
                    group.span(),
                )
                .with_help(format!("move these entries to `[project.{replacement}]`")),
            );
            continue;
        }
        if !is_entry_point_group(group_name) {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("invalid entry point group `{group_name}`"),
                    group.span(),
                )
                .with_help("group names are letters, digits, and underscores, separated by dots"),
            );
        }
        let location = format!("project.entry-points.{group_name}");
        if let Some(entries) = context.expect_table(&location, entries) {
            check_entry_points(context, &location, entries);
        }
    }
}

fn is_entry_point_group(name: &str) -> bool {
    name.split('.').all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|char| char == '_' || char.is_alphanumeric())
    })
}

/// Checks `import-names` and `import-namespaces` (PEP 794).
fn check_import_names(context: &mut Context<'_>, table: &DeTable<'_>) {
    let mut listed: Vec<(&str, &str, Range<usize>)> = Vec::new();
    for key in ["import-names", "import-namespaces"] {
        let Some((_, value)) = get(table, key) else {
            continue;
        };
        let entries = context.expect_string_array(&format!("project.{key}"), value);
        if key == "import-namespaces"
            && entries.is_empty()
            && value
                .get_ref()
                .as_array()
                .is_some_and(|array| array.is_empty())
        {
            context.report(Diagnostic::new(
                Rule::InvalidValue,
                "`project.import-namespaces` cannot be empty",
                value.span(),
            ));
        }
        for (entry, span) in entries {
            let name = strip_private(entry);
            let valid = if name.is_empty() {
                key == "import-names"
            } else {
                is_dotted_identifier(name)
            };
            if !valid {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("invalid import name `{entry}`"),
                        span.clone(),
                    )
                    .with_help("use a dotted Python name, optionally followed by `; private`"),
                );
            }
            listed.push((key, name, span));
        }
    }
    for (index, (key, name, span)) in listed.iter().enumerate() {
        if *key != "import-namespaces" || name.is_empty() {
            continue;
        }
        if let Some((_, _, other)) = listed[..index]
            .iter()
            .find(|(other_key, other_name, _)| *other_key == "import-names" && other_name == name)
        {
            context.report(
                Diagnostic::new(
                    Rule::InvalidValue,
                    format!("`{name}` is listed in both `import-names` and `import-namespaces`"),
                    span.clone(),
                )
                .with_label(other.clone(), "also listed in `import-names` here"),
            );
        }
    }
}

/// Removes a trailing `; private` marker from an import name.
fn strip_private(entry: &str) -> &str {
    match entry.rsplit_once(';') {
        Some((name, marker)) if marker.trim() == "private" => name.trim(),
        _ => entry.trim(),
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::diagnostics;

    /// Checks a `[project]` table body and returns (rule, message, spanned text).
    fn project(body: &str) -> Vec<(&'static str, String, String)> {
        let text = format!("[project]\n{body}");
        diagnostics(&text)
            .into_iter()
            .map(|(rule, message, span)| (rule, message, span.to_owned()))
            .collect()
    }

    fn rules(body: &str) -> Vec<&'static str> {
        project(body).into_iter().map(|(rule, ..)| rule).collect()
    }

    const BASE: &str = "name = \"demo\"\nversion = \"0.1.0\"\n";

    #[test]
    fn minimal_and_complete_projects_are_valid() {
        assert_eq!(project(BASE), []);
        let body = r#"name = "Demo_Project"
version = "1.0.post1"
description = "A demo"
readme = { file = "README", content-type = "text/markdown; charset=UTF-8; variant=GFM" }
requires-python = ">=3.11, <4"
authors = [{ name = "A. Person", email = "a@example.com" }, { email = "team@example.org" }]
maintainers = [{ name = "Someone" }]
keywords = ["toml"]
classifiers = ["Programming Language :: Python :: 3"]
urls = { Homepage = "https://example.com", "Bug Tracker" = "https://example.com/issues" }
scripts = { demo = "demo.cli:main" }
gui-scripts = { demo-gui = "demo.gui:run [gui]" }
entry-points = { "demo.plugins" = { a = "demo.plugins.a" } }
import-names = ["demo", "_demo_private ; private"]
import-namespaces = ["demo_ns"]
dynamic = ["dependencies"]
"#;
        assert_eq!(project(body), []);
    }

    #[test]
    fn unknown_keys_suggest_corrections() {
        let found = project(&format!("{BASE}requires_python = \">=3.11\"\n"));
        assert_eq!(found[0].0, "unknown-key");
        assert_eq!(found[0].2, "requires_python");
    }

    #[test]
    fn required_keys() {
        assert_eq!(
            project("version = \"1\"\n"),
            [(
                "missing-key",
                "`[project]` is missing the required key `name`".to_owned(),
                "project".to_owned()
            )]
        );
        assert_eq!(rules("name = \"demo\"\n"), ["missing-key"]);
        assert_eq!(
            rules("name = \"demo\"\ndynamic = [\"version\"]\n"),
            [] as [&str; 0]
        );
    }

    #[test]
    fn dynamic_rules() {
        let found = project(
            "name = \"demo\"\nversion = \"1\"\ndynamic = [\"name\", \"version\", \"licence\", \"dependencies\"]\ndependencies = []\n",
        );
        let summary: Vec<_> = found
            .iter()
            .map(|(rule, _, span)| (*rule, span.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("invalid-dynamic", "version"),
                ("invalid-dynamic", "\"name\""),
                ("invalid-dynamic", "\"licence\""),
                ("invalid-dynamic", "dependencies"),
            ]
        );
    }

    #[test]
    fn extending_static_keys_is_a_warning() {
        use crate::Severity;
        let severities = |text: &str| -> Vec<Severity> {
            crate::check(text.as_bytes().to_vec())
                .diagnostics
                .iter()
                .map(crate::Diagnostic::severity)
                .collect()
        };
        assert_eq!(
            severities(
                "[project]\nname = \"d\"\nversion = \"1\"\ndependencies = []\ndynamic = [\"dependencies\"]\n"
            ),
            [Severity::Warning]
        );
        assert_eq!(
            severities(
                "[project]\nname = \"d\"\nversion = \"1\"\ndescription = \"x\"\ndynamic = [\"description\"]\n"
            ),
            [Severity::Error]
        );
    }

    #[test]
    fn names_versions_and_specifiers() {
        assert_eq!(
            project(
                "name = \"-demo\"\nversion = \"1.0-beta.x\"\nrequires-python = \">=3.11,=<4\"\n"
            )
            .into_iter()
            .map(|(rule, _, span)| (rule, span))
            .collect::<Vec<_>>(),
            [
                ("invalid-value", "\"-demo\"".to_owned()),
                ("invalid-value", "\"1.0-beta.x\"".to_owned()),
                ("invalid-value", "=<4".to_owned()),
            ]
        );
    }

    #[test]
    fn description_and_readme() {
        assert_eq!(
            rules(&format!("{BASE}description = \"\"\"two\nlines\"\"\"\n")),
            ["invalid-value"]
        );
        assert_eq!(
            rules(&format!("{BASE}readme = \"README\"\n")),
            ["invalid-value"]
        );
        assert_eq!(
            rules(&format!("{BASE}readme = \"README.MD\"\n")),
            [] as [&str; 0]
        );
        assert_eq!(
            rules(&format!(
                "{BASE}readme = {{ file = \"a\", text = \"b\", content-type = \"text/html\" }}\n"
            )),
            ["invalid-value", "invalid-value"]
        );
        assert_eq!(
            rules(&format!("{BASE}readme = {{ text = \"b\" }}\n")),
            ["missing-key"]
        );
        assert_eq!(rules(&format!("{BASE}readme = 1\n")), ["invalid-type"]);
    }

    #[test]
    fn people() {
        let found = rules(&format!(
            "{BASE}authors = [\"me\", {{}}, {{ name = \"A, B\", email = \"not an email\", web = \"x\" }}]\n"
        ));
        assert_eq!(
            found,
            [
                "invalid-type",
                "missing-key",
                "invalid-value",
                "invalid-value",
                "unknown-key"
            ]
        );
    }

    #[test]
    fn urls() {
        let found = project(&format!(
            "{BASE}urls = {{ \"A very long label that exceeds the limit\" = \"https://example.com\", Docs = \"not a url\" }}\n"
        ));
        let summary: Vec<_> = found
            .iter()
            .map(|(rule, _, span)| (*rule, span.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "invalid-value",
                    "\"A very long label that exceeds the limit\""
                ),
                ("invalid-value", "\"not a url\""),
            ]
        );
    }

    #[test]
    fn entry_points() {
        let found = project(&format!(
            "{BASE}scripts = {{ demo = \"demo:main:x\" }}\n[project.entry-points.console_scripts]\ndemo = \"demo:main\"\n[project.entry-points.\"bad group\"]\nx = \"demo:x\"\n"
        ));
        let summary: Vec<_> = found
            .iter()
            .map(|(rule, _, span)| (*rule, span.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("invalid-value", "\"demo:main:x\""),
                ("invalid-value", "console_scripts"),
                ("invalid-value", "\"bad group\""),
            ]
        );
    }

    #[test]
    fn import_names() {
        let found = project(&format!(
            "{BASE}import-names = [\"ok\", \"\", \"bad-name\", \"shared\"]\nimport-namespaces = [\"shared ; private\"]\n"
        ));
        let summary: Vec<_> = found
            .iter()
            .map(|(rule, _, span)| (*rule, span.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("invalid-value", "\"bad-name\""),
                ("invalid-value", "\"shared ; private\"")
            ]
        );
        assert_eq!(
            rules(&format!("{BASE}import-namespaces = []\n")),
            ["invalid-value"]
        );
    }
}
