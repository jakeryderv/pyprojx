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
    fn versions_and_names() {
        assert!(check_version("1.0.post1").is_ok());
        assert!(check_version("1.0-beta.x").is_err());
        assert_eq!(normalize_name("Foo_Bar.baz").unwrap(), "foo-bar-baz");
        assert!(normalize_name("-foo").is_err());
    }
}
