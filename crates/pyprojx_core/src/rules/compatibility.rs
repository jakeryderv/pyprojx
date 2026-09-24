//! Features of `[project]` that the build backend lacks in some or all of the
//! versions `[build-system]` allows.
//!
//! Which releases support what comes from `crate::backends`. Only known
//! releases are judged: a requirement without version specifiers resolves to
//! the newest release, and releases newer than the data might add support.

use std::ops::Range;

use toml::de::DeTable;

use super::Context;
use crate::backends::{self, BackendData, Feature, Since};
use crate::diagnostic::{Diagnostic, Rule};
use crate::document::get;
use crate::standards::{VersionSet, required_versions};

/// The build backend a project uses, and the versions of it that it allows.
struct Declared {
    backend: &'static BackendData,
    /// `None` when no requirement has version specifiers.
    versions: Option<VersionSet>,
    /// The span of the requirement for the backend in `build-system.requires`;
    /// `None` when the project has no `[build-system]`.
    requirement: Option<Range<usize>>,
}

impl Declared {
    /// The known releases an installer could pick.
    fn candidates(&self) -> VersionSet {
        let backend = self.backend;
        let allowed = match &self.versions {
            Some(versions) => versions.clone(),
            // Installers pick the newest release.
            None => VersionSet::at_least(backend.latest),
        };
        allowed.difference(&VersionSet::before(backend.first))
    }
}

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((project_key, project)) = get(root, "project") else {
        return;
    };
    let Some(project) = project.get_ref().as_table() else {
        return;
    };
    let Some(declared) = declared(root) else {
        return;
    };

    // Without `[project]`, the other features do not matter.
    if !report(
        context,
        &declared,
        Feature::ProjectTable,
        project_key.span(),
    ) {
        return;
    }
    if let Some((_, license)) = get(project, "license")
        && license.get_ref().as_str().is_some()
    {
        report(
            context,
            &declared,
            Feature::LicenseExpression,
            license.span(),
        );
    }
    if let Some((key, _)) = get(project, "license-files") {
        report(context, &declared, Feature::LicenseFiles, key.span());
    }
}

/// Finds the build backend, if pyprojx knows it and the requirement for it.
fn declared(root: &DeTable<'_>) -> Option<Declared> {
    let Some((_, build_system)) = get(root, "build-system") else {
        // Tools fall back to the newest setuptools.
        return Some(Declared {
            backend: backends::by_name("setuptools")?,
            versions: None,
            requirement: None,
        });
    };
    let build_system = build_system.get_ref().as_table()?;
    if get(build_system, "backend-path").is_some() {
        // The backend is part of the project, perhaps wrapping a known one.
        return None;
    }
    let backend = match get(build_system, "build-backend") {
        Some((_, reference)) => backends::by_reference(reference.get_ref().as_str()?)?,
        // PEP 517's fallback, `setuptools.build_meta:__legacy__`.
        None => backends::by_name("setuptools")?,
    };

    let (_, requires) = get(build_system, "requires")?;
    let mut declared: Option<Declared> = None;
    for entry in requires.get_ref().as_array()? {
        let Some(text) = entry.get_ref().as_str() else {
            continue;
        };
        let Some((name, versions)) = required_versions(text) else {
            continue;
        };
        if name != backend.name {
            continue;
        }
        declared = Some(match declared {
            // Several requirements for the backend all apply.
            Some(previous) => Declared {
                versions: match (previous.versions, versions) {
                    (Some(a), Some(b)) => Some(a.intersection(&b)),
                    (a, b) => a.or(b),
                },
                ..previous
            },
            None => Declared {
                backend,
                versions,
                requirement: Some(entry.span()),
            },
        });
    }
    declared
}

