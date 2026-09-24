//! Detecting syntax that requires TOML 1.1.
//!
//! uv and Ruff accept TOML 1.1, but `tomllib` in Python 3.14 and earlier only
//! accepts TOML 1.0. Build backends that read `pyproject.toml` with it, such as
//! hatchling, fail on those Python versions.

use std::ops::Range;

use toml_parser::decoder::Encoding;
use toml_parser::parser::{EventReceiver, parse_document};
use toml_parser::{ErrorSink, Source, Span};

use crate::diagnostic::{Diagnostic, Rule};

const HELP: &str = "uv and Ruff accept TOML 1.1, but tomllib in Python 3.14 and earlier rejects it, so build backends that use it (such as hatchling) fail on those versions";

/// Reports syntax in `text` that is valid TOML 1.1 but not TOML 1.0.
///
/// Expects `text` to be valid TOML; syntax errors are ignored.
pub fn check_toml_1_1_syntax(text: &str) -> Vec<Diagnostic> {
    let source = Source::new(text);
    let tokens: Vec<_> = source.lex().collect();
    let mut detector = Detector {
        text,
        containers: Vec::new(),
        diagnostics: Vec::new(),
    };
    parse_document(&tokens, &mut detector, &mut ());
    detector.diagnostics
}

enum Container {
    Array,
    InlineTable {
        open: Span,
        /// Whether the table has already been reported as spanning lines.
        multiline: bool,
        /// A comma not yet followed by another entry.
        pending_comma: Option<Span>,
    },
}

struct Detector<'t> {
    text: &'t str,
    containers: Vec<Container>,
    diagnostics: Vec<Diagnostic>,
}

impl Detector<'_> {
    fn report(&mut self, message: &str, span: Range<usize>) {
        self.diagnostics
            .push(Diagnostic::new(Rule::Toml11Syntax, message, span).with_help(HELP));
    }

    /// Records that the current inline table has content after its last comma.
    fn entry(&mut self) {
        if let Some(Container::InlineTable { pending_comma, .. }) = self.containers.last_mut() {
            *pending_comma = None;
        }
    }

    /// Reports a newline or comment directly inside an inline table.
    fn line_break(&mut self) {
        if let Some(Container::InlineTable {
            open, multiline, ..
        }) = self.containers.last_mut()
            && !*multiline
        {
            *multiline = true;
            let span = open.start()..open.end();
            self.report(
                "inline table spans multiple lines, which requires TOML 1.1",
                span,
            );
        }
    }

    fn check_escapes(&mut self, span: Span, encoding: Option<Encoding>) {
        if !matches!(
            encoding,
            Some(Encoding::BasicString | Encoding::MlBasicString)
        ) {
            return;
        }
        let raw = &self.text.as_bytes()[span.start()..span.end()];
        let mut index = 0;
        while index < raw.len() {
            if raw[index] != b'\\' {
                index += 1;
                continue;
            }
            let start = span.start() + index;
            match raw.get(index + 1) {
                Some(b'e') => self.report("`\\e` escape requires TOML 1.1", start..start + 2),
                Some(b'x') => {
                    let end = (start + 4).min(span.end());
                    self.report("`\\xHH` escape requires TOML 1.1", start..end);
                }
                _ => {}
            }
            index += 2;
        }
    }

    fn check_time(&mut self, span: Span, encoding: Option<Encoding>) {
        if encoding.is_some() {
            return;
        }
        let raw = &self.text[span.start()..span.end()];
        if let Some(offset) = time_without_seconds(raw) {
            let start = span.start() + offset;
            self.report("time without seconds requires TOML 1.1", start..start + 5);
        }
    }
}

