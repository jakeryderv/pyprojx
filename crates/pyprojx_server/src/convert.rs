//! Converting pyprojx's diagnostics to the protocol's.

use lsp_types::{Code, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Message, Uri};
use pyprojx_core::{Diagnostic, Severity};

use crate::position::LineIndex;

/// A diagnostic in the protocol's terms: the rule name as its code, labels as
/// related information, and the help and note after the message, since the
/// protocol has no place for them.
pub fn diagnostic(
    diagnostic: &Diagnostic,
    uri: &Uri,
    index: &LineIndex<'_>,
) -> lsp_types::Diagnostic {
    let mut message = diagnostic.message.clone();
    if let Some(help) = &diagnostic.help {
        message.push_str(&format!("\nhelp: {help}"));
    }
    if let Some(note) = &diagnostic.note {
        message.push_str(&format!("\nnote: {note}"));
    }
    let related = diagnostic
        .labels
        .iter()
        .map(|label| DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range: index.range(label.span.clone()),
            },
            message: label.message.clone(),
        })
        .collect::<Vec<_>>();
    lsp_types::Diagnostic {
        range: index.range(diagnostic.span.clone()),
        severity: Some(match diagnostic.severity() {
            Severity::Error => DiagnosticSeverity::Error,
            Severity::Warning => DiagnosticSeverity::Warning,
        }),
        code: Some(Code::String(diagnostic.rule.name().to_owned())),
        source: Some("pyprojx".to_owned()),
        message: Message::String(message),
        related_information: (!related.is_empty()).then_some(related),
        ..lsp_types::Diagnostic::default()
    }
}
