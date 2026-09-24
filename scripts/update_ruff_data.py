# /// script
# requires-python = ">=3.11"
# dependencies = ["packaging"]
# ///
"""Generate pyprojx's data about Ruff's configuration across releases.

For every Ruff release since 0.1.0, reads the JSON schema of its configuration
(`ruff.schema.json` at the release's tag) and records which releases accept
each option and rule selector, and which deprecate each option. From the latest
release, it records option deprecation messages (`ruff config`), each rule's
status (`ruff rule --all`), and the rule codes Ruff redirects to others.

Schemas are cached, so later runs only download new releases. The script needs
the network, so it runs on a schedule (.github/workflows/ruff-data.yml) rather
than in CI, or by hand:

    uv run scripts/update_ruff_data.py
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import re
import subprocess
import sys
import urllib.request

from packaging.version import Version
from schema_history import (
    CACHE,
    ROOT,
    Option,
    collect,
    fetch,
    first_where,
    generate_options,
    probe,
    ranges,
    releases,
    rust_ranges,
    rust_string,
)

OUTPUT = ROOT / "crates/pyprojx_core/src/ruff_data.rs"
FIRST = Version("0.1.0")
RUFF_CACHE = CACHE / "ruff"


def schema(version: Version) -> dict | None:
    """The configuration schema at the release's tag, which some releases prefix
    with `v`, or `None` if the release has no tag."""
    urls = (
        f"https://raw.githubusercontent.com/astral-sh/ruff/{tag}/ruff.schema.json"
        for tag in (str(version), f"v{version}")
    )
    content = fetch(urls, RUFF_CACHE / f"{version}.schema.json")
    return None if content is None else json.loads(content)


def selectors(document: dict) -> list[str]:
    """The rule selectors a release accepts, such as `E`, `E5`, and `E501`."""
    return document["definitions"]["RuleSelector"]["enum"]


def ruff_config(version: Version) -> dict:
    """The options a release reports with `ruff config` (from Ruff 0.5)."""
    output = subprocess.run(
        ["uvx", "--quiet", f"ruff@{version}", "config", "--output-format", "json"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return json.loads(output)


def same_options(a: Version, b: Version) -> bool:
    """Whether two releases report the same options and deprecations."""

    def summary(version: Version) -> dict[str, bool]:
        return {
            path: bool(o.get("deprecated")) for path, o in ruff_config(version).items()
        }

    return summary(a) == summary(b)


def deprecation_messages(latest: Version) -> dict[str, str]:
    """Deprecation messages of the latest release's options, without Markdown links."""
    messages = {}
    for path, option in ruff_config(latest).items():
        deprecated = option.get("deprecated")
        if deprecated and deprecated.get("message"):
            message = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", deprecated["message"])
            messages[path] = " ".join(message.split())
    return messages


