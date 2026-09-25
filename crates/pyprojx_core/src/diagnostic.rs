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
    /// A feature that a build backend or tool lacks in versions the project allows.
    UnsupportedFeature,
    /// A tool setting, such as an option or a rule code, that the tool has deprecated.
    DeprecatedSetting,
    /// A tool setting that has no effect, such as selecting a preview rule
    /// without preview.
    IneffectiveSetting,
    /// A tool's settings were not checked because pyprojx does not know the
    /// tool versions the project uses, such as releases newer than its data.
    UnknownVersion,
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
            Self::DeprecatedSetting => "deprecated-setting",
            Self::IneffectiveSetting => "ineffective-setting",
            Self::UnknownVersion => "unknown-version",
        }
    }

    /// How serious this rule's diagnostics are by default.
    ///
    /// Errors are problems that tools reject or that break builds. Specification
    /// violations that tools tolerate are reported as warnings instead.
    pub const fn severity(self) -> Severity {
        match self {
            Self::Toml11Syntax
            | Self::DeprecatedMetadata
            | Self::DeprecatedSetting
            | Self::IneffectiveSetting
            | Self::UnknownVersion => Severity::Warning,
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
    /// Optional context, such as the tool versions a setting was checked against.
    pub note: Option<String>,
    /// An edit that fixes the problem, if pyprojx knows one.
    pub fix: Option<Fix>,
}

/// Edits to the source text that fix a problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub applicability: Applicability,
    /// Non-overlapping edits, applied together or not at all.
    pub edits: Vec<Edit>,
}

/// Whether a fix can be applied without review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Applicability {
    /// The fix may change what the configuration means, such as renaming a
    /// misspelled key to a guess, so it is applied only with `--unsafe-fixes`.
    Unsafe,
    /// The tool reads the fixed configuration as it read the original, or the
    /// part removed had no effect, so `--fix` applies it.
    Safe,
}

/// Replaces a range of the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub replacement: String,
}

impl Fix {
    pub fn safe(edits: Vec<Edit>) -> Self {
        Self {
            applicability: Applicability::Safe,
            edits,
        }
    }

    pub fn unsafe_(edits: Vec<Edit>) -> Self {
        Self {
            applicability: Applicability::Unsafe,
            edits,
        }
    }
}

impl Edit {
    pub fn replace(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self::replace(range, "")
    }
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
            note: None,
            fix: None,
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

    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
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
