#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Section Header Block with the byte order magic, then arbitrary blocks.
    let mut buffer = Vec::with_capacity(data.len() + 28);
    buffer.extend_from_slice(&0x0A0D_0D0Au32.to_le_bytes());
    buffer.extend_from_slice(&28u32.to_le_bytes());
    buffer.extend_from_slice(&0x1A2B_3C4Du32.to_le_bytes());
    buffer.extend_from_slice(&1u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&(-1i64).to_le_bytes());
    buffer.extend_from_slice(&28u32.to_le_bytes());
    buffer.extend_from_slice(data);
    let path = std::env::temp_dir().join("packetsage-fuzz-pcapng.bin");
    let _ = packetsage_fuzz::fuzz_capture_bytes(&path, &buffer);
});
