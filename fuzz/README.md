# Fuzz targets (M6 §6.1)

Six targets back the panic-free guarantee:

| target | surface |
|---|---|
| `fuzz_pcap` | legacy PCAP reader |
| `fuzz_pcapng` | PCAPNG reader (sections, interfaces, unknown blocks) |
| `fuzz_decoder` | Ethernet/SLL/VLAN/IP/TCP/UDP/ICMP/application metadata |
| `fuzz_config_yaml` | `EngineConfig` YAML loading |
| `fuzz_rules_yaml` | rule DSL schema + S1–S9 |
| `fuzz_rpc_jsonl` | JSONL RPC request parsing |

`cargo fuzz` needs a nightly toolchain, so CI runs the stable equivalent first
and the nightly targets when a nightly toolchain is available:

```bash
# stable, no nightly required (this is what CI executes)
cargo run -p packetsage-fuzz --bin fuzz-smoke -- --seconds 60

# nightly, real coverage guided fuzzing
rustup toolchain install nightly
cargo install cargo-fuzz
cargo +nightly fuzz run fuzz_pcap -- -max_total_time=60
```

Both harnesses call the same code in `crates/packetsage-fuzz/src/lib.rs`, so a
crash found by one reproduces in the other.
