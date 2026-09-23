# YeLee' PacketSage 🛡️

> **⚠️ 已过期（归档）：** 本文件是 v0.2 设计稿，复选框与“未实现”标注均早于当前代码。
> 现状、命令面与验收判据以 [README.md((../../README.md) 和
> [GUI 前收口文档 v0.1((GUI%20前收口文档%20v0.1.md) §3 为准；本文件只保留设计沿革，不再更新。
> 当前测试基线与机器出口见 `GUI 前收口文档 v0.1` §2.1 / §5。

> 一个面向课程项目与网络安全实验的 **LLM Agent 网络协议分析工具**：把 PCAP/PCAPNG 的结构化分析结果交给 Agent，让它自己决定下一步应该查看什么证据，并最终生成可追溯的分析报告。

[![Core((https://img.shields.io/badge/core-Rust-orange?logo=rust)((https://www.rust-lang.org/)
[![Agent((https://img.shields.io/badge/agent-Python-blue?logo=python)((https://www.python.org/)
[![License((https://img.shields.io/badge/license-MIT-green)((LICENSE)

> **项目状态：开发中 / v0.2 设计版**
>
> 当前仓库尚未正式创建时，请不要把下面的 GitHub 地址视为已存在的公开仓库。

---

## 1. 这是什么

YeLee' PacketSage 的核心目标不是“让 LLM 直接读 pcap”，而是建立一条可靠的证据链：

```text
PCAP / PCAPNG
      │
      ▼
Rust 解析引擎
      │
      ├─ 协议解码
      ├─ TCP 流重组
      ├─ 会话聚合
      ├─ 统计与索引
      └─ 规则检测
      │
      ▼
结构化分析数据 / JSONL
      │
      ▼
Python Agent
      │
      ├─ 查看整体流量
      ├─ 找可疑会话
      ├─ 获取具体报文
      ├─ 重组 TCP 流
      ├─ 查询规则告警
      └─ 关联历史分析
      │
      ▼
可追溯 Markdown 报告
```

**LLM 不负责“计算事实”**。包数量、字节数、时间范围、五元组、规则命中次数等事实必须来自 Rust 引擎或数据库；LLM 主要负责分析路径选择、证据关联、自然语言解释与报告组织。

---

## 2. 为什么拆成 Rust + Python

| 部分 | 技术 | 职责 |
|---|---|---|
| Core | Rust | PCAP/PCAPNG 读取、协议解析、流重组、会话聚合、规则匹配 |
| CLI | Rust + clap | 命令行、参数校验、进度显示、错误码 |
| Agent | Python | LLM 调用、工具编排、Agent 状态、报告生成 |
| Storage | SQLite / PostgreSQL | 任务、会话、告警、Agent 执行记录 |
| Rules | YAML | 可扩展检测规则 |
| IPC | JSONL | Rust Engine 与 Python Agent 之间的数据契约 |

这种拆分的关键不是“为了显得高级”，而是让**高吞吐、强类型、确定性的部分与概率性的 LLM 部分隔离**。

---

## 3. 核心设计原则

### 3.1 证据优先

Agent 不允许凭空创造统计数字。

报告中的：

- 数据包数量
- 会话数量
- IP / Port
- 时间范围
- 流量字节数
- 规则命中次数
- 报文编号

必须能追溯到工具调用、引擎事件或数据库记录。

### 3.2 LLM 输出不是事实源

LLM 可以说：

> “该会话表现出端口扫描特征。”

但必须同时给出机器可验证的依据，例如：

```text
rule: NET-TCP-PORT-SWEEP-001
src: 192.168.1.10
dst: 192.168.1.20
observed_ports: 22, 80, 443, 445, 3389
window: 2.8s
```

### 3.3 pcap 内容是“不可信数据”

HTTP、DNS、TLS 扩展、TCP Payload 中可能出现类似：

```text
ignore previous instructions ...
```

Agent 必须把这些内容视为**抓包对象中的数据**，绝不能当成系统指令。

### 3.4 默认离线分析

MVP 默认只分析本地 PCAP/PCAPNG，不主动向第三方网站发送抓包内容。

向 LLM 发送 Payload 必须显式开启，并进行长度限制与敏感信息脱敏。

### 3.5 先做可解释单体 Agent，再考虑多 Agent

本项目第一版不做“多个 Agent 互相聊天”。

一个 Agent + 一组明确工具已经足够展示：

```text
观察 → 假设 → 调工具 → 获得证据 → 再观察 → 形成结论
```

后续如果真的需要，再拆分为协议分析 Agent、异常检测 Agent、报告 Agent。

---

## 4. MVP 范围

### 必做

- [x( 项目架构与数据契约设计
- [ ( PCAP 读取
- [ ( PCAPNG 读取
- [ ( Ethernet / IPv4 / IPv6
- [ ( TCP / UDP / ICMP / ICMPv6
- [ ( ARP
- [ ( DNS
- [ ( HTTP/1.1 元数据
- [ ( TLS 元数据
- [ ( DHCP
- [ ( TCP 流重组
- [ ( 会话聚合
- [ ( 协议统计
- [ ( YAML 规则引擎
- [ ( SYN Flood / 端口扫描 / DNS 可疑行为 / 畸形包规则
- [ ( Python Agent 工具集
- [ ( Markdown 报告
- [ ( SQLite 历史记录
- [ ( 单元测试 / 集成测试 / 规则测试

### 暂不做

- [ ( 实时抓包
- [ ( Web 前端
- [ ( TLS 解密
- [ ( 完整 HTTP/2 / HTTP/3 协议栈
- [ ( DPI / 深度内容分类
- [ ( 多 Agent 协作
- [ ( 自动处置网络攻击

这些功能并非不可实现，而是为了避免课程项目在“解析器、数据库、Agent、Web、实时流量、模型调用”之间失去主线。

---

## 5. 安装

### 环境

推荐：

- Rust **1.89+**
- Python **3.11+**，推荐 Python 3.12
- SQLite 作为默认数据库
- 可选 PostgreSQL

Rust 侧建议使用：

- `pcap-parser`：PCAP / PCAPNG 流式解析
- `etherparse`：Ethernet、IPv4、IPv6、TCP、UDP、ARP、ICMP 等基础协议解析
- `serde` / `serde_json`：结构化事件
- `clap`：CLI
- `sqlx`：SQLite / PostgreSQL 与迁移

`pcap-parser` 当前提供适合大文件的 streaming parser，并支持 PCAP 与 PCAPNG；`etherparse` 提供常见链路层、网络层、传输层协议解析。详见开发文档中的技术选型依据。

### Python 依赖

Agent 使用 LangChain 当前的 agent API：

```python
from langchain.agents import create_agent
```

项目设计上采用“工具调用循环”，概念上延续 ReAct 的观察—行动模式，但不绑定已经 deprecated 的旧 `create_react_agent` API。

---

## 6. 快速开始

由于项目当前处于开发阶段，以下命令描述目标 CLI，不代表所有命令已经实现。

```bash
# 构建
cargo build --release

# 最基础的解析与统计
packetsage analyze samples/http_traffic.pcap

# 完整模式
packetsage analyze suspicious.pcap \
  --full \
  --report report.md

# 输出机器可读 JSONL
packetsage analyze suspicious.pcap \
  --json events.jsonl

# 交互式分析
packetsage chat suspicious.pcap

# 查看规则
packetsage rules list

# 校验自定义规则
packetsage rules check rules/my_rule.yaml

# 查看环境
packetsage doctor
```

> 项目展示时建议使用 `packetsage` 作为真正的可执行文件名。
>
> `YeLee' PacketSage` 保留为项目品牌名，但不建议把包含单引号的品牌名直接作为 shell binary。

---

## 7. Agent 是怎么工作的

一次典型分析大致是：

```text
用户：帮我分析 suspicious.pcap

Agent
 │
 ├─ get_capture_summary()
 │      ↓
 │   发现：TCP 流量占比很高
 │
 ├─ get_conversations(...)
 │      ↓
 │   发现：某源 IP 建立大量短连接
 │
 ├─ check_alerts(...)
 │      ↓
 │   命中 TCP SYN Burst
 │
 ├─ filter_packets(...)
 │      ↓
 │   获取关键 SYN / SYN-ACK / RST
 │
 ├─ reconstruct_stream(...)
 │      ↓
 │   检查少量相关 TCP 流
 │
 └─ generate_report()
        ↓
     输出结论 + 证据
```

Agent 并不会被要求一次性“读完所有报文”。

它首先获取**低成本摘要**，再决定是否深入。

---

## 8. Agent 工具

MVP 工具如下：

| 工具 | 用途 |
|---|---|
| `get_capture_summary` | 文件、时间范围、包数、协议概况 |
| `get_protocol_stats` | L2/L3/L4/应用层协议统计 |
| `get_conversations` | 五元组会话与 Top 通信端点 |
| `filter_packets` | 按时间、IP、端口、协议、TCP Flags 过滤 |
| `inspect_packets` | 获取有限数量的关键报文详情 |
| `reconstruct_stream` | 按 TCP 会话重组有限长度 Payload |
| `check_alerts` | 查看规则引擎告警 |
| `query_history` | 查询历史任务 / 会话 / 告警 |
| `get_task_artifacts` | 获取分析任务生成的报告或事件文件 |

### 工具调用限制

每个工具都有：

- 最大返回条数
- 最大 Payload
- 最大时间范围
- 超时
- 参数 schema

Agent 不允许直接调用：

```sql
DROP TABLE ...
DELETE ...
UPDATE ...
ATTACH ...
```

Agent 的 `query_history` 使用**结构化过滤参数**，而不是直接暴露数据库写权限。

---

## 9. YAML 规则

第一版不使用自由文本：

```yaml
condition: |
  count(...) > ...
```

而采用结构化 DSL，更容易做 schema 校验与测试。

示例：

```yaml
id: NET-TCP-SYN-BURST-001
version: 1
name: TCP SYN 突发检测
severity: high
scope: packet
match:
  protocol: tcp
  flags:
    contains: [SYN(
    excludes: [ACK(
threshold:
  metric: count
  group_by: [src_ip(
  window: 10s
  operator: gt
  value: 100
description: >
  单一源 IP 在短时间内发送大量未完成 TCP SYN，
  可能表现为 SYN Flood 或扫描前置行为。
tags:
  - tcp
  - scan
  - dos
```

> `100 / 10s` 是项目示例阈值，不代表“100 就一定是攻击”。

规则命中之后：

```text
规则引擎：负责“有没有触发条件”
Agent：负责“如何把多个证据联系起来”
```

---

## 10. 报告示例

最终报告预计包含：

```markdown
# PacketSage Analysis Report

## 1. Capture Overview
- File: suspicious.pcapng
- Duration: 38.42 s
- Packets: 182,431
- Sessions: 4,912

## 2. Findings

### F-001 TCP SYN Burst
Severity: HIGH
Basis: RULE_MATCH

Source:
- 192.168.1.105

Observed:
- 10 s window
- 1,248 SYN packets
- 23 destination ports
- 7 completed handshakes

Evidence:
- Rule: NET-TCP-SYN-BURST-001
- Session: S-00421
- Packet range: 1821-3070

### F-002 Possible Port Sweep
Severity: MEDIUM
Basis: CORRELATED_OBSERVATION

...
```

报告强调：

**结论 → 依据 → 证据引用**

而不是只生成一段“看起来像分析师”的长文本。

---

## 11. 数据存储

默认 SQLite。

主要数据：

```text
analysis_tasks
captures
sessions
alerts
agent_runs
tool_calls
artifacts
```

不会默认把完整 Payload 永久复制进数据库。

抓包本体仍然保留在用户指定的位置；数据库记录引用：

- capture hash
- task id
- packet index
- byte offset（可用时）
- session id

这样可以避免数据库随着抓包文件大小线性膨胀。

---

## 12. 项目结构

目标目录：

```text
YeLee-PacketSage/
├── crates/
│   ├── packetsage-core/       # Rust 核心解析
│   ├── packetsage-rules/      # Rust 规则引擎
│   ├── packetsage-cli/        # Rust CLI
│   └── packetsage-protocol/   # JSONL / schema
├── agent/
│   ├── packetsage_agent/
│   │   ├── agent.py
│   │   ├── tools.py
│   │   ├── prompts.py
│   │   ├── report.py
│   │   └── engine_client.py
│   └── pyproject.toml
├── rules/
│   ├── builtin/
│   └── examples/
├── migrations/
├── samples/
├── tests/
│   ├── parser/
│   ├── rules/
│   ├── integration/
│   └── fixtures/
├── scripts/
├── docs/
├── Cargo.toml
├── README.md
└── 开发文档.md
```

---

## 13. 安全边界

PacketSage 是**分析工具**，不是入侵工具。

它不会：

- 主动扫描网络
- 自动向目标发送报文
- 执行抓包中的命令
- 自动修改防火墙
- 自动封禁 IP
- 自动删除数据库记录

它可以发现：

- SYN 异常
- 扫描特征
- DNS 可疑行为
- 异常会话
- 协议解析错误
- 某些明显的畸形流量

但报告会区分：

```text
observed        已观察到的事实
rule_match      规则命中
correlated      多个证据关联
hypothesis      分析假设
```

不会把“疑似攻击”直接写成“确定攻击”。

---

## 14. 当前开发路线

```text
M0  工程骨架
    └─ Cargo workspace + Python package + CI

M1  输入与解析
    └─ PCAP + PCAPNG + Ethernet + IP + TCP/UDP

M2  分析基础设施
    └─ 会话聚合 + TCP 重组 + JSONL

M3  规则
    └─ YAML schema + SYN + sweep + DNS + malformed

M4  Agent
    └─ engine client + tools + agent loop

M5  报告
    └─ evidence + markdown + history

M6  工程化
    └─ fuzz + benchmark + error handling + demo

M7  可选增强
    └─ Web UI / live capture / richer protocol support
```

---

## 15. 项目定位

PacketSage 最终要展示的不是：

> “我用了一个大模型，所以这是 AI 项目。”

而是：

> **传统网络协议分析负责提供可靠证据，Agent 负责选择分析路径与组织证据。**

这也是整个项目最重要的设计思想。

---

## 16. 参考

- PCAP savefile format: https://www.tcpdump.org/manpages/pcap-savefile.5.html
- PCAPNG Internet-Draft: https://datatracker.ietf.org/doc/html/draft-tuexen-opsawg-pcapng
- `pcap-parser`: https://docs.rs/pcap-parser/
- `etherparse`: https://docs.rs/etherparse/
- Rust Cargo: https://doc.rust-lang.org/cargo/
- clap: https://docs.rs/clap/
- SQLx: https://docs.rs/sqlx/
- sqlparser: https://docs.rs/sqlparser/
- LangChain agents: https://docs.langchain.com/
- ReAct paper: https://arxiv.org/abs/2210.03629
- Sigma: https://sigmahq.io/

---

## License

MIT
