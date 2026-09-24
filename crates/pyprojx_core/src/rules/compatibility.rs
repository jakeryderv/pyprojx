//! Features of `[project]` that the build backend lacks in some or all of the
//! versions `[build-system]` allows.
//!
//! Which releases support what comes from `crate::backends`. Only known
//! releases are judged: a requirement without version specifiers resolves to
//! the newest release, and releases newer than the data might add support.

use std::ops::Range;

use toml::de::DeTable;

use super::Context;
use super::project::EXTENDABLE_KEYS;
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

/// Where a project uses a feature: the span to report, and an optional label.
struct Usage {
    feature: Feature,
    span: Range<usize>,
    label: Option<(Range<usize>, String)>,
}

impl Usage {
    fn new(feature: Feature, span: Range<usize>) -> Self {
        Self {
            feature,
            span,
            label: None,
        }
    }

    fn with_label(mut self, span: Range<usize>, text: &str) -> Self {
        self.label = Some((span, text.to_owned()));
        self
    }
}

pub(super) fn check(context: &mut Context<'_>, root: &DeTable<'_>) {
    let Some((project_key, project)) = get(root, "project") else {
        return;
    };
    let Some(project) = project.get_ref().as_table() else {
        return;
    };
    let declared = declared(root);

    if let Some(declared) = &declared {
        // Without `[project]`, the other features do not matter.
        let usage = Usage::new(Feature::ProjectTable, project_key.span());
        if !report(context, declared, usage) {
            return;
        }
        for usage in features(project) {
            report(context, declared, usage);
        }
    }

    for (key, span, listed) in extended_keys(project) {
        let label = "also listed in `dynamic` here";
        match &declared {
            Some(declared) => {
                let usage = Usage::new(Feature::DynamicExtension, span).with_label(listed, label);
                report(context, declared, usage);
            }
            None => context.report(
                Diagnostic::new(
                    Rule::InvalidDynamic,
                    format!("`{key}` is set statically and also listed in `project.dynamic`"),
                    span,
                )
                .with_label(listed, label)
                .with_help(format!("PEP 808 allows build backends to extend a static `{key}`, but many reject it; check that yours supports it"))
                .as_warning(),
            ),
        }
    }
}

/// The features a project uses, other than `[project]` itself and PEP 808,
/// with where to report them.
fn features(project: &DeTable<'_>) -> Vec<Usage> {
    let mut found = Vec::new();
    let expression = get(project, "license")
        .filter(|(_, license)| license.get_ref().as_str().is_some())
        .map(|(_, license)| license.span());
    if let Some(span) = &expression {
        found.push(Usage::new(Feature::LicenseExpression, span.clone()));
    }
    if let Some((key, _)) = get(project, "license-files") {
        found.push(Usage::new(Feature::LicenseFiles, key.span()));
    }
    for name in ["import-names", "import-namespaces"] {
        if let Some((key, _)) = get(project, name) {
            found.push(Usage::new(Feature::ImportNames, key.span()));
        }
    }
    if let Some(expression) = expression
        && let Some((_, classifiers)) = get(project, "classifiers")
        && let Some(classifiers) = classifiers.get_ref().as_array()
        && let Some(classifier) = classifiers.iter().find(|classifier| {
            classifier
                .get_ref()
                .as_str()
                .is_some_and(|text| text.starts_with("License ::"))
        })
    {
        found.push(
            Usage::new(Feature::LicenseClassifiers, classifier.span())
                .with_label(expression, "license expression set here"),
        );
    }
    found
}

