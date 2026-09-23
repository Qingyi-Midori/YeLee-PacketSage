//! PCAP / PCAPNG input layer (M0~M2 §4.3).
//!
//! # Deviation D-5 (recorded in `docs/architecture.md`)
//!
//! The spec sketches a `PacketSource` trait with `advance()` + `current()`
//! returning a borrowed item. `pcap-parser` hands out blocks that borrow the
//! reader's internal ring buffer, so a `current()` accessor would require a
//! self-referential struct. This module therefore exposes a push-style
//! [`CaptureReader::drive`] loop: it keeps the zero-copy property (the callback
//! receives borrowed record bytes) without unsafe code, which the workspace
//! forbids.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use packetsage_protocol::TsPrecision;
use pcap_parser::traits::{PcapNGPacketBlock, PcapReaderIterator};
use pcap_parser::{
    Block, LegacyPcapBlock, LegacyPcapReader, Linktype, PcapBlockOwned, PcapError, PcapHeader,
    PcapNGReader,
};

use crate::error::{PacketSageError, Result};

/// Capture container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    /// Classic `pcap` savefile.
    Pcap,
    /// `pcapng` block format.
    PcapNg,
}

impl CaptureFormat {
    /// Lowercase name used in events.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureFormat::Pcap => "pcap",
            CaptureFormat::PcapNg => "pcapng",
        }
    }
}

/// Reads the first 16 bytes only and decides the container format.
///
/// # Errors
/// Returns [`PacketSageError::UnsupportedCapture`] for anything that is
/// neither `0xA1B2C3D4` family nor the PCAPNG SHB magic.
pub fn detect_format(header: &[u8; 16], path: &Path) -> Result<CaptureFormat> {
    let magic = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    match magic {
        0xA1B2_C3D4 | 0xD4C3_B2A1 | 0xA1B2_3C4D | 0x4D3C_B2A1 | 0xA1B2_CD34 | 0x34CD_B2A1 => {
            Ok(CaptureFormat::Pcap)
        }
        0x0A0D_0D0A => Ok(CaptureFormat::PcapNg),
        _ => Err(PacketSageError::UnsupportedCapture {
            path: path.to_path_buf(),
            reason: format!("unknown capture magic 0x{magic:08X}"),
        }),
    }
}

/// Timestamp resolution of a capture interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsResolution {
    /// `ticks` units per second (PCAPNG `if_tsresol`, or legacy magic).
    PerSecond(u64),
}

impl TsResolution {
    /// Nanoseconds for one tick denominator.
    #[must_use]
    pub fn ticks_per_second(self) -> u64 {
        match self {
            TsResolution::PerSecond(n) => n.max(1),
        }
    }

    /// Precision metadata derived from the resolution.
    #[must_use]
    pub fn precision(self) -> TsPrecision {
        match self.ticks_per_second() {
            n if n >= 1_000_000_000 => TsPrecision::Ns,
            n if n >= 1_000_000 => TsPrecision::Us,
            n if n >= 1_000 => TsPrecision::Ms,
            _ => TsPrecision::S,
        }
    }
}

/// Converts `seconds + fractional ticks` into canonical nanoseconds.
///
/// The fractional part is scaled with integer arithmetic only, so the result
/// is deterministic across platforms.
#[must_use]
#[allow(clippy::integer_division)] // nanosecond scaling: integer maths on purpose
pub fn ticks_to_unix_ns(seconds: u64, fraction: u64, ticks_per_second: u64) -> i128 {
    let ticks = ticks_per_second.max(1);
    let secs = i128::from(seconds);
    let frac_ns = (i128::from(fraction) * 1_000_000_000i128) / i128::from(ticks);
    secs * 1_000_000_000i128 + frac_ns
}

/// One interface of a capture file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceState {
    /// Interface id, file order.
    pub interface_id: u32,
    /// Link type.
    pub linktype: u32,
    /// Snap length when declared.
    pub snaplen: Option<u32>,
    /// Timestamp resolution.
    pub ts_resol: TsResolution,
    /// PCAPNG `if_tsoffset` (seconds); ignored unless present.
    pub ts_offset: i64,
    /// Last seen timestamp, used as the base for timestamp-less SPB packets.
    pub last_ts_ns: i128,
}

/// Non-fatal reader observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonFatalKind {
    /// A block type outside the supported set was skipped.
    UnknownBlockSkipped,
    /// A known but non-packet block was skipped.
    NonPacketBlockSkipped,
    /// A truncated tail was ignored.
    TruncatedTail,
}

