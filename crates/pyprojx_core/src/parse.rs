//! Parsing TOML into a spanned document, collecting every syntax error.

use std::num::IntErrorKind;
use std::ops::Range;

use toml::Spanned;
use toml::de::{DeInteger, DeTable, DeValue};

use crate::diagnostic::{Diagnostic, Rule};

/// The result of parsing a TOML document.
#[derive(Debug)]
pub struct Parsed<'a> {
    /// The document, recovered as far as possible when there are errors. Every key
    /// and value carries its byte span in the source text.
    pub root: Spanned<DeTable<'a>>,
    /// Syntax errors, sorted by position.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses `text` as TOML, recovering from errors to report as many as possible.
///
/// Parsing uses the same `toml` crate as uv and Ruff and accepts TOML 1.1.
pub fn parse(text: &str) -> Parsed<'_> {
    let (root, errors) = DeTable::parse_recoverable(text);
    let mut diagnostics: Vec<Diagnostic> = errors
        .iter()
        .map(|error| {
            let span = error.span().unwrap_or(0..0);
            Diagnostic::new(
                Rule::InvalidToml,
                describe(text, &span, error.message()),
                span,
            )
        })
        .collect();
    check_integers(root.get_ref(), &mut diagnostics);
    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    Parsed { root, diagnostics }
}

/// Reports integers the parser accepted but that are not valid 64-bit integers.
///
/// The parser keeps integers as digit strings and validates them only on
/// conversion, so an empty `0x` or a number that overflows is not a parse error.
fn check_integers(table: &DeTable<'_>, diagnostics: &mut Vec<Diagnostic>) {
    for value in table.values() {
        check_integers_in_value(value, diagnostics);
    }
}

fn check_integers_in_value(value: &Spanned<DeValue<'_>>, diagnostics: &mut Vec<Diagnostic>) {
    match value.get_ref() {
        DeValue::Integer(integer) => {
            let span = value.span();
            let already_reported = diagnostics.iter().any(|diagnostic| {
                diagnostic.span.start < span.end && span.start < diagnostic.span.end
            });
            if !already_reported && let Some(message) = invalid_integer(integer) {
                diagnostics.push(Diagnostic::new(Rule::InvalidToml, message, span));
            }
        }
        DeValue::Array(array) => {
            for item in array {
                check_integers_in_value(item, diagnostics);
            }
        }
        DeValue::Table(table) => check_integers(table, diagnostics),
        DeValue::String(_) | DeValue::Float(_) | DeValue::Boolean(_) | DeValue::Datetime(_) => {}
    }
}

fn invalid_integer(integer: &DeInteger<'_>) -> Option<&'static str> {
    let error = i64::from_str_radix(integer.as_str(), integer.radix()).err()?;
    Some(match error.kind() {
        IntErrorKind::Empty => "missing digits in integer",
        IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => {
            "integer is out of range for a 64-bit signed integer"
        }
        _ => "invalid digit in integer",
    })
}

/// Rewrites parser messages that are misleading in context.
fn describe(text: &str, span: &Range<usize>, message: &str) -> String {
    if message.starts_with("string values must be quoted") && value_is_missing(text, span.start) {
        return "missing value".to_owned();
    }
    if message == "duplicate key" {
        let name = &text[span.clone()];
        return match table_header(text, span.start) {
            Some(header) => format!("duplicate table `{header}`"),
            None => format!("duplicate key `{name}`"),
        };
    }
    message
        .strip_suffix(", expected nothing")
        .unwrap_or(message)
        .to_owned()
}

/// Whether nothing but whitespace or a comment follows `offset` on its line.
fn value_is_missing(text: &str, offset: usize) -> bool {
    let rest = text[offset..].trim_start_matches([' ', '\t']);
    rest.is_empty() || rest.starts_with(['\n', '\r', '#'])
}

/// The `[header]` containing `offset`, if the line at `offset` is a table header.
fn table_header(text: &str, offset: usize) -> Option<&str> {
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line = text[line_start..].lines().next().unwrap_or_default().trim();
    let close = if line.starts_with("[[") {
        "]]"
    } else if line.starts_with('[') {
        "]"
    } else {
        return None;
    };
    let end = line.find(close)? + close.len();
    Some(&line[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns each diagnostic as (message, spanned source text).
    fn errors(text: &str) -> Vec<(String, &str)> {
        parse(text)
            .diagnostics
            .into_iter()
            .map(|diagnostic| (diagnostic.message, &text[diagnostic.span]))
            .collect()
    }

    #[test]
    fn valid_document_has_no_diagnostics() {
        let parsed = parse("[project]\nname = \"demo\"\n");
        assert!(parsed.diagnostics.is_empty());
        let project = parsed.root.get_ref()["project"]
            .get_ref()
            .as_table()
            .unwrap();
        let (key, value) = project.iter().next().unwrap();
        assert_eq!((key.get_ref().as_ref(), key.span()), ("name", 10..14));
        assert_eq!(value.span(), 17..23);
    }

    #[test]
    fn reports_every_error_in_source_order() {
        let text = "[tool.ruff]\nline-length = \nselect = [\"E\" \"F\"]\n";
        let diagnostics = parse(text).diagnostics;
        let found: Vec<_> = diagnostics
            .iter()
            .map(|d| (d.message.as_str(), d.span.clone()))
            .collect();
        assert_eq!(
            found,
            [
                ("missing value", 26..26),
                ("missing comma between array elements, expected `,`", 41..41),
            ]
        );
    }

    #[test]
    fn missing_value_before_comment_or_end_of_file() {
        assert_eq!(errors("a = # note\n")[0].0, "missing value");
        assert_eq!(errors("a =")[0].0, "missing value");
        assert_eq!(errors("a = \r\nb = 1\r\n")[0].0, "missing value");
    }

    #[test]
    fn unquoted_string_keeps_parser_message() {
        let found = errors("name = demo\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].0.starts_with("string values must be quoted"));
    }

    #[test]
    fn names_duplicate_keys_and_tables() {
        assert_eq!(
            errors("name = \"a\"\nname = \"b\"\n"),
            [("duplicate key `name`".to_owned(), "name")]
        );
        assert_eq!(
            errors("[tool.ruff]\n[tool.ruff]\n"),
            [("duplicate table `[tool.ruff]`".to_owned(), "ruff")]
        );
        assert_eq!(errors("[a]\n[a] # see [b]\n")[0].0, "duplicate table `[a]`");
        assert_eq!(
            errors("[x]\n[[x]] # see [b]\n")[0].0,
            "duplicate table `[[x]]`"
        );
    }

    #[test]
    fn strips_empty_expectations() {
        assert_eq!(
            errors("x = 01\n"),
            [("unexpected leading zero".to_owned(), "0")]
        );
    }

    #[test]
    fn reports_invalid_integers_the_parser_accepts() {
        assert_eq!(
            errors("a = 0x\nb = [9223372036854775808]\nc = { d = 1_0\u{660} }\n"),
            [
                ("missing digits in integer".to_owned(), "0x"),
                (
                    "integer is out of range for a 64-bit signed integer".to_owned(),
                    "9223372036854775808"
                ),
                ("invalid digit in integer".to_owned(), "1_0\u{660}"),
            ]
        );
        assert!(errors("a = -9223372036854775808\nb = 0xff\nc = 1_000\n").is_empty());
    }

    #[test]
    fn accepts_toml_1_1() {
        assert!(parse("a = { x = 1,\n  y = 2, }\n").diagnostics.is_empty());
    }
}
