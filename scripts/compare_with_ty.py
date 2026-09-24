# /// script
# requires-python = ">=3.11"
# ///
"""Compare pyprojx's verdicts on ty configurations with ty's own.

For each ty release given, checks configurations using both that release of ty
and pyprojx (with `ty` pinned to the release in a dependency group), and
reports where one fails, warns, or passes and the other does not. The
configurations set a sample of rules and options whose validity differs
between releases.

Build pyprojx first. This runs ty many times and needs the network:

    cargo build
    uv run scripts/compare_with_ty.py 0.0.83 0.0.60 0.0.30 0.0.2
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
DATA = ROOT / "crates/pyprojx_core/src/ty_data.rs"
HEADER = '[project]\nname = "demo"\nversion = "1"\n'
# Settings under `[tool.ty]`, as TOML lines.
SETTINGS = [
    'rules.unresolved-imprt = "error"',
    'rules.unresolved-import = "fatal"',
    'rules.all = "warn"',
    'environment.python-version = "3.14"',
    'environment.python-version = "3.15"',
    'environment.python-version = "3.16"',
    'environment.python-version = "3.6"',
    'environment.python-platform = "linux"',
    'environment.extra-paths = "typestubs"',
    "environment.bogus = 1",
    'src.root = "."',
    "src.exclude-scripts = true",
    'terminal.output-format = "junit"',
    'terminal.output-format = "json"',
    'terminal.error-on-warning = "yes"',
    "analysis.respect-type-ignore-comments = false",
    "analysis.strict-literal-narrowing = true",
    "analysis.strict-equality-semantics = true",
    'overrides = [{ include = ["x"], rules = { unresolved-import = "ignore" } }]',
    'overrides = [{ include = ["x"], bogus = 1 }]',
    'overrides = [{ include = ["x"], rules = { unresolved-imprt = "ignore" } }]',
    'overrides = [{ include = ["x"], analysis = { respect-type-ignore-comments = false } }]',
    "overrides = 1",
    "rules = 1",
]

# Where pyprojx intentionally disagrees: (setting, ty's verdict, pyprojx's).
DIFFERENCES = {
    # ty accepts the old name of a renamed option silently; pyprojx warns.
    ("analysis.strict-literal-narrowing = true", "ok", "warning"),
}


def verdict(output: str, failed: bool) -> str:
    if failed:
        return "error"
    return "warning" if re.search(r"^warning", output, re.MULTILINE) else "ok"


def ty(version: str, config: str) -> str:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "a.py").write_text("x = 1\n")
        (root / "pyproject.toml").write_text(HEADER + config)
        process = subprocess.run(
            ["uvx", "--quiet", f"ty@{version}", "check", "--no-progress", "a.py"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
    return verdict(process.stdout + process.stderr, process.returncode == 2)


def pyprojx(binary: Path, version: str, config: str) -> str:
    pinned = f'{HEADER}[dependency-groups]\ndev = ["ty=={version}"]\n{config}'
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
    parser.add_argument("versions", nargs="+", help="ty releases to compare")
    parser.add_argument("--sample", type=int, default=20, help="random rules to add")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/pyprojx")
    args = parser.parse_args()

    rules = re.findall(r'^    \("([a-z-]+)", &\[', DATA.read_text(), re.MULTILINE)
    sample = random.Random(args.seed).sample(rules, args.sample)
    configs = [f'[tool.ty.rules]\n{rule} = "warn"\n' for rule in sample]
    configs += [f"[tool.ty]\n{setting}\n" for setting in SETTINGS]
    mismatches = total = 0
    for version in args.versions:
        for config in configs:
            expected = ty(version, config)
            found = pyprojx(args.binary, version, config)
            total += 1
            setting = config.removeprefix("[tool.ty]\n").strip()
            if expected != found and (setting, expected, found) not in DIFFERENCES:
                mismatches += 1
                settings = config.replace("\n", "; ").strip("; ")
                print(f"ty {version}, {settings}: ty {expected}, pyprojx {found}")
    print(f"{mismatches} mismatches in {total} comparisons")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
