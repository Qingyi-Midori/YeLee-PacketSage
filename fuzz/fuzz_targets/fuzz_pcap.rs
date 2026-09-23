#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A legacy PCAP global header followed by arbitrary bytes: the reader must
    // either skip or stop, never panic.
    let mut buffer = Vec::with_capacity(data.len() + 24);
    buffer.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    buffer.extend_from_slice(&2u16.to_le_bytes());
    buffer.extend_from_slice(&4u16.to_le_bytes());
    buffer.extend_from_slice(&0i32.to_le_bytes());
    buffer.extend_from_slice(&0u32.to_le_bytes());
    buffer.extend_from_slice(&65_535u32.to_le_bytes());
    buffer.extend_from_slice(&1u32.to_le_bytes());
    buffer.extend_from_slice(data);
    let path = std::env::temp_dir().join("packetsage-fuzz-pcap.bin");
    let _ = packetsage_fuzz::fuzz_capture_bytes(&path, &buffer);
});
