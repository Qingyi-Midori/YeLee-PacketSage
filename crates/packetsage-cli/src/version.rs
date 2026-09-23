//! `version` normalisation (CLI 收口工程规格书 §5).
//!
//! Two presentation forms share one source of truth: the schema version is
//! always read from `packetsage_protocol::SCHEMA_VERSION` at runtime, never
//! baked in at build time (that is what doctor check 10 verifies).

/// Engine version from the crate metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Event schema version (single source of truth: the protocol crate).
#[must_use]
pub fn schema_version() -> u32 {
    packetsage_protocol::SCHEMA_VERSION
}

/// Short git revision, `unknown` for tarball builds.
#[must_use]
pub fn git_hash() -> &'static str {
    option_env!("PACKETSAGE_GIT_HASH").unwrap_or("unknown")
}

/// RFC3339 build timestamp.
#[must_use]
pub fn build_time() -> &'static str {
    option_env!("PACKETSAGE_BUILD_TIME").unwrap_or("unknown")
}

/// Build profile (`debug` / `release`).
#[must_use]
pub fn profile() -> &'static str {
    option_env!("PACKETSAGE_PROFILE").unwrap_or("unknown")
}

/// rustc version used for the build.
#[must_use]
pub fn rustc() -> &'static str {
    option_env!("PACKETSAGE_RUSTC").unwrap_or("unknown")
}

/// Build target triple.
#[must_use]
pub fn target() -> &'static str {
    option_env!("PACKETSAGE_TARGET").unwrap_or("unknown")
}

/// One line form, identical for `version` and `--version` (§5).
///
/// ```text
/// packetsage 3.8.1 (schema_version=2, git=1a2b3c4d, built=2026-09-20T03:40:00Z, profile=release)
/// ```
#[must_use]
pub fn line() -> String {
    format!(
        "packetsage {} (schema_version={}, git={}, built={}, profile={})",
        VERSION,
        schema_version(),
        git_hash(),
        build_time(),
        profile()
    )
}

/// Machine readable form: stable *json output format v1* (§5).
///
/// The key set and its order are part of the contract; adding or renaming a
/// key means a new format version. `git` ↔ `git_hash` and `built` ↔
/// `build_time` are the only renamed pairs.
#[must_use]
pub fn json() -> String {
    format!(
        concat!(
            "{{\"version\":\"{}\",\"schema_version\":{},\"git_hash\":\"{}\",",
            "\"build_time\":\"{}\",\"profile\":\"{}\",\"rustc\":\"{}\",",
            "\"target\":\"{}\"}}"
        ),
        VERSION,
        schema_version(),
        git_hash(),
        build_time(),
        profile(),
        rustc(),
        target()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_json_agree_with_the_protocol_constant() {
        let line = line();
        assert!(line.contains(&format!("schema_version={}", schema_version())));
        assert!(line.contains(&format!("packetsage {VERSION}")));
    }

    #[test]
    fn json_keys_are_the_stable_v1_set() {
        let json = json();
        for key in [
            "\"version\"",
            "\"schema_version\"",
            "\"git_hash\"",
            "\"build_time\"",
            "\"profile\"",
            "\"rustc\"",
            "\"target\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        assert!(json.contains(&format!("\"schema_version\":{}", schema_version())));
    }

    #[test]
    fn json_key_order_is_frozen() {
        // G1-5 froze the implementation spelling (`git_hash` / `build_time`)
        // instead of adding README aliases; the order is part of the contract.
        let json = json();
        let mut positions = Vec::new();
        for key in [
            "\"version\"",
            "\"schema_version\"",
            "\"git_hash\"",
            "\"build_time\"",
            "\"profile\"",
            "\"rustc\"",
            "\"target\"",
        ] {
            positions.push(json.find(key).expect("every frozen key is present"));
        }
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "json output format v1 key order changed: {json}"
        );
        assert!(!json.contains("\"git\":"));
        assert!(!json.contains("\"built\":"));
    }
}
