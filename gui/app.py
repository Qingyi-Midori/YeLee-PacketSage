"""PacketSage 图形界面 —— 唯一入口（《GUI 工程规格书 v0.1》U8）。

```text
scripts\\run_gui.cmd                       # Windows 双击
streamlit run gui/app.py --server.port 8501
```

界面骨架按 §4.1 映射到 Streamlit：

| 区域 | 落点 | 内容 |
|---|---|---|
| 左栏 | ``st.sidebar`` | provider/model、抓包来源、历史 task、引擎与十项自检 |
| 顶部 | ``st.columns`` + ``st.metric`` | 预算常显（steps / calls / tools / tokens / cost） |
| 中央 | ``st.chat_message`` + ``st.expander`` | 目标、结论、**逐条出现的工具调用卡片** |
| 输入 | ``st.chat_input`` | 追问（chat 模式的系统提示词路径） |
| 控制 | ``st.button`` | 停止（``request_finalize``）、重跑、生成报告 |

四个一等公民（证据链 / 规则告警 / 预算常显 / 降级可见）与报告预览都在主区展开，
不进设置面板（§4.2）。
"""

from __future__ import annotations

import sys
import time
from pathlib import Path
from typing import Any

# Streamlit 把脚本所在目录（gui/）放进 sys.path，但界面要 import 仓库根的 `gui.*`
# 与 `agent/packetsage_agent`，所以这里显式补两条（U8）。
_ROOT = Path(__file__).resolve().parents[1]
for _entry in (str(_ROOT), str(_ROOT / "agent")):
    if _entry not in sys.path:
        sys.path.insert(0, _entry)

import streamlit as st  # noqa: E402

from gui import engine as engine_mod  # noqa: E402
from gui import jobs, views  # noqa: E402
from gui.paths import DB_URL, REPORTS_DIR, UPLOAD_DIR  # noqa: E402
from gui.settings import SETUP_COMMAND, AgentEnv, load_agent_env, provider_blocked  # noqa: E402

#: 一次 run 的默认目标（与 Agent CLI 的默认 `goal` 一致）。
DEFAULT_GOAL = "分析该捕获并给出可追溯结论"

#: RPC 错误码 → 人话（§5.4 规矩 1：任何错误都要在界面上说清楚）。
RPC_CODE_LABEL = {
    "InvalidArgument": "参数不合法（1）",
    "NotFound": "task 不存在（3）",
    "NotImplemented": "引擎还没实现（5）",
    "Internal": "引擎内部错误（4）",
    "Io": "文件/流读写失败（2）",
    "UnsupportedCapture": "抓包格式不在支持矩阵里（5）",
    "CaptureUnavailable": "源抓包不在原路径，任务无法冷恢复（CAPTURE_UNAVAILABLE，3）",
    "RuleError": "规则加载或求值失败（3）",
    "DatabaseError": "持久化失败（3）",
    "ValidationFailed": "结论没过证据校验（V1-V4，4）",
}


# --------------------------------------------------------------------------
# 状态
# --------------------------------------------------------------------------
def _init_state() -> None:
    st.session_state.setdefault("task_id", "")
    st.session_state.setdefault("task_summary", {})
    st.session_state.setdefault("log", [])
    st.session_state.setdefault("job", None)
    st.session_state.setdefault("analyze_job", None)
    st.session_state.setdefault("report", None)
    st.session_state.setdefault("last_budget", None)
    st.session_state.setdefault("upload_saved", ("", 0, ""))
    st.session_state.setdefault("uploaded_path", "")
    st.session_state.setdefault("notice", "")


def _active_job() -> jobs.Job | None:
    for key in ("analyze_job", "job"):
        job = st.session_state.get(key)
        if job is not None and not job.done.is_set():
            return job
    return None


def _mono(text: Any) -> str:
    return views.mono(text)


@st.cache_data(ttl=5, show_spinner=False)
def _task_rows(db_url: str) -> list[dict[str, Any]]:
    """历史 task 列表（``db query --readonly --jsonl``）；5s 缓存，成功分析后主动清。"""
    return engine_mod.list_tasks(db_url, limit=30)


