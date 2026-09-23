# UX walkthrough：10 分钟判据（CLI 收口工程规格书 §12）

> 判据原文：**未读规格者凭 `--help` + README 在 10 分钟内独立完成 install → analyze 样例 → 读懂 summary。**
> 本文件是这条判据的执行记录：命令、实际输出、耗时与踩到的坑。

## 0. 本次记录的环境

| 项 | 值 |
|---|---|
| 平台 | Windows 10/11 x86_64 |
| Rust | `rustc 1.98.1` / host `x86_64-pc-windows-gnu`（`rust-toolchain.toml` 只固定 `stable`） |
| Python | 3.14.3（已装 httpx / jinja2 / pytest / PyYAML / pydantic） |
| 二进制 | `target/debug/packetsage.exe`（release 同流程） |

### 0.1 本机链接器记录（D-12 的实测补充）

GNU 宿主上 `cargo build` 需要 MinGW-w64 的 `dlltool` + 汇编器；本机 MinGW 装在
带空格的路径下（`%LOCALAPPDATA%\Programs\mingw64`），其 gcc 驱动会把 sysroot 路径
按未加引号的形式传给 `ld`，于是链接报 `cannot find C:/Users/Qingyi`。可用的组合是
**二进制目录 + rustup 自带的 self-contained 链接器驱动**：

```powershell
$env:Path = "C:\mingw64\bin;$env:USERPROFILE\.cargo\bin;" + $env:Path   # C:\mingw64 是指向 MinGW 的无空格 junction
$self = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "$self\x86_64-w64-mingw32-gcc.exe"
cargo build --workspace
```

> 这两条环境变量只影响本机构建，不进仓库（换机器/CI 用系统工具链即可）。
>
> 现在已经不需要手敲这一串：`powershell -ExecutionPolicy Bypass -File scripts/build.ps1`
> 会自动完成同样的三步（找 cargo、找 MinGW、指定链接器），
> 加 `-CargoArgs --release` 即 release 构建，`-Command test` / `-Command clippy` 同理。

## 1. 安装（耗时约 3 分钟，含首次编译）

```bash
cargo install --path crates/packetsage-cli
pip install -e ./agent
```

验证（任意目录）：

```console
$ packetsage version
packetsage 0.1.0 (schema_version=2, git=unknown, built=2026-09-20T02:51:44Z, profile=debug)
$ packetsage-agent --help
usage: packetsage-agent [-h] [--config PATH] [-v] [-q] [--version] {run,chat,report,setup} ...
```

两个入口的 `--help` 逐字节一致（`python -m packetsage_agent --help` 与
`packetsage-agent --help` 共用同一个 `prog`），由 `tests/cli/exit_codes.py` 的
"console script and python -m are interchangeable" 用例断言；命令面与退出码见
《Agent CLI 工程规格书 v0.1》与 README §6。

## 1.5 第一步是配置（新装机器必做）

```console
$ packetsage-agent setup
deepseek  openai  local  mock
provider (输入序号或名字) [deepseek]: deepseek
deepseek API key（不回显，留空=跳过）:
provider        deepseek
model           deepseek-chat
base_url        https://api.deepseek.com/v1
api key         *** (新输入)
verify          ✅ https://api.deepseek.com/v1 可达（HTTP 200，2 个模型可见）
written         C:\...\agent\.env

下一步:
  packetsage doctor
  packetsage chat samples/synth-mixed.pcap
```

没配置就跑 `run` / `chat` / `analyze --full` 会以 exit 3 结束并提示上面这条命令；
`mock` 只有显式指定（`--provider mock` / `$PACKETSAGE_LLM_PROVIDER=mock`）才生效，
因此不会出现"没配 key 却在输出分析结论"的情况。

## 2. 自检（耗时约 30 秒）

