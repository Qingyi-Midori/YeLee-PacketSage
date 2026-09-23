//! Protocol decoding matrix (M0~M2 §4.4, 开发文档 §9).
//!
//! Every packet enters through [`Decoder::decode`]. Link-type dispatch and
//! layer attribution are centralised here so that `decode_error` events always
//! name the layer that actually failed.

pub mod app;

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use etherparse::err::packet::SliceError;
use etherparse::{ether_type, IpNumber, LinkSlice, NetSlice, SlicedPacket, TransportSlice, VlanId};
use packetsage_protocol::{
    AppProto, DecodeErrorCode, DecodeErrorInfo, DecodeStatus, IcmpInfo, Layer, LinkInfo, NetProto,
    NetworkInfo, TcpFlag, TransportInfo,
};

use crate::model::Decoded;
use crate::reader::{LINKTYPE_ETHERNET, LINKTYPE_LINUX_SLL};

/// Which application protocols the decoder should attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Enabled application protocols.
    pub app_protocols: BTreeSet<AppProto>,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            app_protocols: [AppProto::Dns, AppProto::Http, AppProto::Tls, AppProto::Dhcp]
                .into_iter()
                .collect(),
        }
    }
}

/// Stateless packet decoder.
#[derive(Debug, Clone, Default)]
pub struct Decoder {
    opts: DecodeOptions,
}

impl Decoder {
    /// Creates a decoder with explicit options.
    #[must_use]
    pub fn new(opts: DecodeOptions) -> Self {
        Self { opts }
    }

    /// Creates a decoder with every application protocol enabled.
    #[must_use]
    pub fn all() -> Self {
        Self {
            opts: DecodeOptions::default(),
        }
    }

    /// Decoding options.
    #[must_use]
    pub fn options(&self) -> &DecodeOptions {
        &self.opts
    }

    /// Decodes one raw record.
    ///
    /// Never panics and never returns `Err`: failures are reported through
    /// [`Decoded::errors`] so the pipeline can keep going (不崩溃原则).
    #[must_use]
    pub fn decode<'a>(&self, linktype: u32, data: &'a [u8]) -> Decoded<'a> {
        let mut decoded = Decoded::default();
        let sliced = match linktype {
            LINKTYPE_ETHERNET => SlicedPacket::from_ethernet(data),
            LINKTYPE_LINUX_SLL => SlicedPacket::from_linux_sll(data),
            other => {
                decoded.errors.push(DecodeErrorInfo {
                    layer: Layer::Link,
                    code: DecodeErrorCode::UnsupportedLinktype,
                    message: Some(format!("linktype {other} is outside the supported matrix")),
                });
                return decoded;
            }
        };
        let sliced = match sliced {
            Ok(sliced) => sliced,
            Err(err) => {
                let (layer, code) = classify_slice_error(&err);
                decoded.errors.push(DecodeErrorInfo {
                    layer,
                    code,
                    message: Some(err.to_string()),
                });
                return decoded;
            }
        };

        decoded.link = Some(link_info(&sliced));
        decoded.network = network_info(&sliced.net);
        decoded.transport = transport_info(&sliced.transport);

        if let Some(net) = &sliced.net {
            if matches!(net, NetSlice::Arp(_)) {
                decoded.network = arp_network_info(net);
            }
        }

        // Application payload: the innermost bytes of the packet.
        let payload: Option<&'a [u8]> = match (&sliced.transport, sliced.ip_payload()) {
            (Some(TransportSlice::Tcp(tcp)), _) => Some(tcp.payload()),
            (Some(TransportSlice::Udp(udp)), _) => Some(udp.payload()),
            (Some(TransportSlice::Icmpv4(icmp)), _) => Some(icmp.payload()),
            (Some(TransportSlice::Icmpv6(icmp)), _) => Some(icmp.payload()),
            (None, Some(ip_payload)) => Some(ip_payload.payload),
            _ => None,
        };
        decoded.payload = payload.filter(|p| !p.is_empty());

        if let (Some(transport), Some(payload)) = (decoded.transport.as_ref(), decoded.payload) {
            let fragmented = sliced.is_ip_payload_fragmented();
            if !fragmented {
                if let Some(app) =
                    app::detect(&self.opts, transport, payload, decoded.network.as_ref())
                {
                    decoded.application = Some(app);
                }
            }
        }

        decoded
    }

    /// Decode status derived from the collected errors.
    #[must_use]
    pub fn status(decoded: &Decoded<'_>) -> DecodeStatus {
        if decoded.errors.is_empty() {
            DecodeStatus::Ok
        } else if decoded.network.is_some() || decoded.transport.is_some() {
            DecodeStatus::Partial
        } else {
            DecodeStatus::Error
        }
    }
}

