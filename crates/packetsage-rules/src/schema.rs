//! YAML DSL v1 (M3~M6 §3.2) plus the S1-S9 static validator.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::RuleError;

/// Severity whitelist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// High.
    High,
    /// Medium.
    Medium,
    /// Low.
    Low,
    /// Info.
    Info,
}

impl Severity {
    /// Lowercase wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Info => "info",
        }
    }
}

/// Evaluation scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Evaluated per packet (and per decode error).
    Packet,
    /// Evaluated on `SessionSummary` events only.
    Session,
}

/// Aggregation primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    /// Number of matching events in the window.
    Count,
    /// Number of distinct values of `field` in the window.
    DistinctCount,
    /// Sum of captured bytes in the window.
    SumBytes,
    /// Share of matching events that also satisfy the `field` sub-condition.
    Ratio,
}

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operator {
    /// `>`.
    Gt,
    /// `>=`.
    Gte,
    /// `<`.
    Lt,
    /// `<=`.
    Lte,
    /// `==`.
    Eq,
    /// `!=`.
    Neq,
}

impl Operator {
    /// Applies the operator.
    #[must_use]
    pub fn apply(self, value: f64, threshold: f64) -> bool {
        match self {
            Operator::Gt => value > threshold,
            Operator::Gte => value >= threshold,
            Operator::Lt => value < threshold,
            Operator::Lte => value <= threshold,
            Operator::Eq => (value - threshold).abs() < f64::EPSILON,
            Operator::Neq => (value - threshold).abs() >= f64::EPSILON,
        }
    }

    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Operator::Gt => "gt",
            Operator::Gte => "gte",
            Operator::Lt => "lt",
            Operator::Lte => "lte",
            Operator::Eq => "eq",
            Operator::Neq => "neq",
        }
    }
}

/// Grouping key components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupField {
    /// Source address.
    SrcIp,
    /// Destination address.
    DstIp,
    /// Source port.
    SrcPort,
    /// Destination port.
    DstPort,
    /// Transport protocol.
    Protocol,
    /// Session id (scope: session, or packet with a known session).
    Session,
    /// Decode error code.
    DecodeCode,
}

/// Port condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PortCondition {
    /// Shorthand `src_port: 53`.
    Eq(u16),
    /// Full form.
    Full {
        /// `>=`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gte: Option<u16>,
        /// `<=`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lte: Option<u16>,
        /// `==`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eq: Option<u16>,
        /// Membership.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#in: Option<Vec<u16>>,
    },
}

impl PortCondition {
    /// Evaluates the condition.
    #[must_use]
    pub fn matches(&self, port: Option<u16>) -> bool {
        let Some(port) = port else {
            return false;
        };
        match self {
            PortCondition::Eq(value) => port == *value,
            PortCondition::Full { gte, lte, eq, r#in } => {
                let ok_gte = gte.map_or(true, |v| port >= v);
                let ok_lte = lte.map_or(true, |v| port <= v);
                let ok_eq = eq.map_or(true, |v| port == v);
                let ok_in = r#in.as_ref().map_or(true, |list| list.contains(&port));
                ok_gte && ok_lte && ok_eq && ok_in
            }
        }
    }
}

/// TCP flag condition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagCondition {
    /// Flags that must all be set.
    #[serde(default)]
    pub contains: Vec<String>,
    /// Flags that must all be clear.
    #[serde(default)]
    pub excludes: Vec<String>,
}

/// DNS sub-conditions (match filters, ADR-016).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsCondition {
    /// Query name longer than this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qname_len_gt: Option<u32>,
    /// TXT data longer than this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txt_len_gt: Option<u32>,
    /// Query name entropy above this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entropy_gt: Option<f32>,
}

/// Match filter (AND semantics).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    /// Protocol name (`tcp`, `udp`, `icmp`, `icmpv6`, `arp`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// TCP flag conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<FlagCondition>,
    /// Source port condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_port: Option<PortCondition>,
    /// Destination port condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_port: Option<PortCondition>,
    /// Condition on **either** side's port (source or destination).
    ///
    /// Needed for request/response protocols where the well known port sits on
    /// the server side of one direction only (e.g. a DNS response has
    /// `src_port: 53`, a query has `dst_port: 53`). `either_port` is accepted as
    /// an alias so both spellings in the review are usable.
    #[serde(
        default,
        alias = "either_port",
        skip_serializing_if = "Option::is_none"
    )]
    pub any_port: Option<PortCondition>,
    /// DNS sub-conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<DnsCondition>,
    /// Decode-error layer (`any` matches every layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_layer: Option<String>,
    /// Decode-error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_code: Option<String>,
}

