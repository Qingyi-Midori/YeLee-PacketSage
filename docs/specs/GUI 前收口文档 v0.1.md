# GUI 前收口文档 v0.2

> 定位：GUI 开工前的**收口判据（§3）+ 模拟测试剧本（§4）+ 接口冻结清单（§5）**。
> 本文件本身只写文档，不改代码、不动 ADR；**执行记录见 §3 的行尾证据与 §4.7**。
> 2026-09-20 已执行 B1–B3（§7 顺序表），P0 项除 G5-2 外全部关闭。
> **v0.2 修订**：GUI 的设计与实现已拆出为《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》。
> 本文档只保留"后端必须冻结什么"，不再包含界面设计、桌面应用集成项与界面验收。
> 文件名保持 `v0.1` 以维持既有 13 处交叉引用（五份规格书 + `README_v0.2.md` + `docs/adr/README.md`）；**版本以本行与 §10 为准**。

---

## 0. 文档元信息

| 项 | 内容 |
|---|---|
| 版本 | v0.2（文件名仍为 `GUI 前收口文档 v0.1.md`，见文件头说明） |
| 日期 | 2026-09-20 |
| 状态 | **B1–B3 已执行完毕**：G1-1…G1-5、G2-1…G2-4、G5-1…G5-4、G6-1…G6-4 全部关闭（G5-2 的 280MB 产物已清理） |
| GUI 范围 | **本文档不含 GUI 的设计与实现**。技术栈（Tauri + Rust/Python 双 sidecar）、进程与数据交换协议、集成项 U1–U10、打包分发、界面验收（S57–S70、M1–M24）均在《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》；分工见本文 §1.4 |
| 基线 | 工作区尚未 `git init`；基线 = 2026-09-20 本机构建（`target/release/packetsage.exe`，built=2026-09-20T06:24:08Z）+ 本次 B1–B3 改动后的复测 |
| 本机环境 | Windows 11 26200 / rustc 1.98.1 `x86_64-pc-windows-gnu` / Python 3.14.3 |
| 上游文档 | 《开发文档 v0.3》《M0～M2 Rust 工程规格书》《M3～M6 工程规格书 v0.2》《CLI 收口工程规格书 v0.2》《Agent CLI 工程规格书 v0.1》《Agent 系统提示词规格 v0.1》，以及同步点 [S1–S4](../archive/CLI%20收口同步点%20S1-S4.md) / [S5–S6](Agent%20提示词同步点%20S5-S6.md) / [S7–S8](../archive/Agent%20CLI%20同步点%20S7-S8.md) |

---

## 1. 目的与范围

### 1.1 为什么需要这份文档

GUI 只从三处取数：**CLI 的机器输出**、**stdout 的 JSONL 事件流 / RPC**、**数据库**。

当前后端功能已经跑通（见 §2.1），但这三处存在若干"**只有人读输出、没有机器输出**"的缝：

* `packetsage query {sessions,stats,alerts,findings}` 输出的是人读表格，**没有 `--jsonl`**；
* `packetsage-agent run` stdout 只有一行摘要，`report` 产物是 Markdown，**没有结构化出口**；
* `packetsage schema` 没暴露 `agent_max_llm_calls`；
* `version --json` 的真实键名与 README 写的不一致。

如果带着这些缝开工，GUI 会被迫去正则解析人类表格，后端一改输出就全碎。**这就是"GUI 前收口"的全部意义**：把缝补上，把契约钉死。

### 1.2 范围

本文件负责三件事：

1. **收口判据**（§3）：可逐条勾选、带阻塞级别的 DoD；
2. **模拟测试剧本**（§4）：可直接执行的验收用例，分"已实测 / 模拟桩 / 外部预期"三层；
3. **接口冻结**（§5）：GUI 可以依赖、后端不得单方面更改的契约。

不在范围内：**GUI 的设计与实现**（技术栈落地、界面结构、桌面应用打包与交互验收——已拆出为《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》）；后端架构调整（ADR 不动）；新分析能力。

两份文档的分工见 §1.4。

### 1.3 执行纪律

* 按 §3 的 **P0 → P1 → P2** 顺序做；每关一条就地勾选 `[x]`，并在行尾追加证据（命令 + 关键输出）。
* §4.2 的 8 条是**回归基线**，每个批次开工前先跑一遍，绿了再动别的。
* §4.6 的 L3 条目**没有跑过就不要勾**，只能写"待外部环境"。
* §5 的契约**只能新增字段**，不得改名、不得改语义；确需变更走 ADR。
* 不要用"文档写了就算完成"来关闭条目——每条都要有可复现的命令输出。

### 1.4 与 GUI 文档的分工（v0.2 修订）

"GUI 前收口"与"GUI 本身"是两件事：前者是后端工作，验收方式是命令与退出码；后者是产品与实现工作，
验收方式是界面行为。写在一起会让收口条目被界面细节淹掉，也会让界面决策被后端判据绑住，因此拆开。

| 问题 | 归属 | 验收方式 |
|---|---|---|
| 后端给 GUI 什么契约？冻结了没有？ | **本文档** §3（G1–G7）、§5 | 命令 + 退出码 + 契约快照测试（§4.2–§4.4） |
| 界面做成什么样？桌面应用怎么接？用户怎么用？ | 《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§2–§8 | 该文 U1–U10、S57–S70、人工验收 M1–M24 |

唯一的交叉点是 **§5.12 的 Python 侧集成面**：它属于本文档（"后端冻结了什么"），
而"界面怎么用它"属于 GUI 文档（U1）。

技术栈选 **Tauri** 对本文档有一个有效后果：GUI 不再与 Agent 同进程，两者之间必须走**进程间协议**
（GUI 文档 §4，本版新增），而不是 import Python 模块。因此 §5.12 的 Python 模块面从"GUI 的直接取数路径"
变成"**Agent sidecar 的内部实现面**"——GUI 的取数与操作一律走 §5.4 的 RPC 加上新的 sidecar 协议；
§5.10 / §5.11 的 CLI 机器出口继续作为脚本、CI 与排障用的补偿（已有 S40–S45 覆盖）。

---

## 2. 现状基线

### 2.1 已实测通过（2026-09-20，本次收口评估实测）

| 层 | 命令 | 实测结果 |
|---|---|---|
| Rust 测试 | `powershell -File scripts/build.ps1 -Command test` | **175 passed / 0 failed**（exit 0；新增 G1 契约单测 6 条） |
| Rust lint | `powershell -File scripts/build.ps1 -Command clippy` | **0 warning** |
| Rust 格式 | `powershell -File scripts/build.ps1 -Command fmt` | exit 0 |
| Python 测试 | `cd agent; python -m pytest -q` | **148 passed**（25.3s） |
| Python lint/类型 | `cd agent; python -m ruff check .` / `python -m mypy` | **All checks passed / 0 error**（G2-1、G2-2） |
| Python 依赖锁 | `cd agent; python -m uv lock --check` | 锁定 64 包（含 `langchain` 可选组，G2-3） |
| CLI 契约 | `python tests/cli/exit_codes.py --binary target/debug/packetsage.exe` | **43/43 passed** |
| **GUI 契约** | `python tests/cli/gui_contract.py --binary target/release/packetsage.exe` | **7/7 passed**（S40–S45，新增） |
| 十步主线 | `python tests/integration/demo_mainline.py --binary target/release/packetsage.exe` | 全过，报告 `sha256=23b0b2aaedbb…`（动态 run id 使其每次不同） |
| 自检 | `packetsage doctor --json --no-net` | 10 项全 `ok`（id 见 §5.8） |
| 契约快照 | `packetsage version --json` / `packetsage schema` | 见 §5.1、§5.7 |

