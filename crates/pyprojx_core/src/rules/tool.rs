//! Tool configuration tables, such as `[tool.ruff]`, checked against the tool
//! releases the project allows.
//!
//! The allowed releases come from the tool's requirements in the project's
//! dependencies, extras, and dependency groups, any of which might be used,
//! narrowed by a setting such as Ruff's `required-version`, which the tool
//! enforces. Without either, the latest release is assumed.

use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::{Context, suggest};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::standards::{VersionSet, allowed_versions, required_versions};
use crate::tool::{OptionData, OptionKind, Tool, ValueType};

/// Checks a tool's table against the releases the project allows.
pub(super) struct Checker {
    pub tool: &'static Tool,
    /// Indexes into the tool's releases, oldest first.
    pub candidates: Vec<usize>,
    /// Where the allowed versions are set.
    pub source: Option<Range<usize>>,
}

/// Tool-specific checks, called as [`Checker`] walks a table.
pub(super) trait Extension {
    /// Reports a deprecated option, returning `false` to leave it to the checker.
    fn deprecated(&mut self, _prefix: &str, _name: &str, _span: Range<usize>) -> bool {
        false
    }

    /// Checks a value, or a map's value, that every allowed release accepts.
    fn accepted(
        &mut self,
        _context: &mut Context<'_>,
        _checker: &Checker,
        _option: &OptionData,
        _value: &toml::Spanned<DeValue<'_>>,
    ) {
    }

    /// Checks an option that some allowed release supports, after its value.
    fn option(
        &mut self,
        _context: &mut Context<'_>,
        _checker: &Checker,
        _option: &OptionData,
        _value: &toml::Spanned<DeValue<'_>>,
    ) {
    }
}

impl Checker {
    /// Finds the releases of `tool` the project allows: those of any
    /// `requirement` in the project's dependencies, narrowed by the setting
    /// `required_version` in `table`, reporting it if invalid.
    pub fn new(
        context: &mut Context<'_>,
        tool: &'static Tool,
        root: &DeTable<'_>,
        table: &DeTable<'_>,
        requirement: Option<&str>,
        required_version: Option<&str>,
    ) -> Self {
        let releases = tool.releases;
        let latest = tool.latest();
        let mut candidates: Option<Vec<usize>> = None;
        let mut source = None;
        for (versions, span) in requirement
            .map(|name| requirements(root, name))
            .unwrap_or_default()
        {
            source.get_or_insert(span);
            let candidates = candidates.get_or_insert_default();
            match versions {
                // Installers pick the newest release.
                None => candidates.push(latest),
                Some(versions) => candidates.extend(
                    (0..releases.len()).filter(|&index| versions.contains(releases[index])),
                ),
            }
        }

        if let Some(key) = required_version
            && let Some((_, required)) = get(table, key)
            && let Some(text) = required.get_ref().as_str()
        {
            match allowed_versions(text) {
                Ok(versions) => {
                    // Without requirements, any release might run, but the tool
                    // refuses to run unless it matches.
                    let allowed = candidates.get_or_insert_with(|| (0..releases.len()).collect());
                    allowed.retain(|&index| versions.contains(releases[index]));
                    source = Some(required.span());
                }
                Err(problem) => {
                    let message = format!("invalid `{}.{key}`: {}", tool.table, problem.message);
                    context.invalid_string(message, required.span(), &problem);
                }
            }
        }
        let mut candidates = candidates.unwrap_or_else(|| vec![latest]);
        candidates.sort_unstable();
        candidates.dedup();
        Self {
            tool,
            candidates,
            source,
        }
    }

    /// The newest allowed release. Checkers run only with candidates.
    pub fn newest(&self) -> usize {
        *self
            .candidates
            .last()
            .expect("checked only with candidates")
    }

