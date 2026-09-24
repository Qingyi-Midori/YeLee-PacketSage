# GUI 工程规格书 v0.2

> 定位：PacketSage **桌面应用**的唯一权威文档——技术栈与架构、进程与数据交换协议、
> 待关闭项（U1–U10）、设计基线、打包分发、自动与人工验收。
>
> 交付形态：**Tauri 桌面应用，双击安装即用**；目标机不需要 Python / Rust / Node。
>
> 与《[GUI 前收口文档 v0.1](GUI%20前收口文档%20v0.1.md)》的分工：收口文档管"后端已经冻结了什么契约"，
> 本文档管"桌面应用怎么用这些契约、做成什么样、怎么打包"。
>
> 本文件只写文档，不改代码。

---

## 0. 文档元信息

| 项 | 内容 |
|---|---|
| 版本 | v0.3（v0.1 的 Streamlit 方案已作废；文件名保持 v0.2，版本以本行与 §10 为准） |
| 日期 | 2026-09-22 |
| 状态 | U1–U11 逐条见 §3（U2 / U6 / U9 仍标未关闭） |
| 关键决策 | **Tauri**；交付物是**可双击安装的 Windows 桌面应用**；Rust/Python 分工保持不变（引擎 Rust、Agent Python） |
| 时间预算 | ≥13 周 |
| 上游契约 | 《GUI 前收口文档 v0.1》§5，尤其 §5.2 退出码 / §5.4 RPC 方法与信封 / §5.11 `run --json` 终态对象 / §5.12 Python 侧模块面 |
| 本机环境 | Windows 11 26200 / rustc 1.98.1 `x86_64-pc-windows-gnu`（**Tauri 需补 MSVC target**，见 §7.7）/ node+npm 已装 / WebView2 153.x 已装 |
| 现存资产 | `gui/`（Streamlit 原型，约 71KB Python，可用）——见 §1.3 迁移关系 |

---

## 1. 定位与范围

### 1.1 交付形态变了，几乎是别的项目

v0.1 的目标是"跑起来给人看"（`streamlit run` + 浏览器标签页）。v0.2 的目标是
**像 LM Studio / Claude Desktop 那样：拿到一个安装包，双击，装完就能用**。

这一条决定了后面几乎所有选择：

| 由交付形态推出的约束 | 后果 |
|---|---|
| 目标机**没有** Python / Rust / Node | Agent 必须打成独立可执行文件（PyInstaller sidecar），引擎二进制随包分发 |
| 界面不能是浏览器标签页 | Tauri 原生窗口（WebView2 内核，系统自带） |
| IPC 不能依赖"同进程 import" | **必须定义一套 GUI ↔ Agent 的进程间协议** → §4.5，本版最大新增项 |
| 密钥不能散在 `agent/.env` | 存进 Windows 凭据管理器，启动时以环境变量交给 sidecar |
| 卸载/升级要有明确行为 | 安装到 `%LOCALAPPDATA%\Programs`，用户数据（库、报告、日志）默认保留 |

### 1.2 与收口文档的分工

| 问题 | 归属 | 验收方式 |
|---|---|---|
| 后端已经冻结了什么契约？ | 《GUI 前收口文档 v0.1》§3、§5 | 命令 + 退出码 + 契约快照测试 |
| 桌面应用怎么做？需要哪些**新**接口？ | **本文档** §2–§8 | U1–U10、S57–S70、人工验收 M1–M24 |

治理规则：**GUI 需要但尚不存在的新接口，先在本文档定义并冻结，再回填《收口文档》§5**
（本版新增的是 §4.5 的 Agent sidecar 协议）。这样"已冻结的"与"待冻结的"不会混在一起。

### 1.3 现存资产：Streamlit 原型的迁移关系

`gui/` 已经是一个**能用的 Streamlit 实现**（分析 → 调查 → 证据链 → 报告全链路）。它的**设计成果继续有效**，
渲染与线程模型的实现方式作废：

| 现存文件 | 内容 | Tauri 下的处置 |
|---|---|---|
| `gui/views.py`（19.5KB） | 全部面板的信息结构：`tool_card` / `budget_bar` / `alerts_panel` / `evidence_panel` / `degradation_notice` / `report_panel` / `doctor_panel` / `empty_state` / `trace_table` | **保留为规格输入**：前端每个面板展示哪些字段，照它做（§5.1–§5.2） |
| `gui/engine.py`（19.7KB） | `EngineSession`（一进程一 `serve`、重启、心跳、stderr tail）+ 类型化包装 `capture_summary` / `protocol_stats` / `conversations` / `alerts` / `findings` / `trace_entries` / `artifacts` / `doctor_report` | **保留为规格输入**：这定义了"GUI 需要引擎哪些方法、什么参数"，Rust 侧按它实现（§4.4） |
| `gui/jobs.py`（8KB） | 后台线程 + 队列、`request_finalize()` 停止语义、终态不吞异常 | **语义保留，实现作废**：Tauri 用 Rust 异步任务 + Channel 代替线程 + 轮询 |
| `gui/app.py`（29KB，169 处 `st.*`） | 页面编排与渲染 | **作废**：换成 React 前端 |
| `gui/paths.py` / `gui/settings.py` | 仓库路径锚点、provider 读取、预算解析 | 部分作废：打包后没有"仓库"，路径改为 app data 目录 |
| `tests/gui/app_cases.py`（20.4KB，CI 里真跑） | Streamlit 界面的行为断言（全链路 + 停止 + 引擎崩溃重建 + provider 门禁） | **保留为验收输入**：这些断言描述的是"界面必须做到什么"，Tauri 版要逐条对应（映射到 §8.1 的 S 系列） |
| `agent/pyproject.toml` 的 `gui = [streamlit]` 可选组 | 原型依赖 | 保留给原型；**不再是交付物依赖** |

**建议**：Streamlit 版冻结为**参考原型**（仓库保留，README 标注"原型，非交付物"），
Tauri 版达到功能对等后再决定删除。两份同时演进必然漂移，所以原型只修 bug、不加功能。

---

## 2. 技术栈与架构

### 2.1 选型

| 层 | 选择 | 理由 |
|---|---|---|
| 外壳 | **Tauri 2**（Rust） | 体积小（复用系统 WebView2）、原生窗口、Rust 侧能管进程与 IPC；与项目 Rust 主力一致 |
| 前端 | **React + TypeScript + Vite**，Tailwind 做样式 | 主流 Agent 桌面端的共同选择，聊天流 + 流式组件生态最成熟 |
| 引擎 | **既有 `packetsage` 二进制**，以 `serve` 模式作 sidecar | 契约已冻结、9 工具与不变量都测过，不重写 |
| Agent | **既有 Python 包**，新增 `serve` 模式作 sidecar | 保留 Rust/Python 分工 |
| 传输 | **stdio JSONL**（§4.2） | 无端口、无防火墙弹窗、无 localhost 鉴权问题、父进程独占、EOF 即崩溃信号 |
| 状态存储 | 引擎的 SQLite（已存在） | 任务/会话/告警/finding/台账都在库里，界面只读不另存 |

### 2.2 进程与职责模型

```text
┌─────────────────────────────────────────────────────────────┐
│  packetsage-desktop.exe  (Tauri 外壳, Rust)                  │
│   ├── SidecarManager：两个子进程的生命周期、读写、重启         │
│   ├── IPC 命令注册（analyze / run_agent / cancel / query_*）  │
│   └── WebView2：React 前端                                    │
└───────┬──────────────────────────────────┬──────────────────┘
        │ A: JSONL RPC                     │ B: JSONL 协议（新增）
        │（收口 §5.4，已冻结）               │（§4.5，本版定义）
        ▼                                  ▼
┌──────────────────────┐        ┌──────────────────────────┐
│ packetsage serve     │        │ packetsage-agent serve   │
│ (Rust 引擎, sidecar)  │        │ (Python Agent, sidecar)   │
│  · 解析/规则/重组     │◄───────┤  · 工具循环 / policy      │
│  · 内存态 + 冷恢复    │ 通道 A  │  · provider / 报告生成    │
│  · tc 台账 / finding │ 同样适用 │  · 不碰 SQL，不碰解析     │
└──────────┬───────────┘        └──────────────────────────┘
           │ sqlx
           ▼
      SQLite（唯一真相源）
```

**核心原则：证据路径不经过 Python。**
界面读摘要、会话、告警、finding、台账、报告素材，一律**直连引擎**（通道 A，Rust → Rust）。
Python 只负责"下一步查什么、怎么解释"与报告渲染。这不是技术洁癖，而是项目核心
（"每个数字都能回溯到某次工具调用"）的直接推论：让证据少跳一次进程，就少一次漂移机会。

两个 sidecar 的关系：**Agent 自己也要连引擎**（工具调用走同一条协议，只是另一个连接）。
引擎允许两个客户端；`check_alerts` 的单源路由（C8）与冷恢复（ADR-019）都在 `serve` 内，
两个客户端看到同一份状态。

### 2.3 为什么不把 `packetsage-core` 直接链进 Tauri

同语言（都是 Rust）确实能 in-process 调用，省一层 IPC。**本版不做**：

1. `serve` 里沉淀着三条不变量：`check_alerts` 单源路由、重放式冷恢复、tc id 的铸造点（ADR-024）。
   在 Tauri 里重做一遍 = 两份实现、两套 bug。
2. 会多出一个 `core` 的消费者 → `core` 的 API 稳定性负担翻倍。
3. 成本收益不成立：612 包解析 0.1s 级，IPC 开销可忽略；而 `serve` 路径已被 43 项契约用例覆盖。

留作将来优化（真要做，应是"把 `serve` 的逻辑抽成库，CLI 与 GUI 共用"，而不是 GUI 直接碰 `core`）。

### 2.4 明确不做

* 不做 macOS / Linux 打包（先 Windows；Tauri 的跨平台能力保留，不投入）；
* 不做多窗口 / 多标签并行任务（一次一个 `task_id`）；
* 不做账号体系、云同步、遥测上报；
* ~~不做 token 级打字机流式~~ → **v0.3 起做**（§4.5 `llm_delta`，U11）；
* 不做插件市场、MCP、模型市场；
* 不做内嵌编辑器 / diff 视图。

---

## 3. 待关闭项（U1–U11）

> U1–U4 是**开工阻塞项（P0）**：不关掉就没有可用的骨架。
> U1 是本版最大的一项——它是"零 IPC 的 Streamlit"变成"双 sidecar 的桌面应用"的质变点。

