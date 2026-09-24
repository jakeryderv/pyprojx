# /// script
# requires-python = ">=3.11"
# ///
"""Compare pyprojx's verdicts on Ruff configurations with Ruff's own.

For each Ruff release given, checks configurations using both that release of
Ruff and pyprojx (with `required-version` pinned to the release), and reports
where one fails, warns, or passes and the other does not. The configurations
select and ignore a sample of rule selectors, with and without preview, and
set options to values whose validity differs between releases.

Build pyprojx first. This runs Ruff many times and needs the network:

    cargo build
    uv run scripts/compare_with_ruff.py 0.16.8 0.12.0 0.5.0 0.1.0
"""

from __future__ import annotations

import argparse
import os
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "crates/pyprojx_core/src/ruff_data.rs"
# Removed, deprecated, redirected, preview, prefix, name, and unknown selectors.
SPECIAL = [
    "ANN101", "PLR1701", "AIR003", "B04", "E501", "E", "T1", "C9",
    "too-many-positional-arguments", "XYZ123", "E5O1",
]  # fmt: skip


# Settings under `[tool.ruff]`, as TOML lines.
VALUES = [
    'target-version = "py313"', 'target-version = "py314"', 'target-version = "py315"',
    'target-version = "py399"', 'output-format = "text"', 'output-format = "concise"',
    'output-format = "rdjson"', 'line-length = 0', 'line-length = 400',
    'line-length = "88"', 'fix = "yes"', "indent-width = 0", 'extend-exclude = "x"',
    "lint = 1", '[tool.ruff.format]\nquote-style = "preserve"',
    '[tool.ruff.format]\nquote-style = "Single"', '[tool.ruff.format]\nline-ending = "cr-lf"',
    "[tool.ruff.lint.pylint]\nmax-args = -1",
    '[tool.ruff.lint.pylint]\nallow-magic-value-types = ["tuple"]',
    '[tool.ruff.lint.pydocstyle]\nconvention = "pep8"',
    '[tool.ruff.analyze]\ndirection = "Dependencies"',
    '[tool.ruff.analyze]\ndirection = "dependencies"',
    "[tool.ruff.lint]\nper-file-ignores = 1",
]  # fmt: skip


def verdict(output: str, failed: bool) -> str:
    if failed:
        return "error"
    return "warning" if re.search(r"^warning", output, re.MULTILINE) else "ok"


def ruff(version: str, config: str) -> str:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "a.py").write_text("x = 1\n")
        (root / "pyproject.toml").write_text(config)
        process = subprocess.run(
            ["uvx", "--quiet", f"ruff@{version}", "check", "--no-cache", "a.py"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
    return verdict(process.stdout + process.stderr, process.returncode == 2)


def pyprojx(binary: Path, version: str, config: str) -> str:
    pinned = config.replace(
        "[tool.ruff]\n", f'[tool.ruff]\nrequired-version = "=={version}"\n', 1
    )
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "pyproject.toml"
        path.write_text(pinned)
        process = subprocess.run(
            [binary, "check", path],
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, "NO_COLOR": "1"},
        )
    failed = re.search(r"^error", process.stdout, re.MULTILINE) is not None
    return verdict(process.stdout, failed)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("versions", nargs="+", help="Ruff releases to compare")
    parser.add_argument(
        "--sample", type=int, default=25, help="random rule codes to add"
    )
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/pyprojx")
    args = parser.parse_args()

    codes = re.findall(r'selector: "([A-Z][A-Z0-9]*)"', DATA.read_text())
    selectors = random.Random(args.seed).sample(codes, args.sample) + SPECIAL
    header = '[project]\nname = "demo"\nversion = "1"\n[tool.ruff]\n'
    configs = [
        f"{header}[tool.ruff.lint]\n{option} = [{selector!r}]\npreview = {preview}\n"
        for option in ("select", "ignore")
        for preview in ("false", "true")
        for selector in selectors
    ]
    configs += [f"{header}{value}\n" for value in VALUES]
    mismatches = total = 0
    for version in args.versions:
        for config in configs:
            expected = ruff(version, config)
            found = pyprojx(args.binary, version, config)
            total += 1
            if expected != found:
                mismatches += 1
                settings = config.removeprefix(header).replace("\n", "; ").strip("; ")
                print(f"Ruff {version}, {settings}: Ruff {expected}, pyprojx {found}")
    print(f"{mismatches} mismatches in {total} comparisons")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