/// Non-fatal reader event payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonFatalInfo {
    /// What happened.
    pub kind: NonFatalKind,
    /// Byte offset where it happened.
    pub offset: u64,
    /// Human readable detail.
    pub message: String,
}

/// One raw packet record, borrowed from the reader buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPacketRecord<'a> {
    /// Zero-based packet index over the whole file.
    pub packet_index: u64,
    /// Interface id.
    pub interface_id: u32,
    /// Link type.
    pub linktype: u32,
    /// Canonical timestamp in nanoseconds.
    pub ts_ns: i128,
    /// Original timestamp precision.
    pub ts_precision: TsPrecision,
    /// Captured length.
    pub caplen: u32,
    /// Original length.
    pub origlen: u32,
    /// True when `caplen < origlen`.
    pub truncated: bool,
    /// Raw link-layer bytes.
    pub data: &'a [u8],
}

/// Item produced by [`CaptureReader::drive`].
#[derive(Debug, Clone)]
pub enum SourceItem<'a> {
    /// A packet record.
    Packet(RawPacketRecord<'a>),
    /// A non-fatal observation.
    NonFatal(NonFatalInfo),
}

/// Summary returned by [`CaptureReader::drive`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaderSummary {
    /// Packets delivered.
    pub packets: u64,
    /// Non-fatal observations.
    pub nonfatal: u64,
    /// Bytes captured across all packets.
    pub bytes: u64,
    /// True when the file ended with a truncated record.
    pub truncated_tail: bool,
}

/// Streaming reader over a PCAP / PCAPNG file.
pub struct CaptureReader {
    inner: Box<dyn PcapReaderIterator + Send>,
    format: CaptureFormat,
    interfaces: Vec<InterfaceState>,
    packet_index: u64,
    nonfatal: u64,
    consumed_bytes: u64,
    capacity: usize,
}

impl std::fmt::Debug for CaptureReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureReader")
            .field("format", &self.format)
            .field("interfaces", &self.interfaces)
            .field("packet_index", &self.packet_index)
            .finish()
    }
}

impl CaptureReader {
    /// Opens a capture file and probes its format.
    ///
    /// # Errors
    /// Returns [`PacketSageError`] when the file cannot be read or is not a
    /// supported capture.
    pub fn open(path: &Path, capacity: usize) -> Result<Self> {
        let mut probe = [0u8; 16];
        {
            let mut file = File::open(path)?;
            let read = file.read(&mut probe)?;
            if read < 16 {
                return Err(PacketSageError::UnsupportedCapture {
                    path: path.to_path_buf(),
                    reason: "file shorter than a capture header".to_owned(),
                });
            }
        }
        let format = detect_format(&probe, path)?;
        let file = File::open(path)?;
        let inner: Box<dyn PcapReaderIterator + Send> = match format {
            CaptureFormat::PcapNg => {
                Box::new(PcapNGReader::new(capacity.max(65_536), file).map_err(|e| {
                    PacketSageError::UnsupportedCapture {
                        path: path.to_path_buf(),
                        reason: format!("pcapng header: {e}"),
                    }
                })?)
            }
            CaptureFormat::Pcap => Box::new(
                LegacyPcapReader::new(capacity.max(65_536), file).map_err(|e| {
                    PacketSageError::UnsupportedCapture {
                        path: path.to_path_buf(),
                        reason: format!("pcap header: {e}"),
                    }
                })?,
            ),
        };
        Ok(Self {
            inner,
            format,
            interfaces: Vec::new(),
            packet_index: 0,
            nonfatal: 0,
            consumed_bytes: 0,
            capacity: capacity.max(65_536),
        })
    }

    /// Container format detected at open time.
    #[must_use]
    pub fn format(&self) -> CaptureFormat {
        self.format
    }

    /// Interfaces seen so far, in declaration order.
    #[must_use]
    pub fn interfaces(&self) -> &[InterfaceState] {
        &self.interfaces
    }

    /// Number of packets delivered so far.
    #[must_use]
    pub fn packet_index(&self) -> u64 {
        self.packet_index
    }