/// Reports a feature used at `span` if some allowed versions lack it, and
/// returns whether any allowed version supports it.
fn report(
    context: &mut Context<'_>,
    declared: &Declared,
    feature: Feature,
    span: Range<usize>,
) -> bool {
    let backend = declared.backend;
    let support = backend.support(feature);
    let (last_unsupported, since) = match support.since {
        Since::First => return true,
        Since::Version { before, version } => (before, Some(version)),
        Since::Never => (backend.latest, None),
    };
    let mut candidates = declared.candidates();
    if let Some(since) = since {
        // No releases fall between these, whatever the requirement allows.
        candidates = candidates.difference(&VersionSet::between(last_unsupported, since));
    }
    let unsupported = VersionSet::from_through(backend.first, last_unsupported);
    if candidates.intersection(&unsupported).is_empty() {
        return true;
    }
    let none_supported = candidates.is_subset_of(&unsupported);

    let name = backend.name;
    let what = feature.description();
    let latest = backend.latest;
    let earlier = if feature == Feature::ProjectTable {
        "earlier versions do not read it"
    } else if support.fails {
        "builds with earlier versions fail"
    } else {
        "earlier versions leave it out of the metadata or fail to build"
    };
    let (message, label, help) = match (since, none_supported) {
        (Some(since), false) => (
            format!("{name} versions before {since} do not support {what}"),
            "allows those versions".to_owned(),
            format!("require `{name}>={since}`; {earlier}"),
        ),
        (Some(since), true) => (
            format!("no {name} version allowed by `[build-system]` supports {what}"),
            format!("allows only versions before {since}"),
            format!("require `{name}>={since}`; {earlier}"),
        ),
        (None, false) => (
            format!("{name} does not support {what}, as of {latest}"),
            "set here".to_owned(),
            format!(
                "releases after {latest} might; otherwise, use a build backend that supports it"
            ),
        ),
        (None, true) => (
            format!("{name} does not support {what}"),
            "set here".to_owned(),
            "use a build backend that supports it".to_owned(),
        ),
    };
    let mut diagnostic = Diagnostic::new(Rule::UnsupportedFeature, message, span).with_help(help);
    if let Some(requirement) = &declared.requirement {
        diagnostic = diagnostic.with_label(requirement.clone(), label);
    }
    // An error only when builds fail with every release an installer could pick.
    let failing = VersionSet::from_through(backend.reads_project(), last_unsupported);
    if !(support.fails && candidates.is_subset_of(&failing)) {
        diagnostic = diagnostic.as_warning();
    }
    context.report(diagnostic);
    !none_supported
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    /// Checks a project built with `requires`, returning (severity, message, spanned text).
    fn check(requires: &str, body: &str) -> Vec<(Severity, String, String)> {
        let text = format!(
            "[build-system]\nrequires = [{requires}]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = \"demo\"\nversion = \"1\"\n{body}"
        );
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .filter(|d| d.rule.name() == "unsupported-feature")
            .map(|d| (d.severity(), d.message, text[d.span].to_owned()))
            .collect()
    }

    const LICENSE: &str = "license = \"MIT\"\n";

    #[test]
    fn supported_everywhere_allowed() {
        assert_eq!(check("\"setuptools>=77\"", LICENSE), []);
        assert_eq!(check("\"setuptools>=77.0.1,<90\"", LICENSE), []);
        // Unpinned requirements resolve to the newest release.
        assert_eq!(check("\"setuptools\"", LICENSE), []);
        // Versions beyond the known ones are not judged.
        assert_eq!(check("\"setuptools>=100\"", LICENSE), []);
    }

    #[test]
    fn some_allowed_versions_lack_a_feature() {
        assert_eq!(
            check("\"setuptools>=61\"", LICENSE),
            [(
                Severity::Warning,
                "setuptools versions before 77.0.1 do not support license expressions in `project.license`".to_owned(),
                "\"MIT\"".to_owned()
            )]
        );
        // Versions before 61 ignore `[project]` instead of failing.
        let found = check("\"setuptools<70\"", LICENSE);
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .all(|(severity, ..)| *severity == Severity::Warning)
        );
        assert_eq!(found[0].2, "project");
    }

    #[test]
    fn no_allowed_version_supports_a_feature() {
        let found = check(
            "\"setuptools>=61,<77\"",
            "license = \"MIT\"\nlicense-files = [\"LICENSE\"]\n",
        );
        assert_eq!(
            found,
            [
                (
                    Severity::Error,
                    "no setuptools version allowed by `[build-system]` supports license expressions in `project.license`".to_owned(),
                    "\"MIT\"".to_owned()
                ),
                (
                    Severity::Error,
                    "no setuptools version allowed by `[build-system]` supports `project.license-files`".to_owned(),
                    "license-files".to_owned()
                ),
            ]
        );
        // Several requirements for the backend narrow the allowed versions together.
        assert_eq!(
            check("\"setuptools>=61\", \"setuptools<77\"", LICENSE).len(),
            1
        );
    }

    #[test]
    fn ignoring_the_project_table_is_a_warning() {
        let found = check("\"setuptools<61\"", LICENSE);
        assert_eq!(
            found,
            [(
                Severity::Warning,
                "no setuptools version allowed by `[build-system]` supports the `[project]` table"
                    .to_owned(),
                "project".to_owned()
            )]
        );
    }

    #[test]
    fn backends_are_found_by_reference_and_requirement() {
        let text = "[build-system]\nrequires = [\"Hatchling >= 1.20\"]\nbuild-backend = \"hatchling.build\"\n[project]\nname = \"demo\"\nversion = \"1\"\nlicense = \"MIT\"\n";
        let found = crate::check(text.as_bytes().to_vec()).diagnostics;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity(), Severity::Warning);
        assert_eq!(
            &text[found[0].labels[0].span.clone()],
            "\"Hatchling >= 1.20\""
        );

        for skipped in [
            // Unknown backends, local backends, and backends not required are skipped.
            "[build-system]\nrequires = [\"mybackend<1\"]\nbuild-backend = \"mybackend\"\n",
            "[build-system]\nrequires = [\"setuptools<60\"]\nbuild-backend = \"backend\"\nbackend-path = [\"_build\"]\n",
            "[build-system]\nrequires = [\"wheel\"]\nbuild-backend = \"setuptools.build_meta\"\n",
            "[build-system]\nrequires = [\"setuptools @ https://example.com/setuptools.zip\"]\n",
        ] {
            let text = format!(
                "{skipped}[project]\nname = \"demo\"\nversion = \"1\"\nlicense = \"MIT\"\n"
            );
            assert_eq!(
                crate::check(text.as_bytes().to_vec()).diagnostics,
                [],
                "{skipped}"
            );
        }
        // Without `build-backend`, tools use setuptools.
        let text = "[build-system]\nrequires = [\"setuptools==76.1.0\"]\n[project]\nname = \"demo\"\nversion = \"1\"\nlicense = \"MIT\"\n";
        assert_eq!(crate::check(text.as_bytes().to_vec()).diagnostics.len(), 1);
    }
}
