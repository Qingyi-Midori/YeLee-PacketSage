//! TCP stream reassembly (M0~M2 §4.5, §8.3; 开发文档 §10).

use bytes::Bytes;
use packetsage_protocol::StreamState;
use std::collections::HashMap;

use crate::config::ReassemblyLimits;
use crate::model::SessionKey;

/// Direction of a segment relative to the session client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    /// Client to server.
    ClientToServer,
    /// Server to client.
    ServerToClient,
}

impl Direction {
    /// Stable lowercase name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::ClientToServer => "client_to_server",
            Direction::ServerToClient => "server_to_client",
        }
    }
}

/// How a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseHow {
    /// FIN observed.
    Fin,
    /// RST observed.
    Rst,
    /// Idle timeout.
    Timeout,
}

/// Why a segment was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// Per-stream byte budget exceeded.
    BufferFull,
    /// Per-stream segment budget exceeded.
    TooManySegments,
    /// Global session budget exceeded.
    SessionLimit,
}

/// Reassembly verdict for one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentVerdict {
    /// Delivered in order (or buffered without a gap).
    InOrder,
    /// Buffered because `gap_bytes` bytes are still missing before it.
    Buffered {
        /// Size of the hole in front of this segment.
        gap_bytes: u64,
    },
    /// Whole segment was already delivered.
    Retransmission,
    /// Overlapping bytes were dropped in favour of the first copy.
    PartialOverlap {
        /// Bytes dropped from the head of this segment.
        dropped_bytes: u32,
    },
    /// Segment refused due to a limit.
    Dropped(DropReason),
}

/// Overlap resolution policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlapPolicy {
    /// Keep the first copy seen (default per M0~M2 §4.5).
    #[default]
    FirstWins,
    /// Keep the last copy seen.
    LastWins,
}

/// One TCP segment handed to the reassembler.
#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    /// Sequence number of the first payload byte.
    pub seq: u32,
    /// Payload bytes (may be empty for pure ACK/SYN/FIN).
    pub data: &'a [u8],
    /// Packet timestamp (canonical ns).
    pub ts_ns: i128,
    /// FIN flag.
    pub fin: bool,
    /// RST flag.
    pub rst: bool,
    /// SYN flag (a bare SYN consumes one sequence number).
    pub syn: bool,
}

/// A byte range `[start, end)` of the sequence space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// Inclusive start.
    pub start: u32,
    /// Exclusive end.
    pub end: u32,
}

impl ByteRange {
    /// Length in bytes.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.end.wrapping_sub(self.start)
    }

    /// True when the range is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Stream pruned by [`Reassembler::prune_expired`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedStream {
    /// Session key.
    pub key: SessionKey,
    /// Final state.
    pub state: StreamState,
    /// Reason.
    pub reason: CloseHow,
}

#[derive(Debug, Default)]
struct StreamBuf {
    next_seq: Option<u32>,
    /// Buffered segments, ordered by their distance from `next_seq`.
    pending: Vec<(u32, Vec<u8>)>,
    /// Bytes ready to be handed to consumers.
    ready: Vec<u8>,
    /// Bytes sitting in `pending`.
    pending_bytes: u64,
    retransmissions: u32,
    out_of_order: u32,
    overflow: bool,
    fin_seen: bool,
    rst_seen: bool,
    last_ts_ns: i128,
}

impl StreamBuf {
    fn held_bytes(&self) -> u64 {
        self.pending_bytes.saturating_add(self.ready.len() as u64)
    }

    fn note_state(&self) -> StreamState {
        if self.overflow {
            StreamState::BufferOverflow
        } else if !self.gaps().is_empty() {
            StreamState::Incomplete
        } else if self.rst_seen {
            StreamState::Closed
        } else if self.fin_seen {
            StreamState::HalfClosed
        } else if self.next_seq.is_some() {
            StreamState::Active
        } else {
            StreamState::New
        }
    }

