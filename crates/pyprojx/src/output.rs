//! Writing the diagnostics of checked files in each output format.

use std::fmt::Write;

use pyprojx_core::{Applicability, Diagnostic, Severity};

use crate::check::Report;
use crate::render;

/// How `pyprojx check` writes diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Annotated source snippets and a summary.
    #[default]
    Full,
    /// GitHub Actions workflow commands, which show as annotations on pull
    /// requests.
    Github,
    /// A JSON array of diagnostics. Experimental: fields may change.
    Json,
}

/// Annotated snippets for each diagnostic, then what was fixed, the counts,
/// and what is fixable.
pub fn full(reports: &[Report], fixable: Applicability) -> String {
    let mut output = String::new();
    for report in reports {
        for diagnostic in &report.diagnostics {
            let snippet = render::render(diagnostic, &report.text, &report.display);
            let _ = writeln!(output, "{snippet}\n");
        }
    }
    let diagnostics: Vec<&Diagnostic> = reports.iter().flat_map(|r| &r.diagnostics).collect();
    let fixed: usize = reports.iter().map(|report| report.fixed).sum();
    if fixed > 0 {
        let _ = writeln!(output, "Fixed {}.", plural(fixed, "problem"));
    }
    let errors = count(&diagnostics, Severity::Error);
    let warnings = count(&diagnostics, Severity::Warning);
    let _ = writeln!(output, "{}", summary(errors, warnings));
    if let Some(line) = fixable_summary(&diagnostics, fixable) {
        let _ = writeln!(output, "{line}");
    }
    output
}

/// A GitHub Actions workflow command for each diagnostic, such as
/// `::error file=pyproject.toml,line=2,...::message`.
pub fn github(reports: &[Report]) -> String {
    let mut output = String::new();
    for report in reports {
        for diagnostic in &report.diagnostics {
            let level = match diagnostic.severity() {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let (line, column) = position(&report.text, diagnostic.span.start);
            let (end_line, end_column) = position(&report.text, diagnostic.span.end);
            let mut message = diagnostic.message.clone();
            for extra in [&diagnostic.help, &diagnostic.note].into_iter().flatten() {
                message.push_str(&format!("\n{extra}"));
            }
            let _ = writeln!(
                output,
                "::{level} title={},file={},line={line},col={column},endLine={end_line},endColumn={end_column}::{}",
                escape_property(&format!("pyprojx ({})", diagnostic.rule.name())),
                escape_property(&report.display),
                escape_data(&message),
            );
        }
    }
    output
}

/// A JSON array with an object for each diagnostic.
pub fn json(reports: &[Report]) -> String {
    let mut entries = Vec::new();
    for report in reports {
        for diagnostic in &report.diagnostics {
            let (line, column) = position(&report.text, diagnostic.span.start);
            let (end_line, end_column) = position(&report.text, diagnostic.span.end);
            let severity = match diagnostic.severity() {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let optional =
                |text: &Option<String>| text.as_deref().map_or("null".to_owned(), json_string);
            let fix = match &diagnostic.fix {
                None => "null".to_owned(),
                Some(fix) => format!(
                    "{{\"title\": {}, \"applicability\": {}}}",
                    json_string(&fix.title),
                    json_string(match fix.applicability {
                        Applicability::Safe => "safe",
                        Applicability::Unsafe => "unsafe",
                    })
                ),
            };
            entries.push(format!(
                "  {{\"file\": {}, \"rule\": {}, \"severity\": {}, \"message\": {}, \"start\": {{\"line\": {line}, \"column\": {column}}}, \"end\": {{\"line\": {end_line}, \"column\": {end_column}}}, \"help\": {}, \"note\": {}, \"fix\": {fix}}}",
                json_string(&report.display),
                json_string(diagnostic.rule.name()),
                json_string(severity),
                json_string(&diagnostic.message),
                optional(&diagnostic.help),
                optional(&diagnostic.note),
            ));
        }
    }
    if entries.is_empty() {
        "[]\n".to_owned()
    } else {
        format!("[\n{}\n]\n", entries.join(",\n"))
    }
}

/// A unified diff of each file whose fixes change it.
pub fn diff(reports: &[Report]) -> String {
    let mut output = String::new();
    for report in reports.iter().filter(|report| report.fixed > 0) {
        let diff = similar::TextDiff::from_lines(&report.original, &report.text);
        let _ = write!(
            output,
            "{}",
            diff.unified_diff().header(&report.display, &report.display)
        );
    }
    output
}

/// The 1-based line and column, in characters, of a byte offset in `text`.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    (line, column)
}

/// Escapes a workflow command's message.
fn escape_data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a workflow command's property value.
fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

/// `text` as a JSON string.
fn json_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for char in text.chars() {
        match char {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            char if u32::from(char) < 0x20 => {
                let _ = write!(output, "\\u{:04x}", u32::from(char));
            }
            char => output.push(char),
        }
    }
    output.push('"');
    output
}