fn classify_slice_error(err: &SliceError) -> (Layer, DecodeErrorCode) {
    use etherparse::err::Layer as EthernetLayer;
    match err {
        SliceError::Len(len) => {
            let layer = match len.layer {
                EthernetLayer::LinuxSllHeader
                | EthernetLayer::Ethernet2Header
                | EthernetLayer::EtherPayload
                | EthernetLayer::VlanHeader
                | EthernetLayer::MacsecHeader
                | EthernetLayer::MacsecPacket => Layer::Link,
                EthernetLayer::IpHeader
                | EthernetLayer::Ipv4Header
                | EthernetLayer::Ipv4Packet
                | EthernetLayer::IpAuthHeader
                | EthernetLayer::Arp => Layer::Ipv4,
                EthernetLayer::Ipv6Header
                | EthernetLayer::Ipv6Packet
                | EthernetLayer::Ipv6ExtHeader
                | EthernetLayer::Ipv6HopByHopHeader
                | EthernetLayer::Ipv6DestOptionsHeader
                | EthernetLayer::Ipv6RouteHeader
                | EthernetLayer::Ipv6FragHeader => Layer::Ipv6,
                EthernetLayer::UdpHeader | EthernetLayer::UdpPayload => Layer::Udp,
                EthernetLayer::TcpHeader => Layer::Tcp,
                EthernetLayer::Icmpv4 => Layer::Icmp,
                EthernetLayer::Icmpv4Timestamp | EthernetLayer::Icmpv4TimestampReply => Layer::Icmp,
                EthernetLayer::Icmpv6 => Layer::Icmpv6,
            };
            let code = if len.required_len > len.len {
                DecodeErrorCode::TruncatedHeader
            } else {
                DecodeErrorCode::BadLength
            };
            (layer, code)
        }
        SliceError::LinuxSll(_) | SliceError::Macsec(_) => {
            (Layer::Link, DecodeErrorCode::InvalidField)
        }
        SliceError::Ip(_) => (Layer::Ipv4, DecodeErrorCode::InvalidField),
        SliceError::Ipv4(_) | SliceError::Ipv4Exts(_) => {
            (Layer::Ipv4, DecodeErrorCode::InvalidField)
        }
        SliceError::Ipv6(_) | SliceError::Ipv6Exts(_) => {
            (Layer::Ipv6, DecodeErrorCode::InvalidField)
        }
        SliceError::Tcp(_) => (Layer::Tcp, DecodeErrorCode::InvalidField),
    }
}

fn mac_to_string(bytes: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
}

fn link_info(sliced: &SlicedPacket<'_>) -> LinkInfo {
    let mut info = LinkInfo {
        src_mac: None,
        dst_mac: None,
        ethertype: None,
        vlan_ids: Vec::new(),
        sll_protocol: None,
    };
    match sliced.link.as_ref() {
        Some(LinkSlice::Ethernet2(eth)) => {
            info.src_mac = Some(mac_to_string(&eth.source()));
            info.dst_mac = Some(mac_to_string(&eth.destination()));
            info.ethertype = Some(eth.ether_type().0);
        }
        Some(LinkSlice::LinuxSll(sll)) => {
            info.sll_protocol = Some(u16::from(sll.protocol_type()));
            info.src_mac = Some(mac_to_string(&[
                sll.sender_address_full()[0],
                sll.sender_address_full()[1],
                sll.sender_address_full()[2],
                sll.sender_address_full()[3],
                sll.sender_address_full()[4],
                sll.sender_address_full()[5],
            ]));
        }
        _ => {}
    }
    info.vlan_ids = sliced.vlan_ids().into_iter().map(VlanId::value).collect();
    if info.ethertype.is_none() {
        info.ethertype = vlan_ether_type(sliced);
    }
    info
}

fn vlan_ether_type(sliced: &SlicedPacket<'_>) -> Option<u16> {
    sliced.payload_ether_type().map(|t| t.0)
}

fn network_info(net: &Option<NetSlice<'_>>) -> Option<NetworkInfo> {
    match net.as_ref()? {
        NetSlice::Ipv4(ipv4) => {
            let header = ipv4.header();
            Some(NetworkInfo {
                src: IpAddr::V4(header.source_addr()).to_string(),
                dst: IpAddr::V4(header.destination_addr()).to_string(),
                protocol: ip_number_to_proto(header.protocol()),
                ip_version: 4,
                ttl: Some(header.ttl()),
                hop_limit: None,
                is_fragment: ipv4.is_payload_fragmented(),
            })
        }
        NetSlice::Ipv6(ipv6) => {
            let header = ipv6.header();
            Some(NetworkInfo {
                src: IpAddr::V6(header.source_addr()).to_string(),
                dst: IpAddr::V6(header.destination_addr()).to_string(),
                protocol: ip_number_to_proto(header.next_header()),
                ip_version: 6,
                ttl: None,
                hop_limit: Some(header.hop_limit()),
                is_fragment: ipv6.is_payload_fragmented(),
            })
        }
        NetSlice::Arp(_) => None,
    }
}