/// Threshold block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Threshold {
    /// Aggregation primitive.
    pub metric: Metric,
    /// Field used by `distinct_count` / `sum_bytes` / `ratio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Sub-condition for the ratio numerator, e.g. `field_gt: 100`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_gt: Option<f64>,
    /// Grouping keys.
    pub group_by: Vec<GroupField>,
    /// Window length (`1s`, `5s`, `10s`, `30s`, `1m`, `5m`).
    pub window: String,
    /// Comparison operator.
    pub operator: Operator,
    /// Threshold value.
    pub value: f64,
}

/// Dedupe block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dedupe {
    /// Per-group cooldown.
    #[serde(default = "default_cooldown")]
    pub per_group_cooldown: String,
}

fn default_cooldown() -> String {
    "1m".to_owned()
}

impl Default for Dedupe {
    fn default() -> Self {
        Self {
            per_group_cooldown: default_cooldown(),
        }
    }
}

/// One rule file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// Rule id (also the file name without extension).
    pub id: String,
    /// Rule version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Human readable name.
    pub name: String,
    /// Severity.
    pub severity: Severity,
    /// Evaluation scope.
    #[serde(default = "default_scope")]
    pub scope: Scope,
    /// Match filter.
    #[serde(default, rename = "match")]
    pub match_filter: MatchSpec,
    /// Threshold block.
    pub threshold: Threshold,
    /// Cooldown block.
    #[serde(default)]
    pub dedupe: Dedupe,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_version() -> u32 {
    1
}

fn default_scope() -> Scope {
    Scope::Packet
}

/// Window / cooldown whitelist (S4).
pub const WINDOW_WHITELIST: [&str; 6] = ["1s", "5s", "10s", "30s", "1m", "5m"];

/// Field whitelist (S3).
pub const FIELD_WHITELIST: [&str; 10] = [
    "dst_port",
    "src_port",
    "dst_ip",
    "src_ip",
    "bytes",
    "dns.qname_len",
    "dns.txt_len",
    "dns.entropy",
    "tls.sni",
    "http.host",
];

/// Flag whitelist (S5).
pub const FLAG_WHITELIST: [&str; 8] = ["FIN", "SYN", "RST", "PSH", "ACK", "URG", "ECE", "CWR"];

/// Static validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleIssue {
    /// Reject the rule (strict mode) or dead-letter it (lenient mode).
    Error(String),
    /// Keep the rule but warn.
    Warn(String),
}

impl RuleIssue {
    /// Human readable message.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            RuleIssue::Error(message) | RuleIssue::Warn(message) => message,
        }
    }

    /// True for blocking issues.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self, RuleIssue::Error(_))
    }
}

/// Parses a window/cooldown string into nanoseconds.
///
/// # Errors
/// Returns [`RuleError`] when the value is outside the whitelist.
pub fn window_ns(text: &str) -> Result<i128, RuleError> {
    let seconds: i128 = match text {
        "1s" => 1,
        "5s" => 5,
        "10s" => 10,
        "30s" => 30,
        "1m" => 60,
        "5m" => 300,
        other => {
            return Err(RuleError::Schema(format!(
                "window/cooldown {other:?} is outside the whitelist {WINDOW_WHITELIST:?}"
            )))
        }
    };
    Ok(seconds * 1_000_000_000)
}

impl RuleFile {
    /// Parses one rule document.
    ///
    /// # Errors
    /// Returns [`RuleError::Schema`] when the YAML is malformed.
    pub fn parse(text: &str) -> Result<Self, RuleError> {
        serde_yaml::from_str(text).map_err(|e| RuleError::Schema(e.to_string()))
    }

