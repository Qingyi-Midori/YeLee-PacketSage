"""Report generator with the two level anti-hallucination lint (M3~M6 §5).

* section 1 (Executive Summary) violations are a **hard failure**: exit 4, no
  report is written (`anti-hallucination` appears in the stderr message);
* body violations are replaced with `[unverified by engine]` and the report is
  still written, with `status = degraded`.
"""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .engine_client import EngineError, RpcCaller
from .prompts import PROMPT_VERSION

TEMPLATE_VERSION = "v1"

UNVERIFIED_MARKER = "[unverified by engine]"

RULE_ID_RE = re.compile(r"\b[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*-\d{3}\b")
SESSION_ID_RE = re.compile(r"\bS-\d{5,6}\b")
ALERT_ID_RE = re.compile(r"\balert_[0-9A-HJKMNP-TV-Z]{26}\b")
TC_ID_RE = re.compile(r"\btc_[0-9A-HJKMNP-TV-Z]{26}\b")
IPV4_RE = re.compile(r"\b\d{1,3}(?:\.\d{1,3}){3}\b")
NUMBER_RE = re.compile(r"(?<![\w.-])\d+(?:\.\d+)?(?![\w.-])")
NUMERIC_RANGE_RE = re.compile(r"(?<![\d.])(\d+)\s*[-–]\s*(\d+)(?![\d.])")

#: Identifiers that are part of the project's own vocabulary rather than
#: capture facts.
STRUCTURAL_RE = re.compile(r"^(?:ADR-\d{3}|E\d|V\d|S\d|I\d|D-\d)$")


class AntiHallucinationError(EngineError):
    """Executive Summary contained a claim the engine never produced."""


@dataclass
class ReportMeta:
    """Metadata returned after rendering (the three fixed values included)."""

    path: str
    sha256: str
    degraded: bool
    unverified_count: int
    status: str
    template_version: str = TEMPLATE_VERSION
    prompt_version: str = PROMPT_VERSION
    agent_run_id: str = ""
    model: str = "unknown"
    provider: str = "unknown"
    temperature: float = 0.0
    tokens_in: int = 0
    tokens_out: int = 0
    cost_cents: int = 0


@dataclass
class LegalFacts:
    """Every token the engine actually produced during the run."""

    tokens: set[str] = field(default_factory=set)

    def add_numbers(self, value: Any) -> None:
        if isinstance(value, bool):
            return
        if isinstance(value, int):
            self.tokens.add(str(value))
            self.tokens.add(f"{value}.0")
        elif isinstance(value, float):
            self.tokens.add(str(value))
            self.tokens.add(f"{int(value)}")
            self.tokens.add(f"{value:.1f}")
        elif isinstance(value, str):
            self.tokens.update(extract_tokens(value))
        elif isinstance(value, dict):
            for item in value.values():
                self.add_numbers(item)
        elif isinstance(value, list):
            for item in value:
                self.add_numbers(item)


def extract_tokens(text: str) -> set[str]:
    """Collects every verifiable token from a chunk of engine output."""
    tokens = set(IPV4_RE.findall(text))
    tokens.update(RULE_ID_RE.findall(text))
    tokens.update(SESSION_ID_RE.findall(text))
    tokens.update(ALERT_ID_RE.findall(text))
    tokens.update(TC_ID_RE.findall(text))
    tokens.update(NUMBER_RE.findall(text))
    return tokens


def lint(text: str, legal: LegalFacts) -> list[str]:
    """Returns the tokens of `text` that the engine never produced."""
    offenders = []
    tokens = extract_tokens(text)
    # A packet range like `1821-3070` is legal when both endpoints are: the
    # individual endpoints are dropped from the offender list.
    for start, end in NUMERIC_RANGE_RE.findall(text):
        if start in legal.tokens and end in legal.tokens:
            tokens.discard(start)
            tokens.discard(end)
    for token in sorted(tokens):
        if token in legal.tokens:
            continue
        if STRUCTURAL_RE.match(token):
            continue
        # ip:port / port:ip forms
        if ":" in token and any(part in legal.tokens for part in token.split(":")):
            continue
        offenders.append(token)
    return offenders


