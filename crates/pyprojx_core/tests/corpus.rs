//! Checks real-world `pyproject.toml` files (see `corpus/SOURCES.md`).
//!
//! Every diagnostic is recorded in a snapshot so that new checks misfiring on
//! real projects show up in review. These projects build successfully, so
//! errors are almost certainly false positives.

use std::fmt::Write;
use std::fs;
use std::path::Path;

use pyprojx_core::{Severity, check};

fn line_col(text: &str, offset: usize) -> (usize, usize) {
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

#[test]
fn corpus_diagnostics() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect();
    files.sort();
    assert!(files.len() >= 20, "corpus has only {} files", files.len());

    let mut report = String::new();
    let mut errors = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy();
        let checked = check(fs::read(path).unwrap());
        for diagnostic in &checked.diagnostics {
            let (line, column) = line_col(&checked.text, diagnostic.span.start);
            let entry = format!(
                "{name}:{line}:{column}: {}[{}] {}",
                if diagnostic.severity() == Severity::Error {
                    "error"
                } else {
                    "warning"
                },
                diagnostic.rule.name(),
                diagnostic.message
            );
            if diagnostic.severity() == Severity::Error {
                errors.push(entry.clone());
            }
            writeln!(report, "{entry}").unwrap();
        }
    }
    writeln!(
        report,
        "{} files, {} diagnostics",
        files.len(),
        report.lines().count()
    )
    .unwrap();
    insta::assert_snapshot!(report);
    assert!(
        errors.is_empty(),
        "errors in real projects:\n{}",
        errors.join("\n")
    );
}
