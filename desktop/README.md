# YeLee' PacketSage 桌面应用（Tauri 外壳）

《GUI 工程规格书 v0.2》的落地目录。三层各管各的：

```text
desktop/
├── index.html, src/            前端（React + TS + Vite + Tailwind）——界面与状态机
│   ├── App.tsx                 桌面 Agent 版式：左栏 + 状态条 + 会话流 + 输入框
│   ├── session.ts              会话流的纯逻辑：轮次装配（历史读回来先筛空壳）与时间线泳道次序
│   ├── envelope.ts             收尾信封的纯逻辑：模型原文 → 人话（流式与收尾同一个口径）
│   ├── components.tsx          会话流里的渲染件：结论卡 / 工具卡 / 证据链 / 报告（react-markdown）
│   ├── Wizard.tsx              U7 首次运行向导（第 1 步就是 API Key）
│   └── styles.css              四类面 + 四档字号/间距/圆角；状态只有绿蓝灰（+红/琥珀）
└── src-tauri/                  外壳（Rust + Tauri 2）——两个 sidecar 的生命周期与事件转发
    ├── src/jsonl.rs            stdio JSONL 子进程：请求/应答配对 + EOF 即刻感知死亡
    ├── src/sidecars.rs         引擎 / Agent 的解析与启动（打包后不依赖 PATH）
    ├── src/provider.rs         provider.json（非密钥）+ 环境注入
    ├── src/secrets.rs          Windows 凭据管理器（api key）
    ├── src/commands.rs         前端能调的命令（通道 A/B 的封装）
    └── tests/                  不需要窗口的验收：sidecar_smoke（S58/S60/S61/P1）
                                与 packaged_sidecars（P11 打包态连接 + U7 校验/握手）
```

界面版式按 §5.1「主流桌面 Agent」落地，一屏只有一个主角（结论）：

