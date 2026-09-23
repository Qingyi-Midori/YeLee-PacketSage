//! Internal packet model and the canonical five-tuple (M0~M2 §4.2).

use std::net::IpAddr;

use packetsage_protocol::{
    ApplicationInfo, DecodeErrorInfo, DecodeStatus, LinkInfo, NetworkInfo, PayloadRef, TaskId,
    TransportInfo, TsPrecision,
};

/// Transport protocol of a session key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TransportProto {
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
}

impl TransportProto {
    /// Lowercase wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TransportProto::Tcp => "tcp",
            TransportProto::Udp => "udp",
        }
    }
}

/// One side of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Endpoint {
    /// Address.
    pub ip: IpAddr,
    /// Port.
    pub port: u16,
}

impl Endpoint {
    /// Creates an endpoint.
    #[must_use]
    pub fn new(ip: IpAddr, port: u16) -> Self {
        Self { ip, port }
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.ip, self.port)
    }
}

/// Canonical five-tuple: both directions of one conversation hash equally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    /// Transport protocol.
    pub proto: TransportProto,
    /// Lexically smaller endpoint.
    pub a: Endpoint,
    /// Lexically larger endpoint.
    pub b: Endpoint,
}

impl SessionKey {
    /// Canonicalises `(src, dst)` so that both directions produce one key.
    ///
    /// Ordering follows `std` `IpAddr` `Ord` (V4 < V6, V4 compared bytewise);
    /// no custom comparator is allowed (M0~M2 §4.2).
    #[must_use]
    pub fn canonicalize(proto: TransportProto, src: Endpoint, dst: Endpoint) -> Self {
        if (src.ip, src.port) <= (dst.ip, dst.port) {
            Self {
                proto,
                a: src,
                b: dst,
            }
        } else {
            Self {
                proto,
                a: dst,
                b: src,
            }
        }
    }

    /// Renders the key as `proto|ip:port|ip:port`.
    #[must_use]
    pub fn render(&self) -> String {
        format!("{}|{}|{}", self.proto.as_str(), self.a, self.b)
    }

    /// The endpoint that is not `endpoint` (falls back to `a`).
    #[must_use]
    pub fn other_endpoint(&self, endpoint: Endpoint) -> Endpoint {
        if endpoint == self.a {
            self.b
        } else {
            self.a
        }
    }
}

/// Which endpoint is the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSide {
    /// `key.a` is the client.
    A,
    /// `key.b` is the client.
    B,
}

/// Link-layer decoding result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DecodedLink {
    /// Protocol DTO.
    pub info: Option<LinkInfo>,
}

/// Decoded layers for one packet.
#[derive(Debug, Clone, Default)]
pub struct Decoded<'a> {
    /// Link layer.
    pub link: Option<LinkInfo>,
    /// Network layer.
    pub network: Option<NetworkInfo>,
    /// Transport layer.
    pub transport: Option<TransportInfo>,
    /// Application layer.
    pub application: Option<ApplicationInfo>,
    /// Innermost payload slice (application data).
    pub payload: Option<&'a [u8]>,
    /// Per-layer decode errors.
    pub errors: Vec<DecodeErrorInfo>,
}

/// One packet as seen by the pipeline.
#[derive(Debug, Clone)]
pub struct PacketRecord<'a> {
    /// Task identifier.
    pub task_id: TaskId,
    /// Zero-based packet index inside the capture.
    pub packet_index: u64,
    /// Canonical timestamp in nanoseconds.
    pub ts_ns: i128,
    /// Original timestamp precision.
    pub ts_precision: TsPrecision,
    /// Interface id.
    pub interface_id: u32,
    /// Captured length.
    pub captured_len: u32,
    /// Original length.
    pub original_len: u32,
    /// Link type.
    pub linktype: u32,
    /// Raw link-layer bytes.
    pub raw: &'a [u8],
    /// Decoded layers.
    pub decoded: Decoded<'a>,
    /// Decode outcome.
    pub decode_status: DecodeStatus,
}

