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
    /// A key that the table's specification does not define.
    UnknownKey,
    /// A required key is missing.
    MissingKey,
    /// A value has the wrong TOML type.
    InvalidType,
    /// A value has the right type but is not valid, such as a malformed version.
    InvalidValue,
    /// `project.dynamic` is inconsistent with the rest of `[project]`.
    InvalidDynamic,
    /// Two names are the same after normalization.
    DuplicateName,
    /// Metadata that a specification has deprecated.
    DeprecatedMetadata,
    /// A feature that the build backend lacks in versions `[build-system]` allows.
    UnsupportedFeature,
}

impl Rule {
    /// The stable name shown in output, such as `invalid-toml`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::InvalidToml => "invalid-toml",
            Self::ByteOrderMark => "byte-order-mark",
            Self::Toml11Syntax => "toml-1-1-syntax",
            Self::UnknownKey => "unknown-key",
            Self::MissingKey => "missing-key",
            Self::InvalidType => "invalid-type",
            Self::InvalidValue => "invalid-value",
            Self::InvalidDynamic => "invalid-dynamic",
            Self::DuplicateName => "duplicate-name",
            Self::DeprecatedMetadata => "deprecated-metadata",
            Self::UnsupportedFeature => "unsupported-feature",
        }
    }

    /// How serious this rule's diagnostics are by default.
    ///
    /// Errors are problems that tools reject or that break builds. Specification
    /// violations that tools tolerate are reported as warnings instead.
    pub const fn severity(self) -> Severity {
        match self {
            Self::Toml11Syntax | Self::DeprecatedMetadata => Severity::Warning,
            Self::InvalidToml
            | Self::ByteOrderMark
            | Self::UnknownKey
            | Self::MissingKey
            | Self::InvalidType
            | Self::InvalidValue
            | Self::InvalidDynamic
            | Self::DuplicateName
            | Self::UnsupportedFeature => Severity::Error,
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
    /// Defaults to the rule's severity.
    pub severity: Severity,
    pub message: String,
    /// Byte range in the source text. An empty range points between two characters.
    pub span: Range<usize>,
    /// Related locations, such as where a duplicate was first defined.
    pub labels: Vec<Label>,
    /// Optional advice on how to fix the problem.
    pub help: Option<String>,
}

/// A related source location with an explanation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub span: Range<usize>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(rule: Rule, message: impl Into<String>, span: Range<usize>) -> Self {
        Self {
            rule,
            severity: rule.severity(),
            message: message.into(),
            span,
            labels: Vec::new(),
            help: None,
        }
    }

    #[must_use]
    pub fn with_label(mut self, span: Range<usize>, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Reports this diagnostic as a warning, for a violation tools tolerate.
    #[must_use]
    pub fn as_warning(mut self) -> Self {
        self.severity = Severity::Warning;
        self
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }
}
