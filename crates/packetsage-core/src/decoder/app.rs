//! Application-layer metadata extraction (开发文档 §9).
//!
//! Only *metadata* is produced, and only from fields that are fully parseable
//! inside a single packet (M2v0.2 §3.1). Headers that would need cross-packet
//! reassembly are deliberately left empty, which can under-count but never
//! over-report.

use std::collections::BTreeMap;

use packetsage_protocol::{
    AppDetail, AppProto, ApplicationInfo, DhcpDetail, DnsDetail, HttpDetail, NetworkInfo,
    TlsDetail, TransportInfo,
};

use super::DecodeOptions;

/// Well known ports used by the heuristics.
const DNS_PORT: u16 = 53;
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;

const HTTP_METHODS: [&str; 9] = [
    "GET", "POST", "HEAD", "PUT", "DELETE", "OPTIONS", "TRACE", "CONNECT", "PATCH",
];

/// Classifies a transport payload.
#[must_use]
pub fn detect(
    opts: &DecodeOptions,
    transport: &TransportInfo,
    payload: &[u8],
    network: Option<&NetworkInfo>,
) -> Option<ApplicationInfo> {
    if payload.is_empty() {
        return None;
    }
    let src = transport.src_port.unwrap_or(0);
    let dst = transport.dst_port.unwrap_or(0);
    let is_udp = network.is_some_and(|n| n.protocol == packetsage_protocol::NetProto::Udp);
    let is_tcp = !is_udp;

    if opts.app_protocols.contains(&AppProto::Dhcp)
        && is_udp
        && (src == DHCP_SERVER_PORT
            || dst == DHCP_SERVER_PORT
            || src == DHCP_CLIENT_PORT
            || dst == DHCP_CLIENT_PORT)
    {
        if let Some(detail) = parse_dhcp(payload) {
            return Some(ApplicationInfo {
                protocol: AppProto::Dhcp,
                detail: Some(AppDetail::Dhcp(detail)),
            });
        }
    }

    if opts.app_protocols.contains(&AppProto::Dns) && (src == DNS_PORT || dst == DNS_PORT) {
        if let Some(detail) = parse_dns(payload) {
            return Some(ApplicationInfo {
                protocol: AppProto::Dns,
                detail: Some(AppDetail::Dns(detail)),
            });
        }
    }

    if opts.app_protocols.contains(&AppProto::Tls) && is_tcp && looks_like_tls(payload) {
        if let Some(detail) = parse_tls(payload) {
            return Some(ApplicationInfo {
                protocol: AppProto::Tls,
                detail: Some(AppDetail::Tls(detail)),
            });
        }
    }

    if opts.app_protocols.contains(&AppProto::Http) && is_tcp && looks_like_http(payload) {
        if let Some(detail) = parse_http(payload) {
            return Some(ApplicationInfo {
                protocol: AppProto::Http,
                detail: Some(AppDetail::Http(detail)),
            });
        }
    }

    None
}

fn looks_like_tls(payload: &[u8]) -> bool {
    matches!(payload.first(), Some(0x14..=0x17))
        && payload.len() >= 5
        && payload[1] == 0x03
        && payload[2] <= 0x04
}

fn looks_like_http(payload: &[u8]) -> bool {
    if payload.starts_with(b"HTTP/1.") {
        return true;
    }
    let first_line_end = payload
        .iter()
        .position(|b| *b == b'\n')
        .unwrap_or(payload.len())
        .min(64);
    let line = &payload[..first_line_end];
    HTTP_METHODS
        .iter()
        .any(|m| line.starts_with(m.as_bytes()) && line.get(m.len()) == Some(&b' '))
}