样本侧事实（供对拍用）：`samples/synth-mixed.pcap` = 612 包 / 33724 B / 570 会话；规则命中 `NET-TCP-SYN-BURST-001`(high) 与 `NET-TCP-PORT-SWEEP-001`(medium)。

### 2.2 三个必须先知道的坑

1. **不要直接 `cargo test`**。Windows GNU host 上会报
   `error calling dlltool 'dlltool.exe': program not found`（`getrandom` 编译失败）。
   必须走 `scripts/build.ps1`，它会自己找 cargo、MinGW 与 rustup 自带的链接器驱动。
   这条要写进 GUI 的构建说明，否则新对话必然重踩。
2. **本机没有 `cargo` 在 PATH 上**。默认 shell 里 `cargo` 不可直接调用，实际路径是
   `%USERPROFILE%\.cargo\bin\cargo.exe`。
3. **工作区不是 git 仓库**。当前目录没有 `.git`，`git status` 直接报
   `not a git repository`。远程 `Qingyi-Midori/YeLee-PacketSage` 只有一个 `Initial commit`
   （仅含 `.gitignore` + `LICENSE`），本地内容一次都没推过。按仓库根 `AGENTS.md` 的约定，
   **项目完成前任何文件都不得上传**。

### 2.3 文档与实测的漂移（需回填）

| # | 文档声称 | 实测 | 处置 |
|---|---|---|---|
| R1 | `README.md` §7：Rust 168 / Python 143 | 175 / 148 | ✅ 已回填（`README.md` §7，含 GUI 契约 7 项） |
| R2 | `Agent 提示词同步点 S5-S6.md` §6：Python 129 + Rust 166 + CLI 39 | 148 / 175 / 43（+GUI 7） | ✅ 已回填 |
| R3 | `README.md` §3：`version --json` 含 `git`、`built` | 真实键名是 `git_hash`、`build_time` | ✅ 定稿：以实现为准（`git_hash`/`build_time`），README 已改；顺序由 `crates/packetsage-cli/src/version.rs` 单测 + S43 锁定 |
| R4 | `docs/benchmarks.md`：ratchet +10% 告警 / +20% 阻断生效 | history 内 `ratchet: {"status":"no-baseline"}`，且 `/docs/benchmarks/history/*.json` 被 `.gitignore` 排除 | 修采集 + 基线入库 |
| R5 | `M3～M6` §6.2：四档 benchmark、1GB RSS ≤1GB | 只测了 1MB / 10MB；`rss_peak_mib` **恒为 0.0** | 修 RSS 采集，补 100MB / 1GB |
| R6 | 三份规格书 DoD 复选框 | **95 条未勾选**（M3~M6 35 / README_v0.2 26 / M0~M2 13 / CLI 收口 12 / Agent CLI 5 / 提示词 4） | ✅ 统一标注：五份规格书页首 + `README_v0.2.md` 页首均声明"验收由本文档 §3/§4 接管"（G6-3） |
| R7 | `README_v0.2.md` 全篇 | 复选框全空，且与现状相反（"PCAP 读取"等已实现项也未勾） | ✅ 页首已加"已过期（归档）"标注，指向 README.md 与本文档 §3 |
| R8 | `analyze --full` 有 `--provider/--model`，但 `packetsage-agent run/chat` 已移除同名字段 | 两侧命令面不一致 | 在 §5.1 明确"哪一侧用什么"，避免 GUI 误用 |

### 2.4 规格中"明确未实现 / 主动降级"的项（不是漏做）

| 项 | 状态 | 出处 |
|---|---|---|
| V5（Python 侧自由文本数字扫描） | **未实现**，反幻觉只靠 Rust V1–V4 + 报告 lint | `docs/architecture.md` D-18 |
| ADR-019 store-backed 冷恢复 | 改为"按 `source_path` 重放 pipeline"的等价路径 | D-16 |
| provider 轮级干净取消 / `agent_runs.status=interrupted` | 判定不实现，改"跑完本轮 + WARN" | S1–S4 #28 |
| 规则窗口预算超限语义 | 保留最新一半事件 + `degraded=true`，非计数摘要 | D-10 |
| Agent CLI 命令面 | 移除 `--capture/--provider/--model/--scenario` 与 `eval` 子命令 | D-15 |

这五条要在评审时**逐条确认**，确认后写进《开发文档》的偏差表；在此之前它们是"待确认的偏离"，不是"已完成"。

---

## 3. 收口判据（DoD）

阻塞级别：**P0** = GUI 开工前必须关；**P1** = 与 GUI 并行可接受；**P2** = 记录在案即可。

### G1 · GUI 取数出口（全部 P0）

- [x] **G1-1** `packetsage query {sessions,stats,alerts,findings}` 增加 `--jsonl`（并支持 `--json <PATH>`→文件）。字段表落在**新增 §5.10**（原 §5.2 实为退出码表，编号沿用错误已在此纠正），快照由 S40/S41 锁定。
      证据：`crates/packetsage-cli/src/commands/query.rs`、`src/cli.rs`；`python tests/cli/gui_contract.py --binary target/release/packetsage.exe` → **7/7**（S40/S41 + `--json` 文件模式）。
      字段语义修正：DB 里的 `rulematch`/`halfclosed` 在机器行中归一为线协议词汇 `rule_match`/`half_closed`，原始值保留在 `basis_stored`/`state_stored`。
- [x] **G1-2** `analyze --jsonl` 的 9 类事件字段确认足够：S45 断言 `stats` 含 `protocol_stats/decode_errors/truncated_packets/incomplete_sessions/dropped_sessions/rule_window_evictions/submit_rejects/top_conversations`，`alert` 含 `severity/first_packet/last_packet/evidence{metric,value,window_ns,operator,threshold,sample_packets}`；无需补字段。
- [x] **G1-3** `packetsage-agent run --json` 落地（冻结定义见新增 §5.11）：stdout 恰好一行 JSON，键含 `summary_version/run_id/task_id/status/stop_reason/accepted/submit_rejects/malformed_output/steps/calls/tool_calls/tokens{in,out,total}/cache{hit,miss}/cost_cents/prompt_version/model/provider/summary/findings[]`（`cache` 与 `summary` 为 2026-09-22 v0.4 追加，键只增不改，S42 同步），finding 元素为 `id/severity/basis/title/evidence_ids`。
      证据：`agent/packetsage_agent/cli.py`；S42 实测（mock provider）→ `status=completed accepted=2`，findings 带 `F-001/F-002` 与 tc 证据 id。
- [x] **G1-4** `packetsage schema` 增加 `limits.agent_max_llm_calls=24`，并把 digest 抽成可单测函数。
      证据：`crates/packetsage-cli/src/commands/schema.rs`；S44 断言五项限额整体相等。