# --------------------------------------------------------------------------
# 抓包来源（上传 / samples）
# --------------------------------------------------------------------------
def _save_upload(uploaded: Any) -> Path:
    """上传文件先落盘再分析（§4.4 约束 5 / S62：路径含空格与中文必须可用）。"""
    name = Path(str(uploaded.name)).name or "capture.pcap"
    data = uploaded.getvalue()
    signature = (name, len(data), str(uploaded.file_id if hasattr(uploaded, "file_id") else ""))
    saved = st.session_state.get("upload_saved")
    if saved and saved == signature and Path(saved[2] if len(saved) > 2 else "").is_file():
        return Path(saved[2])
    UPLOAD_DIR.mkdir(parents=True, exist_ok=True)
    target = UPLOAD_DIR / name
    target.write_bytes(data)
    st.session_state.upload_saved = (name, len(data), str(target))
    return target


def _pick_capture() -> Path | None:
    uploaded = st.file_uploader(
        "上传抓包（pcap / pcapng）", type=["pcap", "pcapng"], key="uploader"
    )
    if uploaded is not None:
        try:
            return _save_upload(uploaded)
        except OSError as exc:
            st.error(f"上传文件落盘失败：{exc}")
            return None
    options = engine_mod.samples()
    if not options:
        st.caption(
            "samples/ 里没有抓包，先合成一份：`python scripts/gen_traffic.py "
            "--out samples/synth-mixed.pcap --packets 600 --profile mixed`"
        )
        return None
    labels = [path.name for path in options]
    index = st.selectbox(
        "或从 samples/ 选",
        options=list(range(len(options))),
        format_func=lambda item: labels[int(item)],
        key="sample_pick",
    )
    return options[int(index)] if index is not None else None


# --------------------------------------------------------------------------
# 侧栏
# --------------------------------------------------------------------------
def _render_sidebar(
    session: engine_mod.EngineSession | None,
    env: AgentEnv,
    doctor: dict[str, Any],
    blocked: str | None,
    engine_notice: str,
) -> None:
    with st.sidebar:
        st.markdown("### 🛡️ PacketSage")
        if env.ok:
            st.markdown(
                f"**provider** `{env.provider}` · **model** `{env.model}`\n\n"
                f"prompt `{env.prompt_version}` · 配置来源 `{getattr(env.loaded, 'source', '-')}`"
            )
        if blocked:
            st.error(f"provider 没配好：{blocked}")
            st.code(SETUP_COMMAND, language="text")
            st.caption("配置写进 `agent/.env`（gitignored）；界面不会静默改用 mock。")

        st.divider()
        st.markdown("#### 抓包")
        capture = _pick_capture()
        analyze_clicked = st.button(
            "▶ 开始分析",
            key="start_analyze",
            disabled=blocked is not None or session is None or _active_job() is not None,
            help="analyze_file：落库 + 留在引擎内存，后续 run 直接引用同一个 task",
        )
        if analyze_clicked and session is not None and capture is not None:
            _start_analyze(session, capture)

        st.divider()
        st.markdown("#### 历史 task")
        rows = _task_rows(DB_URL)
        if rows:
            labels = [
                f"{row.get('id', '?')[:18]}… · {Path(str(row.get('source_path') or '')).name} · "
                f"{row.get('packet_count', '-')} pkts"
                for row in rows
            ]
            choice = st.selectbox(
                "打开一个已分析的任务",
                options=list(range(len(rows))),
                format_func=lambda item: labels[int(item)],
                key="history_pick",
            )
            if st.button("载入该任务", key="load_task"):
                _load_task(str(rows[int(choice)].get("id") or ""))
        else:
            st.caption("数据库里还没有任务。")

        st.divider()
        st.markdown("#### 当前 task")
        task_id = st.session_state.get("task_id") or ""
        if task_id:
            st.markdown(_mono(task_id))
            summary = st.session_state.get("task_summary") or {}
            if summary:
                st.caption(
                    f"{summary.get('packets', '-')} 包 · {summary.get('sessions', '-')} 会话 · "
                    f"{summary.get('alerts', '-')} 告警"
                )
            if st.button("▶ 开始调查（run）", key="start_run", disabled=_active_job() is not None):
                _start_run(session, env, task_id, DEFAULT_GOAL, mode="run")
        else:
            st.caption("还没有选中的任务。")

        st.divider()
        st.markdown("#### 引擎")
        if engine_notice:
            st.warning(engine_notice)
        if session is None and blocked:
            st.caption("provider 配好之后再启动引擎（现在还用不到它）。")
        elif session is None:
            st.error("引擎不可用。")
            st.code(engine_mod.doctor_failure_hint(), language="text")
        elif session is not None:
            st.markdown(
                f"- pid `{session.pid}` · 重建 `{session.restarts}` 次\n"
                f"- db `{session.display_url()}`\n"
                f"- rpc {session.stats.calls} 次 / 错误 {session.stats.errors} 次"
            )
            st.caption(f"引擎日志：{session.log_path}")
            columns = st.columns(2)
            if columns[0].button("健康检查", key="engine_ping"):
                if session.heartbeat():
                    st.success("ping 成功。")
                else:
                    st.error("ping 失败：引擎可能已经死了，点「重启引擎」。")
            if columns[1].button("重启引擎", key="engine_restart"):
                try:
                    session.restart("用户手动重启")
                except Exception as exc:  # noqa: BLE001 - 重建失败也要能看
                    st.error(f"重建失败：{exc}")
                else:
                    st.rerun()

        with st.expander("packetsage doctor（十项自检）", expanded=False):
            if st.button("重新自检", key="doctor_refresh"):
                engine_mod.doctor_report.clear()
                st.rerun()
            views.doctor_panel(doctor)


