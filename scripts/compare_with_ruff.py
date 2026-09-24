# /// script
# requires-python = ">=3.11"
# ///
"""Compare pyprojx's verdicts on Ruff rule selectors with Ruff's own.

For each Ruff release given, checks configurations selecting and ignoring a
sample of rule selectors, with and without preview, using both that release of
Ruff and pyprojx (with `required-version` pinned to the release), and reports
where one fails, warns, or passes and the other does not.

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
        "[tool.ruff.lint]",
        f'[tool.ruff]\nrequired-version = "=={version}"\n[tool.ruff.lint]',
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
    mismatches = total = 0
    for version in args.versions:
        for option in ("select", "ignore"):
            for preview in ("false", "true"):
                for selector in selectors:
                    config = (
                        '[project]\nname = "demo"\nversion = "1"\n'
                        f'[tool.ruff.lint]\n{option} = ["{selector}"]\npreview = {preview}\n'
                    )
                    expected = ruff(version, config)
                    found = pyprojx(args.binary, version, config)
                    total += 1
                    if expected != found:
                        mismatches += 1
                        print(
                            f"Ruff {version}, {option} = [{selector!r}], preview = {preview}: "
                            f"Ruff {expected}, pyprojx {found}"
                        )
    print(f"{mismatches} mismatches in {total} comparisons")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
