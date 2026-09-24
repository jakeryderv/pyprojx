//! `[dependency-groups]`, from the dependency groups specification (PEP 735).
//!
//! As a validator, pyprojx checks every group, like uv does, rather than only
//! the groups being installed.

use std::collections::HashMap;
use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::{Context, suggest};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::{describe_type, get};
use crate::standards::{check_requirement, normalize_name};

/// An `{ include-group = "..." }` entry.
struct Include {
    /// Normalized name of the group containing the include.
    from: String,
    /// Normalized name of the included group.
    target: String,
    /// The name as written.
    written: String,
    span: Range<usize>,
}

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((_, value)) = get(root, "dependency-groups") else {
        return;
    };
    let Some(groups) = context.expect_table("dependency-groups", value) else {
        return;
    };

    let mut names: HashMap<String, Range<usize>> = HashMap::new();
    let mut includes = Vec::new();
    for (key, entries) in groups {
        let name = key.get_ref().as_ref();
        let normalized = match normalize_name(name) {
            Ok(normalized) => normalized,
            Err(_) => {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("invalid dependency group name `{name}`"),
                        key.span(),
                    )
                    .with_help("names must start and end with a letter or digit and contain only letters, digits, `-`, `_`, and `.`"),
                );
                continue;
            }
        };
        if let Some(first) = names.get(&normalized) {
            context.report(
                Diagnostic::new(
                    Rule::DuplicateName,
                    format!(
                        "dependency group `{name}` is the same as another group after normalization"
                    ),
                    key.span(),
                )
                .with_label(first.clone(), "first defined here")
                .with_help(format!(
                    "group names are compared in normalized form, `{normalized}`"
                )),
            );
        } else {
            names.insert(normalized.clone(), key.span());
        }
        check_entries(context, name, &normalized, entries, &mut includes);
    }

    for include in &includes {
        if !names.contains_key(&include.target) {
            let known: Vec<&str> = names.keys().map(String::as_str).collect();
            let mut diagnostic = Diagnostic::new(
                Rule::InvalidValue,
                format!(
                    "included dependency group `{}` does not exist",
                    include.written
                ),
                include.span.clone(),
            );
            if let Some(suggestion) = suggest(&include.target, &known) {
                diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
            }
            context.report(diagnostic);
        }
    }
    report_cycles(context, &includes);
    check_extra_names(context, root, &names);
}

fn check_entries(
    context: &mut Context<'_>,
    name: &str,
    normalized: &str,
    entries: &toml::Spanned<DeValue<'_>>,
    includes: &mut Vec<Include>,
) {
    let location = format!("dependency-groups.{name}");
    let Some(array) = entries.get_ref().as_array() else {
        context.type_mismatch(&location, "an array", entries);
        return;
    };
    for entry in array {
        match entry.get_ref() {
            DeValue::String(requirement) => {
                if let Err(problem) = check_requirement(requirement) {
                    let message = format!("invalid requirement: {}", problem.message);
                    context.invalid_string(message, entry.span(), &problem);
                }
            }
            DeValue::Table(table) => {
                context.check_keys(table, "a dependency group include", &["include-group"]);
                match get(table, "include-group") {
                    Some((_, target)) => {
                        let Some(written) =
                            context.expect_string(&format!("{location}.include-group"), target)
                        else {
                            continue;
                        };
                        match normalize_name(written) {
                            Ok(target_name) => includes.push(Include {
                                from: normalized.to_owned(),
                                target: target_name,
                                written: written.to_owned(),
                                span: target.span(),
                            }),
                            Err(_) => context.report(Diagnostic::new(
                                Rule::InvalidValue,
                                format!("invalid dependency group name `{written}`"),
                                target.span(),
                            )),
                        }
                    }
                    None => context.report(
                        Diagnostic::new(
                            Rule::MissingKey,
                            "dependency group tables must set `include-group`",
                            entry.span(),
                        )
                        .with_help("use `{ include-group = \"other-group\" }`"),
                    ),
                }
            }
            other => context.report(Diagnostic::new(
                Rule::InvalidType,
                format!(
                    "entries of `{location}` must be strings or tables, found {}",
                    describe_type(other)
                ),
                entry.span(),
            )),
        }
    }
}

/// Reports each cycle of includes once, at the include that closes it.
fn report_cycles(context: &mut Context<'_>, includes: &[Include]) {
    let mut edges: HashMap<&str, Vec<&Include>> = HashMap::new();
    for include in includes {
        edges
            .entry(include.from.as_str())
            .or_default()
            .push(include);
    }
    let mut starts: Vec<&str> = edges.keys().copied().collect();
    starts.sort_unstable();

    let mut finished: Vec<&str> = Vec::new();
    let mut reported: Vec<Vec<&str>> = Vec::new();
    for start in starts {
        let mut path = vec![start];
        visit(context, &edges, &mut path, &mut finished, &mut reported);
    }
}

