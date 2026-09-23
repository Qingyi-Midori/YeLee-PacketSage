//! End-to-end flow assertions: per session **and per direction**.
//!
//! This is the test class that the direction-mixing generator bug slipped
//! through: packet/byte totals stay self-consistent even when frames carry the
//! wrong addresses, so only direction level assertions can catch it (M3~M6 §7).

// Tests assert with `expect()`; production code uses `?` (workspace lints).
#![allow(clippy::expect_used)]

use std::path::Path;

use etherparse::{PacketBuilder, TcpHeader};
use packetsage_core::pipeline::{CollectingSink, EventSink};
use packetsage_core::query::StreamReconstructor;
use packetsage_core::query::{QueryEngine, StreamQuery};
use packetsage_core::reassembly::Direction;
use packetsage_core::{analyze_file, EngineConfig};
use packetsage_protocol::{DirectionBasis, StreamState};

const CLIENT: [u8; 4] = [10, 0, 0, 10];
const SERVER: [u8; 4] = [10, 0, 1, 10];
const CLIENT_MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
const SERVER_MAC: [u8; 6] = [2, 0, 0, 0, 0, 2];
const CLIENT_PORT: u16 = 40_000;
const SERVER_PORT: u16 = 80;
const ISN_CLIENT: u32 = 1_000;
const ISN_SERVER: u32 = 5_000;

/// One Ethernet/IPv4/TCP frame whose MAC, IP and ports all follow
/// `from_client` (getting this wrong is the bug this file exists for).
fn frame(
    from_client: bool,
    seq: u32,
    ack: u32,
    syn: bool,
    ack_flag: bool,
    fin: bool,
    payload: &[u8],
) -> Vec<u8> {
    let (src, dst, sport, dport, src_mac, dst_mac) = if from_client {
        (
            CLIENT,
            SERVER,
            CLIENT_PORT,
            SERVER_PORT,
            CLIENT_MAC,
            SERVER_MAC,
        )
    } else {
        (
            SERVER,
            CLIENT,
            SERVER_PORT,
            CLIENT_PORT,
            SERVER_MAC,
            CLIENT_MAC,
        )
    };
    let mut header = TcpHeader::new(sport, dport, seq, 8192);
    header.acknowledgment_number = ack;
    header.syn = syn;
    header.ack = ack_flag;
    header.fin = fin;
    let builder = PacketBuilder::ethernet2(src_mac, dst_mac)
        .ipv4(src, dst, 64)
        .tcp_header(header);
    let mut packet = Vec::with_capacity(builder.size(payload.len()));
    builder.write(&mut packet, payload).expect("build frame");
    packet
}