- [x] **G1-5** 定稿：**以实现为准**——`version --json` 键顺序冻结为 `version, schema_version, git_hash, build_time, profile, rustc, target`，不加 `git`/`built` 别名；README 已按实现修正。
      证据：`crates/packetsage-cli/src/version.rs` 单测 `json_key_order_is_frozen`；S43 连跑两次逐字节一致且键序相等。

### G2 · 质量门（全部 P0）

- [x] **G2-1** `agent/pyproject.toml` 增加 `[tool.ruff]`（`select = E4/E7/E9/F/W/I/UP/B/C4`，`line-length=100`，`src=packetsage_agent,tests`）；自动修复 14 处，手工修 2 处 `C416`；`python -m ruff check .` → **All checks passed!**
- [x] **G2-2** `[tool.mypy]`（`python_version=3.10`、`files=["packetsage_agent"]`、`ignore_missing_imports=true`）；修掉 9 处真实类型问题（`readline` 桩缺口、`Mode` Literal、`last_finding_error` 重复注解、`argparse.error` 返回 Never、`rules` 变量 str/int 混用等）→ **Success: no issues found in 17 source files**。
- [x] **G2-3** `agent/uv.lock` 生成并入库：64 包，含 `langchain`/`langchain-openai` 可选组与 `dev` 组（ruff/mypy/uv/pytest）。验证命令 `cd agent; python -m uv lock --check`。
- [x] **G2-4** `.github/workflows/ci.yml` 的 `python` job 已加三步：`ruff check .`、`mypy`、`uv lock --check`，并新增 `GUI contract S40-S45` 步骤（依赖已生成的 `samples/synth-mixed.pcap`）。

### G3 · 性能与证据（P1）

> 本轮（B1–B3）未执行：按 §7 顺序表，G3 是 B4，需要先有 G2 的 CI 改动。以下四项保持未勾选，勾选前必须补实测命令输出。

- [ ] **G3-1** 修 `scripts/bench.py` 的 RSS 采集（Windows 用 `GetProcessMemoryInfo` 或 `psutil`），`rss_peak_mib` 不得为 0。
- [ ] **G3-2** 生成 `docs/benchmarks/history/baseline-<hw>.json`，并解除 `/docs/benchmarks/history/*.json` 的 `.gitignore` 排除（否则基线永远进不了库）。
- [ ] **G3-3** CI 增加 ratchet job，并做一次"人为 +20% 触发阻断"的演练留档。
- [ ] **G3-4** 补 100MB 档（≤10s）与 1GB 档（≤60s、RSS ≤1GB）实测；不可得时按规格写书面说明。

### G4 · 评测（P1）

> 本轮（B1–B3）未执行：按 §7 顺序表，G4 是 B5，需要真实 key 与预算，且 L2 纪律禁止真调外部 LLM。以下四项保持未勾选。

- [ ] **G4-1** 定位并处理 E2 的真实模型失败（`docs/agent-eval/2026-09-20-openai.md`：`status=failed`，`path_match=0.50`）——是期望过严还是提示词没引导到 `reconstruct_stream`，二选一并留档。
- [ ] **G4-2** 真实模型 E1–E6 重新归档到 `docs/agent-eval/`，标注 pinned 模型、日期、token、成本。
- [ ] **G4-3** 定位 E3/E6 `findings=0`（模型没提交，还是被 V1–V4 拒了）。stderr 已打印拒绝原因，把原因抄进报告 notes。
- [ ] **G4-4** 决定 V5 做还是正式降级；若降级，在《开发文档》里把它从"待实现"移到"明确不做"，并说明风险（模型自由文本数字无 Python 侧防线）。

### G5 · 发布与合规（G5-1/2/3 P0，G5-4 P1）

- [x] **G5-1** `wireshark/` 已进 `.gitignore`（`/wireshark/`），并新增 `docs/third-party-captures.md` 逐文件登记来源与授权状态（实测 10 文件 / 8.2MB；PROTOS 两件标注"未核实可再分发"）。
      证据：`.gitignore`、`docs/third-party-captures.md`；`rg -n "wireshark" --glob '!wireshark/**'` 仅剩本地验证/报告引用。
- [x] **G5-2** 删除前实测 33 文件 / 280.2MB（两份 130MB `packetsage.exe`、29MB tarball、17 个测试 `.db`、`RESULTS.md/RUN.md`）；已在确认目录就在仓库内之后逐文件清空并删除 `install-test/`，`Test-Path` 返回 `False`。`.gitignore` 的 `/install-test/` 保留，防止下次跑 `install_smoke.py` 时产物入库。
      证据：`Get-ChildItem -LiteralPath install-test -File -Recurse` → 0；目录已不存在。注：环境策略只允许"逐文件非递归删除"，`Remove-Item -Recurse` 会被拒绝，故采用枚举+单文件删除。
- [x] **G5-3** 新增端到端脱敏用例（`agent/tests/test_cli.py::test_secrets_never_reach_streams_report_or_engine_log`）：`-vv` 配置 dump、报告正文、引擎日志、引擎启动失败的错误路径四路都对合成 key `sk-live-hygiene-check-…` 断言"不出现且显示 `***`"；原有 `doctor` 脱敏用例继续覆盖 Rust 侧。
      证据：该用例通过；`tests/cli/exit_codes.py` 的 `config dump is redacted` 仍 43/43 绿。
- [x] **G5-4** 全仓明文扫描（排除 `target/`、`install-test/`、`.git/`）：真实明文只出现在 **`临时deepseekapikey.txt`、`agent/.env`** 两处，二者均已被 `.gitignore` 覆盖（`.env` 模式匹配任意层级；密钥文件名逐条列出）；其余命中全部是测试/文档占位串（如 `sk-do-not-print-me`）或代码里的 `api_key=` 变量名。
      证据：`rg -c -e 'sk-[A-Za-z0-9_-]{12,}' -e 'api_key=' -e 'Bearer ' -e 'DEEPSEEK_API_KEY='`；命中清单见上。

### G6 · 文档一致性（P1）

- [x] **G6-1** `README.md` §7 回填为 Rust 175 / Python 148 / CLI 43 / GUI 7；§3 的 `version --json` 键名改为 `git_hash`/`build_time`，`query` 行补 `--jsonl`/`--json <PATH>`；`Agent 提示词同步点 S5-S6.md` §6 回填为 148+175+43+7。
- [x] **G6-2** `README_v0.2.md` 页首加"⚠️ 已过期（归档）"标注，指向 `README.md` 与本文档 §3。
- [x] **G6-3** 采用"统一标注"方案：`M0～M2`、`M3～M6 v0.2`、`CLI 收口 v0.2`、`Agent CLI v0.1`、`Agent 系统提示词 v0.1` 五份规格书页首均加"验收口径（2026-09-20）：复选框不再逐条维护，验收由《GUI 前收口文档 v0.1》§3/§4 接管"。
- [x] **G6-4** 新增 `docs/adr/README.md` 索引：ADR-024…026 指向独立文件；ADR-001…007 指向《开发文档 v0.3》§33；ADR-013…023 指向各规格书章节；**ADR-008…012 全仓无任何引用，登记为缺口**待用户裁定（补文或声明弃号）。
- [ ] **G6-5** §2.4 的五条偏离已整理为评审清单，见**本文档 §7**；在用户逐条确认前不写入《开发文档》的"已确认偏差"表（保持"待确认"语义）。

