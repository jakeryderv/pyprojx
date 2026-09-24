//! `[build-system]`, from the pyproject.toml specification and PEP 517.

use toml::de::DeTable;

use super::{Context, is_object_reference};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::standards::check_requirement;

const KEYS: &[&str] = &["requires", "build-backend", "backend-path"];

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((key, value)) = get(root, "build-system") else {
        // The table is optional; tools fall back to setuptools.
        return;
    };
    let Some(table) = context.expect_table("build-system", value) else {
        return;
    };
    context.check_keys(table, "`[build-system]`", KEYS);

    match get(table, "requires") {
        Some((_, requires)) => {
            for (requirement, span) in context.expect_string_array("build-system.requires", requires)
            {
                if let Err(problem) = check_requirement(requirement) {
                    let message = format!("invalid requirement: {}", problem.message);
                    context.invalid_string(message, span, &problem);
                }
            }
        }
        None => context.report(
            Diagnostic::new(
                Rule::MissingKey,
                "`[build-system]` is missing the required key `requires`",
                key.span(),
            )
            .with_help("list the packages needed to build the project, for example `requires = [\"hatchling\"]`"),
        ),
    }

    if let Some((_, backend)) = get(table, "build-backend")
        && let Some(reference) = context.expect_string("build-system.build-backend", backend)
        && !is_object_reference(reference)
    {
        context.report(
            Diagnostic::new(
                Rule::InvalidValue,
                format!("invalid build backend `{reference}`"),
                backend.span(),
            )
            .with_help(
                "use a Python object reference such as `package.module` or `package.module:object`",
            ),
        );
    }

    if let Some((_, paths)) = get(table, "backend-path") {
        for (path, span) in context.expect_string_array("build-system.backend-path", paths) {
            if !is_contained_relative_path(path) {
                context.report(
                    Diagnostic::new(
                        Rule::InvalidValue,
                        format!("backend path `{path}` is outside the project"),
                        span,
                    )
                    .with_help("backend paths must be relative to the directory containing pyproject.toml and stay inside it"),
                );
            }
        }
    }
}

/// Whether `path` is relative and does not leave its base directory.
fn is_contained_relative_path(path: &str) -> bool {
    let is_absolute = path.starts_with(['/', '\\'])
        || path
            .as_bytes()
            .get(1..3)
            .is_some_and(|rest| rest[0] == b':' && matches!(rest[1], b'/' | b'\\'));
    if is_absolute {
        return false;
    }
    let mut depth: usize = 0;
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => match depth.checked_sub(1) {
                Some(parent) => depth = parent,
                None => return false,
            },
            _ => depth += 1,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::test_support::diagnostics;

    #[test]
    fn valid_build_system() {
        let text = r#"
[build-system]
requires = ["hatchling>=1.27", "hatch-vcs"]
build-backend = "hatchling.build"
backend-path = ["_build", "./tools/../backend"]
"#;
        assert_eq!(diagnostics(text), []);
        assert_eq!(
            diagnostics("[project]\nname = \"x\"\nversion = \"1\"\n"),
            []
        );
    }

    #[test]
    fn missing_requires_points_at_header() {
        assert_eq!(
            diagnostics("[build-system]\nbuild-backend = \"hatchling.build\"\n"),
            [(
                "missing-key",
                "`[build-system]` is missing the required key `requires`".to_owned(),
                "build-system"
            )]
        );
    }

    #[test]
    fn unknown_keys_and_types() {
        assert_eq!(
            diagnostics("[build-system]\nrequires = \"setuptools\"\nbuild_backend = 1\n"),
            [
                (
                    "invalid-type",
                    "`build-system.requires` must be an array of strings, found a string"
                        .to_owned(),
                    "\"setuptools\""
                ),
                (
                    "unknown-key",
                    "unknown key `build_backend` in `[build-system]`".to_owned(),
                    "build_backend"
                ),
            ]
        );
        assert_eq!(
            diagnostics("build-system = 1\n"),
            [(
                "invalid-type",
                "`build-system` must be a table, found an integer".to_owned(),
                "1"
            )]
        );
    }

    #[test]
    fn invalid_requirements_point_inside_the_string() {
        assert_eq!(
            diagnostics("[build-system]\nrequires = [\"setuptools >= 77.*\", 3]\n"),
            [
                (
                    "invalid-value",
                    "invalid requirement: Operator >= cannot be used with a wildcard version specifier"
                        .to_owned(),
                    ">= 77.*"
                ),
                (
                    "invalid-type",
                    "entries of `build-system.requires` must be strings, found an integer"
                        .to_owned(),
                    "3"
                ),
            ]
        );
    }

    #[test]
    fn backend_reference_and_paths() {
        let text = "[build-system]\nrequires = []\nbuild-backend = \"hatchling-build\"\nbackend-path = [\"../outside\", \"/abs\", \"C:\\\\x\"]\n";
        let found: Vec<_> = diagnostics(text).into_iter().map(|d| d.2).collect();
        assert_eq!(
            found,
            [
                "\"hatchling-build\"",
                "\"../outside\"",
                "\"/abs\"",
                "\"C:\\\\x\""
            ]
        );
    }

    #[test]
    fn object_references() {
        assert!(is_object_reference("setuptools.build_meta:__legacy__"));
        assert!(is_object_reference("backend"));
        assert!(!is_object_reference("backend:"));
        assert!(!is_object_reference("a..b"));
        assert!(!is_object_reference("1backend"));
    }
}