/// Bounded HTTP/1.1 metadata extraction.
#[must_use]
pub fn parse_http(payload: &[u8]) -> Option<HttpDetail> {
    const MAX_HEADER_BYTES: usize = 8_192;
    let window = &payload[..payload.len().min(MAX_HEADER_BYTES)];
    let text = String::from_utf8_lossy(window);
    let mut lines = text.split("\r\n");
    let start_line = lines.next()?;
    let mut detail = HttpDetail {
        is_request: false,
        method: None,
        status_code: None,
        host: None,
        user_agent: None,
        content_length: None,
        connection: None,
    };

    if let Some(rest) = start_line.strip_prefix("HTTP/1.") {
        let mut parts = rest.splitn(2, ' ');
        let _minor = parts.next()?;
        let code = parts.next()?.split(' ').next()?;
        detail.status_code = code.parse::<u16>().ok();
    } else {
        let mut parts = start_line.split(' ');
        let method = parts.next()?;
        if !HTTP_METHODS.contains(&method) {
            return None;
        }
        detail.is_request = true;
        detail.method = Some(method.to_owned());
    }

    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "host" => detail.host = Some(value.to_ascii_lowercase()),
            "user-agent" => detail.user_agent = Some(value.to_owned()),
            "content-length" => detail.content_length = value.parse::<u64>().ok(),
            "connection" => detail.connection = Some(value.to_owned()),
            _ => {}
        }
    }
    Some(detail)
}

/// TLS record metadata, plus SNI when a complete ClientHello is in this packet.
#[must_use]
pub fn parse_tls(payload: &[u8]) -> Option<TlsDetail> {
    if payload.len() < 5 {
        return None;
    }
    let record_type = payload[0];
    let version = u16::from_be_bytes([payload[1], payload[2]]);
    let record_len = usize::from(u16::from_be_bytes([payload[3], payload[4]]));
    let body = payload.get(5..)?.get(..record_len.min(payload.len() - 5))?;

    let mut detail = TlsDetail {
        record_type,
        version,
        is_client_hello: false,
        sni: None,
    };

    if record_type == 0x16 && body.first() == Some(&0x01) && body.len() >= 4 {
        let handshake_len =
            (usize::from(body[1]) << 16) | (usize::from(body[2]) << 8) | usize::from(body[3]);
        let handshake = body.get(4..)?.get(..handshake_len.min(body.len() - 4))?;
        if handshake.len() >= 34 {
            detail.is_client_hello = true;
            let mut cursor = 34usize; // legacy_version + random
            let session_id_len = usize::from(*handshake.get(cursor)?);
            cursor += 1 + session_id_len;
            let cipher_len = usize::from(u16::from_be_bytes([
                *handshake.get(cursor)?,
                *handshake.get(cursor + 1)?,
            ]));
            cursor += 2 + cipher_len;
            let compression_len = usize::from(*handshake.get(cursor)?);
            cursor += 1 + compression_len;
            if cursor + 2 <= handshake.len() {
                let ext_total = usize::from(u16::from_be_bytes([
                    handshake[cursor],
                    handshake[cursor + 1],
                ]));
                cursor += 2;
                let ext_end = (cursor + ext_total).min(handshake.len());
                while cursor + 4 <= ext_end {
                    let ext_type = u16::from_be_bytes([handshake[cursor], handshake[cursor + 1]]);
                    let ext_len = usize::from(u16::from_be_bytes([
                        handshake[cursor + 2],
                        handshake[cursor + 3],
                    ]));
                    cursor += 4;
                    let ext_body = handshake.get(cursor..)?.get(..ext_len)?;
                    if ext_type == 0x0000 {
                        if let Some(name) = parse_sni(ext_body) {
                            detail.sni = Some(name);
                        }
                    }
                    cursor += ext_len;
                }
            }
        }
    }
    Some(detail)
}

fn parse_sni(ext_body: &[u8]) -> Option<String> {
    let list_len = usize::from(u16::from_be_bytes([*ext_body.first()?, *ext_body.get(1)?]));
    let list = ext_body.get(2..)?.get(..list_len.min(ext_body.len() - 2))?;
    if list.first() != Some(&0x00) {
        return None;
    }
    let name_len = usize::from(u16::from_be_bytes([*list.get(1)?, *list.get(2)?]));
    let name = list.get(3..)?.get(..name_len)?;
    Some(String::from_utf8_lossy(name).to_ascii_lowercase())
}