fn visit<'i>(
    context: &mut Context<'_>,
    edges: &HashMap<&'i str, Vec<&'i Include>>,
    path: &mut Vec<&'i str>,
    finished: &mut Vec<&'i str>,
    reported: &mut Vec<Vec<&'i str>>,
) {
    let current = *path.last().expect("path is never empty");
    if finished.contains(&current) {
        return;
    }
    for include in edges.get(current).into_iter().flatten() {
        let target = include.target.as_str();
        if let Some(position) = path.iter().position(|group| *group == target) {
            let mut members: Vec<&str> = path[position..].to_vec();
            members.sort_unstable();
            if !reported.contains(&members) {
                let mut cycle: Vec<&str> = path[position..].to_vec();
                cycle.push(target);
                context.report(Diagnostic::new(
                    Rule::InvalidValue,
                    format!(
                        "dependency groups include each other in a cycle: {}",
                        cycle
                            .iter()
                            .map(|group| format!("`{group}`"))
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                    include.span.clone(),
                ));
                reported.push(members);
            }
            continue;
        }
        path.push(target);
        visit(context, edges, path, finished, reported);
        path.pop();
    }
    finished.push(current);
}

/// Warns about groups named like extras, which the specification advises against.
fn check_extra_names(
    context: &mut Context<'_>,
    root: &DeTable<'_>,
    groups: &HashMap<String, Range<usize>>,
) {
    let extras = get(root, "project")
        .and_then(|(_, project)| project.get_ref().as_table())
        .and_then(|project| get(project, "optional-dependencies"))
        .and_then(|(_, extras)| extras.get_ref().as_table());
    let Some(extras) = extras else {
        return;
    };
    for extra in extras.keys() {
        let Ok(normalized) = normalize_name(extra.get_ref()) else {
            continue;
        };
        if let Some(group) = groups.get(&normalized) {
            context.report(
                Diagnostic::new(
                    Rule::DuplicateName,
                    format!("dependency group `{normalized}` has the same name as an extra"),
                    group.clone(),
                )
                .with_label(extra.span(), "extra defined here")
                .with_help("tools may treat this as an error; give the group a different name")
                .as_warning(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    /// Checks a `[dependency-groups]` body, returning (rule, severity, message, spanned text).
    fn groups(body: &str) -> Vec<(&'static str, Severity, String, String)> {
        let text = format!("[dependency-groups]\n{body}");
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

    fn spans(body: &str) -> Vec<(&'static str, String)> {
        groups(body)
            .into_iter()
            .map(|(rule, _, _, span)| (rule, span))
            .collect()
    }

    #[test]
    fn valid_groups() {
        let body = "test = [\"pytest>=8\"]\ndev = [{ include-group = \"Test\" }, \"ruff\"]\nall = [{ include-group = \"dev\" }]\n";
        assert_eq!(groups(body), []);
    }

    #[test]
    fn names_and_duplicates() {
        assert_eq!(
            spans("\"a b\" = []\ndev = []\nDev = []\n"),
            [
                ("invalid-value", "\"a b\"".to_owned()),
                ("duplicate-name", "Dev".to_owned())
            ]
        );
    }

    #[test]
    fn entries() {
        assert_eq!(
            spans(
                "a = \"pytest\"\nb = [\"requests >= 2.3.*\", 1, { set-phasers-to = \"stun\" }]\n"
            ),
            [
                ("invalid-type", "\"pytest\"".to_owned()),
                ("invalid-value", ">= 2.3.*".to_owned()),
                ("invalid-type", "1".to_owned()),
                ("missing-key", "{ set-phasers-to = \"stun\" }".to_owned()),
                ("unknown-key", "set-phasers-to".to_owned()),
            ]
        );
    }

    #[test]
    fn missing_includes_suggest_names() {
        let found = groups("test = []\ndev = [{ include-group = \"tests\" }]\n");
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].2,
            "included dependency group `tests` does not exist"
        );
    }

    #[test]
    fn cycles_are_reported_once() {
        let found = groups(
            "a = [{ include-group = \"b\" }]\nb = [{ include-group = \"c\" }]\nc = [{ include-group = \"a\" }]\nd = [{ include-group = \"d\" }]\n",
        );
        let messages: Vec<_> = found
            .iter()
            .map(|(_, _, message, _)| message.as_str())
            .collect();
        assert_eq!(
            messages,
            [
                "dependency groups include each other in a cycle: `a` -> `b` -> `c` -> `a`",
                "dependency groups include each other in a cycle: `d` -> `d`",
            ]
        );
    }

    #[test]
    fn group_named_like_an_extra_is_a_warning() {
        let text = "[project]\nname = \"demo\"\nversion = \"1\"\n[project.optional-dependencies]\nDocs = []\n[dependency-groups]\ndocs = []\n";
        let found: Vec<_> = crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| (d.rule.name(), d.severity()))
            .collect();
        assert_eq!(found, [("duplicate-name", Severity::Warning)]);
    }
}