### G7 · 外部环境（P2，本轮不做，但要留结论）

- [ ] **G7-1** PostgreSQL matrix（`--features postgres`；本机因 sqlx-postgres 的 Windows 依赖链被 AV 隔离而无法编译，见 D-8）。**本轮未做**：需 Linux/CI 环境。
- [ ] **G7-2** MSRV 1.80 实测（CI 有 job，本机只有 1.98.1）。**本轮未做**：本机无 1.80 toolchain。
- [ ] **G7-3** 六个 fuzz 目标累计 ≥3h 基线（当前只有 60s smoke）。**本轮未做**：长跑占用机时，按 §4.6 留在外部环境。
- [ ] **G7-4** 干净机器上按 README 走一遍"10 分钟判据"。**本轮未做**：需干净机/容器。

### G8 · GUI 集成（已迁出）

> v0.2 修订：GUI 的设计与实现不属本收口文档范围。原 G8 的 8 项（run 实时过程接口、流式粒度定稿、
> 引擎/会话生命周期管理器、后台线程渲染模型、停止按钮、数据结构冻结、provider 状态前置、入口与依赖）
> 已**迁至《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§3**，重编号为 **U1–U10**；其中 U1（Agent
> sidecar 协议）是 Tauri 方案独有的新增项，规格见该文 §4。
>
> 编号 G8 保留占位，避免 G1–G7 的既有引用错位；本收口文档不再跟踪界面侧条目。

---

## 4. 模拟测试

### 4.1 三层约定

| 层 | 含义 | 允许的模拟手段 |
|---|---|---|
| **L1 实测** | 直接跑真二进制 / 真测试 | 无 |
| **L2 模拟桩** | 替换外部依赖，专门验分支与退出码 | `PACKETSAGE_LLM_PROVIDER=mock`；`agent/tests/fake_engine.py`；清空 provider；非 tty；只读库；坏 YAML；黑洞端口 `127.0.0.1:9` |
| **L3 外部预期** | 本机构造不出来，只能写预期 | PostgreSQL、MSRV 1.80、1GB 档、3h fuzz、真实商用模型 |

三条硬规矩：

1. L2 **禁止真调外部 LLM**。要测"真 provider 的失败路径"，把 base_url 指到黑洞端口，不要打真域名。
2. 每条用例必须**同时断言 exit code + stdout 关键行 + stderr 关键行**。只断言退出码不算通过。
3. 用例脚本落在 `tests/cli/` 或 `agent/tests/` 并进 CI，不要写成一次性手敲命令。

### 4.2 L1 回归基线（本次已实测，每批次先跑这一组）

| # | 命令 | 预期 | 本次实测 |
|---|---|---|---|
| S01 | `powershell -File scripts/build.ps1 -Command test` | 0 failed | ✅ 175 / 0（2026-09-20 复测） |
| S02 | `powershell -File scripts/build.ps1 -Command clippy` | 0 warning | ✅ |
| S03 | `powershell -File scripts/build.ps1 -Command fmt` | exit 0 | ✅ |
| S04 | `cd agent; python -m pytest -q` | 全绿 | ✅ 148（2026-09-20 复测） |
| S05 | `python tests/cli/exit_codes.py --binary target/debug/packetsage.exe` | 43/43 | ✅ |
| S06 | `python tests/integration/demo_mainline.py --binary target/release/packetsage.exe` | 十步全过，报告落盘 | ✅ |
| S07 | `packetsage doctor --json --no-net` | 10 items，id 顺序稳定 | ✅ |
| S08 | `packetsage version --json` + `packetsage schema` | `schema_version=2` | ✅ |

### 4.3 L2 已覆盖用例（由 S05 的 43 项直接对号，无需重写）

| # | 场景 | S05 中的用例名 | 规格出处 |
|---|---|---|---|
| S09 | provider 未配置 → 不静默 mock | `unset provider is setup, not a silent mock` | Agent CLI §3 |
| S10 | `setup` 写 `.env` 且不回显 key | `setup writes agent/.env and masks the key` | Agent CLI §3 |
| S11 | chat 启动器四级解析全失败 | `chat launcher four steps fail` | 收口 §2.3 |
| S12 | chat 在非 tty 上拒绝（Windows 记录态） | `chat: pty REPL on POSIX, documented non-tty refusal on Windows` | 收口 §11 #43 |
| S13 | 引擎异常死亡透传退出码 | `panic guard` | 收口 §4 #32 |
| S14 | `--full` 中 Agent 失败降级 | `analyze --full agent failure degrades` | 报告降级矩阵 |
| S15 | 反幻觉硬失败 → exit 4 | `analyze --full lint hard failure` | 收口 §4 C9 |
| S16 | 未配置 provider 的 `--full` → exit 3 | `analyze --full without a provider is exit 3` | Agent CLI §3 |
| S17 | 非法规则 → `rules check` 失败 | `rules check invalid rule` | 收口 §4 #33 |
| S18 | `doctor` 全绿 / 失败项退出 3 | `doctor all green`、`doctor failing check exits 3` | 收口 §11 |
| S19 | 只读库三道闸（`mode=ro` / SELECT 前置 / 强制 `--readonly`） | `db query url without mode=ro`、`db query non-select pre-check`、`db query --readonly is mandatory` | 收口 §8.1 |
| S20 | `db migrate` 三态 | `db migrate --yes ok`、`db migrate non-interactive without --yes`、`db migrate second run is a no-op` | 收口 §8.2 |
| S21 | 损坏的工作目录配置层 | `analyze broken working-directory config` | 收口 §7.1 |
| S22 | 未知旗标 / 缺位置参数 | `analyze unknown flag`、`analyze missing positional` | 收口 §4 |
| S23 | `--jsonl` stdout 纯净性 | `--jsonl stdout is pure, stderr never carries events` | 收口 §3.2 T7 |
| S24 | 进度开关前后逐字节一致 | `progress on/off is byte-identical` | 收口 §3.3 T7 |
| S25 | 配置 dump 脱敏 | `config dump is redacted` | 收口 §7.2 T1 |
| S26 | 四 shell 补全幂等 | `completions idempotent for 4 shells` | 收口 §6 |
| S27 | help 快照稳定 + ASCII 标题 | `help snapshot is stable`、`ASCII title heads both --help pages` | 收口 §3.1 T8 |
| S28 | 跨进程：`agent run` 读已落库 task | `agent run over a persisted task` | Agent CLI §1 路径 C |
| S29 | 两种入口等价 | `console script and python -m are interchangeable` | 收口 §2.2 T8 |
| S30 | `doctor --json` 稳定且无副作用 | `doctor --json is stable and side-effect free` | 收口 §11 T6 |

### 4.4 L2 新增用例（CLI / 引擎侧，**必须先写脚本再跑**）

> 本节的 S40–S49 都是**后端行为**（机器出口字段、tty 进度节奏、并发写库、serve 常驻、大文件内存），
> 它们只是"因为 GUI 会这么用"才被测到。界面层的用例（S57–S65）已迁出，见 §4.8。

