#!/usr/bin/env python3
"""Benchmark harness (M3~M6 §6.2, ADR-022).

Runs `packetsage analyze` over a synthetic dataset and records wall clock,
peak RSS, packets/s and MB/s. Results land in
`docs/benchmarks/history/<date>-<hw>.json` with a human readable summary in
`docs/benchmarks.md`.

The ratchet thresholds from the spec are applied when a baseline exists:
more than +10% is a warning, more than +20% is a failure.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import platform
import subprocess
import sys
import time
from pathlib import Path


def peak_rss_mb() -> float:
    """Peak RSS of this process tree, in MiB (best effort, no dependencies)."""
    try:  # pragma: no cover - platform specific
        import resource

        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        divisor = 1024 * 1024 if sys.platform == "darwin" else 1024
        return usage.ru_maxrss / divisor
    except Exception:
        return 0.0


def run_once(binary: str, capture: Path, extra: list[str]) -> dict:
    """Runs one analysis and measures it."""
    command = [binary, "analyze", str(capture), *extra]
    started = time.perf_counter()
    completed = subprocess.run(command, capture_output=True, text=True)  # noqa: S603
    duration = time.perf_counter() - started
    stdout = completed.stdout
    packets = 0
    bytes_captured = 0
    for line in stdout.splitlines():
        if line.startswith("packets "):
            parts = line.split()
            packets = int(parts[1])
        if line.startswith("sessions "):
            pass
    try:
        bytes_captured = capture.stat().st_size
    except OSError:
        bytes_captured = 0
    mb = bytes_captured / (1024 * 1024)
    return {
        "capture": str(capture),
        "exit_code": completed.returncode,
        "wall_s": round(duration, 4),
        "packets": packets,
        "file_mib": round(mb, 3),
        "packets_per_s": round(packets / duration) if duration > 0 else 0,
        "mib_per_s": round(mb / duration, 3) if duration > 0 else 0.0,
        "rss_peak_mib": round(peak_rss_mb(), 2),
        "acceptance": {
            "100MB_under_10s": (mb < 100) or (duration <= 10.0),
            "rss_under_1GiB": peak_rss_mb() <= 1024,
        },
    }


def baseline_for(history_dir: Path) -> dict | None:
    """Most recent baseline file, if any."""
    baselines = sorted(history_dir.glob("baseline-*.json"))
    if not baselines:
        return None
    try:
        return json.loads(baselines[-1].read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def ratchet(result: dict, baseline: dict | None) -> dict:
    """Applies the +10% warn / +20% block ratchet."""
    if not baseline:
        return {"status": "no-baseline"}
    previous = baseline.get("results", [{}])[0].get("wall_s")
    current = result["wall_s"]
    if not previous:
        return {"status": "no-baseline"}
    change = (current - previous) / previous
    if change > 0.20:
        return {"status": "block", "change": round(change, 3)}
    if change > 0.10:
        return {"status": "warn", "change": round(change, 3)}
    return {"status": "ok", "change": round(change, 3)}


def main() -> int:
    """Benchmark entry point."""
    parser = argparse.ArgumentParser(description="PacketSage benchmark harness")
    parser.add_argument("--binary", default="target/release/packetsage")
    parser.add_argument(
        "--captures",
        nargs="+",
        default=["samples/synth-mixed.pcap", "samples/synth-synflood.pcapng"],
    )
    parser.add_argument("--full", action="store_true", help="include the rule engine")
    parser.add_argument("--out-dir", default="docs/benchmarks")
    parser.add_argument("--json", default=None, help="extra path for the raw JSON")
    args = parser.parse_args()

    extra = [] if args.full else ["--no-rules"]
    results = []
    for capture in args.captures:
        path = Path(capture)
        if not path.exists():
            print(f"bench: {capture} does not exist (run scripts/gen_traffic.py)", file=sys.stderr)
            return 2
        results.append(run_once(args.binary, path, extra))

    history_dir = Path(args.out_dir) / "history"
    history_dir.mkdir(parents=True, exist_ok=True)
    hardware_label = (
        f"{platform.system()}-{platform.machine()}-{platform.processor() or 'cpu'}"
    )
    hardware = "".join(
        ch if ch.isalnum() or ch in "-_." else "_" for ch in hardware_label
    )[:64]
    document = {
        "date": dt.datetime.now().isoformat(timespec="seconds"),
        "hardware": hardware_label,
        "python": platform.python_version(),
        "rules_enabled": args.full,
        "results": results,
        "ratchet": ratchet(results[0], baseline_for(history_dir)),
    }
    stamp = dt.date.today().isoformat()
    out_path = history_dir / f"{stamp}-{hardware}.json"
    out_path.write_text(json.dumps(document, ensure_ascii=False, indent=2), encoding="utf-8")
    if args.json:
        Path(args.json).write_text(json.dumps(document, indent=2), encoding="utf-8")

    lines = [
        "## PacketSage benchmark",
        "",
        f"- date: {document['date']}",
        f"- hardware: {hardware_label}",
        f"- rules: {'on' if args.full else 'off'}",
        "",
        "| capture | MiB | packets | wall s | packets/s | MiB/s | RSS MiB |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for item in results:
        lines.append(
            f"| {item['capture']} | {item['file_mib']} | {item['packets']} | "
            f"{item['wall_s']} | {item['packets_per_s']} | {item['mib_per_s']} | "
            f"{item['rss_peak_mib']} |"
        )
    lines.append("")
    lines.append(f"ratchet: `{document['ratchet']}`")
    lines.append("")
    text = "\n".join(lines)
    print(text)
    # Append the human readable table to docs/benchmarks.md (the parent of the
    # history folder), not inside the history folder itself.
    report = Path(args.out_dir).parent / "benchmarks.md"
    with report.open("a", encoding="utf-8") as handle:
        handle.write(text)
        handle.write("\n")
    print(f"written to {out_path} and {report}", file=sys.stderr)
    return 0 if all(item["exit_code"] == 0 for item in results) else 3


if __name__ == "__main__":
    raise SystemExit(main())