impl PacketRecord<'_> {
    /// Packet is a bare SYN (SYN set, ACK clear).
    #[must_use]
    pub fn is_bare_syn(&self) -> bool {
        use packetsage_protocol::TcpFlag;
        match &self.decoded.transport {
            Some(t) => t.flags.contains(&TcpFlag::Syn) && !t.flags.contains(&TcpFlag::Ack),
            None => false,
        }
    }

    /// Packet is a SYN+ACK.
    #[must_use]
    pub fn is_syn_ack(&self) -> bool {
        use packetsage_protocol::TcpFlag;
        match &self.decoded.transport {
            Some(t) => t.flags.contains(&TcpFlag::Syn) && t.flags.contains(&TcpFlag::Ack),
            None => false,
        }
    }

    /// True when the packet carries an RST.
    #[must_use]
    pub fn has_rst(&self) -> bool {
        use packetsage_protocol::TcpFlag;
        self.decoded
            .transport
            .as_ref()
            .is_some_and(|t| t.flags.contains(&TcpFlag::Rst))
    }

    /// True when the packet carries a FIN.
    #[must_use]
    pub fn has_fin(&self) -> bool {
        use packetsage_protocol::TcpFlag;
        self.decoded
            .transport
            .as_ref()
            .is_some_and(|t| t.flags.contains(&TcpFlag::Fin))
    }

    /// Session key when the packet has IP + transport information.
    #[must_use]
    pub fn session_key(&self) -> Option<SessionKey> {
        let network = self.decoded.network.as_ref()?;
        let transport = self.decoded.transport.as_ref()?;
        let src_ip: IpAddr = network.src.parse().ok()?;
        let dst_ip: IpAddr = network.dst.parse().ok()?;
        let proto = match network.protocol {
            packetsage_protocol::NetProto::Tcp => TransportProto::Tcp,
            packetsage_protocol::NetProto::Udp => TransportProto::Udp,
            _ => return None,
        };
        let src = Endpoint::new(src_ip, transport.src_port.unwrap_or(0));
        let dst = Endpoint::new(dst_ip, transport.dst_port.unwrap_or(0));
        Some(SessionKey::canonicalize(proto, src, dst))
    }

    /// Builds a `PayloadRef` from the decoded payload slice.
    #[must_use]
    pub fn payload_ref(&self) -> Option<PayloadRef> {
        let payload = self.decoded.payload?;
        if payload.is_empty() {
            return None;
        }
        let base = self.raw.as_ptr() as usize;
        let start = payload.as_ptr() as usize;
        let offset = start.checked_sub(base)?;
        let offset = u32::try_from(offset).ok()?;
        let length = u32::try_from(payload.len()).ok()?;
        Some(PayloadRef {
            packet_index: self.packet_index,
            offset,
            length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ep(ip: &str, port: u16) -> Endpoint {
        Endpoint::new(
            ip.parse().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            port,
        )
    }

    #[test]
    fn canonicalize_is_direction_agnostic() {
        let a = SessionKey::canonicalize(
            TransportProto::Tcp,
            ep("192.168.1.105", 51344),
            ep("192.168.1.20", 80),
        );
        let b = SessionKey::canonicalize(
            TransportProto::Tcp,
            ep("192.168.1.20", 80),
            ep("192.168.1.105", 51344),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn canonicalize_orders_ip_then_port() {
        let key = SessionKey::canonicalize(
            TransportProto::Udp,
            ep("10.0.0.9", 53),
            ep("10.0.0.1", 40000),
        );
        assert_eq!(key.a.ip.to_string(), "10.0.0.1");
        assert_eq!(key.b.ip.to_string(), "10.0.0.9");
    }

    #[test]
    fn v4_sorts_before_v6() {
        let key = SessionKey::canonicalize(
            TransportProto::Tcp,
            ep("2001:db8::1", 80),
            ep("192.0.2.1", 80),
        );
        assert_eq!(key.a.ip.to_string(), "192.0.2.1");
        assert_eq!(key.b.ip.to_string(), "2001:db8::1");
    }

    #[test]
    fn render_is_stable() {
        let key = SessionKey::canonicalize(
            TransportProto::Tcp,
            ep("10.0.0.2", 80),
            ep("10.0.0.1", 1234),
        );
        assert_eq!(key.render(), "tcp|10.0.0.1:1234|10.0.0.2:80");
    }
}
