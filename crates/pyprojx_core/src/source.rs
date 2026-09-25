//! Decoding file contents into source text.

use crate::diagnostic::{Diagnostic, Edit, Fix, Rule};

const BYTE_ORDER_MARK: char = '\u{feff}';

/// Decodes `bytes` as UTF-8.
///
/// On invalid UTF-8, returns the text with invalid sequences replaced by U+FFFD
/// (so it can still be displayed) and a diagnostic pointing at the first invalid
/// sequence. Its span is valid in the returned text.
pub fn decode(bytes: Vec<u8>) -> (String, Option<Diagnostic>) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(error) => {
            let start = error.utf8_error().valid_up_to();
            let text = String::from_utf8_lossy(error.as_bytes()).into_owned();
            let span = start..start + char::REPLACEMENT_CHARACTER.len_utf8();
            let diagnostic = Diagnostic::new(Rule::InvalidToml, "invalid UTF-8", span)
                .with_help("TOML files must be encoded as UTF-8");
            (text, Some(diagnostic))
        }
    }
}

/// Reports a UTF-8 byte order mark at the start of `text`.
///
/// The `toml` crate accepts it, but `tomllib` (used by pip and most build
/// backends) and uv reject it.
pub fn check_byte_order_mark(text: &str) -> Option<Diagnostic> {
    text.starts_with(BYTE_ORDER_MARK).then(|| {
        Diagnostic::new(
            Rule::ByteOrderMark,
            "file starts with a UTF-8 byte order mark",
            0..BYTE_ORDER_MARK.len_utf8(),
        )
        .with_help("tomllib (used by pip and most build backends) and uv reject it; save the file as UTF-8 without a byte order mark")
        // Every reader treats the file the same without it.
        .with_fix(Fix::safe(vec![Edit::delete(0..BYTE_ORDER_MARK.len_utf8())]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_is_unchanged() {
        assert_eq!(decode(b"a = 1\n".to_vec()), ("a = 1\n".to_owned(), None));
    }

    #[test]
    fn invalid_utf8_points_at_first_invalid_sequence() {
        let (text, diagnostic) = decode(b"a = \"\xff\"\n".to_vec());
        let diagnostic = diagnostic.unwrap();
        assert_eq!(diagnostic.message, "invalid UTF-8");
        assert_eq!(&text[diagnostic.span], "\u{fffd}");
    }

    #[test]
    fn detects_byte_order_mark() {
        let diagnostic = check_byte_order_mark("\u{feff}a = 1\n").unwrap();
        assert_eq!(diagnostic.span, 0..3);
        assert!(check_byte_order_mark("a = 1\n\u{feff}").is_none());
    }
}
