# /// script
# requires-python = ">=3.11"
# ///
"""Compare pyprojx's verdicts on uv configurations with uv's own.

For each uv release given, runs `uv lock` on projects with each configuration
and checks them with pyprojx, both pinned to the release with
`required-version`, and reports where one fails, warns, or passes and the other
does not. The configurations set options and values whose validity, or uv's
treatment of them, differs between options and releases. `required-version`
needs uv 0.5.14 or later.

Build pyprojx first. This runs uv many times and needs the network:

    cargo build
    uv run scripts/compare_with_uv.py 0.12.18 0.9.0 0.7.0 0.5.14
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADER = '[project]\nname = "demo"\nversion = "1"\nrequires-python = ">=3.8"\n'
INDEX = 'url = "https://example.com/simple"'
# Settings under `[tool.uv]`, as TOML lines.
SETTINGS = [
    "managed = true",
    'managed = "yes"',
    "package = 1",
    'index-url = "https://example.com/simple"',
    "index-url = 1",
    'index-strategy = "first"',
    'index-strategy = "unsafe-best-match"',
    "index-ur = 1",
    "dev-dependencies = []",
    "native-tls = true",
    "system-certs = true",
    "python-fetch = 'automatic'",
    "python-downloads = 'automatic'",
    "exclude-dependencies = ['idna']",
    "default-groups = 'all'",
    "default-groups = ['dev']",
    "conflicts = [[{ extra = 'a' }, { extra = 'b' }]]",
    "environments = ['sys_platform == \"linux\"']",
    "environments = 1",
    "sources = 1",
    "workspace = { members = [] }",
    "workspace = { membrs = [] }",
    "pip = { group = ['dev'] }",
    "pip = { group = [{ name = 'dev' }] }",
    "pip = { strict = 'yes' }",
    "pip = { bogus = 1 }",
    f"index = [{{ {INDEX} }}]",
    f"index = [{{ {INDEX}, explict = true }}]",
    "index = [{ name = 'x' }]",
    f"index = [{{ {INDEX}, explicit = 'yes' }}]",
    "index = 1",
    "dependency-metadata = [{ name = 'idna', version = '1.0' }]",
    "dependency-metadata = [{ version = '1.0' }]",
    "cache-keys = [{ file = 'pyproject.toml' }]",
    "concurrent-downloads = 0",
    "concurrent-downloads = -1",
    "preview-features = ['bogus-feature']",
]

# Where pyprojx intentionally disagrees: (setting, uv's verdict, pyprojx's).
DIFFERENCES = {
    # uv ignores unknown keys in indexes silently; pyprojx warns.
    (f"index = [{{ {INDEX}, explict = true }}]", "ok", "warning"),
    # uv warns about preview features it does not know; pyprojx does not check
    # their names yet.
    ("preview-features = ['bogus-feature']", "warning", "ok"),
}


def verdict(output: str, failed: bool) -> str:
    if failed:
        return "error"
    return "warning" if re.search(r"^warning", output, re.MULTILINE) else "ok"


def uv(version: str, config: str) -> str:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "pyproject.toml").write_text(HEADER + config)
        process = subprocess.run(
            [
                "uvx",
                "--quiet",
                "--from",
                f"uv=={version}",
                "uv",
                "lock",
                "--offline",
                "--no-cache",
            ],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
            # Without what `uv run` sets, such as `VIRTUAL_ENV`, which uv warns
            # about when it runs in a project.
            env={
                name: value
                for name, value in os.environ.items()
                if name not in ("VIRTUAL_ENV", "UV") and not name.startswith("UV_RUN_")
            },
        )
    return verdict(process.stdout + process.stderr, process.returncode != 0)


def pyprojx(binary: Path, config: str) -> str:
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "pyproject.toml"
        path.write_text(HEADER + config)
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
    parser.add_argument("versions", nargs="+", help="uv releases to compare")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/pyprojx")
    args = parser.parse_args()

    mismatches = total = 0
    for version in args.versions:
        for setting in SETTINGS:
            config = f'[tool.uv]\nrequired-version = "=={version}"\n{setting}\n'
            expected = uv(version, config)
            found = pyprojx(args.binary, config)
            total += 1
            if expected != found and (setting, expected, found) not in DIFFERENCES:
                mismatches += 1
                print(f"uv {version}, {setting}: uv {expected}, pyprojx {found}")
    print(f"{mismatches} mismatches in {total} comparisons")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
