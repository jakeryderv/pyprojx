//! `[tool.uv]`, checked against the uv releases the project allows.
//!
//! uv enforces `required-version`, so the allowed releases are those it allows,
//! or the latest release without it. Whether pyprojx reports a problem as an
//! error depends on what uv does with it, which the data records: uv rejects
//! invalid project fields such as `sources`, but only warns about invalid
//! settings such as `index-url`, and then ignores the file's other settings.
//!
//! `[tool.uv.build-backend]` is for `uv_build`, the build backend, which uv
//! ignores, so it is checked against the `uv_build` releases that
//! `[build-system]` allows, and reported if the project uses another backend.

use std::ops::Range;

use toml::de::DeTable;

use super::tool::{Checker, Extension};
use super::{Context, uv_references};
use crate::backends;
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::standards::{VersionSet, required_versions};
use crate::uv::{UV, UV_BUILD};

/// uv's checks beyond options and their values.
struct UvChecks;

impl Extension for UvChecks {
    fn skips(&self, path: &str) -> bool {
        // uv ignores the build backend's settings, which `uv_build` reads.
        path == "build-backend"
    }
}

/// Checks only `[tool.uv.build-backend]`.
struct BuildBackendChecks;

impl Extension for BuildBackendChecks {
    fn skips(&self, path: &str) -> bool {
        path != "build-backend" && !path.starts_with("build-backend.")
    }
}

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((uv, span)) = get(root, "tool")
        .and_then(|(_, tool)| tool.get_ref().as_table())
        .and_then(|tool| get(tool, "uv"))
        .and_then(|(_, uv)| Some((uv.get_ref().as_table()?, uv.span())))
    else {
        return;
    };
    let checker = Checker::new(context, &UV, root, uv, None, Some("required-version"));
    // Nothing to say about releases pyprojx does not know.
    if checker.candidates.is_empty() {
        return;
    }
    checker.check_table(context, &mut UvChecks, (uv, span.clone()), "");
    uv_references::check(context, &checker, root, uv);
    check_build_backend(context, root, (uv, span));
}

