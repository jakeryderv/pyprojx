//! Diagnostics reported by pyprojx checks.

use std::ops::Range;

/// A check that can report diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// The file is not valid UTF-8 or not valid TOML.
    InvalidToml,
    /// The file starts with a UTF-8 byte order mark, which common readers reject.
    ByteOrderMark,
    /// The file uses syntax that requires TOML 1.1.
    Toml11Syntax,
}

impl Rule {
    /// The stable name shown in output, such as `invalid-toml`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::InvalidToml => "invalid-toml",
            Self::ByteOrderMark => "byte-order-mark",
            Self::Toml11Syntax => "toml-1-1-syntax",
        }
    }

    /// How serious this rule's diagnostics are.
    pub const fn severity(self) -> Severity {
        match self {
            Self::InvalidToml | Self::ByteOrderMark => Severity::Error,
            Self::Toml11Syntax => Severity::Warning,
        }
    }
}

/// How serious a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Warning,
    Error,
}

/// A problem found in a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub rule: Rule,
    pub message: String,
    /// Byte range in the source text. An empty range points between two characters.
    pub span: Range<usize>,
    /// Optional advice on how to fix the problem.
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(rule: Rule, message: impl Into<String>, span: Range<usize>) -> Self {
        Self {
            rule,
            message: message.into(),
            span,
            help: None,
        }
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn severity(&self) -> Severity {
        self.rule.severity()
    }
}