def _load_task(task_id: str) -> None:
    """切任务：清掉上一轮的 log / 报告 / 预算（一次只跟一个 task）。"""
    if not task_id:
        return
    st.session_state.task_id = task_id
    st.session_state.task_summary = {}
    st.session_state.log = []
    st.session_state.report = None
    st.session_state.last_budget = None
    st.session_state.notice = f"已载入 {task_id}（首次访问会按源抓包冷恢复该任务）"
    st.rerun()


def _start_analyze(session: engine_mod.EngineSession, capture: Path) -> None:
    st.session_state.analyze_job = jobs.start_analyze(session, capture)
    st.session_state.report = None
    st.rerun()


def _start_run(
    session: engine_mod.EngineSession | None,
    env: AgentEnv,
    task_id: str,
    goal: str,
    mode: str = "run",
) -> None:
    if session is None or not task_id:
        return
    try:
        job = jobs.start_run(session, env, task_id, goal, mode=mode)
    except Exception as exc:  # noqa: BLE001 - 装配失败（没 key / 坏配置）也要说清楚
        st.session_state.log.append({"role": "error", "message": jobs.error_message(exc)})
        return
    st.session_state.job = job
    st.session_state.log.append({"role": "user", "text": goal, "mode": mode})


# --------------------------------------------------------------------------
# 后台任务的推进（脚本线程唯一的渲染点）
# --------------------------------------------------------------------------
def _drive_analyze(session: engine_mod.EngineSession) -> None:
    job: jobs.AnalyzeJob = st.session_state.analyze_job
    placeholder = st.empty()
    while True:
        with placeholder.container():
            st.markdown(f"⏳ **正在解析抓包**（`analyze_file`）· 已用 {job.elapsed_s:.1f}s")
            st.caption(str(job.path))
            st.progress(0.0, text="解析完成后会显示包数、会话数与任务摘要")
        if job.done.is_set():
            break
        time.sleep(jobs.POLL_INTERVAL_S)
    st.session_state.analyze_job = None
    _task_rows.clear()
    if job.error is not None:
        st.session_state.log = []
        st.session_state.log.append(
            {"role": "error", "message": jobs.error_message(job.error), "retryable": True}
        )
        st.session_state.notice = "分析失败：见下面的错误（修好后可以重试，不用重启引擎）"
    else:
        st.session_state.task_id = job.task_id
        st.session_state.task_summary = job.summary
        st.session_state.log = []
        st.session_state.report = None
        st.session_state.notice = (
            f"分析完成：{job.summary.get('packets', '-')} 包 / "
            f"{job.summary.get('sessions', '-')} 会话 / {job.summary.get('alerts', '-')} 告警"
        )
    st.rerun()


