//! Snapshot tests for `pyprojx check`.

use std::fs;
use std::path::Path;
use std::process::Command;

use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
use tempfile::TempDir;

/// A temporary project directory for running `pyprojx check`.
struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn with_pyproject(contents: &[u8]) -> Self {
        let project = Self::new();
        project.write("pyproject.toml", contents);
        project
    }

    fn write(&self, path: &str, contents: &[u8]) {
        let path = self.dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn check(&self) -> Command {
        self.check_in(self.dir.path())
    }

    fn check_in(&self, cwd: &Path) -> Command {
        let mut command = Command::new(get_cargo_bin("pyprojx"));
        command
            .arg("check")
            .current_dir(cwd)
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR_FORCE");
        command
    }
}

/// Makes output with paths identical across platforms.
macro_rules! snapshot {
    ($command:expr) => {
        insta::with_settings!({ filters => vec![(r"\\", "/")] }, {
            assert_cmd_snapshot!($command);
        });
    };
}

#[test]
fn valid_file_passes() {
    let project = Project::with_pyproject(b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\n");
    snapshot!(project.check());
}

#[test]
fn empty_file_passes() {
    let project = Project::with_pyproject(b"");
    snapshot!(project.check());
}

#[test]
fn reports_every_syntax_error_in_order() {
    let project = Project::with_pyproject(
        b"[tool.ruff]\nline-length =\nselect = [\"E\" \"F\"]\n\n[project]\nname = \"a\"\nname = \"b\"\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_byte_order_mark_with_help() {
    let project = Project::with_pyproject(b"\xef\xbb\xbf[project]\nname = \"demo\"\n");
    snapshot!(project.check());
}

#[test]
fn reports_invalid_utf8() {
    let project = Project::with_pyproject(b"[project]\nname = \"d\xffmo\"\n");
    snapshot!(project.check());
}

#[test]
fn finds_pyproject_in_parent_directory() {
    let project = Project::with_pyproject(b"[project]\nname =\n");
    project.write("src/pkg/__init__.py", b"");
    snapshot!(project.check_in(&project.dir.path().join("src/pkg")));
}

#[test]
fn checks_explicit_file() {
    let project = Project::new();
    project.write("configs/other.toml", b"key = \"value\"\nkey = 1\n");
    snapshot!(project.check().arg("configs/other.toml"));
}

#[test]
fn checks_pyproject_in_explicit_directory() {
    let project = Project::new();
    project.write("packages/app/pyproject.toml", b"[project\n");
    snapshot!(project.check().arg("packages/app"));
}

#[test]
fn missing_explicit_file_is_an_error() {
    let project = Project::new();
    snapshot!(project.check().arg("missing.toml"));
}

#[test]
fn missing_pyproject_is_an_error() {
    let project = Project::new();
    snapshot!(project.check());
}

#[test]
fn warns_about_toml_1_1_syntax_without_failing() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nauthors = [\n  { name = \"A\",\n    email = \"a@example.com\" },\n]\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_build_system_problems() {
    let project = Project::with_pyproject(
        b"[build-system]\nrequires = [\"setuptools >= 77.*\"]\nbuild_backend = \"setuptools.build_meta\"\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_project_problems_with_labels() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\ndynamic = [\"version\"]\nrequires_python = \">=3.11\"\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_dependency_and_license_problems() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\nlicense = \"Apache 2.0\"\ndependencies = [\"requests >= 2.3.*\"]\nclassifiers = [\"License :: OSI Approved :: Apache Software License\"]\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_dependency_group_problems() {
    let project = Project::with_pyproject(
        b"[dependency-groups]\ntest = [\"pytest\"]\ndev = [{ include-group = \"tests\" }]\nlint = [{ include-group = \"docs\" }]\ndocs = [{ include-group = \"lint\" }]\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_classifier_problems() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\nclassifiers = [\n  \"Programming Language :: Pythn :: 3\",\n  \"Programming Language :: Python :: 3.10\",\n]\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_features_the_build_backend_lacks() {
    let project = Project::with_pyproject(
        b"[build-system]\nrequires = [\"setuptools>=61,<77\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = \"demo\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_ruff_option_problems() {
    let project = Project::with_pyproject(
        b"[dependency-groups]\ndev = [\"ruff>=0.5,<0.6\"]\n\n[tool.ruff]\nline-lenght = 88\nselect = [\"E\"]\nignore = [\"E501\"]\n\n[tool.ruff.analyze]\ndetect-string-imports = true\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_ruff_rule_selector_problems() {
    let project = Project::with_pyproject(
        b"[dependency-groups]\ndev = [\"ruff==0.16.8\"]\n\n[tool.ruff.lint]\nselect = [\"E\", \"E5O1\", \"ANN101\", \"PLR1701\", \"AIR003\"]\nignore = [\"ANN102\"]\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_ty_problems() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\n\n[dependency-groups]\ndev = [\"ty>=0.0.40\"]\n\n[tool.ty.environment]\npython-version = \"3.12\"\n\n[tool.ty.rules]\nunresolved-imprt = \"error\"\nmissing-direct-dependency = \"warn\"\n\n[[tool.ty.overrides]]\ninclude = [\"tests/**\"]\nrule = { unresolved-reference = \"ignore\" }\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_uv_problems() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[tool.uv]\nrequired-version = \">=0.9\"\nindex-ur = \"https://example.com/simple\"\nmanaged = \"yes\"\ndev-dependencies = [\"pytest\"]\nexclude-dependencies = [\"idna\"]\n\n[[tool.uv.index]]\nname = \"internal\"\nexplict = true\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_uv_reference_problems() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\ndependencies = [\"torch\"]\n\n[dependency-groups]\ndev = [\"pytest\"]\n\n[tool.uv]\ndefault-groups = [\"test\"]\n\n[tool.uv.sources]\ntorch = { index = \"pytorch-cu\" }\nrequests = { git = \"https://github.com/psf/requests\", tag = \"v2.32.0\", branch = \"main\" }\n\n[[tool.uv.index]]\nname = \"pytorch-cpu\"\nurl = \"https://download.pytorch.org/whl/cpu\"\nexplicit = true\n",
    );
    snapshot!(project.check());
}

#[test]
fn reports_ruff_value_problems() {
    let project = Project::with_pyproject(
        b"[project]\nname = \"demo\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\n\n[dependency-groups]\ndev = [\"ruff>=0.12,<0.13\"]\n\n[tool.ruff]\ntarget-version = \"py314\"\nline-length = \"88\"\n\n[tool.ruff.format]\nquote-style = \"Single\"\n",
    );
    snapshot!(project.check());
}
