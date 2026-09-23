# YeLee' PacketSage 🛡️

> **Rust 负责把网络流量变成可靠的结构化证据，Agent 负责决定下一步该找什么证据，
> 报告负责把结论、证据和局限性一起交给人。**

一个面向课程项目与网络安全实验的 **LLM Agent 网络协议分析工具**：
PCAP/PCAPNG 的结构化分析全部由 Rust 引擎完成，Python Agent 只做“下一步查什么”的决策，
报告里的每个数字都能回溯到某次工具调用。

[![Core](https://img.shields.io/badge/core-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Agent](https://img.shields.io/badge/agent-Python-blue?logo=python)](https://www.python.org/)
[![Schema](https://img.shields.io/badge/schema-v2-green)](#事件与-rpc-契约)

---

## 0. 版本号约定

版本号是 **`大更新.小更新.bug`** 三段：

| 段 | 什么时候加 | 例 |
|---|---|---|
| 大更新 | 交付形态或契约变了（新命令面、新协议版本、架构改动） | `3.x.x` → `4.0.0` |
| 小更新 | 能力增加但契约不变（新界面、新规则、新工具） | `3.8.x` → `3.9.0` |
| bug | 只修问题（内测期最常出现） | `3.8.1` → `3.8.2` |

**当前版本 `3.8.1`**（内测期）；它是 git 之后的第一版，因此**从这一版起**按上面的规则递增。
版本号只有两个权威落点，改的时候一起改，其余地方（安装包名、tarball 名、`packetsage version`、
Agent 的 `--version`）都是从它们推导出来的：

| 落点 | 文件 |
|---|---|
| 引擎 + CLI + 桌面外壳 | `Cargo.toml` 的 `[workspace.package] version` 与 `desktop/src-tauri/{Cargo.toml,tauri.conf.json}` |
| Python Agent | `agent/pyproject.toml`（`_version.py` 读它，不重复存） |

## 1. 快速开始（三行，源码路径）

```bash
cargo install --path crates/packetsage-cli && python -m pip install -e ./agent
packetsage-agent setup                                   # ① 第一步：选 provider、存 key（写 agent/.env）
packetsage doctor                                        # ② 装对没有（10 项自检）
packetsage analyze samples/synth-mixed.pcap              # 引擎视角：摘要 + 告警
packetsage chat samples/synth-mixed.pcap                 # 交互式 Agent（需要真终端）

# 手工直调 Agent：task 先由 analyze（或 --full）创建，再引用它
packetsage analyze samples/synth-mixed.pcap --db sqlite://packetsage.db   # 输出里的 task_…
packetsage-agent run --task-id <task_…> --db sqlite://packetsage.db      # 批式调查，stdout 一行摘要
```

没有样例文件时先合成一份（不需要 scapy）：

```bash
python scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 --profile mixed
```

想先确认环境是否就绪，跑 `packetsage doctor`（十项检查，`--json` 给脚本用）；
想只看引擎而不装 Python 侧，用 tarball 路径（`analyze` / `rules` / `query` / `db` /
`doctor` / `version` / `completions` 可用，`chat` 需要补装 Agent）——
见 [INSTALL.md](INSTALL.md) §4「怎么运行」（三条命令面 + "引擎路径"与"task 从哪来"两个坑），
以及 [docs/ux-walkthrough.md](docs/ux-walkthrough.md) 里对"10 分钟判据"的逐步记录。

> `packetsage-agent` 手工直调时默认执行 `packetsage serve`：`packetsage` 不在 `PATH` 上就
> 用 `--engine "<全路径> serve"` 或先设 `$PACKETSAGE_ENGINE`；而 `packetsage chat` /
> `analyze --full` 由 launcher 自动交接引擎路径，不用配。
>
> **没配置 provider 就不会跑**：`run` / `chat` / `analyze --full` 以 exit 3 结束并提示
> `packetsage-agent setup`；`mock`（确定性脚本回放）只有显式指定才生效，所以不会出现
> "看着像在分析、其实是假结果"。API key 只写 `agent/.env`（gitignored），永不写进
> `packetsage.yaml`，也不进 git。

端到端（解析 → 规则 → Agent → 报告）等价写法：

```bash
packetsage analyze samples/synth-mixed.pcap --full --report reports/demo.md
```

**Windows / GNU 宿主**：直接 `cargo build` 会两条坑——`dlltool.exe` 找不到（rustup 的
gnu 工具链不带汇编器），以及 MinGW 装在含空格路径时 `ld` 报 `cannot find C:/Users/...`。
用仓库里的构建脚本即可，它会自动找 cargo、找 MinGW、并用 rustup 自带的链接器驱动：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build.ps1              # debug
powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release
powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Command test   # 也支持 test/clippy/run
```

也可以照常 `cargo build`，但需要自己把含 `dlltool` + `as` 的 MinGW `bin` 放进 `PATH`
（背景见 [docs/ux-walkthrough.md](docs/ux-walkthrough.md) §0.1 与
[docs/architecture.md](docs/architecture.md) 偏差表 D-12）。

**别双击 `packetsage.exe`**：它是控制台程序，不带子命令时会打印一份 help 就退出，
所以双击只会"闪一下窗口"（这不是崩溃）。要么在终端里带子命令跑，要么用双击可用的启动器：

```powershell
.\install-test\RUN-CLI.cmd                          # 双击等价：自动找二进制、跑 version + analyze、然后暂停
.\install-test\RUN-CLI.cmd doctor --no-net          # 带参数：跑你指定的命令，然后暂停
```

## 2. 项目结构

```text
crates/
├── packetsage-protocol/   L0 事件与 RPC schema（无 IO/时钟/随机数）
├── packetsage-core/       L1 reader / decoder / reassembly / conversation / query / pipeline
├── packetsage-rules/      L2 YAML DSL + S1–S9 校验 + 滑动窗口评估器
├── packetsage-storage/    L1' Repository + SQLite 实现 + migrations（sqlx 唯一入口）
├── packetsage-cli/        L3 唯一 binary `packetsage`
└── packetsage-fuzz/       稳定版 fuzz smoke harness
agent/packetsage_agent/    engine client / tools / policy / provider / agent / report / eval
gui/                       Streamlit 原型界面（参考实现；交付形态改为 Tauri，见《GUI 工程规格书 v0.2》）
desktop/                   Tauri 桌面应用：src/ 前端（React+TS+Vite+Tailwind）、src-tauri/ 外壳（Rust）
rules/builtin|examples/    四条内置规则（随二进制内嵌）+ 三条示例规则
migrations/                SQLite/PostgreSQL 双方言 migration
scripts/                   gen_traffic.py（合成流量）、bench.py（benchmark）
tests/integration/         demo_mainline.py（§38 十步演示主线）
fuzz/fuzz_targets/         cargo-fuzz 目标（nightly）
docs/                      architecture / error-codes / report-spec / benchmarks / agent-eval
```

## 3. 命令

| 命令 | 作用 |
|---|---|
| `packetsage version [--json]` | 版本行 / 稳定 JSON（`json 输出格式 v1`，键顺序冻结：`version, schema_version, git_hash, build_time, profile, rustc, target`） |
| `packetsage doctor [--json] [--no-net]` | 10 项自检（含修复提示；`--no-net` 只跳过第 8 项的连通性探测） |
| `packetsage analyze <cap>` | 人读摘要（格式/包数/字节/会话/Top conversations/告警） |
| `packetsage analyze <cap> --jsonl` | 完整 JSONL 事件流（schema v2，stdout 纯机器流） |
| `packetsage analyze <cap> --full --report out.md` | 规则 + Agent + 报告（Agent 失败降级为 WARN，退出码仍为 0） |
| `packetsage serve` | JSONL RPC worker（Agent 的唯一出口） |
| `packetsage query {sessions,stats,alerts,findings}` | 结构化查询数据库（永远不接受裸 SQL）；`--jsonl` 打机器行，`--json <PATH>` 写文件 |
| `packetsage db query --readonly --sql "SELECT ..."` | 只读 SQL：url 必须含 `mode=ro`、语句必须是 SELECT/WITH/EXPLAIN，`--limit`/`--timeout`/`--jsonl` |
| `packetsage db migrate [--yes]` | 打印 current→target 与 pending 后执行迁移（非交互无 `--yes` 时 fail-closed） |
| `packetsage rules list / check` | 规则清单 / 严格校验 |
| `packetsage chat <cap>` | 交互式会话：analyze + 落库后启动 Python Agent（四级解析，Ctrl-D 退出） |
| `packetsage schema` | 事件、RPC、退出码与限额的机器可读摘要 |
| `packetsage completions bash\|zsh\|fish\|powershell` | 补全脚本（幂等） |

全局旗标（所有子命令通用）：`--config <path>`、`-v`/`-vv`（info / debug + 脱敏配置 dump）、
`-q`（关进度与 info 日志，WARN/错误仍走 stderr）、`--no-color`（`$NO_COLOR` 非空同样生效）。

退出码见 [docs/error-codes.md](docs/error-codes.md)（0/1/2/3/4/5，
其中 exit 4 有“internal error”与“anti-hallucination 硬失败”双语义）；
逐命令场景化断言在 `tests/cli/exit_codes.py`：

```bash
python tests/cli/exit_codes.py --binary target/release/packetsage
```

### 3.1 三流分离与进度

| 流 | 内容 | 约束 |
|---|---|---|
| stdout | summary 表格、db query 表格、doctor ✅/❌/⏭ 表、version、completions | `--jsonl`（仅 `analyze` 与 `db query`）时是本子命令的纯机器流：逐行 JSON、无 ANSI |
| stderr | 日志、进度、人读错误、WARN、修复提示 | 不会出现 EngineEvent 行 |
| 退出码 | §4 矩阵 | panic 兜底为 4 |

进度形如 `analyze: phase=parse packets=120000 bytes=84.3MiB speed=41.2k pkt/s elapsed=2.9s`，
终止行 `done: N packets in T s`；tty 每 200ms（或每 10 万包）刷新，非 tty 每 5s 一行。
进度只读引擎的原子计数器，因此 `-q` 开关前后 `--jsonl` 输出逐字节一致（`tests/cli` 的 T7 用例）。

### 3.2 配置发现链与脱敏

文件发现链：`--config` → `$PACKETSAGE_CONFIG` → `./packetsage.yaml` → 内置默认值；
键值优先级链：CLI 旗标 → 环境变量 → 配置文件 → 默认值。
显式层与"存在但不可解析"的层 3 都是 exit 3；未知键按字段路径报错；文件里出现
`api_key` 一类凭据直接拒绝（请用 `$PACKETSAGE_LLM_API_KEY` 或 `agent/.env`）。

脱敏按键名**分段**匹配（`api_key`、`*_secret`、`password`、单独的 `token`/`tokens`），
不做子串匹配，所以 `max_tokens_total` 这类合法键不会被误伤；`-vv` 的配置 dump
与 doctor 的生效配置展示都走同一套规则。

### 3.3 shell 补全

```bash
# bash
packetsage completions bash > ~/.local/share/bash-completion/completions/packetsage
# zsh
packetsage completions zsh > ~/.zfunc/_packetsage        # 且 fpath 需包含 ~/.zfunc
```

`fish` 与 `powershell` 同样支持；同一参数两次输出逐字节一致（测试断言）。

## 4. 事件与 RPC 契约

事件流（stdout，schema v2，ADR-013）：

```json
{"event":"packet","schema_version":2,"task_id":"task_01J...","packet_index":1821,
 "ts_unix_ns":"1717689600123456789","ts_precision":"us","interface_id":0,
 "captured_len":74,"original_len":74,"linktype":1,"truncated":false,
 "network":{"src":"192.168.1.105","dst":"192.168.1.20","protocol":"tcp",
            "ip_version":4,"ttl":64,"is_fragment":false},
 "transport":{"src_port":51344,"dst_port":80,"flags":["SYN"],"seq":1,"ack":0,"window":8192},
 "payload_ref":{"packet_index":1821,"offset":54,"length":438},
 "decode_status":"ok"}
```

工具结果信封（冻结契约）：

```json
{"_id":"tc_01M2Y6FHRMSHMT35922FPKCC8","source":"engine","trusted_as_instruction":false,
 "redactions":[],"method":"get_capture_summary","content":{...}}
```

`_id` 由 Rust 在产生工具结果时分配，是**唯一**合法的证据锚点；
envelope `_id` = 台账主键 = `EvidenceRef._id` = 报告 Evidence 索引（四段对拍）。
结果体里还会带 `_anchor`（同一 tc id）、`_ref_ids`（该结果可达的会话/告警/规则/包号）
与 `_numbers`（该结果的数值字段），让模型可以逐字复制而不是猜（ADR-026）。

## 5. 规则 DSL

```yaml
id: NET-TCP-SYN-BURST-001
version: 1
name: TCP SYN 突发检测
severity: high
scope: packet
match:
  protocol: tcp
  flags: { contains: [SYN], excludes: [ACK] }
  # src_port / dst_port 用 `any_port: 53`（别名 either_port）表示"任一侧为该端口"，
  # 用于请求/响应端口在不同侧的协议（如 DNS 响应是 src_port 53）
threshold:
  metric: count            # count | distinct_count | sum_bytes | ratio
  group_by: [src_ip]
  window: 10s              # ∈ {1s,5s,10s,30s,1m,5m}
  operator: gt
  value: 100
dedupe:
  per_group_cooldown: 1m   # 与 window 同一白名单
```

内置四条：`NET-TCP-SYN-BURST-001`、`NET-TCP-PORT-SWEEP-001`、
`NET-DNS-SUSPICIOUS-001`、`NET-MALFORMED-BURST-001`（随二进制内嵌，
`rules/` 下同名文件可覆盖）。示例规则见 `rules/examples/`（ratio、distinct_count、session scope）。

规则不做任意代码：只有四个聚合原语 + 白名单字段；S1–S9 静态校验在加载期拒绝/隔离非法规则。

## 6. Agent 与报告

```text
get_capture_summary → get_protocol_stats / get_conversations / check_alerts
                    → filter_packets → inspect_packets → reconstruct_stream
                    → query_history → get_task_artifacts
```

Agent 自己的命令面（权威定义见《Agent CLI 工程规格书 v0.1》；`--task-id` 指向一个
**已存在**的 task，Agent 进程本身不连库，ADR-018）：

```text
packetsage-agent run    --task-id <id> [--engine <cmd>] [--db <url>]            批式调查，stdout 一行终态摘要
packetsage-agent chat   --task-id <id> [--engine <cmd>] [--db <url>] [--report [<path>]]
packetsage-agent report --task-id <id> [--engine <cmd>] [--db <url>] [--report <path>]
packetsage-agent --version | --help
```

* 引擎由 Agent 侧 spawn（默认 `packetsage serve`，可用 `--engine` / `$PACKETSAGE_ENGINE` 指定）；
  `--db` 只拼给引擎，跨进程的任务由 `serve` 依 ADR-019 从库里冷恢复；
* 全局旗标：`--config`、`-v`/`-vv`、`-q`（进度与 usage 行只走 stderr，且仅 tty）；
* 退出码：argparse/非 tty = 1、引擎查无 task / 引擎起不来 / LLM/配置 = 3、引擎异常死亡按
  `{2,3,4}` 透传、lint 硬失败与未捕获异常 = 4；降级收尾仍是 0 + WARN；
* 评测 runner 是模块入口，不是子命令：`python -m packetsage_agent.eval.run_eval --capture …`。

* 九工具全部 Pydantic/内置 schema 校验，工具结果上限 8000 字符（超限走结构化摘要）；
* 硬预算：`max_steps=12`、`max_llm_calls=24`、`max_tool_calls=20`、连续同质调用 5 次即停；
* 证据链：`validate_finding`（Rust V1–V4）→ Python V5 → `submit_finding`（Rust 分配 `F-{n:03}`）；
* 系统提示词 **v2**（《Agent 系统提示词规格 v0.1》）：R1–R6 红线 + 工具策略 + 调查纪律 +
  run/chat 两个输出块；v1 保留为回滚基线，`agent.prompt_version` 可选，逐 run 落库。
  `packetsage-agent --help` 与 REPL 启动时显示项目 ASCII 标题；
* 报告九节（Executive Summary…Analysis Trace），反幻觉 lint 两级处置：
  Executive Summary 编数 → exit 4 且不落盘；正文编数 → 替换为 `[unverified by engine]` 并标注降级。

## 7. 测试与验证

```bash
cargo test --workspace                      # Rust：175 项
cd agent && python -m pytest -q             # Python：175 项（含 Agent CLI 契约、setup、通道 B 收口、token 级流式、思考回传、缓存命中与模型总结）
python tests/cli/exit_codes.py --binary target/debug/packetsage   # CLI：43 项（§4 矩阵 + 三流 + Agent 端到端 + setup）
python tests/cli/gui_contract.py --binary target/debug/packetsage # GUI 契约：7 项（S40–S45，机器出口快照）
python tests/gui/app_cases.py --binary target/release/packetsage  # GUI 界面：8 项（S57–S65 / M4–M12，AppTest 真跑界面）
python tests/sidecar/protocol_cases.py --binary target/release/packetsage  # Agent sidecar 协议：8 项（S57–S70，通道 B）
cd desktop/src-tauri && cargo test                               # 桌面外壳：21 项（sidecar 层 + P1 快速失败 + 打包态连接 P11 + U7 校验与握手 + 流式帧优先丢弃）
python -m ruff check gui --config gui/ruff.toml                  # GUI：同一组 ruff 规则族
cargo run --release -p packetsage-fuzz --bin fuzz-smoke -- --seconds 60
python tests/integration/demo_mainline.py --binary target/release/packetsage
PYTHONPATH=agent python -m packetsage_agent.eval.run_eval --capture samples/synth-mixed.pcap
python scripts/bench.py --binary target/release/packetsage --captures samples/synth-10m.pcapng --full

# release tarball（§12：binary + rules/builtin/ + INSTALL.md + SHA256SUMS）
python scripts/package_release.py --binary target/release/packetsage

# 本地 install 测试目录（两条安装路径 + 能力矩阵，产出 install-test/RESULTS.md）
python scripts/install_smoke.py --run-cli   # 装完顺便把 CLI 跑一遍，转录见 install-test/RUN.md
```

CI（`.github/workflows/ci.yml`）跑：fmt/clippy、cargo test、Python 侧 ruff + mypy + `uv lock --check`、
Python 测试 + demo 主线 + **GUI 契约 S40–S45** + **Agent sidecar 协议 S57–S70** +
**GUI 界面用例 S57–S65** + E1–E6、
**CLI 退出码矩阵**、MSRV(1.80) check、fuzz smoke 60s、install-from-scratch，
以及 tag 触发的 release tarball。

测试分级里有两条硬约定：**流级**断言（per-session per-direction 的 bytes 与
missing_ranges，见 `crates/packetsage-core/tests/flow_directions.rs`）与
**零长度段语义**（纯 ACK 不占序号空间、SYN 占一个序号）——它们是第一次端到端
评审暴露出的两类盲区。

## 8. 图形界面

> **状态（2026-09-20）**：`gui/` 是 **Streamlit 原型**——可用、已验收（`tests/gui/app_cases.py` 在 CI 里真跑），
> 但**交付形态将改为 Tauri 桌面应用**（可双击安装，目标机无需 Python / Rust / Node），
> 见《[GUI 工程规格书 v0.2](docs/specs/GUI%20工程规格书%20v0.2.md)》。原型冻结为参考实现：**只修 bug，不加功能**。
> 下面 §8.1 / §8.2 描述的是这条原型路径怎么跑。

> **交付形态（桌面应用）见 [desktop/README.md](desktop/README.md)**：
> `powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1` 一条命令产出外壳
> 与 NSIS 安装包（`YeLee’ PacketSage_3.8.1_x64-setup.exe`）；两个 sidecar（引擎 + PyInstaller 打的 Agent）随包分发，
> 目标机不需要 Python / Rust / Node。

命令行不是唯一入口：`gui/` 把同一套后端契约做成对话式界面，**证据链、规则告警、
预算常显、降级可见**是四个一等公民（《GUI 工程规格书 v0.2》§5.2），
取数一律走 Python 模块 API（收口文档 §5.12），不解析 CLI 的人读表格。

### 8.1 启动（三步）

```powershell
python -m pip install -e "agent[gui]"     # ① Streamlit 是 agent 的可选组
packetsage-agent setup                    # ② 选 provider、存 key（写 agent/.env；没配就不让跑）
.\scripts\run_gui.cmd                     # ③ 双击等价；浏览器开 http://localhost:8501
```

前置：仓库里有一份引擎（`target/release|debug/packetsage.exe`，或 `packetsage` 在
`PATH`，或 `$PACKETSAGE_ENGINE`）、`samples/` 里有抓包（没有就
`python scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 --profile mixed`）。
`run_gui.cmd` 会自己找这三样，并在缺件时把该跑的命令打印出来。

直接跑也可以（等价）：

```powershell
python -m streamlit run gui/app.py --server.port 8501
```

### 8.2 界面怎么用

| 步骤 | 操作 | 界面会做什么 |
|---|---|---|
| 1 | 左栏选/上传抓包 → 「▶ 开始分析」 | `analyze_file`：落库 + 留在引擎内存，拿回 `task_id`，显示 612 包 / 570 会话 |
| 2 | 「▶ 开始调查（run）」或底部输入框追问 | 后台线程跑 agent；**工具调用卡片一条条出现**，预算条同步增长 |
| 3 | 随时「⏹ 停止调查」 | `request_finalize()`：下一个边界收尾，已提交的结论保留，标注 `degraded` + `stop_reason` |
| 4 | 看「证据链」 | 结论 → `tc_*` 锚点 → 工具名/参数/耗时/状态，四段对拍且可点开 |
| 5 | 看「规则告警」 | 规则 id / 版本 / `rule_content_hash` / 窗口证据 / 样例包（窗口被裁剪时显式标黄） |
| 6 | 「生成报告」 | 九节 Markdown 预览 + 一键下载；反幻觉硬失败按 exit 4 的语义原样报错 |

启动即做 `packetsage doctor --json --no-net`：**provider 没配好就不给分析**（只显示
`packetsage-agent setup`），不会静默改用 mock。引擎崩了会显示 `stderr_tail(3)` 并
一键重建，不白屏。

### 8.3 界面侧的环境变量

| 变量 | 作用 |
|---|---|
| `PACKETSAGE_ENGINE` | 引擎可执行文件（`run_gui.cmd` 会自动设） |
| `PACKETSAGE_GUI_DB` | 换一个存储 URL（默认 `sqlite://<repo>/packetsage.db`） |
| `PACKETSAGE_LLM_*` / `PACKETSAGE_ENV_FILE` | 与 CLI 同一套发现链（界面不另立一套） |

## 9. 当前状态与已知限制

已经端到端跑通并验证的：

* PCAP/PCAPNG（多 section/多接口/VLAN/QinQ、二进制 tsresol）解析；
* Ethernet/SLL/VLAN/ARP/IPv4/IPv6/TCP/UDP/ICMP/ICMPv6 + DNS/HTTP/TLS/DHCP 元数据；
* 规范化五元组会话聚合（双向收敛为一条）、TCP 重组（重传/乱序/缺口/超限/超时）；
* 四条内置规则在合成样本上真实触发；JSONL 事件流、RPC 九工具、SQLite 落库与结构化查询；
* Agent 工具循环（mock provider 确定性）+ 四级证据校验 + 九节报告 + 反幻觉 lint；
* Streamlit 界面：分析 → 调查 → 证据链 → 报告全链路（含停止按钮、引擎崩溃重建、
  provider 门禁），由 `tests/gui/app_cases.py` 在 CI 里真跑界面断言；
* 桌面应用第一批：**Agent sidecar 协议（通道 B）**已实现并冻结——`packetsage-agent serve`
 提供 `hello/run/chat/report/cancel/status/shutdown` 与 10 类事件，`run_finished` 与
  `run --json` 同构，`seq` 单调、事件 ≤8 KiB、协议清单快照进 CI（`tests/sidecar/`）；
* 桌面应用第二批：**外壳可构建、可安装**——`desktop/`（React+TS 前端 + Rust 外壳，MSVC
  工具链）产出 `YeLee’ PacketSage_3.8.1_x64-setup.exe`；外壳的 sidecar 层有 4 条
  不依赖窗口的验收用例；Agent sidecar 由 PyInstaller `--onedir` 打成 67 MB 独立可执行；
* fuzz smoke（10 万级随机输入零 panic）、benchmark、doctor、demo 主线。

仍需在其它环境完成（本机条件不具备，已在 [docs/architecture.md](docs/architecture.md) 登记）：

* PostgreSQL matrix（`--features postgres`，需 Linux/CI）；MSRV 1.80 实测；1 GB 档 benchmark；
* 真实 LLM provider（`openai` / `local`）的 E1–E6 pinned 模型评测；
* golden fixtures 的逐字节固化（当前以单测 + demo 主线覆盖）；
* 真实样本授权说明（`wireshark/` 目录仅用于本地验证）。

## 10. 文档索引

| 文档 | 内容 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 分层、契约映射、全部实现偏差（D-5…D-13） |
| [docs/error-codes.md](docs/error-codes.md) | 退出码、RPC 错误码、事件与解码枚举 |
| [docs/report-spec.md](docs/report-spec.md) | 报告九节数据来源、反幻觉 lint、降级矩阵 |
| [docs/benchmarks.md](docs/benchmarks.md) | benchmark 方法、验收目标、ratchet 阈值 |
| [docs/ux-walkthrough.md](docs/ux-walkthrough.md) | “10 分钟判据”逐步记录（install → analyze → 读懂 summary） |
| [INSTALL.md](INSTALL.md) | 双路径能力矩阵与 tarball 安装 |
| [CLI 收口同步点 S1-S4.md](docs/specs/CLI%20收口同步点%20S1-S4.md) | 本批次对《开发文档》/《M3～M6》的文档同步点 |
| [GUI 前收口文档 v0.1.md](docs/specs/GUI%20前收口文档%20v0.1.md) | 后端侧收口判据（G1–G7）、模拟测试剧本（S01–S56）、GUI 接口冻结清单 |
| [GUI 工程规格书 v0.2.md](docs/specs/GUI%20工程规格书%20v0.2.md) | 桌面应用：Tauri 架构、进程与数据交换协议、集成项 U1–U10、打包分发、人工验收 M1–M24 |
| [项目报告 v1.md](docs/specs/项目报告%20v1.md) | **最新状态**（2026-09-23）：完成度自评、本轮实测、缺陷 F1–F5、决策 D1–D9、收尾路线 |
| [项目报告 v0.1.md](docs/specs/项目报告%20v0.1.md) | 前身（2026-09-21）：交付物、连接层 11 条问题、HTTP/MCP/in-process 评估 |
| [docs/agent-eval/](docs/agent-eval/) | E1–E6 评测结果 |
| [M0～M2 Rust 工程规格书.md](docs/specs/M0～M2%20Rust%20工程规格书.md) | 上游规格 |
| [M3～M6 工程规格书 v0.2.md](docs/specs/M3～M6%20工程规格书%20v0.2.md) | 上游规格 |
| [开发文档 v0.3.md](docs/specs/开发文档%20v0.3.md) | 总开发文档 |

## 11. 安全边界

PacketSage 是**分析工具**，不是入侵工具：不抓包、不发包、不执行抓包中的命令、
不改防火墙、不封 IP。抓包内容一律视为不可信数据（`trusted_as_instruction=false`），
工具结果默认脱敏 `Authorization` / `Cookie` 等字段，payload 只有显式调用
`reconstruct_stream` 才返回且有界（≤256 KiB）。

## License

MIT