def _render_live_run(job: jobs.RunJob, placeholder: Any) -> None:
    """一帧实时画面：预算条 + 逐条出现的工具卡片（S58/S61）。"""
    with placeholder.container():
        state = job.live_state()
        traces = job.live_traces()
        label = "降级收尾中…" if job.stop_requested else "调查进行中…"
        st.markdown(f"⏳ **{label}** · 已用 {job.elapsed_s:.1f}s · 工具调用 {len(traces)} 次")
        if job.detail:
            st.caption(job.detail)
        views.budget_bar(state, job.budget, live=True)
        if not traces:
            st.caption("模型正在决定第一个动作；工具调用会一条条出现在这里（不是等一整轮刷新）。")
        for index, entry in enumerate(traces, start=1):
            views.tool_card(dict(entry), index, expanded=index == len(traces))


def _pump_run(job: jobs.RunJob) -> None:
    """轮询到 run 结束；每帧都经过 ``st.*``，所以重跑请求能在帧边界被接住。"""
    placeholder = st.empty()
    while True:
        _render_live_run(job, placeholder)
        if job.done.is_set():
            break
        time.sleep(jobs.POLL_INTERVAL_S)
    _render_live_run(job, placeholder)


def _finding_rows(result: Any) -> list[dict[str, Any]]:
    """把 ``AgentRunResult`` 的结论与引擎分配的 ``F-{n:03}`` 对齐（M8）。"""
    stored = [row for row in (getattr(result, "stored_findings", None) or []) if isinstance(row, dict)]
    rows: list[dict[str, Any]] = []
    for index, draft in enumerate(getattr(result, "findings", []) or []):
        row = stored[index] if index < len(stored) else {}
        rows.append(
            {
                "finding_id": str(row.get("finding_id") or ""),
                "severity": str(getattr(draft, "severity", "")),
                "basis": str(getattr(draft, "basis", "")),
                "title": str(getattr(draft, "title", "")),
                "summary": str(getattr(draft, "summary", "")),
                "validator_status": str(row.get("validator_status") or "accepted"),
                "evidence": [ref.as_dict() for ref in getattr(draft, "evidence", []) or []],
            }
        )
    return rows


def _absorb_run(job: jobs.RunJob) -> None:
    st.session_state.job = None
    if job.error is not None:
        st.session_state.log.append(
            {
                "role": "error",
                "message": jobs.error_message(job.error),
                "retryable": jobs.error_retryable(job.error),
            }
        )
        st.session_state.notice = "调查中断：见下面的错误"
    else:
        result = job.result
        rows = _finding_rows(result)
        st.session_state.log.append(
            {
                "role": "assistant",
                "goal": job.goal,
                "mode": job.mode,
                "status": str(getattr(result, "status", "")),
                "stop_reason": getattr(result, "stop_reason", None),
                "notes": list(getattr(result, "notes", []) or []),
                "malformed": bool(getattr(result, "malformed_output", False)),
                "run_id": str(getattr(result, "agent_run_id", "")),
                "model": str(getattr(result, "model", "")),
                "prompt_version": str(getattr(result, "prompt_version", "")),
                "submit_rejects": int(getattr(result, "submit_rejects", 0) or 0),
                "findings": rows,
                "trace": [views.record_to_dict(record) for record in getattr(result, "trace", []) or []],
                "state": job.live_state(),
                "budget": job.budget,
                "interrupted": bool(job.stop_requested),
            }
        )
        st.session_state.last_budget = job.live_state()
        st.session_state.notice = ""
    st.rerun()