```console
$ packetsage doctor
packetsage doctor
MARK SUMMARY                    CHECK
✅    engine binary              packetsage 0.1.0 (schema_version=2, git=unknown, built=2026-09-20T02:51:44Z, profile=debug) | rustc 1.98.1 | target x86_64-pc-windows-gnu
✅    configuration              source=built-in defaults | db=sqlite://packetsage.db rules=rules provider=(unset) api_key=unset
✅    rules directory            0 on disk + 4 built-in rule(s) from rules
✅    sample captures            8 capture(s); samples\synth-10m.pcapng detected as pcapng
✅    database                   sqlite://packetsage.db: read-only probe ok, 1 migration(s) applied
✅    output directories         writable: .
⚠     python environment         python 3.14.3; the packetsage_agent package is not importable
                                -> pip install -e ./agent
⚠     llm provider               not configured: run `packetsage-agent setup` (or export $PACKETSAGE_LLM_API_KEY)
                                -> packetsage-agent setup   # writes agent/.env; keys never go into packetsage.yaml
✅    end-to-end self check      ping RPC round trip over stdin/stdout
✅    schema version consistency version --json reports "schema_version":2 (protocol constant 2)

all checks passed (10 check(s))
```

读法：✅ 通过、⚠ 需要知道但不阻塞（如"还没装 Python 侧"）、❌ 失败（exit 3）、
⏭ 被 `--no-net` 跳过。每行失败都带 `-> 修复动作`。

## 3. 分析一个样例（耗时约 15 秒）

```console
$ packetsage analyze samples/synth-mixed.pcap
task            task_01M2YBQ0TCNDAPQ71J4E4KP4VK
file            samples\synth-mixed.pcap
sha256          90e21db307617665fd6eafda4a69049c0da7423163bbe47401f88abb4d0a5a6c
format          pcap
packets         612 (32.93 KiB captured)
time range      1700000000.000100 .. 1700000000.061200 s
duration        0.061100 s
sessions        570
alerts          2
decode errors   0
truncated       0
incomplete TCP  0
dropped sessions 0
protocols       tcp=604 udp=8
```

进度走在 stderr：

```console
$ packetsage analyze samples/synth-10m.pcapng 2>progress.txt >summary.txt
$ cat progress.txt
analyze: phase=parse packets=400000 bytes=12.1MiB speed=1.2M pkt/s elapsed=0.3s
done: 400000 packets in 0.3 s
```

`--jsonl` 时 stdout 是纯事件流（`schema_version=2`），可以 `jq` 直接消费：

```bash
packetsage analyze samples/synth-mixed.pcap --jsonl | jq -r 'select(.event=="alert") | .rule_id'
```

## 4. 读懂 summary（耗时约 1 分钟）

| 字段 | 含义 | 想追下去用 |
|---|---|---|
| `packets` / `(… captured)` | 包数 / 抓到的字节数（不是原始线速字节） | `--jsonl` 里的 `packet` 事件 |
| `time range` / `duration` | 首末包时间戳（秒，微秒精度）与跨度 | `query sessions --sort-by duration` |
| `sessions` | 规范化五元组会话数（双向收敛为一条） | `query sessions` |
| `alerts` | 规则引擎告警数（触发即内嵌规则） | `query alerts --severity high` |
| `decode errors` / `truncated` / `incomplete TCP` | 解码失败、截断包、未完成握手 | `analyze --errors-only --jsonl` |
| `protocols` | 传输层协议分布 | `query stats --layer tcp` |

结论：判据三个环节（install → analyze → 读懂 summary）在 10 分钟内完成，
全程只用到 `--help` 与 README §1/§3。

## 5. 顺手验证的其它入口（各 1 分钟）

```console
$ packetsage db migrate --db sqlite://demo.db --yes          # current=none target=20260920… → migrated
$ packetsage db query --readonly --db sqlite://demo.db?mode=ro \
      --sql "SELECT id, packet_count FROM analysis_tasks"     # 只读表格
$ packetsage db query --readonly --db sqlite://demo.db --sql "SELECT 1"
packetsage: sqlite://demo.db does not contain `mode=ro`; refusing to open a writable handle. …
$ echo $?   # 1：护栏生效
$ packetsage completions zsh > ~/.zfunc/_packetsage
$ packetsage chat samples/synth-mixed.pcap
task task_01M2YBVX8MASE64V13J2Z2BZT9 | packets=612 sessions=570 alerts=2 decode_errors=0 rules=4 | provider=mock model=-
packetsage·task_01M>> /help
```

`chat` 先在 Rust 侧 analyze + 落库，再由 Python Agent 起 `serve` 冷恢复该 task；
stdin/stdout 不是终端时直接 exit 1（提示改用 `packetsage-agent run`），REPL 细则见
《Agent CLI 工程规格书》§5。
