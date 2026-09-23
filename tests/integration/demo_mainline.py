#!/usr/bin/env python3
"""演示主线：§38 的十步流程，用 mock provider 自动化跑通。

    python tests/integration/demo_mainline.py --binary target/debug/packetsage

Steps ①→⑩ mirror 开发文档 §38:

1. open the capture (engine `analyze_file`)
2. Rust protocol statistics
3. top sessions
4. rules find a SYN burst / port sweep / DNS anomaly
5. the agent notices the alert
6. the agent filters the related packets
7. the agent inspects a TCP session
8. evidence is correlated
9. a finding with evidence is produced
10. the markdown report is rendered
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "agent"))

from packetsage_agent.agent import build_agent  # noqa: E402
from packetsage_agent.engine_client import EngineClient  # noqa: E402
from packetsage_agent.report import ReportGenerator  # noqa: E402


def main() -> int:
    """Runs the ten step demo and prints the evidence chain."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/packetsage")
    parser.add_argument("--capture", default="samples/synth-mixed.pcap")
    parser.add_argument("--report", default="reports/demo-mainline.md")
    parser.add_argument("--provider", default="mock")
    args = parser.parse_args()

    capture = Path(args.capture)
    if not capture.exists():
        print(f"demo: {capture} is missing; run scripts/gen_traffic.py first", file=sys.stderr)
        return 2

    steps: list[str] = []
    with EngineClient([args.binary, "serve"]) as client:
        analyzed = client.call("analyze_file", {"path": str(capture), "emit": "none"})
        task_id = analyzed["task_id"]
        summary = analyzed["summary"]
        steps.append(
            f"① open PCAP: {summary['packets']} packets, {summary['bytes']} bytes, "
            f"{summary['sessions']} sessions ({capture})"
        )

        stats = client.call(
            "get_protocol_stats", {"task_id": task_id, "layer": "transport", "top": 5}
        )["content"]
        rows = ", ".join(f"{r['protocol']}={r['packets']}" for r in stats["rows"])
        steps.append(f"② Rust protocol statistics: {rows}")

        conversations = client.call(
            "get_conversations", {"task_id": task_id, "sort_by": "bytes", "limit": 3}
        )["content"]["conversations"]
        top = conversations[0]
        steps.append(
            f"③ top session: {top['session_id']} {top['src_ip']}:{top['src_port']} -> "
            f"{top['dst_ip']}:{top['dst_port']} ({top['bytes']} bytes)"
        )

        alerts = client.call("check_alerts", {"task_id": task_id})["content"]["alerts"]
        steps.append(
            "④ rules detected: "
            + "; ".join(f"{a['rule_id']} ({a['severity']})" for a in alerts)
        )
        if not alerts:
            print("demo: the capture produced no alerts; regenerate with "
                  "`python scripts/gen_traffic.py --profile mixed`", file=sys.stderr)
            return 3

        agent = build_agent(client, provider_kind=args.provider, scenario="E2")
        result = agent.run(task_id, "按演示主线分析该捕获")
        steps.append(
            f"⑤⑥⑦⑧ agent tool path: {' -> '.join(t.tool_name for t in result.trace)}"
        )
        steps.append(
            f"⑨ findings: {len(result.findings)} (basis: "
            + ", ".join(f.basis for f in result.findings)
            + ")"
        )
        generator = ReportGenerator(
            client,
            task_id,
            result.agent_run_id,
            model=result.model,
            provider=agent.provider_kind,
            temperature=0.0,
            tokens_in=result.tokens_in,
            tokens_out=result.tokens_out,
            cost_cents=result.cost_cents,
        )
        meta = generator.generate(Path(args.report))
        steps.append(f"⑩ report: {meta.path} sha256={meta.sha256[:12]} status={meta.status}")

    for step in steps:
        print(step)
    payload = {
        "task_id": task_id,
        "agent_run_id": result.agent_run_id,
        "findings": [f.as_dict() for f in result.findings],
        "trace": [t.__dict__ for t in result.trace],
        "report": meta.path,
        "report_sha256": meta.sha256,
    }
    Path("reports").mkdir(exist_ok=True)
    Path("reports/demo-mainline.json").write_text(
        json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(f"\njson summary: reports/demo-mainline.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
