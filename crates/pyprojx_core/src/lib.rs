//! Core analysis for pyprojx: parsing, diagnostics, schemas, and version detection.
//!
//! This crate must stay free of CLI concerns such as argument parsing and
//! output rendering so that other front ends (a language server, or Python
//! bindings) can reuse it.

#[rustfmt::skip]
mod backend_data;
pub mod backends;
pub mod diagnostic;
pub mod document;
pub mod fix;
pub mod lock;
pub mod parse;
pub mod ruff;
#[rustfmt::skip]
mod ruff_data;
mod rules;
pub mod source;
pub mod standards;
pub mod toml_version;
pub mod tool;
pub mod trove;
#[rustfmt::skip]
mod trove_data;
pub mod ty;
#[rustfmt::skip]
mod ty_data;
pub mod uv;
#[rustfmt::skip]
mod uv_data;

pub use diagnostic::{Applicability, Diagnostic, Edit, Fix, Rule, Severity};
pub use lock::{Lock, LockKind};

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
    check_with_lock(bytes, None)
}

/// Checks the contents of a `pyproject.toml` file, checking tool settings
/// against the versions `lock` locks, if given.
pub fn check_with_lock(bytes: Vec<u8>, lock: Option<&Lock>) -> Checked {
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
    let parsed = parse::parse(&text);
    let is_valid_toml = parsed.diagnostics.is_empty();
    diagnostics.extend(parsed.diagnostics);
    if is_valid_toml {
        diagnostics.extend(toml_version::check_toml_1_1_syntax(&text));
        diagnostics.extend(rules::check(parsed.root.get_ref(), &text, lock));
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    Checked { text, diagnostics }
}