    /// Drives the reader, invoking `on_item` for every record.
    ///
    /// The callback also receives the interface table as it is known *at that
    /// point in the file*, which is what lets the pipeline emit `CaptureInfo`
    /// before the first packet event.
    ///
    /// # Errors
    /// Returns [`PacketSageError::CaptureCorrupted`] when the file structure is
    /// damaged beyond recovery, or when the callback fails.
    pub fn drive<F>(&mut self, mut on_item: F) -> Result<ReaderSummary>
    where
        F: FnMut(SourceItem<'_>, &[InterfaceState]) -> Result<()>,
    {
        let mut summary = ReaderSummary::default();
        loop {
            let consumed_now = self.consumed_bytes;
            let offset = match self.inner.next() {
                Ok((offset, block)) => {
                    let item = Self::handle_block(
                        &mut self.interfaces,
                        &mut self.packet_index,
                        &mut self.nonfatal,
                        consumed_now,
                        &block,
                        &mut summary,
                    )?;
                    if let Some(item) = item {
                        on_item(item, &self.interfaces)?;
                    }
                    offset
                }
                Err(PcapError::Eof) => break,
                Err(PcapError::UnexpectedEof) => {
                    self.nonfatal += 1;
                    summary.truncated_tail = true;
                    summary.nonfatal = self.nonfatal;
                    on_item(
                        SourceItem::NonFatal(NonFatalInfo {
                            kind: NonFatalKind::TruncatedTail,
                            offset: consumed_now,
                            message: "capture ends with a truncated record".to_owned(),
                        }),
                        &self.interfaces,
                    )?;
                    break;
                }
                Err(PcapError::Incomplete(_)) => {
                    if self.inner.reader_exhausted() {
                        self.nonfatal += 1;
                        summary.truncated_tail = true;
                        summary.nonfatal = self.nonfatal;
                        on_item(
                            SourceItem::NonFatal(NonFatalInfo {
                                kind: NonFatalKind::TruncatedTail,
                                offset: consumed_now,
                                message: "capture ends with a truncated record".to_owned(),
                            }),
                            &self.interfaces,
                        )?;
                        break;
                    }
                    self.refill(consumed_now)?;
                    continue;
                }
                Err(PcapError::BufferTooSmall) => {
                    self.capacity = self.capacity.saturating_mul(2);
                    let capacity = self.capacity;
                    let grew = self.inner.grow(capacity);
                    if !grew {
                        return Err(PacketSageError::CaptureCorrupted {
                            offset: consumed_now,
                            reason: "record larger than the reader buffer".to_owned(),
                        });
                    }
                    self.refill(consumed_now)?;
                    continue;
                }
                Err(PcapError::HeaderNotRecognized) => {
                    return Err(PacketSageError::UnsupportedCapture {
                        path: PathBuf::from("<stream>"),
                        reason: "header not recognized".to_owned(),
                    })
                }
                Err(e) => {
                    return Err(PacketSageError::CaptureCorrupted {
                        offset: consumed_now,
                        reason: format!("{e}"),
                    })
                }
            };
            self.inner.consume(offset);
            self.consumed_bytes += offset as u64;
        }
        Ok(summary)
    }

    fn refill(&mut self, offset: u64) -> Result<()> {
        match self.inner.refill() {
            Ok(()) => Ok(()),
            Err(PcapError::Eof) => Ok(()),
            Err(PcapError::ReadError) => Err(PacketSageError::Io(format!(
                "read failed while refilling capture buffer at byte {offset}"
            ))),
            Err(e) => Err(PacketSageError::CaptureCorrupted {
                offset,
                reason: format!("{e}"),
            }),
        }
    }

    fn handle_block<'a>(
        interfaces: &mut Vec<InterfaceState>,
        packet_index: &mut u64,
        nonfatal: &mut u64,
        consumed_bytes: u64,
        block: &'a PcapBlockOwned<'a>,
        summary: &mut ReaderSummary,
    ) -> Result<Option<SourceItem<'a>>> {
        match block {
            PcapBlockOwned::LegacyHeader(header) => {
                register_legacy_header(interfaces, header);
                Ok(None)
            }
            PcapBlockOwned::Legacy(packet) => {
                if let Some(record) = legacy_record(interfaces, packet_index, packet) {
                    summary.packets += 1;
                    summary.bytes += u64::from(record.caplen);
                    Ok(Some(SourceItem::Packet(record)))
                } else {
                    Ok(None)
                }
            }
            PcapBlockOwned::NG(ng) => handle_ng_block(
                interfaces,
                packet_index,
                nonfatal,
                consumed_bytes,
                ng,
                summary,
            ),
        }
    }
}

