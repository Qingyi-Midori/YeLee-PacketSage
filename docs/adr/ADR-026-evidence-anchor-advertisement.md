# ADR-026：工具结果内显式公布 `_anchor` / `_ref_ids` / `_numbers`

**状态：已采纳**（来自首次真实模型评测的行为数据）

**背景：** 真实模型（deepseek-flash）跑 E1–E6 时出现两类稳定的证据引用错误：

1. 把 `alert_id` 当 envelope `_id` 引用 → **V2 拒**（"evidence id alert_... was never
   issued by this engine"），首次运行 6 场里 5 场命中该模式；
2. 自行编造 `ref_id`（如 `"packets 10-29"`、`"capture summary"`）→ **V3 拒**。

原因是工具结果里同一个 JSON 同时出现 `alert_id`、`session_id`、`rule_id` 等实体 id，
而唯一合法的锚点是 envelope 的 `_id`（ADR-024），模型无从分辨。

**决议：** 引擎在 `tool_result` 生成 envelope 的同一处，把三类"可引用事实"写进
`content`：

| 字段 | 含义 | 对应校验 |
|---|---|---|
| `_anchor` | 该结果的 tc id（与 envelope `_id` 同源、同一条语句写入，不会漂移） | V2 |
| `_ref_ids` | 通过该结果可达的实体 id（会话 / 告警 / 规则 / 包号，上限 200 条） | V3 |
| `_numbers` | 台账中该结果的数值字段（点号路径 → 数值） | V4 |

系统提示词同步声明：`EvidenceRef._id` 只能取 `_anchor`；`ref_id` 只能逐字复制
`_ref_ids`；数字只能取自 `_numbers`。

**证据（同一模型、同一 `samples/synth-mixed.pcap`）：**
修复前 E3/E5 出现 V2（alert id 当锚点）与 V3（编造 ref_id）拒绝；
公布三个字段并更新 prompt 后，E3/E5 的 V1–V4 拒绝**清零**，六场证据有效率均 1.00，
残留拒绝仅为"引用了该结果不可达的 ref"（V3 正常工作）。

**实现落点：** `crates/packetsage-cli/src/serve.rs`（`tool_result`）、
`agent/packetsage_agent/prompts.py`（锚点纪律）。
