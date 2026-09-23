# YeLee' PacketSage — Agent CLI 工程规格书 v0.1
> **验收口径（2026-09-20）：** 本文件正文保持不变，正文内复选框不再逐条维护；
> 当前验收由《GUI 前收口文档 v0.1》§3（G1–G5）与 §4（S01–S56）接管，测试基线见该文 §2.1。
> **版本：v0.1**（继承《M0～M2 Rust 工程规格书 v0.2 评审修订版》《M3～M6 工程规格书 v0.2》全部契约；配套《开发文档 v0.3》§21/§22/§24/§25）
> **状态：工程设计 / 待评审**
> **定位：`packetsage-agent`（Python 侧）命令面的权威定义**——M3~M6v0.2§4.1 只给了拓扑级调用式、§5.3 只给了编排链引用，本规格补齐其 CLI 契约。与《CLI 收口工程规格书 v0.2》为**姊妹篇**：彼管 Rust `packetsage` 入口，此管 Python `packetsage-agent` 入口
> **标记体系：** 【事实】上游已核实 /【设计】本规格契约 /【待验证】需实现测试确认 / **【继承-…】** 直接消费已冻结契约 / **【收编-收口v0.2§x】** 权威定义自彼处移入 / **【修订-…】** 对上游的显式修订
> **总原则重申：** ADR-018——agent 进程自身**不直接连库**（`--db` 唯一去向是子进程引擎）；M2v0.2§7.3——引擎 stderr 转发本地日志文件，**绝不进入 LLM 上下文**；三流分离与"墙钟仅呈现"（ADR-015）同 Rust 侧口径。
---
## 0. 背景与边界
### 0.1 为什么需要本规格
- 【事实】《CLI 收口 v0.2》的主体是 Rust `packetsage`：`packetsage-agent` 在其中只出现两次——§2.2 安装目标（console_script）、§2.3 launcher 解析目标。其自身命令面（子命令、旗标、退出码、输出纪律、配置消费）**未成规格**；
- 【事实】既有冻结面只有拓扑级碎片：M3~M6v0.2§4.1 `run --task-id T --engine "packetsage serve" [--db …]` / `chat --task-id T`；§5.3 编排链引用 `agent(report --task-id)`；§4.2 引擎退出码映射 {0,2,3,4}；
- 本规格把上述碎片整合为可实现、可测试的完整 CLI 契约。
### 0.2 覆盖 / 不覆盖
| 覆盖 | 不覆盖 |
|---|---|
| `run`/`chat`/`report` 三子命令、全局旗标、入口与 prog 名、version 快路径、Python 侧配置消费、三流纪律、exit 矩阵、错误呈现、REPL 行为细则（收编）、与 Rust launcher/编排的接口、测试/DoD/实现顺序 | agent 内部机制（engine_client/tools/policy/prompts/provider/评测——M4 §4.2–§4.7 已冻结）、Rust `packetsage` 自身（收口 v0.2 全文有效）、报告模板与 lint 语义（M5 §5）、E 套件、Python 打包分发细节（属 M6 tarball 面） |
### 0.3 与《CLI 收口 v0.2》的分工
| 事项 | 归属 |
|---|---|
| console_script 安装、双形态一致、prog 名固定 | 收口 v0.2 §2.2【继承】；本规格 §2 消费 |
| launcher 四级解析、探测 3s 超时 | 收口 v0.2 §2.3【继承】——本规格 §2 的 version 快路径是该探测的**被依赖方** |
| REPL 行为细则（提示符/横幅/Ctrl-C/斜杠命令/usage 行） | **本规格 §5【收编-收口v0.2§10】**；彼处保留 launcher 责任与 UX 验收判据 |
| 配置文件发现链、`.env` 仅 Python 加载、脱敏规则 | 收口 v0.2 §7【继承】；本规格 §9 给 Python 侧消费细则与**跨节忽略**规则 |
| exit code 语义 0–5 | dev doc §23【继承】；本规格 §8 给 agent 侧矩阵 |
### 0.4 契约冻结声明（只实现、不修改）
| 冻结契约 | 本规格落点 |
|---|---|
| ADR-018 单一写入方 | agent 不连库；findings/report_path 全走 RPC（SUBMIT_FINDING / SUBMIT_REPORT_META） |
| 引擎退出码 ∈ {0,2,3,4} 映射【继承-M3~M6v0.2§4.2】 | §8 矩阵"引擎死亡"行透传机制 |
| EngineCrashed 不自动重启、心跳 60s×2 判 unhealthy【继承-M3~M6v0.2§4.2】 | §4 run/§5 chat 呈现层消费 |
| policy 硬门禁与降级产出（预算→强制收尾；LLM 失败重试 1 次→部分结果）【继承-M3~M6v0.2§4.4】 | §4 终态与退出码语义直接消费：**降级收尾 = exit 0 + WARN** |
| `analyze --full --report` exit 5 直至 M5【继承-M3~M6v0.2§4.8】 | §6：`report` 子命令随 M5 落地，M4 窗口 exit 5（标记解除先例同 M2v0.2§5.2） |
### 0.5 文档同步点（随本规格实施提交）
| S# | 内容 | 目标 |
|---|---|---|
| S7 | M3~M6v0.2§4.1 的命令面表述标注"权威定义见《Agent CLI 工程规格书》"；收口 v0.2 §10 标注"REPL 细则已收编" | 【M3~M6v0.2】+【收口v0.2】修订页 |
| S8 | dev doc §24 / M2v0.2§9.3 示例中 `agent.max_` 键名疑似截断（实义待 #42 回填，如 `max_tokens_total`）——文档修正 | 《开发文档》§24 修订页 |
---
## 1. 进程拓扑与三种调用路径【继承-M3~M6v0.2§4.1/§5.3】
```text
A. Rust 编排（M5 全链）：packetsage analyze <cap> --full --report r.md
   → ② spawn: packetsage-agent run --task-id T [--db …]
   → ③ spawn: packetsage-agent report --task-id T …
B. Rust launcher（交互）：packetsage chat <capture>
   → spawn: packetsage-agent chat --task-id T [--db …]（tty 继承）
C. 人工直调（开发/调试）：packetsage-agent run|chat|report --task-id T …
   —— task 必须已存在（引擎查无 → exit 3，见 §8）；不触发 analyze
```
- 三路径共用同一 `packetsage-agent` 入口与同一套参数语义；B/C 的区别只是谁负责创建 task；
- **launcher 接口契约【设计】**：Rust 侧 spawn argv = `packetsage-agent <sub> --task-id <id> [--db <url>]`；env 透传 `PACKETSAGE_CONFIG`（收口 v0.2 §7.1）；`--db` 仅当用户显式给了非默认值时透传；退出码原样上抛，launcher 不吞不改。
## 2. 入口、prog 名与 version
- 入口【继承-收口v0.2§2.2/S1】：`[project.scripts] packetsage-agent = "packetsage_agent.cli:main"` + `python -m packetsage_agent`（`__main__.py` → 同一 `main()`）；
- **prog 名固定 `packetsage-agent`**（argparse `prog=` 显式设置），双形态 `--help` 逐字节一致【继承-收口v0.2 C7 纪律】；
- **`--version` 快路径【设计；被收口 v0.2 §2.3 探测依赖】**：单行 `packetsage-agent <version>`，version 取 pyproject（PEP 621）经 `importlib.metadata`——**单一事实源**，CI 断言与 pyproject 一致；执行约束：不读配置、不 spawn 引擎、不触网、不加载 .env，启动到退出 **< 1s**（launcher 探测预算 3s，#37 同族实测）；
- 无 `version` 子命令、无 `--json`（与 Rust 侧 §5 区分：launcher 探测只认单行文本）。
## 3. 命令面总览
```text
packetsage-agent run    --task-id <id> [--engine <cmd>] [--db <url>]            批式调查：跑完即出摘要
packetsage-agent chat   --task-id <id> [--engine <cmd>] [--db <url>] [--report [<path>]]   交互 REPL
packetsage-agent report --task-id <id> [--engine <cmd>] [--db <url>] [--report <path>]    由台账生成报告（M5）
packetsage-agent --version | --help
```
- `--task-id`：必填位置性旗标，argparse 仅查非空；**存在性由引擎 RPC 判定**（不发明格式校验）；
- `--engine <cmd>`【设计】：子进程引擎命令前缀，`shlex.split` 解析；缺省 `packetsage serve`；
- `--db <url>`【设计】：**唯一去向是拼入引擎 argv**（机制以 M4 实现回填，#41）；与 `--engine` 内已含 `--db` 同时出现 → argparse 拒绝（exit 1，fail-fast）；url 出现在任何错误信息中时按收口 v0.2 §7.2 同规则脱敏（password 段 `***`）。
## 4. `run` 子命令（批式调查）
- 生命周期：spawn 引擎 → ping 握手 → agent loop（工具调用/预算/policy 硬门禁全按 M4 §4.2–§4.4，**本规格零改动**）→ finalize（SUBMIT_FINDING 批量提交）→ stdout 终态摘要 → 引擎优雅关闭；
- **进度【设计；墙钟仅呈现】**：stderr、仅 tty、`-q` 关闭——每工具调用完成一行 `#<step> <tool> ok|err`，每 LLM 轮后一行预算余量（`steps 3/12 · calls 5/24 · 4.2k tok · 3.1¢/50¢`）；
- **终态摘要（stdout）【设计】**：findings 计数（accepted / `submit_rejects`）、run 状态（completed | degraded | malformed_output 计数）、预算用量；无 `--jsonl`——机器消费走 DB/报告/评测 runner（进程内），不造第二事件流；
- **降级即完成**【继承-M3~M6v0.2§4.4】：预算强制收尾、LLM 失败重试后部分产出、unhealthy 后工具预置错误 → 均 **exit 0 + stderr WARN**（run 状态字段如实记录降级）；
- **Ctrl-C（run 中）【设计】**：首次 SIGINT → 复用 policy 的"请基于已有证据总结"强制收尾路径（优雅 finalize，已提交 findings 不回滚）→ exit 0 + stderr `run interrupted, partial results kept`；二次 SIGINT → 立即退出 exit 4（`interrupted` 落库视 #28/S3）。
## 5. `chat` 子命令（交互 REPL）【收编-收口v0.2§10 并细化】
- 启动：spawn 引擎 + ping 握手 → 横幅一行 task 概要（包数/告警数/规则数，RPC 取得）→ 进入提示符 `packetsage·<task_id 前 8 位>> `；
- **非 tty（stdin 或 stdout 非终端）→ 拒绝**：exit 1 + stderr 提示"交互模式需要终端；批式请用 `packetsage-agent run`"【设计】；未来脚本化对话需求另议，不在本版；
- 空闲 Ctrl-C：一行提示 `（/quit 或 Ctrl-D 退出）`，不退出不清屏；
- 轮中 Ctrl-C：取消当前 provider 调用【待验证 #28：轮级干净取消能力】，回到提示符；当前轮已提交的 findings 保留；
- 引擎 unhealthy / LLM 错误：REPL 错误行 + 修复提示，**进程不退出**（连错不炸；fake engine/mock provider 测试覆盖）；
- 每轮结束 stderr 一行 usage（同 §4 格式，`-q` 关）；REPL I/O 显式 UTF-8，Windows 控制台 best-effort【待验证 #43】；
- 斜杠命令仅 `/help`、`/quit`（= Ctrl-D，exit 0）；
- **`--report [<path>]`【继承-M3~M6v0.2§5.3 "chat --report 亦可用"】**：会话正常结束后生成报告，path 缺省 `report-<task_id>.md`；生成失败 → stderr WARN，**不影响 exit 0**。
## 6. `report` 子命令（M5 窗口）
- M5 §5.1 ReportGenerator 的 CLI 入口：由引擎 RPC 取数 → 渲染 `report.md.j2` → 写盘 → `SUBMIT_REPORT_META` 回填（ADR-018）→ lint；stdout 仅一行 `report written: <path> sha256=<…> degraded=<n>`；
- `--report <path>` 必填性【设计】：缺省 `report-<task_id>.md`（与 chat 一致）；
- lint 硬失败（anti-hallucination）→ **exit 4**，stderr 含关键字（对齐收口 v0.2 §4 矩阵同场景行）；
- **M4 窗口桩行为【设计；标记解除先例】**：子命令已注册但未实现 → exit 5 + stderr `report 子命令随 M5 打开`（对齐 M2v0.2§5.2 `--report` exit 5 先例）。
## 7. 全局旗标与三流
| 旗标 | 语义 |
|---|---|
| `--config <path>` | 显式配置文件（收口 v0.2 §7.1 发现链层 1） |
| `-v` / `-vv` | stderr 日志 info / debug；**默认 warn**（`-vv` 含配置 dump，脱敏同 §9） |
| `-q` | 关进度与 usage 行；错误与 WARN 仍走 stderr |
| 流 | stdout：run 终态摘要 / chat REPL / report 单行结果；stderr：日志、进度、usage、WARN、错误。**禁止**：stdout 出现日志或进度；chat 的 REPL 回显不得进 stderr |
## 8. exit code 矩阵（dev doc §23 语义的 agent 侧细化；脚本化断言入 CI）
| 场景 | 期望 |
|---|---|
| run 正常完成（含降级收尾、预算耗尽收尾） | 0（降级时 stderr WARN） |
| run Ctrl-C 优雅收尾 / chat Ctrl-D、/quit | 0 |
| run 二次 Ctrl-C 硬中断 | 4 |
| argparse 错误（缺 --task-id、--db 冲突、未知旗标） | 1 |
| chat 非 tty | 1 |
| 引擎查无 task / spawn 失败 / 心跳超时前连接失败 / LLM provider 错误 / 配置不可解析 | 3（task 不存在场景附提示：先 `packetsage analyze` 或 `packetsage chat`） |
| 引擎子进程死亡（EngineCrashed） | **透传映射**：引擎退出码 {2,3,4} → 同码；0 → 4（引擎不该静默退出）【继承-M3~M6v0.2§4.2】 |
| report：lint 硬失败 / 未捕获 Python 异常 | 4（后者附完整 traceback，见 §10） |
| report（M4 窗口桩） | 5 |
## 9. 配置消费（Python 侧）
- 发现链与键值优先级链**全量继承收口 v0.2 §7.1**（`--config` > `PACKETSAGE_CONFIG` > `./packetsage.yaml` > 内置默认；键级 CLI > env > config > defaults）；
- **节消费范围【设计】**：Python 只消费 `agent` / `llm` 两节；`engine`/`rules`/`storage` 节对本进程**合法但忽略**（它们的消费者是被 spawn 的 Rust 引擎，经同一文件/env）——未知键 fail-fast 的作用域 = `agent`/`llm` 节内（防止把 Rust 侧合法键当 Python 侧未知键误杀）；
- `.env` 由 Python 侧加载（收口 v0.2 §7.1 定稿；与 #31 呼应）；API key 只走 env/.env，出现在 config 文件 → 报错（同收口 v0.2 §7.1）；
- 消费键【继承-M2v0.2§9.3】：`agent.max_steps / max_llm_calls / max_tokens_total(键名待 #42 回填) / max_payload_bytes / prompt_version / temperature`、`llm.provider / model`；env：`PACKETSAGE_LLM_PROVIDER / MODEL / API_KEY`；
- 脱敏：doctor 式展示与 `-vv` dump 按收口 v0.2 §7.2（分段精确匹配，`max_tokens_total` 不误伤）。
## 10. 错误呈现
- 模板三要素同收口 v0.2 §9（标识 + 原因 + 建议动作）；
- LlmError → provider/model + env 变量名（不回显 key）；
- 引擎 spawn 失败 → 完整引擎命令回显（`--db` url 脱敏）+ PATH/安装提示；
- EngineCrashed → 引擎退出码 + 本地日志文件路径（stderr 转发目的地【继承-M2v0.2§7.3】）+ 尾部 3 行摘要；
- **traceback 策略【设计；与 Rust 侧 panic 摘要有意不同】**：受众是开发者——未捕获异常默认完整 traceback 上 stderr + exit 4，不压缩成一句"internal error"。
## 11. 测试矩阵与 DoD
| 层 | 用例 |
|---|---|
| 入口 | 双形态 `--help` 逐字节一致；prog 名断言；任意目录可用 |
| version | 快路径单测（monkeypatch 断言未触网/未 spawn/未读配置）+ 启动耗时 < 1s（CI 宽松阈值） |
| run | §8 矩阵逐行（fake engine + mock provider）；降级三场景各 exit 0 + WARN；Ctrl-C 优雅收尾断言**部分 findings 已入库**；进度/usage 行仅 tty |
| chat | pexpect/pty 驱动：横幅、提示符、空闲/轮中 Ctrl-C、连错不炸、/quit=0、--report 生成（Linux/macOS；Windows 跳过 #43）；非 tty → 1 |
| 三流 | stdout 无日志/进度（断言）；REPL 回显不进 stderr |
| 配置 | 跨节忽略（engine 节存在不报错）；agent/llm 节内未知键 exit 3；.env 加载；`max_` 键名回填后收紧断言 |
| 集成 | Rust `packetsage chat` 端到端（launcher→agent→引擎，人工验收脚本，对齐 M4 DoD 既有项） |
**DoD**
- [ ] 三子命令 + 全局旗标落地，§8 矩阵脚本化全绿入 CI
- [ ] version 快路径与双形态一致测试绿
- [ ] REPL pty 用例绿（#43 平台除外）；三流断言绿
- [ ] Python 侧配置用例绿（含跨节忽略）
- [ ] S7/S8 提交；#41–#43 结论回填
## 12. 实现顺序（A1–A6；与收口 v0.2 C 批次并行不冲突，C1 的 Python 半边并入 A1）
```text
A1 入口+prog 名+version 快路径+双形态一致       （半天）
A2 配置消费：发现链/env/.env/跨节忽略/脱敏       （半天）
A3 run：spawn+心跳+进度+终态摘要+退出码（fake engine）（1 天）
A4 chat/REPL：收编细则+Ctrl-C+usage 行+--report  （1 天）
A5 错误呈现+exit 矩阵全绿+traceback 策略          （半天）
A6 report 子命令（M5 窗口）+Rust 编排集成验证     （半天，随 M5）
```
## 13. 待验证项（接续提示词规格 #40 → #41+）
| # | 项 | 时点 |
|---|---|---|
| 41 | `--db` 透传机制在 M4 实现中的既有形态（拼入引擎 argv vs env）与 engine cmd 已含 `--db` 的冲突处置回填 | A3 前 |
| 42 | dev doc §24 / M2v0.2§9.3 `agent.max_` 键名实义（疑为 `max_tokens_total` 截断）——以实现为准回填并触发 S8 | A2 前 |
| 43 | Windows 控制台 REPL：pty 测试替代方案 + GBK 编码下 UTF-8 I/O（best-effort，对齐收口 v0.2 #29） | A4 后 |
---
## 附：与上游契约的衔接自查
| 上游条目 | 本规格落点 | 判定 |
|---|---|---|
| M3~M6v0.2§4.1 `run/chat --task-id` 拓扑式 | §1/§3 | 一致；补旗标与缺省值 |
| M3~M6v0.2§5.3 编排链（run→report 两段 spawn） | §1 路径 A / §6 | 一致；`report` 补为第三子命令 |
| M3~M6v0.2§4.2 引擎退出码 {0,2,3,4} 映射 | §8 | 一致；补"引擎 0 退出→4" |
| M3~M6v0.2§4.4 降级产出=部分结果 | §4 | 一致；定 exit 0 + WARN |
| M3~M6v0.2§4.8 `analyze --full --report` M5 打开 | §6 桩行为 | 一致；标记解除先例复用 |
| M2v0.2§7.3 引擎 stderr 不入 LLM 上下文 | §10 呈现层 | 一致；转发日志路径提示 |
| 收口 v0.2 §2.2/§2.3/§7/§9/§10 | §0.3 分工表 | 继承/收编边界显式化 |
| dev doc §23 exit 0–5 | §8 | 不新增、不重定义 |

