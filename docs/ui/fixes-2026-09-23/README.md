# 2026-09-23 修复验收截图

对应《[GUI 待修问题 2026-09-22](../../specs/GUI%20待修问题%202026-09-22.md)》的四条问题与
《GUI 工程规格书 v0.2》§5.1.1 第 22 / 25 / 26 条。

跑法：`npm run tauri build` 出来的 `packetsage-desktop.exe`（release），
`LOCALAPPDATA` 指到一个临时目录（不碰本机真实的任务库），provider 用向导里的
**演示模式（mock）**——所以图里的结论都是确定性回放，不是真模型输出。

| 图 | 说明 |
|---|---|
| `01-right-dock-capture.png` | 右栏「抓包里有什么」：612 包 / 570 会话 / 2 条规则告警 + 按窗口列出的告警明细（第 22 条） |
| `02-right-dock-legend.png` | 同一栏切到「图例」：四层输出 + 严重度色阶 |
| `03-right-dock-report.png` | 同一栏切到「报告」：真生成一份九节报告，正文就地渲染（页签变「报告 · 已生成」） |
| `04-dock-plus-menu.png` | 顶栏 `＋` 菜单：`抓包里有什么` / `图例` / `报告`（挂在 `.main` 上，不被顶栏裁掉） |
| `05-open-capture-empty-state.png` | 打开抓包后会话流**只有空态**：没有「打开 XXX」那一轮，输入框那一带只剩 `＋ / 抓包名 / 模型 ▾ / 发送`（第 25 条） |
| `06-three-captures-switch-back.png` | 连开三份抓包后切回第一份：会话流仍然没有空壳轮次 |
| `07-third-capture-opened.png` | 第三份抓包打开的瞬间（同上，只是多一份对照） |
| `08-history-one-turn.png` | 跑过一次 `开始调查` 后切走再切回：左栏「历史对话 · 1」，会话流正好 1 轮 |
| `09-history-two-turns.png` | 再追问一次（`chat`）：左栏「历史对话 · 2」，会话流正好 2 轮 |
| `10-after-model-restart-ok.png` | 在模型面板里「保存并重启 Agent」之后：只有一行「已切到 mock，Agent 已重启。」，没有「Agent 已退出 / 调查没能完成」（第 26 条） |