| # | 用例 | 命令 / 手段 | 通过判据 |
|---|---|---|---|
| S40 | findings 机器可读 | `packetsage query findings --db sqlite://packetsage.db --jsonl` | 每行 `json.loads` 成功；键集合与 §5.10 快照一致。**依赖 G1-1** |
| S41 | sessions / stats / alerts 机器可读 | 同上换子命令 | 同上。**依赖 G1-1** |
| S42 | `agent run` 结构化出口 | `packetsage-agent run --task-id … --json`（mock provider） | stdout 恰好一行 JSON；键集合含 §5.11 列出的全部字段。**依赖 G1-3** |
| S43 | `version --json` 键快照 | 连跑两次，比对键集合与顺序 | 两次逐字节一致；键集合 = §5.1 |
| S44 | `schema` 键快照 | 同上 | 含 `agent_max_llm_calls`。G1-4 已落地，S44 7/7 绿 |
| S45 | 事件流文件模式一致 | `analyze X --json events.jsonl` 与 `analyze X --jsonl > events.jsonl` | 两份文件逐字节一致（排除 `task_id` 与时间戳后） |
| S46 | 进度节奏 | tty 下 `analyze samples/synth-10m.pcapng`；非 tty 下重定向 | tty 约 200ms/行、非 tty 约 5s/行；`-q` 时 stderr 无进度；终止行 `done: N packets in T s` |
| S47 | 并发写同一库 | 同时启动两个 `analyze … --db sqlite://c.db` | 无 panic、无损坏；`busy_timeout` 生效；`db query` 可读 |
| S48 | serve 常驻稳定性 | 连续 `ping` 100 次（GUI 常驻会这么用） | 100/100 应答，无超时、无 fd 泄漏 |
| S49 | 大文件内存 | `analyze samples/synth-10m.pcapng --full` | 进程 RSS 可被读取且记录（依赖 G3-1） |

**执行结果（2026-09-20）：** S40–S45 已落脚本 `tests/cli/gui_contract.py` 并全部通过（release 二进制 **7/7**，含额外的 `query --json <PATH>` 文件模式）。脚本自建临时库：先 `analyze` 落一个固定 task，再用 mock provider 跑一次 `agent run` 产生 findings，随后逐个子命令断言键集合、退出码与三流纪律。

S46–S49（tty 节奏、并发写库、serve 100 ping、大文件 RSS）仍未做：S46 需要真 tty，S49 依赖 G3-1 的 RSS 采集。

### 4.5 失败注入矩阵（L2，全部要脚本化）

| 注入 | 手段 | 预期 |
|---|---|---|
| provider 未配置 | 清空 `PACKETSAGE_LLM_PROVIDER` 与 `agent/.env` | exit 3 + `packetsage-agent setup` 提示 |
| provider 401 / 超时 | base_url 指 `http://127.0.0.1:9` 或假 key | `analyze --full` 降级 WARN、exit 0；`packetsage-agent run` exit 3 |
| 引擎半途死亡 | `agent/tests/fake_engine.py` 正常应答后退出 | 退出码按 `{2,3,4}` 透传 |
| JSONL 半行 / 乱序 | fake engine 故意截断 | 不 panic，错误可读，退出码可判定 |
| 非 tty 调 chat | 管道调用 | exit 1 |
| GBK 控制台 | `PYTHONIOENCODING=gbk` 跑 chat/analyze | 进度行退化为替换字符，不 `UnicodeEncodeError` |
| 只读库被写 | `db query --readonly` 且 url 无 `mode=ro` | 拒绝 + exit 3 |
| 非法规则 YAML | 改坏一条 `rules/**` | `rules check` 失败（实测为 exit 3） |
| 未知配置键 | 在 `agent`/`llm` 节加 `unknown: 1` | exit 3 |
| 配置内出现凭据 | `packetsage.yaml` 写 `api_key:` | 拒绝加载 |
| 输出目录不可写 | 目标目录设只读 | 修复提示，不 panic |
| 坏包 | `packetsage analyze samples/synth-malformed.pcap` | `decode_error` 事件后任务继续，`task_finished.packets` 含坏包计数 |
| snaplen 截断 | 小 snaplen 样本 | 事件带 `truncated: true` |
| 空 / 0 字节文件 | 空文件当输入 | exit 2（capture error），不得 panic |

### 4.6 L3 外部环境预期（**没有跑过，不许勾**）

| # | 项 | 判据 | 当前证据 |
|---|---|---|---|
| S50 | PostgreSQL matrix | `cargo test -p packetsage-storage --features postgres` 全绿；`--db postgres://…` 端到端 | 仅代码就绪，feature 默认关（D-8） |
| S51 | MSRV 1.80 | `cargo +1.80.0 check --workspace` 通过 | 本机无 1.80；CI job 未在此环境跑过 |
| S52 | 100MB / 1GB 档 | 100MB ≤10s；1GB ≤60s 且 RSS ≤1GB | 只测到 10MB，RSS 采集为 0 |
| S53 | fuzz 3h 基线 | 六目标累计 ≥3h 零 panic | 只有 60s smoke |
| S54 | E1–E6 pinned 真实模型 | 6/6 ok 且报告归档 | openai 记录：E2 failed、E3/E6 findings=0 |
| S55 | 干净机器 10 分钟判据 | README quickstart 三步可跑通 | 未在干净机器验证 |
| S56 | golden fixtures | 三类样本 normalize 后逐字节比对 | 仓库无任何 golden 文件 |

### 4.7 模拟运行记录（本次 dry-run）

* **L1（S01–S08）**：全部实测通过，数字见 §2.1（2026-09-20 复测：Rust 175 / Python 148 / CLI 43 / GUI 7）。
* **L2（S09–S30）**：由 S05 的 43/43 覆盖，本次实测通过。
* **L2 新增（S40–S45）**：✅ 已跑，7/7 通过（`tests/cli/gui_contract.py`）；**S46–S49 未跑**（真 tty / 并发 / 常驻 / RSS，依赖 G3-1）。
* **L3（S50–S56）**：**未跑**，仅有表中"当前证据"那一列的间接依据。

结论（v0.2 修订）：**回归基线是绿的，CLI 侧机器出口已经补齐并脚本化**（Rust 175 / Python 148 / CLI 43 / GUI 契约 7）。
本收口文档的 P0 项——G1、G2、G5 与 G6 的文档部分——已关闭，**后端侧契约已经冻结，可以移交给 GUI 对话**。

GUI 定为 **Tauri 桌面应用**后，界面不再与 Agent 同进程：取数走下面 §5.4 的 RPC，
操作走《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§4.5 的新协议（本版最大的新增接口）。
这不改变上面的结论；界面侧剩余的开工阻塞项见该文 §3 的 U1–U4。

### 4.8 GUI 层用例（已迁出）

> v0.2 修订：原 S57–S65 属界面层验收，已迁至《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§8.1，
> 并随 Tauri 方案扩为 **S57–S70**（新增握手版本协商、sidecar 崩溃感知、事件体积上限、并发命令、协议清单快照）；
> 该文 §8.2 另有人工验收清单 **M1–M24**，重心是干净机器安装与桌面行为。本文档 §4 只保留后端用例（S01–S56）。

---

## 5. GUI 接口冻结清单

以下内容取自**实际运行输出**（2026-09-20），不是从文档抄的。GUI 只依赖本节；本节未列出的东西一律视为内部实现。

### 5.1 命令面与机器输出

