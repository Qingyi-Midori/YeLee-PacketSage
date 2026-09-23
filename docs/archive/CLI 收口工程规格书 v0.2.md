# YeLee' PacketSage — CLI 收口工程规格书 v0.2
> **验收口径（2026-09-20）：** 本文件正文保持不变，正文内复选框不再逐条维护；
> 当前验收由《GUI 前收口文档 v0.1》§3（G1–G5）与 §4（S01–S56）接管，测试基线见该文 §2.1。
> **版本：v0.2**（继承《M0～M2 Rust 工程规格书 v0.2 评审修订版》与《M3～M6 工程规格书 v0.2》全部契约；配套《开发文档 v0.3》§22～§25。v0.2 = v0.1 + 《v0.1 自审评意见》B1–B4、C1–C10 全量回收，映射见 §0.0）
> **状态：工程设计 / 待评审**
> **定位：里程碑 M6a**——M6 §6.3/§6.4 的前置兑现批次 + 开发文档 §22 CLI 契约的完整落地。上游输入：《从"跑得起来"到"像个产品"：CLI 收口路线》（下称《收口路线》）
> **标记体系：** 【事实】上游已核实 /【设计】本规格契约 /【待验证】需实现测试确认 / **【继承-M2v0.2§x】【继承-M3~M6v0.2§x】** 直接消费已冻结契约 / **【修订-收口路线§n】** 标注上游建议来源
> **总原则重申：** 不改引擎语义、不改协议与 DSL、不动任何已冻结契约；墙钟只允许出现在呈现与 I/O 节拍（ADR-015 的语义边界）；三流分离是 serve 正确性约束（JSONL over stdin/stdout）在 CLI 模式的直接延伸。
---
## 0. 文档范围、契约冻结与 ADR 判定
### 0.0 v0.2 修订概览（《v0.1 自审评意见》→ 修订位置映射）
| 意见 | 问题 | 修订落点 |
|---|---|---|
| B1 | §7.2 脱敏子串匹配误伤合法键（`max_tokens_total`） | §7.2 重定为分段精确匹配；§14.1 配置行 T1 负向用例 |
| B2 | launcher `<repo>` 路径不可解析 | §2.3 重定**四级**解析链 + `PACKETSAGE_AGENT_BIN` 显式覆盖（S2 增补） |
| B3 | Quickstart 判据在 tarball 安装路径必败 | §12 双路径能力矩阵 + tarball 附 `INSTALL.md`；§14.1 分发行 T3 |
| B4 | §3.2 `--jsonl` 全局定义与 §8.1 行流矛盾 | §3.2 重定义为"**该子命令的纯机器流**"并圈定适用面 |
| C1 | 文件发现链与 §24 键值优先级链未缝合；层 3 损坏处置缺失；.env 归属未写 | §7.1 三条增补；§4 措辞对齐 |
| C2 | exit 矩阵缺行 | §4 增 7 行 |
| C3 | `--jsonl` 适用面未声明 | §3.2/§8.1 注记 |
| C4 | doctor 无副作用约束缺失、`--no-net` 跳过集未定义、规则检查模式未定、"出海自检"歧义 | §11.1 第 3/5/8/9 项 + `--no-net` 行 |
| C5 | `--timeout` 在 sqlx/SQLite 无查询级中断；migrate 探测措辞过强 | §8.1/§8.2 语义降级与 best-effort 措辞；#36 |
| C6 | version 两形态字段名不一致；"schema v1" 与协议 SCHEMA_VERSION 命名碰撞 | §5 字段映射 + "json 输出格式 v1"更名（doctor 同口径） |
| C7 | `--help` 逐字节断言的 prog 名陷阱 | §2.2 显式固定 prog 名 |
| C8 | §9 错误表选择准则缺失 | §9 表头注记 + 表外错误归属 |
| C9 | 进度采样对象措辞含混；缺确定性守护 | §3.3 采样对象定稿 + 进度 A/B 逐字节一致（T7） |
| C10 | 探测 1s 超时偏紧；默认日志级别未定义；S3 目标面不全 | §2.3 改 3s（#37）；§3.1 默认 warn；S3 目标补 M3~M6v0.2§3.7 |
### 0.1 覆盖与不覆盖
| 覆盖 | 不覆盖 |
|---|---|
| 入口安装（cargo install + console_script）、全局参数一致性、三流分离、进度反馈、exit code 核对、version 规范化、shell 补全、配置发现链与脱敏、db 护栏、错误信息人话化、REPL 体验、doctor 升级、README/分发 tarball | 引擎/规则/Agent/报告语义（归 M3～M5 已交付范围与 M6 正式批次）、PostgreSQL matrix、benchmark、fuzz 基线、E1–E6 评测、任何新分析特性 |
### 0.2 与 Checklist #15–30 及 M6 DoD 的关系
- 本规格为**可插队侧轨**：C1–C10（§15）可穿插在 #21–#28 之间执行，不破坏 #15–#30 的串行编号与验收点；
- **前置兑现 ≠ 关闭**：M6 §6.6 的对应 DoD 项（install-from-scratch 绿、doctor 十项全过、README 可复现、error-codes 文档校验）在 M6 验收时仍须以**当时的 CI 证据**重新勾选；本规格交付物使 M6 验收"只剩确认"；
- 预计消耗：4～6 个半天（§15；C8 排最后，不阻塞 M6a 验收）。
### 0.3 契约冻结声明（只实现、不修改）
| 冻结契约 | 本规格落点 |
|---|---|
| ExitCode 0–5【继承 dev doc §23 / M2v0.2§5】+ exit 4 双语义拆注【继承-M3~M6v0.2§5.5/§6.5】 | §4 核对矩阵逐命令验证；不新增、不重定义 |
| 配置优先级 CLI > Environment > config.yaml > defaults【继承 dev doc §24】 | §7 实现细化：**文件发现链（§7.1）与键值优先级链双链缝合**，错误处置 fail-fast——属细化非偏离 |
| serve JSONL over stdin/stdout【继承 dev doc §21】 | §3.2 三流分离向 CLI 模式延伸 |
| schema_version=2【继承 ADR-013】 | §5 version 输出运行时读 `packetsage_protocol::SCHEMA_VERSION`，禁止双源硬编码 |
| Fatal/Event 分离 + `PacketSageError` 枚举【继承 M2v0.2§5 / dev doc §25】 | §9 仅改 Display 呈现层；类型、退出码零变更 |
| ADR-018 单一写入方 | §8 `db query --readonly` 是消费侧护栏；`db migrate` 是既有 migrations 的执行入口，不引入第二写方 |
| ADR-015 包时间 | §3.3 进度使用墙钟——仅呈现与 I/O 节拍（先例：M3~M6v0.2§3.7 刷盘 200ms），**不进入任何业务判定** |
| 规则 DSL / RPC 树 / prompts / 报告模板 | 一律不触碰 |
### 0.4 ADR 判定（《收口路线》"不需要新 ADR"结论的逐项核实）
| # | 上游建议 | 判定 | 依据 |
|---|---|---|---|
| 1 | 安装入口收口 | 无需 ADR | 纯 packaging，无契约面 |
| 2 | `packetsage chat` spawn `packetsage-agent`（而非裸 `python -m`） | 无需 ADR；**取代**【M3~M6v0.2§4.1】spawn 目标表述（该节为【设计】非冻结契约） | §2.3；含 fallback 链 |
| 3 | stderr 进度（墙钟） | 无需 ADR | 呈现层；ADR-015 边界显式声明（§3.3） |
| 4 | version 规范化 | 无需 ADR | 增量展示；schema_version 单一事实源（§5） |
| 5 | shell 补全 | 无需 ADR | 增量子命令 |
| 6 | 配置发现链 | 无需 ADR | §24 已定优先级；本规格补文件路径/错误处置（§7） |
| 7 | db 护栏（query --readonly / migrate 确认） | 无需 ADR | §22.5 已要求 readonly；migrate 为新命令 → **同步点 S2** |
| 8 | 错误信息人话化 | 无需 ADR | 仅 Display；类型与退出码零变更 |
| 9 | doctor 修复提示升级 | 无需 ADR | §6.4"失败提示含"列已含安装命令/env 变量名；提前落地 + 十项口径定稿（**待验证 #27**） |
| 10 | REPL（Ctrl-C 语义） | 无需 ADR；若落库新增 `agent_runs.status='interrupted'` → **同步点 S3** | §10；【待验证 #28】 |
**结论：上游"全是 M6 §6.3/§6.4 提前兑现、不影响冻结契约、不需要新 ADR"的判断成立**；但连带产生 **4 个文档同步点**（0.5），不随同步提交则文档溯源链条断裂——这是本规格对上游结论的唯一修正性补充。
### 0.5 文档同步点（随本规格实施提交）
| S# | 内容 | 目标 |
|---|---|---|
| S1 | pyproject 增 `[project.scripts]`：`packetsage-agent = "packetsage_agent.cli:main"` | 【M3~M6v0.2§2.2】增补 |
| S2 | `packetsage db` 子命令（建议 §22.7）+ `version`/`completions`（建议 §22.8）+ launcher 解析顺序与 `PACKETSAGE_AGENT_BIN`/`PACKETSAGE_CONFIG` 环境变量名 | 《开发文档》§22 增补（建议出 v0.4 修订页） |
| S3 | `agent_runs.status` 增 `interrupted` 枚举值（视 #28 实测结论） | 《开发文档》§19 + 【M3~M6v0.2§3.7】（Rust 侧枚举/建表同源） |
| S4 | §4.1 chat spawn 目标表述 | 【M3~M6v0.2§4.1】由本规格 §2.3 取代 |
---
## 1. 继承与现状基线
- 已交付：M3 后端（analyze/serve/落库/查询可跑）。本规格一切 CLI 改造以现有 clap 命令树【继承 dev doc §22】为底，**不重排命令名、不改位置参数语义**；
- binary 名 `packetsage`【事实】dev doc §0/§22；Python 入口现况 `python -m packetsage_agent`【事实】M3~M6v0.2§2.1。
---
## 2. 入口收口【修订-收口路线§1】
### 2.1 Rust binary
```bash
cargo install --path crates/packetsage-cli
```
- 验收：任意目录 `packetsage version` 有响应（§5 格式）；
- CI release tarball 提前到本批次（§12），`cargo install` 路径与 tarball 路径产出的 `--version` 输出**字段一致**（仅 git_hash 可为 `unknown`）。
### 2.2 Python console_script
```toml
[project.scripts]
packetsage-agent = "packetsage_agent.cli:main"
```
- `pip install -e ./agent` 后 `packetsage-agent --help` 必须可用；
- **一致性约束**：两侧入口以**同一 prog 名**呈现（argparse `prog="packetsage-agent"` 或等价机制，禁止回退 `__main__`），且走同一 `main()`——测试断言两者 `--help` 输出与各场景 exit code 逐字节一致（同步点 S1；§14.1 help 快照行 T8）。
### 2.3 chat launcher 解析顺序【设计；取代 M3~M6v0.2§4.1 的 spawn 目标表述】
```text
1. $PACKETSAGE_AGENT_BIN              （显式覆盖；兼作 §14.1 launcher 测试注入 fake agent 的挂点）
2. PATH 中的 packetsage-agent        （spawn --version 探测，3s 超时；阈值定稿见 #37）
3. ./agent/.venv/bin/packetsage-agent （仅当 CWD 为仓库根时的开发布局；对安装用户通常未命中，无害）
4. python -m packetsage_agent        （fallback；stderr WARN 一行：提示 pip install -e ./agent）
全部失败 → exit 3，stderr 给出上述四条路径的排查提示
```
- Windows：console_script 为 `packetsage-agent.exe`，探测用 `where`【待验证 #29，best-effort】；
- launcher 本身零 agent 逻辑【继承-M3~M6v0.2§4.1 "Rust 不实现任何 agent 逻辑"】。
---
## 3. 全局参数与三流分离
### 3.1 全局旗标（全部子命令一致；clap global = true）
| 旗标 | 语义 |
|---|---|
| `--config <path>` | 显式配置（§7） |
| `-v` / `-vv` | stderr 日志级别 info / debug；**默认 warn**（`-vv` 含配置 dump，脱敏同 §7.2） |
| `-q` | 静默：关进度与 info 日志；**错误与 WARN 仍走 stderr** |
| 位置参数在前 | clap 默认；每个子命令 help 顶部给一行示例 |
- **help 即文档**【修订-收口路线§2】：每个命令与参数写 doc-comment，`--help` 快照入测试（改动 help 文本必须有意刷新快照）。
### 3.2 三流分离契约【设计；serve 正确性约束的 CLI 延伸】
| 流 | 内容 | 禁止 |
|---|---|---|
| stdout | 人读结果：summary 表格、db query 表格、doctor ✅/❌ 表、version、completions 脚本；`--jsonl` = **该子命令的纯机器流**：analyze → EngineEvent JSON 行（schema_version=2），db query → 行对象 JSON（§8.1）。**适用面仅此两者**——serve 本身即 JSONL 流，version/completions 为静态输出，均不得声明 `--jsonl`【修订-自审评B4/C3】 | 任何日志、进度、ANSI 色码（--jsonl 强制无色） |
| stderr | 日志、进度（§3.3）、人类可读错误（§9）、WARN、修复提示 | EngineEvent JSON 行（防双向污染） |
| 退出码 | §4 矩阵 | — |
- 非 tty stdout 自动去色；`NO_COLOR` 环境变量【事实】no-color.org：值非空即全局去色；
- 测试：`analyze --jsonl` 的 stdout 逐行 `json.loads` 可解析且 `schema_version=2`；piped stdout 无 ANSI 转义字节（断言）；stderr 不出现完整 EngineEvent 行（断言）。
### 3.3 进度反馈【修订-收口路线§2】
- 形态（stderr 单行 `\r` 刷新；非 tty stderr 降为每 5s 一行）：
```text
analyze: phase=parse packets=120000 bytes=84.3MiB speed=41.2k pkt/s elapsed=2.9s
```
- 节拍【设计】：tty 每 200ms 或每 100_000 包（先到者）；`-q` 关闭；结束时输出终行 `done: N packets in T s`；
- **墙钟边界声明（ADR-015 不破）**：进度计时只用于呈现与 I/O 节拍（先例：M3~M6v0.2§3.7 刷盘 200ms），**不进入任何业务判定**——不参与窗口、冷却、flush、超时、预算；
- 实现路径【设计，零侵入优先】：cli 侧定时采样 **analyze 进程内 pipeline 的 Stats** 既有计数器（serve 为长驻服务、无进度需求，不涉及 AnalysisStore）；仅当现有计数缺 phase/offset 字段时，才允许在 pipeline 增加最小进度回调钩子（原子计数器，无锁、无 channel 背压）【待验证 #30】；
- **确定性守护【设计；修订-自审评C9】**：进度实现不得影响事件流——同一输入下"开进度"与"`-q` 关进度"两种方式的 `--jsonl` stdout 输出**逐字节一致**（§14.1 三流分离行 T7）。
---
## 4. exit code 核对矩阵（验证活动，不改码）
逐命令逐场景脚本化断言（`echo $?`），归档 `tests/cli/exit_codes.py` 并入 CI：
| 命令 | 场景 | 期望 |
|---|---|---|
| analyze | 正常（含 --jsonl） | 0 |
| analyze | 魔数不识别/结构损坏 | 2 |
| analyze | 配置不可解析/未知键；显式指定（`--config`/环境层）的文件缺失【修订-自审评C1 措辞对齐】 | 3 |
| analyze | 未知旗标/缺位置参数 | 1 |
| analyze --full | Agent 阶段失败（降级路径） | 0 + stderr WARN【继承-M3~M6v0.2§5.5】 |
| analyze --full | lint 硬失败 | 4，stderr 含 `anti-hallucination`【继承 C9 拆注】 |
| serve | 启动失败（配置/DB/源文件缺失） | 3 |
| rules check | 规则非法（严格加载） | 3（RuleError 归配置类；以 M3 实际实现回填，**待验证 #33**） |
| doctor | 任一 ❌ | 3【继承-M3~M6v0.2§6.4】 |
| db query --readonly | 正常【修订-自审评C2】 | 0 |
| db query --readonly | 连接失败 | 3 |
| db query --readonly | url 未含 `mode=ro` / 非 SELECT 预检拒绝【修订-自审评C2】 | 1 |
| db migrate | 正常（确认执行）【修订-自审评C2】 | 0 |
| db migrate | 非交互且无 `--yes` | 1（fail-closed） |
| db migrate | 交互拒绝 | 0 + stderr `migrate aborted by user` |
| query alerts / rules list | 正常【修订-自审评C2】 | 0 |
| query alerts | 库不存在/连接失败【修订-自审评C2】 | 3 |
| chat | 启动时 provider 不可用【修订-自审评C2】 | 3 + 提示运行 doctor |
| chat | launcher 四级解析全失败（§2.3）【修订-自审评C2】 | 3 |
| chat | Ctrl-D 正常退出 | 0 |
| version / completions | 正常 | 0 |
| 任意 | panic（不该发生） | 4 + stderr panic 摘要（策略对齐 M2 工程规范，**待验证 #32**） |
---
## 5. version 规范化【修订-收口路线§3】
```text
$ packetsage version        # 与 --version 等价（单行）
packetsage 0.1.0 (schema_version=2, git=1a2b3c4d, built=2026-09-20T03:40:00Z, profile=release)

$ packetsage version --json
{"version":"0.1.0","schema_version":2,"git_hash":"1a2b3c4d","build_time":"2026-09-20T03:40:00Z","profile":"release","rustc":"1.84.1","target":"x86_64-unknown-linux-gnu"}
```
- **单一事实源**：schema_version 运行时读 `packetsage_protocol::SCHEMA_VERSION`；CI 断言 `version --json` 的值与常量一致（防未来 bump 忘改展示层）；
- 构建元数据：build.rs 注入（cli crate 私有，见 §13）；脏工作区 git_hash 加 `-dirty` 后缀；`built` 尊重 `SOURCE_DATE_EPOCH`【事实】reproducible-builds.org；
- 无 git 信息（tarball 构建）→ 字段输出 `unknown`，不报错；
- **单行展示字段与 `--json` 键的映射固定**：`git`↔`git_hash`、`built`↔`build_time`（其余同名）【修订-自审评C6】；`--json` 键集合为稳定 **json 输出格式 v1**——该格式版本与 EngineEvent `schema_version` 无关（避免命名混淆，doctor `--json` 同此口径），变更须 bump 并记入文档。
---
## 6. shell 补全【修订-收口路线§3】
- `packetsage completions <bash|zsh|fish|powershell>` → stdout 输出脚本（clap_complete【事实】）；
- 幂等（同参数两次输出逐字节一致，测试断言）；`--help` 列全支持的 shell；
- README 给 bash/zsh 各一行安装示例。
---
## 7. 配置发现链与脱敏【设计；细化 dev doc §24】
### 7.1 发现链
```text
1. --config <path>            （显式）
2. $PACKETSAGE_CONFIG
3. ./packetsage.yaml          （工作目录）
4. 内置默认                    （不告警）
```
- **不引入 `~/.config` 隐式路径**【设计】：离线分析工具不应有隐藏全局状态；需要"全局配置"的用户可自行 `export PACKETSAGE_CONFIG`；
- 显式指定（层 1/2）的文件不存在或不可解析 → exit 3，stderr 给出绝对路径与原因；
- 层 3 文件**存在但不可解析/含未知键** → 同样 exit 3——被发现即视为用户的配置，无"静默跳过"；仅当层 3 **不存在**时才落到层 4 且不告警【修订-自审评C1】；
- **两条链的关系【设计】**：本链是"用哪个文件"（文件发现链）；dev doc §24 的 `CLI > Environment > config.yaml > defaults` 是"单个键取哪个值"（键值优先级链）。`--config` 只选择文件，不构成键级覆盖；键级覆盖仅发生在 CLI 旗标（现有命令无此类旗标）与环境变量两层，Rust/Python 两侧行为一致并有测试（§14.1 配置行）【修订-自审评C1】；
- `.env` 仅由 Python 侧加载（§24 所列支持）；Rust 侧**不**自动加载 .env，只读进程环境变量——避免双 dotenv 行为漂移（与 #31 呼应）【修订-自审评C1】；
- **未知键 → 报错 exit 3**（deny_unknown_fields），提示逐字段路径——配置是契约，静默忽略拼错的键（如 `max_stesp`）是经典事故源。与规则 YAML 的宽松模式（dead-letter）**有意不同**：规则面向外部作者，配置面向部署者；
- config 文件中发现 `api_key` 类键 → 报错并提示移至环境变量/.env（§24 "API Key 不进入 git" 的执行化）；
- 两侧读同一发现链：Rust = engine/rules/storage 节；Python = agent/llm 节；launcher 以 `PACKETSAGE_CONFIG` 环境变量传递路径给子进程（env 继承，不新增 flag）【待验证 #31：与 Python 侧 .env 自动加载的相互作用】；
- Environment 层每键映射【设计】：
```text
PACKETSAGE_STORAGE_URL / PACKETSAGE_RULES_PATH / PACKETSAGE_LLM_PROVIDER / PACKETSAGE_LLM_MODEL / PACKETSAGE_LLM_API_KEY（Python 侧消费）
```
### 7.2 脱敏
- **键名匹配规则【设计；修订-自审评B1】**：键名按分隔符（`.`/`_`）**分段后整段匹配**——段集恰为 {api, key}（即 `api_key`）、或恰为 {secret}、{token}、{tokens}、{password}、或末两段组合 `*_api_key`/`*_secret`/`*_password` → 值一律 `***`（不回显长度）；**子串匹配被明确禁止**（会误伤 `max_tokens_total` 等合法键，负向用例 T1）；环境变量**名**可回显【继承-M3~M6v0.2§6.4 "env 变量名"】。适用面：doctor 生效配置展示、`-vv` 配置 dump。
---
## 8. db 子命令护栏【修订-收口路线§3；细化 dev doc §22.5】
### 8.1 `packetsage db query --readonly --sql "SELECT ..."`（--sql 必填，无交互式 SQL 输入）
- SQLite【事实】：连接串强制 `?mode=ro`——不含 `mode=ro` 的 url 直接拒绝执行（exit 1）；
- PostgreSQL（M6 matrix 打开后）：会话级 `default_transaction_read_only=on`；
- 客户端预检：非 `SELECT/WITH/EXPLAIN` 前缀 → 拒绝（exit 1）——**这只是 UX 预检，强制力在只读连接**；
- `--limit N`（默认 200，上限 10000）外包一层 SELECT；`--timeout`（默认 10s）**语义 = 连接建立与锁等待超时**（SQLite `busy_timeout`）——sqlx/SQLite 无查询级中断能力（#36），长查询防护靠 `--limit`；若 #36 证实可中断再升级语义【修订-自审评C5】；
- 输出：stdout 表格；`--jsonl` 时行流（行对象 JSON，§3.2 适用面内）。
### 8.2 `packetsage db migrate [--yes]`
- 执行前打印：当前版本 → 目标版本 + pending 列表（sqlx migrate 版本比对【继承-M3~M6v0.2§6.4】）；
- 非 tty 且无 `--yes` → 拒绝执行，exit 1（fail-closed）；交互拒绝 → exit 0 + stderr `migrate aborted by user`；
- SQLite：执行前以短 busy_timeout 连接做 `BEGIN IMMEDIATE` 探测——**best-effort 概率性警示**（SQLite 锁是事务级存在，探测存在 TOCTOU 窗口），被占 → WARN 提示可能有 serve 在跑（ADR-018 单写方）【修订-自审评C5 措辞降级】；
- 打印一行备份提示（SQLite：先 `cp packetsage.db packetsage.db.bak`）——**不做自动备份**（避免隐式副本漂移），提示为准；
- migrate 过程输出走 stderr，结果行走 stdout。
---
## 9. 错误信息人话化【修订-收口路线§3】
Display 层改造；**类型与退出码零变更**【继承 M2v0.2§5 / dev doc §25】。**本表只覆盖 CLI 顶层 Fatal 呈现路径**（表外归属见表后注）【修订-自审评C8】。全部模板含：路径或标识 + 原因 + **建议动作**：
| 错误 | stderr 模板要点 |
|---|---|
| UnsupportedCapture | `<path>: 文件前 16 字节 <hex> 不匹配任何已知魔数——确认是 pcap/pcapng？` |
| CaptureCorrupted | `<path>: offset=<n> 处结构损坏（<原因>）；可先用 analyze --jsonl 定位首个坏包` |
| CAPTURE_UNAVAILABLE | `task <id> 的源文件 <path> 已不可读（冷恢复要求源文件在原路径）`【继承 ADR-019】 |
| DatabaseError | `<url>: <原因>；若为 migration 缺失 → packetsage db migrate` |
| LlmError | provider/model + env 变量名（不回显 key） |
| RuleError | 规则 id + 违反的静态校验条目（S1–S9） |
- **表外错误归属**：`DecodeError` 是事件流成员（Event 非 Fatal，按 §25.1 单包容错——只进 summary 计数与 `--jsonl` 流）；`ToolError` 走 Agent 降级路径【继承-M3~M6v0.2§5.5，报告第 6 节】；`InvalidArgument` 由 clap usage 呈现（exit 1）；`Internal` 由下方 panic 兜底覆盖【修订-自审评C8】；
- `-v` 追加完整错误链（source chain）与 Debug 表示；
- panic 兜底：release 捕获后 stderr 输出"internal error (panic)，请附 `-vv` 日志提 issue" → exit 4【待验证 #32】。
---
## 10. REPL（chat）体验【修订-收口路线§3】
> **同步点 S7【修订】**：本节细则（提示符/横幅/Ctrl-C 语义/斜杠命令/usage 行/非 tty 拒绝）已**收编**至《Agent CLI 工程规格书 v0.1》§5，权威定义在彼处；本节保留 launcher 责任与 UX 验收判据。
- prompt：`packetsage·<task_id 前 8 位>> `；启动横幅一行 task 概要（包数/告警数/规则数）；
- 历史：Python 侧 stdlib `readline`（POSIX；Windows best-effort）——**不新增 prompt_toolkit 依赖**；
- Ctrl-C 语义【设计，最终以 #28 实测定稿】：
  - 空闲 prompt：显示"Ctrl-D 或 /quit 退出"，不直接退出；
  - LLM 轮中：请求取消当前轮；成功 → 该轮 `agent_runs.status='interrupted'`（同步点 S3），token/预算照记；
  - provider 不支持干净取消 → 完成本轮再回 prompt + WARN（**会话不炸优先于即时中断**）；
