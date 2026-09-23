# CLI 收口同步点 S1–S4（《CLI 收口工程规格书 v0.2》§0.5 执行记录）

《CLI 收口工程规格书 v0.2》§0.4 判定"不需要新 ADR"，但要求 4 个文档同步点随实施提交。
本文件记录每个同步点的落点与证据；对应"《开发文档》§22 增补（v0.4 修订页）"的建议。

| S# | 规格要求 | 本仓库落点 | 证据 |
|---|---|---|---|
| S1 | `pyproject` 增 `[project.scripts]`：`packetsage-agent = "packetsage_agent.cli:main"` | `agent/pyproject.toml`（既有）+ `agent/packetsage_agent/cli.py` 固定 `prog="packetsage-agent"` | `tests/cli/exit_codes.py::console script and python -m are interchangeable` |
| S2 | 新增 `db` 子命令与 `version`/`completions`；launcher 解析顺序；`PACKETSAGE_AGENT_BIN` / `PACKETSAGE_CONFIG` 环境变量名 | `crates/packetsage-cli/src/cli.rs`（`db`/`version --json`/`completions`）、`src/agent_launcher.rs`（四级解析）、`src/config.rs`（发现链） | §4 矩阵中 `db *` 6 行 + `chat launcher four steps fail` |
| S3 | `agent_runs.status` 增 `interrupted` 枚举（视 #28 实测结论） | **未落库**：本批次 #28 实测结论为 provider（HTTP 一次性请求）不支持轮级干净取消，按规格 §10 走"完成本轮再回 prompt + WARN"分支，因此不新增枚举值 | 见下节"#28 结论" |
| S4 | §4.1 chat spawn 目标表述由本规格 §2.3 取代 | `src/agent_launcher.rs` 头部注释与实现：`$PACKETSAGE_AGENT_BIN` → PATH `packetsage-agent`（3s 探测）→ `agent/.venv` → `python -m` fallback（WARN） | `chat launcher four steps fail` / `chat Ctrl-D exits 0` |

## 环境变量命名（#35 结论）

规格 §7.1 公布的新名字与实现里已有的旧名字**同时支持**，新名字优先：

| 语义 | 新名字（规格） | 兼容旧名字（既有实现） |
|---|---|---|
| 数据库 | `PACKETSAGE_STORAGE_URL` | `PACKETSAGE_DB` |
| 规则目录 | `PACKETSAGE_RULES_PATH` | `PACKETSAGE_RULES` |
| provider | `PACKETSAGE_LLM_PROVIDER` | `PACKETSAGE_PROVIDER` |
| 模型 | `PACKETSAGE_LLM_MODEL` | — |
| API Key | `PACKETSAGE_LLM_API_KEY` | `OPENAI_API_KEY`（Python 侧消费） |
| 引擎二进制 | — | `PACKETSAGE_ENGINE`（launcher 传递） |
| 解释器 | — | `PACKETSAGE_PYTHON`（doctor 第 7 项） |

Rust 侧**不**加载 `.env`（避免双 dotenv 漂移，§7.1）；`.env` 仅由 Python 侧
`packetsage_agent.config.load_dotenv()` 读取，且环境变量优先。

## 待验证项的本批次结论

| # | 项 | 结论 |
|---|---|---|
| 27 | doctor 检查项口径（8 vs 10） | 采用规格 §11.1 的 10 项权威清单；`--json` 的 `items[].id` 顺序被测试锁定 |
| 28 | provider 轮级干净取消 | 不支持：`httpx` 单次请求无法在半途回滚，故走 §10 的"完成本轮 + WARN"，不新增 `interrupted` 状态 |
| 29 | Windows console_script 探测 | `agent_launcher::find_on_path` 走 `PATHEXT`，覆盖 `packetsage-agent.exe`；`chat Ctrl-D exits 0` 在本机实测通过 |
| 30 | pipeline 进度字段覆盖度 | 现有 `AnalysisStore` 计数不足（无 phase/offset），故按规格允许的最小钩子新增 `ProgressCounters`（三个原子变量、无锁、无 channel） |
| 31 | `PACKETSAGE_CONFIG` 与 .env 互动 | Rust 不读 `.env`；launcher 用 env 传递配置路径，Python 侧 `.env` 只补充环境变量，不覆盖 |
| 32 | release panic 策略 | `unwind` + `catch_unwind` → stderr 一行 panic 摘要 + exit 4；`PACKETSAGE_PANIC_FOR_TEST` 钩子让该行可脚本化断言 |
| 33 | `rules check` 退出码 | 实测为 3（规则非法按配置类归口），已回填 §4 矩阵 |
| 36 | sqlx/SQLite 查询级中断 | 不可用，故 `--timeout` 语义 = 连接建立与锁等待（`busy_timeout`），长查询防护靠 `--limit`（≤10000） |
| 37 | launcher 探测超时 | 采用 3s（`agent_launcher::PROBE_TIMEOUT`） |
| 34 | tarball install-from-scratch 演练记录 | 本机演练目录由 `scripts/install_smoke.py` 生成：`install-test/RESULTS.md`（源码安装 + tarball 解包 + SHA256SUMS + 能力矩阵 17 项全过）；CI 侧对应 `install-from-scratch` 与 `release-tarball` 两个 job |

## 附带修复（v0.2 实施期发现，随本批次提交）

| 源 | 现象 | 修复 |
|---|---|---|
| `scripts/run_cli.cmd`（双击启动器，`install_smoke` 复制成 `install-test\RUN-CLI.cmd`） | ① 优先挑 **tarball** 那份（发行产物、不含 Python Agent），并在 "Useful commands" 里推荐这条路径上跑不通的 `chat`；② 不打印 ASCII 标题；③ PATH 提示永远写 `%USERPROFILE%\.cargo\bin`，与实际在用的二进制不是同一个 | 选择顺序改为 **源码安装 → `target/release` → `target/debug` → tarball** 并打印 `Via:`；探测 Agent（`where packetsage-agent` 或 `python -m packetsage_agent --version`）决定是否列 `chat`/`--full`，否则给 `pip install -e agent` 提示；标题取自 `crates/packetsage-cli/src/banner.txt`（与二进制内嵌同源，脚本自身保持纯 ASCII）；PATH 提示改用实际二进制的目录 |
| `crates/packetsage-cli/build.rs` | `built=` 是**冻结值**：脚本只声明 `rerun-if-changed=build.rs`，等于关掉 Cargo 默认的"包变更即重跑"，于是 12:57 编出的二进制报 `built=11:32`，版本行的构建时间不可信（§5 的构建元数据失真） | 改为监视 crate 真实输入（`build.rs`/`Cargo.toml`/`src` + 四个被链接的 workspace crate 的 `src`），保留 `rerun-if-env-changed=SOURCE_DATE_EPOCH`（可复现构建不变）；修复后 `built=` 与本次构建时刻一致（实测 `05:11:26Z`） |