/// The offset of an `HH:MM` time without seconds in a datetime or time value.
fn time_without_seconds(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let is_date = bytes.len() > 10 && bytes[4] == b'-' && bytes[7] == b'-';
    let start = if is_date && matches!(bytes[10], b'T' | b't' | b' ') {
        11
    } else if bytes.get(2) == Some(&b':') {
        0
    } else {
        return None;
    };
    let time = bytes.get(start..start + 5)?;
    let is_hours_minutes = time[..2].iter().all(u8::is_ascii_digit)
        && time[2] == b':'
        && time[3..].iter().all(u8::is_ascii_digit);
    (is_hours_minutes && bytes.get(start + 5) != Some(&b':')).then_some(start)
}

impl EventReceiver for Detector<'_> {
    fn inline_table_open(&mut self, span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.entry();
        self.containers.push(Container::InlineTable {
            open: span,
            multiline: false,
            pending_comma: None,
        });
        true
    }

    fn inline_table_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        if let Some(Container::InlineTable {
            pending_comma: Some(comma),
            ..
        }) = self.containers.pop()
        {
            self.report(
                "trailing comma in inline table requires TOML 1.1",
                comma.start()..comma.end(),
            );
        }
    }

    fn array_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.entry();
        self.containers.push(Container::Array);
        true
    }

    fn array_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.containers.pop();
    }

    fn simple_key(&mut self, span: Span, encoding: Option<Encoding>, _error: &mut dyn ErrorSink) {
        self.entry();
        self.check_escapes(span, encoding);
    }

    fn scalar(&mut self, span: Span, encoding: Option<Encoding>, _error: &mut dyn ErrorSink) {
        self.entry();
        self.check_escapes(span, encoding);
        self.check_time(span, encoding);
    }

    fn value_sep(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        if let Some(Container::InlineTable { pending_comma, .. }) = self.containers.last_mut() {
            *pending_comma = Some(span);
        }
    }

    fn newline(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.line_break();
    }

    fn comment(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.line_break();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns each diagnostic as (message, spanned source text).
    fn found(text: &str) -> Vec<(String, &str)> {
        check_toml_1_1_syntax(text)
            .into_iter()
            .map(|diagnostic| (diagnostic.message, &text[diagnostic.span]))
            .collect()
    }

    #[test]
    fn toml_1_0_has_no_diagnostics() {
        let text = r#"
a = { x = [
  1, # comment in an array
  2,
], y = """multi
line""" }
b = "A\n\\e \\x"
c = 1979-05-27 07:32:00
d = 07:32:00.5
e = 1979-05-27T07:32:00+01:00
f = 1979-05-27
"#;
        assert_eq!(found(text), []);
    }

    #[test]
    fn multiline_inline_table() {
        let multiline = "inline table spans multiple lines, which requires TOML 1.1".to_owned();
        assert_eq!(
            found("a = { x = 1,\n  y = { z = 2,\n } }\n"),
            [
                (multiline.clone(), "{"),
                (multiline, "{"),
                (
                    "trailing comma in inline table requires TOML 1.1".to_owned(),
                    ","
                ),
            ]
        );
    }

    #[test]
    fn comment_in_inline_table() {
        assert_eq!(found("a = { # note\n x = 1 }\n").len(), 1);
    }

    #[test]
    fn trailing_comma_in_inline_table() {
        assert_eq!(
            found("a = { x = 1, }\n"),
            [(
                "trailing comma in inline table requires TOML 1.1".to_owned(),
                ","
            )]
        );
    }

    #[test]
    fn new_escapes_in_values_and_keys() {
        assert_eq!(
            found(r#""k\x41" = "\e[0m""#),
            [
                ("`\\xHH` escape requires TOML 1.1".to_owned(), r"\x41"),
                ("`\\e` escape requires TOML 1.1".to_owned(), r"\e"),
            ]
        );
    }

    #[test]
    fn times_without_seconds() {
        let found = found("a = 07:32\nb = 1979-05-27 07:32\nc = 1979-05-27T07:32Z\n");
        assert_eq!(found.len(), 3);
        assert!(found.iter().all(|(_, span)| *span == "07:32"));
    }
}