---
## 14. 实施回填（本次重构落地记录）

> 本节由实施方追加，记录 §12/§13 的落地结论与显式偏离；逐条证据见
> [Agent CLI 同步点 S7-S8.md](Agent%20CLI%20同步点%20S7-S8.md)。

- **A1–A5 已落地**（入口/prog 名/version 快路径/配置消费/run/chat/错误呈现与退出矩阵），
  A6 的 `report` 子命令按 §6"随 M5 落地"实现为**真实渲染**而非 exit 5 桩——本仓库
  `agent/packetsage_agent/report.py` 与九节报告测试已在 M5 完成；
- **§13 #41 回填**：`packetsage serve` 无 `--db` 旗标，`--db` 走子进程环境
  （`PACKETSAGE_STORAGE_URL`/`PACKETSAGE_DB`），`--engine` 已含 `--db` 时 argparse 拒绝；
- **§13 #42 回填**：预算键实义为 `agent.max_tokens_total`（旧拼写 `max_tokens` 仍接受，
  同时出现报错），S8 已提交；
- **§13 #43 回填**：`chat` 非 tty 直接 exit 1；PTY 用例在 POSIX 跑、Windows 跳过；
  stdout/stderr 的 error handler 放宽为 `replace` 以免 GBK 控制台把好运行变成 exit 4；
