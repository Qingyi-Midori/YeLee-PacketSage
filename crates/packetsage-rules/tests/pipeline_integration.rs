//! End-to-end check: the rule engine is really wired into the pipeline.

// Tests assert with `expect()`; production code uses `?` (workspace lints).
#![allow(clippy::expect_used)]

use std::path::Path;

use etherparse::{PacketBuilder, TcpHeader};
use packetsage_core::pipeline::{AnalyzePipeline, CollectingSink, EventSink};
use packetsage_core::{ids, EngineConfig};
use packetsage_protocol::EngineEvent;
use packetsage_rules::{RuleEngine, WindowBudget};

/// Minimal legacy-pcap writer (little endian, microseconds).
fn write_pcap(path: &Path, frames: &[Vec<u8>]) {
    let mut out = Vec::new();
    out.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&65_535u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    for (index, frame) in frames.iter().enumerate() {
        #[allow(clippy::integer_division)] // seconds + whole milliseconds
        let seconds = 1_700_000_000u32 + (index as u32 / 1_000_000);
        let usec = (index as u32 % 1_000_000) * 1_000;
        out.extend_from_slice(&seconds.to_le_bytes());
        out.extend_from_slice(&usec.to_le_bytes());
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(frame);
    }
    std::fs::write(path, out).expect("write pcap");
}

fn syn_frame(port: u16) -> Vec<u8> {
    let mut header = TcpHeader::new(50_000u16.saturating_add(port), port, 1, 8192);
    header.syn = true;
    let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
        .ipv4([10, 8, 8, 8], [10, 0, 2, 30], 64)
        .tcp_header(header);
    let mut packet = Vec::with_capacity(builder.size(0));
    builder.write(&mut packet, &[]).expect("build");
    packet
}

fn pipeline_with_rules(sink: Box<dyn EventSink>) -> AnalyzePipeline {
    let mut pipeline = AnalyzePipeline::new(EngineConfig::default(), sink);
    pipeline.rules = Some(Box::new(RuleEngine::with_builtin_rules(
        WindowBudget::default(),
    )));
    pipeline
}

#[test]
fn port_sweep_sample_fires_the_rule_engine() {
    let path = std::env::temp_dir().join("packetsage-rule-pipeline.pcap");
    let frames: Vec<Vec<u8>> = (1..=60u16).map(syn_frame).collect();
    write_pcap(&path, &frames);

    let task = ids::new_task_id().expect("task");
    let mut pipeline = pipeline_with_rules(Box::new(CollectingSink::default()));
    let result = pipeline
        .run(&path, &task, "2026-01-01T00:00:00Z")
        .expect("run");

    assert_eq!(result.summary.packets, 60);
    assert_eq!(result.store.alerts.len(), 1, "port sweep must alert once");
    let alert = &result.store.alerts[0];
    assert_eq!(alert.rule_id, "NET-TCP-PORT-SWEEP-001");
    assert_eq!(alert.group_key[0].1, "10.8.8.8");
    assert!(alert.evidence.value > 50.0);
}

#[test]
fn syn_burst_thresholds_are_exact_end_to_end() {
    for (count, expected) in [(99u16, 0usize), (100, 0), (101, 1)] {
        let path = std::env::temp_dir().join(format!("packetsage-syn-{count}.pcap"));
        let frames: Vec<Vec<u8>> = (0..count).map(|port| syn_frame(80 + (port % 2))).collect();
        write_pcap(&path, &frames);
        let task = ids::new_task_id().expect("task");
        let mut pipeline = pipeline_with_rules(Box::new(CollectingSink::default()));
        let result = pipeline
            .run(&path, &task, "2026-01-01T00:00:00Z")
            .expect("run");
        assert_eq!(result.store.alerts.len(), expected, "count {count}");
    }
}

#[test]
fn alert_events_reach_the_sink() {
    let path = std::env::temp_dir().join("packetsage-rule-events.pcap");
    let frames: Vec<Vec<u8>> = (0..120u16).map(|port| syn_frame(80 + (port % 2))).collect();
    write_pcap(&path, &frames);
    let task = ids::new_task_id().expect("task");
    let shared = SharedSink::default();
    let mut pipeline = pipeline_with_rules(Box::new(shared.clone()));
    let _ = pipeline
        .run(&path, &task, "2026-01-01T00:00:00Z")
        .expect("run");
    let events = shared.events();
    assert!(events
        .iter()
        .any(|event| matches!(event, EngineEvent::Alert(_))));
}

#[derive(Clone, Default)]
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<EngineEvent>>>);

impl SharedSink {
    fn events(&self) -> Vec<EngineEvent> {
        self.0.lock().map(|guard| guard.clone()).unwrap_or_default()
    }
}

impl EventSink for SharedSink {
    fn emit(&mut self, event: &EngineEvent) -> std::io::Result<()> {
        if let Ok(mut guard) = self.0.lock() {
            guard.push(event.clone());
        }
        Ok(())
    }
}