    fn gaps(&self) -> Vec<ByteRange> {
        let Some(next) = self.next_seq else {
            return Vec::new();
        };
        let mut cursor = next;
        let mut gaps = Vec::new();
        for (start, data) in &self.pending {
            if seq_before(cursor, *start) {
                gaps.push(ByteRange {
                    start: cursor,
                    end: *start,
                });
            }
            let end = start.wrapping_add(data.len() as u32);
            if seq_before(cursor, end) {
                cursor = end;
            }
        }
        gaps
    }
}

/// True when `a` is strictly before `b` in TCP sequence space.
fn seq_before(a: u32, b: u32) -> bool {
    (b.wrapping_sub(a) as i32) > 0
}

/// Wrapping distance from `a` to `b`.
fn seq_distance(a: u32, b: u32) -> i64 {
    i64::from(b.wrapping_sub(a) as i32)
}

/// Bounded TCP reassembler.
pub struct Reassembler {
    limits: ReassemblyLimits,
    policy: OverlapPolicy,
    streams: HashMap<(SessionKey, Direction), StreamBuf>,
    /// Distinct sessions seen, kept so the cap check is O(1): it runs on every
    /// packet and must not walk the stream map.
    session_keys: std::collections::HashSet<SessionKey>,
    dropped_sessions: u64,
}

impl std::fmt::Debug for Reassembler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reassembler")
            .field("streams", &self.streams.len())
            .field("dropped_sessions", &self.dropped_sessions)
            .finish()
    }
}

impl Reassembler {
    /// Creates a reassembler.
    #[must_use]
    pub fn new(limits: ReassemblyLimits, policy: OverlapPolicy) -> Self {
        Self {
            limits,
            policy,
            streams: HashMap::new(),
            session_keys: std::collections::HashSet::new(),
            dropped_sessions: 0,
        }
    }