fn arp_network_info(net: &NetSlice<'_>) -> Option<NetworkInfo> {
    let arp = net.arp_ref()?.to_packet().try_eth_ipv4().ok()?;
    let [a, b, c, d] = arp.sender_ipv4;
    let [e, f, g, h] = arp.target_ipv4;
    Some(NetworkInfo {
        src: IpAddr::V4(Ipv4Addr::new(a, b, c, d)).to_string(),
        dst: IpAddr::V4(Ipv4Addr::new(e, f, g, h)).to_string(),
        protocol: NetProto::Arp,
        ip_version: 4,
        ttl: None,
        hop_limit: None,
        is_fragment: false,
    })
}

fn ip_number_to_proto(number: IpNumber) -> NetProto {
    if number == IpNumber::TCP {
        NetProto::Tcp
    } else if number == IpNumber::UDP {
        NetProto::Udp
    } else if number == IpNumber::ICMP {
        NetProto::Icmp
    } else if number == IpNumber::IPV6_ICMP {
        NetProto::Icmpv6
    } else {
        NetProto::Other
    }
}

fn transport_info(transport: &Option<TransportSlice<'_>>) -> Option<TransportInfo> {
    match transport.as_ref()? {
        TransportSlice::Tcp(tcp) => Some(TransportInfo {
            src_port: Some(tcp.source_port()),
            dst_port: Some(tcp.destination_port()),
            flags: tcp_flags(tcp),
            seq: Some(tcp.sequence_number()),
            ack: Some(tcp.acknowledgment_number()),
            window: Some(tcp.window_size()),
            udp_len: None,
            icmp: None,
        }),
        TransportSlice::Udp(udp) => Some(TransportInfo {
            src_port: Some(udp.source_port()),
            dst_port: Some(udp.destination_port()),
            flags: Vec::new(),
            seq: None,
            ack: None,
            window: None,
            udp_len: Some(udp.length()),
            icmp: None,
        }),
        TransportSlice::Icmpv4(icmp) => Some(TransportInfo {
            src_port: None,
            dst_port: None,
            flags: Vec::new(),
            seq: None,
            ack: None,
            window: None,
            udp_len: None,
            icmp: Some(IcmpInfo {
                icmp_type: icmp.type_u8(),
                code: icmp.code_u8(),
            }),
        }),
        TransportSlice::Icmpv6(icmp) => Some(TransportInfo {
            src_port: None,
            dst_port: None,
            flags: Vec::new(),
            seq: None,
            ack: None,
            window: None,
            udp_len: None,
            icmp: Some(IcmpInfo {
                icmp_type: icmp.type_u8(),
                code: icmp.code_u8(),
            }),
        }),
    }
}

fn tcp_flags(tcp: &etherparse::TcpSlice<'_>) -> Vec<TcpFlag> {
    let mut flags = Vec::new();
    if tcp.fin() {
        flags.push(TcpFlag::Fin);
    }
    if tcp.syn() {
        flags.push(TcpFlag::Syn);
    }
    if tcp.rst() {
        flags.push(TcpFlag::Rst);
    }
    if tcp.psh() {
        flags.push(TcpFlag::Psh);
    }
    if tcp.ack() {
        flags.push(TcpFlag::Ack);
    }
    if tcp.urg() {
        flags.push(TcpFlag::Urg);
    }
    if tcp.ece() {
        flags.push(TcpFlag::Ece);
    }
    if tcp.cwr() {
        flags.push(TcpFlag::Cwr);
    }
    flags
}

/// EtherType constant re-export used by the application heuristics.
pub use ether_type::IPV4 as ETHER_TYPE_IPV4;

