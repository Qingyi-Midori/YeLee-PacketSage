#!/usr/bin/env python3
"""Synthetic capture generator (M3~M6 §6.2, ADR-022).

Pure standard library: it writes PCAP and PCAPNG files itself so the benchmark
dataset is reproducible without scapy. The generator is parameterised over the
four format dimensions required by the spec:

* timestamp precision (legacy microsecond / nanosecond magic, PCAPNG binary
  2^-n resolution);
* interface count (PCAPNG EPB with several interfaces);
* VLAN tagging (802.1Q, including QinQ on the outermost frame);
* container format (pcap / pcapng).

Usage:
    python scripts/gen_traffic.py --out samples/synth-10mb.pcapng \\
        --format pcapng --packets 100000 --flows 200 --profile mixed \\
        --retransmit 0.02 --reorder 0.01 --interfaces 2 --vlan 100
"""

from __future__ import annotations

import argparse
import random
import struct
from pathlib import Path

ETHERTYPE_IPV4 = 0x0800
ETHERTYPE_VLAN = 0x8100
LINKTYPE_ETHERNET = 1
PCAP_MAGIC_US = 0xA1B2C3D4
PCAP_MAGIC_NS = 0xA1B23C4D
PCAPNG_SHB = 0x0A0D0D0A
PCAPNG_IDB = 0x00000001
PCAPNG_EPB = 0x00000006


def checksum(data: bytes) -> int:
    """Internet checksum."""
    if len(data) % 2:
        data += b"\x00"
    total = 0
    for index in range(0, len(data), 2):
        total += (data[index] << 8) + data[index + 1]
    while total >> 16:
        total = (total & 0xFFFF) + (total >> 16)
    return (~total) & 0xFFFF


def mac(seed: int) -> bytes:
    """Deterministic MAC address."""
    return bytes([0x02, 0x00, (seed >> 24) & 0xFF, (seed >> 16) & 0xFF, (seed >> 8) & 0xFF, seed & 0xFF])


def ipv4_header(src: str, dst: str, protocol: int, payload_len: int, ident: int = 0) -> bytes:
    """20 byte IPv4 header."""
    total_len = 20 + payload_len
    header = struct.pack(
        "!BBHHHBBH4s4s",
        0x45,
        0,
        total_len,
        ident & 0xFFFF,
        0x4000,
        64,
        protocol,
        0,
        bytes(int(part) for part in src.split(".")),
        bytes(int(part) for part in dst.split(".")),
    )
    # checksum field is the 11th/12th byte
    csum = checksum(header)
    return header[:10] + struct.pack("!H", csum) + header[12:]


def tcp_segment(
    sport: int,
    dport: int,
    seq: int,
    ack: int,
    flags: int,
    payload: bytes,
) -> bytes:
    """TCP header plus payload."""
    window = 8192
    header = struct.pack(
        "!HHIIBBHHH",
        sport,
        dport,
        seq & 0xFFFFFFFF,
        ack & 0xFFFFFFFF,
        (5 << 4),
        flags,
        window,
        0,
        0,
    )
    return header + payload


def udp_segment(sport: int, dport: int, payload: bytes) -> bytes:
    """UDP header plus payload."""
    return struct.pack("!HHHH", sport, dport, 8 + len(payload), 0) + payload


def dns_query(qname: str, txid: int, high_entropy: bool) -> bytes:
    """DNS query packet payload."""
    body = struct.pack("!HHHHHH", txid, 0x0100, 1, 0, 0, 0)
    for label in qname.split("."):
        body += bytes([len(label)]) + label.encode()
    body += b"\x00"
    body += struct.pack("!HH", 1 if not high_entropy else 16, 1)
    return body


def dns_response(txid: int, qname: str, txt: bytes | None = None) -> bytes:
    """DNS response for `qname`, optionally with one TXT record."""
    body = struct.pack("!HHHHHH", txid, 0x8180, 1, 1, 0, 0)
    for label in qname.split("."):
        body += bytes([len(label)]) + label.encode()
    body += b"\x00"
    body += struct.pack("!HH", 1, 1)
    # answer: name pointer to the question, then the record
    if txt is None:
        body += b"\xc0\x0c" + struct.pack("!HHIH", 1, 1, 60, 4)
        body += bytes([93, 184, 216, 34])
    else:
        rdata = bytes([len(txt)]) + txt
        body += b"\xc0\x0c" + struct.pack("!HHIH", 16, 1, 60, len(rdata))
        body += rdata
    return body


