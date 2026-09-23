# Agent CLI 同步点 S7–S8（《Agent CLI 工程规格书 v0.1》§0.5 / §12 / §13 执行记录）

本文件记录本次重构（Agent CLI v0.1 落地）的同步点落点、待验证项结论与
实施中的显式偏离，供评审按条目核对。命令面契约见
[Agent CLI 工程规格书 v0-1.md](Agent%20CLI%20工程规格书%20v0-1.md)。

## 1. 文档同步点

| S# | 规格要求 | 落点 | 证据 |
|---|---|---|---|
| S7 | M3~M6v0.2 §4.1 标注"命令面权威定义见《Agent CLI 工程规格书》"；收口 v0.2 §10 标注"REPL 细则已收编" | [M3～M6 工程规格书 v0.2.md](../specs/M3～M6%20工程规格书%20v0.2.md) §4.1 同步点块；[CLI 收口工程规格书 v0.2.md](CLI%20收口工程规格书%20v0.2.md) §10 首行 | 本文件 §4 的测试矩阵全绿 |
| S8 | 开发文档 §24 / M2v0.2§9.3 的 `agent.max_` 键名回填为 `max_tokens_total` | [开发文档 v0.3.md](../specs/开发文档%20v0.3.md) §24 示例 + 同步点块 | `agent/tests/test_cli.py::test_token_budget_accepts_both_spellings` |

## 2. 待验证项回填（§13 #41–#43）

| # | 项 | 结论 |
|---|---|---|
| 41 | `--db` 透传机制与 `--engine` 已含 `--db` 的冲突处置 | `packetsage serve` **没有** `--db` 旗标（`Serve` 变体无字段），拼 argv 会被 clap 拒绝；实现取既有形态：`--db <url>` 写入子进程环境 `PACKETSAGE_STORAGE_URL` + `PACKETSAGE_DB`（`serve.rs` 的 `env_first` 顺序读它）。`--engine` 中已含 `--db`/`--db=` 时 argparse 层 fail-fast（exit 1），避免两个存储 URL 并存 |
| 42 | `agent.max_` 键名实义 | `max_tokens_total`（token 预算，默认 200000）。Python 侧 `max_tokens_total` 与历史 `max_tokens` 都接受，同时出现报配置错（exit 3） |
| 43 | Windows 控制台 REPL 与 GBK 下的 UTF-8 I/O | ①`chat` 在非 tty 上直接 exit 1（§5），pty 用例在 POSIX 跑、Windows 跳过；②`cli._make_streams_safe()` 把 stdout/stderr 的 error handler 放宽为 `replace`，进度行的 `·`/`¢` 在 GBK 控制台退化为替换字符而不是 `UnicodeEncodeError`；③真正交互时由 Rust launcher 透传 `PYTHONIOENCODING=utf-8` / `PYTHONUTF8=1` |

## 3. 实施中的显式偏离（需评审确认）

| 项 | 规格原文 | 实施 | 理由 |
|---|---|---|---|
| `report` 子命令 | §6：M4 窗口内为桩（exit 5），随 M5 打开 | 直接实现真实渲染（M5 已落地：`agent/packetsage_agent/report.py` + 九节报告测试） | 本仓库已越过 M4 窗口，桩行为会把已实现能力退回 |
| `run`/`chat` 的 `--capture`、`--provider`、`--model`、`--scenario` | §3 命令面只有 `--task-id`/`--engine`/`--db`（+`chat`/`report` 的 `--report`） | 全部移除：provider/model/scenario/budget 只走 `agent`/`llm` 节 + 环境变量（§9 键值优先级链） | 命令面收口到规格；键级覆盖由 `PACKETSAGE_LLM_PROVIDER/MODEL` 承担，Rust launcher 负责导出生效值 |
| `eval` 子命令 | §3 未列；§0.2 明示评测不属本规格 | 从 `packetsage-agent` 移除，保留模块入口 `python -m packetsage_agent.eval.run_eval` | 评测是开发工具（M3~M6§4.7），不该占据产品命令面 |
| 引擎冷恢复（ADR-019） | M3~M6§3.8：内存 miss → store-backed 重建元数据 + 源文件按需 payload | 在 `serve` 的 dispatch 前做**重放式冷恢复**：按库中 `source_path` 重跑同一条 pipeline（同 task_id）后入内存；源文件缺失仍走 `CAPTURE_UNAVAILABLE` | `--task-id` 契约要求"另一个进程分析的 task 可被 serve 服务"；store-backed 重建属 M3 未勾选项（§3.9 DoD），重放是等价且小得多的落地路径，已在 §4 用端到端用例锁住可观察行为 |

