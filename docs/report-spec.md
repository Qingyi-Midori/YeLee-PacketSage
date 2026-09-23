# Report specification (M5)

Template version `v1`, prompt version `v2` (`agent.prompt_version` selects).
The renderer is
`agent/packetsage_agent/report.py`; every value comes from an RPC call, never
from the model's memory.

## Sections and data sources

| § | section | source |
|---|---|---|
| 1 | Executive Summary | `query_history kind=findings` (accepted findings only; `hypothesis` is listed separately as 待人工确认) |
| 2 | Capture Overview | `get_capture_summary` + `get_task_artifacts` |
| 3 | Protocol Statistics | `get_protocol_stats` for link / network / transport / application |
| 4 | Top Conversations | `get_conversations sort_by=bytes limit=20` |
| 5 | Rule Alerts | `check_alerts` (rule id, version, hash, window evidence, sample packets) |
| 6 | Agent Findings | `query_history kind=findings` |
| 7 | Evidence | `findings[].evidence` + the tc index table |
| 8 | Limitations | `get_capture_summary` counters + application stats |
| 9 | Analysis Trace | `query_history kind=trace` (the engine's tc-id ledger: `_id, stage, method, args, duration_ms, status, ts_unix_ns`) |

`kind=trace` rows also carry `numbers`, `ref_ids` and `tokens` — the evidence
facts the engine recorded when it produced each result (ADR-023). They are the
reason a report rendered by a **later process** can still prove the numbers a
stored finding cites: `run` and `report` are separate spawns (Agent CLI §6), and
the tool-result bodies themselves are never persisted. The three fields travel
through the database (`tool_calls.numbers_json / ref_ids_json / tokens_json`,
migration `0002_tool_call_evidence_facts.sql`).

Evidence references render as `[_id] tc_01M2Y6... · check_alerts · S-000001`, which
only works because the envelope `_id`, the ledger primary key, the
`EvidenceRef._id` and the report index are the same string (B2).

`stage` is `agent` for calls the agent made and `report` for the ones this
generator made while collecting the nine sections (§5.1 requires them to travel
through RPC). Only `stage=agent` calls count against the agent budget, so the
report can never make the policy look over-budget.

## Header

The front matter carries the three fixed values plus usage, and the same values
are submitted through `SUBMIT_REPORT_META` (stored in `agent_runs`):

```text
- task: task_01M2... · agent run: run_...
- model: mock · provider: mock · temperature: 0.0
- template: v1 · prompt: v1
- usage: tokens_in 1244 · tokens_out 204 · cost_cents 0
```

## Rendering rules introduced by the first report review

| # | rule |
|---|---|
| C1 | integer metrics render without `.0` (the schema keeps them as floats) |
| C2 | §5 column is named `packet_range`, not `packets` |
| C3 | `report_path` is omitted when unknown instead of printing `None` |
| C4 | evidence without an entity id renders `tc_... · method` (no `-`) |
| C5 | the evidence index lists each `(_id, method, ref_id)` once |
| C6 | paths use `/` on every platform |
| C7 | §9 carries args / duration / status / stage |
| C8 | §4 prints `endpoint_a`, `endpoint_b` **and** `client -> server`, because section keys are canonical (lexical) while rules and tools speak client/server |

## Anti-hallucination lint

1. Every token the engine produced during the run (numbers, IPs, ports, session
   ids, rule ids, alert ids, tc ids) forms the *legal set*.
   Cross-process reports rebuild that set from the RPC data **plus** the
   persisted ledger facts of the task's earlier runs (see the note above); the
   rule itself is unchanged — a number the engine never produced is still an
   offender, and the Executive Summary gate is still a hard failure.
2. Finding titles and summaries are **model output** and are therefore excluded
   from the legal set — only their identifiers and evidence anchors count.
3. Executive Summary offenders → `AntiHallucinationError` → exit 4, no file
   written, stderr contains `anti-hallucination`.
4. Body offenders → replaced by `[unverified by engine]`, counted, and reported
   in the header as `⚠ UNVERIFIED CONTENT: n items` with `status=degraded`.
5. Numeric ranges such as `1821-3070` are accepted when both endpoints are
   legal (they are the alert's `first_packet`/`last_packet`).

## Degradation matrix (`analyze --full --report`)

| failure | behaviour |
|---|---|
| parse / rules / storage fails | exit 2 or 3, no report |
| agent stage fails (LLM / tool) | section 6 states the failure, `status=degraded`, exit 0 + stderr WARN |
| body lint hits | report written with markers, exit 0, `status=degraded` |
| Executive Summary lint hits | exit 4, no report, stderr `anti-hallucination` |