# --------------------------------------------------------------------------
# 主区
# --------------------------------------------------------------------------
def _render_log_entry(entry: dict[str, Any]) -> None:
    role = entry.get("role")
    if role == "user":
        with st.chat_message("user"):
            st.markdown(str(entry.get("text") or ""))
            if entry.get("mode") == "chat":
                st.caption("追问（chat 模式系统提示词）")
        return
    if role == "error":
        with st.chat_message("assistant", avatar="⚠️"):
            st.error(str(entry.get("message") or "未知错误"))
            if entry.get("retryable"):
                st.caption("这一类错误可以重建引擎后重试；配置类错误（exit 3）要先跑 setup。")
        return
    with st.chat_message("assistant"):
        status = str(entry.get("status") or "")
        heading = "结论" if status in {"ok", "completed"} else f"结论（{views.STATUS_LABEL.get(status, status)}）"
        st.markdown(f"**{heading}** · {_mono(entry.get('run_id') or '-')} · 模型 `{entry.get('model')}`")
        if status in {"degraded", "failed"}:
            views.degradation_notice(
                status=status,
                stop_reason=entry.get("stop_reason"),
                notes=entry.get("notes") or [],
                malformed=bool(entry.get("malformed")),
            )
        elif entry.get("notes"):
            for note in entry["notes"]:
                st.warning(f"WARN {note}")
        if entry.get("submit_rejects"):
            st.warning(f"{entry['submit_rejects']} 条结论被引擎的证据校验拒绝（V1-V4）。")
        findings = entry.get("findings") or []
        if not findings:
            st.info("这一轮没有产生 accepted 结论（模型没提交，或被 V1-V4 拒绝）。")
        for finding in findings:
            st.markdown(
                f"**{finding.get('finding_id') or '（未落库）'} {finding.get('title')}** — "
                f"{views.severity_badge(str(finding.get('severity')))} · "
                f"{views.basis_label(str(finding.get('basis')))}"
            )
            st.markdown(str(finding.get("summary") or ""))
            anchors = [
                f"{_mono(ref.get('_id'))}(`{ref.get('method')}`)"
                for ref in finding.get("evidence") or []
            ]
            if anchors:
                st.markdown("证据锚点：" + " · ".join(anchors))
            st.divider()
        if entry.get("state"):
            views.budget_bar(entry["state"], entry.get("budget"))
        trace = entry.get("trace") or []
        if trace:
            st.markdown(f"**已使用 {len(trace)} 次工具调用**")
            views.tool_cards(trace)


def _task_data(session: engine_mod.EngineSession, task_id: str) -> dict[str, Any]:
    """一次取齐任务面板需要的五份数据；每一份失败都单独记账（§5.4 规矩 1）。"""
    data: dict[str, Any] = {"errors": []}
    loaders = (
        ("summary", lambda: engine_mod.capture_summary(session, task_id), {}),
        ("alerts", lambda: engine_mod.alerts(session, task_id), []),
        ("findings", lambda: engine_mod.findings(session, task_id), []),
        ("trace", lambda: engine_mod.trace_entries(session, task_id), []),
        ("artifacts", lambda: engine_mod.artifacts(session, task_id), {}),
    )
    for name, loader, fallback in loaders:
        try:
            data[name] = loader()
        except Exception as exc:  # noqa: BLE001 - 每条都单独呈现
            data[name] = fallback
            data["errors"].append((name, exc))
    return data