## 4. 验证证据（本机实测，Windows / rustc 1.98.1 / Python 3.14.3）

| 层 | 用例 | 结果 |
|---|---|---|
| 入口/version | 双形态 `--help` 逐字节一致、`--version` 单行且不读配置（<1s） | `agent/tests/test_cli.py`（Python 共 143 项全绿） |
| exit 矩阵 | argparse=1、非 tty=1、查无 task=3、spawn 失败=3、引擎死亡 `{2,3,4}` 透传、`0`→4、lint 硬失败=4、未捕获异常=4+traceback | `agent/tests/test_cli.py::test_*exit*` / `test_engine_death_passes_the_exit_code_through` |
| 三流 | stdout 只有产物（run 摘要/REPL/report 行）；进度与 usage 仅 stderr 且仅 tty；`-q` 关闭 | `test_run_keeps_progress_off_stdout_but_writes_it_to_a_tty` 等 |
| 配置 | 跨节忽略（`engine`/`rules`/`storage` 合法且忽略）；`agent`/`llm` 未知键 exit 3；`.env` 加载；脱敏 | `agent/tests/test_cli.py` + `agent/tests/test_config.py` |
| Rust 侧 | `chat` 先解析 launcher（tarball 无 agent → exit 3 + 四级提示）；`analyze --full` = analyze+落库 → `run` → `report` | `cargo test --workspace`（168 项）+ `tests/cli/exit_codes.py`（43 项） |
| 安装 | 两条安装路径 + tarball 能力矩阵 + 未配置 provider 的 setup 提示 | `python scripts/install_smoke.py --run-cli`（21/21，见 `install-test/RESULTS.md`） |
| 端到端 | `analyze --db` → `packetsage-agent run --task-id`（跨进程冷恢复）→ `F-001/F-002` 入库；`analyze --full --report` 全链 | `tests/cli/exit_codes.py::agent run over a persisted task`、`packetsage analyze --full --report` 手工实测（见 §5） |

## 4a. 实施期追加：先配置，再运行（provider 默认"未配置"）

| 项 | 旧行为（问题） | 现行为 |
|---|---|---|
| 默认 provider | 两侧都默认 `mock`：不配置也能 `run`/`chat`，输出的是**脚本回放**的结论，看起来像真实分析 | 默认 **未配置**：`run`/`chat`/`analyze --full` 直接 exit 3 并提示 `packetsage-agent setup`；`mock` 只在显式指定时生效（CI/演示/E 套件） |
| 配置入口 | 只能靠手写 `agent/.env` 或 export 环境变量 | 新增 `packetsage-agent setup`（§3 扩展）：交互选 provider（`mock|openai|deepseek|local`）→ model/base_url → key（不回显）→ `GET /models` 校验 → 写 `agent/.env`（0600、gitignored、永不写 `packetsage.yaml`）；`--print` 只预览（key 打码）、`--no-verify` 跳过校验 |
| provider 种类 | 文档示例用 `deepseek`，但实现只有 `mock/openai/local` → 照抄配置会 exit 3 | 新增一等 `deepseek`（默认 `https://api.deepseek.com/v1` + `deepseek-chat`）；`local` 仍免 key；key 支持 `PACKETSAGE_LLM_API_KEY` / `OPENAI_API_KEY` / `DEEPSEEK_API_KEY` |
| doctor 第 8 项 | 永远显示 `✅ mock: deterministic, no network, no key`（把"没配置"说成"已就绪"） | 未配置 → `⚠ not configured: run packetsage-agent setup`（不阻塞 exit 0）；`mock` 行改文案为"CI/演示用脚本回放，不是真实分析"；`deepseek` 有可达性探测 |
| 启动器/脚本 | `install_smoke` 与 harness 依赖隐式 mock | 需要成功路径的用例都**显式**声明 `PACKETSAGE_LLM_PROVIDER=mock`；另加两条断言：未配置 → exit 3 + setup 提示（harness `unset provider is setup, not a silent mock`、`install_smoke` 同名检查）、`setup` 写文件且不回显 key（harness `setup writes agent/.env and masks the key`） |

## 4b. 模拟真实用户实测（临时 DeepSeek key，install-test 里的安装）发现并修复的问题

实测路径：`setup`（写 `agent/.env` + 真调 `/models` 校验）→ `doctor` → `analyze --db` →
`packetsage-agent run --task-id`（真模型 `deepseek-flash`）→ `report`（新进程）→ `chat`（非 tty）
→ `analyze --full --report`。真模型单次 run：7–11 步、25–45k tokens、20–45s。

