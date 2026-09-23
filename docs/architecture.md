# PacketSage 架构与实现边界

> 本文件记录实现与《M0～M2 Rust 工程规格书》《M3～M6 工程规格书 v0.2》之间的
> 对应关系，以及所有**已登记的实现偏差**。规格书是契约，本文件是“契约 → 代码”的地图。

## 1. 分层与依赖方向

```text
packetsage-cli ──▶ core ──▶ protocol
      │              │
      ├──▶ storage ──┤        (sqlx 只出现在 packetsage-storage)
      └──▶ rules ────┴──▶ protocol + core(只读视图)
packetsage-fuzz ──▶ protocol + core + rules
agent(Python) ──stdin/stdout JSONL──▶ packetsage serve   (唯一出口, ADR-018)
```

* `packetsage-protocol`（L0）：纯 DTO / schema，无 IO、无时钟、无随机数。
* `packetsage-core`（L1）：reader / decoder / reassembly / conversation / query / pipeline。
  唯一允许出现 `pcap-parser`、`etherparse` 的 crate。
* `packetsage-rules`（L2）：YAML DSL、S1–S9 静态校验、滑动窗口评估器。
  通过 `core::pipeline::RuleHook` 反向注入，保证 `core` 不依赖规则引擎。
* `packetsage-storage`（L1'）：`Repository` trait + SQLite 实现 + migrations。
* `packetsage-cli`：只做参数解析、装配与格式化输出。
* `agent/`（Python）：engine client、工具 envelope、policy、provider、报告与评测。

## 2. 事件与 ID 契约

| 契约 | 位置 |
|---|---|
| `SCHEMA_VERSION = 2`（ADR-013） | `crates/packetsage-protocol/src/lib.rs` |
| `EngineEvent`（`task_started`…`alert`…`task_finished`） | `protocol::events` |
| 工具结果信封 `{_id, source, trusted_as_instruction, redactions, content}` | `protocol::rpc::ToolEnvelope` |
| tc ID `tc_{ulid}` | Rust 在产生 envelope 的同一路径 mint（ADR-024），Python 只引用 |
| `F-{n:03}` finding id | Rust 在 `submit_finding` 分配（ADR-018） |
| `S-{n:06}` session id | 会话聚合器按出现顺序分配 |
| EvidenceRef / V1–V4 | `protocol::evidence`（Rust），V5 + 报告全文 lint 在 Python（ADR-023） |

## 3. 规格条目 → 代码位置

| 规格 | 实现 |
|---|---|
| M0 §2 workspace / lints | `Cargo.toml`、`rust-toolchain.toml` |
| M0 §5.2 serve `ping` | `crates/packetsage-cli/src/serve.rs` |
| M0 §5.4 exit codes | `crates/packetsage-cli/src/exit.rs` |
| M1 §4.3 reader | `crates/packetsage-core/src/reader.rs` |
| M1 §4.4 decoder 矩阵 | `crates/packetsage-core/src/decoder/{mod.rs,app.rs}` |
| M2 §4.5 reassembly | `crates/packetsage-core/src/reassembly.rs` |
| M2 §4.6 conversation | `crates/packetsage-core/src/conversation.rs` |
| M2 §4.7 query | `crates/packetsage-core/src/query.rs` |
| M2 §4.8 pipeline | `crates/packetsage-core/src/pipeline.rs`、`src/store.rs` |
| M3 §3.1–3.6 rules | `crates/packetsage-rules/src/{schema,loader,evaluator}.rs` |
| M3 §3.7 storage | `crates/packetsage-storage/src/{sqlite,repository}.rs`、`migrations/` |
| M4 §4.2 engine_client | `agent/packetsage_agent/engine_client.py` |
| M4 §4.3 tools | `agent/packetsage_agent/tools.py` |
| M4 §4.4 policy | `agent/packetsage_agent/policy.py`、`agent.py` |
| M4 §4.5 prompts | `agent/packetsage_agent/prompts.py` |
| Agent 系统提示词规格 v0.1（SYSTEM_PROMPT v2） | `agent/packetsage_agent/prompts_text/{v1,v2,chat_zh}.txt`（数据文件，机械抽取自规格）+ `prompts.py`（唯一渲染器） |
| CLI ASCII 标题 | 两边各自硬编码：`crates/packetsage-cli/src/banner.txt`（编译期 `include_str!`）与 `agent/packetsage_agent/banner.txt`（包数据，`banner.py` 读取） |
| M5 §5.2–5.4 report | `agent/packetsage_agent/report.py` |
| Agent CLI v0.1 §2–§10 命令面 | `agent/packetsage_agent/cli.py`（入口/退出矩阵/三流）、`progress.py`（进度与 usage 行）、`config.py`（agent/llm 节消费与跨节忽略） |
| M6 §6.1 fuzz | `crates/packetsage-fuzz/`、`fuzz/fuzz_targets/` |
| M6 §6.2 benchmark | `scripts/gen_traffic.py`、`scripts/bench.py` |
| M6 §6.4 doctor | `crates/packetsage-cli/src/doctor.rs` |
| M6 §6.5 error codes | `docs/error-codes.md` |