    pub fn release(&self, index: usize) -> &'static str {
        self.tool.release(index)
    }

    /// Checks the options in the table at `prefix`, such as `lint.`.
    pub fn check_table(
        &self,
        context: &mut Context<'_>,
        extension: &mut dyn Extension,
        table: &DeTable<'_>,
        prefix: &str,
    ) {
        let tool = self.tool;
        for (key, value) in table {
            let name = key.get_ref().as_ref();
            let path = format!("{prefix}{name}");
            let Some(option) = tool.option(&path) else {
                self.report_unknown(context, prefix, name, key.span());
                continue;
            };
            let supported = self
                .candidates
                .iter()
                .filter(|&&index| option.is_present(index))
                .count();
            if supported < self.candidates.len() {
                self.report_unsupported(context, option, key.span(), supported == 0);
                if supported == 0 {
                    continue;
                }
            }

            let newest = self.newest();
            if option.is_deprecated(newest) && !extension.deprecated(prefix, name, key.span()) {
                self.report_deprecated(context, option, newest, key.span());
            }
            self.check_value(context, extension, option, value);
            extension.option(context, self, option, value);
            if option.kind == OptionKind::Table {
                match value.get_ref() {
                    DeValue::Table(table) => {
                        self.check_table(context, extension, table, &format!("{path}."));
                    }
                    _ => context.type_mismatch(&format!("{}.{path}", tool.table), "a table", value),
                }
            }
        }
    }

    /// Checks an option's value, or each value of a map, against its type in the
    /// allowed releases.
    fn check_value(
        &self,
        context: &mut Context<'_>,
        extension: &mut dyn Extension,
        option: &OptionData,
        value: &toml::Spanned<DeValue<'_>>,
    ) {
        let tool = self.tool;
        let name = tool.name;
        let path = format!("{}.{}", tool.table, option.path);
        let values: Vec<&toml::Spanned<DeValue<'_>>> = match (option.kind, value.get_ref()) {
            (OptionKind::Table, _) => return,
            (OptionKind::Map, DeValue::Table(map)) => map.values().collect(),
            (OptionKind::Map, _) => {
                context.type_mismatch(&path, "a table", value);
                return;
            }
            (OptionKind::Value, _) => vec![value],
        };
        for value in values {
            let accepts = |index: usize| {
                option
                    .value_type(index)
                    .is_none_or(|ty| ty.accepts(value.get_ref()))
            };
            let rejecting: Vec<usize> = self
                .candidates
                .iter()
                .copied()
                .filter(|&index| !accepts(index))
                .collect();
            let Some(&first_rejecting) = rejecting.first() else {
                extension.accepted(context, self, option, value);
                continue;
            };
            let expected = option
                .value_type(self.newest())
                .or_else(|| option.value_type(first_rejecting))
                .map_or_else(String::new, ValueType::describe);
            if rejecting.len() == self.candidates.len() {
                // Suggest the releases that accept it, if any do.
                let accepted = (0..tool.releases.len())
                    .filter(|&index| option.is_present(index) && accepts(index))
                    .collect::<Vec<_>>();
                let mut diagnostic = Diagnostic::new(
                    Rule::InvalidValue,
                    format!("invalid value for `{path}`: expected {expected}"),
                    value.span(),
                );
                if let (Some(&first), Some(&last)) = (accepted.first(), accepted.last()) {
                    let history = if first > first_rejecting {
                        format!("{name} accepts it from {}", self.release(first))
                    } else {
                        format!("{name} accepted it until {}", self.release(last))
                    };
                    diagnostic = diagnostic.with_help(history);
                    if let Some(source) = &self.source {
                        diagnostic = diagnostic
                            .with_label(source.clone(), "allows only versions that reject it");
                    }
                }
                context.report(diagnostic);
            } else {
                let mut diagnostic = Diagnostic::new(
                    Rule::UnsupportedFeature,
                    format!(
                        "some {name} versions the project allows do not accept this value for `{path}`"
                    ),
                    value.span(),
                )
                .with_help(format!(
                    "{name} {} expects {}",
                    self.release(first_rejecting),
                    option
                        .value_type(first_rejecting)
                        .map_or_else(String::new, ValueType::describe)
                ))
                .as_warning();
                if let Some(source) = &self.source {
                    diagnostic =
                        diagnostic.with_label(source.clone(), "allows versions that reject it");
                }
                context.report(diagnostic);
            }
        }
    }

    fn table_name(&self, prefix: &str) -> String {
        match prefix.strip_suffix('.') {
            Some(table) => format!("`[{}.{table}]`", self.tool.table),
            None => format!("`[{}]`", self.tool.table),
        }
    }

    /// Reports a key that no known release accepts.
    fn report_unknown(
        &self,
        context: &mut Context<'_>,
        prefix: &str,
        name: &str,
        span: Range<usize>,
    ) {
        let tool = self.tool;
        let known = tool.names_in(prefix, self.newest());
        let help = match suggest(name, &known) {
            Some(suggestion) => format!("did you mean `{suggestion}`?"),
            None => format!(
                "no {} release from {} to {} has this option; see {}",
                tool.name,
                tool.release(0),
                tool.release(tool.latest()),
                tool.docs
            ),
        };
        context.report(
            Diagnostic::new(
                Rule::UnknownKey,
                format!("unknown key `{name}` in {}", self.table_name(prefix)),
                span,
            )
            .with_help(help),
        );
    }

    /// Reports an option that some or all allowed releases lack.
    fn report_unsupported(
        &self,
        context: &mut Context<'_>,
        option: &OptionData,
        span: Range<usize>,
        none_supported: bool,
    ) {
        let lacking = *self
            .candidates
            .iter()
            .find(|&&index| !option.is_present(index))
            .expect("some candidate lacks the option");
        self.report_missing(
            context,
            &format!("`{}.{}`", self.tool.table, option.path),
            (option.added_after(lacking), option.removed_before(lacking)),
            span,
            none_supported,
        );
    }

    /// Reports something that some or all allowed releases lack, given when the
    /// tool added or removed it relative to one of those releases.
    pub fn report_missing(
        &self,
        context: &mut Context<'_>,
        subject: &str,
        (added, removed): (Option<usize>, Option<usize>),
        span: Range<usize>,
        none_supported: bool,
    ) {
        let name = self.tool.name;
        let help = match (added, removed) {
            (Some(added), _) => format!(
                "{name} added it in {}; {}",
                self.release(added),
                self.tool.upgrade_to(added)
            ),
            (None, removed) => format!(
                "{name} removed it in {}",
                removed.map_or("", |index| self.release(index))
            ),
        };
        let (message, label) = if none_supported {
            (
                format!("no {name} version the project allows supports {subject}"),
                "allows only versions without it",
            )
        } else {
            (
                format!("some {name} versions the project allows do not support {subject}"),
                "allows versions without it",
            )
        };
        let mut diagnostic =
            Diagnostic::new(Rule::UnsupportedFeature, message, span).with_help(help);
        if let Some(source) = &self.source {
            diagnostic = diagnostic.with_label(source.clone(), label);
        }
        // The tool rejects what it does not know.
        if !none_supported {
            diagnostic = diagnostic.as_warning();
        }
        context.report(diagnostic);
    }

    fn report_deprecated(
        &self,
        context: &mut Context<'_>,
        option: &OptionData,
        newest: usize,
        span: Range<usize>,
    ) {
        let since = option
            .deprecated_since(newest)
            .map_or("", |index| self.release(index));
        let mut diagnostic = Diagnostic::new(
            Rule::DeprecatedSetting,
            format!(
                "`{}.{}` is deprecated since {} {since}",
                self.tool.table, option.path, self.tool.name
            ),
            span,
        );
        if let Some(message) = option.message {
            diagnostic = diagnostic.with_help(message);
        }
        context.report(diagnostic);
    }
}

/// The versions allowed by each requirement on `name` in the project's
/// dependencies, extras, and dependency groups (`None` for no version
/// specifiers), with the requirement's span.
fn requirements(root: &DeTable<'_>, name: &str) -> Vec<(Option<VersionSet>, Range<usize>)> {
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
                && let Some((package, versions)) = required_versions(text)
                && package == name
            {
                found.push((versions, entry.span()));
            }
        }
    }
    found
}
