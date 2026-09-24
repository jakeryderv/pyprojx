//! Validation of Python packaging standards: names, versions, specifiers, and
//! dependency specifiers.
//!
//! This module is the only user of uv's crates, whose APIs are unstable, so
//! updates to them stay contained here. Using uv's parsers means pyprojx accepts
//! and rejects exactly what uv does.

use std::ops::Range;
use std::str::FromStr;

use uv_normalize::PackageName;
use uv_pep440::{Version, VersionSpecifier};
use uv_pep508::{Requirement, VerbatimUrl};

/// Why a string is invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    /// The invalid part as a byte range within the string, when known.
    pub range: Option<Range<usize>>,
}

impl Problem {
    fn new(message: impl Into<String>, range: Option<Range<usize>>) -> Self {
        Self {
            message: message.into(),
            range,
        }
    }
}

/// Checks a dependency specifier, such as `requests>=2.31; python_version < "3.12"`.
pub fn check_requirement(value: &str) -> Result<(), Problem> {
    Requirement::<VerbatimUrl>::from_str(value)
        .map(|_| ())
        .map_err(|error| {
            let end = (error.start + error.len).min(value.len());
            Problem::new(error.message.to_string(), Some(error.start..end))
        })
}

/// Checks a version, such as `1.0.post1`.
pub fn check_version(value: &str) -> Result<(), Problem> {
    Version::from_str(value)
        .map(|_| ())
        .map_err(|error| Problem::new(error.to_string(), None))
}

/// Checks comma-separated version specifiers, such as `>=3.11,<4`.
///
/// Each specifier is parsed separately so the invalid one can be located.
pub fn check_specifiers(value: &str) -> Result<(), Problem> {
    if value.trim().is_empty() {
        return Err(Problem::new(
            "expected at least one version specifier",
            None,
        ));
    }
    let mut start = 0;
    for part in value.split(',') {
        let trimmed = part.trim();
        let offset = start + (part.len() - part.trim_start().len());
        if let Err(error) = VersionSpecifier::from_str(trimmed) {
            let range = offset..offset + trimmed.len();
            let message = if trimmed.is_empty() {
                "empty version specifier".to_owned()
            } else {
                error.to_string()
            };
            return Err(Problem::new(message, Some(range)));
        }
        start += part.len() + 1;
    }
    Ok(())
}

/// The result of checking an SPDX license expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LicenseExpression {
    Valid,
    /// Valid, but tools normalize it to this capitalization.
    Case(String),
    /// Valid, but uses deprecated identifiers; this is the modern form.
    Deprecated(String),
    /// Invalid, with a likely intended expression when one can be inferred.
    Invalid {
        problem: Problem,
        suggestion: Option<String>,
    },
}

/// Parsing that matches what Python's `packaging` accepts, which build backends
/// use: GPL `+` suffixes and deprecated identifiers are allowed, but imprecise
/// names such as `Apache 2.0` are not.
const LICENSE_MODE: spdx::ParseMode = spdx::ParseMode {
    allow_slash_as_or_operator: false,
    allow_imprecise_license_names: false,
    allow_postfix_plus_on_gpl: true,
    allow_deprecated: true,
    allow_unknown: false,
};

/// Checks an SPDX license expression, such as `MIT OR Apache-2.0`.
pub fn check_license_expression(value: &str) -> LicenseExpression {
    let canonical = spdx::Expression::canonicalize(value).ok().flatten();
    match spdx::Expression::parse_mode(value, LICENSE_MODE) {
        Ok(_) => match canonical {
            Some(canonical) if canonical.eq_ignore_ascii_case(value) => {
                LicenseExpression::Case(canonical)
            }
            Some(canonical) => LicenseExpression::Deprecated(canonical),
            None => LicenseExpression::Valid,
        },
        Err(error) => match canonical {
            // Identifiers are case-insensitive for `packaging`.
            Some(canonical)
                if canonical.eq_ignore_ascii_case(value)
                    && spdx::Expression::parse_mode(&canonical, LICENSE_MODE).is_ok() =>
            {
                LicenseExpression::Case(canonical)
            }
            suggestion => LicenseExpression::Invalid {
                problem: Problem::new(error.reason.to_string(), Some(error.span)),
                suggestion,
            },
        },
    }
}

