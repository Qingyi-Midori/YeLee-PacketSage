# ADR-027：token 级流式（`llm_delta`）

**状态：已采纳**（用户 2026-09-22 明确授权改规格："要做真 token 级流式，规格书会改来改去很正常"）

**背景：**《GUI 工程规格书 v0.2》§4.5 规则 3 原本写的是**不做 token 级流式**，理由是
`provider.OpenAIProvider` 是单次 `httpx.post`、没有 SSE，事件粒度只能是"每次工具调用 +
每轮 LLM"。界面因此只能在"一轮跑完"之后才显示模型这一轮做了什么：等待期间屏幕上
只有一个进度条，最长的一次实测往返是 7.6 s（`samples/synth-mixed.pcap`，deepseek-chat）。

**决议：** provider 支持 SSE，agent 把碎块合并成新事件 `llm_delta`，三条边界写死：

1. **合并粒度**：agent 侧按 **120 ms 或 256 字符**（先到者）flush 一帧；单条事件仍受 §4.5
   的 8 KiB 截断约束。粒度是"看起来在打字"和"别把 stdout / 事件队列灌满"之间的折中；
2. **装饰帧**：`llm_delta` 丢了不影响任何结论、证据与预算。外壳事件队列（容量 256）满时
   **先丢装饰帧且不计入 `dropped`**，`events_dropped` 仍然只表示"真事件少了"；
3. **不改变计价**：只有实现了 `llm_delta` 钩子的 observer（桌面外壳的 `EventSink`）才会触发
   SSE；CLI 的 `Progress` 没这个钩子 → 仍走单次 POST，命令行输出字节级不变。端点不支持
   SSE（4xx）或流里没给 `usage` 时，agent 自动退回整块路径；缺 `usage` 的那一轮 tokens **记 0**
   （不编数字）并写进 run 的 notes。

界面侧的证据链纪律不变：`llm_delta` 只是"模型正在写"，**不是**证据——结论仍然只能由
`finding_accepted` 提交、由引擎验证后落地（V1–V4 与 ADR-024/026 不受影响）。

**证据（2026-09-22）：**

* `agent/tests/test_provider.py` 新增 8 项：SSE 解析（工具调用分片、正文分片）、
  `stream_options` 被拒后去掉重试、端点拒流后回退整块、缺 usage 时记 0 并停止流式、
  合并粒度、跨工具切段、mock 单帧、CLI 无钩子时零输出；`pytest -q` → 169 passed；
* `tests/sidecar/protocol_cases.py::S70`（协议快照 + 真 run 字段对拍）覆盖 `llm_delta`；
* `desktop/src-tauri/src/jsonl.rs` 的 `event_queue_gives_up_streaming_frames_before_real_events`
  断言"装饰帧先走、计数不动、真事件顺序不变"；
* 版式与手感：`desktop/src/App.tsx` 的 `llm_delta` 分支 + `FeedLine` 的 delta 渲染
  （跑的时候只画尾巴并带光标，一轮结束后折成「模型原文」）。

**实现落点：** `agent/packetsage_agent/provider.py`（`DeltaCoalescer` / `_decide_streaming` /
`_read_stream` / `_decision_from_message`）、`agent.py::_delta_hook`、`serve.py::EventSink.llm_delta`、
`desktop/src/{App.tsx,components.tsx,types.ts,styles.css}`、`desktop/src-tauri/src/jsonl.rs`。

**代价与已知边界：**

* 流式路径拿不到 usage 的网关，那一轮 tokens 记 0（预算条会偏低），run 的 notes 里留痕；
* 取消语义不变（§4.7）：SSE 让"本轮中途打断"在技术上可行，但本轮**没有**做——`cancel`
  仍然在下一个边界收尾，UI 照旧显示"正在收尾…"。

## 补充（2026-09-22，同一轮的后续改动）

用户指出两处漏掉的官方能力，核对指南后确认属实，一并落地：

1. **思考模式**（[thinking_mode](https://api-docs.deepseek.com/zh-cn/guides/thinking_mode)）：
   DeepSeek **默认就开**，`reasoning_content` 与 `content` 同级返回。provider 现在把它
   累进 `Decision.reasoning` 并走 `llm_delta` 的 `reasoning` 泳道（界面「思考」）。
   更关键的是官方那条约束：**带 `tools` 时历史轮次的 `reasoning_content` 必须回传**
   （会被拼进上下文）——不回传等于模型每轮从零想，也跟前缀缓存的实际前缀不一致。
   回传的那一份挂在 `trace_payload` 条目上，**只活在内存**：`result.trace` 与引擎台账
   都不含思维链，ADR-007 不变。`PACKETSAGE_LLM_THINKING=disabled` 可关掉（省钱/加速）。
2. **上下文硬盘缓存**（[kv_cache](https://api-docs.deepseek.com/zh-cn/guides/kv_cache)）：
   `usage.prompt_cache_hit_tokens / prompt_cache_miss_tokens` 现在一路带到
   `llm_round`（每轮累计）与 `run_finished.cache{hit,miss}`，界面顶栏给一个「缓存 N%」，
   每轮的账目行给「缓存 48%（2.0k/4.3k 命中）」，CLI 的 usage 行在同名数据存在时追加
   `· cache N%`（没有数据就不显示——"不报"和"没命中"必须分得开）。
