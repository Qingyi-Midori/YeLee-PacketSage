# Exit codes, RPC codes and decode enums

Single source of truth: `crates/packetsage-cli/src/exit.rs`,
`crates/packetsage-protocol/src/rpc.rs`, `crates/packetsage-protocol/src/events.rs`.
This document mirrors them; `tests/integration/demo_mainline.py` and the CI
`error-codes` check compare the two.

## Exit codes

| code | meaning | source |
|---|---|---|
| 0 | success | `ExitCode::Success` |
| 1 | invalid CLI argument / bad user input | `PacketSageError::InvalidArgument` |
| 2 | capture format or parsing error | `UnsupportedCapture`, `CaptureCorrupted`, `Io` |
| 3 | configuration / database / LLM / rule error | `RuleError`, `DatabaseError`, `LlmError` |
| 4a | internal error | `Internal`, `ToolError` |
| 4b | **anti-hallucination hard failure**: the Executive Summary cited a number the engine never produced; the report is *not* written and stderr contains `anti-hallucination` (C9 double meaning of exit 4) | `agent/packetsage_agent/report.py` |
| 5 | unsupported feature (e.g. `--report` without `--full`, agent not installed) | `ExitCode::Unsupported` |

Decode errors inside a task do **not** change the exit code: they are part of
the event stream (`decode_error`), and the task still finishes with 0.

Panics are a bug, not a user error: the CLI installs a panic hook, prints one
`internal error (panic)` line on stderr and exits 4 (CLI 收口工程规格书 §4/§9,
#32). The line is scriptable through `PACKETSAGE_PANIC_FOR_TEST=1`.

The per-command verification of this table lives in
`tests/cli/exit_codes.py` (one case per §4 row, plus the three-stream and
determinism assertions of §14.1):

```bash
python tests/cli/exit_codes.py --binary target/release/packetsage
```

## RPC error codes (`RpcErrorCode`)

| code | meaning |
|---|---|
| `INVALID_ARGUMENT` | bad or missing parameters |
| `NOT_FOUND` | unknown task / session / packet |
| `NOT_IMPLEMENTED` | method exists but is not available |
| `INTERNAL` | unexpected engine failure |
| `IO` | file or stream failure |
| `UNSUPPORTED_CAPTURE` | capture format outside the matrix |
| `CAPTURE_UNAVAILABLE` | cold recovery requested but the source file is gone (ADR-019) |
| `RULE_ERROR` | rule loading / evaluation failure |
| `DATABASE_ERROR` | persistence failure |
| `VALIDATION_FAILED` | finding rejected by V1-V4 |

## Event schema

`SCHEMA_VERSION = 2` (ADR-013). Event names:

`task_started`, `capture_info`, `packet`, `decode_error`, `session_summary`,
`stream_state`, `stats`, `alert`, `task_finished`.

## Decode error codes and layers

| code | meaning |
|---|---|
| `truncated_header` | header shorter than the protocol requires |
| `bad_length` | length field inconsistent with the captured bytes |
| `invalid_field` | field value invalid for the protocol |
| `unsupported_linktype` | link type outside the matrix |
| `unsupported_protocol` | protocol outside the matrix |

`layer` ∈ `link`, `vlan`, `arp`, `ipv4`, `ipv6`, `tcp`, `udp`, `icmp`,
`icmpv6`, `dns`, `http`, `tls`, `dhcp`.

## Structural identifiers used by the anti-hallucination lint

The report lint ignores tokens that belong to the project vocabulary rather
than to a capture: `ADR-nnn`, `En`, `Vn`, `Sn`, `In`, `D-n`.
