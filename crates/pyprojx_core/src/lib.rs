//! Core analysis for pyprojx: parsing, diagnostics, schemas, and version detection.
//!
//! This crate must stay free of CLI concerns such as argument parsing and
//! output rendering so that other front ends (a language server, or Python
//! bindings) can reuse it.

pub mod diagnostic;
pub mod parse;
pub mod source;

pub use diagnostic::{Diagnostic, Rule, Severity};

/// The result of checking a file.
#[derive(Debug)]
pub struct Checked {
    /// The decoded source text that diagnostic spans refer to.
    pub text: String,
    /// Diagnostics sorted by position.
    pub diagnostics: Vec<Diagnostic>,
}

/// Checks the contents of a `pyproject.toml` file.
pub fn check(bytes: Vec<u8>) -> Checked {
    let (text, encoding_error) = source::decode(bytes);
    if let Some(diagnostic) = encoding_error {
        // Further diagnostics on text with replaced characters could be misleading.
        return Checked {
            text,
            diagnostics: vec![diagnostic],
        };
    }

    let mut diagnostics: Vec<Diagnostic> =
        source::check_byte_order_mark(&text).into_iter().collect();
    diagnostics.extend(parse::parse(&text).diagnostics);
    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    Checked { text, diagnostics }
}