| # | 现象 | 根因 | 修复 |
|---|---|---|---|
| 1 | 配好 key 后 `setup` 仍说"需要 API key" | `setup` 只在 provider 未变时复用 `settings.api_key`，而 provider 是从未配置切过来的 | 只要有 key（env 或 `agent/.env`）就复用；`--api-key` 优先，来源标注为"命令行/已存在" |
| 2 | 把"key + 注释两行"一起贴进来，`agent/.env` 被写坏（多出一行裸文本） | 换行值直接写进 KEY=VALUE | `_write_env_file` 拒绝含换行的值（exit 1，提示只贴 key 本身） |
| 3 | `.env` 已配好，`packetsage chat` 仍 exit 3 报未配置 | Rust launcher 把**空的** `PACKETSAGE_LLM_PROVIDER=` 导出给子进程，而空值也算"已设置"，顶掉了 `.env` | Python 侧 `load_dotenv` 把空变量视为未设置；Rust 侧只在非空时才导出 provider/model |
| 4 | `packetsage doctor` 在 `.env` 配好后仍报"未配置" | Rust 侧不读 `.env`（§7.1 有意为之），doctor 只看 env/config | doctor 第 8 项改为**去问 Agent**（`packetsage-agent setup --print --no-verify`，5s 超时），拿到 `deepseek / deepseek-flash` 即 ✅，拿不到才 ⚠ 提示 setup |
| 5 | 真模型 findings 全是英文，与中文目标不符 | `PacketSageAgent.run(task_id, goal)` 的 goal 从未传给 provider，模型只看到系统提示词 | provider `decide(..., goal)` 增加 goal 参数并放进第一条 user 消息（唯一自由文本入口）；`packetsage-agent run/chat` 的目标/提问现在真的到模型 |
| 6 | `report`（新进程）Executive Summary 判为编造：`cites 30, 54 …` → exit 4，报告不落盘 | findings 只在产生它的引擎进程内存里；`submit_finding` 不落库、`report` 的分支只带 `numbers/ref_ids` 且从没写过 | `submit_finding` 写穿到库；冷恢复回填 findings**与台账**；`tool_calls` 新增 `numbers_json/ref_ids_json/tokens_json`（migration `0002`）；`tools.result` 记录"可引用 token"（IP、ip:port、id、数值；正文/预览不入库）；trace 暴露三者供 lint 使用 |
| 7 | 台账 basis 回读失败（`correlated_observation` 存成 `correlatedobservation`） | `persist.rs` 用 `format!("{:?}").to_lowercase()` 丢了下划线 | 读取侧归一化（兼容两种拼写），解析失败时 `tracing::warn!` 不再静默丢弃 |

**未改动**：报告 lint 的硬失败语义保持原样（见 D-18）——修的是"让合法事实跨进程可用"，
不是"放宽判据"；`test_executive_summary_hallucination_is_a_hard_failure` 仍绿。

**真模型观察（留给 #38 调优）**：同一份 synth-mixed，几次 run 分别得到 accepted=2 / accepted=1
+submit_rejects=2 / accepted=0（均为 `completed`）；引擎的 V1–V4 会拒绝模型编的 ref_id（现在
stderr 会打印拒绝原因）；输出语言仍偏英文（goal 已送达，但系统提示词与 JSON schema 都是英文）。

## 5. 手工验收实录（节选）

```console
$ packetsage analyze samples/synth-mixed.pcap --db sqlite://e2e.db
task            task_01M2YHAE9S4AH8X0DP4KD0XHBQ
...
database        sqlite://e2e.db (task stored)

$ packetsage-agent run --task-id task_01M2YHAE9S4AH8X0DP4KD0XHBQ --engine "target\debug\packetsage.exe serve" --db sqlite://e2e.db
run run_bc66ef04-… task=task_01M2YHAE9S4AH8X0DP4KD0XHBQ status=completed accepted=2 submit_rejects=0 malformed_output=0 steps 9/12 · calls 9/24 · 1.5k tok · 0.0¢/50¢
  high   rule_match               F-001 规则命中 NET-TCP-SYN-BURST-001
  medium rule_match               F-002 规则命中 NET-TCP-PORT-SWEEP-001

$ packetsage analyze samples/synth-mixed.pcap --full --report reports/e2e-report.md --db sqlite://e2e.db
database        sqlite://e2e.db (task stored)
run run_f485ce06-… status=completed accepted=2 submit_rejects=0 malformed_output=0 steps 9/12 · calls 9/24 · 1.5k tok · 0.0¢/50¢
report written: reports\e2e-report.md sha256=b550cb67… degraded=0
report          reports\e2e-report.md
```
