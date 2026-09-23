//! Rule directory scanning, strict / lenient loading and content hashing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Result, RuleError};
use crate::schema::{window_ns, RuleFile, RuleIssue};

/// One loaded rule plus its provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRule {
    /// Parsed rule file.
    pub file: RuleFile,
    /// Path the rule came from (`<embedded>` for built-ins).
    pub source: PathBuf,
    /// SHA-256 of the YAML text.
    pub content_hash: String,
    /// Window length in nanoseconds.
    pub window_ns: i128,
    /// Cooldown in nanoseconds.
    pub cooldown_ns: i128,
}

impl CompiledRule {
    /// Builds a compiled rule from text.
    ///
    /// # Errors
    /// Returns [`RuleError::Schema`] when the YAML is invalid.
    pub fn from_text(text: &str, source: PathBuf) -> Result<Self> {
        let file = RuleFile::parse(text)?;
        let window = window_ns(&file.threshold.window)?;
        let cooldown = window_ns(&file.dedupe.per_group_cooldown)?;
        Ok(Self {
            file,
            source,
            content_hash: sha256_hex(text),
            window_ns: window,
            cooldown_ns: cooldown,
        })
    }

    /// First eight hex characters of the content hash.
    #[must_use]
    pub fn short_hash(&self) -> String {
        self.content_hash.chars().take(8).collect()
    }
}

/// Rule that could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetter {
    /// File the rule came from.
    pub path: PathBuf,
    /// Why it was rejected.
    pub reason: String,
}

/// Result of a lenient load.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Successfully loaded rules.
    pub loaded: usize,
    /// Rules that were skipped.
    pub dead_letters: Vec<DeadLetter>,
    /// Non-blocking warnings.
    pub warnings: Vec<String>,
}

/// Loads every `*.yaml` / `*.yml` in `dir` in strict mode (S1-S9 enforced).
///
/// # Errors
/// Returns [`RuleError`] on the first invalid rule.
pub fn load_dir(dir: &Path) -> Result<Vec<CompiledRule>> {
    let mut rules = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for path in rule_files(dir)? {
        let text = std::fs::read_to_string(&path).map_err(|e| RuleError::Io(e.to_string()))?;
        let compiled = CompiledRule::from_text(&text, path.clone())?;
        let stem = file_stem(&path);
        let issues = compiled.file.validate(&stem, &seen);
        if let Some(issue) = issues.iter().find(|i| i.is_error()) {
            return Err(RuleError::Schema(format!(
                "{}: {}",
                path.display(),
                issue.message()
            )));
        }
        seen.insert(compiled.file.id.clone());
        rules.push(compiled);
    }
    Ok(rules)
}

/// Loads every rule in `dir`, keeping the ones that are valid (ADR-017).
///
/// # Errors
/// Returns [`RuleError::Io`] only when the directory itself cannot be listed.
pub fn load_dir_lenient(dir: &Path, into: &mut Vec<CompiledRule>) -> Result<LoadReport> {
    let mut report = LoadReport::default();
    // Rules already loaded (e.g. the embedded built-ins) can be *overridden* by
    // a same-id file on disk (M3~M6 §3.5); only duplicates inside the same
    // directory are dead-lettered.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for path in rule_files(dir)? {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                report.dead_letters.push(DeadLetter {
                    path,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let compiled = match CompiledRule::from_text(&text, path.clone()) {
            Ok(compiled) => compiled,
            Err(error) => {
                report.dead_letters.push(DeadLetter {
                    path,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let stem = file_stem(&path);
        let issues: Vec<RuleIssue> = compiled.file.validate(&stem, &seen);
        if let Some(issue) = issues.iter().find(|i| i.is_error()) {
            tracing::warn!(path = %path.display(), "rule dead-lettered: {}", issue.message());
            report.dead_letters.push(DeadLetter {
                path,
                reason: issue.message().to_owned(),
            });
            continue;
        }
        for issue in issues.iter().filter(|i| !i.is_error()) {
            report
                .warnings
                .push(format!("{}: {}", path.display(), issue.message()));
        }
        seen.insert(compiled.file.id.clone());
        report.loaded += 1;
        match into
            .iter_mut()
            .find(|existing| existing.file.id == compiled.file.id)
        {
            Some(existing) => *existing = compiled,
            None => into.push(compiled),
        }
    }
    Ok(report)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Lists rule files in a directory (sorted for determinism).
///
/// # Errors
/// Returns [`RuleError::Io`] when the directory cannot be read.
pub fn rule_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| RuleError::Io(e.to_string()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_rule = path.extension().is_some_and(|e| e == "yaml" || e == "yml");
        if path.is_file() && is_rule {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// SHA-256 of a rule document.
#[must_use]
pub fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = r"
id: NET-TEST-DEMO-001
version: 1
name: demo
severity: low
scope: packet
match:
  protocol: tcp
threshold:
  metric: count
  group_by: [src_ip]
  window: 10s
  operator: gt
  value: 10
description: demo rule
tags: [demo]
";

    fn write_rules(dir: &Path, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).expect("mkdir");
        for (name, text) in files {
            std::fs::write(dir.join(name), text).expect("write rule");
        }
    }

    #[test]
    fn strict_loading_rejects_a_bad_rule() {
        let dir = std::env::temp_dir().join("packetsage-rules-strict");
        let _ = std::fs::remove_dir_all(&dir);
        write_rules(
            &dir,
            &[
                ("NET-TEST-DEMO-001.yaml", RULE),
                ("broken.yaml", "id: nope\n"),
            ],
        );
        assert!(load_dir(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lenient_loading_dead_letters_a_bad_rule() {
        let dir = std::env::temp_dir().join("packetsage-rules-lenient");
        let _ = std::fs::remove_dir_all(&dir);
        write_rules(
            &dir,
            &[
                ("NET-TEST-DEMO-001.yaml", RULE),
                ("broken.yaml", "id: nope\n"),
            ],
        );
        let mut rules = Vec::new();
        let report = load_dir_lenient(&dir, &mut rules).expect("load");
        assert_eq!(report.loaded, 1);
        assert_eq!(report.dead_letters.len(), 1);
        assert_eq!(rules.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn content_hash_is_stable() {
        let a = CompiledRule::from_text(RULE, PathBuf::from("x")).expect("rule");
        let b = CompiledRule::from_text(RULE, PathBuf::from("y")).expect("rule");
        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.short_hash().len(), 8);
    }
}
