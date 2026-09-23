# ADR-024：tool_call_id 采用 `tc_{ulid}`，废除计数器方案

**状态：已采纳**（取代仓库版《M3～M6 工程规格书 v0.2》§1.2 / §4.3 中的
`tc-{n:06d}` 条目；本文档是唯一权威）

**背景：** tc ID 是 `tool_calls` 的全局主键并进入 evidence 链。计数器格式要求
“游标随任务状态持久化 + serve 重启续读”，该机制未实现，且引入独立失败模式
（游标丢失 → 重启后主键碰撞 → V2 校验假阴性）。`ulid` 已是 workspace 依赖
（`task_{ulid}` / `alert_{ulid}` 同源）。

**决议：**

1. 格式 `tc_{ulid}`；`serve` 在产生 envelope 的同一路径 mint 并写入
   `tool_calls`（继承原 §1.2 硬契约，不变）。
2. 唯一性域 = 全局（跨任务、跨 `serve` 重启），由 ULID 结构保证；
   原“游标持久化”条款随之废除——是删一条 TODO，不是建一套机制。
3. ULID 字典序 = 时间序，报告 Evidence 索引与 trace 排序语义不变。
4. 代价：报告引用中 ID 变长（约 26 字符），可接受。

**权威 ID 表（以本 ADR 为准）：**

| ID | 格式 | 生成方 | 生成时机 | 用途 |
|---|---|---|---|---|
| RPC transport id | uuid4 | Python | 请求发出前 | 请求/响应配对；禁止进 evidence |
| `tool_call_id` | `tc_{ulid}` | Rust engine | 产生 envelope 同路径 | 证据引用唯一键 = `tool_calls` 主键 |
| `task_id` | `task_{ulid}` | Rust engine | 任务创建 | 任务主键 |
| `alert_id` | `alert_{ulid}` | Rust engine | 规则命中 | 告警主键 |
| `session_id` | `S-{n:06}` | Rust engine | 会话首次出现 | 会话编号（任务内唯一） |
| `finding_id` | `F-{n:03}` | Rust engine | `submit_finding` | finding 编号 |

**实现落点：** `packetsage_protocol::ids::{format_tool_call_id, validate_tool_call_id}`、
`packetsage_core::AnalysisStore::next_tool_call_id`、`packetsage-cli::serve::tool_result`；
格式校验与拒绝路径见 `validate_tool_call_id` 的单测与
`agent/tests/test_engine_client.py`。
