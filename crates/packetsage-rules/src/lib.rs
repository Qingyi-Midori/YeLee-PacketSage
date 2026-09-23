//! PacketSage rule engine (L2).
//!
//! Implements the `RuleHook` contract that `packetsage-core` exposes, so the
//! dependency arrow stays `rules -> core` (M0~M2 §2.3).

#![forbid(unsafe_code)]
// Tests assert with `expect()`; production code uses `?` (workspace lints).
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod error;
pub mod evaluator;
pub mod loader;
pub mod schema;

pub use error::{Result, RuleError};
pub use evaluator::{RuleEngine, Sample, WindowBudget, BUILTIN_RULES};
pub use loader::{load_dir, load_dir_lenient, CompiledRule, DeadLetter, LoadReport};
pub use schema::{
    window_ns, Dedupe, GroupField, MatchSpec, Metric, Operator, PortCondition, RuleFile, RuleIssue,
    Scope, Severity, Threshold, FIELD_WHITELIST, FLAG_WHITELIST, WINDOW_WHITELIST,
};

use packetsage_protocol::{AlertEvent, EngineEvent};

impl packetsage_core::pipeline::RuleHook for RuleEngine {
    fn evaluate(&mut self, event: &EngineEvent, now_ns: i128) -> Vec<AlertEvent> {
        self.last_task_id = event.task_id().to_owned();
        RuleEngine::evaluate(self, event, now_ns)
    }

    fn flush(&mut self, now_ns: i128) -> Vec<AlertEvent> {
        let task_id = self.last_task_id.clone();
        self.flush_for_task(&task_id, now_ns)
    }