    /// Runs the S1-S9 checks.
    #[must_use]
    pub fn validate(&self, file_stem: &str, seen_ids: &BTreeSet<String>) -> Vec<RuleIssue> {
        let mut issues = Vec::new();

        // S1: id pattern, uniqueness and file name agreement.
        if !valid_rule_id(&self.id) {
            issues.push(RuleIssue::Error(format!(
                "S1: rule id {:?} does not match ^[A-Z][A-Z0-9]*(-[A-Z0-9]+)*-[0-9]{{3}}$",
                self.id
            )));
        }
        if seen_ids.contains(&self.id) {
            issues.push(RuleIssue::Error(format!(
                "S1: duplicate rule id {:?}",
                self.id
            )));
        }
        if file_stem != self.id {
            issues.push(RuleIssue::Error(format!(
                "S1: file name {:?} does not match rule id {:?}",
                file_stem, self.id
            )));
        }

        // S2: metric whitelist and required `field`.
        match self.threshold.metric {
            Metric::DistinctCount | Metric::SumBytes => {
                if self.threshold.field.is_none() {
                    issues.push(RuleIssue::Error(format!(
                        "S2: metric {:?} requires `field`",
                        self.threshold.metric
                    )));
                }
            }
            Metric::Count => {}
            Metric::Ratio => {
                if self.threshold.field.is_none() {
                    issues.push(RuleIssue::Error(
                        "S2: metric ratio requires `field` (numerator sub-condition)".to_owned(),
                    ));
                }
                if self.threshold.field_gt.is_none() {
                    issues.push(RuleIssue::Error(
                        "S2: metric ratio requires `field_gt` so the numerator is defined"
                            .to_owned(),
                    ));
                }
            }
        }

        // S3: field whitelist.
        if let Some(field) = &self.threshold.field {
            if !FIELD_WHITELIST.contains(&field.as_str()) {
                issues.push(RuleIssue::Error(format!(
                    "S3: field {field:?} is outside the whitelist {FIELD_WHITELIST:?}"
                )));
            }
        }

        // S4: window and cooldown share one whitelist.
        if window_ns(&self.threshold.window).is_err() {
            issues.push(RuleIssue::Error(format!(
                "S4: window {:?} is outside {WINDOW_WHITELIST:?}",
                self.threshold.window
            )));
        }
        if window_ns(&self.dedupe.per_group_cooldown).is_err() {
            issues.push(RuleIssue::Error(format!(
                "S4: per_group_cooldown {:?} is outside {WINDOW_WHITELIST:?}",
                self.dedupe.per_group_cooldown
            )));
        }

        // S5: flags are only meaningful for TCP packet rules.
        if let Some(flags) = &self.match_filter.flags {
            let protocol = self.match_filter.protocol.as_deref().unwrap_or("");
            if self.scope != Scope::Packet || protocol != "tcp" {
                issues.push(RuleIssue::Error(
                    "S5: `flags` requires scope: packet and protocol: tcp".to_owned(),
                ));
            }
            for flag in flags.contains.iter().chain(flags.excludes.iter()) {
                if !FLAG_WHITELIST.contains(&flag.as_str()) {
                    issues.push(RuleIssue::Error(format!(
                        "S5: flag {flag:?} is outside {FLAG_WHITELIST:?}"
                    )));
                }
            }
        }

        // S6: DNS conditions need UDP/53.
        if self.match_filter.dns.is_some() {
            let protocol_ok = self.match_filter.protocol.as_deref() == Some("udp");
            let port_ok = self
                .match_filter
                .dst_port
                .as_ref()
                .is_some_and(|c| c.matches(Some(53)));
            if !protocol_ok || !port_ok {
                issues.push(RuleIssue::Error(
                    "S6: `dns.*` requires protocol: udp and dst_port: 53".to_owned(),
                ));
            }
        }

        // S7: ratio value must be a probability and the operator must be gt/gte.
        if self.threshold.metric == Metric::Ratio {
            if !(self.threshold.value > 0.0 && self.threshold.value <= 1.0) {
                issues.push(RuleIssue::Error(format!(
                    "S7: ratio value {} must be inside (0, 1]",
                    self.threshold.value
                )));
            }
            if !matches!(self.threshold.operator, Operator::Gt | Operator::Gte) {
                issues.push(RuleIssue::Error(
                    "S7: ratio operator must be gt or gte".to_owned(),
                ));
            }
        }

        // S8: description and tags (warning only).
        if self
            .description
            .as_ref()
            .map_or(true, |d| d.trim().is_empty())
        {
            issues.push(RuleIssue::Warn("S8: description is empty".to_owned()));
        }
        if self.tags.len() > 8 {
            issues.push(RuleIssue::Warn(format!(
                "S8: {} tags exceed the limit of 8",
                self.tags.len()
            )));
        }

        // S9: unknown top level fields are rejected by `deny_unknown_fields`.
        if self.threshold.group_by.is_empty() {
            issues.push(RuleIssue::Error(
                "S9: `group_by` must not be empty".to_owned(),
            ));
        }

        // S10: `any_port` only makes sense for protocols that have ports, and
        // combining it with `src_port`/`dst_port` is almost always a mistake.
        if let Some(any_port) = &self.match_filter.any_port {
            let protocol = self.match_filter.protocol.as_deref().unwrap_or("");
            if !matches!(protocol, "tcp" | "udp") {
                issues.push(RuleIssue::Error(
                    "S10: `any_port` requires protocol: tcp or udp".to_owned(),
                ));
            }
            if self.match_filter.src_port.is_some() || self.match_filter.dst_port.is_some() {
                issues.push(RuleIssue::Warn(
                    "S10: `any_port` combined with `src_port`/`dst_port` is redundant".to_owned(),
                ));
            }
            let _ = any_port;
        }

        issues
    }
}