- 错误不炸会话：轮内任何错误按 §9 模板打印一行后回到 prompt；连续失败 ≥3 次 → 提示运行 `packetsage doctor`；
- 内置命令：`/help`、`/quit`（等价 Ctrl-D）；不新增其他斜杠命令（`--report` 已由 CLI 旗标覆盖【继承-M3~M6v0.2§5.5】）。
---
## 11. doctor 升级【修订-收口路线§4；落地 M3~M6v0.2§6.4】
### 11.1 检查项定稿（10 项；解决 §6.4 表 8 项 vs §6.6 "十项" 口径差 → 待验证 #27）
| # | 检查项 | 失败提示必含 |
|---|---|---|
| 1 | binary 版本 / toolchain | — |
| 2 | 配置文件解析（含发现链来源展示） | 逐字段路径 |
| 3 | rules 目录——**宽松加载**：dead-letter 列示计 WARN，不置 ❌（严格校验归 rules check）【修订-自审评C4】 | dead-letter id 列表 |
| 4 | samples 可读 | path |
| 5 | DB 连接 + migration 状态——**mode=ro 无副作用探测**：库不存在 ≠ ❌（提示 migrate），连接失败才是 ❌【修订-自审评C4】 | 当前/期望版本 或 migrate 提示 |
| 6 | 输出/DB 目录可写性（【设计】新增；探测临时文件用后即删） | 目录 path + 修复动作（mkdir/chmod） |
| 7 | Python 环境 | 安装命令 |
| 8 | LLM provider——配置存在性检查；**可达性探测仅在未 `--no-net` 时执行**【修订-自审评C4】 | env 变量名 |
| 8a | **未配置 provider = ⚠ + 提示 `packetsage-agent setup`**（实施期修订）：默认 provider 不再是 `mock`；`mock` 仅显式指定时生效（CI/演示/E 套件），详见《Agent CLI 工程规格书 v0.1》§14 | setup 命令 + env 变量名 |
| 9 | 端到端自检：spawn 自身 serve 完成 ping RPC（v0.2 更名弃用"出海自检"歧义名；**本地无外网依赖**，外网可达性归第 8 项）【修订-自审评C4】 | stderr/stdout 隔离验证 |
| 10 | `version --json` 与 SCHEMA_VERSION 一致（【设计】新增，防双源） | 两侧取值 |
- 每行：✅/❌/⏭（`--no-net` 跳过标注）+ 检查名 + 摘要 + **修复提示**（缺 Python → `pip install -e ./agent`；缺 rules → 指向 builtin 目录来源等）；
- `--no-net` 跳过集合【设计；修订-自审评C4】：仅第 8 项的可达性探测（配置存在性检查保留）；第 9 项为本地 spawn，不在跳过集合；
- 任一 ❌ → exit 3【继承-M3~M6v0.2§6.4】；
- `doctor --json`【设计，新增】：稳定 **json 输出格式 v1**（命名口径同 §5，与 EngineEvent schema_version 无关）——`{items:[{id,name,status,detail,repair_hint}]}`，脱敏同 §7.2；README/CI 用它做冒烟断言。
---
## 12. 分发与文档【修订-收口路线§4】
- README 顶部 **Quickstart 三行（源码路径，判据基准）**：`cargo install --path crates/packetsage-cli && pip install -e ./agent` → `packetsage analyze samples/xx.pcap` → `packetsage chat`，可复制粘贴跑通即合格；
- **双路径能力矩阵【修订-自审评B3】**：①源码路径 = 全能力；②tarball 路径 = analyze / rules / query / db / doctor / version / completions（**不含 chat**——tarball 不带 Python 侧），tarball 内附 `INSTALL.md` 写明能力矩阵与补装方法（`pip install -e ./agent`）；
- CI tag 触发 release tarball：`packetsage-<ver>-<os>-<arch>.tar.gz`（binary + `rules/builtin/` + `SHA256SUMS` + `INSTALL.md`）；即 §6.3 内容提前，Windows best-effort 不变；
- **10 分钟判据**（验收活动）：未读规格者凭 `--help` + README 在 10 分钟内独立完成 install → analyze 样例 → 读懂 summary；步骤记录归档 `docs/ux-walkthrough.md`。
---
## 13. 依赖增量（仅 cli 层 / pyproject）
```toml
# crates/packetsage-cli：clap_complete = "4"（补全）；版本元数据走 build.rs（vergen 系或手写 env 注入）
# agent/pyproject.toml：无新增（readline 为 stdlib）
```
遵守依赖分层【继承-M3~M6v0.2§2.3】：新增依赖不进 core/protocol/rules/storage。
---
## 14. 测试与 DoD
### 14.1 测试矩阵（对齐 dev doc §26 风格）
| 层 | 用例（T# 对应自审评 T1–T10） |
|---|---|
| 三流分离 | --jsonl stdout 逐行可解析 + 无 ANSI；piped 无色；进度只出现在 stderr；**进度开/关 A/B：--jsonl 输出逐字节一致（T7）** |
| exit code | §4 矩阵脚本化全绿含新增 7 行（T10；含 `anti-hallucination` 关键字断言） |
| 配置 | 文件发现链 × 键值优先级**双链**用例（Rust/Python 双侧，T5）；层 3 损坏 exit 3；未知键 exit 3；api_key 键报错；**脱敏正负向：`max_tokens_total` 不掩码 / `llm.api_key` 掩码（T1）** |
| db | ro 连接写操作被拒断言；migrate 三态（确认/拒绝/非交互）；url 缺 mode=ro → exit 1（T9）；--jsonl 行流逐行可解析（T4） |
| version | --json 键集稳定（json 输出格式 v1）；SCHEMA_VERSION 一致性断言 |
| completions | 4 shell 幂等（两次输出逐字节一致） |
| launcher | 四级解析 + `PACKETSAGE_AGENT_BIN` 注入 fake agent（T2）+ fallback WARN + 全失败 exit 3 |
| help 快照 | console_script 与 python -m 双形态 `--help` 逐字节一致（T8） |
| REPL | 空闲 Ctrl-C / 轮中 Ctrl-C（mock provider）/ 连错不炸（fake engine） |
| doctor | 10 项各有触发用例；**无副作用断言：运行前后目录清单一致（T6）**；--json 格式快照；脱敏断言 |
| 分发 | tarball 干净容器安装冒烟（#34）；tarball 路径 chat 失败提示断言 + INSTALL.md 存在性（T3） |
### 14.2 M6a DoD
- [ ] 任意目录 `packetsage version` 与 `packetsage-agent --help` 可用
- [ ] §4 exit code 矩阵脚本化全绿并入 CI
- [ ] --jsonl 纯净性 + 进度隔离测试全绿；1GB 样例进度可感知（人工项）
- [ ] 配置发现链/未知键/脱敏用例全绿
- [ ] db query --readonly 写操作被拒；migrate 三态行为符合 §8.2
- [ ] 错误模板覆盖 §9 全表并有快照测试
- [ ] completions 4 shell 幂等
- [ ] doctor 十项 + --json + 修复提示（含一台无外网机器 provider=mock 全过——M6 §6.6 的提前演练）
- [ ] README quickstart 三行（源码路径）可复制跑通；10 分钟判据 walkthrough 归档
- [ ] tarball 双路径能力矩阵落地（INSTALL.md + tarball 路径 chat 失败提示断言，T3）
- [ ] launcher `PACKETSAGE_AGENT_BIN` 注入测试绿（T2）；进度 A/B 确定性守护绿（T7）；脱敏正负向绿（T1）
- [ ] 文档同步点 S1–S4 提交（S3 视 #28 结论）
---
## 15. 实现顺序 Checklist（C1–C10，可插队执行）
```text
C1 入口：cargo install 路径 + console_script + version 规范化（半天）
C2 三流分离 + 进度 + -q + NO_COLOR（半天）
C3 exit code 矩阵脚本化核对（半天，可与 C2 并行）
C4 配置发现链 + doctor 生效配置展示（脱敏）（半天）
C5 db 护栏：query --readonly + migrate 三态（半天）
C6 错误人话化映射表 + -v 全链（半天）
C7 completions（1～2 小时）
C8 REPL 打磨（#28 实测后定稿 Ctrl-C）（余力）
C9 doctor 十项定稿 + 修复提示 + --json（半天）
C10 README quickstart + tarball + SHA256SUMS + walkthrough（半天）
```
节奏对齐《收口路线》"建议顺序"1–4；C8 排最后，不阻塞 M6a 验收。
---
## 16. 待验证项（接续 M3~M6v0.2 §11 编号）
| # | 项 | 时点 |
|---|---|---|
| 27 | doctor 检查项口径定稿（M3~M6v0.2§6.4 表 8 项 vs §6.6"十项"；本规格 §11.1 提出 10 项权威清单，M6 评审确认；第 9 项已更名"端到端自检"弃用"出海自检"歧义名）【修订-自审评C4 扩径】 | C9 前 |
| 28 | provider/langchain 轮级干净取消能力（Ctrl-C 语义 + `interrupted` 状态是否落库，同步点 S3；含取消轮的 usage 记账精度——流式 usage 尾包可能丢失） | C8 前 |
| 29 | Windows console_script 探测（where/PATHEXT）与 launcher fallback | C1（best-effort） |
| 30 | 现有 pipeline Stats 计数器对进度字段（phase/offset/bytes）的覆盖度；是否需要最小回调钩子 | C2 前 |
| 31 | PACKETSAGE_CONFIG 经 env 传递给 Python 子进程与 .env 自动加载的相互作用（双 dotenv 漂移风险；Rust 不加载 .env 已定稿 §7.1） | C4 |
| 32 | release panic 策略（unwind+catch→exit 4 vs abort）与 M2 工程规范既有约定的对齐 | C6 |
| 33 | rules check/loader 退出码在 M3 实现中的实际值，回填 §4 矩阵 | C3 |
| 34 | tarball 在干净容器的 install-from-scratch 提前演练结果记录（M6 §6.3 job 的前置验证） | C10 |
| 35 | M3 已实现中实际读取的环境变量名与 §7.1 `PACKETSAGE_*` 新命名及 dev doc §24 的对齐——**实现既有名优先**，S2 修订时统一【修订-自审评】 | C4 前 |
| 36 | sqlx/SQLite 查询级中断能力（rusqlite `interrupt_handle` 或等价物）→ 决定 db query `--timeout` 是否升级语义【修订-自审评C5】 | C5 前 |
| 37 | 冷启动/AV 环境下 spawn+`--version` 探测耗时分布 → launcher 探测超时（暂定 3s）定稿【修订-自审评C10】 | C1 |
---
## 附：与《收口路线》及既有契约的衔接自查
| 收口路线条目 | 本规格落点 | 判定差异 |
|---|---|---|
| 入口（cargo install + console_script） | §2 | 一致；补 fallback 链与 Windows best-effort |
| 参数一致性 | §3.1 | 一致 |
| help 文本（doc-comment 即文档） | §3.1/§14 | 补 help 快照测试 |
| 输出分离 | §3.2 | 一致；补 --jsonl 强制无色与双向污染禁令 |
| 进度反馈 | §3.3 | 一致；补墙钟边界声明（ADR-015）与零侵入实现路径 |
| exit code 核对 | §4 | 一致 |
| version 规范化 | §5 | 一致；补单一事实源断言 |
| 补全 | §6 | 一致 |
| REPL | §10 | 一致；Ctrl-C 语义细化 + #28 |
| db 护栏 | §8 | 一致；补三态语义与写锁探测 |
| 错误人话化 | §9 | 一致；补 -v 全链与 panic 兜底 |
| doctor 修复提示 | §11 | 一致；补十项口径定稿（#27）与 --json |
| README/tarball | §12 | 一致；补 SHA256SUMS 与 10 分钟判据 walkthrough |
| "不用动引擎任何代码" | §3.3 | **修正**：不动引擎语义；进度采样若现有计数不足，允许最小回调钩子（#30） |
| "不需要新 ADR" | §0.4 | **成立**，但附 4 个文档同步点（S1–S4），须随实施提交 |