    fn window_evictions(&self) -> u64 {
        self.window_evictions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packetsage_protocol::{
        AppDetail, AppProto, ApplicationInfo, DnsDetail, EngineEvent, Layer, NetProto, NetworkInfo,
        PacketEvent, SessionSummaryEvent, StreamState, TaskStartedEvent, TcpFlag, TransportInfo,
        TsPrecision, SCHEMA_VERSION,
    };

    const TASK: &str = packetsage_protocol::GOLDEN_TASK_ID;

    fn syn_event(index: u64, ts_ns: i128, src: &str, dst_port: u16) -> EngineEvent {
        EngineEvent::Packet(PacketEvent {
            schema_version: SCHEMA_VERSION,
            task_id: TASK.to_owned(),
            packet_index: index,
            ts_unix_ns: ts_ns.to_string(),
            ts_precision: TsPrecision::Us,
            interface_id: 0,
            captured_len: 74,
            original_len: 74,
            linktype: 1,
            truncated: false,
            link: None,
            network: Some(NetworkInfo {
                src: src.to_owned(),
                dst: "10.0.0.2".to_owned(),
                protocol: NetProto::Tcp,
                ip_version: 4,
                ttl: Some(64),
                hop_limit: None,
                is_fragment: false,
            }),
            transport: Some(TransportInfo {
                src_port: Some(40000),
                dst_port: Some(dst_port),
                flags: vec![TcpFlag::Syn],
                seq: Some(1),
                ack: Some(0),
                window: Some(8192),
                udp_len: None,
                icmp: None,
            }),
            application: None,
            payload_ref: None,
            decode_status: packetsage_protocol::DecodeStatus::Ok,
        })
    }

    fn dns_event(index: u64, ts_ns: i128, entropy: f32, txt_len: u32) -> EngineEvent {
        EngineEvent::Packet(PacketEvent {
            schema_version: SCHEMA_VERSION,
            task_id: TASK.to_owned(),
            packet_index: index,
            ts_unix_ns: ts_ns.to_string(),
            ts_precision: TsPrecision::Us,
            interface_id: 0,
            captured_len: 120,
            original_len: 120,
            linktype: 1,
            truncated: false,
            link: None,
            network: Some(NetworkInfo {
                src: "10.0.0.9".to_owned(),
                dst: "8.8.8.8".to_owned(),
                protocol: NetProto::Udp,
                ip_version: 4,
                ttl: Some(64),
                hop_limit: None,
                is_fragment: false,
            }),
            transport: Some(TransportInfo {
                src_port: Some(40000),
                dst_port: Some(53),
                flags: Vec::new(),
                seq: None,
                ack: None,
                window: None,
                udp_len: Some(100),
                icmp: None,
            }),
            application: Some(ApplicationInfo {
                protocol: AppProto::Dns,
                detail: Some(AppDetail::Dns(DnsDetail {
                    transaction_id: 1,
                    is_response: false,
                    qname: Some("a.example.com".to_owned()),
                    qtype: Some(16),
                    qname_len: Some(60),
                    max_label_len: Some(32),
                    entropy: Some(entropy),
                    txt_len: Some(txt_len),
                    answer_count: 0,
                })),
            }),
            payload_ref: None,
            decode_status: packetsage_protocol::DecodeStatus::Ok,
        })
    }

    fn malformed_event(index: u64, code: packetsage_protocol::DecodeErrorCode) -> EngineEvent {
        EngineEvent::DecodeError(packetsage_protocol::DecodeErrorEvent {
            schema_version: SCHEMA_VERSION,
            task_id: TASK.to_owned(),
            packet_index: index,
            layer: Layer::Ipv4,
            code,
            message: "boom".to_owned(),
        })
    }

    fn session_event(state: StreamState) -> EngineEvent {
        EngineEvent::SessionSummary(SessionSummaryEvent {
            schema_version: SCHEMA_VERSION,
            task_id: TASK.to_owned(),
            session_id: "S-000001".to_owned(),
            protocol: "tcp".to_owned(),
            src_ip: "10.0.0.1".to_owned(),
            src_port: Some(40000),
            dst_ip: "10.0.0.2".to_owned(),
            dst_port: Some(80),
            first_ts_ns: "1000".to_owned(),
            last_ts_ns: "2000".to_owned(),
            packets: 10,
            bytes: 1000,
            src_packets: 5,
            dst_packets: 5,
            src_bytes: 500,
            dst_bytes: 500,
            syn_count: 1,
            syn_ack_count: 1,
            rst_count: 0,
            fin_count: 1,
            retransmission_count: 0,
            out_of_order_count: 0,
            state,
            app_protocol: Some(AppProto::Http),
            direction_basis: packetsage_protocol::DirectionBasis::SynFirst,
            interface_id: Some(0),
            vlan_tag: None,
        })
    }

    #[test]
    fn syn_burst_thresholds_are_exact() {
        for (count, expected) in [(99u64, 0usize), (100, 0), (101, 1)] {
            let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
            let alerts = engine.evaluate(
                &EngineEvent::TaskStarted(TaskStartedEvent {
                    schema_version: SCHEMA_VERSION,
                    task_id: TASK.to_owned(),
                    started_at: "2026-01-01T00:00:00Z".to_owned(),
                    source_path: "x.pcap".to_owned(),
                    engine_version: "0.1.0".to_owned(),
                }),
                0,
            );
            assert!(alerts.is_empty());
            let mut emitted = 0usize;
            for index in 0..count {
                let event = syn_event(
                    index,
                    1_000_000_000 + index as i128 * 1_000_000,
                    "10.0.0.1",
                    80,
                );
                emitted += engine
                    .evaluate(&event, 1_000_000_000 + index as i128 * 1_000_000)
                    .len();
            }
            assert_eq!(emitted, expected, "count {count}");
        }
    }

    #[test]
    fn cooldown_suppresses_repeated_alerts() {
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut alerts = 0;
        for index in 0..400u64 {
            let ts = 1_000_000_000 + index as i128 * 1_000_000;
            alerts += engine
                .evaluate(&syn_event(index, ts, "10.0.0.1", 80), ts)
                .len();
        }
        assert_eq!(alerts, 1, "cooldown must collapse the burst to one alert");
    }

    #[test]
    fn different_sources_are_not_merged() {
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut alerts = 0;
        for index in 0..60u64 {
            let ts = 1_000_000_000 + index as i128 * 1_000_000;
            alerts += engine
                .evaluate(&syn_event(index, ts, "10.0.0.1", 80), ts)
                .len();
            alerts += engine
                .evaluate(&syn_event(index, ts, "10.0.0.9", 80), ts)
                .len();
        }
        assert_eq!(alerts, 0, "60 SYNs per source stay below the threshold");
    }

    #[test]
    fn port_sweep_uses_distinct_ports() {
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut alerts = 0;
        for index in 0..51u64 {
            let ts = 1_000_000_000 + index as i128 * 1_000_000;
            alerts += engine
                .evaluate(&syn_event(index, ts, "10.0.0.1", 1000 + index as u16), ts)
                .len();
        }
        assert_eq!(alerts, 1);
    }

    #[test]
    fn dns_rule_ignores_low_entropy_queries() {
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut alerts = 0;
        for index in 0..1000u64 {
            let ts = 1_000_000_000 + index as i128 * 1_000_000;
            alerts += engine.evaluate(&dns_event(index, ts, 1.2, 0), ts).len();
        }
        assert_eq!(alerts, 0);
        let mut alerts = 0;
        for index in 0..201u64 {
            let ts = 1_000_000_000 + index as i128 * 1_000_000;
            alerts += engine.evaluate(&dns_event(index, ts, 4.2, 0), ts).len();
        }
        assert_eq!(alerts, 1);
    }

    #[test]
    fn malformed_rule_groups_decode_codes() {
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut alerts = 0;
        for index in 0..501u64 {
            let event =
                malformed_event(index, packetsage_protocol::DecodeErrorCode::TruncatedHeader);
            alerts += engine.evaluate(&event, 0).len();
        }
        assert_eq!(alerts, 1);
    }

    #[test]
    fn late_events_produce_the_same_flush_result() {
        // sorted input
        let mut sorted = RuleEngine::with_builtin_rules(WindowBudget::default());
        // two interleaved interfaces arriving out of order
        let mut shuffled = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut sorted_alerts = Vec::new();
        let mut shuffled_alerts = Vec::new();
        let mut events = Vec::new();
        for index in 0..120u64 {
            events.push((index, 1_000_000_000 + index as i128 * 10_000_000));
        }
        for (index, ts) in &events {
            sorted_alerts.extend(sorted.evaluate(&syn_event(*index, *ts, "10.0.0.1", 80), *ts));
        }
        let mut late: Vec<(u64, i128)> = events.clone();
        late.reverse();
        for (index, ts) in late {
            shuffled_alerts.extend(shuffled.evaluate(&syn_event(index, ts, "10.0.0.1", 80), ts));
        }
        sorted_alerts.extend(sorted.flush_for_task(TASK, 2_500_000_000));
        shuffled_alerts.extend(shuffled.flush_for_task(TASK, 2_500_000_000));
        assert_eq!(sorted_alerts.len(), 1);
        assert_eq!(shuffled_alerts.len(), 1);
        assert_eq!(
            sorted_alerts[0].evidence.value,
            shuffled_alerts[0].evidence.value
        );
        assert_eq!(sorted_alerts[0].evidence.value, 101.0);
    }

    #[test]
    fn ratio_rule_with_empty_window_does_not_trigger() {
        let mut engine = RuleEngine::new(WindowBudget::default());
        engine
            .add_rule_text(include_str!(
                "../../../rules/examples/NET-DNS-TUNNEL-RATIO-001.yaml"
            ))
            .expect("ratio rule");
        let alerts = engine.flush_for_task(TASK, 10_000_000_000);
        assert!(alerts.is_empty());
        // The rule counts DNS *responses* (`src_port: 53`): TXT records only
        // exist in responses, which is exactly the gap `any_port` closes.
        let response = |index: u64, ts: i128| {
            let mut event = dns_event(index, ts, 2.0, 200);
            if let EngineEvent::Packet(packet) = &mut event {
                let transport = packet.transport.as_mut().expect("transport");
                transport.src_port = Some(53);
                transport.dst_port = Some(40_000);
                if let Some(packetsage_protocol::AppDetail::Dns(detail)) =
                    packet.application.as_mut().and_then(|a| a.detail.as_mut())
                {
                    detail.is_response = true;
                }
            }
            event
        };
        let mut alerts = Vec::new();
        for index in 0..10u64 {
            let ts = 1_000_000_000 + index as i128 * 1_000_000;
            alerts.extend(engine.evaluate(&response(index, ts), ts));
        }
        assert_eq!(
            alerts.len(),
            1,
            "all samples are TXT-heavy, ratio = 1 > 0.5"
        );
        let evidence = &alerts[0].evidence;
        // The ratio is window-local: the first matching event already yields
        // numerator == denominator == 1, which is `> 0.5`.
        assert_eq!(evidence.value, 1.0);
        assert_eq!(evidence.numerator, Some(1.0));
        assert_eq!(evidence.denominator, Some(1.0));
    }

    #[test]
    fn window_budget_degrades_evidence() {
        let budget = WindowBudget {
            max_groups: 8,
            max_events_per_group: 8,
        };
        let mut engine = RuleEngine::new(budget);
        engine
            .add_rule_text(
                r"
id: NET-TEST-BUDGET-001
version: 1
name: window budget demo
severity: low
scope: packet
match:
  protocol: tcp
  flags:
    contains: [SYN]
    excludes: [ACK]
threshold:
  metric: count
  group_by: [src_ip]
  window: 1s
  operator: gt
  value: 4
dedupe:
  per_group_cooldown: 1s
description: exercises the WindowBudget degradation path
tags: [test]
",
            )
            .expect("rule");
        let mut alerts = Vec::new();
        for index in 0..200u64 {
            let ts = 1_000_000_000 + index as i128 * 10_000_000;
            alerts.extend(engine.evaluate(&syn_event(index, ts, "10.0.0.1", 80), ts));
        }
        assert!(!alerts.is_empty());
        let degraded: Vec<_> = alerts.iter().filter(|a| a.evidence.degraded).collect();
        assert_eq!(
            degraded.len(),
            1,
            "exactly the post-degradation alert is flagged"
        );
        assert!(degraded[0].evidence.sample_packets.is_empty());
    }

    #[test]
    fn session_rules_only_fire_on_session_events() {
        let mut engine = RuleEngine::new(WindowBudget::default());
        engine
            .add_rule_text(include_str!(
                "../../../rules/examples/NET-SESSION-INCOMPLETE-001.yaml"
            ))
            .expect("session rule");
        assert!(engine
            .evaluate(&syn_event(0, 1, "10.0.0.1", 80), 1)
            .is_empty());
        let alerts = engine.evaluate(&session_event(StreamState::Incomplete), 2_000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts[0].evidence.session_state,
            Some(StreamState::Incomplete)
        );
        assert_eq!(alerts[0].session_id.as_deref(), Some("S-000001"));
    }

    #[test]
    fn builtin_rules_are_valid_and_loaded() {
        let engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        assert_eq!(engine.rule_count(), (4, 0));
        let ids: Vec<&str> = engine.rules().iter().map(|r| r.file.id.as_str()).collect();
        assert!(ids.contains(&"NET-TCP-SYN-BURST-001"));
        assert!(ids.contains(&"NET-TCP-PORT-SWEEP-001"));
        assert!(ids.contains(&"NET-DNS-SUSPICIOUS-001"));
        assert!(ids.contains(&"NET-MALFORMED-BURST-001"));
    }

    #[test]
    fn dns_rule_matches_a_decoded_udp_packet_event() {
        // This is a real `packet` event produced by the engine for a synthetic
        // high entropy DNS query (scripts/gen_traffic.py --profile dns-tunnel).
        let json = r#"{"event":"packet","schema_version":2,
            "task_id":"task_TEST0000000000000000000000","packet_index":0,
            "ts_unix_ns":"1700000000000100000","ts_precision":"us","interface_id":0,
            "captured_len":119,"original_len":119,"linktype":1,"truncated":false,
            "link":{"src_mac":"02:00:00:00:00:01","dst_mac":"02:00:00:00:00:02","ethertype":2048},
            "network":{"src":"10.0.0.10","dst":"8.8.8.8","protocol":"udp","ip_version":4,
                       "ttl":64,"is_fragment":false},
            "transport":{"src_port":40000,"dst_port":53,"udp_len":85},
            "application":{"protocol":"dns","detail":{"kind":"dns","transaction_id":0,
                            "is_response":false,"qname":"abc.tunnel.example.com",
                            "qtype":16,"qname_len":59,"max_label_len":40,"entropy":4.5,
                            "answer_count":0}},
            "payload_ref":{"packet_index":0,"offset":42,"length":77},
            "decode_status":"ok"}"#;
        let event: EngineEvent = serde_json::from_str(json).expect("packet event");
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let mut alerts = 0;
        for index in 0..201u64 {
            let mut event = event.clone();
            if let EngineEvent::Packet(packet) = &mut event {
                packet.packet_index = index;
                packet.ts_unix_ns = format!("{}", 1_700_000_000_000_000_000i128 + index as i128);
            }
            alerts += engine.evaluate(&event, 1_700_000_000_000_000_000).len();
        }
        assert_eq!(alerts, 1, "the DNS burst rule must fire once");
    }