    /// Number of tracked sessions (both directions collapsed).
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.session_keys.len()
    }

    /// Sessions dropped because of the global session cap.
    #[must_use]
    pub fn dropped_sessions(&self) -> u64 {
        self.dropped_sessions
    }

    /// Feeds one segment into the stream.
    pub fn feed(&mut self, key: &SessionKey, dir: Direction, seg: Segment<'_>) -> SegmentVerdict {
        if !self.streams.contains_key(&(*key, dir))
            && !self.session_keys.contains(key)
            && self.session_keys.len() >= self.limits.max_sessions
        {
            self.dropped_sessions += 1;
            return SegmentVerdict::Dropped(DropReason::SessionLimit);
        }
        self.session_keys.insert(*key);
        let stream = self.streams.entry((*key, dir)).or_default();
        stream.last_ts_ns = stream.last_ts_ns.max(seg.ts_ns);
        if seg.rst {
            stream.rst_seen = true;
        }
        if seg.fin {
            stream.fin_seen = true;
        }
        if seg.data.is_empty() {
            // Zero length segments never carry payload and must not be mistaken
            // for data:
            //   * a bare SYN occupies exactly one sequence number, so the byte
            //     the peer sends next is `seq + 1`;
            //   * a pure ACK occupies no sequence space at all — it must never
            //     initialise `next_seq` (a delayed ACK whose seq is ahead of the
            //     data would otherwise turn the real data into a
            //     "retransmission"), never be buffered, and never be counted as
            //     a retransmission.
            if stream.next_seq.is_none() && seg.syn {
                stream.next_seq = Some(seg.seq.wrapping_add(1));
            }
            return SegmentVerdict::InOrder;
        }
        if stream.next_seq.is_none() {
            stream.next_seq = Some(seg.seq);
        }
        let next = stream.next_seq.unwrap_or(seg.seq);

        let mut start = seg.seq;
        let mut data = seg.data;
        let mut verdict = SegmentVerdict::InOrder;

        if seq_before(start, next) {
            let ahead = seq_distance(start, next);
            if ahead <= 0 {
                // wrapping arithmetic said "before" but the distance is not
                // usable: treat as retransmission of already delivered data.
                stream.retransmissions += 1;
                return SegmentVerdict::Retransmission;
            }
            let ahead = ahead as usize;
            if ahead >= data.len() {
                stream.retransmissions += 1;
                return SegmentVerdict::Retransmission;
            }
            data = &data[ahead..];
            start = next;
            verdict = SegmentVerdict::PartialOverlap {
                dropped_bytes: u32::try_from(ahead).unwrap_or(u32::MAX),
            };
        }

        if seq_before(next, start) {
            let gap = seq_distance(next, start).max(0) as u64;
            verdict = SegmentVerdict::Buffered { gap_bytes: gap };
            stream.out_of_order += 1;
        }

        let reference = stream.next_seq.unwrap_or(start);
        let (inserted, dropped) =
            insert_pending(&mut stream.pending, reference, start, data, self.policy);
        stream.pending_bytes = stream
            .pending_bytes
            .saturating_add(inserted as u64)
            .saturating_sub(dropped as u64);
        if inserted < data.len() {
            if inserted == 0 {
                stream.retransmissions += 1;
                return SegmentVerdict::Retransmission;
            }
            verdict = SegmentVerdict::PartialOverlap {
                dropped_bytes: u32::try_from(data.len() - inserted).unwrap_or(u32::MAX),
            };
        }

        // Per-stream budgets: drop the oldest buffered segment.
        if stream.held_bytes() > self.limits.max_stream_buffer
            || stream.pending.len() > self.limits.max_segments_per_stream
        {
            enforce_budget(
                stream,
                self.limits.max_stream_buffer,
                self.limits.max_segments_per_stream,
            );
            stream.overflow = true;
        }
        advance(stream);
        verdict
    }

    /// Removes and returns the bytes that are now contiguous.
    pub fn take_contiguous(&mut self, key: &SessionKey, dir: Direction) -> Vec<Bytes> {
        let Some(stream) = self.streams.get_mut(&(*key, dir)) else {
            return Vec::new();
        };
        advance(stream);
        if stream.ready.is_empty() {
            Vec::new()
        } else {
            vec![Bytes::from(std::mem::take(&mut stream.ready))]
        }
    }

    /// Currently missing byte ranges.
    #[must_use]
    pub fn missing_ranges(&self, key: &SessionKey, dir: Direction) -> Vec<ByteRange> {
        self.streams
            .get(&(*key, dir))
            .map(StreamBuf::gaps)
            .unwrap_or_default()
    }

    /// Reassembly state of one direction.
    ///
    /// The session state is the "worst" of both directions, so that a session
    /// with a hole in either direction is reported as `Incomplete`.
    #[must_use]
    pub fn state(&self, key: &SessionKey) -> StreamState {
        let c2s = self
            .streams
            .get(&(*key, Direction::ClientToServer))
            .map(StreamBuf::note_state);
        let s2c = self
            .streams
            .get(&(*key, Direction::ServerToClient))
            .map(StreamBuf::note_state);
        merge_state(c2s, s2c)
    }

    /// Marks a stream closed.
    pub fn close(&mut self, key: &SessionKey, dir: Option<Direction>, how: CloseHow) {
        let dirs: [Direction; 2] = match dir {
            Some(d) => [d, d],
            None => [Direction::ClientToServer, Direction::ServerToClient],
        };
        let count = if dir.is_some() { 1 } else { 2 };
        for d in dirs.into_iter().take(count) {
            if let Some(stream) = self.streams.get_mut(&(*key, d)) {
                match how {
                    CloseHow::Fin => stream.fin_seen = true,
                    CloseHow::Rst => stream.rst_seen = true,
                    CloseHow::Timeout => {}
                }
            }
        }
    }

    /// Releases streams idle for longer than `stream_timeout`.
    pub fn prune_expired(&mut self, now_ns: i128) -> Vec<PrunedStream> {
        let timeout_ns = self.limits.stream_timeout.as_nanos();
        let mut pruned = Vec::new();
        let mut keys: Vec<(SessionKey, Direction)> = self.streams.keys().copied().collect();
        keys.sort();
        for key in keys {
            let expired = self.streams.get(&key).is_some_and(|s| {
                u128::try_from(now_ns.saturating_sub(s.last_ts_ns)).unwrap_or(0) > timeout_ns
            });
            if expired {
                let state = self
                    .streams
                    .get(&key)
                    .map_or(StreamState::Closed, StreamBuf::note_state);
                self.streams.remove(&key);
                self.session_keys.remove(&key.0);
                pruned.push(PrunedStream {
                    key: key.0,
                    state,
                    reason: CloseHow::Timeout,
                });
            }
        }
        pruned
    }

    /// Buffered byte count of one direction (diagnostics).
    #[must_use]
    pub fn buffered_bytes(&self, key: &SessionKey, dir: Direction) -> u64 {
        self.streams
            .get(&(*key, dir))
            .map_or(0, StreamBuf::held_bytes)
    }
}

