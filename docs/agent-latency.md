# Agent 往返延迟（"太慢了"那一轮）

> 结论先写：**慢的是串行的 LLM 往返，不是工具**。一次 run 里工具总耗时
> **10 ms** 量级，其余全在模型往返。所以修法只有两个方向——**减少往返次数**、
> 让每次往返更容易命中 provider 的上下文缓存。

## 1. 先测：`llm_ms` / `tool_ms` 分列

`PolicyState` 现在累计 `llm_ms`（`provider.decide` 的墙钟）与 `tool_ms`
（引擎 RPC 的 `duration_ms`），`llm_round` 事件把它们一起播出去（§4.5 字段**只增**，
快照见 `tests/sidecar/protocol_v1.json`），每条台账行也带 `llm_ms`——即"这次调用
是等了多久的模型才发出来的"。

不复用这一层就会优化错地方：慢数据库和慢模型在界面上长得一模一样。

## 2. 怎么量（没有真 key 也能量）

```bash
cd agent && python tests/bench_roundtrips.py --ttft 1.5
```

引擎用真 `packetsage serve`（工具耗时是真的），provider 用 `mock` +
`$PACKETSAGE_MOCK_LATENCY_S` —— 每次"模型往返"固定睡 `ttft` 秒，模拟真实 provider
的首字延迟。两组对照唯一的差别就是往返次数：

* **探索式**：`preload=off` + 默认脚本（摘要/统计/会话/告警都要模型自己点）；
* **预取式**：`preload=on` + `lean` 脚本（事实已在第一条消息里，只按需 pivot）。

## 3. 实测（2026-09-21，本机，`--ttft 1.5`）

| 方案 | 模型往返 | 步数 | 工具调用 | 模型耗时 | 工具耗时 | 墙钟 | 结论 |
|---|---:|---:|---:|---:|---:|---:|---|
| 探索式 | 9 | 9 | 8 | 13500 ms | 10 ms | **13571 ms** | 现状 |
| 预取式 | 4 | 4 | 3 | 6000 ms | 13 ms | **6075 ms** | −55% |

两组都拿到 2 条结论、证据有效性与 `path_match` 不变（`E1–E6` mock 6/6 ok，
`path_match 1.00`、`forged_refs 0`）。

**口径提醒**：`ttft` 是模拟值，绝对值没有意义；有意义的是**往返次数的乘数效应**
——真实 provider 下墙钟 ≈ 往返次数 ×（TTFT + 输出时间）。真 key 的 E1–E6 复测要单独
跑（要预算），本文件不含真模型数字。

## 4. 改了什么

| # | 改动 | 位置 |
|---|---|---|
| 1 | LLM / 工具耗时分开记账并播出去 | `policy.py`（`llm_ms`/`tool_ms`/`preloaded`）、`agent.py`、`serve.py::EventSink.llm_round` |
| 2 | **预取确定性事实**：第一次模型往返之前就把摘要、规则告警、协议统计、Top 会话取回来（同一批工具、同一本台账、真 `tc_*` 锚点）写进第一条消息，模型从"分析"起步而不是"探索" | `preload.py`、`agent.py::run`、`provider.py` 第一条 user 消息 |
| 3 | **一轮多工具**：模型在同一轮里请求的多个工具全部执行（原来只跑第一个） | `provider.py::Decision.calls` / `OpenAIProvider.decide`、`agent.py::_run_tool` |
| 4 | **缓存友好的 prompt**：system prompt / 工具清单 / 预取事实 / goal / 收尾格式全部进**第一条**消息，易变历史追加在后面——每轮都在上一轮基础上追加，前缀不再被尾部的 FINALIZE 消息冲掉 | `provider.py::decide`（原来 `FINALIZE_INSTRUCTION` 挂在尾部，等于每轮让缓存从第 3 条消息起全部失效） |
| 5 | 两阶段观感：结论在 run 进行中就逐条落到结论卡（`finding_accepted` → 界面），报告是单独的按需动作 | `desktop/src/App.tsx`（前一轮已做） |

## 5. 还没做

* **真模型 E1–E6**：`--provider deepseek` + 真 key 的复测（要预算与你的点头）；
  它会验证两件事：往返次数真的降到 3–4，以及"没有尾部 FINALIZE 提醒"时模型仍会收尾。
* **流式输出（stream=True）**：把 TTFT 变成"看得见的字"，感知延迟还能再降一档；
  但会改动 provider 的解析路径（`tools` + 流式 function calling 的拼装），单独排期。
* **大文件分块**：见 `docs/adr/` 里关于 capture 规模的条目；本轮的预取上限是
  `preload.FACTS_MAX_CHARS = 6000`。