    /// `any_port` fires when the well known port is on either side: the same
    /// rule now covers a DNS query (`dst_port: 53`) and a response
    /// (`src_port: 53`), which `src_port`/`dst_port` alone could not express.
    #[test]
    fn any_port_matches_queries_and_responses() {
        let rule_text = r"
id: NET-TEST-ANYPORT-001
version: 1
name: any port demo
severity: low
scope: packet
match:
  protocol: udp
  any_port: 53
threshold:
  metric: count
  group_by: [src_ip]
  window: 10s
  operator: gt
  value: 1
description: demonstrates the any_port primitive
tags: [test]
";
        let mut engine = RuleEngine::new(WindowBudget::default());
        engine.add_rule_text(rule_text).expect("rule");
        assert_eq!(
            engine.evaluate(&dns_event(0, 1_000, 2.0, 0), 1_000).len(),
            0,
            "one query is not > 1"
        );
        // A response: port 53 is now the source port.
        let mut response = dns_event(1, 2_000, 2.0, 0);
        if let EngineEvent::Packet(packet) = &mut response {
            let transport = packet.transport.as_mut().expect("transport");
            transport.src_port = Some(53);
            transport.dst_port = Some(40_000);
            if let Some(packetsage_protocol::AppDetail::Dns(detail)) =
                packet.application.as_mut().and_then(|a| a.detail.as_mut())
            {
                detail.is_response = true;
            }
        }
        assert_eq!(
            engine.evaluate(&response, 2_000).len(),
            1,
            "query + response == 2 > 1"
        );
    }