fn register_legacy_header(interfaces: &mut Vec<InterfaceState>, header: &PcapHeader) {
    let resolution = if header.is_nanosecond_precision() {
        TsResolution::PerSecond(1_000_000_000)
    } else {
        TsResolution::PerSecond(1_000_000)
    };
    interfaces.clear();
    interfaces.push(InterfaceState {
        interface_id: 0,
        linktype: header.network.0.max(0) as u32,
        snaplen: Some(header.snaplen),
        ts_resol: resolution,
        ts_offset: 0,
        last_ts_ns: 0,
    });
}

fn legacy_record<'a>(
    interfaces: &[InterfaceState],
    packet_index: &mut u64,
    packet: &'a LegacyPcapBlock<'a>,
) -> Option<RawPacketRecord<'a>> {
    let interface = interfaces.first()?;
    let ts_ns = ticks_to_unix_ns(
        u64::from(packet.ts_sec),
        u64::from(packet.ts_usec),
        interface.ts_resol.ticks_per_second(),
    );
    let index = *packet_index;
    *packet_index += 1;
    Some(RawPacketRecord {
        packet_index: index,
        interface_id: interface.interface_id,
        linktype: interface.linktype,
        ts_ns,
        ts_precision: interface.ts_resol.precision(),
        caplen: packet.caplen,
        origlen: packet.origlen,
        truncated: packet.caplen < packet.origlen,
        data: packet.data,
    })
}

fn handle_ng_block<'a>(
    interfaces: &mut Vec<InterfaceState>,
    packet_index: &mut u64,
    nonfatal: &mut u64,
    consumed_bytes: u64,
    block: &'a Block<'a>,
    summary: &mut ReaderSummary,
) -> Result<Option<SourceItem<'a>>> {
    match block {
        Block::SectionHeader(shb) => {
            tracing::debug!(big_endian = shb.big_endian(), "pcapng section header");
            interfaces.clear();
            Ok(None)
        }
        Block::InterfaceDescription(idb) => {
            let resolution = idb
                .ts_resolution()
                .map_or(TsResolution::PerSecond(1_000_000), TsResolution::PerSecond);
            let interface_id = interfaces.len() as u32;
            interfaces.push(InterfaceState {
                interface_id,
                linktype: idb.linktype.0.max(0) as u32,
                snaplen: Some(idb.snaplen),
                ts_resol: resolution,
                ts_offset: idb.ts_offset(),
                last_ts_ns: 0,
            });
            Ok(None)
        }
        Block::EnhancedPacket(epb) => {
            let Some(interface) = interfaces.iter().find(|i| i.interface_id == epb.if_id) else {
                *nonfatal += 1;
                summary.nonfatal = *nonfatal;
                return Ok(Some(SourceItem::NonFatal(NonFatalInfo {
                    kind: NonFatalKind::NonPacketBlockSkipped,
                    offset: consumed_bytes,
                    message: format!("packet references unknown interface {}", epb.if_id),
                })));
            };
            let ticks_per_second = interface.ts_resol.ticks_per_second();
            let ts_offset = u64::try_from(interface.ts_offset.max(0)).unwrap_or(0);
            let (secs, fraction) = epb.decode_ts(ts_offset, ticks_per_second);
            let ts_ns = ticks_to_unix_ns(u64::from(secs), u64::from(fraction), ticks_per_second);
            let index = *packet_index;
            *packet_index += 1;
            let record = RawPacketRecord {
                packet_index: index,
                interface_id: interface.interface_id,
                linktype: interface.linktype,
                ts_ns,
                ts_precision: interface.ts_resol.precision(),
                caplen: epb.caplen,
                origlen: epb.origlen,
                truncated: epb.caplen < epb.origlen,
                data: epb.packet_data(),
            };
            summary.packets += 1;
            summary.bytes += u64::from(record.caplen);
            if let Some(state) = interfaces.iter_mut().find(|i| i.interface_id == epb.if_id) {
                state.last_ts_ns = ts_ns;
            }
            Ok(Some(SourceItem::Packet(record)))
        }
        Block::SimplePacket(spb) => {
            let data = spb.packet_data();
            let Some(interface) = interfaces.first().cloned() else {
                *nonfatal += 1;
                summary.nonfatal = *nonfatal;
                return Ok(Some(SourceItem::NonFatal(NonFatalInfo {
                    kind: NonFatalKind::NonPacketBlockSkipped,
                    offset: consumed_bytes,
                    message: "SPB before any interface description".to_owned(),
                })));
            };
            let index = *packet_index;
            *packet_index += 1;
            let caplen = u32::try_from(data.len()).unwrap_or(u32::MAX);
            let record = RawPacketRecord {
                packet_index: index,
                interface_id: interface.interface_id,
                linktype: interface.linktype,
                ts_ns: interface.last_ts_ns,
                ts_precision: interface.ts_resol.precision(),
                caplen,
                origlen: spb.origlen,
                truncated: caplen < spb.origlen,
                data,
            };
            summary.packets += 1;
            summary.bytes += u64::from(caplen);
            Ok(Some(SourceItem::Packet(record)))
        }
        Block::NameResolution(_)
        | Block::InterfaceStatistics(_)
        | Block::SystemdJournalExport(_)
        | Block::DecryptionSecrets(_)
        | Block::ProcessInformation(_) => {
            *nonfatal += 1;
            summary.nonfatal = *nonfatal;
            tracing::debug!("skipping non-packet pcapng block");
            Ok(None)
        }
        Block::Custom(custom) => {
            *nonfatal += 1;
            summary.nonfatal = *nonfatal;
            Ok(Some(SourceItem::NonFatal(NonFatalInfo {
                kind: NonFatalKind::UnknownBlockSkipped,
                offset: consumed_bytes,
                message: format!("custom block 0x{:08X} skipped", custom.block_type),
            })))
        }
        Block::Unknown(unknown) => {
            *nonfatal += 1;
            summary.nonfatal = *nonfatal;
            Ok(Some(SourceItem::NonFatal(NonFatalInfo {
                kind: NonFatalKind::UnknownBlockSkipped,
                offset: consumed_bytes,
                message: format!("unknown block 0x{:08X} skipped", unknown.block_type),
            })))
        }
    }
}