/// DHCP / BOOTP metadata.
#[must_use]
pub fn parse_dhcp(payload: &[u8]) -> Option<DhcpDetail> {
    const BOOTP_LEN: usize = 236;
    const COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
    if payload.len() < BOOTP_LEN + 4 {
        return None;
    }
    let message_type = payload[0];
    if payload[BOOTP_LEN..BOOTP_LEN + 4] != COOKIE {
        return None;
    }
    let mut detail = DhcpDetail {
        message_type,
        dhcp_type: None,
        client_id: None,
        requested_ip: None,
    };
    let mut cursor = BOOTP_LEN + 4;
    while cursor < payload.len() {
        let code = payload[cursor];
        if code == 0 {
            cursor += 1;
            continue;
        }
        if code == 255 {
            break;
        }
        let len = usize::from(*payload.get(cursor + 1)?);
        let value = payload.get(cursor + 2..)?.get(..len)?;
        match code {
            53 => detail.dhcp_type = value.first().copied(),
            50 if value.len() == 4 => {
                detail.requested_ip = Some(format!(
                    "{}.{}.{}.{}",
                    value[0], value[1], value[2], value[3]
                ));
            }
            61 => {
                detail.client_id = Some(if value.iter().all(|b| b.is_ascii_graphic()) {
                    String::from_utf8_lossy(value).to_string()
                } else {
                    value
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join("")
                });
            }
            _ => {}
        }
        cursor += 2 + len;
    }
    Some(detail)
}

/// DNS metadata.
#[must_use]
pub fn parse_dns(payload: &[u8]) -> Option<DnsDetail> {
    if payload.len() < 12 {
        return None;
    }
    let transaction_id = u16::from_be_bytes([payload[0], payload[1]]);
    let flags = u16::from_be_bytes([payload[2], payload[3]]);
    let qdcount = u16::from_be_bytes([payload[4], payload[5]]);
    let ancount = u16::from_be_bytes([payload[6], payload[7]]);

    let mut detail = DnsDetail {
        transaction_id,
        is_response: flags & 0x8000 != 0,
        qname: None,
        qtype: None,
        qname_len: None,
        max_label_len: None,
        entropy: None,
        txt_len: None,
        answer_count: ancount,
    };

    if qdcount > 0 {
        let (name, next, max_label) = read_name(payload, 12)?;
        let name = name.to_ascii_lowercase();
        let qtype = payload
            .get(next..)
            .and_then(|rest| rest.get(..2))
            .map(|b| u16::from_be_bytes([b[0], b[1]]));
        detail.qname_len = Some(name.len() as u32);
        detail.max_label_len = Some(max_label as u32);
        detail.entropy = Some(shannon_entropy_bits(name.as_bytes()));
        detail.qtype = qtype;
        detail.qname = Some(name);
    }

    if ancount > 0 && detail.qname.is_some() {
        let mut cursor = answer_start(payload)?;
        let mut txt_total = 0u32;
        for _ in 0..ancount {
            let (_, next, _) = read_name(payload, cursor)?;
            let header = payload.get(next..)?.get(..10)?;
            let rtype = u16::from_be_bytes([header[0], header[1]]);
            let rdlength = usize::from(u16::from_be_bytes([header[8], header[9]]));
            let rdata = payload.get(next + 10..)?.get(..rdlength)?;
            if rtype == 16 {
                let mut c = 0usize;
                while c < rdata.len() {
                    let len = usize::from(rdata[c]);
                    if c + 1 + len > rdata.len() {
                        break;
                    }
                    txt_total += len as u32;
                    c += 1 + len;
                }
            }
            cursor = next + 10 + rdlength;
        }
        detail.txt_len = Some(txt_total);
    }

    Some(detail)
}

fn answer_start(payload: &[u8]) -> Option<usize> {
    let (_, after_name, _) = read_name(payload, 12)?;
    Some(after_name + 4)
}