def _render_task_panels(
    session: engine_mod.EngineSession, env: AgentEnv, task_id: str
) -> None:
    data = _task_data(session, task_id)
    if data["errors"]:
        for name, exc in data["errors"]:
            st.error(f"取 `{name}` 失败：{jobs.error_message(exc)}")
        columns = st.columns([1, 3])
        if columns[0].button("重建引擎后重试", key="rebuild_engine"):
            try:
                session.restart("从面板重建")
            except Exception as exc:  # noqa: BLE001
                st.error(f"重建失败：{exc}")
            else:
                st.rerun()
        return
    summary = data["summary"]
    if summary:
        st.session_state.task_summary = {
            key: summary.get(key) for key in ("packets", "sessions", "alerts", "decode_errors")
        }
    st.markdown("#### 任务摘要")
    views.capture_metrics(summary)

    st.divider()
    st.markdown("#### 规则告警（引擎视角）")
    views.alerts_panel(data["alerts"])

    st.divider()
    st.markdown("#### 证据链（结论 → tc → 工具调用）")
    views.evidence_panel(data["findings"], data["trace"])
    with st.expander("按 tc_id 查台账 / 分析轨迹（§9，不含思维过程）"):
        views.evidence_anchor_picker(data["trace"])
        views.trace_table(data["trace"])

    st.divider()
    st.markdown("#### 报告")
    _render_report_section(session, env, task_id, data["artifacts"])


def _render_report_section(
    session: engine_mod.EngineSession,
    env: AgentEnv,
    task_id: str,
    artifacts: dict[str, Any],
) -> None:
    report = st.session_state.get("report")
    columns = st.columns([1, 3])
    if columns[0].button("生成报告", key="make_report", disabled=_active_job() is not None):
        with st.spinner("收集九节数据并跑反幻觉 lint…"):
            report = _build_report(session, env, task_id, artifacts)
        st.session_state.report = report
        st.rerun()
    meta = artifacts.get("report_meta") or {}
    if meta and not report:
        columns[1].caption(
            f"引擎里已有一份报告记录：{meta.get('report_path', '-')}（"
            f"{meta.get('status', '-')}，模板 {meta.get('template_version', '-')}）"
        )
    views.report_panel(report)


def _build_report(
    session: engine_mod.EngineSession,
    env: AgentEnv,
    task_id: str,
    artifacts: dict[str, Any],
) -> dict[str, Any]:
    """跑一次 ``ReportGenerator``；反幻觉硬失败按 exit 4 的语义原样呈现。"""
    from packetsage_agent.report import AntiHallucinationError, ReportGenerator

    meta = artifacts.get("report_meta") or {}
    last = st.session_state.get("last_budget") or {}
    path = REPORTS_DIR / f"gui-{task_id}.md"
    generator = ReportGenerator(
        session.agent_client(),
        task_id,
        str(meta.get("agent_run_id") or ""),
        model=str(meta.get("model") or (env.model if env.ok else "unknown")),
        provider=str(meta.get("provider") or (env.provider if env.ok else "unknown")),
        temperature=float(meta.get("temperature") or 0.0),
        tokens_in=int(meta.get("tokens_in") or last.get("tokens_in") or 0),
        tokens_out=int(meta.get("tokens_out") or last.get("tokens_out") or 0),
        cost_cents=int(meta.get("cost_cents") or last.get("cost_cents") or 0),
        prompt_version=str(
            meta.get("prompt_version") or getattr(env.settings, "prompt_version", None) or ""
        ),
    )
    try:
        written = generator.generate(path)
    except AntiHallucinationError as exc:
        st.error(
            "反幻觉硬失败（exit 4）：报告的 Executive Summary 引用了引擎从未产生的内容，"
            f"报告未写出。\n\n{exc}"
        )
        return None
    except Exception as exc:  # noqa: BLE001 - 引擎类错误照原样显示
        st.error(f"报告生成失败：{jobs.error_message(exc)}")
        return None
    text = Path(written.path).read_text(encoding="utf-8")
    return {
        "text": text,
        "meta": {
            "path": written.path,
            "file_name": Path(written.path).name,
            "sha256": written.sha256,
            "degraded": written.degraded,
            "unverified_count": written.unverified_count,
            "status": written.status,
            "template_version": written.template_version,
            "prompt_version": written.prompt_version,
            "model": written.model,
            "provider": written.provider,
            "tokens_in": written.tokens_in,
            "tokens_out": written.tokens_out,
            "cost_cents": written.cost_cents,
        },
    }


