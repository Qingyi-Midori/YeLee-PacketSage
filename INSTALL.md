# PacketSage 安装与能力矩阵

> 本文件随 release tarball 一起分发（CLI 收口工程规格书 §12 / B3），也随源码仓库存在。

## 1. 两种安装路径

### ① 源码路径（全能力）

```bash
cargo install --path crates/packetsage-cli
pip install -e ./agent
```

装好之后，任意目录都能直接用 `packetsage` 与 `packetsage-agent` 两个命令。

> Windows 上若 `cargo build` 报 `dlltool.exe` 找不到，先用
> `powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release` 构建，
> 或用 `--root` 把 `cargo install` 装到本地目录：
> `cargo install --path crates/packetsage-cli --root <某目录>`。
> 另外确认 `%USERPROFILE%\.cargo\bin` 与 Python 的 `Scripts` 目录在 `PATH` 里，
> 否则要写全路径才能调用 `packetsage` / `packetsage-agent`。

### ② tarball 路径（不含 Python Agent）

```bash
tar -xzf packetsage-<version>-<os>-<arch>.tar.gz
cd packetsage-<version>-<os>-<arch>
./packetsage version                    # 直接用，无需安装
./packetsage analyze samples/xx.pcap    # 需要自备 pcap/pcapng
```

tarball 里只有 Rust 引擎。想要 `chat` / `--full`（Agent 阶段），补装 Python 侧：

```bash
pip install -e ./agent      # 在源码仓库根目录执行
```

## 2. 能力矩阵

| 能力 | 源码路径 | tarball 路径 |
|---|---|---|
| `packetsage analyze`（含 `--jsonl`） | ✅ | ✅ |
| `packetsage rules list / check` | ✅ | ✅ |
| `packetsage query ...` | ✅ | ✅ |
| `packetsage db query --readonly` / `db migrate` | ✅ | ✅ |
| `packetsage doctor` / `version` / `completions` | ✅ | ✅ |
| `packetsage serve`（JSONL RPC worker） | ✅ | ✅ |
| `packetsage chat` | ✅ | ❌（需 `pip install -e ./agent`） |
| `packetsage analyze --full --report` | ✅ | ❌（同上） |

tarball 路径上执行 `packetsage chat` 会以 exit 3 结束，并在 stderr 列出四级解析链
（`$PACKETSAGE_AGENT_BIN` → PATH 中的 `packetsage-agent` → `agent/.venv` →
`python -m packetsage_agent`）与补装命令。

## 3. 校验

```bash
sha256sum -c SHA256SUMS      # macOS: shasum -a 256 -c SHA256SUMS
```

想在本机提前把两条路径都走一遍（含能力矩阵与 `chat` 失败提示），跑
`python scripts/install_smoke.py`：它会在 `install-test/` 里分别做一次
源码安装与 tarball 解包，并写出 `install-test/RESULTS.md`。该目录已被
`.gitignore` 忽略，可随时删除重建。

想把装好的 CLI **真跑一遍并留档**：

```bash
python scripts/install_smoke.py --run-cli      # 安装 + 运行，生成 RESULTS.md 与 RUN.md
python scripts/cli_demo.py --from-install install-test   # 只重跑运行部分
```

`RUN.md` 是逐条命令的转录（命令、stdout/stderr、退出码），15 步全部核对预期退出码。

## 4. 怎么运行（第一条命令是配置）

装完有两个命令：`packetsage`（Rust 引擎 + CLI）与 `packetsage-agent`（Python Agent）。
不在 `PATH` 里时用全路径，或把 `python -m packetsage_agent` 当作 `packetsage-agent`（§5）。

```bash
# 0) 第一步：配置 provider 与 API key（写进 agent/.env，gitignored；不进 config、不进 git）
packetsage-agent setup                     # 交互：选 provider → 填 model/base_url → 贴 key → 校验
packetsage-agent setup --provider deepseek --model deepseek-flash --api-key sk-... --no-verify
packetsage-agent setup --provider deepseek --api-key sk-... --print    # 只看要写入的内容（key 打码）

# 0.5) 自检：装没装对、引擎能不能跑、provider 生效没有（10 项，--json 给脚本用）
packetsage doctor

# 1) 只看引擎：静态分析 + 结构化查询，不需要 LLM、不联网
packetsage analyze samples/synth-mixed.pcap                        # 人读摘要
packetsage analyze samples/synth-mixed.pcap --jsonl                # 纯事件流（stdout）
packetsage analyze samples/synth-mixed.pcap --db sqlite://packetsage.db   # 顺便落库
packetsage query alerts --db sqlite://packetsage.db                # 从库里查告警
packetsage rules list                                              # 规则清单

# 2) 交互式 Agent（必须有真终端：stdin 与 stdout 都是 tty）
packetsage chat samples/synth-mixed.pcap
packetsage chat samples/synth-mixed.pcap --report reports/chat.md  # 会话结束后出报告

# 3) Agent 命令面（批式调查 / 报告 / 会话）—— task 必须已经存在
packetsage-agent run    --task-id <task_…> [--engine "<packetsage 全路径> serve"] [--db sqlite://packetsage.db]
packetsage-agent report --task-id <task_…> [--report reports/x.md] [--db …]
packetsage-agent chat   --task-id <task_…> [--report reports/x.md] [--db …]
packetsage-agent --version | --help          # 全局旗标：--config / -v / -vv / -q

# 4) 一条命令跑完全链：解析 → 规则 → Agent → 报告
packetsage analyze samples/synth-mixed.pcap --full --report reports/demo.md --db sqlite://packetsage.db
```

