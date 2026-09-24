//! `[tool.uv]`, checked against the uv releases the project allows.
//!
//! uv enforces `required-version`, so the allowed releases are those it allows,
//! or the latest release without it. Whether pyprojx reports a problem as an
//! error depends on what uv does with it, which the data records: uv rejects
//! invalid project fields such as `sources`, but only warns about invalid
//! settings such as `index-url`, and then ignores the file's other settings.

use toml::de::DeTable;

use super::tool::{Checker, Extension};
use super::{Context, uv_references};
use crate::document::get;
use crate::uv::UV;

/// uv's checks beyond options and their values.
struct UvChecks;

impl Extension for UvChecks {
    fn skips(&self, path: &str) -> bool {
        // uv ignores the build backend's settings, which `uv_build` reads.
        path == "build-backend"
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
    checker.check_table(context, &mut UvChecks, (uv, span), "");
    uv_references::check(context, &checker, root, uv);
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
    fn build_backend_is_left_to_uv_build() {
        assert_eq!(
            uv(None, "[tool.uv.build-backend]\nmodule-nme = \"demo\"\n"),
            []
        );
    }
}