| 区域 | 内容 |
|---|---|
| 左栏 | 打开抓包、历史抓包（`task_*`）、模型一行、诊断与自检（可折叠）；折叠状态记在 `localStorage` |
| 顶部状态条 | 当前抓包 + 状态药丸 + 用量（步数 / 工具 / tokens / 费用）；预算分子分母在「调查过程」里 |
| 中央会话流 | 按轮次往下排，**一轮的形态按模式分**：`run` 调查 = 用户那句 → 模型总结 → **Hero 结论** → 时间线 → 原始数据 / 报告；`chat` 追问 = 用户那句 → **模型回答（正文）** → 结论平铺 → 时间线。翻过篇的轮次留在上面（虚线分隔） |
| 结论 Hero | 没有边框与底色；状态是图标徽章（🛡/⚠/▲/○），动作只留一个描边按钮，数字以等宽 stat 块躺在结论下面（包/会话/步骤/工具/耗时/tokens/费用/缓存） |
| 色彩纪律 | **彩色只表达状态与分类**：绿=无异常、琥珀=注意/降级、红=失败、蓝=调查中；交互一律中性（主按钮浅色实心、次级描边）——CSS 里没有"主色变量" |
| 时间线 | 默认展开：引擎预取 → 工具调用 → 结论；工具节点可展开看入参/结果，结论就地挂 `tc_*` 锚点（点结论行的"证据 N"会跳过去并展开）。**按轮分块，块内按泳道自上而下排**（`session.ts` 的 `orderTimeline()`） |
| 不是什么 | 调查本身**不是卡片**（只有一个标题行 + 时间轴）：只有工具调用是可展开卡片；模型文字（总结/思考/输出）都是纯文本行——曾经的嵌套 `details` 会让"点思考子卡片"把整块调查一起收掉 |
| 思考模式 | 向导第 1 步可选（自动/开/关）→ `provider.json.thinking` → 启动时注入 `PACKETSAGE_LLM_THINKING`；端点不认这个字段会自动去掉重发，不让整轮挂掉 |
| 模型面板 | 输入框右下角「模型 ▾」朝上弹出：模型名可选可填（候选来自端点 `GET /models`），思考模式三段（自动/开/关），保存即重启 Agent（key 沿用已存的） |
| 泳道 | 每条左侧一个标签：`思考`（模型的 `reasoning_content`，DeepSeek 思考模式默认打开）/ `调用`（工具 + 参数 + 结果）/ `输出`（模型写正文）/ `结论` / `拒绝`；轮内次序固定为 **调用 → 思考 → 输出 → 拒绝**（自上而下）。`输出` 正在流的时候只画在结论卡里，一轮收尾后以折叠原文回到那一轮最下 |
| 缓存命中 | 顶栏一枚「缓存 N%」+ 每轮账目行「缓存 48%（2.0k/4.3k 命中）」——数字来自 `usage.prompt_cache_hit/miss_tokens`（DeepSeek 上下文硬盘缓存）；provider 不报就不显示 |
| 模型总结 | 收尾那一轮模型自己写 2–4 句（`run_finished.summary`），显示在结论卡上方——`chat` 模式下它就是回答；模型没给就不显示（界面不替它编）。每条结论下面另有该 finding 自己的 `summary` |
| 对话历史 | 左栏「对话 · N」列出各个抓包下的每一轮（标题 = 你问的那句，点一下切到那个抓包并跳到那一轮）；按 `task_id` 存在本机 `localStorage`（引擎库没有对话文本，换机器就没了）。**只有真跑过的 run / chat 才算一轮**：归档前与载入时都过同一道门（`session.ts` 的 `isRealTurn()`），"打开抓包"不建轮也不再留下任何空壳 |
| 跟随滚动 | 调查中自动跟到最新；**用户手动往上翻就停下**（滚轮/拖滚动条/点进流里才算），给「↓ 回到最新」，回到底部自动恢复 |
| 报告坞 | 生成报告后在**右侧**打开（可关、可打开所在文件夹），对话列与输入框宽度不动 |
| 流式输出 | **token 级（U11 / §4.5 `llm_delta`）**：模型正在写的那一段实时长出来（正文只画尾巴 + 光标），一轮结束折成「模型原文」；工具起止、结论落地、每轮账目接着排在同一条时间轴上，跑完整体折成「调查过程」 |
| 图例 | 空态四张图例卡（结论 / 工具 / 证据 / 报告）+ 严重度色阶；会话流里同样一份收进「图例 · 这份输出怎么看」 |
| 底部输入框 | 常驻；`Enter` 发送、`Shift+Enter` 换行；跑起来时右侧变成「停止」 |
| 设置（向导） | 顶栏 ⚙ / 左栏「设置」打开同一个向导（模型、key、强度、自检）。**底部一块只读「本机参数」**：外壳版本、Agent 版本、协议版本、引擎 schema、能力、provider/模型/端点/强度、数据目录与两个 sidecar 路径——版本号各只有一处来源（外壳读 `Cargo.toml`，其余读握手） |
| 折叠与窄窗口 | 左栏收起后主区仍是整幅宽；输入框永远钉在窗口底部（内容再长也只滚会话流）；顶栏 ⚙ 与左栏「设置」是同一个向导；窄下来按顺序丢费用 → tokens → 抓包名，不压扁文字 |

M16 的拖放也在这里：把 `.pcap` / `.pcapng` 拖进窗口就开始分析（`App.tsx` 里走
`getCurrentWebview().onDragDropEvent`，和左栏按钮共用 `analyzePath`）。

界面四种状态的截图（未开始 / 有结论 / 无异常 / 只有规则告警）在 `../docs/ui/`，**本地留存、未入库**——
**它们是旧版式的截图**，重构后待重拍。

