# /// script
# requires-python = ">=3.11"
# dependencies = ["packaging"]
# ///
"""Find which build backend versions support which pyproject.toml features.

For each backend and feature, bisects the backend's releases on PyPI, building
a small project with `uv build` and checking the wheel's metadata, and writes
the first supporting version of each to pyprojx's embedded data. Each build
resolves the build environment as of the backend release's upload date, so old
releases get the dependencies they shipped with.

This needs the network, so it runs by hand rather than in CI:

    uv run scripts/calibrate_backends.py            # print results
    uv run scripts/calibrate_backends.py --write    # also update the data file

Bisecting assumes that once a release supports a feature, later ones do too.
To catch histories where that is not so, releases spread across each side of
the threshold are also built, and the release before the threshold is built
without the feature, to show that it fails because of the feature. Results
that fail these checks are reported as unverified and are not written.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import json
import shutil
import subprocess
import sys
import tempfile
import textwrap
import urllib.request
import zipfile
from dataclasses import dataclass, field
from pathlib import Path

from packaging.version import InvalidVersion, Version

OUTPUT = Path(__file__).resolve().parents[1] / "crates/pyprojx_core/src/backend_data.rs"
PYTHON = "3.11"


@dataclass(frozen=True)
class Backend:
    """A build backend: its distribution, its PEP 517 module, and its project layout.

    `first` skips releases that cannot build anything; pyprojx says nothing
    about versions before it.
    """

    name: str
    module: str
    layout: str = "flat"
    first: str | None = None


BACKENDS = [
    Backend("setuptools", "setuptools.build_meta"),
    Backend("hatchling", "hatchling.build"),
    Backend("flit-core", "flit_core.buildapi"),
    Backend("pdm-backend", "pdm.backend"),
    Backend("poetry-core", "poetry.core.masonry.api"),
    # 0.6.3 to 0.6.5 ship a binary without the `build-backend` command.
    Backend("uv-build", "uv_build", layout="src", first="0.6.6"),
]


@dataclass(frozen=True)
class Feature:
    """A feature: its Rust variant, the project lines using it, and the metadata proving it worked."""

    variant: str
    lines: str
    expected: tuple[str, ...]
    metadata_version: str | None = None
    files: dict[str, str] = field(default_factory=dict)


FEATURES = [
    # Backends that ignore `[project]` still build, but not with its metadata.
    Feature("ProjectTable", "", ("Name: demo", "Version: 0.1.0")),
    # PEP 639 fields belong to metadata 2.4; some backends wrote them earlier
    # under older versions, which PyPI rejects.
    Feature(
        "LicenseExpression", 'license = "MIT"', ("License-Expression: MIT",), "2.4"
    ),
    # A name no backend includes by default, so only `license-files` adds it.
    Feature(
        "LicenseFiles",
        'license-files = ["TERMS.txt"]',
        ("License-File: TERMS.txt",),
        "2.4",
        files={"TERMS.txt": "Terms\n"},
    ),
]


@dataclass
class Result:
    backend: str
    feature: str
    # The last release without the feature and the first with it; `None` for
    # either when every release, or none, supports it.
    before: str | None
    since: str | None
    probes: dict[Version, str]
    problem: str = ""
    # Whether builds fail with every checked release that reads `[project]` but
    # lacks the feature, rather than leaving the feature out of the metadata.
    fails: bool = False


def set_fails(results: list[Result]) -> None:
    """Sets `fails` for each result, from the releases that read `[project]`."""
    for backend in BACKENDS:
        rows = [row for row in results if row.backend == backend.name]
        project = next(row for row in rows if row.feature == "ProjectTable")
        reads_project = Version(project.since) if project.before else Version("0")
        for row in rows:
            if row.feature == "ProjectTable":
                # The demo project has no backend-specific metadata, so builds
                # fail where real projects might fall back to it.
                continue
            outcomes = [
                outcome
                for version, outcome in row.probes.items()
                if version >= reads_project and outcome != SUPPORTED
            ]
            row.fails = bool(outcomes) and all(
                outcome == FAILED for outcome in outcomes
            )


def releases(backend: Backend) -> list[tuple[Version, str]]:
    """Final, unyanked releases with files, oldest first, with their upload times."""
    with urllib.request.urlopen(
        f"https://pypi.org/pypi/{backend.name}/json"
    ) as response:
        data = json.load(response)
    first = Version(backend.first) if backend.first else None
    found = []
    for raw, files in data["releases"].items():
        try:
            version = Version(raw)
        except InvalidVersion:
            continue
        files = [file for file in files if not file["yanked"]]
        if version.is_prerelease or version.is_devrelease or not files:
            continue
        if first and version < first:
            continue
        uploaded = max(file["upload_time_iso_8601"] for file in files)
        found.append((version, uploaded))
    return sorted(found)


SUPPORTED = "yes"
IGNORED = "ignored"
FAILED = "fails"


def build(
    backend: Backend, version: Version, uploaded: str, feature: Feature | None
) -> str:
    """Builds the demo project, with `feature` if given, and checks its metadata.

    Returns `SUPPORTED`, `IGNORED` if the build works but the metadata lacks
    the feature, or `FAILED`.
    """
    with tempfile.TemporaryDirectory(prefix="pyprojx-calibrate-") as directory:
        root = Path(directory)
        package = root / ("src/demo" if backend.layout == "src" else "demo")
        package.mkdir(parents=True)
        (package / "__init__.py").write_text('"""Demo."""\n\n__version__ = "0.1.0"\n')
        for name, content in (feature.files if feature else {}).items():
            (root / name).write_text(content)
        project = textwrap.dedent(f"""\
            [build-system]
            requires = ["{backend.name}=={version}"]
            build-backend = "{backend.module}"

            [project]
            name = "demo"
            version = "0.1.0"
            description = "Demo"
            """)
        if feature and feature.lines:
            project += feature.lines + "\n"
        (root / "pyproject.toml").write_text(project)
        # The environment as of a day after the release, so that the release
        # itself and its dependencies at the time are available.
        cutoff = datetime.datetime.fromisoformat(uploaded) + datetime.timedelta(days=1)
        process = subprocess.run(
            [
                "uv",
                "build",
                "--no-config",
                "--force-pep517",
                "--quiet",
                "--python",
                PYTHON,
                "--exclude-newer",
                cutoff.strftime("%Y-%m-%dT%H:%M:%SZ"),
                "--out-dir",
                "dist",
            ],
            cwd=root,
            capture_output=True,
            check=False,
        )
        wheels = list((root / "dist").glob("*.whl"))
        if process.returncode != 0 or len(wheels) != 1:
            return FAILED
        if feature is None:
            return SUPPORTED
        with zipfile.ZipFile(wheels[0]) as wheel:
            name = next(
                n for n in wheel.namelist() if n.endswith(".dist-info/METADATA")
            )
            lines = wheel.read(name).decode().splitlines()
    if not all(line in lines for line in feature.expected):
        return IGNORED
    if feature.metadata_version:
        found = next(line for line in lines if line.startswith("Metadata-Version: "))
        if Version(found.split(": ")[1]) < Version(feature.metadata_version):
            return IGNORED
    return SUPPORTED


def spread(indexes: range, count: int) -> list[int]:
    """Up to `count` indexes spread evenly over `indexes`, including both ends."""
    if len(indexes) <= count:
        return list(indexes)
    step = (len(indexes) - 1) / (count - 1)
    return sorted({indexes[round(i * step)] for i in range(count)})


def calibrate(
    backend: Backend, feature: Feature, history: list[tuple[Version, str]], samples: int
) -> Result:
    probes: dict[Version, str] = {}

    def supports(index: int) -> bool:
        version, uploaded = history[index]
        if version not in probes:
            probes[version] = build(backend, version, uploaded, feature)
        return probes[version] == SUPPORTED

    def works_without_feature(index: int) -> bool:
        version, uploaded = history[index]
        return build(backend, version, uploaded, None) == SUPPORTED

    last = len(history) - 1
    if not supports(last):
        threshold = last + 1
        if not works_without_feature(last):
            return Result(
                backend.name,
                feature.variant,
                None,
                None,
                probes,
                "the latest release fails without the feature too",
            )
    elif supports(0):
        threshold = 0
    else:
        low, high = 0, last  # `low` fails and `high` passes
        while high - low > 1:
            middle = (low + high) // 2
            if supports(middle):
                high = middle
            else:
                low = middle
        threshold = high
        # `[project]` is what the baseline build uses, so it cannot be checked this way.
        if feature.variant != "ProjectTable" and not works_without_feature(low):
            problem = f"{history[low][0]} fails without the feature too"
            return Result(backend.name, feature.variant, None, None, probes, problem)

    inconsistent = [
        history[index][0]
        for index in spread(range(threshold), samples)
        + spread(range(threshold, last + 1), samples)
        if supports(index) != (index >= threshold)
    ]
    before = str(history[threshold - 1][0]) if threshold > 0 else None
    since = str(history[threshold][0]) if threshold <= last else None
    problem = (
        f"not monotonic; see {', '.join(map(str, inconsistent))}"
        if inconsistent
        else ""
    )
    return Result(backend.name, feature.variant, before, since, probes, problem)


def rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def generate(
    results: list[Result], histories: dict[str, list[tuple[Version, str]]], uv: str
) -> str:
    lines = [
        "// @generated by scripts/calibrate_backends.py; do not edit.",
        f"// Calibrated on {datetime.datetime.now(datetime.UTC).date().isoformat()} with {uv} and Python {PYTHON}.",
        "",
        "use crate::backends::{BackendData, Feature, FeatureSupport, Since};",
        "",
        "pub const BACKENDS: &[BackendData] = &[",
    ]
    for backend in BACKENDS:
        history = histories[backend.name]
        lines += [
            "    BackendData {",
            f"        name: {rust_string(backend.name)},",
            f"        module: {rust_string(backend.module)},",
            f"        first: {rust_string(str(history[0][0]))},",
            f"        latest: {rust_string(str(history[-1][0]))},",
            "        features: &[",
        ]
        for row in (r for r in results if r.backend == backend.name):
            builds = ", ".join(
                f"{v} {outcome}" for v, outcome in sorted(row.probes.items())
            )
            if row.before is None:
                since = "Since::First"
            elif row.since is None:
                since = "Since::Never"
            else:
                since = f"Since::Version {{ before: {rust_string(row.before)}, version: {rust_string(row.since)} }}"
            lines += [
                f"            // Supported: {builds}.",
                f"            FeatureSupport {{ feature: Feature::{row.feature}, since: {since}, fails: {str(row.fails).lower()} }},",
            ]
        lines += ["        ],", "    },"]
    lines += ["];", ""]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", action="store_true", help=f"update {OUTPUT.name}")
    parser.add_argument("--jobs", type=int, default=8, help="builds to run at once")
    parser.add_argument(
        "--samples",
        type=int,
        default=8,
        help="releases to check on each side of a threshold",
    )
    args = parser.parse_args()
    if not shutil.which("uv"):
        print("uv is required", file=sys.stderr)
        return 1
    uv = subprocess.run(
        ["uv", "--version"], capture_output=True, text=True, check=True
    ).stdout.split(" (")[0]

    histories = {backend.name: releases(backend) for backend in BACKENDS}
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        futures = [
            pool.submit(
                calibrate, backend, feature, histories[backend.name], args.samples
            )
            for backend in BACKENDS
            for feature in FEATURES
        ]
        results = [future.result() for future in futures]
    set_fails(results)

    for row in results:
        since = "first" if row.before is None else row.since or "never"
        problem = f"  UNVERIFIED: {row.problem}" if row.problem else ""
        fails = "fails" if row.fails else ""
        print(
            f"{row.backend:12} {row.feature:18} {since:12} {fails:6} ({len(row.probes)} builds){problem}"
        )
    if any(row.problem for row in results):
        print("some results are unverified; not writing", file=sys.stderr)
        return 1
    if args.write:
        OUTPUT.write_text(generate(results, histories, uv), encoding="utf-8")
        print(f"wrote {OUTPUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
