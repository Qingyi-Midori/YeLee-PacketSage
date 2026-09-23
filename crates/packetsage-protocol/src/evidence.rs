//! Finding / evidence model plus the Rust half of the two-stage evidence
//! validator (ADR-023): `validate_finding` implements V1-V4, Python owns V5.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Finding severity whitelist (C7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// High.
    High,
    /// Medium.
    Medium,
    /// Low.
    Low,
    /// Informational.
    Info,
}

impl Severity {
    /// Parses the severity, rejecting anything outside the whitelist.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "high" => Some(Severity::High),
            "medium" => Some(Severity::Medium),
            "low" => Some(Severity::Low),
            "info" => Some(Severity::Info),
            _ => None,
        }
    }

    /// Lowercase wire representation.
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

/// Epistemic basis of a finding (开发文档 §17.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Directly read from a tool result.
    DirectObservation,
    /// A rule fired.
    RuleMatch,
    /// Several observations were correlated by the agent.
    CorrelatedObservation,
    /// The agent's inference; must be presented as unconfirmed.
    Hypothesis,
}

/// Validator verdict stored in `findings.validator_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorStatus {
    /// Passed V1-V5.
    Accepted,
    /// Kept but downgraded (e.g. to hypothesis) because a check failed.
    Downgraded,
    /// Dropped.
    Rejected,
}

/// Reference to a tool result that supports a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// Tool-call anchor, `tc-{n:06}`, allocated by the engine.
    #[serde(rename = "_id")]
    pub id: String,
    /// RPC method that produced the referenced result.
    pub method: String,
    /// Optional entity reference inside that result (session id, rule id, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    /// Timestamp of the tool call (canonical ns as a string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts_unix_ns: Option<String>,
    /// One-line human summary used by the report index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// A numeric claim made by the model in a structured field.
///
/// V4 compares it against the engine-held ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumericClaim {
    /// Ledger tool call the number is attributed to.
    #[serde(rename = "_id")]
    pub id: String,
    /// Dotted path inside the referenced tool result, e.g. `packets`.
    pub field: String,
    /// Value stated by the model.
    pub value: f64,
}

/// Draft submitted by the Python agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindingDraft {
    /// Finding title.
    pub title: String,
    /// Severity, restricted to the whitelist.
    pub severity: Severity,
    /// Epistemic basis.
    pub basis: Basis,
    /// Natural-language summary.
    pub summary: String,
    /// Evidence anchors.
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    /// Structured numbers stated by the model (V4 input).
    #[serde(default)]
    pub claims: Vec<NumericClaim>,
}

/// Stored finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// `F-{n:03}` allocated by the engine.
    pub finding_id: String,
    /// Task identifier.
    pub task_id: String,
    /// Title.
    pub title: String,
    /// Severity.
    pub severity: Severity,
    /// Epistemic basis.
    pub basis: Basis,
    /// Natural-language summary.
    pub summary: String,
    /// Evidence anchors.
    pub evidence: Vec<EvidenceRef>,
    /// Validator verdict.
    pub validator_status: ValidatorStatus,
}

/// One validator complaint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// Check that failed: `V1` ... `V4`.
    pub check: String,
    /// What went wrong.
    pub message: String,
    /// Whether the issue must reject the draft (true) or only downgrade it.
    pub fatal: bool,
}

/// Ledger entry for one issued tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Task the call belongs to.
    pub task_id: String,
    /// Method name.
    pub method: String,
    /// Call timestamp, canonical ns as string.
    pub ts_unix_ns: String,
    /// Compact, already redacted argument summary (JSON text).
    #[serde(default)]
    pub args_json: String,
    /// Wall clock duration of the call in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// `ok` or `error`.
    #[serde(default)]
    pub status: String,
    /// Entities reachable through this result (session ids, rule ids, packet
    /// index strings).
    #[serde(default)]
    pub ref_ids: BTreeSet<String>,
    /// Numeric fields reachable through this result, keyed by dotted path.
    #[serde(default)]
    pub numbers: BTreeMap<String, f64>,
    /// Token-shaped strings the result surfaced (IPs, `ip:port`, session / rule /
    /// alert ids, packet indices, bare numbers). The report's anti-hallucination
    /// lint needs them because a later process cannot see the tool result body
    /// any more, only what the ledger kept. Deliberately *not* advertised to the
    /// model (that stays `_numbers` / `_ref_ids`).
    #[serde(default)]
    pub tokens: BTreeSet<String>,
}