## 构建

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1
```

它按顺序做四件事：把 cargo 放进 PATH → 打 Agent sidecar（PyInstaller）→ 摆放 sidecar
→ `npm run tauri build`。产物：

* `desktop/src-tauri/target/release/packetsage-desktop.exe`（外壳）
* `desktop/src-tauri/target/release/bundle/nsis/YeLee’ PacketSage_4.0.0_x64-setup.exe`（安装包）

只想跑代码检查（外壳已装 clippy/rustfmt 组件）：

```powershell
cd desktop; npm run build                  # 前端类型 + 打包
cd desktop/src-tauri; cargo test           # 外壳验收：11 单元 + 6 外壳 + 3 打包态
cd desktop/src-tauri; cargo clippy --all-targets -- -D warnings
```

`tests/packaged_sidecars.rs`（P11）连的是**真 exe**：引擎用 `target/release/packetsage.exe`，
Agent 用 `binaries/agent-sidecar/packetsage-agent.exe`（安装包里那一份）。先跑
`python scripts/stage_desktop_sidecars.py` 把产物摆好；没摆好时这条用例打一行 SKIP。
它同时覆盖 U7 的两条关键路径：向导的"校验"（`setup --verify --print` 的三态）与
"没配 provider → 握手说未配置；注入 provider → 说配置好了"。

## 开发模式

```powershell
cd desktop; npm run tauri dev
```

开发模式下 shell 会依次找：`$PACKETSAGE_ENGINE` / `$PACKETSAGE_AGENT` → 程序旁的
sidecar → PATH → 开发树（`target/release/packetsage.exe`、`python -m packetsage_agent serve`）。
所以在本仓库里 `npm run tauri dev` 不需要预先打包。

## 用户数据与凭据

* 数据：`%LOCALAPPDATA%\PacketSage\{packetsage.db, reports\, logs\}`（§7.1，卸载默认保留）
* 日志：`logs\engine.log` / `logs\agent.log`（stderr 只作诊断，界面只显示尾部 3 行）
* provider（U7）：首次启动（或点侧栏「接上模型」）会开**向导**——第 1 步就是 API key，
  校验走 Agent 自己的 `setup --verify --print`；保存后 **key 进 Windows 凭据管理器**
  （target `PacketSage:llm-api-key`），provider/model/base_url 进
  `%LOCALAPPDATA%\PacketSage\provider.json`，两者在启动时拼成
  `PACKETSAGE_LLM_*` 环境变量交给 Agent sidecar。

  外壳**故意**把 `PACKETSAGE_ENV_FILE` 指到 `<data>\agent.env`（一个不存在的文件），
  所以开发树里的 `agent/.env` 不会影响桌面应用：凭据管理器是唯一的 key 来源。
  没配 provider 时握手报 `providers.configured=false`——**只**拦住「开始调查 / 追问」，
  打开抓包、分析、规则告警、证据链、报告照常可用（`S67` 只要求拒绝 run）。
  想切回确定性回放：向导里选「演示模式（mock）」，文案会写明这是脚本回放。

## 已知缺口（对照 v0.2 的 U 系列）

| 项 | 状态 |
|---|---|
| U1 Agent sidecar 协议 | ✅ 完成（`tests/sidecar/protocol_cases.py` 8/8） |
| U2 Agent sidecar 打包 | ✅ 完成（`scripts/build_agent_sidecar.py`，67 MB onedir） |
| U3 外壳与进程管理 | ✅ 代码完成 + 外壳测试通过；M14（关窗口后进程消失）待人工确认 |
| U4 前端流式渲染 | ✅ 代码完成；M6（肉眼看到卡片逐条出现）待人工确认 |
| U5 取消语义 | ✅ ack → `finalizing` → `run_finished` 的文案与状态机已实现 |
| U6 引擎会话与重建 | ⬜ 目前只在启动时起一次；崩溃后的**界面重建按钮**待补 |
| U7 provider 与凭据管理器 | ✅ 首次运行向导（`src/Wizard.tsx`）+ Windows 凭据管理器（`src-tauri/src/secrets.rs`）；见下 |
| U8 数据读取路径 | ✅ 走 RPC；历史列表走 `db query --readonly --jsonl` |
| U9 安装包与首次运行 | ⬜ NSIS 安装包已能产出；开始菜单/卸载确认/首次运行向导待做 |
| U10 协议版本协商 | ✅ 握手 `protocol_version` + `PROTOCOL_MISMATCH` 拒绝后续命令 |
| CI | ⬜ 桌面的构建/测试还没进 CI（Linux runner 缺 WebKitGTK；先只跑 `npm run build` 是可行的下一步） |

## 体积

安装包 44.1 MB（NSIS/LZMA），文件名 `YeLee’ PacketSage_4.0.0_x64-setup.exe`。里面装的是：
外壳 5 MB + 引擎 125 MB（未 strip）+ Agent
sidecar 67 MB。引擎那一份远超 §7.2 的 5–15 MB 预算，原因是没有 strip 调试信息——
给引擎的发布档加 `strip = true`（或 `debug = false`）会显著缩小，属引擎构建策略，待与
CLI 发布一起定。