def random_label(rng: random.Random, length: int = 40) -> str:
    """High entropy label for the DNS tunnel profile."""
    alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
    return "".join(rng.choice(alphabet) for _ in range(length))


def ethernet_frame(
    src_mac: bytes,
    dst_mac: bytes,
    payload: bytes,
    vlan: int | None,
    qinq: bool = False,
) -> bytes:
    """Ethernet II frame with optional 802.1Q / QinQ tagging."""
    header = dst_mac + src_mac
    if vlan is None:
        header += struct.pack("!H", ETHERTYPE_IPV4)
    elif qinq:
        header += struct.pack("!HH", ETHERTYPE_VLAN, vlan & 0x0FFF)
        header += struct.pack("!HH", ETHERTYPE_VLAN, (vlan + 1) & 0x0FFF)
        header += struct.pack("!H", ETHERTYPE_IPV4)
    else:
        header += struct.pack("!HH", ETHERTYPE_VLAN, vlan & 0x0FFF)
        header += struct.pack("!H", ETHERTYPE_IPV4)
    return header + payload


class PacketFactory:
    """Builds the synthetic traffic mix."""

    def __init__(self, profile: str, rng: random.Random, vlan: int | None, qinq: bool) -> None:
        self.profile = profile
        self.rng = rng
        self.vlan = vlan
        self.qinq = qinq
        self.client_mac = mac(1)
        self.server_mac = mac(2)

    def http_flow(self, index: int) -> list[bytes]:
        """A full TCP handshake with a request/response pair.

        Sequence arithmetic and the direction of every frame are exact: the SYN
        consumes one sequence number, the first payload byte is `isn + 1`, and
        server frames carry the server's MAC, IP and ports (getting this wrong
        creates phantom gaps, see tests/flow_directions.rs).
        """
        client = f"10.0.0.{10 + index % 200}"
        server = f"10.0.1.{10 + index % 50}"
        cport = 40000 + (index % 2000)
        request = b"GET /index.html HTTP/1.1\r\nHost: example.com\r\nUser-Agent: gen/1\r\n\r\n"
        response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"
        isn_c, isn_s = 1000, 5000
        c_data = isn_c + 1
        s_data = isn_s + 1
        c_end = c_data + len(request)
        s_end = s_data + len(response)
        return [
            self.tcp_frame(client, server, cport, 80, True, isn_c, 0, 0x02),
            self.tcp_frame(client, server, cport, 80, False, isn_s, isn_c + 1, 0x12),
            self.tcp_frame(client, server, cport, 80, True, c_data, s_data, 0x10),
            self.tcp_frame(client, server, cport, 80, True, c_data, s_data, 0x18, request),
            self.tcp_frame(client, server, cport, 80, False, s_data, c_end, 0x18, response),
            self.tcp_frame(client, server, cport, 80, True, c_end, s_end, 0x11),
            self.tcp_frame(client, server, cport, 80, False, s_end, c_end + 1, 0x11),
        ]

    def tcp_frame(
        self,
        client: str,
        server: str,
        client_port: int,
        server_port: int,
        from_client: bool,
        seq: int,
        ack: int,
        flags: int,
        payload: bytes = b"",
    ) -> bytes:
        """One TCP frame with MAC, IP and ports all following `from_client`."""
        if from_client:
            src, dst = client, server
            sport, dport = client_port, server_port
            src_mac, dst_mac = self.client_mac, self.server_mac
        else:
            src, dst = server, client
            sport, dport = server_port, client_port
            src_mac, dst_mac = self.server_mac, self.client_mac
        segment = tcp_segment(sport, dport, seq, ack, flags, payload)
        body = ipv4_header(src, dst, 6, len(segment))
        return ethernet_frame(src_mac, dst_mac, body + segment, self.vlan, self.qinq)

    def dns_flow(self, index: int, tunnel: bool) -> list[bytes]:
        """DNS query + response (query optional high entropy, for tunnels)."""
        # A tunnel is one client; keep the source stable so the src_ip window
        # grouping in NET-DNS-SUSPICIOUS-001 can actually accumulate.
        client = "10.0.0.10" if tunnel else f"10.0.0.{10 + index % 5}"
        if tunnel:
            qname = f"{random_label(self.rng)}.tunnel.example.com"
        else:
            qname = f"www{index % 20}.example.com"
        cport = 40000 + index % 2000
        txid = index & 0xFFFF
        frames = []
        query = dns_query(qname, txid, tunnel)
        frames.append(self.udp_frame(client, "8.8.8.8", cport, 53, query, True))
        txt = random_label(self.rng, 120).encode() if tunnel else None
        response = dns_response(txid, qname, txt)
        frames.append(self.udp_frame(client, "8.8.8.8", cport, 53, response, False))
        return frames

    def udp_frame(
        self,
        client: str,
        server: str,
        client_port: int,
        server_port: int,
        payload: bytes,
        from_client: bool,
    ) -> bytes:
        """One UDP frame with MAC, IP and ports all following `from_client`."""
        if from_client:
            src, dst = client, server
            sport, dport = client_port, server_port
            src_mac, dst_mac = self.client_mac, self.server_mac
        else:
            src, dst = server, client
            sport, dport = server_port, client_port
            src_mac, dst_mac = self.server_mac, self.client_mac
        datagram = udp_segment(sport, dport, payload)
        body = ipv4_header(src, dst, 17, len(datagram))
        return ethernet_frame(src_mac, dst_mac, body + datagram, self.vlan, self.qinq)

    def syn_flood(self, index: int) -> list[bytes]:
        """Many bare SYNs from one source (NET-TCP-SYN-BURST profile)."""
        client = "10.9.9.9"
        server = "10.0.2.20"
        segment = tcp_segment(40000 + (index % 5000), 80 + (index % 20), 1000 + index, 0, 0x02, b"")
        body = ipv4_header(client, server, 6, len(segment))
        return [
            ethernet_frame(
                self.client_mac, self.server_mac, body + segment, self.vlan, self.qinq
            )
        ]

    def port_sweep(self, index: int) -> list[bytes]:
        """Sequential SYNs to many ports (NET-TCP-PORT-SWEEP profile)."""
        client = "10.8.8.8"
        server = "10.0.2.30"
        segment = tcp_segment(50000 + index % 1000, 1 + (index % 1000), 1, 0, 0x02, b"")
        body = ipv4_header(client, server, 6, len(segment))
        return [
            ethernet_frame(
                self.client_mac, self.server_mac, body + segment, self.vlan, self.qinq
            )
        ]

    def malformed(self) -> list[bytes]:
        """A frame whose IPv4 header is cut short."""
        body = b"\x45\x00\x00\x14\x00\x00"
        return [ethernet_frame(self.client_mac, self.server_mac, body, self.vlan, self.qinq)]