    #[test]
    fn either_port_is_accepted_as_an_alias() {
        let text = r"
id: NET-TEST-EITHER-001
version: 1
name: either port alias
severity: low
scope: packet
match:
  protocol: tcp
  either_port: { in: [80, 443] }
threshold:
  metric: count
  group_by: [src_ip]
  window: 10s
  operator: gt
  value: 100
description: alias check
tags: [test]
";
        let rule = RuleFile::parse(text).expect("either_port alias must parse");
        assert!(rule.match_filter.any_port.is_some());
        assert!(rule
            .validate(&rule.id, &std::collections::BTreeSet::new())
            .iter()
            .all(|issue| !issue.is_error()));
    }

    #[test]
    fn s10_requires_a_port_protocol() {
        let text = r"
id: NET-TEST-ANYPORT-002
version: 1
name: bad any port
severity: low
scope: packet
match:
  protocol: icmp
  any_port: 53
threshold:
  metric: count
  group_by: [src_ip]
  window: 10s
  operator: gt
  value: 10
description: invalid combination
tags: [test]
";
        let rule = RuleFile::parse(text).expect("rule");
        let issues = rule.validate(&rule.id, &std::collections::BTreeSet::new());
        assert!(issues
            .iter()
            .any(|issue| issue.is_error() && issue.message().contains("S10")));
    }
}