/// Checks `[tool.uv.build-backend]` against the `uv_build` releases that
/// `[build-system]` allows, or reports that the project's backend ignores it.
fn check_build_backend(
    context: &mut Context<'_>,
    root: &DeTable<'_>,
    (uv, span): (&DeTable<'_>, Range<usize>),
) {
    let Some((key, _)) = get(uv, "build-backend") else {
        return;
    };
    let build_system = get(root, "build-system").and_then(|(_, table)| table.get_ref().as_table());
    let backend = build_system
        .and_then(|table| get(table, "build-backend"))
        .and_then(|(_, value)| Some((value.get_ref().as_str()?, value.span())));
    let uv_build = backends::by_name("uv-build").expect("uv_build is a known backend");
    let uses_uv_build = backend
        .as_ref()
        .and_then(|(reference, _)| backends::by_reference(reference))
        .is_some_and(|data| data.name == uv_build.name);
    if !uses_uv_build {
        let diagnostic = match &backend {
            Some((reference, value)) => Diagnostic::new(
                Rule::IneffectiveSetting,
                format!(
                    "the build backend is `{reference}`, which ignores `[tool.uv.build-backend]`"
                ),
                key.span(),
            )
            .with_label(value.clone(), "the build backend"),
            None => Diagnostic::new(
                Rule::IneffectiveSetting,
                "without a `build-backend` in `[build-system]`, the build backend is setuptools, which ignores `[tool.uv.build-backend]`",
                key.span(),
            ),
        };
        context.report(diagnostic.with_help(
            "only `uv_build` reads these settings; set `build-backend = \"uv_build\"` in `[build-system]` to use it",
        ));
        return;
    }

    // Installers pick the newest release the requirement allows.
    let requirement = build_system
        .and_then(|table| get(table, "requires"))
        .and_then(|(_, requires)| requires.get_ref().as_array())
        .into_iter()
        .flatten()
        .find_map(|entry| {
            let (name, versions) = required_versions(entry.get_ref().as_str()?)?;
            (name == uv_build.name).then(|| (versions, entry.span()))
        });
    let releases = UV_BUILD.releases;
    let published = VersionSet::at_least(uv_build.first);
    let (candidates, source) = match requirement {
        Some((Some(versions), span)) => (
            (0..releases.len())
                .filter(|&index| {
                    published.contains(releases[index]) && versions.contains(releases[index])
                })
                .collect(),
            Some(span),
        ),
        Some((None, span)) => (vec![UV_BUILD.latest()], Some(span)),
        None => (vec![UV_BUILD.latest()], None),
    };
    let checker = Checker::with_candidates(&UV_BUILD, candidates, source);
    // Nothing to say about releases pyprojx does not know.
    if checker.candidates.is_empty() {
        return;
    }
    checker.check_table(context, &mut BuildBackendChecks, (uv, span), "");
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    /// Checks `[tool.uv]` settings with uv pinned to `version` (or unpinned),
    /// returning (rule, severity, message, help, spanned text).
    fn uv(
        version: Option<&str>,
        body: &str,
    ) -> Vec<(&'static str, Severity, String, String, String)> {
        let pin = version.map_or_else(String::new, |v| format!("required-version = \"=={v}\"\n"));
        let text = format!(
            "[project]\nname = \"demo\"\nversion = \"1\"\ndependencies = [\"idna\"]\n[tool.uv]\n{pin}{body}"
        );
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .map(|d| {
                (
                    d.rule.name(),
                    d.severity(),
                    d.message,
                    d.help.unwrap_or_default(),
                    text[d.span].to_owned(),
                )
            })
            .collect()
    }

    fn short<'a>(
        found: &'a [(&'static str, Severity, String, String, String)],
    ) -> Vec<(&'static str, Severity, &'a str)> {
        found
            .iter()
            .map(|(rule, severity, _, _, span)| (*rule, *severity, span.as_str()))
            .collect()
    }

    #[test]
    fn valid_configuration() {
        let body = "managed = true\nindex-url = \"https://example.com/simple\"\n[tool.uv.pip]\ngroup = [\"dev\"]\n[tool.uv.sources]\nidna = { index = \"example\" }\n[[tool.uv.index]]\nname = \"example\"\nurl = \"https://example.com/simple\"\nexplicit = true\n";
        assert_eq!(uv(None, body), []);
    }

    #[test]
    fn unknown_keys_follow_what_uv_does() {
        // uv warns and ignores the other settings.
        let found = uv(None, "index-ur = \"https://example.com/simple\"\n");
        assert_eq!(
            short(&found),
            [("unknown-key", Severity::Warning, "index-ur")]
        );
        assert!(
            found[0]
                .3
                .starts_with("did you mean `index-url`? uv warns about it"),
            "{}",
            found[0].3
        );
        // uv ignores unknown keys in indexes without a word.
        let found = uv(
            None,
            "[[tool.uv.index]]\nurl = \"https://example.com/simple\"\nexplict = true\n",
        );
        assert_eq!(
            short(&found),
            [("unknown-key", Severity::Warning, "explict")]
        );
        assert_eq!(found[0].3, "did you mean `explicit`? uv ignores it");
        // uv rejects unknown keys in `[tool.uv.workspace]`.
        let found = uv(None, "[tool.uv.workspace]\nmembrs = []\n");
        assert_eq!(short(&found), [("unknown-key", Severity::Error, "membrs")]);
    }

    #[test]
    fn invalid_values_follow_what_uv_does() {
        assert_eq!(
            short(&uv(None, "managed = \"yes\"\n")),
            [("invalid-value", Severity::Error, "\"yes\"")]
        );
        assert_eq!(
            short(&uv(None, "index-strategy = \"first\"\n")),
            [("invalid-value", Severity::Warning, "\"first\"")]
        );
        assert_eq!(
            short(&uv(None, "[[tool.uv.index]]\nname = \"x\"\n")),
            [("missing-key", Severity::Error, "[[tool.uv.index]]")]
        );
    }

    #[test]
    fn deprecated_and_version_specific_options() {
        let found = uv(None, "dev-dependencies = []\n");
        assert_eq!(
            short(&found),
            [("deprecated-setting", Severity::Warning, "dev-dependencies")]
        );
        assert_eq!(found[0].3, "use `dependency-groups.dev` instead");
        // `exclude-dependencies` was added in 0.9.8. uv treats options it does
        // not know like unknown keys: here, it warns.
        let body = "exclude-dependencies = [\"idna\"]\n";
        let found = uv(Some("0.9.0"), body);
        assert_eq!(
            short(&found),
            [(
                "unsupported-feature",
                Severity::Warning,
                "exclude-dependencies"
            )]
        );
        assert!(
            found[0].3.starts_with(
                "uv added it in 0.9.8; set `required-version = \">=0.9.8\"`; uv warns about it"
            ),
            "{}",
            found[0].3
        );
        assert_eq!(uv(Some("0.9.8"), body), []);
    }

    #[test]
    fn build_backend_settings_are_for_uv_build() {
        let project = |backend: &str, body: &str| {
            let text = format!(
                "[project]\nname = \"demo\"\nversion = \"1\"\n{backend}[tool.uv.build-backend]\n{body}"
            );
            crate::check(text.as_bytes().to_vec())
                .diagnostics
                .into_iter()
                .map(|d| {
                    (
                        d.rule.name(),
                        d.severity(),
                        d.help.unwrap_or_default(),
                        text[d.span].to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let uv_build = |version: &str| {
            format!(
                "[build-system]\nrequires = [\"uv_build{version}\"]\nbuild-backend = \"uv_build\"\n"
            )
        };
        // `uv_build` ignores unknown keys but rejects invalid values.
        let found = project(&uv_build(">=0.9,<0.10"), "module-nme = \"demo\"\n");
        assert_eq!(
            (found[0].0, found[0].1, found[0].3.as_str()),
            ("unknown-key", Severity::Warning, "module-nme")
        );
        assert_eq!(
            found[0].2,
            "did you mean `module-name`? uv_build ignores it"
        );
        let found = project(&uv_build(""), "module-root = 1\n");
        assert_eq!(
            (found[0].0, found[0].1, found[0].3.as_str()),
            ("invalid-value", Severity::Error, "1")
        );
        assert_eq!(
            project(&uv_build("==0.9.0"), "module-name = \"demo\"\n"),
            []
        );
        // Other backends ignore the settings.
        let hatchling =
            "[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n";
        let found = project(hatchling, "module-name = \"demo\"\n");
        assert_eq!(
            (found[0].0, found[0].1, found[0].3.as_str()),
            ("ineffective-setting", Severity::Warning, "build-backend")
        );
        let found = project("", "module-name = \"demo\"\n");
        assert_eq!(
            (found[0].0, found[0].3.as_str()),
            ("ineffective-setting", "build-backend")
        );
    }
}