三条容易踩的边界：

* **没配置就不跑**：`run` / `chat` / `analyze --full` 在 provider 未配置时直接 exit 3 并提示
  `packetsage-agent setup`；`mock`（确定性脚本回放，CI/演示用）只有显式写
  `--provider mock` 或 `PACKETSAGE_LLM_PROVIDER=mock` 才生效，永远不会"悄悄用假结果"。
  `doctor` 第 8 项在未配置时是 ⚠（不阻塞 exit 0），配置好之后才是 ✅。
  支持的 provider：`mock | openai | deepseek | local`（deepseek 走 OpenAI 兼容协议，
  默认 `https://api.deepseek.com/v1` + `deepseek-flash`；`deepseek-chat` / `deepseek-reasoner`
  已于 2026-09 下线，现役是 `deepseek-flash` 与 `deepseek-v4-pro`，都支持思考模式）。

* **引擎路径**：`packetsage chat` 与 `analyze --full` 由 Rust launcher 自己把引擎路径传给
  Agent（`$PACKETSAGE_ENGINE`），不用你管；但**手工直调** `packetsage-agent …` 时它默认执行
  `packetsage serve`，所以要么 `packetsage` 在 `PATH` 上，要么用 `--engine "<全路径> serve"`
  或先设 `PACKETSAGE_ENGINE`。否则 exit 3 并提示"check that the engine is installed and on PATH"。
* **task 从哪来**：Agent 自己不连库也不分析文件，`--task-id` 指向的任务由
  `packetsage analyze`（或 `--full`）创建；`--db` 只转给引擎，跨进程的任务由 `serve`
  按 ADR-019 从库里冷恢复。查无 task → exit 3，并提示先 `packetsage analyze`。

### 让两个命令随处可用（Windows）

```powershell
# 当前会话
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"                              # packetsage
$env:PATH = "$(python -c "import sysconfig;print(sysconfig.get_path('scripts'))");$env:PATH"   # packetsage-agent

# 永久（用户级，重开终端生效）
[Environment]::SetEnvironmentVariable(
  "PATH",
  "$env:USERPROFILE\.cargo\bin;$(python -c "import sysconfig;print(sysconfig.get_path('scripts'))");$env:PATH",
  "User")

packetsage version && packetsage-agent --version     # 两条都在 PATH 上
```

> **升级注意**：如果本机早就装过一版 `packetsage`，务必重新
> `cargo install --path crates/packetsage-cli`。旧引擎没有 ADR-019 冷恢复，
> `packetsage-agent run --task-id …` 会 exit 3 报 "task … is not in memory"。
> 判断方法：`packetsage version` 的 `built=…` 就是本次构建的 UTC 时间
> （源码有改动即刷新；需要可复现构建时用 `$env:SOURCE_DATE_EPOCH` 固定它）。

LLM 提供方默认 `mock`（确定性、不联网、不需要 key）；换成真实模型：

```powershell
$env:PACKETSAGE_LLM_PROVIDER = "openai"        # 或 local（OpenAI 兼容端点）
$env:PACKETSAGE_LLM_MODEL    = "gpt-4o-mini"
$env:PACKETSAGE_LLM_API_KEY  = "sk-..."        # 也可以写进 agent/.env（不进 git）
```

provider 是 `openai` 却没有 key 时，`chat` / `run` 以 exit 3 结束并提示跑 `packetsage doctor`。

## 5. Windows：双击 vs 终端

`packetsage.exe` 是**控制台程序**：不带子命令运行时它打印 help 后立即退出，所以双击只会
闪一下窗口，看起来像"运行失败"。两种正确用法：

```bat
:: ① 双击可用（打印 ASCII 标题 + 挑最全能力的那个已装版本 + version/analyze，最后 pause）
install-test\RUN-CLI.cmd
install-test\RUN-CLI.cmd analyze samples\synth-mixed.pcap

:: ② 在任何终端里带子命令运行
install-test\tarball\packetsage-3.8.1-windows-x86_64\packetsage.exe analyze samples\synth-mixed.pcap
```

启动器（`scripts\run_cli.cmd`，`install_smoke` 会复制成 `install-test\RUN-CLI.cmd`）的选择顺序是
**源码安装 → `target/release` → `target/debug` → tarball**：tarball 只是发行产物、不含 Python
Agent，只有当本机没有别的构建时才会被选中；它还会探测 Agent 是否可达，只有可达时才在
"Useful commands" 里列出 `chat` / `--full`，否则给出 `pip install -e agent` 的补装提示。
标题直接取自 `crates/packetsage-cli/src/banner.txt`（与二进制内嵌的是同一份，天然一致）。

想直接敲 `packetsage` 而不写全路径，把 `%USERPROFILE%\.cargo\bin` 加进用户 `PATH`
（`cargo install` 会把二进制装在那里），或把 tarball 解包目录加进 `PATH`。

## 6. 规则目录

tarball 自带 `rules/builtin/`（引擎内嵌的四条规则的可编辑副本）：

```bash
./packetsage analyze capture.pcap --rules-dir rules/builtin
```

不指定时引擎直接使用内嵌副本，因此离线单文件也能工作。

## 7. 配置

不对配置文件做任何隐式假设（§7.1），发现链是：

1. `--config <path>`
2. `$PACKETSAGE_CONFIG`
3. `./packetsage.yaml`
4. 内置默认值

未知键、以及写在文件里的 `api_key` 一类凭据都会导致 exit 3 —— 凭据请放
`$PACKETSAGE_LLM_API_KEY` 或 `agent/.env`。