/// Checks a glob pattern as defined for `license-files`, returning the reason
/// it is invalid.
pub fn check_glob_pattern(pattern: &str) -> Result<(), &'static str> {
    if pattern.contains("..") {
        return Err("patterns cannot contain `..`");
    }
    if pattern.starts_with('/') || pattern.contains(":\\") {
        return Err("patterns must be relative and cannot start with `/`");
    }
    let valid_char = |char: char| {
        char.is_alphanumeric()
            || matches!(char, '_' | ' ' | '-' | '.' | '/' | '*' | '?' | '[' | ']')
    };
    if pattern.is_empty() || !pattern.chars().all(valid_char) {
        return Err(
            "patterns may only contain letters, digits, spaces, `_`, `-`, `.`, `/`, `*`, `?`, `[`, and `]`",
        );
    }
    Ok(())
}

/// Whether `specifiers` allow some release of Python `major.minor`, or `None`
/// if they are invalid.
///
/// Checks the first, a middle, and a late patch release, so lower bounds such as
/// `>=3.8.1` still count as supporting 3.8.
pub fn allows_python_minor(specifiers: &str, major: u64, minor: u64) -> Option<bool> {
    let specifiers = uv_pep440::VersionSpecifiers::from_str(specifiers).ok()?;
    Some(
        [0, 1, 99]
            .into_iter()
            .any(|patch| specifiers.contains(&Version::new([major, minor, patch]))),
    )
}

/// Checks a project, extra, or group name against the name format and returns
/// its normalized form.
pub fn normalize_name(value: &str) -> Result<String, Problem> {
    PackageName::from_str(value)
        .map(|name| name.to_string())
        .map_err(|error| Problem::new(error.to_string(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirements() {
        assert!(check_requirement(r#"requests>=2.31; python_version < "3.12""#).is_ok());
        assert!(check_requirement("pkg[extra] @ https://example.com/pkg.whl").is_ok());
        let problem = check_requirement("requests >= 2.3.*").unwrap_err();
        assert_eq!(
            problem.message,
            "Operator >= cannot be used with a wildcard version specifier"
        );
        assert_eq!(&"requests >= 2.3.*"[problem.range.unwrap()], ">= 2.3.*");
    }

    #[test]
    fn specifiers_locate_the_invalid_part() {
        assert!(check_specifiers(">=3.11, <4").is_ok());
        let value = ">=3.11, =>4";
        let problem = check_specifiers(value).unwrap_err();
        assert_eq!(&value[problem.range.unwrap()], "=>4");
        assert!(check_specifiers("").is_err());
        let value = ">=3.11,";
        assert_eq!(check_specifiers(value).unwrap_err().range, Some(7..7));
    }

    #[test]
    fn license_expressions() {
        use LicenseExpression::*;
        assert_eq!(check_license_expression("MIT OR Apache-2.0"), Valid);
        assert_eq!(check_license_expression("LicenseRef-Proprietary"), Valid);
        assert_eq!(check_license_expression("mit"), Case("MIT".to_owned()));
        assert_eq!(
            check_license_expression("MIT or Apache-2.0"),
            Case("MIT OR Apache-2.0".to_owned())
        );
        assert_eq!(
            check_license_expression("GPL-3.0+"),
            Deprecated("GPL-3.0-or-later".to_owned())
        );
        let Invalid { suggestion, .. } = check_license_expression("Apache 2.0") else {
            panic!("expected invalid");
        };
        assert_eq!(suggestion.as_deref(), Some("Apache-2.0"));
        assert!(matches!(check_license_expression(""), Invalid { .. }));
        assert!(matches!(
            check_license_expression("Not-A-License"),
            Invalid { .. }
        ));
    }

    #[test]
    fn glob_patterns() {
        for valid in [
            "LICEN[CS]E*",
            "licenses/**/*.txt",
            "LICENSE.txt",
            "COPYING GPL",
        ] {
            assert_eq!(check_glob_pattern(valid), Ok(()), "{valid}");
        }
        for invalid in [
            "../LICENSE",
            "/LICENSE",
            "C:\\LICENSE",
            "LICEN{CSE*",
            "licenses\\x",
            "",
        ] {
            assert!(check_glob_pattern(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn versions_and_names() {
        assert!(check_version("1.0.post1").is_ok());
        assert!(check_version("1.0-beta.x").is_err());
        assert_eq!(normalize_name("Foo_Bar.baz").unwrap(), "foo-bar-baz");
        assert!(normalize_name("-foo").is_err());
    }
}
