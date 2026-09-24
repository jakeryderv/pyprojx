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
use crate::document::{describe_type, get};
use crate::standards::{VersionSet, allowed_versions, allows_python_minor, required_versions};
use crate::tool::{OptionData, OptionKind, Tool, Treatment, ValueType};

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
    /// Whether to leave the option at `path` to other checks.
    fn skips(&self, _path: &str) -> bool {
        false
    }

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

    /// Checks the options in the table at `prefix`, such as `lint.`, which
    /// spans `span`.
    pub fn check_table(
        &self,
        context: &mut Context<'_>,
        extension: &mut dyn Extension,
        (table, span): (&DeTable<'_>, Range<usize>),
        prefix: &str,
    ) {
        let tool = self.tool;
        self.check_required(context, table, span, prefix);
        for (key, value) in table {
            let name = key.get_ref().as_ref();
            let path = format!("{prefix}{name}");
            if extension.skips(&path) {
                continue;
            }
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
            let prefix = format!("{path}.");
            let name = format!("{}.{path}", tool.table);
            let treatment = tool.invalid_value(&path);
            match (option.kind, value.get_ref()) {
                (OptionKind::Table, DeValue::Table(table)) => {
                    self.check_table(context, extension, (table, value.span()), &prefix);
                }
                (OptionKind::Table, _) => {
                    let diagnostic = mismatch(&format!("`{name}`"), "a table", value);
                    context.report(self.treat(diagnostic, treatment));
                }
                (OptionKind::TableArray, DeValue::Array(entries)) => {
                    for entry in entries {
                        match entry.get_ref() {
                            DeValue::Table(table) => {
                                self.check_table(
                                    context,
                                    extension,
                                    (table, entry.span()),
                                    &prefix,
                                );
                            }
                            _ => {
                                let subject = format!("entries of `{name}`");
                                let diagnostic = mismatch(&subject, "tables", entry);
                                context.report(self.treat(diagnostic, treatment));
                            }
                        }
                    }
                }
                (OptionKind::TableArray, _) => {
                    let diagnostic = mismatch(&format!("`{name}`"), "an array of tables", value);
                    context.report(self.treat(diagnostic, treatment));
                }
                (OptionKind::Map | OptionKind::Value, _) => {}
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
            (OptionKind::Table | OptionKind::TableArray, _) => return,
            (OptionKind::Map, DeValue::Table(map)) => map.values().collect(),
            (OptionKind::Map, _) => {
                let diagnostic = mismatch(&format!("`{path}`"), "a table", value);
                context.report(self.treat(diagnostic, tool.invalid_value(option.path)));
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
                context.report(self.treat(diagnostic, tool.invalid_value(option.path)));
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

    /// The table at `prefix` in messages, such as "`[tool.ruff.lint]`".
    fn table_name(&self, prefix: &str) -> String {
        let table = self.tool.table;
        match prefix.strip_suffix('.') {
            Some(path)
                if self
                    .tool
                    .option(path)
                    .is_some_and(|option| option.kind == OptionKind::TableArray) =>
            {
                format!("`[[{table}.{path}]]`")
            }
            Some(path) => format!("`[{table}.{path}]`"),
            None => format!("`[{table}]`"),
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
        let diagnostic = Diagnostic::new(
            Rule::UnknownKey,
            format!("unknown key `{name}` in {}", self.table_name(prefix)),
            span,
        )
        .with_help(help);
        let table = prefix.strip_suffix('.').unwrap_or_default();
        context.report(self.treat(diagnostic, tool.unknown_key(table)));
    }

    /// Reports options the table at `prefix` requires but lacks.
    fn check_required(
        &self,
        context: &mut Context<'_>,
        table: &DeTable<'_>,
        span: Range<usize>,
        prefix: &str,
    ) {
        let tool = self.tool;
        let newest = self.newest();
        let path = prefix.strip_suffix('.').unwrap_or_default();
        for &required in tool.required {
            let Some(name) = required
                .strip_prefix(prefix)
                .filter(|name| !name.contains('.'))
            else {
                continue;
            };
            let present = tool
                .option(required)
                .is_some_and(|option| option.is_present(newest));
            if present && get(table, name).is_none() {
                let diagnostic = Diagnostic::new(
                    Rule::MissingKey,
                    format!("missing `{name}` in {}", self.table_name(prefix)),
                    span.clone(),
                )
                .with_help(format!("{} requires it", tool.name));
                context.report(self.treat(diagnostic, tool.invalid_value(path)));
            }
        }
    }

    /// Applies what the tool does with a setting it cannot read: an error if it
    /// rejects the setting, and otherwise a warning that says what it does.
    fn treat(&self, mut diagnostic: Diagnostic, treatment: Treatment) -> Diagnostic {
        let note = match treatment {
            Treatment::Rejects => return diagnostic,
            Treatment::Warns => self.tool.warning.map(str::to_owned),
            Treatment::Ignores => Some(format!("{} ignores it", self.tool.name)),
        };
        if let Some(note) = note {
            diagnostic.help = Some(match diagnostic.help.take() {
                Some(help) if help.ends_with('?') => format!("{help} {note}"),
                Some(help) => format!("{help}; {note}"),
                None => note,
            });
        }
        diagnostic.as_warning()
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
        // The tool treats an option it does not know as an unknown key.
        let table = option.path.rsplit_once('.').map_or("", |(table, _)| table);
        self.report_missing(
            context,
            &format!("`{}.{}`", self.tool.table, option.path),
            (option.added_after(lacking), option.removed_before(lacking)),
            span,
            none_supported,
            self.tool.unknown_key(table),
        );
    }

    /// Reports something that some or all allowed releases lack, given when the
    /// tool added or removed it relative to one of those releases. If none of
    /// them has it, `treatment` says what the tool does with it.
    pub fn report_missing(
        &self,
        context: &mut Context<'_>,
        subject: &str,
        (added, removed): (Option<usize>, Option<usize>),
        span: Range<usize>,
        none_supported: bool,
        treatment: Treatment,
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
        context.report(if none_supported {
            self.treat(diagnostic, treatment)
        } else {
            diagnostic.as_warning()
        });
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

/// Warns when a tool's Python version setting at `path`, such as Ruff's
/// `target-version`, names Python 3.`minor`, newer than the oldest Python that
/// `requires-python` allows. `spell` spells a version for the setting, and
/// `risk` says what can go wrong, such as "Ruff may suggest code that needs".
pub(super) fn check_python_version(
    context: &mut Context<'_>,
    root: &DeTable<'_>,
    checker: &Checker,
    (path, value, minor): (&str, &toml::Spanned<DeValue<'_>>, u64),
    spell: fn(u64) -> String,
    risk: &str,
) {
    // Invalid values are reported already.
    let accepted = checker
        .tool
        .option(path)
        .and_then(|option| option.value_type(checker.newest()))
        .is_some_and(|ty| ty.accepts(value.get_ref()));
    if !accepted {
        return;
    }
    let Some((_, requires)) = get(root, "project")
        .and_then(|(_, project)| project.get_ref().as_table())
        .and_then(|project| get(project, "requires-python"))
    else {
        return;
    };
    let Some(specifiers) = requires.get_ref().as_str() else {
        return;
    };
    let Some(oldest) =
        (7..minor).find(|&older| allows_python_minor(specifiers, 3, older) == Some(true))
    else {
        return;
    };
    let name = checker.tool.name;
    let setting = path.rsplit('.').next().unwrap_or(path);
    context.report(
        Diagnostic::new(
            Rule::InvalidValue,
            format!("`{setting}` is Python 3.{minor}, but `requires-python` allows Python 3.{oldest}"),
            value.span(),
        )
        .with_label(requires.span(), "`requires-python` set here")
        .with_help(format!(
            "{risk} Python 3.{minor}; set `{setting} = \"{}\"`, or remove it so that {name} uses `requires-python`",
            spell(oldest)
        ))
        .as_warning(),
    );
}

/// A diagnostic for `subject`, such as "`tool.uv.pip`", having the wrong type.
fn mismatch(subject: &str, expected: &str, value: &toml::Spanned<DeValue<'_>>) -> Diagnostic {
    Diagnostic::new(
        Rule::InvalidType,
        format!(
            "{subject} must be {expected}, found {}",
            describe_type(value.get_ref())
        ),
        value.span(),
    )
}