def _render_gate(title: str, detail: str, code: str, hint: str = "") -> None:
    """首屏阻塞项：说清楚"哪里不对 + 复制哪条命令"（§5.1/U7）。"""
    st.error(f"**{title}**\n\n{detail}")
    st.code(code, language="text")
    if hint:
        st.markdown(hint)
    st.caption("修好后刷新页面即可；界面不会在未配置的情况下静默改用 mock。")


def _main() -> None:
    st.set_page_config(
        page_title="PacketSage · 证据优先的抓包调查",
        page_icon="🛡️",
        layout="wide",
        initial_sidebar_state="expanded",
    )
    _init_state()

    env = load_agent_env()
    doctor = engine_mod.doctor_report()
    doctor_provider = engine_mod.doctor_item(doctor, "provider")
    blocked = provider_blocked(env, str(doctor_provider.get("status", "")))

    session: engine_mod.EngineSession | None = None
    engine_notice = ""
    if not env.error and not blocked:
        session, engine_notice = engine_mod.engine_or_rebuild(DB_URL)

    _render_sidebar(session, env, doctor, blocked, engine_notice)

    st.title("🛡️ PacketSage")
    st.caption(
        "证据优先的抓包调查：结论 → 证据锚点 `tc_*` → 工具调用，每一段都能点开；"
        "降级与预算永远显示出来。"
    )
    notice = st.session_state.get("notice")
    if notice:
        st.info(notice)

    if env.error:
        _render_gate(
            "配置读取失败",
            env.error,
            "$PACKETSAGE_CONFIG 或 packetsage.yaml",
            "修好配置文件后刷新。",
        )
        st.stop()
    if blocked:
        _render_gate(
            "provider 还没有配置好",
            blocked,
            SETUP_COMMAND,
            "`setup` 会把 provider / model / base_url / API key 写进 `agent/.env`"
            "（gitignored，永不进 config、不进 git）。",
        )
        st.stop()
    if session is None:
        _render_gate(
            "引擎不可用",
            engine_notice or "无法启动 `packetsage serve`。",
            "powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release",
            "或者把 `packetsage` 放进 PATH / 设 `$PACKETSAGE_ENGINE`。",
        )
        st.stop()

    analyze_job: jobs.AnalyzeJob | None = st.session_state.get("analyze_job")
    if analyze_job is not None:
        _drive_analyze(session)

    run_job: jobs.RunJob | None = st.session_state.get("job")
    if run_job is not None and run_job.done.is_set():
        _absorb_run(run_job)

    st.divider()
    st.markdown("## 调查")
    log = st.session_state.get("log") or []
    task_id = st.session_state.get("task_id") or ""
    if not log:
        views.empty_state(has_task=bool(task_id))
    for entry in log:
        _render_log_entry(entry)

    run_job = st.session_state.get("job")
    if run_job is not None and not run_job.done.is_set():
        columns = st.columns([1, 3])
        if columns[0].button("⏹ 停止调查", key="stop_run"):
            if run_job.request_stop():
                st.success(run_job.detail)
        columns[1].caption(
            "停止 = 下一个边界收尾：当前工具调用跑完就停，已提交的结论保留"
            "（`request_finalize()`）。想立刻掐断就用页面右上角的 STOP。"
        )
        _pump_run(run_job)
        _absorb_run(run_job)

    if task_id:
        st.divider()
        st.markdown(f"## 任务面板 · {_mono(task_id)}")
        _render_task_panels(session, env, task_id)
    else:
        st.divider()
        st.caption("分析一份抓包或载入一个历史 task 之后，这里会出现任务摘要、规则告警、证据链与报告。")

    goal = st.chat_input(
        "追问或换个调查目标（例如：哪些会话不完整？）",
        key="chat_goal",
        disabled=_active_job() is not None or not task_id,
    )
    if goal:
        _start_run(session, env, task_id, goal, mode="chat")
        st.rerun()


# Streamlit 用 `__main__` 执行脚本（`streamlit run` 与 AppTest 都是），所以导入
# `gui.app` 只拿到函数，不会顺手把界面跑一遍。
if __name__ == "__main__":
    _main()