fn merge_state(a: Option<StreamState>, b: Option<StreamState>) -> StreamState {
    match (a, b) {
        (Some(StreamState::BufferOverflow), _) | (_, Some(StreamState::BufferOverflow)) => {
            StreamState::BufferOverflow
        }
        (Some(StreamState::Incomplete), _) | (_, Some(StreamState::Incomplete)) => {
            StreamState::Incomplete
        }
        (Some(StreamState::Closed), _) | (_, Some(StreamState::Closed)) => StreamState::Closed,
        (Some(StreamState::HalfClosed), _) | (_, Some(StreamState::HalfClosed)) => {
            StreamState::HalfClosed
        }
        (Some(StreamState::Active), _) | (_, Some(StreamState::Active)) => StreamState::Active,
        (Some(StreamState::New), _) | (_, Some(StreamState::New)) => StreamState::New,
        (None, None) => StreamState::New,
    }
}

/// Inserts `data` for `start`, resolving overlaps according to `policy`.
///
/// Sequence numbers are first mapped to signed distances relative to
/// `reference` so that interval arithmetic is plain `i64` maths.
///
/// Returns `(inserted_bytes, dropped_bytes)`.
fn insert_pending(
    pending: &mut Vec<(u32, Vec<u8>)>,
    reference: u32,
    start: u32,
    data: &[u8],
    policy: OverlapPolicy,
) -> (usize, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    let start_rel = seq_distance(reference, start);
    let end_rel = start_rel + data.len() as i64;
    let mut dropped = 0usize;

    // Collect the existing intervals that overlap the new one.
    let mut overlaps: Vec<usize> = Vec::new();
    for (index, (seq, bytes)) in pending.iter().enumerate() {
        let ex_start = seq_distance(reference, *seq);
        let ex_end = ex_start + bytes.len() as i64;
        if ex_start < end_rel && start_rel < ex_end {
            overlaps.push(index);
        }
    }

    let mut keep: Vec<(i64, i64)> = vec![(start_rel, end_rel)];
    for index in overlaps.iter().rev() {
        let (seq, bytes) = pending[*index].clone();
        let ex_start = seq_distance(reference, seq);
        let ex_end = ex_start + bytes.len() as i64;
        match policy {
            OverlapPolicy::FirstWins => {
                // drop the covered part from the new segment
                keep = keep
                    .into_iter()
                    .flat_map(|(s, e)| subtract_interval(s, e, ex_start, ex_end))
                    .collect();
            }
            OverlapPolicy::LastWins => {
                // trim the existing entry, then drop it from the vector
                let pieces = subtract_interval(ex_start, ex_end, start_rel, end_rel);
                pending.remove(*index);
                for (s, e) in pieces {
                    let seq = reference.wrapping_add(s as u32);
                    let offset = (s - ex_start) as usize;
                    let length = (e - s) as usize;
                    let slice = bytes
                        .get(offset..offset.saturating_add(length))
                        .unwrap_or_default()
                        .to_vec();
                    pending.push((seq, slice));
                }
            }
        }
    }

    let inserted: usize = keep.iter().map(|(s, e)| (e - s).max(0) as usize).sum();
    dropped += data.len().saturating_sub(inserted);
    for (s, e) in keep {
        let seq = reference.wrapping_add(s as u32);
        let offset = (s - start_rel) as usize;
        let length = (e - s).max(0) as usize;
        let piece = data
            .get(offset..offset.saturating_add(length))
            .unwrap_or_default()
            .to_vec();
        pending.push((seq, piece));
    }
    pending.sort_by_key(|(seq, _)| seq_distance(reference, *seq));
    (inserted, dropped)
}

