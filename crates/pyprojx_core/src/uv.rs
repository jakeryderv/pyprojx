//! uv's configuration options across releases.
//!
//! The data is generated from each release's configuration schema, and from
//! running uv, by `scripts/update_uv_data.py`.

use crate::tool::Tool;
use crate::uv_data::{INVALID_VALUES, OPTIONS, RELEASES, REQUIRED, SOURCE_KINDS, UNKNOWN_KEYS};

/// uv's configuration, `[tool.uv]`.
pub static UV: Tool = Tool {
    name: "uv",
    table: "tool.uv",
    docs: "https://docs.astral.sh/uv/reference/settings/",
    upgrade: "set `required-version = \">={version}\"`",
    releases: RELEASES,
    options: OPTIONS,
    required: REQUIRED,
    unknown_keys: UNKNOWN_KEYS,
    invalid_values: INVALID_VALUES,
    // Measured by running uv: a setting it cannot read makes it skip the
    // file's settings, while it still reads the project's sources.
    warning: Some(
        "uv warns about it and then ignores every other `[tool.uv]` setting too, such as `index-url` and `[[tool.uv.index]]`, though it still reads `sources`",
    ),
};

/// The first release that rejects two indexes with the same name, found by
/// running releases; earlier ones accept them.
pub const DUPLICATE_INDEX_NAMES_REJECTED_SINCE: &str = "0.6.4";

/// The first release that rejects more than one default index, found by
/// running releases; earlier ones accept them.
pub const MULTIPLE_DEFAULT_INDEXES_REJECTED_SINCE: &str = "0.10.0";

/// The kinds of `tool.uv.sources` entries in the latest release, by the key
/// each requires, such as `git`, with the keys each allows.
pub fn source_kinds() -> &'static [(&'static str, &'static [&'static str])] {
    SOURCE_KINDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Treatment;

    #[test]
    fn data_is_consistent() {
        crate::tool::tests::assert_consistent(&UV);
    }

    #[test]
    fn lookups() {
        let latest = UV.latest();
        assert!(UV.option("index.url").unwrap().is_present(latest));
        assert!(UV.is_required("index.url"));
        assert_eq!(UV.unknown_key(""), Treatment::Warns);
        assert_eq!(UV.unknown_key("index"), Treatment::Ignores);
        assert_eq!(UV.unknown_key("workspace"), Treatment::Rejects);
        assert_eq!(UV.invalid_value("index-url"), Treatment::Warns);
        assert_eq!(UV.invalid_value("managed"), Treatment::Rejects);
        let dev = UV.option("dev-dependencies").unwrap();
        assert!(dev.is_deprecated(latest) && dev.message.is_some());
    }
}