| 命令 | 机器出口 | 备注 |
|---|---|---|
| `packetsage version [--json]` | ✅ JSON（键序冻结：`version, schema_version, git_hash, build_time, profile, rustc, target`） | G1-5 已定稿：以实现为准，无 `git`/`built` 别名 |
| `packetsage schema` | ✅ JSON | 事件表、退出码、限额、RPC 方法表 |
| `packetsage doctor [--json] [--no-net]` | ✅ JSON（`items[]`，每项 `id/name/status/detail/repair_hint`） | `status ∈ {ok, warn, fail, skipped}`（源码 `doctor.rs` 的 `Status` 四个变体） |
| `packetsage analyze <cap>` | ❌ 人读表格 | GUI 用 `--jsonl` |
| `packetsage analyze <cap> --jsonl` | ✅ JSONL（schema v2） | stdout 纯机器流 |
| `packetsage analyze <cap> --json <PATH>` | ✅ 落盘的 JSONL | 与 `--jsonl` 同源 |
| `packetsage analyze <cap> --full --report <p>` | ⚠️ 报告是 Markdown | 结构化 findings 需 RPC 或 DB |
| `packetsage query {sessions,stats,alerts,findings}` | ✅ 人读表格（默认）/ `--jsonl` 机器行 / `--json <PATH>` 落盘 | G1-1 已落地；字段表见 §5.10 |
| `packetsage-agent run --task-id …` | ✅ 人读摘要（默认）/ `--json` 单行对象 | G1-3 已落地；对象定义见 §5.11 |
| `packetsage db query --readonly --jsonl` | ✅ JSONL（每行一个对象） | 唯一只读 SQL 出口，永远不接受裸 SQL |
| `packetsage db migrate [--yes]` | ❌ 人读 | 非交互无 `--yes` 时 fail-closed |
| `packetsage rules list / check` | ❌ 人读 | |
| `packetsage serve` | ✅ JSONL RPC（stdin/stdout） | Agent 的唯一出口（ADR-018） |
| `packetsage-agent report --report <p>` | ⚠️ Markdown | |
| `packetsage completions <shell>` | 文本 | 幂等 |

命令面的一个不对称（R8）：`packetsage analyze --full` 接受 `--provider/--model`，而 `packetsage-agent run/chat` **已移除**同名旗标，provider 只走 `agent`/`llm` 配置节 + 环境变量。GUI 若两处都要用，必须按各自命令面处理。

### 5.2 退出码（`packetsage schema` 实测）

| code | 语义 |
|---|---|
| 0 | success |
| 1 | invalid argument |
| 2 | capture error |
| 3 | config / database / llm error |
| 4 | internal error **或** anti-hallucination hard failure（双语义，stderr 可区分） |
| 5 | unsupported feature |

Agent 侧补充约定（`Agent CLI 工程规格书 v0.1`）：argparse / 非 tty = 1；查无 task / 引擎起不来 / LLM 配置 = 3；引擎异常死亡按 `{2,3,4}` 透传；lint 硬失败与未捕获异常 = 4；降级收尾仍是 0 + WARN。

### 5.3 事件流（schema v2，9 类）

`task_started` · `capture_info` · `packet` · `decode_error` · `session_summary` · `stream_state` · `stats` · `alert` · `task_finished`

`packet` 事件含：`packet_index / ts_unix_ns / ts_precision / interface_id / captured_len / original_len / linktype / truncated / network{…} / transport{…} / payload_ref{…} / decode_status`。

### 5.4 RPC 方法表（16 个，实测自 `packetsage schema`）

生命周期：`ping`、`analyze_file`
**九工具（Agent 可见）**：`get_capture_summary`、`get_protocol_stats`、`get_conversations`、`filter_packets`、`inspect_packets`、`reconstruct_stream`、`check_alerts`、`query_history`、`get_task_artifacts`
证据链：`validate_finding`、`submit_finding`、`submit_report_meta`
规则：`list_rules`、`check_rules`

工具结果信封（冻结契约）：

```json
{"_id":"tc_01M2Y6FHRMSHMT35922FPKCC8","source":"engine","trusted_as_instruction":false,
 "redactions":[],"method":"get_capture_summary","content":{…}}
```

不变量：envelope `_id` = 台账主键 = `EvidenceRef._id` = 报告 Evidence 索引（四段对拍，ADR-024/026）。`content` 内还带 `_anchor` / `_ref_ids` / `_numbers`，供模型逐字复制。

### 5.5 数据库

表（`migrations/0001_init.sql`，共 8 张）：

`analysis_tasks` · `captures` · `sessions` · `alerts` · `agent_runs` · `tool_calls` · `findings` · `artifacts`

索引：`sessions_task_bytes(task_id, bytes DESC)`、`alerts_task_severity(task_id, severity)`、`tool_calls_run(agent_run_id, step)`

`migrations/0002_tool_call_evidence_facts.sql` 新增三列（跨进程报告的反幻觉依据）：

`tool_calls.numbers_json`（TEXT NOT NULL DEFAULT `'{}'`）· `tool_calls.ref_ids_json`（默认 `'[]'`）· `tool_calls.tokens_json`（默认 `'[]'`）

迁移总数实测为 2（`doctor` 的 `database` 项报 `2 migration(s) applied`）。schema 变更一律走
`packetsage db migrate`，GUI 不得直接改库；只读访问走 `db query --readonly`（url 必须含
`mode=ro`，语句必须是 SELECT/WITH/EXPLAIN）。

### 5.6 报告九节（`docs/report-spec.md`）

| § | 节 | 数据来源 |
|---|---|---|
| 1 | Executive Summary | `query_history kind=findings`（仅 accepted） |
| 2 | Capture Overview | `get_capture_summary` + `get_task_artifacts` |
| 3 | Protocol Statistics | `get_protocol_stats` |
| 4 | Top Conversations | `get_conversations sort_by=bytes limit=20` |
| 5 | Rule Alerts | `check_alerts` |
| 6 | Agent Findings | `query_history kind=findings` |
| 7 | Evidence | `findings[].evidence` + tc 索引表 |
| 8 | Limitations | `get_capture_summary` 计数 + 应用层统计 |
| 9 | Analysis Trace | `query_history kind=trace`（`_id, stage, method, args, duration_ms, status, ts_unix_ns`） |

模板版本 `v1`、提示词版本 `v2`（`agent.prompt_version` 可选 `v1` 回滚）。

### 5.7 限额常量（`packetsage schema` 实测）

| 常量 | 值 | 是否已暴露 |
|---|---|---|
| `agent_max_steps` | 12 | ✅ |
| `agent_max_tool_calls` | 20 | ✅ |
| `reconstruct_stream_bytes` | 262144 | ✅ |
| `tool_result_chars` | 8000 | ✅ |
| `agent_max_llm_calls` | 24 | ✅（G1-4 已落地） |

### 5.8 `doctor --json` 的 10 个 id（顺序被测试锁定）

`binary` · `config` · `rules` · `samples` · `database` · `paths` · `python` · `provider` · `e2e` · `schema`

GUI 可直接拿 `provider` 项的 `status/detail` 判断是否需要引导用户跑 `packetsage-agent setup`。

### 5.9 GUI 不得依赖的东西

