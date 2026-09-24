//! Checks for the standard tables of `pyproject.toml`.

use std::ops::Range;

use toml::de::DeTable;

use crate::diagnostic::{Diagnostic, Rule};
use crate::document::{Value, describe_type, string_span};
use crate::standards::Problem;

mod build_system;

/// Checks the standard tables of a valid TOML document.
pub fn check(root: &DeTable<'_>, text: &str) -> Vec<Diagnostic> {
    let mut context = Context {
        text,
        diagnostics: Vec::new(),
    };
    build_system::check(&mut context, root);
    context.diagnostics
}

/// Shared state and helpers for rules.
struct Context<'t> {
    text: &'t str,
    diagnostics: Vec<Diagnostic>,
}

impl Context<'_> {
    fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Reports keys of `table` that are not in `allowed`, suggesting close matches.
    fn check_keys(&mut self, table: &DeTable<'_>, table_name: &str, allowed: &[&str]) {
        for key in table.keys() {
            let name = key.get_ref().as_ref();
            if allowed.contains(&name) {
                continue;
            }
            let help = match suggest(name, allowed) {
                Some(suggestion) => format!("did you mean `{suggestion}`?"),
                None => format!("expected one of {}", quoted_list(allowed)),
            };
            self.report(
                Diagnostic::new(
                    Rule::UnknownKey,
                    format!("unknown key `{name}` in `[{table_name}]`"),
                    key.span(),
                )
                .with_help(help),
            );
        }
    }

    /// Reports a type mismatch unless `value` is a table.
    fn expect_table<'v, 'a>(
        &mut self,
        name: &str,
        value: &'v Value<'a>,
    ) -> Option<&'v DeTable<'a>> {
        let table = value.get_ref().as_table();
        if table.is_none() {
            self.type_mismatch(name, "a table", value);
        }
        table
    }

    /// Reports a type mismatch unless `value` is a string.
    fn expect_string<'v>(&mut self, name: &str, value: &'v Value<'_>) -> Option<&'v str> {
        let string = value.get_ref().as_str();
        if string.is_none() {
            self.type_mismatch(name, "a string", value);
        }
        string
    }

    /// Returns the strings in an array, reporting the array or any entry that
    /// has the wrong type.
    fn expect_string_array<'v>(
        &mut self,
        name: &str,
        value: &'v Value<'_>,
    ) -> Vec<(&'v str, Range<usize>)> {
        let Some(array) = value.get_ref().as_array() else {
            self.type_mismatch(name, "an array of strings", value);
            return Vec::new();
        };
        let mut strings = Vec::new();
        for item in array {
            match item.get_ref().as_str() {
                Some(string) => strings.push((string, item.span())),
                None => self.report(Diagnostic::new(
                    Rule::InvalidType,
                    format!(
                        "entries of `{name}` must be strings, found {}",
                        describe_type(item.get_ref())
                    ),
                    item.span(),
                )),
            }
        }
        strings
    }

    fn type_mismatch(&mut self, name: &str, expected: &str, value: &Value<'_>) {
        self.report(Diagnostic::new(
            Rule::InvalidType,
            format!(
                "`{name}` must be {expected}, found {}",
                describe_type(value.get_ref())
            ),
            value.span(),
        ));
    }

    /// Reports an invalid string value, pointing at the invalid part when known.
    fn invalid_string(&mut self, message: String, span: Range<usize>, problem: &Problem) {
        let span = match &problem.range {
            Some(range) => string_span(self.text, span, range.clone()),
            None => span,
        };
        self.report(Diagnostic::new(Rule::InvalidValue, message, span));
    }
}

/// The allowed name closest to `name`, if any is close enough to be a likely typo.
fn suggest<'a>(name: &str, allowed: &[&'a str]) -> Option<&'a str> {
    let normalized = name.to_ascii_lowercase().replace('_', "-");
    allowed
        .iter()
        .map(|candidate| (*candidate, strsim::jaro_winkler(&normalized, candidate)))
        .filter(|(_, score)| *score >= 0.85)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(candidate, _)| candidate)
}

fn quoted_list(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
pub(crate) mod test_support {
    /// Checks `text` and returns each diagnostic as (rule name, message, spanned text).
    pub fn diagnostics(text: &str) -> Vec<(&'static str, String, &str)> {
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|diagnostic| {
                (
                    diagnostic.rule.name(),
                    diagnostic.message,
                    &text[diagnostic.span],
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_keys() {
        let allowed = ["requires", "build-backend", "backend-path"];
        assert_eq!(suggest("build_backend", &allowed), Some("build-backend"));
        assert_eq!(suggest("require", &allowed), Some("requires"));
        assert_eq!(suggest("build-backed", &allowed), Some("build-backend"));
        assert_eq!(suggest("dependencies", &allowed), None);
    }
}