/// Helper used by tests: build an IPv6 address string.
#[must_use]
pub fn ipv6_to_string(addr: Ipv6Addr) -> String {
    addr.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::{Ethernet2Header, IpNumber, PacketBuilder};

    fn tcp_packet(payload: &[u8]) -> Vec<u8> {
        let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
            .ipv4([192, 168, 1, 1], [192, 168, 1, 2], 64)
            .tcp(12345, 80, 1, 8192);
        let mut packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut packet, payload).expect("build");
        // flags byte of the TCP header: ethernet(14) + ipv4(20) + 13
        if let Some(flags) = packet.get_mut(14 + 20 + 13) {
            *flags |= 0x02; // SYN
        }
        packet
    }

    #[test]
    fn decodes_ethernet_ipv4_tcp() {
        let decoder = Decoder::all();
        let packet = tcp_packet(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n");
        let decoded = decoder.decode(LINKTYPE_ETHERNET, &packet);
        assert!(decoded.errors.is_empty());
        let network = decoded.network.expect("network");
        assert_eq!(network.src, "192.168.1.1");
        assert_eq!(network.dst, "192.168.1.2");
        assert_eq!(network.protocol, NetProto::Tcp);
        assert_eq!(network.ttl, Some(64));
        let transport = decoded.transport.expect("transport");
        assert_eq!(transport.src_port, Some(12345));
        assert_eq!(transport.dst_port, Some(80));
        assert!(transport.flags.contains(&TcpFlag::Syn));
        assert!(!transport.flags.contains(&TcpFlag::Ack));
        assert_eq!(transport.seq, Some(1));
        assert_eq!(transport.window, Some(8192));
        let app = decoded.application.expect("http");
        assert_eq!(app.protocol, AppProto::Http);
    }

    #[test]
    fn truncated_ipv4_header_is_reported_as_truncated() {
        let decoder = Decoder::all();
        // Ethernet header + 6 bytes of IPv4 header.
        let mut packet = vec![0u8; 14];
        packet[12] = 0x08;
        packet[13] = 0x00;
        packet.extend_from_slice(&[0x45, 0, 0, 20, 0, 0]);
        let decoded = decoder.decode(LINKTYPE_ETHERNET, &packet);
        assert!(!decoded.errors.is_empty());
        assert_eq!(decoded.errors[0].code, DecodeErrorCode::TruncatedHeader);
    }

    #[test]
    fn unsupported_linktype_is_reported() {
        let decoder = Decoder::all();
        let decoded = decoder.decode(147, &[0u8; 32]);
        assert_eq!(decoded.errors[0].layer, Layer::Link);
        assert_eq!(decoded.errors[0].code, DecodeErrorCode::UnsupportedLinktype);
    }

    #[test]
    fn vlan_ids_are_extracted() {
        let payload = [1u8, 2, 3, 4];
        let vlan = etherparse::VlanHeader::Single(etherparse::SingleVlanHeader {
            pcp: etherparse::VlanPcp::try_new(0).expect("pcp"),
            drop_eligible_indicator: false,
            vlan_id: etherparse::VlanId::try_new(100).expect("vlan"),
            ether_type: ether_type::IPV4,
        });
        let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
            .vlan(vlan)
            .ipv4([10, 0, 0, 1], [10, 0, 0, 2], 64)
            .udp(53, 5353);
        let mut packet = Vec::with_capacity(builder.size(4));
        builder.write(&mut packet, &payload).expect("build");
        let decoded = Decoder::all().decode(LINKTYPE_ETHERNET, &packet);
        let link = decoded.link.expect("link");
        assert_eq!(link.vlan_ids, vec![100]);
    }

    #[test]
    fn arp_is_decoded() {
        let arp = etherparse::ArpPacket::new(
            etherparse::ArpHardwareId::ETHERNET,
            ether_type::IPV4,
            etherparse::ArpOperation::REQUEST,
            &[1, 2, 3, 4, 5, 6],
            &[192, 168, 1, 1],
            &[0, 0, 0, 0, 0, 0],
            &[192, 168, 1, 2],
        )
        .expect("arp");
        let mut packet = Ethernet2Header {
            source: [1, 2, 3, 4, 5, 6],
            destination: [0xff; 6],
            ether_type: ether_type::ARP,
        }
        .to_bytes()
        .to_vec();
        packet.extend_from_slice(&arp.to_bytes());
        let decoded = Decoder::all().decode(LINKTYPE_ETHERNET, &packet);
        let network = decoded.network.expect("arp network info");
        assert_eq!(network.protocol, NetProto::Arp);
        assert_eq!(network.src, "192.168.1.1");
        assert_eq!(network.dst, "192.168.1.2");
    }

    #[test]
    fn icmp_is_decoded() {
        let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
            .ipv4([10, 0, 0, 1], [10, 0, 0, 2], 64)
            .icmpv4(etherparse::Icmpv4Type::EchoRequest(
                etherparse::IcmpEchoHeader { id: 1, seq: 1 },
            ));
        let mut packet = Vec::with_capacity(builder.size(0));
        builder.write(&mut packet, &[]).expect("build");
        let decoded = Decoder::all().decode(LINKTYPE_ETHERNET, &packet);
        assert!(decoded.errors.is_empty());
        assert_eq!(decoded.network.expect("network").protocol, NetProto::Icmp);
        assert_eq!(
            decoded.transport.expect("icmp").icmp.map(|i| i.icmp_type),
            Some(8)
        );
        assert_eq!(IpNumber::ICMP.0, 1);
    }
}