def rule_statuses(latest: Version, versions: list[Version]) -> dict[str, str]:
    """Each rule's status in the latest release, as Rust `RuleStatus` values."""
    output = subprocess.run(
        [
            "uvx",
            "--quiet",
            f"ruff@{latest}",
            "rule",
            "--all",
            "--output-format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    ).stdout

    def index(since: str) -> int:
        """The first release from `since` on; earlier versions map to the first."""
        since = Version(since.removeprefix("v"))
        return next(i for i, version in enumerate(versions) if version >= since)

    statuses = {}
    for rule in json.loads(output):
        [(kind, detail)] = rule["status"].items()
        if kind == "Preview":
            statuses[rule["code"]] = "RuleStatus::Preview"
        elif kind == "Stable":
            statuses[rule["code"]] = f"RuleStatus::Stable({index(detail['since'])})"
        elif kind == "Removed":
            statuses[rule["code"]] = f"RuleStatus::Removed({index(detail['since'])})"
        elif kind == "Deprecated":
            # Not seen yet; deprecated rules have so far been removed.
            raise RuntimeError(
                f"{rule['code']} is deprecated; record it like removed rules"
            )
        else:
            raise RuntimeError(f"unknown status {kind} for {rule['code']}")
    return statuses


def check(version: Version, config: str) -> str:
    """Ruff's output when a release checks a file with the given configuration."""
    return probe(
        RUFF_CACHE / "probes.json",
        ["uvx", "--quiet", f"ruff@{version}", "check", "--no-cache", "a.py"],
        {"a.py": "x = 1\n", "pyproject.toml": config},
    )


def warns_deprecated(version: Version, code: str) -> bool:
    """Whether a release warns that the rule is deprecated when it is selected."""
    output = check(version, f'[tool.ruff.lint]\nselect = ["{code}"]\n')
    return f"`{code}` is deprecated" in output


def python_in_development(
    found: dict[str, Option], versions: list[Version]
) -> list[tuple[str, int, int]]:
    """For each `target-version` value, the releases that warn that support for
    it is under development, as (value, first, last) index ranges."""
    target = found["target-version"]
    windows = []
    variants = sorted(
        {v for t in target.types.values() if t[0] == "enum" for v in t[1]}
    )
    for variant in variants:
        present = [i for i, t in sorted(target.types.items()) if variant in t[1]]
        config = f'[tool.ruff]\ntarget-version = "{variant}"\n'

        def warns(index: int, config: str = config) -> bool:
            return "is under development" in check(versions[index], config)

        if not warns(present[0]):
            continue
        stable = first_where(present, lambda index: not warns(index))
        last = present[-1] if stable is None else present[present.index(stable) - 1]
        windows.append((variant, present[0], last))
    return windows


# Values Ruff rejects although the schema still lists them, and the releases
# that deprecated them first, found with scripts/compare_with_ruff.py.
VALUE_OVERRIDES = [
    # (option, value, deprecated from, rejected from, message)
    ("output-format", "text", "0.2.0", "0.5.0", 'use "full" or "concise"'),
]


def apply_overrides(
    found: dict[str, Option], versions: list[Version]
) -> list[tuple[str, str, int, int, str]]:
    """Removes values Ruff rejects from their options' types, and returns the
    deprecation windows of those values."""
    deprecations = []
    for path, value, deprecated, rejected, message in VALUE_OVERRIDES:
        rejected_index = versions.index(Version(rejected))
        option = found[path]
        for index, value_type in option.types.items():
            if index >= rejected_index and value_type[0] == "enum":
                variants = tuple(v for v in value_type[1] if v != value)
                option.types[index] = ("enum", variants)
        first = versions.index(Version(deprecated))
        deprecations.append((path, value, first, rejected_index - 1, message))
    return deprecations


def deprecated_since(
    code: str, present: list[int], versions: list[Version]
) -> int | None:
    """The first release that deprecates a removed rule, found by bisecting the
    releases that accept it; `None` if none did."""
    return first_where(present, lambda index: warns_deprecated(versions[index], code))


def redirects(latest: Version) -> dict[str, str]:
    """Rule codes Ruff accepts and redirects to others, from its source."""
    url = f"https://raw.githubusercontent.com/astral-sh/ruff/{latest}/crates/ruff_linter/src/rule_redirects.rs"
    with urllib.request.urlopen(url) as response:
        source = response.read().decode()
    table = source[source.index("HashMap::from_iter([") : source.index("])")]
    pairs = dict(re.findall(r'\("([A-Z0-9]+)",\s*"([A-Z0-9]+)"\)', table))
    if not pairs:
        raise RuntimeError("no redirects found; has rule_redirects.rs changed?")
    return pairs


def generate(
    versions: list[Version],
    found: dict[str, Option],
    messages: dict[str, str],
    rules: dict[str, list[int]],
    statuses: dict[str, str],
    redirected: dict[str, str],
    development: list[tuple[str, int, int]],
    deprecated_values: list[tuple[str, str, int, int, str]],
) -> str:
    lines = [
        "// @generated by scripts/update_ruff_data.py; do not edit.",
        "",
        "use crate::ruff::{RuleStatus, SelectorData};",
        "use crate::tool::{OptionData, OptionKind, ValueType};",
        "",
    ]
    lines += generate_options("Ruff", versions, found, messages)
    lines += [
        "/// Rule selectors (codes, prefixes of codes, and rule names), sorted.",
        "pub const SELECTORS: &[SelectorData] = &[",
    ]
    for selector in sorted(rules):
        status = statuses.get(selector)
        status = f"Some({status})" if status else "None"
        lines.append(
            f"    SelectorData {{ selector: {rust_string(selector)}, "
            f"present: {rust_ranges(ranges(rules[selector]))}, status: {status} }},"
        )
    lines += [
        "];",
        "",
        "/// Rule codes Ruff redirects to others, sorted.",
        "pub const REDIRECTS: &[(&str, &str)] = &[",
    ]
    lines += [
        f"    ({rust_string(old)}, {rust_string(new)}),"
        for old, new in sorted(redirected.items())
    ]
    lines += [
        "];",
        "",
        "/// `target-version` values that releases support only in preview, with",
        "/// inclusive ranges of those releases.",
        "pub const PYTHON_IN_DEVELOPMENT: &[(&str, u16, u16)] = &[",
    ]
    lines += [
        f"    ({rust_string(value)}, {first}, {last}),"
        for value, first, last in development
    ]
    lines += [
        "];",
        "",
        "/// Option values that releases deprecate: option, value, inclusive range of",
        "/// releases, and what to use instead.",
        "pub const DEPRECATED_VALUES: &[(&str, &str, u16, u16, &str)] = &[",
    ]
    lines += [
        f"    ({rust_string(path)}, {rust_string(value)}, {first}, {last}, {rust_string(message)}),"
        for path, value, first, last, message in deprecated_values
    ]
    lines += ["];", ""]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.parse_args()
    versions = releases("ruff", FIRST)
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        schemas = list(pool.map(schema, versions))
    # A few releases have no tag. Use the previous release's schema if the two
    # report the same options.
    for index, document in enumerate(schemas):
        if document is None:
            previous = versions[index - 1]
            if index == 0 or not same_options(previous, versions[index]):
                print(f"no schema for Ruff {versions[index]}", file=sys.stderr)
                return 1
            print(f"Ruff {versions[index]} has no tag; using {previous}'s schema")
            schemas[index] = schemas[index - 1]

    found = collect(versions, schemas, {"RuleSelector": ("selector",)})
    rules: dict[str, list[int]] = {}
    for index, document in enumerate(schemas):
        for selector in selectors(document):
            rules.setdefault(selector, []).append(index)

    latest = versions[-1]
    statuses = rule_statuses(latest, versions)
    for code, status in statuses.items():
        if status.startswith("RuleStatus::Removed"):
            # Removed rules no known release accepts are recorded for their messages.
            present = rules.setdefault(code, [])
            since = deprecated_since(code, present, versions)
            removed = status.removeprefix("RuleStatus::Removed(").removesuffix(")")
            deprecated = "None" if since is None else f"Some({since})"
            statuses[code] = (
                f"RuleStatus::Removed {{ deprecated: {deprecated}, removed: {removed} }}"
            )
    deprecated_values = apply_overrides(found, versions)
    content = generate(
        versions,
        found,
        deprecation_messages(latest),
        rules,
        statuses,
        redirects(latest),
        python_in_development(found, versions),
        deprecated_values,
    )
    OUTPUT.write_text(content, encoding="utf-8")
    print(
        f"wrote {len(found)} options and {len(rules)} rule selectors "
        f"across {len(versions)} releases to {OUTPUT}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
