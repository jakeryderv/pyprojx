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

fn toml_1_0_cases() -> HashSet<&'static Path> {
    toml_test_data::version("1.0.0").collect()
}

fn has_toml_1_1_warning(fixture: &[u8]) -> bool {
    check(fixture.to_vec())
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.rule == Rule::Toml11Syntax)
}

#[test]
fn toml_1_0_documents_have_no_toml_1_1_warnings() {
    let cases = toml_1_0_cases();
    let flagged: Vec<_> = toml_test_data::valid()
        .filter(|case| cases.contains(case.name()))
        .filter(|case| has_toml_1_1_warning(case.fixture()))
        .map(|case| case.name().display().to_string())
        .collect();
    assert!(flagged.is_empty(), "{}", flagged.join("\n"));
}

#[test]
fn documents_only_valid_in_toml_1_1_have_warnings() {
    let toml_1_0 = toml_1_0_cases();
    let toml_1_1 = toml_1_1_cases();
    // Cases TOML 1.0 rejects that are not invalid in TOML 1.1, and cases only in
    // the TOML 1.1 valid set (excluding copies of 1.0 spec examples).
    let newly_invalid: Vec<_> = toml_test_data::invalid()
        .filter(|case| toml_1_0.contains(case.name()) && !toml_1_1.contains(case.name()))
        .map(|case| (case.name().to_owned(), case.fixture().to_vec()))
        .collect();
    let newly_valid: Vec<_> = toml_test_data::valid()
        .filter(|case| toml_1_1.contains(case.name()) && !toml_1_0.contains(case.name()))
        .filter(|case| !case.name().starts_with("valid/spec-1.1.0"))
        .map(|case| (case.name().to_owned(), case.fixture().to_vec()))
        .collect();
    assert!(
        newly_valid.len() >= 5,
        "only {} newly valid cases",
        newly_valid.len()
    );

    let mut checked = 0;
    let mut missing = Vec::new();
    for (name, fixture) in newly_invalid.iter().chain(&newly_valid) {
        let checked_file = check(fixture.clone());
        let accepted = !checked_file
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.rule == Rule::InvalidToml);
        if !accepted {
            continue;
        }
        checked += 1;
        if !has_toml_1_1_warning(fixture) {
            missing.push(name.display().to_string());
        }
    }
    assert!(checked >= 10, "only {checked} cases accepted");
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}
