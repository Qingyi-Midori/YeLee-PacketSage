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
| bug | 只修问题（内测期最常出现） | `4.0.0` → `3.8.2` |

**当前版本 `4.0.0`**（内测期）；它是 git 之后的第一版，因此**从这一版起**按上面的规则递增。
版本号只有两个权威落点，改的时候一起改，其余地方（安装包文件名、`packetsage version`、
Agent 的 `--version`）都是从它们推导出来的：

| 落点 | 文件 |
|---|---|
| 引擎 + CLI + 桌面外壳 | `Cargo.toml` 的 `[workspace.package] version` 与 `desktop/src-tauri/{Cargo.toml,tauri.conf.json}` |
| Python Agent | `agent/pyproject.toml`（`_version.py` 读它，不重复存） |

## 1. 快速开始（桌面应用）

**唯一交付形态是一个 Windows 安装包**，目标机不需要 Python / Rust / Node：

1. 下载 `YeLee’ PacketSage_4.0.0_x64-setup.exe`；
2. 双击安装（装到当前用户目录，不弹 UAC）；
3. 首次启动开向导：选 provider → 填 API key（存进 **Windows 凭据管理器**，不落明文）→ 保存并自检；
4. 把 `.pcap` / `.pcapng` 拖进窗口，点「开始调查」。

没配模型也能用：打开抓包、分析、规则告警、证据链、报告都走引擎，不经过模型；
只有「开始调查 / 追问」需要 provider。想先跑通全流程可以先选向导里的「演示模式（mock）」——
那是脚本回放，界面与报告里都写明。

### 1.1 从源码构建（开发者）

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release   # 引擎发布档
python -m pip install -e ./agent                                                 # Python Agent
powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1               # sidecar + 外壳 + 安装包
cd desktop; npm run tauri dev                                                    # 开发模式（热重载）
```

`scripts/build.ps1` 自己找 cargo / MinGW 并绕开 Windows + GNU 工具链的两个坑
（`dlltool` 找不到、MinGW 装在含空格路径时 `ld` 报错），背景见
[docs/architecture.md](docs/architecture.md) 偏差表 D-12。

### 1.2 引擎与 Agent 的命令行入口

`packetsage` / `packetsage-agent` 两个二进制现在是**进程入口**：桌面外壳用 `serve`、
`doctor`、`db query --readonly`、`setup --verify` 驱动它们。命令表、事件与 RPC 契约、
三流分离、配置发现链与脱敏都在 Wiki：
**[命令与契约](https://github.com/Qingyi-Midori/YeLee-PacketSage/wiki/命令与契约)**。

### 1.3 样例抓包

仓库不存 pcap（`.gitignore` 排除，第三方抓包的授权登记在
[docs/third-party-captures.md](docs/third-party-captures.md)），需要时现造：

```bash
python scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 --profile mixed
```

## 2. 项目结构

```text
crates/
├── packetsage-protocol/   L0 事件与 RPC schema（无 IO/时钟/随机数）
├── packetsage-core/       L1 reader / decoder / reassembly / conversation / query / pipeline
├── packetsage-rules/      L2 YAML DSL + S1–S9 校验 + 滑动窗口评估器
├── packetsage-storage/    L1' Repository + SQLite 实现 + migrations（sqlx 唯一入口）
├── packetsage-cli/        引擎的进程入口 binary `packetsage`（`serve` / `doctor` / 分析与查询）
└── packetsage-fuzz/       稳定版 fuzz smoke harness
agent/packetsage_agent/    engine client / tools / policy / provider / agent / report / eval
gui/                       Streamlit 原型界面（参考实现；交付形态改为 Tauri，见《GUI 工程规格书 v0.2》）
desktop/                   Tauri 桌面应用：src/ 前端（React+TS+Vite+Tailwind）、src-tauri/ 外壳（Rust）
rules/builtin|examples/    四条内置规则（随二进制内嵌）+ 三条示例规则
migrations/                SQLite/PostgreSQL 双方言 migration
scripts/                   gen_traffic.py（合成流量）、bench.py（benchmark）、build*.ps1（构建）
tests/                     cli 契约 / sidecar 协议 / GUI 原型用例 / 十步演示主线
fuzz/fuzz_targets/         cargo-fuzz 目标（nightly）
docs/                      architecture / error-codes / report-spec / benchmarks / ADR
docs/specs/                开发文档与各工程规格书；docs/archive/ 放已归档的 CLI 规格

