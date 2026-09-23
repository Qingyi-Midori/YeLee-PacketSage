//! Session aggregation (M0~M2 §4.6, 开发文档 §11).

use std::collections::HashMap;

use packetsage_protocol::{
    AppProto, DirectionBasis, SessionSummaryEvent, StreamState, SCHEMA_VERSION,
};

use crate::model::{ClientSide, Endpoint, PacketRecord, SessionKey};
use crate::reassembly::{Direction, SegmentVerdict};

/// Aggregated counters of one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationStats {
    /// First packet timestamp.
    pub first_ts_ns: i128,
    /// Last packet timestamp.
    pub last_ts_ns: i128,
    /// Packets in both directions.
    pub packets: u64,
    /// Bytes in both directions.
    pub bytes: u64,
    /// Packets from endpoint A (lexically smaller).
    pub src_packets: u64,
    /// Packets from endpoint B.
    pub dst_packets: u64,
    /// Bytes from endpoint A.
    pub src_bytes: u64,
    /// Bytes from endpoint B.
    pub dst_bytes: u64,
    /// Bare SYN count.
    pub syn_count: u32,
    /// SYN+ACK count.
    pub syn_ack_count: u32,
    /// RST count.
    pub rst_count: u32,
    /// FIN count.
    pub fin_count: u32,
    /// Retransmissions reported by the reassembler.
    pub retransmission_count: u32,
    /// Out-of-order segments reported by the reassembler.
    pub out_of_order_count: u32,
    /// Reassembly state.
    pub state: StreamState,
    /// First recognised application protocol.
    pub app_protocol: Option<AppProto>,
}

impl Default for ConversationStats {
    fn default() -> Self {
        Self {
            first_ts_ns: 0,
            last_ts_ns: 0,
            packets: 0,
            bytes: 0,
            src_packets: 0,
            dst_packets: 0,
            src_bytes: 0,
            dst_bytes: 0,
            syn_count: 0,
            syn_ack_count: 0,
            rst_count: 0,
            fin_count: 0,
            retransmission_count: 0,
            out_of_order_count: 0,
            state: StreamState::New,
            app_protocol: None,
        }
    }
}

/// One aggregated session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// `S-{n:06}`, assigned in first-seen order.
    pub session_id: String,
    /// Canonical five-tuple.
    pub key: SessionKey,
    /// Counters.
    pub stats: ConversationStats,
    /// Which endpoint acts as the client.
    pub client: ClientSide,
    /// How the client side was determined.
    pub direction_basis: DirectionBasis,
    /// Interface the session was first seen on.
    pub interface_id: Option<u32>,
    /// Outermost VLAN tag when the first packet was tagged.
    pub vlan_tag: Option<u16>,
}

impl SessionEntry {
    /// Client endpoint.
    #[must_use]
    pub fn client_endpoint(&self) -> Endpoint {
        match self.client {
            ClientSide::A => self.key.a,
            ClientSide::B => self.key.b,
        }
    }

    /// Maps a packet direction onto the reassembler direction enum.
    #[must_use]
    pub fn direction_of(&self, src: Endpoint) -> Direction {
        if src == self.client_endpoint() {
            Direction::ClientToServer
        } else {
            Direction::ServerToClient
        }
    }

    /// True when `endpoint` is endpoint A of the canonical key.
    #[must_use]
    pub fn is_a(&self, endpoint: Endpoint) -> bool {
        self.key.a == endpoint
    }
}

/// Aggregator over all sessions of a task.
#[derive(Debug, Default)]
pub struct ConversationAggregator {
    sessions: HashMap<SessionKey, SessionEntry>,
    order: Vec<SessionKey>,
    next_id: u64,
}

impl ConversationAggregator {
    /// Empty aggregator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// True when nothing was aggregated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Sessions in first-seen order.
    #[must_use]
    pub fn sessions(&self) -> Vec<&SessionEntry> {
        self.order
            .iter()
            .filter_map(|k| self.sessions.get(k))
            .collect()
    }

    /// Looks a session up by id.
    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<&SessionEntry> {
        self.sessions.values().find(|s| s.session_id == session_id)
    }