/// `^[A-Z][A-Z0-9]*(-[A-Z0-9]+)*-[0-9]{3}$` without a regex dependency.
#[must_use]
pub fn valid_rule_id(id: &str) -> bool {
    let Some((head, tail)) = id.rsplit_once('-') else {
        return false;
    };
    if tail.len() != 3 || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if head.is_empty() || !head.starts_with(|c: char| c.is_ascii_uppercase()) {
        return false;
    }
    head.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYN_RULE: &str = r"
id: NET-TCP-SYN-BURST-001
version: 1
name: TCP SYN 突发检测
severity: high
scope: packet
match:
  protocol: tcp
  flags:
    contains: [SYN]
    excludes: [ACK]
threshold:
  metric: count
  group_by: [src_ip]
  window: 10s
  operator: gt
  value: 100
dedupe:
  per_group_cooldown: 1m
description: >
  单一源 IP 在时间窗口内产生大量未完成 TCP SYN。
tags: [tcp, dos, scan]
";

    #[test]
    fn parses_the_reference_rule() {
        let rule = RuleFile::parse(SYN_RULE).expect("rule");
        assert_eq!(rule.id, "NET-TCP-SYN-BURST-001");
        assert_eq!(rule.severity, Severity::High);
        assert_eq!(rule.threshold.metric, Metric::Count);
        assert_eq!(rule.threshold.group_by, vec![GroupField::SrcIp]);
        assert!(rule
            .validate("NET-TCP-SYN-BURST-001", &BTreeSet::new())
            .iter()
            .all(|i| !i.is_error()));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let text = format!("{SYN_RULE}\nunknown_field: 1\n");
        assert!(RuleFile::parse(&text).is_err());
    }

    #[test]
    fn s1_checks_id_and_file_name() {
        let rule = RuleFile::parse(SYN_RULE).expect("rule");
        let issues = rule.validate("other-name", &BTreeSet::new());
        assert!(issues
            .iter()
            .any(|i| i.is_error() && i.message().contains("S1")));
        let mut seen = BTreeSet::new();
        seen.insert(rule.id.clone());
        let issues = rule.validate(&rule.id, &seen);
        assert!(issues.iter().any(|i| i.message().contains("duplicate")));
    }

    #[test]
    fn s3_rejects_unknown_field() {
        let text = SYN_RULE.replace(
            "metric: count",
            "metric: distinct_count\n  field: bogus_field",
        );
        let rule = RuleFile::parse(&text).expect("rule");
        let issues = rule.validate(&rule.id, &BTreeSet::new());
        assert!(issues.iter().any(|i| i.message().contains("S3")));
    }

    #[test]
    fn s4_rejects_window_outside_whitelist() {
        let text = SYN_RULE.replace("window: 10s", "window: 7s");
        let rule = RuleFile::parse(&text).expect("rule");
        let issues = rule.validate(&rule.id, &BTreeSet::new());
        assert!(issues.iter().any(|i| i.message().contains("S4")));
    }

    #[test]
    fn s7_ratio_rules_need_probability_values() {
        let text = r"
id: NET-DNS-TUNNEL-RATIO-001
version: 1
name: DNS tunnel ratio
severity: medium
scope: packet
match:
  protocol: udp
  dst_port: 53
  dns:
    txt_len_gt: 100
threshold:
  metric: ratio
  field: dns.txt_len
  field_gt: 100
  group_by: [src_ip]
  window: 1m
  operator: gt
  value: 2.0
description: test
tags: [dns]
";
        let rule = RuleFile::parse(text).expect("rule");
        let issues = rule.validate(&rule.id, &BTreeSet::new());
        assert!(issues.iter().any(|i| i.message().contains("S7")));
    }

    #[test]
    fn window_whitelist_is_enforced() {
        assert_eq!(window_ns("10s").ok(), Some(10_000_000_000));
        assert_eq!(window_ns("5m").ok(), Some(300_000_000_000));
        assert!(window_ns("2m").is_err());
    }

    #[test]
    fn rule_id_pattern() {
        assert!(valid_rule_id("NET-TCP-SYN-BURST-001"));
        assert!(valid_rule_id("A-1-001"));
        assert!(!valid_rule_id("net-tcp-syn-burst-001"));
        assert!(!valid_rule_id("NET-TCP-SYN-BURST-1"));
        assert!(!valid_rule_id("NETTCP"));
    }

    #[test]
    fn port_conditions() {
        assert!(PortCondition::Eq(53).matches(Some(53)));
        assert!(!PortCondition::Eq(53).matches(Some(54)));
        let full = PortCondition::Full {
            gte: Some(1024),
            lte: None,
            eq: None,
            r#in: None,
        };
        assert!(full.matches(Some(40000)));
        assert!(!full.matches(Some(80)));
        let list = PortCondition::Full {
            gte: None,
            lte: None,
            eq: None,
            r#in: Some(vec![80, 443]),
        };
        assert!(list.matches(Some(443)));
        assert!(!list.matches(Some(8080)));
    }
}
