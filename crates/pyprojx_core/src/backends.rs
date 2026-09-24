//! Build backends, and which of their releases support which features.
//!
//! The data is measured, not transcribed from changelogs:
//! `scripts/calibrate_backends.py` builds a project using each feature with
//! releases of each backend, and checks the metadata it produces.

use crate::backend_data::BACKENDS;

/// A feature of `pyproject.toml` that build backends added over time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    ProjectTable,
    LicenseExpression,
    LicenseFiles,
    /// `import-names` and `import-namespaces` (PEP 794).
    ImportNames,
    /// A static key also listed in `dynamic` (PEP 808).
    DynamicExtension,
    /// `License ::` classifiers alongside a license expression, which the
    /// specification lets backends reject.
    LicenseClassifiers,
}

impl Feature {
    pub const ALL: [Self; 6] = [
        Self::ProjectTable,
        Self::LicenseExpression,
        Self::LicenseFiles,
        Self::ImportNames,
        Self::DynamicExtension,
        Self::LicenseClassifiers,
    ];

    /// Describes the feature in diagnostics.
    pub fn description(self) -> &'static str {
        match self {
            Self::ProjectTable => "the `[project]` table",
            Self::LicenseExpression => "license expressions in `project.license`",
            Self::LicenseFiles => "`project.license-files`",
            Self::ImportNames => "`import-names` and `import-namespaces` (PEP 794)",
            Self::DynamicExtension => {
                "extending a static key listed in `project.dynamic` (PEP 808)"
            }
            Self::LicenseClassifiers => "`License ::` classifiers alongside a license expression",
        }
    }

    /// How to fix a project whose backend lacks the feature in every version.
    pub fn fix(self) -> &'static str {
        match self {
            Self::ProjectTable | Self::LicenseExpression | Self::LicenseFiles => {
                "use a build backend that supports it"
            }
            Self::ImportNames => "remove it, or use a build backend that supports it",
            Self::DynamicExtension => {
                "set the key statically or list it in `project.dynamic`, not both"
            }
            Self::LicenseClassifiers => {
                "remove the `License ::` classifiers, which are deprecated; the license expression replaces them"
            }
        }
    }
}

/// Which releases of a backend support a feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    /// Every known release.
    First,
    /// Releases from `version` on; `before` is the release preceding it.
    Version {
        before: &'static str,
        version: &'static str,
    },
    /// No release from `from` on. Earlier releases, if any, accepted it
    /// without checking it, and pyprojx says nothing about them.
    Never { from: &'static str },
}

/// Which releases of a backend support a feature, and what the others do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureSupport {
    pub feature: Feature,
    pub since: Since,
    /// Whether builds fail with every checked release that reads `[project]`
    /// but lacks the feature. Otherwise, some leave it out of the metadata.
    pub fails: bool,
}

/// A build backend and its known releases.
#[derive(Debug)]
pub struct BackendData {
    /// The normalized distribution name.
    pub name: &'static str,
    /// The module named in `build-backend`.
    pub module: &'static str,
    /// The first and latest releases known; pyprojx says nothing about others.
    pub first: &'static str,
    pub latest: &'static str,
    pub features: &'static [FeatureSupport],
}

impl BackendData {
    /// # Panics
    ///
    /// If the data has no entry for `feature`; every backend lists every feature.
    pub fn support(&self, feature: Feature) -> FeatureSupport {
        *self
            .features
            .iter()
            .find(|support| support.feature == feature)
            .expect("every backend lists every feature")
    }

    /// The first release that reads `[project]`.
    pub fn reads_project(&self) -> &'static str {
        match self.support(Feature::ProjectTable).since {
            Since::Version { version, .. } => version,
            Since::First | Since::Never { .. } => self.first,
        }
    }
}

/// The backend whose `build-backend` value is `reference`, such as
/// `setuptools.build_meta:__legacy__`.
pub fn by_reference(reference: &str) -> Option<&'static BackendData> {
    let module = reference
        .split_once(':')
        .map_or(reference, |(module, _)| module);
    BACKENDS.iter().find(|backend| backend.module == module)
}

/// The backend with the normalized distribution name `name`.
pub fn by_name(name: &str) -> Option<&'static BackendData> {
    BACKENDS.iter().find(|backend| backend.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standards::required_versions;

    #[test]
    fn data_is_valid() {
        for backend in BACKENDS {
            let versions = [backend.first, backend.latest].into_iter().chain(
                backend
                    .features
                    .iter()
                    .flat_map(|support| match support.since {
                        Since::Version { before, version } => vec![before, version],
                        Since::Never { from } => vec![from],
                        Since::First => vec![],
                    }),
            );
            for version in versions {
                assert!(
                    required_versions(&format!("{}=={version}", backend.name)).is_some(),
                    "{} {version}",
                    backend.name
                );
            }
            assert_eq!(
                by_name(backend.name).map(|b| b.module),
                Some(backend.module)
            );
            for feature in Feature::ALL {
                backend.support(feature);
            }
        }
    }

    #[test]
    fn lookups() {
        let setuptools = by_reference("setuptools.build_meta:__legacy__").unwrap();
        assert_eq!(setuptools.name, "setuptools");
        assert!(by_reference("setuptools").is_none());
        assert_eq!(setuptools.reads_project(), "61.0.0");
        assert!(setuptools.support(Feature::LicenseExpression).fails);
    }
}
