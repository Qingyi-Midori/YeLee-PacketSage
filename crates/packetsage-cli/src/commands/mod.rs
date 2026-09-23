//! Sub-command implementations: parsing, assembly and formatting only.

pub mod analyze;
pub mod db;
pub mod query;
pub mod rules;
pub mod schema;
pub mod version;

use std::path::{Path, PathBuf};

/// Resolves the effective rules directory.
///
/// `rules/builtin` is preferred when it exists, otherwise the directory itself.
#[must_use]
pub fn builtin_rules_dir(rules_dir: &Path) -> PathBuf {
    let candidate = rules_dir.join("builtin");
    if candidate.is_dir() {
        candidate
    } else {
        rules_dir.to_path_buf()
    }
}

/// Formats a byte count for the human readable output.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Formats a nanosecond timestamp as seconds with microsecond precision.
#[must_use]
pub fn human_seconds(ns: i128) -> String {
    format!("{:.6}", (ns as f64) / 1_000_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.00 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.00 MiB");
    }

    #[test]
    fn human_seconds_is_microsecond_precise() {
        assert_eq!(human_seconds(1_500_000_000), "1.500000");
    }

    #[test]
    fn rules_dir_falls_back_to_the_directory() {
        assert_eq!(
            builtin_rules_dir(Path::new("rules-nonexistent")),
            PathBuf::from("rules-nonexistent")
        );
    }
}