/// Engine-held ledger of everything the agent could possibly know.
///
/// It is the single source of truth for V2-V4.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceLedger {
    entries: BTreeMap<String, LedgerEntry>,
}

impl EvidenceLedger {
    /// Empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an issued tool call.
    pub fn issue(&mut self, id: &str, entry: LedgerEntry) {
        self.entries.insert(id.to_owned(), entry);
    }

    /// Looks up an issued tool call.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&LedgerEntry> {
        self.entries.get(id)
    }

    /// True when the id was issued at least once.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// Number of issued calls.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing has been issued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All issued ids, ascending.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// Every issued call, ordered by id. ULID ids sort by mint time (ADR-024),
    /// so this is also emission order.
    #[must_use]
    pub fn entries(&self) -> Vec<(String, LedgerEntry)> {
        self.entries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect()
    }

    /// Resolves a dotted numeric path through the ledger.
    #[must_use]
    pub fn number(&self, id: &str, field: &str) -> Option<f64> {
        self.entries.get(id)?.numbers.get(field).copied()
    }
}

/// Outcome of the Rust-side V1-V4 validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationOutcome {
    /// Suggested verdict; the caller applies the "stricter wins" merge rule.
    pub status: ValidatorStatus,
    /// Adjusted basis (downgraded to `hypothesis` when V4 failed).
    pub basis: Basis,
    /// All complaints, in check order.
    pub issues: Vec<ValidationIssue>,
}