- **`run`/`chat` 不再接受 `--capture`/`--provider`/`--model`/`--scenario`**，`eval` 子命令
  移出（保留 `python -m packetsage_agent.eval.run_eval`）：命令面严格等于 §3；
- **新增 `setup` 子命令（§3 的显式扩展，v0.2 候选）**：`packetsage-agent setup` 是本产品的
  **第一步**——交互选 provider（`mock|openai|deepseek|local`）、填 model/base_url/API key，
  校验端点后写入 `agent/.env`（gitignored；永不写 `packetsage.yaml`），并打印下一步。
  同时把"未配置 provider"从旧的 `mock` 静默兜底改成 **exit 3 + setup 提示**：
  `run`/`chat` 在 provider 未配置或缺少 key 时不启动 Agent 循环；`mock` 只在显式指定时生效
  （CI/演示/E 套件）；`analyze --full` 也按配置错误 exit 3，而不是降级 WARN；
- **`deepseek` 成为一等 provider**（默认 `https://api.deepseek.com/v1` + `deepseek-chat`），
  使 dev doc §24 的示例配置真正可用；
- **任务可见性**：`--task-id` 指向的任务由 `serve` 在 dispatch 前按 ADR-019 冷恢复
  （M3~M6§3.8 的 store-backed 重建在本实现中取"按 `source_path` 重放"的等价路径），
  因此路径 A/B/C 三条调用链都能看到 `analyze` 阶段创建的 task。