/// Keys set statically and also listed in `dynamic`, which PEP 808 allows for
/// some keys, with the spans of the key and of its entry in `dynamic`.
fn extended_keys<'t>(project: &'t DeTable<'_>) -> Vec<(&'t str, Range<usize>, Range<usize>)> {
    let Some((_, dynamic)) = get(project, "dynamic") else {
        return Vec::new();
    };
    let Some(dynamic) = dynamic.get_ref().as_array() else {
        return Vec::new();
    };
    dynamic
        .iter()
        .filter_map(|entry| {
            let name = entry.get_ref().as_str()?;
            let (key, _) = get(project, name)?;
            EXTENDABLE_KEYS
                .contains(&name)
                .then(|| (name, key.span(), entry.span()))
        })
        .collect()
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

/// Reports a feature if some allowed versions lack it, and returns whether any
/// allowed version supports it.
fn report(context: &mut Context<'_>, declared: &Declared, usage: Usage) -> bool {
    let Usage {
        feature,
        span,
        label,
    } = usage;
    let backend = declared.backend;
    let support = backend.support(feature);
    let mut candidates = declared.candidates();
    if feature != Feature::ProjectTable {
        // Releases that ignore `[project]` are reported for that instead.
        candidates = candidates.difference(&VersionSet::before(backend.reads_project()));
    }
    let (unsupported, since) = match support.since {
        Since::First => return true,
        Since::Version { before, version } => {
            // No releases fall between these, whatever the requirement allows.
            candidates = candidates.difference(&VersionSet::between(before, version));
            (
                VersionSet::from_through(backend.first, before),
                Some(version),
            )
        }
        Since::Never { from } => {
            candidates = candidates.difference(&VersionSet::before(from));
            (VersionSet::from_through(from, backend.latest), None)
        }
    };
    if candidates.intersection(&unsupported).is_empty() {
        return true;
    }
    let none_supported = candidates.is_subset_of(&unsupported);

    let name = backend.name;
    let what = feature.description();
    let (message, requirement_label, help) = match since {
        Some(since) => {
            let earlier = if feature == Feature::ProjectTable {
                "earlier versions do not read it"
            } else if support.fails {
                "builds with earlier versions fail"
            } else {
                "earlier versions leave it out of the metadata or fail to build"
            };
            let help = format!("require `{name}>={since}`; {earlier}");
            if none_supported {
                let message =
                    format!("no {name} version allowed by `[build-system]` supports {what}");
                (
                    message,
                    format!("allows only versions before {since}"),
                    help,
                )
            } else {
                let message = format!("{name} versions before {since} do not support {what}");
                (message, "allows those versions".to_owned(), help)
            }
        }
        None => {
            let message = if none_supported {
                format!("{name} does not support {what}")
            } else {
                // Releases after the latest known one might.
                format!("{name} does not support {what}, as of {}", backend.latest)
            };
            (
                message,
                "build backend required here".to_owned(),
                feature.fix().to_owned(),
            )
        }
    };

    if feature == Feature::LicenseClassifiers {
        // This replaces the deprecation warning, with the same fix.
        context.diagnostics.retain(|diagnostic| {
            !(diagnostic.rule == Rule::DeprecatedMetadata && diagnostic.span == span)
        });
    }
    let mut diagnostic = Diagnostic::new(Rule::UnsupportedFeature, message, span);
    if let Some((span, text)) = label {
        diagnostic = diagnostic.with_label(span, text);
    }
    diagnostic = match &declared.requirement {
        Some(requirement) => diagnostic
            .with_label(requirement.clone(), requirement_label)
            .with_help(help),
        None => diagnostic.with_help(format!(
            "{help} (without `[build-system]`, tools build with {name})"
        )),
    };
    // An error only when builds fail with every release an installer could pick.
    if !(none_supported && support.fails) {
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
        // Versions before 61 ignore `[project]`, which is reported instead;
        // installers pick 69, whose builds fail.
        let found = check("\"setuptools<70\"", LICENSE);
        let found: Vec<_> = found
            .iter()
            .map(|(s, _, span)| (*s, span.as_str()))
            .collect();
        assert_eq!(
            found,
            [(Severity::Warning, "project"), (Severity::Error, "\"MIT\"")]
        );
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

    /// Checks a project built with `backend` and `requires`, returning (rule,
    /// severity, message, spanned text) for diagnostics about compatibility.
    fn check_with(
        backend: &str,
        requires: &str,
        body: &str,
    ) -> Vec<(&'static str, Severity, String, String)> {
        let text = format!(
            "[build-system]\nrequires = [{requires}]\nbuild-backend = \"{backend}\"\n\n[project]\nname = \"demo\"\nversion = \"1\"\n{body}"
        );
        crate::check(text.as_bytes().to_vec())
            .diagnostics
            .into_iter()
            .filter(|d| matches!(d.rule.name(), "unsupported-feature" | "invalid-dynamic"))
            .map(|d| {
                (
                    d.rule.name(),
                    d.severity(),
                    d.message,
                    text[d.span].to_owned(),
                )
            })
            .collect()
    }

    const EXTENDED: &str = "dependencies = [\"requests\"]\ndynamic = [\"dependencies\"]\n";

    #[test]
    fn extending_static_keys() {
        let pinned = check_with("hatchling.build", "\"hatchling==1.32.4\"", EXTENDED);
        assert_eq!(
            pinned,
            [(
                "unsupported-feature",
                Severity::Error,
                "hatchling does not support extending a static key listed in `project.dynamic` (PEP 808)".to_owned(),
                "dependencies".to_owned()
            )]
        );
        // Releases after the latest known one might support it.
        let unpinned = check_with("hatchling.build", "\"hatchling\"", EXTENDED);
        assert_eq!(unpinned[0].1, Severity::Warning);
        assert!(unpinned[0].2.ends_with(", as of 1.32.4"));
        // hatchling before 0.15.0 accepted it without checking.
        assert_eq!(
            check_with("hatchling.build", "\"hatchling<0.15\"", EXTENDED),
            []
        );
        assert_eq!(
            check_with("poetry.core.masonry.api", "\"poetry-core>=2\"", EXTENDED),
            []
        );
        // Unknown backends get a general warning.
        let unknown = check_with("maturin", "\"maturin>=1\"", EXTENDED);
        assert_eq!(
            (unknown[0].0, unknown[0].1),
            ("invalid-dynamic", Severity::Warning)
        );
    }

    #[test]
    fn import_names() {
        let body = "import-names = [\"demo\"]\n";
        let found = check_with("hatchling.build", "\"hatchling>=1.27\"", body);
        assert_eq!(
            found[0].2,
            "hatchling versions before 1.32.0 do not support `import-names` and `import-namespaces` (PEP 794)"
        );
        assert_eq!(
            check_with("hatchling.build", "\"hatchling>=1.32\"", body),
            []
        );
        // pdm-backend builds, but leaves import names out of the metadata.
        let found = check_with("pdm.backend", "\"pdm-backend==2.4.10\"", body);
        assert_eq!(found[0].1, Severity::Warning);
    }

    #[test]
    fn license_classifiers_with_an_expression() {
        let body =
            "license = \"MIT\"\nclassifiers = [\"License :: OSI Approved :: MIT License\"]\n";
        assert_eq!(
            check_with("hatchling.build", "\"hatchling>=1.27\"", body),
            []
        );
        let found = check_with("flit_core.buildapi", "\"flit_core>=3.11,<5\"", body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, Severity::Warning);
        assert_eq!(found[0].3, "\"License :: OSI Approved :: MIT License\"");
        let found = check_with("flit_core.buildapi", "\"flit_core>=3.11,<=4.1.0\"", body);
        assert_eq!(found[0].1, Severity::Error);
    }

    #[test]
    fn projects_without_build_system_use_setuptools() {
        let text = "[project]\nname = \"demo\"\nversion = \"1\"\nimport-names = [\"demo\"]\n";
        let found = crate::check(text.as_bytes().to_vec()).diagnostics;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity(), Severity::Warning);
        assert!(
            found[0]
                .help
                .as_deref()
                .unwrap()
                .ends_with("(without `[build-system]`, tools build with setuptools)")
        );
    }
}