fn count(diagnostics: &[&Diagnostic], severity: Severity) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity() == severity)
        .count()
}

pub fn plural(count: usize, noun: &str) -> String {
    let suffix = match (count, noun.ends_with('x')) {
        (1, _) => "",
        (_, true) => "es",
        _ => "s",
    };
    format!("{count} {noun}{suffix}")
}

/// Says how many of `diagnostics` `--fix` would fix, and how many more
/// `--unsafe-fixes` would, if any. With `--unsafe-fixes`, `fixable` is
/// [`Applicability::Unsafe`], and they are counted together.
fn fixable_summary(diagnostics: &[&Diagnostic], fixable: Applicability) -> Option<String> {
    let count = |applicability: Applicability| {
        diagnostics
            .iter()
            .filter(|d| {
                d.fix
                    .as_ref()
                    .is_some_and(|fix| fix.applicability == applicability)
            })
            .count()
    };
    let (safe, unsafe_) = (count(Applicability::Safe), count(Applicability::Unsafe));
    let fixable_with = |count: usize, flags: &str| {
        let verb = if count == 1 { "is" } else { "are" };
        format!("{} {verb} fixable with `{flags}`", plural(count, "problem"))
    };
    match (fixable, safe, unsafe_) {
        (_, 0, 0) => None,
        (Applicability::Unsafe, safe, unsafe_) => Some(format!(
            "{}.",
            fixable_with(safe + unsafe_, "--fix --unsafe-fixes")
        )),
        (Applicability::Safe, 0, unsafe_) => Some(format!(
            "{}.",
            fixable_with(unsafe_, "--fix --unsafe-fixes")
        )),
        (Applicability::Safe, safe, 0) => Some(format!("{}.", fixable_with(safe, "--fix"))),
        (Applicability::Safe, safe, unsafe_) => Some(format!(
            "{} ({unsafe_} more with `--unsafe-fixes`).",
            fixable_with(safe, "--fix")
        )),
    }
}

fn summary(errors: usize, warnings: usize) -> String {
    match (errors, warnings) {
        (0, 0) => "All checks passed!".to_owned(),
        (errors, 0) => format!("Found {}.", plural(errors, "error")),
        (0, warnings) => format!("Found {}.", plural(warnings, "warning")),
        (errors, warnings) => format!(
            "Found {} and {}.",
            plural(errors, "error"),
            plural(warnings, "warning")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_counts() {
        assert_eq!(summary(0, 0), "All checks passed!");
        assert_eq!(summary(1, 0), "Found 1 error.");
        assert_eq!(summary(0, 2), "Found 2 warnings.");
        assert_eq!(summary(2, 1), "Found 2 errors and 1 warning.");
    }

    #[test]
    fn escapes() {
        assert_eq!(json_string("a \"b\"\n\\\u{1}"), r#""a \"b\"\n\\\u0001""#);
        assert_eq!(escape_data("50%\nnext"), "50%25%0Anext");
        assert_eq!(escape_property("a:b,c"), "a%3Ab%2Cc");
    }

    #[test]
    fn positions_count_characters() {
        let text = "a = \"é\"\nb = 1\n";
        assert_eq!(position(text, 0), (1, 1));
        assert_eq!(position(text, text.find('"').unwrap() + 3), (1, 7));
        assert_eq!(position(text, text.find('b').unwrap()), (2, 1));
    }
}
