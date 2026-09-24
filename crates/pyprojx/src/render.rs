//! Rendering diagnostics as annotated source snippets.

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet};
use pyprojx_core::{Diagnostic, Severity};

/// Renders `diagnostic` against the source `text` of the file shown as `path`.
///
/// The output contains ANSI styles; write it through an `anstream` stream so they
/// are removed when the output is not a terminal or `NO_COLOR` is set.
pub fn render(diagnostic: &Diagnostic, text: &str, path: &str) -> String {
    let level = match diagnostic.severity() {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
    };
    let snippet = Snippet::source(text)
        .path(path)
        .fold(true)
        .annotation(AnnotationKind::Primary.span(diagnostic.span.clone()))
        .annotations(diagnostic.labels.iter().map(|label| {
            AnnotationKind::Context
                .span(label.span.clone())
                .label(label.message.as_str())
        }));
    let mut group = level
        .primary_title(diagnostic.message.as_str())
        .id(diagnostic.rule.name())
        .element(snippet);
    if let Some(help) = &diagnostic.help {
        group = group.element(Level::HELP.message(help.as_str()));
    }
    if let Some(note) = &diagnostic.note {
        group = group.element(Level::NOTE.message(note.as_str()));
    }
    Renderer::styled().render(&[group])
}