fn write_pcap(path: &Path, frames: &[(u32, u32, Vec<u8>)]) {
    let mut out = Vec::new();
    out.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&65_535u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    for (sec, usec, data) in frames {
        out.extend_from_slice(&sec.to_le_bytes());
        out.extend_from_slice(&usec.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    std::fs::write(path, out).expect("write pcap");
}

fn analyze(path: &Path) -> packetsage_core::AnalysisStore {
    let sink: Box<dyn EventSink> = Box::new(CollectingSink::default());
    analyze_file(path, EngineConfig::default(), sink, None)
        .expect("analyse")
        .store
}

#[test]
fn two_direction_complete_flow_reconstructs_both_directions() {
    // The same two payloads `scripts/gen_traffic.py --profile http` builds:
    // 66 request bytes and 43 response bytes, which is what the first report
    // showed in the client direction.
    let request = b"GET /index.html HTTP/1.1\r\nHost: example.com\r\nUser-Agent: gen/1\r\n\r\n";
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    assert_eq!(request.len(), 66);
    assert_eq!(response.len(), 43);

    let capture = std::env::temp_dir().join("packetsage-flow-complete.pcap");
    let client_ack = ISN_CLIENT + 1 + request.len() as u32;
    let server_ack = ISN_SERVER + 1 + response.len() as u32;
    let frames = vec![
        (
            1_700_000_000u32,
            0u32,
            frame(true, ISN_CLIENT, 0, true, false, false, &[]),
        ),
        (
            1_700_000_000,
            1_000,
            frame(false, ISN_SERVER, ISN_CLIENT + 1, true, true, false, &[]),
        ),
        (
            1_700_000_000,
            2_000,
            frame(
                true,
                ISN_CLIENT + 1,
                ISN_SERVER + 1,
                false,
                true,
                false,
                &[],
            ),
        ),
        (
            1_700_000_000,
            3_000,
            frame(
                true,
                ISN_CLIENT + 1,
                ISN_SERVER + 1,
                false,
                true,
                false,
                request,
            ),
        ),
        (
            1_700_000_000,
            4_000,
            frame(
                false,
                ISN_SERVER + 1,
                client_ack,
                false,
                true,
                false,
                response,
            ),
        ),
        (
            1_700_000_000,
            5_000,
            frame(true, client_ack, server_ack, false, true, true, &[]),
        ),
        (
            1_700_000_000,
            6_000,
            frame(false, server_ack, client_ack + 1, false, true, true, &[]),
        ),
    ];
    write_pcap(&capture, &frames);
    let store = analyze(&capture);

    assert_eq!(store.sessions.len(), 1, "one session, not two");
    let entry = store.sessions.sessions()[0];
    assert_eq!(entry.direction_basis, DirectionBasis::SynFirst);
    assert_eq!(
        entry.client_endpoint().port,
        CLIENT_PORT,
        "the client is the SYN sender, not the canonical endpoint"
    );
    assert_eq!(entry.client_endpoint().ip.to_string(), "10.0.0.10");
    assert_eq!(
        entry.stats.src_bytes + entry.stats.dst_bytes,
        entry.stats.bytes
    );

    let session_id = entry.session_id.clone();
    let engine = QueryEngine {
        summary: &store.summary,
        sessions: &store.sessions,
        index: &store.index,
        stats: &store.stats,
        payloads: None,
        streams: Some(&store),
    };
    let client_to_server = engine
        .reconstruct_stream(&StreamQuery {
            session_id: session_id.clone(),
            direction: Some("client_to_server".to_owned()),
            max_bytes: Some(65_536),
        })
        .expect("reconstruct client direction");
    let server_to_client = engine
        .reconstruct_stream(&StreamQuery {
            session_id,
            direction: Some("server_to_client".to_owned()),
            max_bytes: Some(65_536),
        })
        .expect("reconstruct server direction");

    assert_eq!(client_to_server.bytes, 66);
    assert_eq!(server_to_client.bytes, 43);
    assert!(client_to_server.missing_ranges.is_empty());
    assert!(server_to_client.missing_ranges.is_empty());
    assert_eq!(client_to_server.status, "complete");
    assert_eq!(server_to_client.status, "complete");
    assert!(client_to_server.preview.starts_with("GET /index.html"));
    assert!(server_to_client.preview.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(server_to_client.content_type, "http_response");

    // FIN seen from one side: half closed, not incomplete, no false retransmit.
    assert_eq!(entry.stats.state, StreamState::HalfClosed);
    assert_eq!(entry.stats.retransmission_count, 0);
    assert_eq!(entry.stats.out_of_order_count, 0);
    let _ = std::fs::remove_file(&capture);
}

#[test]
fn direction_mixing_is_caught_by_per_direction_assertions() {
    // The very same exchange, but every frame claims to come from the client —
    // the generator bug of the first report. Totals stay identical, the per
    // direction assertions do not.
    let request = vec![b'R'; 66];
    let response = vec![b'S'; 43];
    let capture = std::env::temp_dir().join("packetsage-flow-mixed.pcap");
    let frames = vec![
        (
            1_700_000_000u32,
            0u32,
            frame(true, ISN_CLIENT, 0, true, false, false, &[]),
        ),
        (
            1_700_000_000,
            1_000,
            frame(true, ISN_SERVER, ISN_CLIENT + 1, true, true, false, &[]),
        ),
        (
            1_700_000_000,
            2_000,
            frame(
                true,
                ISN_CLIENT + 1,
                ISN_SERVER + 1,
                false,
                true,
                false,
                &request,
            ),
        ),
        (
            1_700_000_000,
            3_000,
            frame(
                true,
                ISN_SERVER + 1,
                ISN_CLIENT + 67,
                false,
                true,
                false,
                &response,
            ),
        ),
    ];
    write_pcap(&capture, &frames);
    let store = analyze(&capture);
    let entry = store.sessions.sessions()[0];

    // Totals look perfectly healthy…
    assert_eq!(entry.stats.bytes, (66 + 43 + 4 * 54) as u64);
    assert_eq!(store.summary.decode_errors, 0);
    // …and the direction assertions are what fails.
    let client = store
        .reconstruct(entry.session_id.as_str(), Direction::ClientToServer, 65_536)
        .expect("reconstruct");
    assert_eq!(client.bytes, 66, "only the contiguous prefix is delivered");
    assert_eq!(client.status, "incomplete");
    assert_eq!(client.missing_ranges, vec![(1_067, 5_001)]);
    let server = store
        .reconstruct(entry.session_id.as_str(), Direction::ServerToClient, 65_536)
        .expect("reconstruct");
    assert_eq!(server.bytes, 0, "the server direction is empty");
    let _ = std::fs::remove_file(&capture);
}