def lint_body(text: str, legal: LegalFacts) -> list[str]:
    """Lints prose only: markdown headings and the front matter are structure."""
    scrub = "\n".join(
        line
        for line in text.splitlines()
        if not line.lstrip().startswith("#") and not line.strip().startswith("- task:")
    )
    return lint(scrub, legal)


def number(value: Any) -> str:
    """Renders counts without a trailing `.0` (C1).

    The event schema keeps `evidence.value` as a float (`M3~M6 §3.6`), but a
    count of 101 is a count: the markdown must not imply decimal precision.
    """
    if isinstance(value, bool) or value is None:
        return str(value)
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return str(value)


class ReportGenerator:
    """Renders the nine section markdown report from RPC data only."""

    def __init__(
        self,
        client: RpcCaller,
        task_id: str,
        agent_run_id: str,
        template_dir: Path | None = None,
        model: str = "unknown",
        provider: str = "unknown",
        temperature: float = 0.0,
        tokens_in: int = 0,
        tokens_out: int = 0,
        cost_cents: int = 0,
        prompt_version: str | None = None,
    ) -> None:
        self.client = client
        self.task_id = task_id
        self.agent_run_id = agent_run_id
        self.template_dir = template_dir
        self.model = model
        self.provider = provider
        self.temperature = temperature
        self.tokens_in = tokens_in
        self.tokens_out = tokens_out
        self.cost_cents = cost_cents
        #: `agent.prompt_version` when the configuration pins one (§9), else the
        #: compiled-in prompt version of `prompts.py`.
        self.prompt_version = prompt_version or PROMPT_VERSION
        self.legal = LegalFacts()
        #: tc ids this generator minted itself, so §9 can label the stage
        #: (`report` vs `agent`) instead of blaming the agent for report reads.
        self.own_calls: set[str] = set()

    # ------------------------------------------------------------ fetching
    def _call(
        self,
        method: str,
        params: dict[str, Any],
        add_legal: bool = True,
    ) -> dict[str, Any]:
        envelope = self.client.call(method, {"task_id": self.task_id, **params})
        if not isinstance(envelope, dict):
            return {}
        if add_legal:
            self.legal.add_numbers(envelope.get("content"))
        own_id = str(envelope.get("_id", ""))
        self.legal.tokens.add(own_id)
        self.own_calls.add(own_id)
        return envelope.get("content") or {}

    def collect(self) -> dict[str, Any]:
        """Collects every data source the report needs (all via RPC)."""
        data: dict[str, Any] = {}
        data["summary"] = self._call("get_capture_summary", {})
        data["stats"] = {
            layer: self._call("get_protocol_stats", {"layer": layer, "top": 10})
            for layer in ("link", "network", "transport", "application")
        }
        conversations = self._call("get_conversations", {"sort_by": "bytes", "limit": 20})
        data["conversations"] = conversations.get("conversations", [])
        data["alerts"] = self._call("check_alerts", {}).get("alerts", [])
        # Finding titles and summaries are model authored: they are *not* part
        # of the legal fact set. Only their identifiers and evidence anchors are.
        findings = self._call(
            "query_history", {"kind": "findings", "limit": 50}, add_legal=False
        ).get("findings", [])
        for finding in findings:
            self.legal.tokens.add(str(finding.get("finding_id", "")))
            for reference in finding.get("evidence", []):
                self.legal.tokens.add(str(reference.get("_id", "")))
                self.legal.tokens.add(str(reference.get("method", "")))
                if reference.get("ref_id"):
                    self.legal.tokens.add(str(reference["ref_id"]))
        data["findings"] = findings
        data["artifacts"] = self._call("get_task_artifacts", {})
        # The analysis trace is the engine's tc-id ledger: tool, args summary and
        # timestamp only, never chain of thought (ADR-007).
        data["trace"] = self._call("query_history", {"kind": "trace", "limit": 200}).get(
            "trace", []
        )
        return data

    # ----------------------------------------------------------- rendering
    def render(self, data: dict[str, Any]) -> tuple[str, int, bool]:
        """Renders the markdown; returns `(text, unverified_count, degraded)`."""
        summary = data.get("summary", {})
        findings = [f for f in data.get("findings", []) if f.get("validator_status") != "rejected"]
        executive = self._render_executive(findings)
        offenders = lint(executive, self.legal)
        if offenders:
            raise AntiHallucinationError(
                "anti-hallucination: Executive Summary cites "
                + ", ".join(offenders[:8])
                + " which the engine never produced"
            )

        body_parts = [
            self._render_overview(summary, data.get("artifacts", {})),
            self._render_protocol_stats(data.get("stats", {})),
            self._render_conversations(data.get("conversations", [])),
            self._render_alerts(data.get("alerts", [])),
            self._render_findings(findings),
            self._render_evidence(findings),
            self._render_limitations(summary, data),
            self._render_trace(data.get("trace", [])),
        ]
        unverified = 0
        cleaned = []
        for part in body_parts:
            offenders = lint_body(part, self.legal)
            for token in offenders:
                part = part.replace(token, UNVERIFIED_MARKER)
            unverified += len(offenders)
            cleaned.append(part)
        degraded = unverified > 0
        header = [
            "# PacketSage Analysis Report",
            "",
            f"- task: `{self.task_id}`",
            f"- agent run: `{self.agent_run_id}`",
            f"- model: `{self.model}` · provider: `{self.provider}` · "
            f"temperature: {self.temperature}",
            f"- template: `{TEMPLATE_VERSION}` · prompt: `{self.prompt_version}`",
            f"- usage: tokens_in {self.tokens_in} · tokens_out {self.tokens_out} · "
            f"cost_cents {self.cost_cents}",
        ]
        if degraded:
            header.append(f"- ⚠ UNVERIFIED CONTENT: {unverified} items")
        header.append("")
        text = "\n".join(header) + executive + "\n" + "\n".join(cleaned)
        return text, unverified, degraded

    def _render_executive(self, findings: list[dict[str, Any]]) -> str:
        lines = ["## 1. Executive Summary", ""]
        confirmed = [
            f
            for f in findings
            if f.get("basis") in ("rule_match", "direct_observation", "correlated_observation")
        ]
        hypotheses = [f for f in findings if f.get("basis") == "hypothesis"]
        if not confirmed and not hypotheses:
            lines.append("确定性引擎未产生 accepted 结论；本次报告只呈现观测数据与限制。")
        for finding in confirmed:
            lines.append(
                f"- **{finding.get('finding_id')} {finding.get('title')}** "
                f"({finding.get('severity')}, {finding.get('basis')}): {finding.get('summary')}"
            )
        if hypotheses:
            lines.append("")
            lines.append("待人工确认（hypothesis）：")
            for finding in hypotheses:
                lines.append(
                    f"- {finding.get('finding_id')} {finding.get('title')}: {finding.get('summary')}"
                )
        lines.append("")
        return "\n".join(lines)

    def _render_overview(self, summary: dict[str, Any], artifacts: dict[str, Any]) -> str:
        def portable(path: Any) -> str:
            """Report paths use forward slashes on every platform (C6)."""
            return str(path).replace("\\", "/")

        lines = [
            "## 2. Capture Overview",
            "",
            f"- file: `{portable(summary.get('source_path'))}`",
            f"- sha256: `{summary.get('source_sha256')}`",
            f"- format: {summary.get('format')}",
            f"- packets: {summary.get('packets')}",
            f"- bytes: {summary.get('bytes')}",
            f"- sessions: {summary.get('sessions')}",
            f"- duration_s: {summary.get('duration_s')}",
            f"- first_ts_ns: {summary.get('first_ts_ns')}",
            f"- last_ts_ns: {summary.get('last_ts_ns')}",
        ]
        # The report path only exists after this file is written; a placeholder
        # (`None`) is worse than omitting the line (C3).
        report_path = artifacts.get("report_path")
        if report_path:
            lines.append(f"- report_path: `{portable(report_path)}`")
        model = artifacts.get("report_meta") or {}
        if model:
            lines.append(
                f"- agent: model `{model.get('model')}` · provider `{model.get('provider')}`"
                f" · temperature {model.get('temperature')} · prompt"
                f" `{model.get('prompt_version')}`"
            )
        lines.append("")
        return "\n".join(lines)

    def _render_protocol_stats(self, stats: dict[str, Any]) -> str:
        lines = ["## 3. Protocol Statistics", ""]
        for layer, payload in stats.items():
            lines.append(f"### {layer}")
            rows = payload.get("rows", [])
            if not rows:
                lines.append("_no rows_")
            else:
                lines.append("| protocol | packets | bytes |")
                lines.append("|---|---:|---:|")
                for row in rows:
                    lines.append(
                        f"| {row.get('protocol')} | {row.get('packets')} | {row.get('bytes')} |"
                    )
            lines.append("")
        return "\n".join(lines)

    def _render_conversations(self, rows: list[dict[str, Any]]) -> str:
        lines = [
            "## 4. Top Conversations",
            "",
            "会话键按规范五元组字典序存放（`endpoint_a` = 字典序较小端）；"
            "`client` 是方向判定结果，规则与工具的 `src/dst` 语义以它为准。",
            "",
            "| session | protocol | endpoint_a | endpoint_b | client -> server | packets | bytes | state | app | direction_basis |",
            "|---|---|---|---|---|---:|---:|---|---|---|",
        ]
        for row in rows:
            endpoint_a = f"{row.get('src_ip')}:{row.get('src_port')}"
            endpoint_b = f"{row.get('dst_ip')}:{row.get('dst_port')}"
            client_to_server = (
                f"{row.get('client_ip')}:{row.get('client_port')} -> "
                f"{row.get('server_ip')}:{row.get('server_port')}"
            )
            lines.append(
                f"| {row.get('session_id')} | {row.get('protocol')} | {endpoint_a} | "
                f"{endpoint_b} | {client_to_server} | {row.get('packets')} | "
                f"{row.get('bytes')} | {row.get('state')} | {row.get('app_protocol')} | "
                f"{row.get('direction_basis')} |"
            )
        lines.append("")
        return "\n".join(lines)

    def _render_alerts(self, alerts: list[dict[str, Any]]) -> str:
        lines = ["## 5. Rule Alerts", ""]
        if not alerts:
            lines.append("_no alerts_")
            lines.append("")
            return "\n".join(lines)
        lines.append(
            "| alert | rule | version | severity | packet_range | value | threshold | sample |"
        )
        lines.append("|---|---|---|---|---:|---:|---:|---|")
        for alert in alerts:
            evidence = alert.get("evidence", {})
            lines.append(
                f"| {alert.get('alert_id')} | {alert.get('rule_id')} | {alert.get('rule_version')} | "
                f"{alert.get('severity')} | {alert.get('first_packet')}-{alert.get('last_packet')} | "
                f"{number(evidence.get('value'))} | {number(evidence.get('threshold'))} | "
                f"{evidence.get('sample_packets')} |"
            )
        lines.append("")
        return "\n".join(lines)

    def _render_findings(self, findings: list[dict[str, Any]]) -> str:
        lines = ["## 6. Agent Findings", ""]
        if not findings:
            lines.append("_no findings_")
            lines.append("")
            return "\n".join(lines)
        for finding in findings:
            lines.append(
                f"### {finding.get('finding_id')} {finding.get('title')}"
            )
            lines.append(
                f"- severity: {finding.get('severity')} · basis: {finding.get('basis')} · "
                f"status: {finding.get('validator_status')}"
            )
            lines.append(f"- {finding.get('summary')}")
            lines.append("")
        return "\n".join(lines)

    def _render_evidence(self, findings: list[dict[str, Any]]) -> str:
        lines = ["## 7. Evidence", ""]
        seen: set[tuple[str, str, str]] = set()
        index_rows: list[tuple[str, str, str]] = []
        for finding in findings:
            for ref in finding.get("evidence", []):
                tc_id = ref.get("_id")
                # Stat-like results (e.g. get_capture_summary) legitimately have
                # no entity id; render nothing instead of a placeholder (C4).
                ref_id = ref.get("ref_id") or ""
                reference = f" · {ref_id}" if ref_id else ""
                lines.append(
                    f"- [{finding.get('finding_id')}] {tc_id} · {ref.get('method')}{reference}"
                )
                key = (str(tc_id), str(ref.get("method")), ref_id)
                if key not in seen:
                    seen.add(key)
                    index_rows.append(key)
        if not index_rows:
            lines.append("_no evidence references_")
        lines.append("")
        lines.append("Evidence index（同一 tool result 只列一行）")
        lines.append("")
        lines.append("| _id | method | ref_id |")
        lines.append("|---|---|---|")
        for tc_id, method, ref_id in index_rows:
            lines.append(f"| {tc_id} | {method} | {ref_id} |")
        lines.append("")
        return "\n".join(lines)

    def _render_limitations(self, summary: dict[str, Any], data: dict[str, Any]) -> str:
        lines = ["## 8. Limitations", ""]
        items = {
            "truncated packets": summary.get("truncated_packets", 0),
            "incomplete / overflowed TCP streams": summary.get("incomplete_sessions", 0),
            "decode errors": summary.get("decode_errors", 0),
            "reader non-fatal events": summary.get("reader_nonfatal", 0),
            "rule window evictions": 0,
            "sessions dropped by the session cap": summary.get("dropped_sessions", 0),
        }
        for name, value in items.items():
            if value:
                lines.append(f"- {name}: {value}")
            else:
                lines.append(f"- {name}: 0 — 无此类限制")
        tls_rows = [
            row
            for row in data.get("stats", {}).get("application", {}).get("rows", [])
            if row.get("protocol") == "tls"
        ]
        if tls_rows:
            lines.append("- TLS payload 未解密，结论限于元数据")
        lines.append(
            "- distinct_count(tls.sni)/distinct_count(http.host) 仅统计单包可解析样本，"
            "mid-stream 会话会静默欠计（只可能漏报，不会误报）"
        )
        lines.append("")
        return "\n".join(lines)

    def _render_trace(self, sessions: list[dict[str, Any]]) -> str:
        lines = [
            "## 9. Analysis Trace",
            "",
            "本节只记录工具调用与结果摘要，不包含模型思维过程（ADR-007）。"
            "`stage=agent` 是 Agent 的取证调用，`stage=report` 是报告自身按 §5.1"
            "通过 RPC 采集数据（不计入 Agent 预算）。",
            "",
            "| tc_id | stage | method | args | duration_ms | status | ts_unix_ns |",
            "|---|---|---|---|---:|---|---|",
        ]
        if not sessions:
            lines.append("| _none_ | | | | | | |")
        for entry in sessions:
            tc_id = str(entry.get("_id"))
            stage = "report" if tc_id in self.own_calls else "agent"
            args = str(entry.get("args") or "{}")
            if len(args) > 120:
                args = args[:120] + "…"
            lines.append(
                f"| {tc_id} | {stage} | {entry.get('method')} | `{args}` | "
                f"{entry.get('duration_ms', 0)} | {entry.get('status', 'ok')} | "
                f"{entry.get('ts_unix_ns')} |"
            )
        lines.append("")
        return "\n".join(lines)

    # ----------------------------------------------------------- lifecycle
    def generate(self, out_path: Path) -> ReportMeta:
        """Collects, renders, lints and writes the report."""
        data = self.collect()
        text, unverified, degraded = self.render(data)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(text, encoding="utf-8")
        digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
        status = "degraded" if degraded else "ok"
        self.client.call(
            "submit_report_meta",
            {
                "task_id": self.task_id,
                "report_path": str(out_path),
                "status": status,
                "unverified_count": unverified,
                "agent_run_id": self.agent_run_id,
                "model": self.model,
                "provider": self.provider,
                "temperature": self.temperature,
                "prompt_version": self.prompt_version,
                "template_version": TEMPLATE_VERSION,
                "tokens_in": self.tokens_in,
                "tokens_out": self.tokens_out,
                "cost_cents": self.cost_cents,
            },
        )
        return ReportMeta(
            path=str(out_path),
            sha256=digest,
            degraded=degraded,
            unverified_count=unverified,
            status=status,
            prompt_version=self.prompt_version,
            agent_run_id=self.agent_run_id,
            model=self.model,
            provider=self.provider,
            temperature=self.temperature,
            tokens_in=self.tokens_in,
            tokens_out=self.tokens_out,
            cost_cents=self.cost_cents,
        )


def render_trace_table(trace: list[dict[str, Any]]) -> str:
    """Renders the agent tool trace table (`analysis trace` source)."""
    lines = ["| # | tool | args | status | duration_ms | tc_id |", "|---|---|---|---|---:|---|"]
    for index, entry in enumerate(trace, start=1):
        lines.append(
            f"| {index} | {entry.get('tool_name')} | "
            f"{json.dumps(entry.get('args'), ensure_ascii=False)} | {entry.get('status')} | "
            f"{entry.get('duration_ms')} | {entry.get('tc_id')} |"
        )
    return "\n".join(lines)
