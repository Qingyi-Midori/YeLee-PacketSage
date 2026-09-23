# Agent 提示词同步点 S5–S6（《Agent 系统提示词规格 v0.1》§7 / §8 执行记录）

记录 SYSTEM_PROMPT v2 的落地：文本来源、渲染契约、同步点落点、验证证据与
实施中的显式追加。逐条命令面契约见 [Agent CLI 同步点 S7-S8.md](Agent%20CLI%20同步点%20S7-S8.md)。

## 1. 文本来源（不手抄）

| 文件 | 来源 | 说明 |
|---|---|---|
| `agent/packetsage_agent/prompts_text/v2.txt` | 《Agent 系统提示词规格 v0.1》§2 ```text 块**机械抽取** | v2 全文（R1–R6 / 工具策略 / 调查纪律 / run+chat 输出块 / 预算 / 语气），5333 字符 |
| `agent/packetsage_agent/prompts_text/v1.txt` | 《M3～M6 工程规格书 v0.2》§4.5 ```text 块**机械抽取** | v1 全文，回滚基线与 E 套件对比锚点 |
| `agent/packetsage_agent/prompts_text/chat_zh.txt` | `System Prompt-v0.1.txt`（工作目录内的中文提示词） | chat 模式追加的中文回答格式（见 §4） |

抽取是"从规格文本到包内数据文件"的机械操作，没有人手转写，因此
`PROMPTS["v2"]` 与规格 §2 逐字节一致（`tests/test_prompts.py` 用渲染快照锁定）。

## 2. 渲染契约（§4）

```python
render_system_prompt(task_id: str, mode: Literal["run", "chat"] = "run",
                     version: str | None = None) -> str
```

- 唯一内容替换 `{task_id}`；唯一结构选择：保留 `## Output — run mode` **或**
  `## Output — chat mode` 块（`prompts.RUN_BLOCK` / `CHAT_BLOCK` / `TAIL_BLOCK` 常量即边界）；
- 渲染器只有 `task_id` / `mode` / `version` 三个入参，pcap 内容在类型上无法进入 prompt；
- 版本集合 `PROMPTS = {"v1", "v2"}`，默认 `DEFAULT_PROMPT_VERSION = "v2"`；
  未知版本/模式抛 `PromptVersionError`，config 里写错 → exit 3（fail-fast，不静默回落）。

## 3. 同步点

| S# | 规格要求 | 落点 | 证据 |
|---|---|---|---|
| S5 | config 示例默认 `agent.prompt_version: "v1"` → `"v2"`（值级修订） | **目标文档（M2v0.2§9.3）不在本仓库**，故在此留痕：默认值由 `prompts.DEFAULT_PROMPT_VERSION` 单点定义，`config.py` 校验取值、`report.py` 回填落库 | `tests/test_prompts.py::test_both_versions_ship_and_v2_is_the_default`、`tests/test_report.py::test_report_carries_the_selected_prompt_version` |
| S6 | M3~M6v0.2§4.5 标注"v1 由 v2 取代（v1 保留）"；开发文档 §16 标注"最低基线，v2 为超集实现" | [M3～M6 工程规格书 v0.2.md](M3～M6%20工程规格书%20v0.2.md) §4.5 同步点块；[开发文档 v0.3.md](开发文档%20v0.3.md) §16 同步点块 | 同上 + `cargo test --workspace` 不受影响（prompt 不在 Rust 侧） |

## 4. 显式追加：chat 模式的中文回答格式

规格 §2 的 v2 全文对 chat 只说"用用户的语言、引用证据 id"。工作目录里的
`System Prompt-v0.1.txt` 提供了更具体的中文回答结构，因此把它（输出格式 / 约束与原则 /
交互风格三节，标题降一级后）作为**静态块追加在 chat 模式**，run 模式逐字节不受影响：

- 落点：`prompts_text/chat_zh.txt`，由 `_select_mode()` 在 mode=chat 时插入到
  `## Output — chat mode` 与 `## Budget and stopping` 之间；
- 位置：`tests/snapshots/prompt-v2-chat.txt`（chat 快照含中文块）、
  `tests/snapshots/prompt-v2-run.txt`（run 快照不含），两者都在 `tests/test_prompts.py` 锁定；
- 判定：属于"结构块内的静态追加"，不影响§4 的"唯一结构选择"约束；若评审要求 chat 与规格
  §2 逐字节一致，删掉 `chat_zh.txt` 即可（渲染器对空文件自动跳过）。

## 5. 链路接入（prompt 真的被用上）

| 环节 | 改动 |
|---|---|
| provider | `OpenAIProvider(mode, prompt_version)` 用 `render_system_prompt(task_id, mode, version)` 渲染 system message（`local` 同路径；`mock` 是脚本回放，不消费 prompt） |
| agent | `build_agent(..., mode="run"|"chat", prompt_version=None)`；`AgentRunResult.prompt_version` 记录本 run 实际使用的版本 |
| CLI | `run` → mode=run、REPL 每轮 → mode=chat，均传 `settings.prompt_version`（`--config`/`PACKETSAGE_CONFIG` 可选 v1） |
| report | `ReportGenerator(prompt_version=…)` 默认取 `prompts.PROMPT_VERSION`（v2）并随 `SUBMIT_REPORT_META` 落 `agent_runs.prompt_version` |

## 6. 验证证据（本机实测）

| 层 | 用例 | 结果 |
|---|---|---|
| 渲染 | run/chat 双快照逐字节；`{task_id}` 白名单；task_id 含中文原样替换；未知版本/模式报错 | `agent/tests/test_prompts.py`（11 项） |
| 落库 | `SUBMIT_REPORT_META.prompt_version` 默认 `v2`、显式 `v1` 时透传 | `tests/test_report.py::test_report_carries_the_selected_prompt_version` |
| 回归 | 全量 Python 148 项 + Rust 175 项 + CLI 契约 43 项 + GUI 契约 7 项 | 全绿（2026-09-20 收口复测） |

## 7. 待验证项（承 §9 #38–#40）

| # | 项 | 现状 |
|---|---|---|
| 38 | v1↔v2 真实模型对比（pinned 夜间） | 未跑：需要真实 provider key；mock 路径不受 prompt 影响，故先以快照 + 结构测试锁定文本，夜间跑出报告后决定是否维持 v2 默认 |
| 39 | chat 多轮下 system prompt 稳定性 | 每轮重新渲染同一静态文本；chat 快照已锁定，真实模型行为待测 |
| 40 | v2 渲染 token 占比 | 可先测：run 5039 字符 / chat 5124 字符（含中文块），中文块本身 500 字符 |
