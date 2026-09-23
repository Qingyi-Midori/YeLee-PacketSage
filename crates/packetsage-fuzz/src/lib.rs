//! Fuzz entry points for the packet parsing surface (M6 §6.1).
//!
//! Each function must return normally for *any* input: a panic is a bug. The
//! same functions are used by `fuzz-smoke` (stable, time boxed) and by the
//! `cargo fuzz` targets in `fuzz/fuzz_targets/`.

#![forbid(unsafe_code)]

use std::path::Path;

use packetsage_core::decoder::Decoder;
use packetsage_core::reader::{detect_format, CaptureReader, SourceItem};
use packetsage_protocol::RpcRequest;
use packetsage_rules::RuleFile;

/// A tiny deterministic PRNG so the harness has no dependencies.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeds the generator.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// Next 64 bit value (xorshift64*).
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Next byte.
    pub fn next_u8(&mut self) -> u8 {
        (self.next_u64() >> 24) as u8
    }

    /// Fills a buffer with random bytes.
    pub fn fill(&mut self, buffer: &mut [u8]) {
        for byte in buffer.iter_mut() {
            *byte = self.next_u8();
        }
    }
}

/// Fuzzes `detect_format` + the packet decoder.
pub fn fuzz_decoder(data: &[u8]) {
    let decoder = Decoder::all();
    if data.len() >= 16 {
        let mut header = [0u8; 16];
        header.copy_from_slice(&data[..16]);
        let _ = detect_format(&header, Path::new("<fuzz>"));
    }
    for linktype in [1u32, 113, 0, 147, u32::MAX] {
        let _ = decoder.decode(linktype, data);
    }
}

/// Fuzzes the capture readers by writing the bytes to a scratch file.
pub fn fuzz_capture_bytes(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)?;
    if let Ok(mut reader) = CaptureReader::open(path, 65_536) {
        let _ = reader.drive(|item, _interfaces| {
            if let SourceItem::Packet(record) = item {
                let _ = Decoder::all().decode(record.linktype, record.data);
            }
            Ok(())
        });
    }
    Ok(())
}

/// Fuzzes the YAML rule loader (schema + S1-S9 are pure functions).
pub fn fuzz_rule_yaml(text: &str) {
    if let Ok(rule) = RuleFile::parse(text) {
        let _ = rule.validate("FUZZ-000", &std::collections::BTreeSet::new());
    }
}

/// Fuzzes the JSONL RPC request parser.
pub fn fuzz_rpc_jsonl(line: &str) {
    let _: Result<RpcRequest, _> = serde_json::from_str(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_never_panic() {
        let mut rng = Rng::new(0x5EED);
        for _ in 0..2_000 {
            let len = (rng.next_u64() % 2_048) as usize;
            let mut buffer = vec![0u8; len];
            rng.fill(&mut buffer);
            fuzz_decoder(&buffer);
            fuzz_rule_yaml(&String::from_utf8_lossy(&buffer));
            fuzz_rpc_jsonl(&String::from_utf8_lossy(&buffer));
        }
    }

    #[test]
    fn truncated_capture_headers_never_panic() {
        let mut rng = Rng::new(7);
        let path = std::env::temp_dir().join("packetsage-fuzz-truncated.bin");
        for len in [0usize, 1, 15, 16, 23, 24, 25, 100, 4096] {
            let mut buffer = vec![0u8; len];
            rng.fill(&mut buffer);
            let _ = fuzz_capture_bytes(&path, &buffer);
        }
        let magic = [
            0xa1u8, 0xb2, 0xc3, 0xd4, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3,
        ];
        let _ = fuzz_capture_bytes(&path, &magic);
        let _ = std::fs::remove_file(&path);
    }
}