- [x] **U1**（P0）**Agent sidecar 协议**：给 `packetsage-agent` 增加 `serve` 子命令，stdio JSONL，命令在上、事件在下。**完整规格见 §4.5**；选型理由见 §4.2。
      判据：用脚本喂一串命令，能拿到 `welcome` → `run_started` → 逐条 `tool_call_*` → `run_finished`，`seq` 单调无缺口；`run_finished` 的对象与 `packetsage-agent run --json`（收口 §5.11）**同构**。
      证据（2026-09-20）：`agent/packetsage_agent/serve.py`（命令表 + 事件表 + 错误模型 + 8 KiB 截断 + `LockedClient` 单写者）、`agent/packetsage_agent/payloads.py`（`run_finished` 与 `run --json` 共用一份载荷）、`agent/packetsage_agent/agent.py` 的四个 additive 钩子（`tool_call_started/tool_call_finished/finding_accepted/finding_rejected`，CLI 的 `Progress` 行为不变，Python 148 项全绿）；
      `python tests/sidecar/protocol_cases.py --binary target/release/packetsage.exe` → **8/8**（S57 握手 / S60 cancel / S61 顺序与完整性 / S62 体积 / S63 BUSY / S67 provider / S70 快照 / report 命令）；
      `python -m mypy` → 0 error（`EngineClient` 的消费者改按 `RpcCaller` Protocol 收口）。待用户确认后回填《收口文档》§5（§4.10 第四项）。
- [ ] **U2**（P0）**Agent sidecar 打包**：PyInstaller `--onedir` 产出 `packetsage-agent.exe`，把 `prompts_text/*.txt` 与 `banner.txt` 打进去（`pyproject.toml` 已声明 package-data，spec 要照抄）。
      判据：在一台**没装 Python** 的机器上，`packetsage-agent.exe --version` 与 `serve` 都能跑。
- [x] **U3**（P0）**Tauri 外壳与进程管理**：`SidecarManager` 持有两个子进程，负责 spawn / 读写 / 崩溃检测 / 优雅关闭。要求 stdout EOF 立即判定死亡（不靠轮询）；`close()` 后不留孤儿进程。
      判据：关窗口后任务管理器里 `packetsage.exe` 与 `packetsage-agent.exe` 都消失（M14）。
      证据（2026-09-20）：`desktop/src-tauri/src/jsonl.rs`（读线程 EOF → `alive=false` + `ExitSink` 回调）、`sidecars.rs`（解析顺序：env → 程序旁 sidecar → PATH → 开发树）、`commands.rs::shutdown`（`shutdown` 命令 → close 两个子进程）+ `lib.rs` 的 `WindowEvent::CloseRequested` / `RunEvent::Exit` 钩子；
      `cd desktop/src-tauri && cargo test` → **7/7**（2 单元 + 5 集成；含 `s58_killing_the_agent_is_detected_by_eof`：关闭 agent 后 10s 内拿到退出通知，且后续命令回 "not running"），`cargo clippy --all-targets -- -D warnings` 干净。
- [x] **U4**（P0）**前端流式渲染**：Rust 侧把事件经 Tauri Channel 推给 WebView；React 增量渲染工具卡片。要求 run 期间卡片**逐条出现**，不是结束后一次性刷出。
      判据：mock provider 下，第一条 `tool_call_finished` 到达时 `run_finished` 尚未发生（S61）。
      证据：`lib.rs` 把 agent 事件经 `Channel` 转给前端，`src/App.tsx` 的 `onEvent` 按事件名增量更新（`tool_call_started` 先立卡片、`tool_call_finished` 定稿）；外壳用例 `s61_shell_streams_events_until_run_finished`（真引擎 + mock，断言首末事件与 `seq` 无缺口）与 `s61_tool_cards_are_visible_before_run_finished`（`fake_engine.py slow:0.2` 桩，断言"看见工具卡片时 `run_finished` 尚未发生"）都通过。肉眼确认见 M6。
- [x] **U5**（P1）**取消语义**：GUI 发 `cancel` → Agent `request_finalize()` → 下一个边界收尾。**已知限制**：provider 是单次 HTTP 请求，当前 LLM 轮无法中途打断（收口 §2.4 / S1–S4 #28）。因此 UI 在收到 `run_finished` 之前必须显示"**正在收尾…**"，不能立刻置灰或假装已停。
      证据：`App.tsx` 的 `cancel()` 把状态置为 `finalizing`、按钮文案变「正在收尾…」并保持禁用直到 `run_finished`；协议侧 `tests/sidecar/protocol_cases.py::S60` 与外壳用例 `s60_shell_cancel_finalizes_and_keeps_findings` 都验证 `degraded` + `stop_reason` + 已得结论保留。
- [ ] **U6**（P1）**引擎会话与重建**：`EngineSession` 的 Rust 版（对应 `gui/engine.py`）：一进程一 `serve`、心跳、`stderr_tail`、崩溃后重建。判据：连续 20 次操作后进程数为 2（引擎 + Agent）；杀掉引擎可重建（S59）。
      **现状**：一进程一 `serve` + `stderr_tail` 已有（`jsonl.rs` / `sidecar_diagnostics` 命令）；**心跳与一键重建未做**（界面目前只能看到 `sidecar-exited` 事件）。
- [x] **U7**（P1）**provider 配置与密钥**：首次运行向导 → 校验（`GET /models`）→ 存 Windows 凭据管理器 → 启动时以 `PACKETSAGE_LLM_PROVIDER/MODEL/API_KEY` 注入 sidecar 环境；**不写明文 `.env`**。未配置时禁止启动 run 并引导到向导（延续 Agent CLI §3 的"不静默 mock"约定）。
      证据：界面 `desktop/src/Wizard.tsx`（三步，第 1 步就是 API key）；外壳 `commands.rs::provider_status/provider_verify/provider_save/provider_clear/restart_agent` + `provider.rs`（`provider.json`）+ `secrets.rs`（`CredWriteW`，target `PacketSage:llm-api-key`）；校验复用 Agent 自己的 `probe_provider`（`sidecars.rs::probe_provider` 跑 `setup --verify --print`，不新增协议、不写文件）；打包态用例 `packaged_sidecars.rs::packaged_agent_verifies_provider_configurations`（mock ✅ / 连不上 ❌ / 缺 key 拒绝）与 `::u7_provider_env_flips_the_handshake`（没配 → `configured=false`，注入 → `true`）。
      覆盖口径：`S67`（未配置拒 run）和"没配也能用引擎那半边"都在安装后的真产物上跑过；**M3 的"key 真的进了凭据管理器"仍需你肉眼确认**（控制面板 → 凭据管理器 → Windows 凭据 → `PacketSage:llm-api-key`）。
- [x] **U8**（P1）**数据读取路径**：列表/详情走通道 A 的 RPC（`get_capture_summary` / `get_conversations` / `check_alerts` / `query_history` / `get_task_artifacts`），**不裸 SQL**；确需 SQL 走 `db query --readonly`（url 含 `mode=ro`，语句限 SELECT/WITH/EXPLAIN）。
      证据：`commands.rs::task_overview`（一次取摘要/告警/findings/台账/artifacts，全走 RPC）、`list_tasks` 走 `db query --readonly --jsonl`，`readonly_url()` 负责补 `mode=ro`；外壳用例 `read_only_task_list_uses_the_cli_exit` 覆盖该出口。
- [ ] **U9**（P1）**安装包与首次运行**：MSI/NSIS 安装器、开始菜单与桌面快捷方式、卸载、首次运行向导，详见 §7。判据：干净机器（无 Python/Rust/Node）双击安装 → 启动 → 向导 → 分析 → 报告（M1–M5）。
- [x] **U10**（P1）**协议版本协商**：`hello` / `welcome` 交换 `protocol_version`、`schema_version`、能力集；不匹配时**拒绝启动并给用户可见提示**，不静默降级（S57 / S70）。
      证据：`agent/packetsage_agent/serve.py::_cmd_hello`（不匹配 → `PROTOCOL_MISMATCH` + 拒绝后续命令）+ `tests/sidecar/protocol_cases.py::S57`（匹配/不匹配两条路径）与 `S70`（清单快照 + 真 run 字段对拍）→ 通过。
- [x] **U11**（P1）**token 级流式**（v0.3 新增，用户 2026-09-22 明确授权改规格）：`provider.OpenAIProvider`
      用 SSE（`stream: true`）读模型应答，agent 把碎块合并成 `llm_delta` 事件（§4.5），外壳原样转发，
      前端按"同一步同一路"拼成一段：跑的时候是"正在打字"（正文只有尾巴，末尾一个光标），
      **一轮结束就折成「模型原文」**，屏幕还给结论。
      判据：mock provider 的一次 run 里能看到 `llm_delta`（`channel=tool_args` 对应工具调用、
      `channel=content` 对应收尾 JSON）；端点不支持 SSE 或没给 `usage` 时**自动退回整块请求**，
      预算数字不因此走样（缺 usage 的那一轮 tokens 记 0，并写进 run 的 notes）。
      证据（2026-09-22）：`agent/packetsage_agent/provider.py`（`DeltaCoalescer` + `_decide_streaming`
      + 三条退路）、`serve.py::EventSink.llm_delta`、`agent.py::_delta_hook`（CLI 没有 `llm_delta` 钩子
      → 仍走单次 POST，字节级不变）、`App.tsx` 的 `llm_delta` 分支与 `FeedLine` 的 delta 渲染、
      `jsonl.rs::EventQueue`（满队列先丢装饰帧、不计入 `dropped`）；
      `cd agent; python -m pytest -q` → **169 passed**（新增 8 项流式用例：SSE 解析 / 缺 usage /
      退回整块 / 合并粒度 / mock 单帧）、`python -m ruff check .` 与 `python -m mypy` 干净；
      `tests/sidecar/protocol_cases.py` 的 S70 快照与真 run 字段对拍覆盖 `llm_delta`；
      `cd desktop/src-tauri; cargo test --lib jsonl` → 5/5（含新增的"先丢装饰帧"用例）。

---

## 4. 数据交换协议

> 本章是 v0.2 的核心：从"同进程 import"改成"跨进程协议"之后，**接口定义就是唯一的耦合面**。
> 通道 A 已冻结（复用即可），通道 B 是新增的、必须在本版定稿的。

### 4.1 三条通道

| 通道 | 谁 ↔ 谁 | 传输 | 状态 |
|---|---|---|---|
| **A** | GUI ⇄ 引擎；Agent ⇄ 引擎 | stdio JSONL RPC（16 方法，收口 §5.4） | ✅ 已冻结 |
| **B** | GUI ⇄ Agent sidecar | stdio JSONL（命令 + 事件） | ⬜ **本版定义 → §4.5** |
| **C** | Rust 外壳 ⇄ 前端 WebView | Tauri IPC（`invoke` + `Channel`） | Tauri 自带，只约定事件名 |

### 4.2 为什么用 stdio JSONL 而不是 localhost HTTP/WebSocket

| 维度 | stdio JSONL | localhost HTTP/WS |
|---|---|---|
| 鉴权 | 管道只有父进程能拿到，**天然私有** | 需自己发 token，否则同机任意进程可调 |
| 端口 | 无 | 需选端口、防冲突、防防火墙弹窗 |
| 崩溃感知 | stdout EOF **立即**可知 | 靠连接断开或轮询，慢且偶发 |
| 生命周期 | 与父进程强绑定 | 可能留下孤儿服务 |
| 与既有风格 | 引擎已是这套（收口 §5.4），**一个解析器吃两个 sidecar** | 多一套栈 |
| 代价 | 不能跨机、不能多客户端 | 简单，但本场景不需要 |

结论：**stdio JSONL**。帧规则统一为：UTF-8、`\n` 结尾、一行一个 JSON 对象、不允许内嵌裸换行
（换行必须转义）、行内不得有 ANSI 转义。