测试记录与验收材料（`docs/ui/` 截图、`docs/agent-eval/` 评测结果）**留在本机、不进版本库**，
见 `.gitignore`；规格与项目报告里提到它们的地方都标了"本地留存"。
```

## 3. 桌面应用（产品）

界面是**一屏一个主角**：左栏去哪儿、中间会话流、右栏报告与「抓包里有什么」。

| 区域 | 内容 |
|---|---|
| 左栏 | 打开抓包、历史抓包、历史对话、模型一行、诊断与自检（默认折叠） |
| 中间 | 一轮对话一条线：**结论 → 证据 → 原始数据**；调查过程是默认展开的时间线（引擎预取 → 工具调用 → 结论），工具节点可展开看入参与结果 |
| 右栏 | `抓包里有什么` / `图例` / `报告` 三个页签，顶栏方框图标开合、`＋` 菜单选内容 |
| 底部 | 常驻输入框：`Enter` 发送、`Shift+Enter` 换行；跑起来时右侧变成「停止」 |

四种状态各有版式：**未开始 / 有结论 / 无异常 / 只有规则告警**；色彩只表达状态
（绿 = 无异常、琥珀 = 注意或降级、红 = 失败、蓝 = 调查中），交互一律中性。

细节见 [desktop/README.md](desktop/README.md)：构建、数据与凭据位置、已知缺口。
数据落在 `%LOCALAPPDATA%\PacketSage\`（卸载默认保留），provider key 存在
**Windows 凭据管理器**（target `PacketSage:llm-api-key`），界面不显示明文。

> `gui/` 的 Streamlit 原型已**冻结为参考实现**（只修 bug、不加功能），
> 跑法见 [Wiki · 测试与验证](https://github.com/Qingyi-Midori/YeLee-PacketSage/wiki/开发与测试)。

## 4. 当前状态与已知限制

已经端到端跑通并验证的：

* PCAP/PCAPNG（多 section/多接口/VLAN/QinQ、二进制 tsresol）解析；
* Ethernet/SLL/VLAN/ARP/IPv4/IPv6/TCP/UDP/ICMP/ICMPv6 + DNS/HTTP/TLS/DHCP 元数据；
* 规范化五元组会话聚合（双向收敛为一条）、TCP 重组（重传/乱序/缺口/超限/超时）；
* 四条内置规则在合成样本上真实触发；JSONL 事件流、RPC 九工具、SQLite 落库与结构化查询；
* Agent 工具循环（mock provider 确定性）+ 四级证据校验 + 九节报告 + 反幻觉 lint；
* Streamlit 原型：分析 → 调查 → 证据链 → 报告全链路（含停止按钮、引擎崩溃重建、
  provider 门禁）。原型已冻结（只修 bug、不加功能），2026-09-23 起不再进 CI；
* 桌面应用第一批：**Agent sidecar 协议（通道 B）**已实现并冻结——`packetsage-agent serve`
 提供 `hello/run/chat/report/cancel/status/shutdown` 与 10 类事件，`run_finished` 与
  `run --json` 同构，`seq` 单调、事件 ≤8 KiB、协议清单快照进 CI（`tests/sidecar/`）；
* 桌面应用第二批：**外壳可构建、可安装**——`desktop/`（React+TS 前端 + Rust 外壳，MSVC
  工具链）产出 `YeLee’ PacketSage_4.0.0_x64-setup.exe`；外壳的 sidecar 层有 4 条
  不依赖窗口的验收用例；Agent sidecar 由 PyInstaller `--onedir` 打成 67 MB 独立可执行；
* fuzz smoke（10 万级随机输入零 panic）、benchmark、doctor、demo 主线。

仍需在其它环境完成（本机条件不具备，已在 [docs/architecture.md](docs/architecture.md) 登记）：

* PostgreSQL matrix（`--features postgres`，需 Linux/CI）；MSRV 1.80 实测；1 GB 档 benchmark；
* 真实 LLM provider（`openai` / `local`）的 E1–E6 pinned 模型评测；
* golden fixtures 的逐字节固化（当前以单测 + demo 主线覆盖）；
* 真实样本授权说明（`wireshark/` 目录仅用于本地验证）。

## 5. 文档索引

| 文档 | 内容 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 分层、契约映射、全部实现偏差（D-5…D-13） |
| [docs/error-codes.md](docs/error-codes.md) | 退出码、RPC 错误码、事件与解码枚举 |
| [docs/report-spec.md](docs/report-spec.md) | 报告九节数据来源、反幻觉 lint、降级矩阵 |
| [docs/benchmarks.md](docs/benchmarks.md) | benchmark 方法、验收目标、ratchet 阈值 |
| [docs/ux-walkthrough.md](docs/ux-walkthrough.md) | “10 分钟判据”逐步记录（install → analyze → 读懂 summary） |
| [INSTALL.md](INSTALL.md) | 桌面应用怎么装；引擎/Agent 的进程入口 |
| [Wiki](https://github.com/Qingyi-Midori/YeLee-PacketSage/wiki) | 命令与契约 / 规则 DSL / Agent 与报告 / 测试与验证（README 里放不下的说明都在这里） |
| [docs/archive/](docs/archive/) | 已归档：CLI 收口工程规格书、Agent CLI 工程规格书与两份同步点 |
| [GUI 前收口文档 v0.1.md](docs/specs/GUI%20前收口文档%20v0.1.md) | 后端侧收口判据（G1–G7）、模拟测试剧本（S01–S56）、GUI 接口冻结清单 |
| [GUI 工程规格书 v0.2.md](docs/specs/GUI%20工程规格书%20v0.2.md) | 桌面应用：Tauri 架构、进程与数据交换协议、集成项 U1–U10、打包分发、人工验收 M1–M24 |
| [项目报告 v1.md](docs/specs/项目报告%20v1.md) | **最新状态**（2026-09-23）：完成度自评、本轮实测、缺陷 F1–F5、决策 D1–D9、收尾路线 |
| [项目报告 v0.1.md](docs/specs/项目报告%20v0.1.md) | 前身（2026-09-21）：交付物、连接层 11 条问题、HTTP/MCP/in-process 评估 |
| `docs/agent-eval/` | E1–E6 评测结果（**本地留存，未入库**） |
| [M0～M2 Rust 工程规格书.md](docs/specs/M0～M2%20Rust%20工程规格书.md) | 上游规格 |
| [M3～M6 工程规格书 v0.2.md](docs/specs/M3～M6%20工程规格书%20v0.2.md) | 上游规格 |
| [开发文档 v0.3.md](docs/specs/开发文档%20v0.3.md) | 总开发文档 |

## 6. 安全边界

PacketSage 是**分析工具**，不是入侵工具：不抓包、不发包、不执行抓包中的命令、
不改防火墙、不封 IP。抓包内容一律视为不可信数据（`trusted_as_instruction=false`），
工具结果默认脱敏 `Authorization` / `Cookie` 等字段，payload 只有显式调用
`reconstruct_stream` 才返回且有界（≤256 KiB）。

## License

MIT