/// Reads a (possibly compressed) DNS name.
///
/// Returns the dotted lowercase name, the offset just past the name in the
/// original message, and the longest label length.
fn read_name(message: &[u8], start: usize) -> Option<(String, usize, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut cursor = start;
    let mut end: Option<usize> = None;
    let mut jumps = 0usize;
    let mut max_label = 0usize;
    loop {
        let len = *message.get(cursor)?;
        if len & 0xC0 == 0xC0 {
            let low = *message.get(cursor + 1)?;
            let pointer = usize::from(u16::from_be_bytes([len & 0x3F, low]));
            if end.is_none() {
                end = Some(cursor + 2);
            }
            jumps += 1;
            if jumps > 8 || pointer >= message.len() {
                return None;
            }
            cursor = pointer;
            continue;
        }
        if len == 0 {
            cursor += 1;
            break;
        }
        let len = usize::from(len);
        let label = message.get(cursor + 1..)?.get(..len)?;
        max_label = max_label.max(len);
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        cursor += 1 + len;
    }
    Some((labels.join("."), end.unwrap_or(cursor), max_label))
}

/// Shannon entropy in bits per character, rounded to three decimals.
#[must_use]
pub fn shannon_entropy_bits(bytes: &[u8]) -> f32 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    for b in bytes {
        *counts.entry(*b).or_insert(0) += 1;
    }
    let total = bytes.len() as f64;
    let mut entropy = 0.0f64;
    for count in counts.values() {
        let p = *count as f64 / total;
        if p > 0.0 {
            entropy -= p * p.log2();
        }
    }
    ((entropy * 1000.0).round() / 1000.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dns_query(name: &str, qtype: u16) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&0x1234u16.to_be_bytes());
        msg.extend_from_slice(&0x0100u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        for label in name.split('.') {
            msg.push(label.len() as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.push(0);
        msg.extend_from_slice(&qtype.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg
    }

    #[test]
    fn dns_query_metadata() {
        let msg = dns_query("Example.COM", 1);
        let detail = parse_dns(&msg).expect("dns");
        assert_eq!(detail.transaction_id, 0x1234);
        assert!(!detail.is_response);
        assert_eq!(detail.qname.as_deref(), Some("example.com"));
        assert_eq!(detail.qname_len, Some("example.com".len() as u32));
        assert_eq!(detail.max_label_len, Some(7));
        assert!(detail.entropy.unwrap_or(0.0) > 2.0);
    }

    #[test]
    fn dns_txt_length_is_summed() {
        let mut msg = dns_query("a.example.com", 16);
        msg[6..8].copy_from_slice(&1u16.to_be_bytes()); // ancount = 1
                                                        // answer: pointer to the question name
        msg.extend_from_slice(&[0xC0, 0x0C]);
        msg.extend_from_slice(&16u16.to_be_bytes()); // TXT
        msg.extend_from_slice(&1u16.to_be_bytes()); // IN
        msg.extend_from_slice(&60u32.to_be_bytes()); // ttl
        let rdata: [u8; 6] = [5, b'h', b'e', b'l', b'l', b'o'];
        msg.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        msg.extend_from_slice(&rdata);
        let detail = parse_dns(&msg).expect("dns");
        assert_eq!(detail.txt_len, Some(5));
    }

    #[test]
    fn entropy_of_random_label_is_high() {
        let random = parse_dns(&dns_query("a8f3k2m9x7qwe4rt.example.com", 1)).expect("dns");
        let normal = parse_dns(&dns_query("www.example.com", 1)).expect("dns");
        assert!(random.entropy.unwrap_or(0.0) > normal.entropy.unwrap_or(0.0));
    }

    #[test]
    fn http_request_metadata() {
        let payload = b"GET /index.html HTTP/1.1\r\nHost: Example.COM\r\nUser-Agent: curl/8\r\nConnection: keep-alive\r\n\r\n";
        let detail = parse_http(payload).expect("http");
        assert!(detail.is_request);
        assert_eq!(detail.method.as_deref(), Some("GET"));
        assert_eq!(detail.host.as_deref(), Some("example.com"));
        assert_eq!(detail.user_agent.as_deref(), Some("curl/8"));
        assert_eq!(detail.connection.as_deref(), Some("keep-alive"));
    }

    #[test]
    fn http_response_metadata() {
        let payload = b"HTTP/1.1 404 Not Found\r\nContent-Length: 12\r\n\r\n";
        let detail = parse_http(payload).expect("http");
        assert!(!detail.is_request);
        assert_eq!(detail.status_code, Some(404));
        assert_eq!(detail.content_length, Some(12));
    }

    #[test]
    fn http_ignores_non_http_payload() {
        assert!(parse_http(b"\x16\x03\x01\x00\x28").is_none());
    }

    #[test]
    fn tls_client_hello_sni() {
        // hand-built ClientHello with a single server_name extension
        let host = b"api.example.com";
        let mut sni_ext = Vec::new();
        sni_ext.extend_from_slice(&0u16.to_be_bytes());
        sni_ext.extend_from_slice(&((host.len() + 5) as u16).to_be_bytes());
        sni_ext.extend_from_slice(&((host.len() + 3) as u16).to_be_bytes());
        sni_ext.push(0);
        sni_ext.extend_from_slice(&(host.len() as u16).to_be_bytes());
        sni_ext.extend_from_slice(host);

        let mut handshake = Vec::new();
        handshake.push(0x01); // ClientHello
        let body = {
            let mut body = Vec::new();
            body.extend_from_slice(&0x0303u16.to_be_bytes());
            body.extend_from_slice(&[0u8; 32]); // random
            body.push(0); // session id
            body.extend_from_slice(&2u16.to_be_bytes());
            body.extend_from_slice(&[0x13, 0x01]); // cipher suites
            body.push(1);
            body.push(0); // compression
            body.extend_from_slice(&(sni_ext.len() as u16).to_be_bytes());
            body.extend_from_slice(&sni_ext);
            body
        };
        handshake.push(((body.len() >> 16) & 0xFF) as u8);
        handshake.push(((body.len() >> 8) & 0xFF) as u8);
        handshake.push((body.len() & 0xFF) as u8);
        handshake.extend_from_slice(&body);

        let mut record = Vec::new();
        record.push(0x16);
        record.extend_from_slice(&0x0301u16.to_be_bytes());
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);

        let detail = parse_tls(&record).expect("tls");
        assert!(detail.is_client_hello);
        assert_eq!(detail.sni.as_deref(), Some("api.example.com"));
    }

    #[test]
    fn tls_without_client_hello_has_no_sni() {
        let record = [0x17u8, 0x03, 0x03, 0x00, 0x04, 1, 2, 3, 4];
        let detail = parse_tls(&record).expect("tls");
        assert!(!detail.is_client_hello);
        assert!(detail.sni.is_none());
    }

    #[test]
    fn dhcp_discover_metadata() {
        let mut payload = vec![0u8; 236];
        payload[0] = 1; // request
        payload.extend_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        payload.extend_from_slice(&[53, 1, 1]); // DHCPDISCOVER
        payload.extend_from_slice(&[50, 4, 192, 168, 1, 50]);
        payload.extend_from_slice(&[61, 4, b'a', b'b', b'c', b'd']);
        payload.push(255);
        let detail = parse_dhcp(&payload).expect("dhcp");
        assert_eq!(detail.message_type, 1);
        assert_eq!(detail.dhcp_type, Some(1));
        assert_eq!(detail.requested_ip.as_deref(), Some("192.168.1.50"));
        assert_eq!(detail.client_id.as_deref(), Some("abcd"));
    }

    #[test]
    fn detection_respects_options() {
        let msg = dns_query("example.com", 1);
        let transport = TransportInfo {
            src_port: Some(40000),
            dst_port: Some(53),
            flags: Vec::new(),
            seq: None,
            ack: None,
            window: None,
            udp_len: Some(msg.len() as u16),
            icmp: None,
        };
        let network = NetworkInfo {
            src: "10.0.0.1".to_owned(),
            dst: "10.0.0.2".to_owned(),
            protocol: packetsage_protocol::NetProto::Udp,
            ip_version: 4,
            ttl: Some(64),
            hop_limit: None,
            is_fragment: false,
        };
        assert!(detect(&DecodeOptions::default(), &transport, &msg, Some(&network)).is_some());
        let opts = DecodeOptions {
            app_protocols: Default::default(),
        };
        assert!(detect(&opts, &transport, &msg, Some(&network)).is_none());
    }
}