* `target/` 下的路径与文件名、二进制大小；
* 人读表格的**列宽与空格**（G1-1 已补 `--jsonl` / `--json <PATH>`，一律走机器出口）；
* 进度行文本格式（`analyze: phase=… packets=… speed=…`）——只可用于日志展示，不得用于状态机；
* stderr 的具体措辞（只保证"hard failure 时含 `anti-hallucination`"这一条）；
* `run_id` / `task_id` 之外的时间戳字符串格式（`ts_unix_ns` 是字符串，不是 int）；
* 内部 crate 结构、`serve` 的进程模型。

### 5.10 `packetsage query --jsonl / --json <PATH>`（G1-1，已冻结）

每行一个 JSON 对象（JSON Lines），键按字段名排序；查询结果为空时 stdout **没有行**（exit 仍是 0），GUI 必须自己处理"零行"。`--json <PATH>` 写同一份行到文件，此时不再打印人读表格；同时给 `--jsonl` 会两处都写。字段集合由 `tests/cli/gui_contract.py` 的 S40/S41 锁定，只能新增、不得改名。

| 子命令 | 每行字段 | 备注 |
|---|---|---|
| `sessions` | `id, task_id, protocol, src_ip, src_port, dst_ip, dst_port, first_ts, last_ts, packets, bytes, state, state_stored, app_protocol, interface_id, vlan_tag, direction_basis` | `state` 为线协议词汇（`new/active/half_closed/closed/incomplete/buffer_overflow`）；`state_stored` 是库内 Debug 拼写（如 `halfclosed`），供排障 |
| `stats` | `task_id, layer, protocol, sessions, bytes` | 会话级聚合，一个 protocol 一行；`stats` 是唯一没有 `id` 的子命令 |
| `alerts` | `id, task_id, rule_id, rule_version, rule_content_hash, severity, first_packet, last_packet, first_ts, last_ts, src_ip, dst_ip, session_id, group_json, evidence_json, group, evidence` | `group`/`evidence` 是把两个 `*_json` 列解析后的便利字段；解析失败时为 `null`，原串仍在 |
| `findings` | `id, task_id, title, severity, basis, basis_stored, summary, evidence_json, evidence, validator_status` | `basis` 已归一为 `rule_match/direct_observation/correlated_observation/hypothesis`；`basis_stored` 是库内拼写（如 `rulematch`）；`evidence` 是已解析数组 |

### 5.11 `packetsage-agent run --json`（G1-3，已冻结）

stdout **恰好一行** JSON（人读摘要、finding 明细行都不再出现；stderr 的进度/WARN 不变）。对象字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `summary_version` | int | 当前为 `1`；结构变化时递增 |
| `run_id` / `task_id` | string | `run_<uuid>` / `task_<ulid>` |
| `status` | string | `completed` / `degraded` / `failed`（`ok` 已映射为 `completed`） |
| `stop_reason` | string \| null | `budget` / `step_budget` / `invalid_arguments` / `homogenisation` … |
| `accepted` | int | 通过 V1–V4 + `submit_finding` 的 finding 数 |
| `submit_rejects` | int | 被引擎拒绝的 finding 数 |
| `malformed_output` | bool | 模型两次产出非法 payload |
| `steps` / `calls` / `tool_calls` | int | 策略计数（`calls` = LLM 轮数） |
| `tokens` | object | `{in, out, total}` |
| `cache` | object | `{hit, miss}`：输入侧缓存命中/未命中 tokens（DeepSeek 上下文硬盘缓存）；provider 不报就是 `0/0`。**v0.4 追加**，与 §4.5 的事件表同源 |
| `cost_cents` | int | 与预算单位一致（10 = 1¢） |
| `prompt_version` | string | 本次 run 用的提示词版本（当前默认 `v2`） |
| `model` / `provider` | string | 生效模型与 provider kind |
| `summary` | string | **模型自己写的那段总结**（2–4 句中文）：对话里"它说了什么"，`chat` 模式下就是回答；模型没给就是空串。**v0.4 追加** |
| `findings` | array | 元素固定为 `id, severity, basis, title, evidence_ids`；`id` 为引擎分配的 `F-{n:03}`，`evidence_ids` 是 tc 台账 `_id` 数组 |

### 5.12 Python 侧集成面（GUI 的首选取数路径，v0.2 新增）

GUI 与 Agent 同为 Python，**首选直接 import，而不是 subprocess 解析文本**。以下对象取自源码实测
（`agent/packetsage_agent/`），字段名冻结后不得改名。

| 对象 | 位置 | GUI 用途 | 稳定性 |
|---|---|---|---|
| `EngineClient(cmd, timeouts, env, log_path)` | `engine_client.py` | spawn 并驱动 `packetsage serve`；`call(method, params, timeout_s)` / `call_envelope(method, params, step)` | 冻结 |
| `RpcTimeouts` | 同上 | `ping=5s / query=10s / analyze_file=60s / reconstruct_stream=30s / default=10s`，可用 `overrides` 覆盖 | 冻结 |
| `CallResult` | 同上 | 单次工具调用结果：`method / args / ok / envelope / error / duration_ms`，另有 `tc_id`、`content` 属性 | 冻结 |
| `EngineSpawnError` / `EngineCrashed` / `RpcToolError` / `ToolTimeout` | 同上 | 错误分类渲染：能否重试、是否要重建引擎；`EngineCrashed.stderr_tail(3)` 给排障尾巴 | 冻结 |
| `EngineClient.heartbeat()` / `.unhealthy` | 同上 | 常驻会话健康探测（连续 2 次失败标记 unhealthy） | 冻结 |
| `AgentBudget` | `policy.py` | 预算常显：`max_steps=12 / max_llm_calls=24 / max_tool_calls=20 / max_same_tool_calls=5 / max_tokens=200000 / max_cost_cents=500` | 冻结 |
| `PolicyState` | `policy.py` | 实时计数：`steps / llm_calls / tool_calls / tokens_in / tokens_out / cost_cents / traces[]`；**`traces` 在 run 中实时追加**（GUI 可用它做实时工具卡片，见《GUI 工程规格书》U1 路径 ①） | 冻结 |
| `build_agent(...)` / `PacketSageAgent(...)` | `agent.py` | 装配；`observer=` 接受任何带 `tool_call` / `llm_round` 方法的对象（CLI 传的是 `progress.Progress`） | 冻结 |
| `PacketSageAgent.run(task_id, goal)` | `agent.py` | 跑一轮 | 冻结 |
| `PacketSageAgent.request_finalize()` | `agent.py` | 停止按钮（下一个边界收尾、保留部分结果） | 冻结 |
| `AgentRunResult` | `agent.py` | 终态对象（字段见下） | 冻结 |
| `ToolCallRecord` | `models.py` | 工具卡片数据：`step / tool_name / args_json / result_summary / status / duration_ms / tc_id` | 冻结 |
| `FindingDraft` + `parse_finding(payload, evidence_lookup)` | `models.py` | finding 卡片构造 | 冻结 |
| observer 钩子 `tool_call(step, tool, ok)` / `llm_round(state, budget)` | `progress.py` | 现有进度钩子；**载荷偏薄**，是否扩展由《GUI 工程规格书》U1 决定 | 待定 |

`AgentRunResult` 字段（实测自源码）：`agent_run_id / task_id / status / findings[] / trace[] / stop_reason /
malformed_output / tokens_in / tokens_out / cost_cents / model / prompt_version / notes[] /
submit_rejects / stored_findings[]`。

