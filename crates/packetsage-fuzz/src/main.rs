//! Stable fuzz smoke runner (M6 §6.1).
//!
//! `cargo run -p packetsage-fuzz --bin fuzz-smoke -- --seconds 60`
//!
//! `cargo fuzz` targets live in `fuzz/fuzz_targets/`; they need a nightly
//! toolchain, this runner does not.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use packetsage_fuzz::{fuzz_capture_bytes, fuzz_decoder, fuzz_rpc_jsonl, fuzz_rule_yaml, Rng};

fn main() {
    let mut seconds = 10u64;
    let mut seed = 0xC0FFEEu64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => {
                if let Some(value) = args.next().and_then(|v| v.parse().ok()) {
                    seconds = value;
                }
            }
            "--seed" => {
                if let Some(value) = args.next().and_then(|v| v.parse().ok()) {
                    seed = value;
                }
            }
            other => {
                eprintln!("fuzz-smoke: ignoring unknown argument {other:?}");
            }
        }
    }

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut rng = Rng::new(seed);
    let scratch = std::env::temp_dir().join("packetsage-fuzz-smoke.bin");
    let mut iterations = 0u64;
    let mut capture_iterations = 0u64;

    while Instant::now() < deadline {
        for _ in 0..200 {
            let len = (rng.next_u64() % 4_096) as usize;
            let mut buffer = vec![0u8; len];
            rng.fill(&mut buffer);
            fuzz_decoder(&buffer);
            fuzz_rule_yaml(&String::from_utf8_lossy(&buffer));
            fuzz_rpc_jsonl(&String::from_utf8_lossy(&buffer));
            iterations += 1;
        }
        // File level readers are slower (they touch the disk), so run fewer.
        let len = (rng.next_u64() % 8_192) as usize;
        let mut buffer = vec![0u8; len];
        rng.fill(&mut buffer);
        if len >= 16 {
            buffer[0..4].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
            buffer[16..20.min(len)].copy_from_slice(&[1, 0, 0, 0][..(20.min(len) - 16)]);
        }
        if fuzz_capture_bytes(&scratch, &buffer).is_err() {
            eprintln!("fuzz-smoke: scratch file is not writable");
            std::process::exit(2);
        }
        capture_iterations += 1;
    }

    let _ = std::fs::remove_file(&scratch);
    println!(
        "fuzz-smoke: {seconds}s, {iterations} decoder/yaml/rpc inputs, \
         {capture_iterations} capture inputs, zero panics (seed {seed:#x})"
    );
}