/// The link type used by Ethernet captures.
pub const LINKTYPE_ETHERNET: u32 = 1;
/// The link type used by Linux cooked capture v1.
pub const LINKTYPE_LINUX_SLL: u32 = 113;

/// Human readable link type name.
#[must_use]
pub fn linktype_name(linktype: u32) -> &'static str {
    match linktype {
        LINKTYPE_ETHERNET => "ethernet",
        LINKTYPE_LINUX_SLL => "linux_sll",
        _ => "unsupported",
    }
}

/// Helper used by tests and tooling: resolve a `Linktype` value.
#[must_use]
pub fn linktype_of(linktype: Linktype) -> u32 {
    linktype.0.max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_format_accepts_all_legacy_magics() {
        let path = Path::new("x.pcap");
        for bytes in [
            [0xA1u8, 0xB2, 0xC3, 0xD4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0xD4, 0xC3, 0xB2, 0xA1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0xA1, 0xB2, 0x3C, 0x4D, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0x4D, 0x3C, 0xB2, 0xA1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ] {
            assert_eq!(detect_format(&bytes, path).ok(), Some(CaptureFormat::Pcap));
        }
    }

    #[test]
    fn detect_format_accepts_pcapng_shb() {
        let bytes = [
            0x0Au8, 0x0D, 0x0D, 0x0A, 0x00, 0x00, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(
            detect_format(&bytes, Path::new("x.pcapng")).ok(),
            Some(CaptureFormat::PcapNg)
        );
    }

    #[test]
    fn detect_format_rejects_garbage() {
        let bytes = [0x11u8; 16];
        let err = detect_format(&bytes, Path::new("junk.bin"));
        assert!(matches!(
            err,
            Err(PacketSageError::UnsupportedCapture { .. })
        ));
    }

    #[test]
    fn ts_conversion_decimal_and_binary() {
        // microseconds
        assert_eq!(
            ticks_to_unix_ns(1, 1_515_933, 1_000_000),
            1_000_000_000 + 1_515_933_000
        );
        // nanoseconds
        assert_eq!(
            ticks_to_unix_ns(0, 1_515_933_236, 1_000_000_000),
            1_515_933_236
        );
        // binary resolution 2^-10 s (ticks_per_second = 1024)
        assert_eq!(ticks_to_unix_ns(1, 512, 1024), 1_000_000_000 + 500_000_000);
        // seconds
        assert_eq!(ticks_to_unix_ns(5, 0, 1), 5_000_000_000);
    }

    #[test]
    fn resolution_precision_mapping() {
        assert_eq!(
            TsResolution::PerSecond(1_000_000_000).precision(),
            TsPrecision::Ns
        );
        assert_eq!(
            TsResolution::PerSecond(1_000_000).precision(),
            TsPrecision::Us
        );
        assert_eq!(TsResolution::PerSecond(1_000).precision(), TsPrecision::Ms);
        assert_eq!(TsResolution::PerSecond(1).precision(), TsPrecision::S);
    }
}