## 4. 已登记的实现偏差

| # | 偏差 | 原因 | 状态 |
|---|---|---|---|
| D-5 | `PacketSource::advance()/current()` 借用式 API 改为 push 式 `CaptureReader::drive(closure)`，回调同时拿到接口表 | `pcap-parser` 的 block 借用 reader 环形缓冲，`current()` 需要自引用结构，而 workspace `forbid(unsafe_code)` | 已实现并测试 |
| D-6 | Golden/测试使用 `task_TEST0000000000000000000000`（26 字符） | 规格 §8.2 的 `task_TEST00000000000000000000` 只有 24 字符，不是合法 ULID | 已实现（`protocol::GOLDEN_TASK_ID`） |
| D-7 | ~~`direction_basis` 使用自拟值 `first_packet`~~ | 已按 ADR-025 改回权威枚举 `first_seen`；会话事件仍按规范五元组字典序统计 `src_*`/`dst_*`，客户端方向由 `direction_basis` + DTO 的 `client_*`/`server_*` 字段表达 | **已关闭**（ADR-025） |
| D-8 | `packetsage-storage` 的 PostgreSQL 支持是默认关闭的 cargo feature（`--features postgres`） | `sqlx-postgres` 的 Windows 依赖链中有一个构建脚本被主机杀软隔离（本机无法加入白名单，非管理员），导致整机无法编译；SQLite 路径不受影响 | 代码已就绪，PostgreSQL matrix 需在 CI/Linux 上验证 |
| D-9 | 验证码（V1–V4）在 Rust、V5 与报告全文 lint 在 Python；`query_history kind=trace` 暴露 tc 台账 | 与 ADR-023 一致；`trace` 是报告第 9 节的数据源 | 已实现 |
| D-10 | 规则窗口预算超限时保留“最新一半事件”并标记 `degraded`，而不是保留计数摘要 | 计数摘要无法与滑动窗口语义共存（窗口滑动后计数失效）；证据侧按规格显式降级（`sample_packets=[]` + `degraded=true`），欠计只会漏报不会误报 | 已实现并测试 |
| D-11 | `ratio` 分子子条件以 `threshold.field_gt` 编码 | 落实待验证项 #26（规格 §3.3-2 语义已定稿，编码形式未定） | 已实现并测试 |
| D-12 | 本机 MSRV 声明为 1.80，实测使用 stable 1.98（GNU host） | 机器上没有 1.80 工具链；`rust-toolchain.toml` 固定 stable，CI 的 `msrv` job 用 1.80 单独验证 | CI 待验证 |
| D-13 | argparse 版 agent CLI（未安装 langchain 时）走同一套工具循环 | `requires-python >=3.10`（规格写 <3.13，本机只有 3.14）；`langchain` 作为可选依赖，工具循环与 policy 不依赖它 | 已实现；E1–E6 用 mock provider 全绿 |
| D-14 | 报告第 9 节的 trace 增加 `stage=agent|report` 列；`max_tool_calls`/`max_llm_calls` 只计 agent 阶段 | 报告采集本身必须走 RPC（§5.1），其 tc 台账与 agent 阶段混在一起会让人/LLM 误判为“Agent 复读”；预算只约束 Agent 行为 | 已实现（`ReportGenerator.own_calls` + policy 只被 agent 循环调用） |
| D-15 | agent CLI 命令面按《Agent CLI 工程规格书 v0.1》重构：`run/chat/report` + `--task-id/--engine/--db`，移除 `--capture`/`--provider/--model/--scenario` 与 `eval` 子命令 | 命令面收口到规格；键级覆盖走 `agent`/`llm` 节 + 环境变量，评测改走 `python -m packetsage_agent.eval.run_eval` | 已实现并测试（`agent/tests/test_cli.py` 111 项） |
| D-16 | `serve` 冷恢复（ADR-019）取“按库中 `source_path` 重放同一 pipeline”的等价路径，而非 §3.8 的 store-backed 元数据重建 | 让“另一个进程分析的 task”可被 `packetsage-agent --task-id` 服务（三条调用链的前提）；store-backed 重建仍是 M3 未勾选项 | 已实现；端到端用例见 `tests/cli/exit_codes.py::agent run over a persisted task` |
| D-17 | 工具调用台账的**证据事实**（`numbers` / `ref_ids` / `tokens`）随 `tool_calls` 行持久化（migration `0002_tool_call_evidence_facts.sql`），冷恢复时重新 issue 回内存台账 | `run` 与 `report` 是两个进程（Agent CLI §6）：报告的反幻觉 lint 需要"这次任务产出过的所有数字/实体"才能证明 stored finding 的正文，否则跨进程报告会被误判为编造（实测：`Executive Summary cites 30, 54 …` → exit 4） | 已实现；`serve::collect_citable_tokens` + trace 暴露 + harness 跨进程报告断言 |
| D-18 | 报告反幻觉 lint 的合法事实集 = 本进程 RPC 数据 ∪ 台账持久化事实（D-17）；**不放宽** Executive Summary 硬失败 | V5（Python 侧自由文本数字扫描）尚未实现，报告 lint 是模型自由文本数字的唯一防线，因此不能把 stored finding 正文直接当合法事实 | 已实现并测试（`test_executive_summary_hallucination_is_a_hard_failure` 仍绿） |
| D-15 | 规则 match 新增 `any_port`（`either_port` 为别名）+ 静态校验 S10 | `src_port`/`dst_port` 各自只能命中一侧，而 DNS 响应把 53 放在 src 侧；没有这个原语，隧道类规则只能靠“拆成两条规则”或漏掉一半流量。S10：该字段要求 protocol ∈ {tcp, udp}（与 src/dst_port 同时出现仅告警） | 已实现（schema + evaluator + 3 个单测） |