### 4.3 握手与版本协商

第一帧必须是 GUI 发出的 `hello`：

```json
{"id":"<uuid4>","type":"cmd","command":"hello",
 "params":{"protocol_version":1,"client":"packetsage-desktop","client_version":"0.1.0"}}
```

Agent 回：

```json
{"id":"<uuid4>","type":"ack","ok":true,"result":{
  "protocol_version":1,"agent_version":"0.1.0","schema_version":2,
  "capabilities":["run","chat","report","cancel","stream_events"],
  "providers":{"kind":"deepseek","model":"deepseek-flash","configured":true}}}
```

规则：

1. `protocol_version` 不等 → 回 `ack.ok=false` + `error.code="PROTOCOL_MISMATCH"`，**拒绝后续命令**；
2. 版本内**只允许加字段**，不允许改字段名或语义；未知字段必须忽略；
3. `capabilities` 用于灰度：GUI 不得调用未声明的能力；
4. `providers.configured=false` 是 GUI 显示首次运行向导的依据（U7）。

### 4.4 通道 A：GUI ↔ 引擎（复用，不改）

方法与信封全部沿用收口文档 §5.4。GUI 实际会用到：

| 用途 | 方法 |
|---|---|
| 分析抓包（落库 + 留在内存） | `analyze_file` |
| 摘要 / 会话 / 统计 | `get_capture_summary` / `get_conversations` / `get_protocol_stats` |
| 告警（规则命中） | `check_alerts` |
| finding 与台账 | `query_history`（`kind=findings` / `kind=trace`） |
| 报告素材 | `get_task_artifacts` |
| 规则清单 | `list_rules` / `check_rules` |
| 存活探测 | `ping` |

超时沿用 `RpcTimeouts`（收口 §5.12）：`ping 5s / query 10s / analyze_file 60s / reconstruct_stream 30s`。

### 4.5 通道 B：GUI ↔ Agent sidecar（新增，本版冻结）

**命令（GUI → Agent）**

| command | params | ack.result | 说明 |
|---|---|---|---|
| `hello` | `protocol_version, client, client_version` | 见 §4.3 | 必须第一帧 |
| `run` | `task_id, goal, mode:"run"\|"chat", prompt_version?` | `{run_id}` | 随后进入事件流；同一时刻只允许一个 run，否则 `BUSY` |
| `chat` | `task_id, question` | `{run_id}` | 语义同 `run(mode="chat")`，单列只为参数更直白 |
| `report` | `task_id, out_path?` | `{report_path, sha256, degraded, unverified_items}` | 生成九节报告（收口 §5.6） |
| `cancel` | `run_id` | `{}` | 下一个边界收尾（U5） |
| `status` | — | `{engine_alive, run:null\|{…}, budget:{…}}` | 重连/重绘后恢复界面 |
| `shutdown` | — | `{}` | 优雅退出；GUI 关闭时发 |

**事件（Agent → GUI，无 `id`，带 `seq`）**

| event | data 字段 | 用途 |
|---|---|---|
| `welcome` | 见 §4.3 | 握手 |
| `run_started` | `run_id, task_id, mode, model, provider, prompt_version, budget{max_steps,max_llm_calls,max_tool_calls,max_tokens,max_cost_cents}` | 初始化预算条（12/24/20/200k/500） |
| `llm_delta` | `step, channel:"content"\|"reasoning"\|"tool_args", text, tool_name, tool_index` | **token 级流式（U11）**：模型正在写的那一段。合并后的碎块，**装饰帧** |
| `llm_round` | `step, llm_calls, tool_calls, tokens_in, tokens_out, cost_cents` + `llm_ms, tool_ms, preloaded` + **`cache_hit_tokens, cache_miss_tokens`**（v0.4） | 预算条实时更新；末两项是输入侧缓存命中情况 |
| `llm_round_started` | 与 `llm_round` 同字段（v0.11） | **一轮开始就说一声**：模型慢慢想的那几分钟里界面不再一动不动（时间线上写"第 N 步 · 正在问模型…"）；轮末仍是 `llm_round` 的那份账 |
| `tool_call_started` | `step, tool_name, args` | **先把"正在调用 X"显示出来**（主流 Agent 观感的来源） |
| `tool_call_finished` | `step, tool_name, args, status, duration_ms, tc_id, result_summary` | 工具卡片定稿（字段 = `ToolCallRecord`） |
| `finding_accepted` | `id:"F-001", severity, basis, title, evidence_ids[]` | 结论卡片 + 证据锚点 |
| `finding_rejected` | `reason_code, reason, title` | **把 V1–V4 的拒绝原因显示出来**（现在只打 stderr）——这是"证据链可信"的正面证据 |
| `report_written` | `path, sha256, degraded, unverified_items` | 报告面板 |
| `run_finished` | 与 `packetsage-agent run --json` 的对象**同构**（收口 §5.11）；v0.4 起含 `cache{hit,miss}` 与 **`summary`**（模型自己写的那段话） | 终态 |
| `error` | `code, message, retryable, where` | 见 §4.8 |

事件通用信封：

```json
{"type":"event","event":"tool_call_finished","run_id":"run_bc66ef04-…","seq":7,
 "ts_unix_ms":1758342000123,
 "data":{"step":4,"tool_name":"check_alerts","args":{},"status":"ok",
         "duration_ms":12,"tc_id":"tc_01M2Y6…","result_summary":"2 alert(s)"}}
```

规则：

1. `seq` 在一次 run 内从 1 起**单调递增、无缺口**（GUI 可据此判断丢帧）；
2. 单条事件 ≤ **8 KiB**，超限则截断并加 `"truncated":true`（`result_summary` 是摘要，结果正文不进事件）；
3. **token 级流式是装饰帧（v0.3 改）**：`provider.OpenAIProvider` 支持 SSE，
   agent 侧把碎块按 **120 ms / 256 字符（先到者）** 合并成 `llm_delta`；单条仍受 8 KiB 截断约束。
   规矩：
   * `channel` 只用三个词：`content`（模型在写正文）/ `reasoning`（推理模型的自述）/
     `tool_args`（正在拼工具参数，`tool_name` 一并给出）；
   * **丢了不影响任何结论、证据与预算**：外壳事件队列吃紧时**先丢它、且不计入 `dropped`**
     （`jsonl.rs::EventQueue`），`events_dropped` 的语义仍然只表示"真事件少了"；
   * 端点不支持 SSE（4xx）或流里没给 `usage` → agent 自动退回"单次 POST"的整块路径，
     不重发已经流出去的内容；缺 `usage` 的那一轮 tokens 记 0（不编数字）并写进 run 的 notes；
   * 只有实现了 `llm_delta` 钩子的 observer（桌面外壳的 `EventSink`）才会触发 SSE——
     CLI 的 `Progress` 没这个钩子，**命令行的行为与字节级输出不变**。
   > 出处：原文（v0.2）写的是"不做 token 级流式"，理由是 provider 单次 httpx 请求、没有 SSE；
   > v0.3 由 `_decide_streaming` 替换该实现，规则随之改写（ADR-027）。
4. `run_finished` 之后该 `run_id` 不再产生事件；
5. 顺序保证：`run_started` 最先、`run_finished` 最后，两者之间至少一条 `tool_call_*` 或
   `llm_round_started`/`llm_round`。**每一轮都是 `llm_round_started` → （`llm_delta`×N）
   → `llm_round` → `tool_call_*`**：开头那条（v0.11）是给"模型正在想"用的心跳，
   没有它，模型慢慢想的那几分钟面板一动不动（用户 2026-09-23 实测的"卡住"）。