def build_packets(args: argparse.Namespace) -> list[bytes]:
    """Generates the packet list according to the profile."""
    rng = random.Random(args.seed)
    factory = PacketFactory(args.profile, rng, args.vlan, args.qinq)
    frames: list[bytes] = []
    flow_index = 0
    while len(frames) < args.packets:
        if args.profile in ("mixed", "http"):
            frames.extend(factory.http_flow(flow_index))
        if args.profile in ("mixed", "dns", "dns-tunnel"):
            frames.extend(factory.dns_flow(flow_index, args.profile == "dns-tunnel"))
        if args.profile in ("mixed", "syn-flood"):
            if args.profile == "mixed":
                # a real burst so the SYN rule can accumulate inside one window
                for offset in range(120):
                    frames.extend(factory.syn_flood(flow_index * 1000 + offset))
            else:
                frames.extend(factory.syn_flood(flow_index))
        if args.profile in ("mixed", "port-sweep"):
            if args.profile == "mixed" and flow_index % 3 == 0:
                for offset in range(60):
                    frames.extend(factory.port_sweep(flow_index * 100 + offset))
            else:
                frames.extend(factory.port_sweep(flow_index))
        if args.profile == "malformed":
            frames.extend(factory.malformed())
        flow_index += 1
    frames = frames[: args.packets]
    if args.retransmit > 0:
        duplicated = []
        for index, frame in enumerate(frames):
            duplicated.append(frame)
            if rng.random() < args.retransmit:
                duplicated.append(frame)
        frames = duplicated
    if args.reorder > 0:
        shuffled = list(frames)
        for index in range(len(shuffled) - 1):
            if rng.random() < args.reorder:
                shuffled[index], shuffled[index + 1] = shuffled[index + 1], shuffled[index]
        frames = shuffled
    return frames