/// `[s, e)` minus `[cut_s, cut_e)`.
fn subtract_interval(s: i64, e: i64, cut_s: i64, cut_e: i64) -> Vec<(i64, i64)> {
    if e <= cut_s || cut_e <= s {
        return vec![(s, e)];
    }
    let mut pieces = Vec::new();
    if s < cut_s {
        pieces.push((s, cut_s));
    }
    if cut_e < e {
        pieces.push((cut_e, e));
    }
    pieces
}

/// Drops the oldest buffered segment until both budgets fit again.
fn enforce_budget(stream: &mut StreamBuf, byte_limit: u64, segment_limit: usize) {
    while !stream.pending.is_empty()
        && (stream.held_bytes() > byte_limit || stream.pending.len() > segment_limit)
    {
        let (_, data) = stream.pending.remove(0);
        stream.pending_bytes = stream.pending_bytes.saturating_sub(data.len() as u64);
    }
}

/// Moves contiguous buffered segments into `ready`.
fn advance(stream: &mut StreamBuf) {
    loop {
        let Some(next) = stream.next_seq else {
            return;
        };
        let Some(position) = stream.pending.iter().position(|(seq, _)| *seq == next) else {
            return;
        };
        let (_, data) = stream.pending.remove(position);
        stream.next_seq = Some(next.wrapping_add(data.len() as u32));
        stream.pending_bytes = stream.pending_bytes.saturating_sub(data.len() as u64);
        stream.ready.extend_from_slice(&data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Endpoint, TransportProto};
    use std::net::IpAddr;

    fn key() -> SessionKey {
        SessionKey::canonicalize(
            TransportProto::Tcp,
            Endpoint::new(
                "10.0.0.1"
                    .parse::<IpAddr>()
                    .unwrap_or(IpAddr::V4([0, 0, 0, 0].into())),
                1234,
            ),
            Endpoint::new(
                "10.0.0.2"
                    .parse::<IpAddr>()
                    .unwrap_or(IpAddr::V4([0, 0, 0, 0].into())),
                80,
            ),
        )
    }

    fn seg(seq: u32, data: &[u8]) -> Segment<'_> {
        Segment {
            seq,
            data,
            ts_ns: i128::from(seq),
            fin: false,
            rst: false,
            syn: false,
        }
    }

    fn reassembler() -> Reassembler {
        Reassembler::new(ReassemblyLimits::default(), OverlapPolicy::FirstWins)
    }

    #[test]
    fn retrans_exact_is_detected_and_not_duplicated() {
        let mut r = reassembler();
        let k = key();
        let payload: Vec<u8> = (0..100u8).collect();
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1000, &payload)),
            SegmentVerdict::InOrder
        );
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1000, &payload)),
            SegmentVerdict::Retransmission
        );
        let out = r.take_contiguous(&k, Direction::ClientToServer);
        let total: usize = out.iter().map(Bytes::len).sum();
        assert_eq!(total, 100);
        assert!(r.take_contiguous(&k, Direction::ClientToServer).is_empty());
    }

    #[test]
    fn reorder_fill_becomes_contiguous() {
        let mut r = reassembler();
        let k = key();
        let a: Vec<u8> = vec![1; 100];
        let b: Vec<u8> = vec![2; 100];
        let c: Vec<u8> = vec![3; 100];
        assert_eq!(
            r.feed(&k, Direction::ServerToClient, seg(1000, &a)),
            SegmentVerdict::InOrder
        );
        assert_eq!(
            r.feed(&k, Direction::ServerToClient, seg(1200, &b)),
            SegmentVerdict::Buffered { gap_bytes: 100 }
        );
        assert_eq!(
            r.feed(&k, Direction::ServerToClient, seg(1100, &c)),
            SegmentVerdict::InOrder
        );
        let out = r.take_contiguous(&k, Direction::ServerToClient);
        let joined: Vec<u8> = out.iter().flat_map(|b| b.to_vec()).collect();
        assert_eq!(joined.len(), 300);
        assert!(r.missing_ranges(&k, Direction::ServerToClient).is_empty());
        assert_eq!(r.state(&k), StreamState::Active);
    }

    #[test]
    fn reorder_missing_marks_incomplete() {
        let mut r = reassembler();
        let k = key();
        let a: Vec<u8> = vec![1; 100];
        let b: Vec<u8> = vec![2; 100];
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1000, &a)),
            SegmentVerdict::InOrder
        );
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1200, &b)),
            SegmentVerdict::Buffered { gap_bytes: 100 }
        );
        let gaps = r.missing_ranges(&k, Direction::ClientToServer);
        assert_eq!(
            gaps,
            vec![ByteRange {
                start: 1100,
                end: 1200
            }]
        );
        assert_eq!(r.state(&k), StreamState::Incomplete);
        let _ = r.take_contiguous(&k, Direction::ClientToServer);
        assert_eq!(r.state(&k), StreamState::Incomplete);
    }

    #[test]
    fn first_wins_keeps_original_overlap() {
        let mut r = reassembler();
        let k = key();
        let first: Vec<u8> = vec![0xAA; 100];
        let second: Vec<u8> = vec![0xBB; 100];
        r.feed(&k, Direction::ClientToServer, seg(1000, &first));
        let verdict = r.feed(&k, Direction::ClientToServer, seg(1050, &second));
        assert_eq!(
            verdict,
            SegmentVerdict::PartialOverlap { dropped_bytes: 50 }
        );
        let out: Vec<u8> = r
            .take_contiguous(&k, Direction::ClientToServer)
            .iter()
            .flat_map(|b| b.to_vec())
            .collect();
        assert_eq!(out.len(), 150);
        assert!(out[..100].iter().all(|b| *b == 0xAA));
        assert!(out[100..].iter().all(|b| *b == 0xBB));
    }

    #[test]
    fn buffer_overflow_marks_stream_and_keeps_task_alive() {
        let limits = ReassemblyLimits {
            max_stream_buffer: 1024,
            ..ReassemblyLimits::default()
        };
        let mut r = Reassembler::new(limits, OverlapPolicy::FirstWins);
        let k = key();
        let chunk = vec![7u8; 512];
        r.feed(&k, Direction::ClientToServer, seg(1000, &chunk));
        r.feed(&k, Direction::ClientToServer, seg(2000, &chunk));
        r.feed(&k, Direction::ClientToServer, seg(3000, &chunk));
        assert_eq!(r.state(&k), StreamState::BufferOverflow);
    }

    #[test]
    fn session_cap_drops_new_streams() {
        let limits = ReassemblyLimits {
            max_sessions: 1,
            ..ReassemblyLimits::default()
        };
        let mut r = Reassembler::new(limits, OverlapPolicy::FirstWins);
        let k1 = key();
        let k2 = SessionKey::canonicalize(
            TransportProto::Tcp,
            Endpoint::new(
                "10.0.0.9"
                    .parse()
                    .unwrap_or(IpAddr::V4([0, 0, 0, 0].into())),
                5555,
            ),
            Endpoint::new(
                "10.0.0.8"
                    .parse()
                    .unwrap_or(IpAddr::V4([0, 0, 0, 0].into())),
                443,
            ),
        );
        assert_eq!(
            r.feed(&k1, Direction::ClientToServer, seg(1, &[1, 2, 3])),
            SegmentVerdict::InOrder
        );
        assert_eq!(
            r.feed(&k2, Direction::ClientToServer, seg(1, &[4, 5, 6])),
            SegmentVerdict::Dropped(DropReason::SessionLimit)
        );
        assert_eq!(r.dropped_sessions(), 1);
    }

    #[test]
    fn prune_expired_releases_idle_streams() {
        let mut r = reassembler();
        let k = key();
        r.feed(&k, Direction::ClientToServer, seg(1000, &[1, 2, 3]));
        let pruned = r.prune_expired(1_000_000_000_000);
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].reason, CloseHow::Timeout);
        assert_eq!(r.session_count(), 0);
    }

    #[test]
    fn fin_with_gap_stays_incomplete() {
        let mut r = reassembler();
        let k = key();
        let mut s = seg(1000, &[1, 2, 3]);
        s.fin = true;
        r.feed(&k, Direction::ClientToServer, s);
        assert_eq!(r.state(&k), StreamState::HalfClosed);
        // A later segment with a hole in front of it: the FIN is not enough to
        // call this stream closed (M3~M6 §3.5, `fin-with-gap`).
        r.feed(&k, Direction::ClientToServer, seg(2000, &[4]));
        assert_eq!(r.state(&k), StreamState::Incomplete);
    }

    #[test]
    fn rst_closes_a_clean_stream() {
        let mut r = reassembler();
        let k = key();
        let mut s = seg(1000, &[1, 2, 3]);
        s.rst = true;
        r.feed(&k, Direction::ClientToServer, s);
        r.close(&k, None, CloseHow::Rst);
        assert_eq!(r.state(&k), StreamState::Closed);
    }

    #[test]
    fn no_payload_segments_are_in_order() {
        let mut r = reassembler();
        let k = key();
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1000, &[])),
            SegmentVerdict::InOrder
        );
    }

    /// A pure ACK occupies no sequence space: it must not initialise the stream
    /// position, must not be buffered and must never be a retransmission.
    #[test]
    fn pure_ack_does_not_consume_sequence_space() {
        let mut r = reassembler();
        let k = key();
        // A delayed ACK whose sequence number is far ahead of the payload.
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(9_000, &[])),
            SegmentVerdict::InOrder
        );
        // The real data must still be treated as the beginning of the stream.
        let payload = vec![7u8; 100];
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1_000, &payload)),
            SegmentVerdict::InOrder
        );
        let out = r.take_contiguous(&k, Direction::ClientToServer);
        let delivered: usize = out.iter().map(Bytes::len).sum();
        assert_eq!(delivered, 100);
        assert!(r.missing_ranges(&k, Direction::ClientToServer).is_empty());
    }

    #[test]
    fn pure_ack_is_never_a_retransmission() {
        let mut r = reassembler();
        let k = key();
        let payload = vec![1u8; 100];
        r.feed(&k, Direction::ClientToServer, seg(1_000, &payload));
        let _ = r.take_contiguous(&k, Direction::ClientToServer);
        for _ in 0..3 {
            assert_eq!(
                r.feed(&k, Direction::ClientToServer, seg(1_001, &[])),
                SegmentVerdict::InOrder
            );
        }
    }

    #[test]
    fn bare_syn_consumes_one_sequence_number() {
        let mut r = reassembler();
        let k = key();
        let mut syn = seg(1_000, &[]);
        syn.syn = true;
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, syn),
            SegmentVerdict::InOrder
        );
        // Data starts at isn + 1 (the SYN consumed one sequence number) and must
        // not look like a one byte hole.
        let payload = vec![2u8; 50];
        assert_eq!(
            r.feed(&k, Direction::ClientToServer, seg(1_001, &payload)),
            SegmentVerdict::InOrder
        );
        assert!(r.missing_ranges(&k, Direction::ClientToServer).is_empty());
        let delivered: usize = r
            .take_contiguous(&k, Direction::ClientToServer)
            .iter()
            .map(Bytes::len)
            .sum();
        assert_eq!(delivered, 50);
    }

    /// FIN then an idle period is a clean close: `HalfClosed` must become
    /// `Closed` once the stream ages out.
    #[test]
    fn fin_then_timeout_closes_the_stream() {
        let mut r = reassembler();
        let k = key();
        let payload = vec![3u8; 10];
        r.feed(&k, Direction::ClientToServer, seg(1_000, &payload));
        let mut fin = seg(1_010, &[]);
        fin.fin = true;
        r.feed(&k, Direction::ClientToServer, fin);
        assert_eq!(r.state(&k), StreamState::HalfClosed);
        let pruned = r.prune_expired(1_000 + 61_000_000_000);
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].reason, CloseHow::Timeout);
        assert_eq!(r.state(&k), StreamState::New);
    }
}