6. **思考模式与缓存（v0.4 补）**：
   * DeepSeek 的 **思考模式默认打开**（[thinking_mode](https://api-docs.deepseek.com/zh-cn/guides/thinking_mode)），
     `reasoning_content` 与 `content` 同级返回 → provider 把它读出来走 `llm_delta` 的
     `reasoning` 泳道（界面上的「思考」）；想强制开关可用 `PACKETSAGE_LLM_THINKING=enabled|disabled`
     （默认 `auto` = 不显式传，跟随厂商默认，也不会让 OpenAI 端点因为多一个字段报 400）。
   * 官方要求**带 `tools` 时历史轮次的 `reasoning_content` 必须回传**（会被拼进上下文）——
     回传用的那一份**只活在内存里**，不进台账（ADR-007 不存思维链）。
   * 上下文硬盘缓存的命中情况（[kv_cache](https://api-docs.deepseek.com/zh-cn/guides/kv_cache)）从
     `usage.prompt_cache_hit_tokens / prompt_cache_miss_tokens` 读出，累计进 `llm_round` 与
     `run_finished.cache{hit,miss}`；**provider 不报就都是 0**——界面必须能分辨"没有这个数据"与"一次都没命中"。

### 4.6 一次 run 的完整时序（示例）

```text
GUI → hello                                      Agent → ack {protocol_version:1, providers:{configured:true}}
GUI → run {task_id:"task_01M2…", goal:"找出扫描与突发", mode:"run"}
                                                 Agent → ack {run_id:"run_bc66…"}
                                                 Agent → event run_started        (budget 12/24/20/200k/500)
                                                 Agent → event llm_delta ×N        (step 1, channel=tool_args,
                                                                                    text='{"tool": "get_capture…')
                                                 Agent → event tool_call_started  (get_capture_summary)
                                                 Agent → event tool_call_finished (tc_01M2…, 8ms)
                                                 Agent → event llm_round          (step 2, calls 2)
                                                 Agent → event llm_delta ×N        (step 2, channel=tool_args,
                                                                                    tool_name=check_alerts)
                                                 Agent → event tool_call_started  (check_alerts)
                                                 Agent → event llm_delta ×N        (step 3, channel=content,
                                                                                    text='{"findings": [{"title"')
                                                 Agent → event finding_accepted   (F-001 high rule_match)
GUI → cancel {run_id:"run_bc66…"}                ← 用户点了停止
                                                 Agent → ack {}
                                                 Agent → event run_finished (status=degraded, stop_reason=finalized_by_user)
```

### 4.7 取消语义（同时是 UI 行为规范）

1. GUI 收到 `cancel` 的 ack **不等于**已停止，只表示"已受理"；
2. 从 ack 到 `run_finished` 之间界面状态是 **`finalizing`**，文案"正在收尾…"；
3. `run_finished.stop_reason` 要原样显示（如 `finalized_by_user`），不要翻译成"已取消"就丢掉细节；
4. 已提交的 finding 保留（`request_finalize()` 的既定语义），但整体标 `degraded`。

### 4.8 错误模型

```json
{"id":"<uuid4>","type":"ack","ok":false,
 "error":{"code":"PROVIDER_UNREACHABLE","message":"…","retryable":true,"where":"provider"}}
```

| code | 含义 | retryable | UI 表现 |
|---|---|---|---|
| `PROTOCOL_MISMATCH` | 版本不匹配 | false | 阻止启动 + 提示重装 |
| `TASK_NOT_FOUND` | 查无 task | false | 提示重新分析 |
| `CONFIG` | 未配置 provider（exit 3 语义） | false | 引导首次运行向导 |
| `PROVIDER_UNREACHABLE` | 网络/鉴权失败 | true | 重试按钮 + 原始信息 |
| `ENGINE_CRASHED` / `ENGINE_SPAWN` | 引擎死 / 起不来 | true | 重建按钮 + `stderr_tail` 3 行 |
| `TOOL_TIMEOUT` | 工具超时 | true | 提示 + 允许重跑 |
| `BUSY` | 已有 run 在跑 | false | 灰掉"开始调查"直到 `run_finished` |
| `INTERNAL` | 未预期异常 | true | 堆栈写日志，界面只显示摘要 |

退出码语义（收口 §5.2）只在 CLI 侧保留；**桌面应用不使用退出码**，一律走上面的 code。

### 4.9 日志、超时与背压

1. **stderr 只作诊断**：两个 sidecar 的 stderr 各写一个日志文件
   （`%LOCALAPPDATA%\PacketSage\logs\{engine,agent}.log`），界面只显示尾部 3 行，**永不解析**。
2. **超时**：命令级默认 `hello 10s / run 无上限（靠事件与心跳）/ report 180s / cancel 10s`；
   引擎调用沿用 `RpcTimeouts`。
3. **心跳**：GUI 每 30s 发一次 `status`；连续 2 次失败判为 sidecar 不健康 → 走重建
   （对应 `EngineClient.heartbeat()` 的 `unhealthy` 语义）。
4. **背压**：一次 run 的事件量本来很小（≤12 步 × ~5 条 ≈ 60 条，每条 ≤8 KiB），
   加了 token 级流式（U11）之后单轮可能有几十条 `llm_delta`，所以 Rust 侧用
   **有界通道 + 分级丢弃**兜底：队列满时**先丢装饰帧**（`llm_delta`，不计入 `dropped`），
   没有装饰帧可丢才丢最旧的普通事件并合成 `events_dropped` 告警（§4.5 规则 3）。

### 4.10 协议冻结的验收

- [ ] 用**假 sidecar**（脚本回放 §4.6 的时序）验证 GUI 全部渲染路径，不依赖真模型；
- [x] 用**真 sidecar + mock provider** 验证端到端（`PACKETSAGE_LLM_PROVIDER=mock`）；
      证据：`tests/sidecar/protocol_cases.py` 的 S61（真引擎 + mock provider，事件顺序/字段/`seq` 逐条对拍）与 report 用例（九节报告 + `report_written`）；"慢到能 cancel / 并发"的两条用 `agent/tests/fake_engine.py slow:<s>` 桩（L2），不拖长 CI。
- [x] 把 §4.5 的命令/事件/字段表落成 `protocol_version = 1` 的机器可读清单（JSON Schema 或快照测试）并纳入 CI
      ——与收口文档 §5.10 / §5.11 的做法一致；
      证据：`tests/sidecar/protocol_v1.json`（清单快照）+ `protocol_cases.py` 的 S70（既比对快照，也用真 run 对拍每个事件名与字段集）；CI 步骤 `sidecar protocol S57-S70 (U1)`。
- [ ] 冻结后回填《收口文档》§5，成为"已冻结契约"。

---

## 5. 设计基线（主流桌面 Agent）

> 只定结构与信息优先级，不定像素。目标：让用过 Claude Desktop / Cursor / LM Studio 的人一眼会用，
> 同时让证据链成为主角。字段级细节可直接参照原型 `gui/views.py`。

### 5.1 窗口布局

| 区域 | 主流桌面 Agent 的做法 | 本项目承载 |
|---|---|---|
| 左栏（可折叠） | 会话 / 项目列表 + 设置入口 | 历史 `task_*`（`query sessions`）、当前抓包、规则集、报告、设置 |
| 顶部状态条 | 模型与用量 | provider / model、`steps 3/12`、`calls 9/24`、tokens、`cost_cents`（500 分上限） |
| 中央 | 消息流 + 工具块 | 用户提问 / 结论 / **工具卡片** / findings / 报告预览 |
| 工具卡片 | 可折叠"已使用 N 个工具" | 每个 `tc_*`：工具名、参数、`duration_ms`、`status`、`result_summary`；点开看会话/包号 |
| 右侧栏（可开关） | Codex 的右侧面板（顶栏方框图标开合、`＋` 选内容） | `抓包里有什么`（统计 + 规则告警明细）/ `图例` / `报告`；**v0.10：报告坞推广成通用右栏** |
| 底部输入 | 常驻输入框 | **只留四样**：`＋ 打开抓包`、当前抓包名、`模型 ▾`、发送 / 停止；追问走这里（`mode=chat`），`Enter` 发送、`Shift+Enter` 换行 |
| 运行控制 | 停止 / 重跑 | `cancel` → `finalizing` → `run_finished`；重跑 = 同一 `task_id` 再 run |
| 空态 | 建议问题 | "这份抓包里有扫描行为吗""哪些会话不完整""有没有可疑 DNS 行为" |

四条与"窗口怎么用"有关的硬规矩（v0.3 补，都是实测踩出来的）：

1. **左栏可折叠，但不许牵连主区**：收起后主区仍是整幅宽（grid 只剩一列），
   顶栏的 `›` 能把它放回来；
2. **底部输入框常驻**：会话流再长也只在自己那一块里滚，输入框永远钉在窗口底部
   （`.main` 必须 `min-height:0`，否则内容一高就把输入框顶到视口外）；
3. **设置入口常驻顶栏**：⚙ 与左栏底部的"设置"是同一个向导——收起左栏、或者
   provider 出问题时，key 仍然改得到；
4. **窄窗口按顺序丢装饰**（最小 1000×640）：先丢顶栏的费用、再丢 tokens、
   再丢输入框里的抓包名，最后才收图例卡；**任何宽度下都不许把文字压扁或顶出边框**。
5. **次要信息都开在右侧栏里**（v0.3 的"报告坞"，v0.10 推广成通用右栏）：点「生成报告」后
   正文进右栏（可关、可开所在文件夹），`抓包里有什么`、`图例` 也长在同一栏里、内容可切；
   对话那一列宽度不动、输入框不动，流里只留状态、指纹与动作。
   **顶栏开关 + `＋` 菜单**是唯一入口（`＋` 菜单挂在 `.main` 上，不能挂进顶栏——顶栏为了
   标题省略号必须 `overflow: hidden`，菜单挂进去会被裁掉，2026-09-23 实测）。
6. **调查中跟随滚动，但用户手动往上翻就跟停**：不按"离底部多远"判断（内容一边长一边跟随
   会误判），只有滚轮 / 拖滚动条 / 点进流里才算"用户动手"；这时给一个「↓ 回到最新」，
   回到底部自动恢复跟随。
7. **会话流按泳道显示 LLM 在干什么**：`思考`（`reasoning_content`，推理模型才有）/
   `调用`（选工具、拼参数、工具结果）/ `输出`（模型写正文）/ `结论` / `拒绝`，每条左侧一个标签。
8. **调查最后必须有"模型说的话"**（v0.4 补）：收尾 JSON 的信封是
   `{"summary": str, "findings": [...]}`，`summary` 渲染在结论卡上方（`chat` 模式下它
  就是回答）；模型给了才显示——**界面不替它编**。每条结论下面另显示引擎库里该 finding
   自己的 `summary`（模型为它写的那句话）。

### 5.1.1 信息层级与色彩纪律（v0.5，按用户评审重做）

9. **色彩只表达状态与分类，不用来吸引点击**：绿=无异常、琥珀=需要注意/降级、红=失败、
   蓝=调查中；**交互强调一律中性**——主按钮浅色实心（深色字）、次级描边、其余黑白灰。
   CSS 里不再有"主色变量"（原 `--accent` 已删除），谁都不许拿状态色当按钮底色。
10. **结论 = Hero，不是带边框的文本块**：没有边框、没有底色；状态是**图标化徽章**
    （🛡/⚠/▲/○，随 tone 变色）；**动作只留一个**（"重新调查"降为描边次级按钮）；
    关键数字以**等宽 stat 块**躺在结论正下方：包 / 会话 / 步骤 / 工具调用 / 耗时 /
    tokens / 费用 / 缓存命中。用户 3 秒内读完"结论 + 依据 + 成本"。
11. **调查过程 = 默认展开的时间线，而且它本身不是卡片**（v0.6 修正）：引擎预取 →
    工具调用 → 结论，一条时间轴走到底；**只有里面的工具调用是卡片**（可展开看入参与结果），
    结论**就地**挂自己的 `tc_*` 锚点（证据链不再另开一块）。整块不能收起——曾经做成外层
    `<details>` + 内层子卡片，点子卡片会把整块一起收掉（用户实测）。
12. **模型的文字不加框**：`summary` 与「思考/输出」的原文都是纯文本行（后者可展开看原文），
    框只留给工具卡片与图示。
13. **每轮的账在每轮最下面**：`预算 x/y 次模型调用 · 工具 · tokens · 费用 · 缓存命中`
    是低饱和的一行（`--faint`），落在该轮末尾，不参与 Hero 的扫读。
14. **消灭重复**：顶栏只留"在看哪份抓包"（task id 与全路径进 tooltip，运行数字归 Hero）；
    模型名只在输入框出现一次（同时是设置入口）；左栏同名抓包靠**导入时间 + 悬停全路径**
    区分，选中态高亮。
15. **文案瘦身**：报告区从"三句话解释"改成一行说明 + 按钮，"怎么算出来的"收进按钮旁的 ⓘ。
16. **追问必须被回答**（v0.6 修正）：`chat` 模式用**独立的收尾指令**——`summary` 就是
    对用户那句话的回答（2–6 句，带上用到的数字与 `tc_*`），答不了要明说抓包里没有能回答
    的字段；`findings` 只在用户明确要求落结论时才给。用 run 的指令去答 chat 会答非所问。
17. **思考模式是可选项**（v0.6）：`provider.json` 新增 `thinking`（`""`/`enabled`/`disabled`），
    向导第 1 步可选；启动时以 `PACKETSAGE_LLM_THINKING` 注入。**端点不认这个字段就去掉重发**
    （思考模式是可选增强，不该让整个调查挂掉），HTTP 错误要带 API 自己的说明。
18. **一轮的形态按模式分**（v0.7）：
    * `chat` 追问 = **一次对话**：用户气泡 → **模型回答（纯文本、正文字号）** → 结论平铺 →
      过程/证据时间线。**不摆 Hero**（"没有发现可疑行为"回答不了"这包和什么有关"）。
    * `run` 调查 = **一次汇报**：用户气泡 → 模型总结（若有）→ Hero 结论 → 时间线 → 原始数据 → 报告。
19. **模型与思考模式在输入框右下角**（v0.8）：点「模型 ▾」朝上弹出面板——模型名可直接填，
    候选来自 **`GET /models`**（agent 的 `setup --list-models`，拉不到就明说"拉不到"，不猜），
    思考模式三段（自动 / 开 / 关）；保存走 U7 的 `provider_save`（key 沿用已存的那一个）
    并只重启 Agent。向导里不再重复这两项。
20. **"没开跑"不许显示"没回答"**（v0.8）：新开一轮时 mode 复位；一轮还没开始（没有事件、
    没有结论）只渲染调查形态的空态，不套 chat 的兜底文案（实测：打开抓包就冒出一句
    "模型这次没有给出文字回答"）。
21. **追问必须有答案**（v0.8）：`chat` 若被策略提前收尾（重复调用 / 预算）而没写出回答，
    agent 会补一次**不带工具的兜底请求**（`{"summary": str}`，只依据已有工具结果）；
    兜底也失败就照实说"没答上"，不编。
22. **次要信息收进右侧栏**（v0.10 定稿，2026-09-23 实测通过）：`图例` / `抓包里有什么` /
    `报告` 都不占会话流版面，全部收进**一个通用的右侧栏**——复用 v0.4 就有的报告坞
    （`.main.docked` + `.dock`），栏内**内容可切**，栏头是三个文字页签。
    入口只有两个，都在顶栏：**方框图标**开合（再点一次回到上次看的那一栏）、**`＋` 菜单**
    列可选内容；`＋` 菜单挂在 `.main` 上而不是顶栏里（顶栏 `overflow: hidden` 会裁掉它）。
    中间形态"输入框下面的胶囊"**作废**：第一版那两个硬 bug（被 flex 排到输入框右侧、
    `pointer-events: none` 点不动）连同胶囊一起删掉；输入框那一带只留
    `＋ 打开抓包` / 当前抓包名 / `模型 ▾` / 发送（或停止）。
    **教训（写进来防止再犯）**：`.composer-wrap` 是 `pointer-events: none`（为了让滚轮透给
    会话流），**任何加进 composer 区域的可点元素都必须自己写 `pointer-events: auto`**。
    验收：右栏可开关、宽度与报告坞一致；切到「抓包里有什么」能看到统计 + 规则告警明细；
    会话流里不再有任何"抓包里有什么"的折叠块。截图见本地留存的
    `docs/ui/fixes-2026-09-23/`（**未入库**：测试记录与验收材料不进版本库，见根 `.gitignore`）。
23. **左栏 = 历史对话**（v0.9）：把所有抓包下的轮次聚合、按时间倒序列出来（历史本来
    就按 `task_id` 分桶存在本机），点一条就切到那个抓包并跳到那一轮；不认识的抓包
    在条目右侧标出来。
24. **模型名跟着厂商走**（v0.9）：`deepseek-chat` / `deepseek-reasoner` 已于 2026-09
    下线，默认改成 **`deepseek-flash`**（现役另有 `deepseek-v4-pro`，两者都支持思考模式）；
    候选一律以 `GET /models` 的实际返回为准（第 19 条），默认值只是没网时的兜底。
25. **打开 / 切回抓包不留痕**（v0.10，2026-09-23 实测修）：`liveTurnId` 与 `asked` **只在
    真的发起 run / chat 时**才设——打开抓包只改顶栏文件名，会话流里不出现「打开 XXX」的空壳。
    抓包这一层的空态（`还没有结论` / `规则命中 N 条告警，还没做调查` + 建议问题 + `开始调查`）
    由**当前抓包**渲染，不由"一轮对话"渲染；这份抓包已经跑过几轮，就把它说成
    「这份抓包有 N 轮调查记录」而不是再喊一次"还没结论"。
    验收：连续打开三份抓包再切回第一份，会话流里的轮次数 == 真正跑过的 run / chat 次数。
26. **主动重启 Agent 不算事故**（v0.10，2026-09-23 实测补）：向导保存模型、输入框里
    「保存并重启 Agent」、左栏「重建 Agent」走的都是"退出旧的再起新的"，而外壳给**真崩溃**
    和**主动重启**发的是同一条 `sidecar-exited`。所以这三条路径在重启前立一个 20 s 的
    **免罪窗口**：窗口期里的 agent 退出不写 `failed`、不弹「Agent 已退出」横幅。
    （实测症状：配完模型界面挂着「Agent 已退出 / 调查没能完成」，把一次正常的配置说成事故。）
27. **界面不显示工程字段**（v0.11，用户 2026-09-23 评审）：只有开发者读得懂的值一律不上界面——
    结论卡的 stat 只留 **包 / 会话 / 费用**（步骤、工具调用、tokens、耗时、缓存命中率、预取次数全删）；
    每轮的账目行只留「第 N 步」；调查中与跑完的说明文字不再拼 `provider/model`、`stop_reason`
    或错误码；工具卡片不再显示 `duration_ms`，状态由机器字（`ok` / `preload` / `invalid_args`）
    翻成人话；报告栏不再显示 `sha256` 指纹；左栏「诊断与自检」去掉「进程状态」按钮与
    `sidecar_diagnostics` 的 JSON dump、去掉引擎/Agent 的可执行文件路径；进度条提示不再写"上限 N 步"
    （满格值仍是规格里的 12，只在代码里当分母）。
    **保留** `tc_*` 证据锚点：可追溯性是本产品的卖点，也是 M9–M10 的验收对象（第 11 条），
    要不要一起去掉由用户另定。
    **这是一处有意偏离**：§5.2 的"预算常显"与 §8.2 的 M7（观察 `steps / calls / tokens / cost_cents`
    增长）按本条改写——预算数字现在只存在于引擎事件与报告里，不再常显在界面上。
28. **轮次装配：只有真跑过的才算一轮**（v0.12，2026-09-23 实测修。前身是第 25 条，那一条只
    管住了"谁来建轮"，没管住"读回来的历史"）：

    * **谁建轮**：`liveTurnId` / `asked` 只由发起 run / chat 的 `startInvestigation()` 设；打开 /
      切回抓包一律不建（第 25 条）。**归档也要过同一道门**：过程、结论、收尾三样全空的"轮"
      连历史都进不去。
    * **读回来先筛一遍**：历史按 `task_id` 分桶存在本机 `localStorage`，老版本留下的
      「打开 XXX」空壳（`feed` / `findings` / `finished` 三空）在**载入时**丢掉——不筛的话它会
      一直在会话流里画出一条「你：打开 XXX」+「这份抓包还没有结论」的假轮次。
    * **切抓包不许把上一份的历史写空**：落盘只在三个动作里**显式**做（发起一轮、换抓包、切任务），
      不再挂一个"`history` 一变就写"的 `useEffect`。原因：`analyzePath()` 先清内存、`await
      api.analyze()` 之后才换 `task_id`，中间那一次渲染仍然挂着**旧** task id，"一变就写"
      会把旧桶写成 `[]`——上一份抓包真正跑过的轮次就这么没了（实测：点左栏「历史对话」，
      该轮的提问与回答不见了，只剩空态）。
    * 验收：跑一轮 → 打开另一份抓包 → 点左栏那一条「历史对话」，那一轮的提问、模型回答、
      结论与收起的调查过程原样回来；连开十份抓包，会话流里的轮次数 == 真正跑过的 run / chat 次数。
29. **时间线按泳道次序排**（v0.12，2026-09-23 实测修，用户原话"把 LLM 输出移到最下、调用移到
    最上"）：调查过程（时间线）按**轮**分块——`第 N 步` 那一行是块头，块内自上而下
    `调用` → `思考` → `输出` → `拒绝`；块与块之间保持先来后到，引擎预取那一块仍在最前面。
    原来只按事件到达顺序排，于是"思考"的流式原文压在了工具调用上面、`输出`夹在中间。
    **正文只有一处**（第 12 条）：正在流的那段仍只画在结论卡里；**一轮结束、原文收进折叠项
    之后**，那一行 `输出` 才回到时间线，排在这一轮的最下面。用户说的"LLM 输出在最下"就是
    这一行；"调用在最上"是工具卡片与"正在准备调用"那两行。
30. **每一轮的用户气泡都靠右，模型那段话与工具卡片同宽**（v0.13，2026-09-23 实测修）：
    * `.turn-user` 自己就是 flex 容器——历史轮次里它只是"用户那一行"的包装节点，原来只有
      当前那一轮（`.turn.turn-user`）吃到 `align-items: flex-end`，于是**翻过篇的轮次气泡
      贴在左边**（用户图一）。
    * `.md` 的 `82ch` 与 `.hero-detail` 的 `76ch` 上界删掉：中栏多宽，模型那段话就多宽。
      第 12 条说的"模型文字不加框"是**不画边框**，不是把它缩成一条（用户实测：工具卡片占满
      中栏、文字只占左边一小条）。
31. **设置里能读到本机参数**（v0.13，用户"设置里看不到任何软件的参数信息"）：向导
    （= 设置）底部一块**只读**折叠表「本机参数：版本 · 协议 · 路径」——外壳版本 / Agent 版本 /
    协议版本 / 引擎 schema 版本 / Agent 能力 / provider / 模型 / 端点 / 推理强度 / 思考模式 /
    key 是否已存 / 数据目录 / 引擎库 / 两个 sidecar 的程序路径与进程状态。
    版本号各有**唯一来源**：外壳读自己的 `Cargo.toml`（`app_info.version`），Agent 与协议版本
    读握手 `welcome`，引擎 schema 也读握手；取不到就写「—」，界面不替它们编。
32. **回答里不出现机器字段**（v0.14，2026-09-23 实测修）：收尾信封**不是合法 JSON** 时
    （实测写法：`[summary]: "…"` + `[findings]: [ … ]`），只把 `summary` / `answer` / `text`
    字段值、截到 `findings` 之前的那段人话当回答；机器字段与原文不丢——它们仍在时间线的
    「输出 · 原文」里（第 29 条）。判据：**文本以信封开头**才清理，普通回答里提到
    `summary` / `findings` 这两个词时一个字都不动。
    配套（在外壳侧，用户实测"滑块能滑但强度存不下来"）：`provider_save` 在**没给新 key**
    但凭据管理器里存过 key 时**沿用**它，只有"需要 key 且一个都没有"才报错——模型面板
    只调强度 / 换模型时发的就是 `apiKey: null`。
33. **模型文字进界面的每一条路都要过渲染器，流式与收尾同一个口径**（v0.15，2026-09-24
    实测修；用户原话只有一句"新建议题：LLM 的 MarkDown 输出"）：

    * **正在流的那段正文不是人话，是收尾信封原文**。`llm_delta` 的 `content` 通道发的是
      模型原文（run / chat 两套收尾指令都要求 `{"summary": …, "findings": […]}` 信封），
      所以**上界面之前先取人话**：`desktop/src/envelope.ts::humanDraft()` 取
      `summary` / `answer` / `text` 的值、按 JSON 字符串读到收尾引号（流到一半、字符串还没
      闭合时也给到当前进度）、解 `\n` `\t` `\"` `\\` `\uXXXX` 转义；机器字段写在前面时按
      **括号深度**只认顶层那个键（`findings` 里每条结论自己的 `summary` 是第二层，不算）。
      判据与服务端 `provider._salvage_summary()` **同一条**：文本以信封开头才清理，普通散文
      一个字不动；取不到人话（本轮只写 `findings` / 信封才开个头）就返回空串、界面照旧显示
      「正在追查…」——**不许把半截 JSON 画到结论卡上**。原文照旧在时间线「输出 · 原文」里
      （第 29 / 32 条），那里是**有意**的原文。
    * **模型写的短字段同样要过渲染器**：每条结论的 `title` 走行内渲染
      （`MarkdownInline`：块级标签摊平、`li` 之间给分隔符，标题在布局里还是行内元素），
      每条结论的 `summary`（模型原话）走 `MarkdownView` 并用 `.finding-detail .md`
      压回小字号紧凑——它是结论行的注解，不是第二段正文。
    * **`.md` 的元素规则补齐**：围栏代码块（`pre` / `pre code`：底色块、边框、内边距、
      横向滚动、12px 等宽、不折行），`ol` 与 `ul` **同一条缩进**（原来 `ol` 吃浏览器默认的
      40px，比 `ul` 深一截），`a` 用中性下划线（不用状态色，第 9 条），补 `hr` / `del` /
      GFM 任务列表，`.md` 首尾不留空档。
    * **宽表格不许顶破版心**：`MarkdownView` 用 `components.table` 给表格套一层
      `.md-table-wrap`（横向滚动壳），表格 `width: max-content; min-width: 100%`，
      单元格 `max-width: 34ch` + `overflow-wrap: anywhere`——窄表格照旧铺满，宽表格自己滚。

> **对话历史的存放位置（限制）**：引擎库的 `agent_runs` 只存模型/状态/耗时/费用，
> **没有 goal 与对话文本**，所以"哪一轮问了什么"由桌面端自己保存——按 `task_id` 分桶写进
> 本机 `localStorage`（token 碎块不入库，只留步骤/工具/结论），换机器或清缓存就没了。
> 真要让历史跨机器，需要引擎侧新增一列/RPC（未做，登记为缺口）。

### 5.2 本项目特有一等公民（不能藏进设置）

| 面板 | 内容 | 数据来源 |
|---|---|---|
| 证据链 | 结论 ↔ `tc_*` ↔ 会话/告警/包号，四段对拍且可点开 | `query_history`（`kind=findings` / `kind=trace`） |
| 规则告警 | 规则 id / 版本 / `rule_content_hash` / 窗口证据 / 样例包 | `check_alerts` |
| 预算常显 | **v0.3 调整**：改成"一个状态 + 折叠明细"——结论卡上只写「调查进行中（第 N 步）」，12 步 / 24 次 LLM / 20 次工具 / 200k tokens / 500 分收进「调查过程」折叠区（原来的五个分数并排没有分子分母，读不出来） | `run_started.budget` + `llm_round` |
| 降级可见 | `degraded`、`[unverified by engine]`、`sample_packets=[]`、`CAPTURE_UNAVAILABLE`、**`finding_rejected` 的拒绝原因** | 事件 + 报告 §8 |
| 报告 | 九节 Markdown 预览、导出、打开所在文件夹 | `report_written` |

### 5.3 桌面应用才有的东西（做了才像"应用"而不是"网页"）

| 能力 | 说明 |
|---|---|
| 拖放 | 把 `.pcap` / `.pcapng` 拖进窗口即开始分析（M16） |
| 原生菜单与快捷键 | 打开抓包、重新分析、停止、导出报告、切换深浅色 |
| 窗口状态记忆 | 尺寸/位置/左栏折叠状态持久化 |
| 系统通知 | 长 run 结束时通知（可选） |
| 打开所在文件夹 | 报告/日志一键定位 |
| 高 DPI 与深色模式 | 150% 缩放下不变形；跟随系统深浅色 |
| 单实例 | 重复双击聚焦已有窗口，不新开引擎 |

### 5.4 明确不做

见 §2.4。界面层补充：不做多标签、不做内嵌终端、不做"技能商店"。

---

## 6. 交互流程与状态机

### 6.1 首次运行

```text
双击安装包 → 安装（per-user，无需管理员）
  → 首次启动
      ├── 检查两个 sidecar 可执行（缺失 → 明确的"安装损坏"提示）
      ├── hello / welcome 握手（版本不匹配 → 阻止 + 提示重装）
      ├── providers.configured=false → 首次运行向导（选 provider → 输 key → 校验 → 存凭据管理器）
      └── doctor 自检 10 项（收口 §5.8）→ 全过才进主界面
```

### 6.2 主流程

```text
选/拖入抓包 → analyze_file（包数、会话数、告警）
  → run（工具卡片逐条出现 + 预算条增长）
  → 结论 + findings（每条带证据锚点，可点开追到 tc_*）
  → 追问（chat 模式，多轮）
  → 报告（九节 Markdown，导出 + 打开所在文件夹）
```

### 6.3 运行状态机（界面必须能表达这七种）

| 状态 | 触发 | 界面表现 |
|---|---|---|
| `idle` | 无任务 | 空态 + 建议问题 + 拖放提示 |
| `analyzing` | `analyze_file` 进行中 | 进度（包数 / 速度） |
| `running` | `run_started` 之后 | 工具卡片逐条追加；预算条实时；**停止可用** |
| `finalizing` | `cancel` 已 ack，等 `run_finished` | 文案"正在收尾…"（当前 LLM 轮可能还要跑完，见 U5） |
| `completed` | `status=completed` | 结论 + findings；预算定格 |
| `degraded` | `status=degraded` / 报告降级 | 结论仍展示，**顶部显式黄色标注**原因 |
| `failed` | `error` / sidecar 死亡 | 可读错误 + 重试 / 重建按钮 |

### 6.4 错误呈现三条规矩

1. **永不吞错**：错误 code 与 `where` 都要显示，不只显示"出错了"；
2. **区分可重试**：以 `error.retryable` 为准（§4.8），可重试就给按钮；
3. **降级永远显式**：`degraded` / `[unverified by engine]` / `finding_rejected` 是诚实的产物，不能藏。

---

## 7. 打包与分发

### 7.1 安装包内容

```text
PacketSage-0.1.0-x64-setup.exe      (NSIS)  或  .msi (WiX)
 └── 安装到 %LOCALAPPDATA%\Programs\PacketSage
      ├── packetsage-desktop.exe      Tauri 外壳（含前端资源）
      ├── packetsage.exe              引擎 sidecar（Rust release）
      ├── packetsage-agent.exe        Agent sidecar（PyInstaller --onedir）
      ├── rules/builtin/*.yaml        内置规则
      └── samples/synth-mixed.pcap    演示样本（可选）

用户数据（不随安装包，卸载默认保留）：
      %LOCALAPPDATA%\PacketSage\{packetsage.db, reports\, logs\}
```

### 7.2 sidecar 打包要点

1. **引擎**：`cargo build --release`；用 Tauri `bundle.externalBin`。注意 Tauri 要求 sidecar 文件按
   target triple 命名（如 `packetsage-x86_64-pc-windows-msvc.exe`），打包脚本里改回正式名。
2. **Agent**：PyInstaller `--onedir`（比 `--onefile` 启动快得多，也不会被 AV 反复扫描）；
   必须包含 `packetsage_agent/prompts_text/*.txt` 与 `banner.txt`（`pyproject.toml` 已声明 package-data，
   spec 要照抄）；排除可选组 `langchain`（当前工具循环不依赖它）。
3. **体积预算**：Agent sidecar 约 40–80MB、引擎约 5–15MB、安装器目标 **< 120MB**。
4. **路径**：sidecar 一律用**绝对路径**启动，工作目录设为 app data 目录（打包后没有"仓库根"）。

### 7.3 安装器

| 项 | 选择 |
|---|---|
| 格式 | NSIS `.exe`（体积小、per-user、无需管理员）为主，MSI 为备 |
| WebView2 | 随包 `embedBootstrapper`（已装则跳过；本机已装 153.x） |
| 安装位置 | `%LOCALAPPDATA%\Programs\PacketSage` |
| 快捷方式 | 开始菜单 + 可选桌面 |
| 卸载 | 删除程序目录；**询问**是否删除用户数据（db / 报告 / 日志） |
| 签名 | 无证书 → SmartScreen 会警告，README 与安装页要写明；有预算再买证书 |
| 更新 | Tauri updater + 签名 manifest。13 周内属可选项，先手动作罢 |

### 7.4 首次运行向导

1. 选 provider（`mock` / `openai` / `deepseek` / `local`）——`mock` 的文案必须写明"脚本回放，不是真实分析"；
2. 输入 key（不回显）→ `GET /models` 校验；
3. 写入 **Windows 凭据管理器**（不落明文 `.env`）；
4. 跑 `doctor` 十项，失败项显示 `repair_hint`，给"重跑自检"按钮；
5. 成功 → 主界面 + 空态建议问题。

### 7.5 卸载与升级路径

* 覆盖安装（同版本/升级）必须能原地替换，且**不动**用户数据；
* 卸载后重装：若用户数据保留，历史任务与报告仍可见（M21 / M24）。

### 7.6 "目标机零依赖"怎么验证

干净 Windows 虚拟机（**未装 Python / Rust / Node / Visual Studio**）：

```text
复制 setup.exe → 双击 → 安装 → 启动 → 向导填 key → 选样本 → 分析 → 调查 → 报告
```

这条走通才算达成"双击就能安装的桌面应用"（M1–M5）。任何一步依赖了开发机上的东西，都是不达标。

### 7.7 开发机需要补的东西（一次性）

| 项 | 现状 | 要做什么 |
|---|---|---|
| Rust target | ✅ 已装 `x86_64-pc-windows-msvc`（并装了 msvc host toolchain，`desktop/src-tauri/rust-toolchain.toml` 固定它） | — |
| MSVC 链接器 | ✅ VS Build Tools 18.8.2（`...\18\BuildTools`，MSVC 生成工具 + Windows 11 SDK 10.0.26100）已装，`cargo build` 已验证能链接 | — |
| Tauri CLI | ✅ 走 `@tauri-apps/cli`（npm devDependency），不必全局装 | — |
| node / npm | 已装 | — |
| WebView2 | 已装（153.x） | — |

**2026-09-20 实测结论（本轮）**：`npm run tauri build` 全绿，产出外壳
`target/release/packetsage-desktop.exe`（4.5 MB）与安装包
`bundle/nsis/PacketSage_0.1.0_x64-setup.exe`（43.9 MB，NSIS/LZMA；§7.2 的"安装器 <120MB"达成）。
**一条待办**：引擎 sidecar 的 release 产物是 **125 MB**（未 strip 的调试信息），远超 §7.2 的
"引擎 5–15MB"预算；安装包体积因此靠压缩兜住。建议给发布档加 `strip`/`debug=false`
（属引擎构建策略，需与 CLI 发布一起决定）。

附带好处：切到 MSVC target 后，今天那类 `dlltool.exe` / MinGW 空格路径问题**整类消失**。
引擎的 GNU 构建保留给 CLI 用户，但发布用的 sidecar 建议与外壳统一到 MSVC，减少"两个工具链"的心智负担。

---

## 8. 测试与验收

### 8.1 自动用例（L2 模拟桩，**全部未跑**）

| # | 用例 | 手段 | 通过判据 |
|---|---|---|---|
| S57 | 握手版本匹配 / 不匹配 | 假 sidecar 回不同 `protocol_version` | 匹配 → 进入主界面；不匹配 → 阻止启动 + `PROTOCOL_MISMATCH` |
| S58 | sidecar 崩溃感知 | run 中 `taskkill` 掉 `packetsage-agent.exe` | GUI 在 **EOF 即刻**（非轮询超时）报可读错误并能重建 |
| S59 | 引擎崩溃与重建 | run 中杀掉引擎 | 错误 + `stderr_tail` 3 行；重建后同一 `task_id` 仍可用（冷恢复） |
| S60 | 取消 | run 中发 `cancel` | ack 后进 `finalizing`；`run_finished.status=degraded` + `stop_reason`；已提交 findings 保留 |
| S61 | 事件顺序与完整性 | mock provider 跑一次真 run | `seq` 单调无缺口；`run_started` 首、`run_finished` 末 |
| S62 | 事件体积上限 | 注入超大 `result_summary` | 截断 + `truncated:true`；单条 ≤8 KiB |
| S63 | 并发命令 | run 中再发 `run` | 回 `BUSY`；GUI 灰掉按钮直到 `run_finished` |
| S64 | 进程数稳定 | 连续 20 次操作 | `packetsage.exe` 与 `packetsage-agent.exe` 各 1 个；无 fd/线程增长 |
| S65 | 长 run 不阻塞 UI | 假 sidecar 每 2s 一条事件、持续 60s | 界面持续响应；事件逐条出现 |
| S66 | 路径健壮性 | 含空格/中文/长路径/UNC 的抓包 | 分析与报告成功；退出语义与 CLI 一致 |
| S67 | provider 未配置 | `welcome.providers.configured=false` | 进首次运行向导；拒绝启动 run；**不静默 mock** |
| S68 | 降级可见 | 注入 `degraded` 样本 | 顶部黄色标注；`[unverified by engine]` 与 `finding_rejected` 可见 |
| S69 | 只读数据路径 | 列表/详情走 RPC | 无裸 SQL；`db query --readonly` 无 `mode=ro` 被拒 |
| S70 | 协议清单快照 | 对 §4.5 表做快照测试 | 命令/事件/字段名变更即失败，必须显式改版本 |

### 8.2 人工验收清单（**构建完成后由项目作者优先亲自逐条测试**）

> 这是"多数用户最终会走的入口"，且交付物是安装包，所以必须在一台**干净的 Windows** 上测，
> 不能只在开发机上点两下。逐条打勾并记录实际观察；不符时按最后一列定位怀疑对象。
> 前置：准备好一个真 provider 的 key；样本用 `samples/synth-mixed.pcap`（612 包 / 570 会话）。

| # | 手工步骤 | 预期观察 | 不符时先怀疑 |
|---|---|---|---|
| M1 | **干净 VM**（无 Python/Rust/Node/VS）双击 `setup.exe` | 装完出现开始菜单图标；全程无需装任何运行时 | U9 / §7.6 |
| M2 | 双击图标启动 | 数秒内出窗口；无控制台黑框；无 traceback | U3 |
| M3 | 首次运行向导：选 deepseek → 填 key → 校验 | 校验通过；key 存入凭据管理器（非明文 `.env`） | U7 |
| M4 | 跑 `doctor` 十项 | 全 ok；失败项有修复提示 | 收口 §5.8 |
| M5 | 选/拖入 `synth-mixed.pcap` → 分析 | 显示 612 包 / 570 会话；进度可见 | 通道 A |
| M6 | 触发调查（真模型） | 工具卡片**逐条出现**，不是 30s 后一次性刷出；期间界面可响应 | U1 / U4 |
| M7 | 观察预算条 | `steps / calls / tokens / cost_cents` 增长；上限 12 / 24 / 200k / 500 | U1 事件 |
| M8 | run 中按"停止" | 文案变"正在收尾…"；随后 `degraded` + `stop_reason`；已得结论保留 | U5 / §4.7 |
| M9 | 看结论 | 中文；每条能追到 `tc_*` 证据锚点；能找到 F-001 / F-002 | 收口 §5.11 |
| M10 | 点开证据锚点 | 追到具体工具调用（工具名、参数、耗时、结果摘要） | 收口 §5.4 |
| M11 | 规则告警面板 | `NET-TCP-SYN-BURST-001`(high) 与 `NET-TCP-PORT-SWEEP-001`(medium)，含窗口与样例包 | `check_alerts` |
| M12 | 追问一轮（"哪些会话不完整？"） | chat 模式；中文；引用证据 id；预算累计 | 提示词 chat 块 |
| M13 | 生成报告 | 九节齐全；可导出；"打开所在文件夹"能定位 | `report_written` |
| M14 | 关掉窗口，看任务管理器 | `packetsage-desktop.exe` / `packetsage.exe` / `packetsage-agent.exe` **全部消失** | U3 |
| M15 | 崩溃注入：run 中在任务管理器结束 `packetsage-agent.exe` | 界面给可读错误（含 stderr 尾 3 行）+ 一键重建 | U3 / S58 |
| M16 | 把 `.pcap` 拖进窗口 | 自动开始分析 | §5.3 |
| M17 | 系统缩放设 150% | 布局不变形；文字不重叠 | §5.3 |
| M18 | 系统切深色模式 | 界面跟随；证据 id 等宽可读 | §5.3 |
| M19 | 快捷键 | `Enter` 发送 / `Shift+Enter` 换行 / 停止有快捷键 | §5.3 |
| M20 | 连续 20 次操作（切任务、追问、重跑） | 不卡死；进程数仍为 3；内存不持续上涨 | U3 / S64 |
| M21 | 卸载 → 重装 | 询问是否保留用户数据；保留后历史任务与报告仍在 | §7.5 |
| M22 | 断网后触发 run | 报 `PROVIDER_UNREACHABLE`；**不产生假结论**；可重试 | U7 / §4.8 |
| M23 | 抓包与导出路径含中文/空格 | 全部成功 | U9 / S66 |
| M24 | 覆盖安装（同版本） | 原地替换成功，用户数据不动 | §7.5 |

### 8.3 演示路径（给评审看的 3 分钟脚本）

1. 干净 VM 上双击安装包 → 启动 → 向导（M1–M4）。**这一条本身就是"交付了一个桌面应用"的证据。**
2. 拖入 `samples/synth-mixed.pcap` → 612 包 / 570 会话（M5 / M16）。
3. 触发调查 → 工具卡片**当场逐条出现**（M6，真 Agent 与假回放的分水岭）。
4. 点一条结论 → 点开它的证据锚点 → 追到具体工具调用（M9–M10）。
5. 指标面板：12 步 / 24 次 LLM / 500 分预算常显（M7）。
6. 规则告警面板：SYN 突发 + 端口扫描（M11）。
7. 生成报告并导出（M13）。

**演示前必做**：确认 provider 可用（别在观众面前填 key）、样本存在、**不依赖网络**；若要演示断网降级，把 M22 放在最后。

---

## 9. 风险

| # | 风险 | 影响 | 处置 |
|---|---|---|---|
| P1 | **从零 IPC 到双 sidecar 是一次架构级改动** | U1 拖住整个进度 | U1 优先级最高，先定协议再写界面；先用假 sidecar 开发前端 |
| P2 | PyInstaller 产物被 AV 误杀 / 启动慢 | 装完打不开，最坏毁掉演示 | 用 `--onedir`；在干净 VM 上早测（M1）；必要时加签名 |
| P3 | Tauri 需要 MSVC，而工作区现在是 GNU | 环境折腾掉几天 | §7.7 一次性处理；顺带消灭 `dlltool` 那一类问题 |
| P4 | 用户装了应用却未配 provider | 输出像真分析的假结论 | U7 + S67：未配置就拒绝启动 run，文案写明 `mock` 是回放 |
| P5 | 真模型 E2 评测失败、E3/E6 findings=0（收口 §2.4 / G4） | 演示时"分析不出东西" | 演示前跑一次确认；必要时用 mock 并如实说明 |
| P6 | 同一信息两套解析（Python 对象 vs `run --json`） | 两条路径给出不同结论 | §4.5：`run_finished` 与 `run --json` 同构；GUI 不解析 CLI 文本 |
| P7 | Streamlit 原型与 Tauri 版并行演进 | 两份漂移，评审看到矛盾的两套 UI | §1.3：原型冻结为参考，只修 bug 不加功能 |
| P8 | 13 周不够做完"应用级"细节（菜单、拖放、通知、更新） | 交付像半成品 | §5.3 的能力按优先级排；更新/通知是可选，M1–M14 是必需 |

---

## 10. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v0.1 | 2026-09-20 | 首版：Streamlit 方案（技术栈与风格决策、U1–U8、设计基线、交互流程、S57–S65、人工验收 M1–M18）。**已作废** |
| **v0.2** | 2026-09-20 | **改用 Tauri，交付形态定为"可双击安装的 Windows 桌面应用"**。主要变化：① §1.1 由交付形态倒推的约束；② §1.3 现存 Streamlit 原型的迁移关系（哪些保留为规格输入、哪些作废）；③ §2 三进程架构与"证据路径不经过 Python"原则；④ **新增 §4 数据交换协议**（通道 A/B/C、帧格式、握手与版本协商、命令与事件表、时序、取消语义、错误模型、背压）；⑤ U 系列扩为 U1–U10（U1 = Agent sidecar 协议）；⑥ **新增 §7 打包与分发**（sidecar 打包、安装器、首次运行向导、零依赖验证、开发机要补的 MSVC 环境）；⑦ 验收扩为 S57–S70 + M1–M24（重心移到干净机器安装与桌面行为）。 |
| **v0.3** | 2026-09-22 | **token 级流式（U11）**，用户授权改规格：① §4.5 事件表新增 `llm_delta`，规则 3 由"不做 token 级流式"改写为"装饰帧"（合并粒度、分级丢弃、三条退路、CLI 不变的边界）；② §4.6 时序补 `llm_delta`；③ §4.9 背压改为"先丢装饰帧"；④ §2.4 移除"不做 token 级打字机流式"；⑤ U 系列扩为 U1–U11。配套改动见 [ADR-027](../adr/ADR-027-token-level-streaming.md)。 |
| **v0.4** | 2026-09-22 | **思考模式、上下文缓存、模型总结**：① §4.5 规则 6 新增——DeepSeek 思考模式默认打开，`reasoning_content` 走「思考」泳道，**带 tools 时必须回传**（仅内存、不进台账）；`PACKETSAGE_LLM_THINKING` 可强制开关；② `llm_round` 追加 `cache_hit_tokens/cache_miss_tokens`，`run_finished` 追加 `cache{hit,miss}`（都来自 `usage`，provider 不报就是 0）；③ **收尾信封加 `summary`**（模型自己写的 2–4 句中文），`run_finished.summary` = 对话里"它说的那段话"，`chat` 模式下就是回答，§5.1 第 8 条；依据：thinking_mode、kv_cache 两篇官方指南 + 《Agent 系统提示词规格》输出信封。 |
| **v0.5** | 2026-09-22 | **按用户评审重做信息层级与色彩**（新增 §5.1.1 第 9–14 条）：色彩只表达状态/分类、交互一律中性（删除 `--accent`）；结论升级为 **Hero**（去边框、图标徽章、动作唯一且降级为描边、统计上提为等宽 stat 块）；**调查过程前置为默认展开的时间线**（预取 → 工具 → 结论，工具节点可展开，证据就地挂结论）；模型总结**不加框**；顶栏/模型选择/同名抓包三处重复清理；报告文案瘦身。 |
| **v0.6** | 2026-09-22 | **三处实测问题的修正**：① **调查本身不再是卡片**——曾经是外层 `<details>` 套内层子卡片，点「思考」子卡片会把整块调查一起收掉；现在只有**工具调用**是卡片（§5.1.1 第 11 条）。② **追问必须被回答**：`chat` 用独立收尾指令（`summary` = 回答），修掉"问了跟抓包有关的什么、只回一份 findings 汇报"（第 16 条）。③ **思考模式成为设置项**：`provider.json.thinking` + 向导选项 + `PACKETSAGE_LLM_THINKING` 注入，且**端点拒绝该字段时自动去掉重发**、HTTP 错误带上 API 说明（第 17 条）。另：每轮的账挪到轮末并降饱和（第 13 条）。 |
| **v0.7** | 2026-09-22 | **追问轮改成对话形态**（§5.1.1 第 18 条）：`chat` 那一轮以**模型的回答**为主角（纯文本、正文字号），结论平铺列在后面，**不摆 Hero 壳**；`run` 那一轮仍是 Hero 汇报。历史轮次按各自 mode 同样渲染。 |
| **v0.8** | 2026-09-22 | **三处实测修正**：① **模型与思考模式挪到输入框右下角**（朝上弹出的面板，候选来自 `GET /models`，`setup --list-models` 是唯一出口；第 19 条）；② **"没开跑"不再显示"没回答"**——新开一轮复位 mode，未开始的那一轮只渲染调查空态（第 20 条）；③ **追问必须有答案**：`chat` 被提前收尾时补一次不带工具的兜底回答请求（第 21 条）。 |
| **v0.9** | 2026-09-22 | **版面再收一次**（§5.1.1 第 22–24 条）：① 图例 / 抓包里有什么 / 报告 从会话流挪到**输入框下面的胶囊**（点开才占屏），报告全文仍走右侧报告坞；② 左栏改成**历史对话**（跨抓包聚合本机轮次、倒序，点一条切抓包并跳到那一轮）；③ 模型名跟厂商走——`deepseek-chat`/`reasoner` 已下线，默认 `deepseek-flash`，候选以 `GET /models` 为准。 |
| **v0.10** | 2026-09-23 | **修完 2026-09-22 实测记录的四条问题**（依据《[GUI 待修问题 2026-09-22](GUI%20待修问题%202026-09-22.md)》）：① **次要信息收进右侧栏**（第 22 条定稿）——"胶囊"作废，报告坞推广成通用右栏（`抓包里有什么` / `图例` / `报告`，顶栏方框图标开合 + `＋` 菜单选内容），输入框那一带只剩 `＋ / 抓包名 / 模型 ▾ / 发送`；② **打开 / 切回抓包不建轮**（第 25 条）——`liveTurnId` / `asked` 只在真的发起 run / chat 时设，抓包级空态改由当前抓包渲染；③ **主动重启 Agent 不算事故**（第 26 条）——给向导保存、模型面板保存、重建 Agent 三条路径加免罪窗口。真机验收截图见本地留存的 `docs/ui/fixes-2026-09-23/`（未入库）。 |
| **v0.11** | 2026-09-23 | **对话层四处实测修正**（用户实测："上面已经调查完了，还是一句人话没说；追问后又一直调工具，接着就卡住"）：① **多轮消息按官方协议累积**——`provider.decide` 不再每轮重建"伪造的工具记录"，改为原样追加模型的 assistant 消息（含 `reasoning_content`/`tool_calls`）与 `role="tool"` + `tool_call_id` 的工具结果（依据 `guides/multi_round_chat`、`guides/thinking_mode`）；② **散文就是回答**——模型没按 JSON 写时不再判 `malformed` 丢弃，`summary` 缺失时 `run` 模式也补一次不带工具的兜底回答（原来只管 `chat`），§4.5 规则 5；③ **不再静默卡住**——一轮开始发 `llm_round_started`（§4.5 事件表新增），每轮有墙钟上限（默认 300 s，`PACKETSAGE_LLM_ROUND_TIMEOUT_S` 覆盖）与可读的失败原因，缺 `usage` 时不再永久关掉流式；④ **追问带上下文**——sidecar、CLI REPL 与 Streamlit 原型都跨回合保留真实消息表（上限 24 条、不留孤儿 `tool` 消息），追问不再从零重扫；⑤ **结论不再白丢**——`ref_id` 写成 `"10,35,60"` 这类多值串拆成单值（每个仍各过 V3），引擎 V1-V4 拒绝时把抱怨原样交回模型**改一次**引用再收尾（`_validate_drafts` 只读预检 + `_push_user_note`），提示词写清 `_id` = `_anchor`、`ref_id` = `_ref_ids` 里的**单个**值；顺带修掉"只靠预取事实回答的追问被标成 `failed`"。 |
| **v0.11** | 2026-09-23 | **界面不再显示工程字段**（新增 §5.1.1 第 27 条，用户评审）：结论卡 stat 从 8 项收到 3 项，每轮账目行只留「第 N 步」，hero 文案去掉 `provider/model`/`stop_reason`/错误码，工具卡片去掉耗时与预取标签、状态翻成人话，报告栏去掉 `sha256` 指纹，左栏去掉「进程状态」与 JSON dump 与可执行文件路径，进度条提示去掉"上限 N 步"。**有意偏离** §5.2「预算常显」与 M7 的旧口径；`tc_*` 证据锚点保留。同期把根目录的 `CLI ASCII标题.txt` 删掉，ASCII 标题改为两侧各自硬编码。 |
| **v0.12** | 2026-09-23 | **修完 2026-09-23 实测记录的两条**（依据《[GUI 待修问题 2026-09-23](GUI%20待修问题%202026-09-23.md)》，新增 §5.1.1 第 28、29 条）：① **轮次装配**——归档与载入两头都过"真跑过才算一轮"这一道门（老版本留下的「打开 XXX」空壳在载入时丢掉），并把历史落盘从"`history` 一变就写"改成三处**显式**写，修掉"切抓包时把上一份抓包的历史写成 `[]`"（表现为点「历史对话」看不到那一轮）；② **时间线泳道次序**——按轮分块、块内 `调用 → 思考 → 输出 → 拒绝` 自上而下（用户原话"把 LLM 输出移到最下、调用移到最上"），正在流的正文仍只画在结论卡里、一轮收尾后才以折叠原文回到这一轮最下。两条的纯逻辑（`sanitizeTurns()` / `orderTimeline()`）搬进 `desktop/src/session.ts` 便于单独验算。 |
| **v0.13** | 2026-09-23 | **修完用户实测 4.1.2 反馈的四条 + 文档对齐**（依据《[GUI 待修问题 2026-09-23](GUI%20待修问题%202026-09-23.md)》第二轮，新增 §5.1.1 第 30、31 条）：① **历史轮次的用户气泡重新靠右**（`.turn-user` 自己成为 flex 容器）；② **模型那段话与工具卡片同宽**（删掉 `.md` / `.hero-detail` 的 ch 上界）；③ **设置里新增只读「本机参数」**（外壳 / Agent 版本、协议版本、引擎 schema、能力、provider / 模型 / 强度、数据目录、两个 sidecar 路径与进程状态；外壳版本走 `app_info` 新增的 `version` 字段）；④ **构建卫生**——`build_desktop.ps1` 打完包自动只留当前版本、镜像进 `dist/windows/` 并重写那里的 README，新增 `scripts/clean.ps1`（`-Engine / -Desktop / -Agent / -Artifacts / -NodeModules / -All`，支持 `-WhatIf`）清缓存，另修掉 agent sidecar **一直自报旧版本**（打包改成塞进 `agent/pyproject.toml`，`stage_desktop_sidecars.py` 复制前清掉 `target/release/agent-sidecar`——Tauri 是增量复制，旧的 dist-info 会一直留着）；⑤ 《开发文档 v0.3》§0 状态与 §35「当前待验证项」按实现现状逐条重写（含证据）。 |
| **v0.14** | 2026-09-23 | **修完用户实测 4.1.3 反馈的两条**（依据同一份记录第三轮，新增 §5.1.1 第 32 条）：① **推理强度存不下来**——`provider_save` 原来见"没给 `api_key` + 该 provider 需要 key"就报错，而模型面板发的正是 `apiKey: null`（"key 沿用已保存的"），于是**面板里的每次保存都在写盘前失败**，`provider.json` 停在旧版本、界面永远显示"自动"；现在只有"需要 key 且凭据管理器里也没有"才算错（`key_action()` + 4 条单测），并顺手统一 key 的 `trim()`；② **回答里带着没清掉的 JSON**——模型把信封写成 `[summary]: "…" / [findings]: […]`（不是合法 JSON）时，旧兜底把整段原文当回答；现在 `_salvage_summary()` 只取人话那一段（原文仍在时间线「输出 · 原文」里），且有反例测试保证普通回答不被截断。另：`_version.py` 改成**源树优先**，改完 `pyproject.toml` 不必再 `pip install -e` 才会报新版本（这个坑让版本一致性测试一天红两次）。 |
| **v0.15** | 2026-09-24 | **按"模型文字进界面的每一条路"整条走一遍修完**（依据《[GUI 待修问题 2026-09-24](GUI%20待修问题%202026-09-24.md)》；用户登记时只有一句"新建议题：LLM 的 MarkDown 输出"，现象未明，故四条形态逐条验，新增 §5.1.1 第 33 条）：① **正在流的那段正文是收尾信封原文**——`llm_delta` 的 `content` 发的是模型原文，界面直接交给渲染器，于是跑的过程中结论卡里是 `{"summary": …\n\n- …", "findings": […]}`（`\n` 是两个字面字符，列表 / 标题 / 表格全压成一整行），收尾后才由 sidecar 解析出人话，同一段正文前后两副面孔；现在新增 `desktop/src/envelope.ts::humanDraft()`，**上界面之前先取人话**（`summary`/`answer`/`text`、按 JSON 字符串读到收尾引号——流到一半也给到当前、解 `\n`/`\t`/`\"`/`\\`/`\uXXXX`、机器字段在前时按括号深度只认顶层键），判据与服务端 `_salvage_summary()` 同一条（**以信封开头才清理**，散文一字不动），取不到人话就显示"正在追查…"、不画半截 JSON，前后端口径用 13 个样本逐条对齐；② **短字段也过渲染器**——每条结论的 `title` 走行内渲染（`MarkdownInline`）、`summary` 走 `MarkdownView` 并保持紧凑；③ **`.md` 补齐规则**——围栏代码块（原来 `pre > code` 吃到行内 `code` 的样式，看着是"一排小药丸"，长行顶破版心）、`ol` 与 `ul` 同一条缩进（原来 `ol` 吃浏览器默认 40px）、`a` 中性下划线、`hr`/`del`/GFM 任务列表；④ **宽表格自己横向滚**（`.md-table-wrap` + `max-content`/`min-width:100%`/单元格 `34ch`），不再顶破中栏或右栏。验收方式：headless Chrome 拿**真渲染件 + 真 CSS** 渲染截图对照，并由用户装 4.1.5 实测。 |