def write_pcap(path: Path, frames: list[bytes], args: argparse.Namespace) -> None:
    """Writes a legacy PCAP file."""
    magic = PCAP_MAGIC_NS if args.ts_precision == "ns" else PCAP_MAGIC_US
    scale = 1_000_000_000 if args.ts_precision == "ns" else 1_000_000
    with path.open("wb") as handle:
        handle.write(
            struct.pack("<IHHiIII", magic, 2, 4, 0, 0, args.snaplen, LINKTYPE_ETHERNET)
        )
        timestamp = 1_700_000_000 * scale
        for frame in frames:
            timestamp += args.frame_gap_ns * scale // 1_000_000_000
            seconds, fraction = divmod(timestamp, scale)
            handle.write(struct.pack("<IIII", seconds, fraction, len(frame), len(frame)))
            handle.write(frame)


def pcapng_block(block_type: int, body: bytes, endian: str = "<") -> bytes:
    """Wraps a body in the PCAPNG block frame."""
    total = 12 + len(body)
    padding = (-len(body)) % 4
    total += padding
    return (
        struct.pack(f"{endian}II", block_type, total)
        + body
        + b"\x00" * padding
        + struct.pack(f"{endian}I", total)
    )


def write_pcapng(path: Path, frames: list[bytes], args: argparse.Namespace) -> None:
    """Writes a PCAPNG file with `args.interfaces` interfaces and tsresol."""
    with path.open("wb") as handle:
        shb_body = struct.pack("<IHHq", 0x1A2B3C4D, 1, 0, -1)
        handle.write(pcapng_block(PCAPNG_SHB, shb_body))
        for _ in range(max(1, args.interfaces)):
            # if_tsresol = 6 (µs) or 0x80 | 10 (binary 2^-10)
            tsresol = 0x80 | args.binary_exp if args.ts_precision == "binary" else 6
            idb_body = struct.pack("<HHI", LINKTYPE_ETHERNET, 0, args.snaplen)
            idb_body += struct.pack("<HH", 9, 1) + bytes([tsresol]) + b"\x00\x00\x00"
            idb_body += struct.pack("<HH", 0, 0)
            handle.write(pcapng_block(PCAPNG_IDB, idb_body))
        resolution = (1 << args.binary_exp) if args.ts_precision == "binary" else 1_000_000
        ticks = 1_700_000_000 * resolution
        ticks_per_frame = max(1, args.frame_gap_ns * resolution // 1_000_000_000)
        for index, frame in enumerate(frames):
            ticks += ticks_per_frame
            interface_id = index % max(1, args.interfaces)
            padded = frame + b"\x00" * ((-len(frame)) % 4)
            body = struct.pack(
                "<IIIII",
                interface_id,
                (ticks >> 32) & 0xFFFFFFFF,
                ticks & 0xFFFFFFFF,
                len(frame),
                len(frame),
            )
            body += padded + struct.pack("<HH", 0, 0)
            handle.write(pcapng_block(PCAPNG_EPB, body))


def main() -> int:
    """Entry point."""
    parser = argparse.ArgumentParser(description="Generate synthetic capture files")
    parser.add_argument("--out", required=True)
    parser.add_argument("--format", choices=["pcap", "pcapng"], default="pcap")
    parser.add_argument("--packets", type=int, default=10_000)
    parser.add_argument("--flows", type=int, default=20, help="accepted for compatibility")
    parser.add_argument(
        "--profile",
        choices=["mixed", "http", "dns", "dns-tunnel", "syn-flood", "port-sweep", "malformed"],
        default="mixed",
    )
    parser.add_argument("--retransmit", type=float, default=0.0)
    parser.add_argument("--reorder", type=float, default=0.0)
    parser.add_argument("--interfaces", type=int, default=1)
    parser.add_argument("--vlan", type=int, default=None)
    parser.add_argument("--qinq", action="store_true")
    parser.add_argument("--ts-precision", choices=["us", "ns", "binary"], default="us")
    parser.add_argument("--binary-exp", type=int, default=10)
    parser.add_argument("--snaplen", type=int, default=262_144)
    parser.add_argument("--frame-gap-ns", type=int, default=100_000)
    parser.add_argument("--seed", type=int, default=1234)
    args = parser.parse_args()
    del args.flows  # kept for CLI compatibility with the spec text

    path = Path(args.out)
    path.parent.mkdir(parents=True, exist_ok=True)
    frames = build_packets(args)
    if args.format == "pcap":
        write_pcap(path, frames, args)
    else:
        write_pcapng(path, frames, args)
    size = path.stat().st_size
    print(f"{path}: {len(frames)} frames, {size} bytes ({size / 1024:.1f} KiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
