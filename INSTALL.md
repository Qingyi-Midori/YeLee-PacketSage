# 安装与运行

> 唯一交付形态是**桌面应用**。`packetsage`（引擎）与 `packetsage-agent`（Agent）两个二进制
> 是进程入口，由桌面外壳自动拉起——使用者不需要单独安装它们。

## 1. 装桌面应用

1. **拿到安装包**：`YeLee’ PacketSage_4.0.0_x64-setup.exe`
   - 构建产物在 `desktop/src-tauri/target/release/bundle/nsis/`；
   - 或自己打一份：`powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1`。
2. **双击安装**：装到当前用户目录（NSIS `currentUser`，不弹 UAC）。
   目标机**不需要** Python / Rust / Node / Visual Studio——两个 sidecar 随包分发。
3. **首次启动 → 向导**：选 provider → 填 API key → 「校验并继续」→ 保存并自检。
   - key 进 **Windows 凭据管理器**（target `PacketSage:llm-api-key`），不写明文文件；
   - `provider / model / base_url / thinking` 进 `%LOCALAPPDATA%\PacketSage\provider.json`；
   - 只想先跑通流程就选「演示模式（mock）」：那是脚本回放，界面和报告里都会写明。
   - 也可以「先不接模型，直接进界面」——打开抓包、分析、规则告警、证据链、报告都不需要模型。
4. **打开抓包**：把 `.pcap` / `.pcapng` 拖进窗口，或用左栏「打开抓包」。

数据都在 `%LOCALAPPDATA%\PacketSage\`（`packetsage.db` / `reports\` / `logs\`），**卸载默认保留**；
重装或升级不会动它。

## 2. 从源码构建

```powershell
# ① 引擎（Windows/GNU 宿主要用脚本：它会找 cargo/MinGW 并绕开 dlltool 与空格路径两个坑）
powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release

# ② Python Agent
python -m pip install -e ./agent

# ③ 外壳 + sidecar + 安装包（PyInstaller 打 Agent sidecar → 摆位 → npm run tauri build）
powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1

# ④ 开发模式（热重载；不需要先打包）
cd desktop; npm run tauri dev
```

命令行自检：`cd desktop/src-tauri; cargo test`（外壳 22 项）、`cd agent; python -m pytest -q`（179 项）。
其余各层怎么跑见 [Wiki · 测试与验证](https://github.com/Qingyi-Midori/YeLee-PacketSage/wiki/开发与测试)。

## 3. 引擎与 Agent 的进程入口

外壳用这几个入口驱动两个 sidecar（**不是**给人敲的日常命令）：

| 入口 | 谁在用 | 说明 |
|---|---|---|
| `packetsage serve` | 引擎 sidecar | JSONL RPC worker，唯一出口；事件 schema v2 |
| `packetsage doctor --json --no-net` | 向导第 3 步 | 十项自检 |
| `packetsage db query --readonly --jsonl` | 左栏历史任务列表 | 唯一 SQL 出口：url 必须含 `mode=ro`、语句限 SELECT/WITH/EXPLAIN |
| `packetsage-agent serve` | Agent sidecar | 通道 B（命令在上、事件在下） |
| `packetsage-agent setup --verify/--list-models --print` | 向导第 1 步 | key 校验与模型下拉，只读、不写文件 |

其余子命令（`analyze` / `rules` / `query` / `schema` / `version` …）留作开发与排障，
不在产品路径上。命令表、退出码、事件与 RPC 契约见
[Wiki · 命令与契约](https://github.com/Qingyi-Midori/YeLee-PacketSage/wiki/命令与契约)。

## 4. 升级与卸载

- **升级**：直接装新版本，覆盖安装即可；`%LOCALAPPDATA%\PacketSage\` 里的数据不动。
  版本号规则是 `大更新.小更新.bug`（见 [README](README.md) §0）。
- **卸载**：控制面板 → 应用 → `YeLee’ PacketSage`；用户数据默认保留，需要清就手工删
  `%LOCALAPPDATA%\PacketSage\`，以及凭据管理器里的 `PacketSage:llm-api-key`。