    /// Looks a session up by key.
    #[must_use]
    pub fn get_by_key(&self, key: &SessionKey) -> Option<&SessionEntry> {
        self.sessions.get(key)
    }

    /// Observes one packet.
    ///
    /// Returns the session key when the packet belongs to a TCP/UDP session.
    pub fn observe(&mut self, rec: &PacketRecord<'_>) -> Option<SessionKey> {
        let key = rec.session_key()?;
        let network = rec.decoded.network.as_ref()?;
        let transport = rec.decoded.transport.as_ref()?;
        let src = Endpoint {
            ip: network.src.parse().ok()?,
            port: transport.src_port.unwrap_or(0),
        };
        let entry = match self.sessions.entry(key) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                self.next_id += 1;
                self.order.push(key);
                slot.insert(SessionEntry {
                    session_id: packetsage_protocol::session_id(self.next_id),
                    key,
                    stats: ConversationStats {
                        first_ts_ns: rec.ts_ns,
                        last_ts_ns: rec.ts_ns,
                        ..ConversationStats::default()
                    },
                    client: if src == key.a {
                        ClientSide::A
                    } else {
                        ClientSide::B
                    },
                    direction_basis: if rec.is_bare_syn() {
                        DirectionBasis::SynFirst
                    } else {
                        DirectionBasis::FirstSeen
                    },
                    interface_id: Some(rec.interface_id),
                    vlan_tag: rec
                        .decoded
                        .link
                        .as_ref()
                        .and_then(|l| l.vlan_ids.first().copied()),
                })
            }
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
        };

        // Direction can be upgraded from "first packet" to "SYN first".
        if rec.is_bare_syn() {
            entry.client = if src == key.a {
                ClientSide::A
            } else {
                ClientSide::B
            };
            entry.direction_basis = DirectionBasis::SynFirst;
        }

        let stats = &mut entry.stats;
        stats.packets = stats.packets.saturating_add(1);
        stats.bytes = stats.bytes.saturating_add(u64::from(rec.captured_len));
        if src == key.a {
            stats.src_packets = stats.src_packets.saturating_add(1);
            stats.src_bytes = stats.src_bytes.saturating_add(u64::from(rec.captured_len));
        } else {
            stats.dst_packets = stats.dst_packets.saturating_add(1);
            stats.dst_bytes = stats.dst_bytes.saturating_add(u64::from(rec.captured_len));
        }
        stats.first_ts_ns = stats.first_ts_ns.min(rec.ts_ns);
        stats.last_ts_ns = stats.last_ts_ns.max(rec.ts_ns);
        if rec.is_bare_syn() {
            stats.syn_count = stats.syn_count.saturating_add(1);
        }
        if rec.is_syn_ack() {
            stats.syn_ack_count = stats.syn_ack_count.saturating_add(1);
        }
        if rec.has_rst() {
            stats.rst_count = stats.rst_count.saturating_add(1);
        }
        if rec.has_fin() {
            stats.fin_count = stats.fin_count.saturating_add(1);
        }
        if stats.app_protocol.is_none() {
            stats.app_protocol = rec
                .decoded
                .application
                .as_ref()
                .map(|app| app.protocol)
                .filter(|p| *p != AppProto::Unknown);
        }
        Some(key)
    }

    /// Records a reassembly verdict against a session.
    pub fn note_verdict(&mut self, key: &SessionKey, verdict: SegmentVerdict) {
        let Some(entry) = self.sessions.get_mut(key) else {
            return;
        };
        match verdict {
            SegmentVerdict::Retransmission => {
                entry.stats.retransmission_count =
                    entry.stats.retransmission_count.saturating_add(1);
            }
            SegmentVerdict::Buffered { .. } => {
                entry.stats.out_of_order_count = entry.stats.out_of_order_count.saturating_add(1);
            }
            SegmentVerdict::PartialOverlap { .. }
            | SegmentVerdict::InOrder
            | SegmentVerdict::Dropped(_) => {}
        }
    }

    /// Records the reassembly state of a session.
    pub fn note_state(&mut self, key: &SessionKey, state: StreamState) {
        if let Some(entry) = self.sessions.get_mut(key) {
            entry.stats.state = state;
        }
    }

    /// Number of sessions whose payload could not be reconstructed completely.
    #[must_use]
    pub fn incomplete_count(&self) -> u64 {
        self.sessions
            .values()
            .filter(|s| {
                matches!(
                    s.stats.state,
                    StreamState::Incomplete | StreamState::BufferOverflow
                )
            })
            .count() as u64
    }

    /// Builds the `SessionSummary` event for one session.
    #[must_use]
    pub fn summary_event(&self, entry: &SessionEntry, task_id: &str) -> SessionSummaryEvent {
        SessionSummaryEvent {
            schema_version: SCHEMA_VERSION,
            task_id: task_id.to_owned(),
            session_id: entry.session_id.clone(),
            protocol: entry.key.proto.as_str().to_owned(),
            src_ip: entry.key.a.ip.to_string(),
            src_port: Some(entry.key.a.port),
            dst_ip: entry.key.b.ip.to_string(),
            dst_port: Some(entry.key.b.port),
            first_ts_ns: entry.stats.first_ts_ns.to_string(),
            last_ts_ns: entry.stats.last_ts_ns.to_string(),
            packets: entry.stats.packets,
            bytes: entry.stats.bytes,
            src_packets: entry.stats.src_packets,
            dst_packets: entry.stats.dst_packets,
            src_bytes: entry.stats.src_bytes,
            dst_bytes: entry.stats.dst_bytes,
            syn_count: entry.stats.syn_count,
            syn_ack_count: entry.stats.syn_ack_count,
            rst_count: entry.stats.rst_count,
            fin_count: entry.stats.fin_count,
            retransmission_count: entry.stats.retransmission_count,
            out_of_order_count: entry.stats.out_of_order_count,
            state: entry.stats.state,
            app_protocol: entry.stats.app_protocol,
            direction_basis: entry.direction_basis,
            interface_id: entry.interface_id,
            vlan_tag: entry.vlan_tag,
        }
    }
}

