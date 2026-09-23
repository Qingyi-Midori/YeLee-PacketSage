# Benchmarks (M6 §6.2)

Dataset: synthetic captures produced by `scripts/gen_traffic.py` (ADR-022).
Parameters cover the four format dimensions: timestamp precision (legacy µs/ns
magic, PCAPNG binary `2^-n`), interface count, VLAN (incl. QinQ) and container
format.

```bash
python scripts/gen_traffic.py --out samples/synth-10mb.pcapng --format pcapng \
    --packets 100000 --profile mixed --interfaces 2 --vlan 100
python scripts/bench.py --binary target/release/packetsage \
    --captures samples/synth-10mb.pcapng --full
```

`scripts/bench.py` records wall clock, peak RSS, packets/s and MiB/s in
`docs/benchmarks/history/<date>-<hw>.json` and appends the table below.

## Acceptance targets (from 开发文档 §27.2)

| target | meaning |
|---|---|
| 100 MB capture ≤ 10 s | end-to-end `analyze` |
| 1 GB capture ≤ 60 s | parse + aggregate |
| RSS ≤ 1 GB | normal workload |

## Ratchet (M3~M6 §6.2)

Against `history/baseline-<hw>.json`: slower by more than **+10 %** is a
warning, by more than **+20 %** fails the run. Baselines are updated only by a
dedicated commit.

## Measured runs
## PacketSage benchmark

- date: 2026-09-20T04:35:08
- hardware: Windows-AMD64-AMD64 Family 26 Model 68 Stepping 0, AuthenticAMD
- rules: on

| capture | MiB | packets | wall s | packets/s | MiB/s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| samples\synth-1mb.pcapng | 1.109 | 12000 | 4.0547 | 2960 | 0.273 | 0.0 |
| samples\synth-10m.pcapng | 11.087 | 120000 | 26.4594 | 4535 | 0.419 | 0.0 |

ratchet: `{'status': 'no-baseline'}`

## PacketSage benchmark

- date: 2026-09-20T04:36:31
- hardware: Windows-AMD64-AMD64 Family 26 Model 68 Stepping 0, AuthenticAMD
- rules: on

| capture | MiB | packets | wall s | packets/s | MiB/s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| samples\synth-10m.pcapng | 11.087 | 120000 | 27.8521 | 4308 | 0.398 | 0.0 |

ratchet: `{'status': 'no-baseline'}`

## PacketSage benchmark

- date: 2026-09-20T04:37:38
- hardware: Windows-AMD64-AMD64 Family 26 Model 68 Stepping 0, AuthenticAMD
- rules: on

| capture | MiB | packets | wall s | packets/s | MiB/s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| samples\synth-10m.pcapng | 11.087 | 120000 | 2.4587 | 48806 | 4.509 | 0.0 |
| samples\synth-mixed.pcap | 0.041 | 612 | 0.3669 | 1668 | 0.113 | 0.0 |

ratchet: `{'status': 'no-baseline'}`

## PacketSage benchmark

- date: 2026-09-20T04:38:06
- hardware: Windows-AMD64-AMD64 Family 26 Model 68 Stepping 0, AuthenticAMD
- rules: on

| capture | MiB | packets | wall s | packets/s | MiB/s | RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| samples\synth-10m.pcapng | 11.087 | 120000 | 2.4199 | 49589 | 4.582 | 0.0 |

ratchet: `{'status': 'no-baseline'}`

