#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    packetsage_fuzz::fuzz_decoder(data);
});