注：`run --json`（§5.11）与 `AgentRunResult` 是**同一份信息的两种出口**，CLI 版做了 `ok → completed`
之类的线协议归一。**GUI 走 Python 时直接用对象，不要再绕 CLI 解析 JSON**（否则同一信息两套解析会漂移）。

---

## 6. GUI 设计基线（已迁出）

> v0.2 修订：原 §6（骨架映射、本项目特有一等公民、明确不做、框架硬约束）属界面设计，
> 已迁至《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§5（设计基线），并在该文 §6 补充了
> **交互流程与运行状态机**（首次运行 / 主流程 / 七种状态 / 错误呈现三条规矩）。
>
> 本文档不再涉及界面结构；如需了解 GUI 长什么样、怎么用，以该文为准。

---

## 7. 执行顺序与估算

| 批次 | 内容 | 依赖 | 预估 |
|---|---|---|---|
| B1（P0） | G1-1、G1-3、G1-4、G1-5：补四个机器出口 + 键名定稿 + 快照测试 | 无 | ✅ **已完成**（含 S40–S45 脚本） |
| B2（P0） | G2-1…G2-4：ruff / mypy / lock / CI | 无 | ✅ **已完成**（uv.lock 64 包） |
| B3（P0） | G5-1、G5-2、G5-3：wireshark 处置、产物清理、密钥路径复核 | 无 | ✅ **已完成**（G5-1/G5-2/G5-3 关闭，G5-4 顺带完成） |
| B4（P1） | G3-1…G3-4：RSS 采集、基线入库、ratchet job、100MB/1GB | B2 之后的 CI 改动 | 未开始（可与 GUI 并行） |
| B5（P1） | G4-1…G4-4：E2 定位、真实模型归档、V5 决策 | 需要真实 key 与预算 | 未开始（需真实 key） |
| B6（P1） | G6-1…G6-5：文档回填 | 与各批同时 | ✅ **已完成 4/5**；G6-5 待用户逐条确认（见 §8） |
| B7（P2） | G7-1…G7-4：外部环境 | 需 Linux / 干净机 / 时间 | 未开始（按计划另排） |

**本文档的后端批次到此为止**：B1–B3 已完成（P0 全关），B4–B7 可与 GUI 并行或另排外部环境。
界面侧的批次（原 B0 / B8）已随 G8 迁出，见《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§3 的 U1–U10。

---

## 8. §2.4 偏离的评审清单（G6-5，待用户确认）

下列五条目前是"**待确认的偏离**"，不是"已完成"。请逐条给出"确认/不接受"，确认后才写入《开发文档》的偏差表：

| # | 偏离 | 现状证据 | 确认后要写的风险 |
|---|---|---|---|
| D1 | V5（Python 侧自由文本数字扫描）未实现，反幻觉只靠 Rust V1–V4 + 报告 lint | `docs/architecture.md` D-18；`validate_finding` 只回 V1–V4 | 模型自由文本里的数字没有 Python 侧防线，只有报告 lint 兜底 |
| D2 | ADR-019 store-backed 冷恢复改为"按 `source_path` 重放 pipeline" | `Agent CLI 同步点 S7-S8` 用例；S42 实测跨进程 task 可用 | 源文件被移动/删除后 `CAPTURE_UNAVAILABLE`（exit 3），恢复成本随文件大小上升 |
| D3 | provider 轮级干净取消 / `agent_runs.status=interrupted` 不实现，改"跑完本轮 + WARN" | `docs/architecture.md` D-16；S1–S4 #28 | Ctrl-C 后当前 LLM 轮仍会跑完，可能多花 token |
| D4 | 规则窗口预算超限语义保留"最新一半事件 + `degraded=true`"，非计数摘要 | 开发文档 D-10 | 窗口证据是采样而非全量，报告需按 `degraded` 标注 |
| D5 | Agent CLI 移除 `--capture/--provider/--model/--scenario` 与 `eval` 子命令 | `Agent CLI 工程规格书 v0.1`；`packetsage-agent --help` 实测 | `analyze --full` 与 `packetsage-agent run/chat` 命令面不对称（R8），GUI 需按各自命令面调用 |

---

## 9. 风险

| # | 风险 | 影响 | 处置 |
|---|---|---|---|
| K1 | 消费方直接解析 `query` 的人读表格 | 后端一改就碎 | ✅ 已消除：G1-1 已补 `--jsonl` / `--json <PATH>`，S40/S41 锁字段 |
| K2 | `version --json` 键名在上线后被改动 | 消费方全线报错 | G1-5 定稿（以实现为准）+ S43 快照 |
| K3 | `wireshark/` 与 `install-test/` 随仓库公开 | 版权 + 体积 | G5-1 / G5-2 |
| K4 | Python 侧无 lint / 无锁 | GUI 后端不可复现 | G2 |
| K5 | 真实模型 E2 失败、E3/E6 findings=0 | 演示/评测时"分析不出东西" | G4-1/G4-3；演示前先跑一次确认 |
| K6 | 规格复选框仍为空 | 评审认为未完成 | G6-3 |

界面侧风险（双 sidecar 生命周期、长 run 阻塞 UI、MSVC 工具链、PyInstaller 产物被 AV 误杀、
同一信息两套解析等）已迁至《[GUI 工程规格书 v0.2](GUI%20工程规格书%20v0.2.md)》§9 的 P1–P8。

---

## 10. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v0.1 | 2026-09-20 | 首版：基于本机实测（Rust 169 / Python 146 / CLI 43 / 主线全过）建立收口判据、三层模拟测试剧本与 GUI 接口冻结清单 |
| v0.1 修订 | 2026-09-20 | B1–B3 执行记录回填：G1/G2/G5/G6 的 P0 项关闭（Rust 175 / Python 148 / CLI 43 / GUI 契约 7），新增 §5.10、§5.11 与 §8 评审清单 |
| **v0.2** | 2026-09-20 | **拆分：GUI 的设计与实现移出本文档**，另立《GUI 工程规格书》（承载技术栈与风格决策、U 系列、设计基线、交互流程与状态机、界面验收、演示路径）。本文档相应修订：§1.2 范围、§1.4 改为"与 GUI 文档的分工"、G8 与 §4.8 与 §6 改为迁出指针、§7 移除界面批次、§9 移除界面风险；**保留并强化 §5.12（Python 侧集成面冻结）**，因为"后端冻结了什么"属收口范畴。另修掉重复的 §7 编号 |
| **v0.3** | 2026-09-20 | GUI 交付形态由 Streamlit 改为 **Tauri 桌面应用**（可双击安装），GUI 文档升到 v0.2 并大改。本文档随之修订：把所有 GUI 文档引用改指 v0.2；§0 GUI 范围行、§1.4 的分工与后果段改按 Tauri 描述（**GUI 不再与 Agent 同进程 → §5.12 的 Python 模块面降为 Agent sidecar 的内部实现面**，GUI 取数走 §5.4 RPC + 新增的 sidecar 协议）；G8 / §4.8 / §6 / §7 / §9 的指针同步更新（U1–U10、S57–S70、M1–M24、P1–P8）。**后端契约本身无变化**，§5 全部照旧有效 |
