# ADR index

The project has 27 numbered decisions. Only 024–027 live as standalone files in
this directory; the rest sit inside the specification set. This index is the
answer to G6-4 of *GUI 前收口文档 v0.1*: instead of back-filling 16 files, every
number is mapped to its authoritative text (or explicitly recorded as missing).

## Standalone files

| ADR | File |
|---|---|
| ADR-024 | [tool call id is a ULID](ADR-024-tool-call-id-ulid.md) |
| ADR-025 | [direction basis: first-seen fallback](ADR-025-direction-basis-first-seen.md) |
| ADR-026 | [evidence anchor advertisement](ADR-026-evidence-anchor-advertisement.md) |
| ADR-027 | [token level streaming (`llm_delta`)](ADR-027-token-level-streaming.md) |

## In `开发文档 v0.3.md` §33 (full text)

| ADR | Decision |
|---|---|
| ADR-001 | 不做 Rust ↔ Python FFI |
| ADR-002 | 采用 `pcap-parser + etherparse` |
| ADR-003 | Agent 使用当前 LangChain agent API |
| ADR-004 | SQLite first |
| ADR-005 | 结构化 YAML DSL |
| ADR-006 | Agent 不直接访问任意 SQL |
| ADR-007 | 不保存 Chain of Thought |

## Referenced in the specs (no standalone file)

These numbers are used as frozen contract names inside the specification set;
the deciding text is the referenced section, not a separate ADR file.

| ADR | Decision (as referenced) | Where it is defined / used |
|---|---|---|
| ADR-008 … ADR-012 | — | **No trace in this tree.** Neither `开发文档 v0.3.md` nor any spec names them. If these decisions exist outside the repository they must be supplied before the number range can be closed. |
| ADR-013 | `SCHEMA_VERSION` bump when the event set changes (1 → 2 for alerts) | `M3～M6 工程规格书 v0.2.md` §3.6; `CLI 收口工程规格书 v0.2.md` §5 |
| ADR-014 | New L1' crate `packetsage-storage` | `M3～M6 工程规格书 v0.2.md` §3.7 |
| ADR-015 | Packet time for all business decisions; wall clock only for presentation and I/O pacing | `M3～M6 工程规格书 v0.2.md` §3.7; `CLI 收口工程规格书 v0.2.md` §3.3 |
| ADR-016 | DSL compromise: length/entropy thresholds as match filters, no mean-type metrics | `M3～M6 工程规格书 v0.2.md` §3.4 |
| ADR-017 | Rules load leniently at runtime, strictly in `rules check` | `M3～M6 工程规格书 v0.2.md` §3.5 |
| ADR-018 | Single writer: the Python agent never opens the database, everything goes through RPC | `M3～M6 工程规格书 v0.2.md` §3.8; `Agent CLI 工程规格书 v0-1.md` §1 |
| ADR-019 | `serve` cold recovery replays the pipeline from `source_path` when the task is not in memory | `M3～M6 工程规格书 v0.2.md` §3.8; `Agent CLI 同步点 S7-S8.md` |
| ADR-020 | Two-level report anti-hallucination: Executive Summary hard-fails, body is downgraded | `M3～M6 工程规格书 v0.2.md` §5.3 |
| ADR-021 | Report template `report.md.j2` is versioned | `M3～M6 工程规格书 v0.2.md` §5.3 |
| ADR-022 | Benchmarks use generated traffic as the formal dataset | `M3～M6 工程规格书 v0.2.md` §6; `docs/benchmarks.md` |
| ADR-023 | Two-stage evidence validation: V1–V4 in Rust, V5 in Python | `M3～M6 工程规格书 v0.2.md` §0.3/§3.8/§4.4; `docs/architecture.md` D-18 |

## Open item

ADR-008 … ADR-012 are a gap in the set. They must either be supplied by the
project owner or the numbering must be declared abandoned; this is recorded in
*GUI 前收口文档 v0.1* §2.3 (R-series) as a documentation debt, not a code debt.