/// Helper: canonical five-tuple of a session summary event.
#[must_use]
pub fn key_of_summary(event: &SessionSummaryEvent) -> SessionKey {
    use crate::model::TransportProto;
    let proto = if event.protocol == "tcp" {
        TransportProto::Tcp
    } else {
        TransportProto::Udp
    };
    let a = Endpoint {
        ip: event
            .src_ip
            .parse()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
        port: event.src_port.unwrap_or(0),
    };
    let b = Endpoint {
        ip: event
            .dst_ip
            .parse()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
        port: event.dst_port.unwrap_or(0),
    };
    SessionKey { proto, a, b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::Decoder;
    use crate::model::Decoded;
    use crate::reader::LINKTYPE_ETHERNET;
    use etherparse::PacketBuilder;
    use packetsage_protocol::{
        DecodeStatus, NetProto, NetworkInfo, TaskId, TcpFlag, TransportInfo, GOLDEN_TASK_ID,
    };

    fn tcp_packet(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, syn: bool) -> Vec<u8> {
        let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
            .ipv4(src, dst, 64)
            .tcp(sport, dport, 1, 8192);
        let mut packet = Vec::with_capacity(builder.size(0));
        builder.write(&mut packet, &[]).expect("build");
        if syn {
            if let Some(flags) = packet.get_mut(14 + 20 + 13) {
                *flags |= 0x02;
            }
        }
        packet
    }

    fn record<'a>(
        task: &TaskId,
        raw: &'a [u8],
        decoded: Decoded<'a>,
        index: u64,
    ) -> PacketRecord<'a> {
        PacketRecord {
            task_id: task.clone(),
            packet_index: index,
            ts_ns: i128::from(index),
            ts_precision: packetsage_protocol::TsPrecision::Us,
            interface_id: 0,
            captured_len: raw.len() as u32,
            original_len: raw.len() as u32,
            linktype: LINKTYPE_ETHERNET,
            raw,
            decoded,
            decode_status: DecodeStatus::Ok,
        }
    }

    #[test]
    fn both_directions_share_one_session() {
        let task = TaskId::parse(GOLDEN_TASK_ID).expect("task");
        let decoder = Decoder::all();
        let mut agg = ConversationAggregator::new();
        for index in 0..500u64 {
            let (raw_a, raw_b) = if index % 2 == 0 {
                (
                    tcp_packet([10, 0, 0, 1], [10, 0, 0, 2], 40000, 80, true),
                    tcp_packet([10, 0, 0, 2], [10, 0, 0, 1], 80, 40000, false),
                )
            } else {
                (
                    tcp_packet([10, 0, 0, 2], [10, 0, 0, 1], 80, 40000, false),
                    tcp_packet([10, 0, 0, 1], [10, 0, 0, 2], 40000, 80, true),
                )
            };
            for raw in [raw_a, raw_b] {
                let decoded = decoder.decode(LINKTYPE_ETHERNET, &raw);
                let rec = record(&task, &raw, decoded, index);
                let key = agg.observe(&rec).expect("session");
                // keep the borrow checker honest: both directions use one key
                assert_eq!(key.a.ip.to_string(), "10.0.0.1");
            }
        }
        assert_eq!(agg.len(), 1);
        let entry = agg.sessions()[0];
        assert_eq!(entry.stats.packets, 1000);
        assert_eq!(entry.stats.src_packets + entry.stats.dst_packets, 1000);
        assert_eq!(
            entry.stats.src_bytes + entry.stats.dst_bytes,
            entry.stats.bytes
        );
        assert_eq!(entry.direction_basis, DirectionBasis::SynFirst);
        assert_eq!(entry.client_endpoint().ip.to_string(), "10.0.0.1");
        assert_eq!(entry.client_endpoint().port, 40000);
    }

    #[test]
    fn summary_event_matches_stats() {
        let task = TaskId::parse(GOLDEN_TASK_ID).expect("task");
        let decoder = Decoder::all();
        let mut agg = ConversationAggregator::new();
        let raw = tcp_packet([10, 0, 0, 1], [10, 0, 0, 2], 40000, 80, true);
        let decoded = decoder.decode(LINKTYPE_ETHERNET, &raw);
        agg.observe(&record(&task, &raw, decoded, 0));
        let entry = agg.sessions()[0];
        let event = agg.summary_event(entry, task.as_str());
        assert_eq!(event.session_id, "S-000001");
        assert_eq!(event.protocol, "tcp");
        assert_eq!(event.packets, 1);
        assert_eq!(event.syn_count, 1);
        assert_eq!(event.src_ip, "10.0.0.1");
        assert_eq!(key_of_summary(&event), entry.key);
    }

    #[test]
    fn missing_syn_uses_first_packet_sender() {
        let task = TaskId::parse(GOLDEN_TASK_ID).expect("task");
        let decoder = Decoder::all();
        let mut agg = ConversationAggregator::new();
        let raw = tcp_packet([10, 0, 0, 9], [10, 0, 0, 2], 50000, 443, false);
        let decoded = decoder.decode(LINKTYPE_ETHERNET, &raw);
        agg.observe(&record(&task, &raw, decoded, 0));
        let entry = agg.sessions()[0];
        assert_eq!(entry.direction_basis, DirectionBasis::FirstSeen);
        assert_eq!(entry.client_endpoint().ip.to_string(), "10.0.0.9");
    }

    #[test]
    fn app_protocol_is_recorded_once() {
        let task = TaskId::parse(GOLDEN_TASK_ID).expect("task");
        let mut agg = ConversationAggregator::new();
        let raw = vec![0u8; 60];
        let decoded = Decoded {
            link: None,
            network: Some(NetworkInfo {
                src: "10.0.0.1".to_owned(),
                dst: "10.0.0.2".to_owned(),
                protocol: NetProto::Tcp,
                ip_version: 4,
                ttl: Some(64),
                hop_limit: None,
                is_fragment: false,
            }),
            transport: Some(TransportInfo {
                src_port: Some(40000),
                dst_port: Some(80),
                flags: vec![TcpFlag::Ack],
                seq: Some(1),
                ack: Some(1),
                window: Some(1000),
                udp_len: None,
                icmp: None,
            }),
            application: Some(packetsage_protocol::ApplicationInfo {
                protocol: AppProto::Http,
                detail: None,
            }),
            payload: None,
            errors: Vec::new(),
        };
        agg.observe(&record(&task, &raw, decoded, 0));
        assert_eq!(agg.sessions()[0].stats.app_protocol, Some(AppProto::Http));
    }
}