## 5. 关键不变量

1. **包时间**：规则窗口、冷却、会话老化、flush 全部使用包时间戳（ADR-015）。
2. **单源路由**：`check_alerts` 命中内存就不查库，绝不合并（C8）。
3. **证据锚点唯一**：只有 Rust 分配的 `tc_{ulid}` 能进 evidence（B2 + ADR-024）。
   工具结果同时在 `content` 里公布 `_anchor`（tc id）、`_ref_ids`（该结果可达的实体）
   与 `_numbers`（该结果的数值字段），供模型逐字复制（ADR-026）。
4. **stdout/stderr 隔离**：JSONL 事件与 RPC 走 stdout，日志走 stderr。
5. **不崩溃**：解析路径上任何输入都不得 panic；`fuzz-smoke` 与 6 个 fuzz 目标背书。
6. **流级断言**：golden/集成测试必须对拍 **per-session per-direction** 的
   `bytes` 与 `missing_ranges`，而不只是包数/字节总量——总量在结构上抓不到方向错误
   （`crates/packetsage-core/tests/flow_directions.rs` 同时钉死正例与方向错例）。
7. **零长度段**：SYN 占 1 个序号、纯 ACK 不占序号空间且永不判 Retransmission
   （`reassembly::tests::pure_ack_*`、`bare_syn_consumes_one_sequence_number`）。
