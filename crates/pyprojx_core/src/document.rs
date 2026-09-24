//! Helpers for reading a parsed TOML document with source spans.

use std::ops::Range;

use toml::Spanned;
use toml::de::{DeString, DeTable, DeValue};

/// A key with its span in the source text.
pub type Key<'a> = Spanned<DeString<'a>>;
/// A value with its span in the source text.
pub type Value<'a> = Spanned<DeValue<'a>>;

/// Looks up `key` in `table`, returning the key and value with their spans.
pub fn get<'t, 'a>(table: &'t DeTable<'a>, key: &str) -> Option<(&'t Key<'a>, &'t Value<'a>)> {
    table
        .iter()
        .find(|(candidate, _)| candidate.get_ref() == key)
}

/// Describes a value's type for messages, such as "a string" or "an array".
pub fn describe_type(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a datetime",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

/// Maps `inner`, a byte range within a string value's contents, to a span in
/// the source text.
///
/// This is exact for single-line strings without escapes, where the contents
/// appear verbatim between the quotes. Otherwise it returns the whole value.
pub fn string_span(text: &str, value_span: Range<usize>, inner: Range<usize>) -> Range<usize> {
    let raw = &text[value_span.clone()];
    let verbatim = raw.len() >= 2
        && !raw.starts_with("\"\"\"")
        && !raw.starts_with("'''")
        && (raw.starts_with('\'') || (raw.starts_with('"') && !raw.contains('\\')));
    let content_len = raw.len().saturating_sub(2);
    if verbatim && inner.start <= inner.end && inner.end <= content_len {
        let start = value_span.start + 1;
        start + inner.start..start + inner.end
    } else {
        value_span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ranges_inside_simple_strings() {
        let text = r#"a = "requests >= 2.3.*""#;
        let span = 4..text.len();
        assert_eq!(&text[string_span(text, span.clone(), 9..17)], ">= 2.3.*");
        let text = "a = 'x y'";
        assert_eq!(&text[string_span(text, 4..9, 2..3)], "y");
    }

    #[test]
    fn falls_back_to_whole_value() {
        let text = r#"a = "x\ty""#;
        assert_eq!(string_span(text, 4..10, 2..3), 4..10);
        let text = "a = \"\"\"x y\"\"\"";
        assert_eq!(string_span(text, 4..13, 2..3), 4..13);
    }
}