/// Runs V1-V4 against the ledger.
///
/// - **V1** every finding cites at least one `EvidenceRef`
/// - **V2** every cited `_id` exists in the ledger and belongs to the task
/// - **V3** every cited `ref_id` is reachable through the cited tool result
/// - **V4** every structured numeric claim matches the ledger value
///
/// V2/V3 failures reject the draft; V4 failures downgrade it to a hypothesis.
/// A draft with no issues is accepted.
#[must_use]
pub fn validate_finding_v1_v4(
    draft: &FindingDraft,
    ledger: &EvidenceLedger,
    task_id: &str,
) -> ValidationOutcome {
    let mut issues = Vec::new();

    if draft.evidence.is_empty() {
        issues.push(ValidationIssue {
            check: "V1".to_owned(),
            message: "finding cites no evidence".to_owned(),
            fatal: true,
        });
    }

    for ev in &draft.evidence {
        match ledger.get(&ev.id) {
            None => issues.push(ValidationIssue {
                check: "V2".to_owned(),
                message: format!("evidence id {} was never issued by this engine", ev.id),
                fatal: true,
            }),
            Some(entry) if entry.task_id != task_id => issues.push(ValidationIssue {
                check: "V2".to_owned(),
                message: format!(
                    "evidence id {} belongs to task {}, not {}",
                    ev.id, entry.task_id, task_id
                ),
                fatal: true,
            }),
            Some(entry) => {
                if let Some(ref_id) = &ev.ref_id {
                    if !entry.ref_ids.contains(ref_id) {
                        issues.push(ValidationIssue {
                            check: "V3".to_owned(),
                            message: format!(
                                "reference {} is not reachable through {} ({})",
                                ref_id, ev.id, entry.method
                            ),
                            fatal: true,
                        });
                    }
                }
            }
        }
    }

    for claim in &draft.claims {
        match ledger.number(&claim.id, &claim.field) {
            None => issues.push(ValidationIssue {
                check: "V4".to_owned(),
                message: format!("claim {}.{} has no ledger value", claim.id, claim.field),
                fatal: false,
            }),
            Some(expected) if (expected - claim.value).abs() > 1e-6 => {
                issues.push(ValidationIssue {
                    check: "V4".to_owned(),
                    message: format!(
                        "claim {}.{} = {} but the engine ledger says {}",
                        claim.id, claim.field, claim.value, expected
                    ),
                    fatal: false,
                })
            }
            Some(_) => {}
        }
    }

    let fatal = issues.iter().any(|i| i.fatal);
    let downgrade = issues.iter().any(|i| !i.fatal);
    let status = if fatal {
        ValidatorStatus::Rejected
    } else if downgrade {
        ValidatorStatus::Downgraded
    } else {
        ValidatorStatus::Accepted
    };
    let basis = if downgrade && !fatal {
        Basis::Hypothesis
    } else {
        draft.basis
    };

    ValidationOutcome {
        status,
        basis,
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK: &str = "task_TEST0000000000000000000000";
    const TC_ID: &str = "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH";

    fn ledger() -> EvidenceLedger {
        let mut ledger = EvidenceLedger::new();
        ledger.issue(
            TC_ID,
            LedgerEntry {
                task_id: TASK.to_owned(),
                method: "get_capture_summary".to_owned(),
                ts_unix_ns: "1717689600000000000".to_owned(),
                args_json: "{}".to_owned(),
                duration_ms: 1,
                status: "ok".to_owned(),
                ref_ids: ["S-000001"].iter().map(|s| (*s).to_owned()).collect(),
                numbers: [("packets".to_owned(), 182_431f64)].into_iter().collect(),
                tokens: ["182431", "S-000001"]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
            },
        );
        ledger
    }

    fn draft(evidence: Vec<EvidenceRef>, claims: Vec<NumericClaim>) -> FindingDraft {
        FindingDraft {
            title: "t".to_owned(),
            severity: Severity::High,
            basis: Basis::DirectObservation,
            summary: "s".to_owned(),
            evidence,
            claims,
        }
    }

    #[test]
    fn v1_requires_evidence() {
        let outcome = validate_finding_v1_v4(&draft(vec![], vec![]), &ledger(), TASK);
        assert_eq!(outcome.status, ValidatorStatus::Rejected);
        assert_eq!(outcome.issues[0].check, "V1");
    }

    #[test]
    fn v2_rejects_unknown_tool_call() {
        let ev = EvidenceRef {
            id: "tc_01J9Z4M8YQ2V7C1W3N5B6D8FZZ".to_owned(),
            method: "get_capture_summary".to_owned(),
            ref_id: None,
            ts_unix_ns: None,
            summary: None,
        };
        let outcome = validate_finding_v1_v4(&draft(vec![ev], vec![]), &ledger(), TASK);
        assert_eq!(outcome.status, ValidatorStatus::Rejected);
        assert_eq!(outcome.issues[0].check, "V2");
    }

    #[test]
    fn v3_rejects_forged_session_id() {
        let ev = EvidenceRef {
            id: "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH".to_owned(),
            method: "get_capture_summary".to_owned(),
            ref_id: Some("S-99999".to_owned()),
            ts_unix_ns: None,
            summary: None,
        };
        let outcome = validate_finding_v1_v4(&draft(vec![ev], vec![]), &ledger(), TASK);
        assert_eq!(outcome.status, ValidatorStatus::Rejected);
        assert!(outcome.issues.iter().any(|i| i.check == "V3"));
    }

    #[test]
    fn v4_mismatch_downgrades_to_hypothesis() {
        let ev = EvidenceRef {
            id: "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH".to_owned(),
            method: "get_capture_summary".to_owned(),
            ref_id: Some("S-000001".to_owned()),
            ts_unix_ns: None,
            summary: None,
        };
        let claim = NumericClaim {
            id: "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH".to_owned(),
            field: "packets".to_owned(),
            value: 999_999.0,
        };
        let outcome = validate_finding_v1_v4(&draft(vec![ev], vec![claim]), &ledger(), TASK);
        assert_eq!(outcome.status, ValidatorStatus::Downgraded);
        assert_eq!(outcome.basis, Basis::Hypothesis);
    }

    #[test]
    fn clean_draft_is_accepted() {
        let ev = EvidenceRef {
            id: "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH".to_owned(),
            method: "get_capture_summary".to_owned(),
            ref_id: Some("S-000001".to_owned()),
            ts_unix_ns: None,
            summary: None,
        };
        let claim = NumericClaim {
            id: "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH".to_owned(),
            field: "packets".to_owned(),
            value: 182_431.0,
        };
        let outcome = validate_finding_v1_v4(&draft(vec![ev], vec![claim]), &ledger(), TASK);
        assert_eq!(outcome.status, ValidatorStatus::Accepted);
        assert!(outcome.issues.is_empty());
    }
}
