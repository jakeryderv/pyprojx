//! Conformance against the toml-test suite for TOML 1.1.

use std::collections::HashSet;
use std::path::Path;

use pyprojx_core::{Rule, check};

fn toml_1_1_cases() -> HashSet<&'static Path> {
    toml_test_data::version("1.1.0").collect()
}

#[test]
fn valid_documents_have_no_invalid_toml_diagnostics() {
    let cases = toml_1_1_cases();
    let mut checked = 0;
    let mut failures = Vec::new();
    for case in toml_test_data::valid().filter(|case| cases.contains(case.name())) {
        checked += 1;
        let checked_file = check(case.fixture().to_vec());
        let invalid: Vec<_> = checked_file
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.rule == Rule::InvalidToml)
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        if !invalid.is_empty() {
            failures.push(format!("{}: {invalid:?}", case.name().display()));
        }
    }
    assert!(checked > 100, "only {checked} valid cases found");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn invalid_documents_have_diagnostics_within_the_source() {
    let cases = toml_1_1_cases();
    let mut checked = 0;
    let mut failures = Vec::new();
    for case in toml_test_data::invalid().filter(|case| cases.contains(case.name())) {
        checked += 1;
        let checked_file = check(case.fixture().to_vec());
        if checked_file.diagnostics.is_empty() {
            failures.push(format!("{}: no diagnostics", case.name().display()));
        }
        for diagnostic in &checked_file.diagnostics {
            if checked_file.text.get(diagnostic.span.clone()).is_none() {
                failures.push(format!(
                    "{}: span {:?} is not a valid range of the text",
                    case.name().display(),
                    diagnostic.span
                ));
            }
        }
    }
    assert!(checked > 100, "only {checked} invalid cases found");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
