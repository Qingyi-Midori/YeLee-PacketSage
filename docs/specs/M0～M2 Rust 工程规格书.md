# YeLee' PacketSage — M0～M2 Rust 工程规格书
> **验收口径（2026-09-20）：** 本文件正文保持不变，正文内复选框不再逐条维护；
> 当前验收由《GUI 前收口文档 v0.1》§3（G1–G5）与 §4（S01–S56）接管，测试基线见该文 §2.1。
> **版本：v0.1**（配套《开发文档 v0.2》§5、§6、§7、§10、§11、§22、§25、§32）
> **状态：工程设计 / 待评审**
> 标记体系沿用开发文档：**【事实】** 上游已核实、**【设计】** 本规格规定的契约、**【待验证】** 需实现/测试确认。
---
## 0. 文档范围
| 里程碑 | 本规格覆盖内容 |
|---|---|
| M0 Skeleton | Workspace 骨架、crate 划分、依赖分层、CLI 骨架、CI、doctor |
| M1 Parser | `packetsage-core::reader`、`decoder`、packet model、JSONL 事件输出 |
| M2 Analyzer | `reassembly`、`conversation`、`query`、聚合稳定性验收 |
**不覆盖**：规则引擎内部实现（M3）、Python Agent（M4）、Report（M5）、fuzz/benchmark 正式基线（M6）。但为其预留的接口在本规格中一并冻结。
---
## 1. 总体工程原则（继承 v0.2 §2、§39）
1. **Rust 是确定性证据的生产者**：M0～M2 全部代码不依赖任何 LLM。
2. **不崩溃原则**：单包解码失败 → 产出 `decode_error` 事件并继续；仅文件结构级损坏可终止任务。库代码禁止 `panic!` / `unwrap()`（见 §7.2）。
3. **边界单向**：`packetsage-protocol` 不依赖任何业务 crate；`packetsage-cli` 不得包含业务逻辑，只做参数解析与装配。
4. **零拷贝优先**：reader 产出借用切片，仅在 reassembly 缓冲处复制必要字节。
---
## 2. Cargo Workspace 骨架
### 2.1 目录树（M0 交付形态）
```text
YeLee-PacketSage/
├── Cargo.toml                    # workspace 根
├── rust-toolchain.toml           # MSRV 固定
├── .github/workflows/ci.yml
├── crates/
│   ├── packetsage-protocol/      # L0：纯 DTO + JSONL/RPC schema
│   │   └── src/{lib.rs, events.rs, rpc.rs, ids.rs}
│   ├── packetsage-core/          # L1：reader/decoder/reassembly/conversation/query/model
│   │   └── src/{lib.rs, error.rs, config.rs, model.rs,
│   │             reader/, decoder/, reassembly/, conversation/, query/, pipeline.rs}
│   ├── packetsage-rules/         # L2：M3 前仅占位（空 lib + TODO 契约注释）
│   │   └── src/lib.rs
│   └── packetsage-cli/           # L3：唯一 binary `packetsage`
│       └── src/{main.rs, commands/, doctor.rs}
├── rules/builtin/                # YAML 规则目录（M3 前为空）
├── samples/                      # 样例捕获
├── tests/
│   ├── parser/  rules/  integration/  fixtures/  golden/
└── fuzz/                         # M6 起，M2 先留目录
```
### 2.2 Workspace 根 `Cargo.toml`
```toml
[workspace]
resolver = "2"
members  = ["crates/packetsage-protocol", "crates/packetsage-core",
            "crates/packetsage-rules",   "crates/packetsage-cli"]
[workspace.package]
version    = "0.1.0"
edition    = "2021"
license    = "MIT OR Apache-2.0"
repository = "TODO"                 # 【待验证】仓库创建后回填
[workspace.dependencies]
pcap-parser = "0.17"                # 【事实】提供 LegacyPcapReader/PcapNGReader 流式解析
etherparse  = "0.19"                # 【事实】SlicedPacket::from_ethernet / from_linux_sll
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
thiserror   = "1"
clap        = { version = "4", features = ["derive"] }
tracing     = "0.1"
bytes       = "1"
ulid        = "1"                   # 【待验证】task_id 生成
humantime-serde = "1"               # 【待验证】60s/5m 等配置解析
[workspace.lints.rust]
unsafe_code = "forbid"
unused_must_use = "deny"
[workspace.lints.clippy]
unwrap_used    = "deny"             # 库与 CLI 一律禁用 unwrap
expect_used    = "warn"             # 测试代码可用 #[allow]
panic          = "deny"
todo           = "warn"
integer_division = "warn"
[profile.release]
lto       = "thin"
debug     = true                    # 保留符号，便于 benchmark 分析
```
### 2.3 Crate 依赖分层（强制）
```text
packetsage-cli ──▶ packetsage-core ──▶ packetsage-protocol
      │                    │
      │                    └──▶（pcap-parser / etherparse 仅在此出现）
      └──▶ packetsage-protocol（RPC 常量）
packetsage-rules ──▶ packetsage-protocol（M3：另依赖 core 的事件模型）
```
| 规则 | 说明 |
|---|---|
| L0 无反向依赖 | `protocol` 依赖仅限 `serde/serde_json`；**禁止**引入 IO、时钟、随机数 |
| L1 独占三方解析 | `pcap-parser`、`etherparse` 只允许出现在 `packetsage-core` 的 `reader` / `decoder` 模块内，其他模块只消费本 crate 类型 |
| L3 无业务逻辑 | `cli` 只做：参数解析 → 构造 config → 调用 `core`/`rules` → 格式化输出 |
| 版本单点 | 所有依赖版本集中在 `[workspace.dependencies]`，crate 内用 `workspace = true` 引用 |
### 2.4 MSRV
**【设计 / 待验证】** MSRV = **1.80**，写入 `rust-toolchain.toml`。理由：nom 8（pcap-parser 0.17 依赖）与 etherparse 0.19 的 MSRV 均低于此，留余量；以 CI 实测为准，达不到则下调并记录 ADR。
---
## 3. `packetsage-protocol`（L0）API 规格
### 3.1 模块树
```text
packetsage-protocol
├── events   # EngineEvent 及全部子结构（对齐开发文档 §8）
├── rpc      # RpcRequest / RpcResponse / 方法名常量（对齐 §21）
└── ids      # task_id / session_id / alert_id / finding_id 生成与格式校验
```
### 3.2 事件模型
```rust
pub const SCHEMA_VERSION: u32 = 1;
/// 引擎事件流顶层枚举。serde tag 固定为 "event"，值为 snake_case 变体名。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EngineEvent {
    TaskStarted(TaskStartedEvent),     // M0
    CaptureInfo(CaptureInfoEvent),     // M1：格式、接口、linktype、snaplen、首末时间
    Packet(PacketEvent),               // M1（对齐 §8.1）
    DecodeError(DecodeErrorEvent),     // M1（对齐 §8.2）
    SessionSummary(SessionSummaryEvent), // M2（对齐 §11）
    StreamState(StreamStateEvent),     // M2：INCOMPLETE / 超限等流级异常
    Stats(StatsEvent),                 // M2：protocol stats / top conversations
    TaskFinished(TaskFinishedEvent),   // M0
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketEvent {
    pub schema_version: u32,
    pub task_id: String,
    pub packet_index: u64,
    /// canonical 纳秒时间戳，字符串化避免 JS Number 精度丢失（§7.1）
    pub ts_unix_ns: String,
    pub ts_precision: TsPrecision,          // ns | us | ms | s（原始精度元数据）
    pub interface_id: u32,
    pub captured_len: u32,
    pub original_len: u32,
    pub linktype: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkInfo>,             // Ethernet II / VLAN / SLL 摘要
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkInfo>,       // src/dst/protocol、ttl、fragment 标记
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportInfo>,   // ports、flags、seq/ack（TCP）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<ApplicationInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<PayloadRef>,
    pub decode_status: DecodeStatus,        // ok | partial | error
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadRef {
    pub packet_index: u64,
    /// payload 相对本 record 原始字节（link 层起点）的偏移
    pub offset: u32,
    pub length: u32,
}
```
`DecodeErrorEvent` 字段固定为：`schema_version, task_id, packet_index, layer, code, message`。
`layer` 枚举：`link | vlan | arp | ipv4 | ipv6 | tcp | udp | icmp | icmpv6 | dns | http | tls | dhcp`。
`code` 首批固定值：`truncated_header | bad_length | invalid_field | unsupported_linktype | unsupported_protocol`。**新增 code 必须在本 crate 定义枚举，不允许裸字符串出库**。
### 3.3 RPC 模型（对齐 §21）
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest { pub id: String, pub method: String, pub params: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")] pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")] pub error: Option<RpcError>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError { pub code: RpcErrorCode, pub message: String }
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RpcErrorCode {
    InvalidArgument, NotFound, NotImplemented, Internal, Io, UnsupportedCapture,
}
```
方法名常量集中在 `rpc::method` 模块（`PING`、`ANALYZE_FILE`、`GET_CAPTURE_SUMMARY`、…），method 字符串是全局唯一事实源。
### 3.4 ID 约定【设计】
| ID | 格式 | 生成 |
|---|---|---|
| `task_id` | `task_{ulid}` | `ulid` crate，UTC 单调 |
| `session_id` | `S-{n:06}`，n 从 1 起，task 内唯一 | 聚合器计数器（对齐 §17 示例 `S-00421`） |
| `alert_id` / `finding_id` | `alert_{ulid}` / `F-{n:03}` | 预留，M3/M5 使用 |
---
## 4. `packetsage-core`（L1）API 规格
### 4.1 模块树
```text
packetsage-core
├── error.rs        # PacketSageError（对齐 §25）
├── config.rs       # EngineConfig（对齐 §24）
├── model.rs        # PacketRecord / Endpoint / SessionKey / 五元组规范化
├── reader/         # detect_format、open_reader、LegacyPcapReader、PcapNgReader、ts 换算
├── decoder/        # Decoder、协议解析矩阵映射（etherparse 对接层）
├── reassembly/     # Reassembler、StreamState、limits
├── conversation/   # ConversationAggregator
├── query/          # QueryEngine（summary / stats / conversations / filter）
└── pipeline.rs     # AnalyzePipeline + EventSink（把 reader→decoder→…→sink 串成一条线）
```
### 4.2 `model`：统一包模型与五元组规范化
```rust
pub struct PacketRecord<'a> {           // 内部模型；对应 protocol::PacketEvent 的超集
    pub task_id: TaskId,
    pub packet_index: u64,
    pub ts_ns: i128,                    // 内部一律 i128 纳秒
    pub ts_precision: TsPrecision,
    pub interface_id: u32,
    pub captured_len: u32,
    pub original_len: u32,
    pub linktype: u32,
    pub raw: &'a [u8],                  // 零拷贝原始字节（link 层起点）
    pub decoded: Decoded<'a>,           // 见 4.4
    pub decode_status: DecodeStatus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Endpoint { pub ip: IpAddr, pub port: u16 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub proto: TransportProto,          // Tcp | Udp
    pub a: Endpoint,                    // 字典序较小端
    pub b: Endpoint,                    // 字典序较大端
}
impl SessionKey {
    /// 规范化五元组（对齐 §10.1）：
    /// 比较顺序 = proto → ip（std IpAddr Ord：V4 < V6，V4 按字节）→ port。
    /// 同一会话两个方向的 key 完全一致 —— 这是 M2 稳定性验收的根。
    pub fn canonicalize(proto: TransportProto, src: Endpoint, dst: Endpoint) -> Self {
        if (src.ip, src.port) <= (dst.ip, dst.port) { Self { proto, a: src, b: dst } }
        else                                        { Self { proto, a: dst, b: src } }
    }
}
```
> 排序规则**【设计】**：直接依赖 `IpAddr`/`Ipv4Addr` 的 std `Ord`（V4 字节序数值比较；V4 < V6 恒成立），禁止自创比较器，保证跨平台一致。
### 4.3 `reader`：PCAP / PCAPNG 输入层
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat { Pcap, PcapNg }
/// 仅读前 16 字节判格式：0xA1B2C3D4 族 → Pcap；0x0A0D0D0A → PcapNg
pub fn detect_format(header: &[u8; 16]) -> Result<CaptureFormat, PacketSageError>;
/// 流式产出（单条记录级），驱动循环由 pipeline 实现
pub trait PacketSource<'a> {
    /// 推进到下一条产出；Ok(()) 后可再取 current()；Err(CaptureCorrupted) 终止
    fn advance(&mut self) -> Result<(), PacketSageError>;
    fn current(&self) -> Option<SourceItem<'_>>;
    fn interfaces(&self) -> &[InterfaceState];   // Reader 状态（§6.1：非全局常量）
}
pub enum SourceItem<'a> {
    Packet(RawPacketRecord<'a>),
    NonFatal(NonFatalInfo),      // 未知 block skipped、bad block skipped —— 计数，不中断
}
pub struct RawPacketRecord<'a> {
    pub packet_index: u64,        // 全文件从 0 计数，含被跳过的坏记录
    pub interface_id: u32,
    pub linktype: u32,
    pub ts_ns: i128,
    pub ts_precision: TsPrecision,
    pub caplen: u32,
    pub origlen: u32,
    pub truncated: bool,          // caplen < origlen → snaplen 截断标记
    pub data: &'a [u8],
}
pub struct InterfaceState {
    pub interface_id: u32,
    pub linktype: u32,
    pub snaplen: Option<u32>,
    pub ts_resol: TsResolution,   // pcapng: if_tsresol；pcap: 由文件头魔数决定
}
/// 统一入口：自动探测格式（内部使用 pcap-parser 的 create_reader 语义）
pub fn open_reader<R: Read + 'a>(input: R) -> Result<Box<dyn PacketSource<'a> + 'a>, PacketSageError>;
pub struct LegacyPcapReader<R> { /* … */ }
pub struct PcapNgReader<R>     { /* … */ }   // 维护 SectionState { endian, interfaces }
```
**与 `pcap-parser` 的对接映射**【事实，依据 docs.rs 0.17】：
| 本 crate 概念 | pcap-parser 对应物 |
|---|---|
| 驱动循环 | `PcapReaderIterator`：`next()` → `(offset, PcapBlock)`；`Ok` → `consume(offset)`；`PcapError::Incomplete` → `refill()`；`PcapError::Eof` → 结束 |
| Legacy 包 | `PcapBlock::Legacy`（含 `ts_sec/ts_usec/caplen/origlen/data`） |
| PCAPNG 包 | `NgBlock::Packet(EnhancedPacketBlock)`（`interface_id / ts_high / ts_low / caplen / origlen / data / options`） |
| 接口注册 | `NgBlock::InterfaceDescription(...)` → 追加 `InterfaceState`（含 `if_tsresol` option） |
| 未知 block | `NgBlock::Custom` / 其他 → `NonFatal::UnknownBlockSkipped`，计数器 +1，**不 panic**（§6.3） |
| 字节序 | PCAP 由文件头魔数决定；PCAPNG 由 SHB byte-order magic 0x1A2B3C4D 决定，逐 Section 更新 |
**时间戳换算**【设计】：
```rust
/// 统一换算为 i128 纳秒；返回值与 ts_precision 一起保存（§7.1）
pub fn ticks_to_unix_ns(ticks: u64, resol: TsResolution) -> i128;
// TsResolution::Decimal(n) → ticks * 1e9 / n   （if_tsresol bit7=0，默认 n=6 → µs）
// TsResolution::Binary(exp)  → ticks << (30 - exp) 纳秒（if_tsresol bit7=1，默认 exp=6）
// Legacy pcap：magic 0xA1B2C3D4 族 → µs；0xA1B23C4D 族 → ns；字节序由魔数字节自判
```
> 公式按开发文档 §6 引用的 `pcap-savefile(5)` 与 PCAPNG draft 定义实现，边界用单元测试覆盖（§8）。
### 4.4 `decoder`：协议解析矩阵映射
```rust
pub struct DecodeOptions { pub app_protocols: BTreeSet<AppProto> }   // 默认全开（§9 矩阵）
pub struct Decoder { opts: DecodeOptions }
pub struct Decoded<'a> {
    pub link: Option<LinkInfo>,           // eth macs / SLL / VLAN ids（含 QinQ，≤3 层）
    pub network: Option<NetworkInfo>,     // 含 is_fragment 标记
    pub transport: Option<TransportInfo>, // TCP flags / seq / ack；UDP 无
    pub application: Option<ApplicationInfo>,
    pub payload: Option<&'a [u8]>,        // 最内层 payload（应用层）
    pub errors: Vec<DecodeErrorInfo>,     // 每层可带多条；单层失败不影响下层已成功字段
}
```
**与 `etherparse` 的对接映射**【事实，依据 docs.rs 0.19】：
| 协议矩阵（§9） | 实现 |
|---|---|
| Ethernet II | `SlicedPacket::from_ethernet(data)` → `link: Option<LinkSlice>` |
| 802.1Q VLAN | `link_exts: ArrayVec<LinkExtSlice, 3>`（QinQ 天然支持，取 `vlan_ids()`） |
| Linux SLL | `SlicedPacket::from_linux_sll(data)`（linktype 113 免费获得，超出矩阵的赠品） |
| ARP / IPv4 / IPv6 | `net: Option<NetSlice>`（含 extension header chain）；分片检测用 `is_ip_payload_fragmented()` |
| TCP / UDP | `transport: Option<TransportSlice>` |
| ICMP/ICMPv6、DNS、HTTP、TLS、DHCP | 本 crate `decoder::app/` 子模块，基于 `ip_payload()` / `transport` payload 自行解析（**仅元数据**，符合 §9"不要求完整语义"） |
解析失败→`DecodeErrorInfo { layer, code }`；单包多错误合并为一个 `decode_error` 事件逐条列出。**不使用 `PacketHeaders`，统一从 `from_ethernet`/`from_linux_sll` 入口**，保证与 linktype 判定一处收口。
`payload_ref.offset` 计算：`payload.as_ptr() as usize - record.raw.as_ptr() as usize`（u32 溢出即 `bad_length` 错误）。
### 4.5 `reassembly`：TCP 流重组
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ReassemblyLimits {            // 对齐 §10.5 / §24 默认值
    #[serde(with = "humantime_serde")] pub stream_timeout: Duration,   // 60s
    pub max_stream_buffer: u64,          // 8 MiB
    pub max_sessions: usize,             // 100_000
    pub max_segments_per_stream: usize,  // 4_096
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamState { New, Active, HalfClosed, Closed, Incomplete, BufferOverflow }
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction { ClientToServer, ServerToClient }
pub struct Segment<'a> { pub seq: u32, pub flags: TcpFlags, pub data: &'a [u8], pub ts_ns: i128 }
pub enum SegmentVerdict {
    InOrder,                             // 直接拼上 next_seq
    Buffered { gap_bytes: u64 },         // 乱序暂存（§10.4）
    Retransmission,                      // seq 区间已存在，不重复输出（§10.3）
    PartialOverlap { dropped_bytes: u32 },// 重叠部分按 policy 丢弃
    Dropped(DropReason),                 // 超限：BufferFull | TooManySegments | SessionLimit
}
pub enum OverlapPolicy { FirstWins, LastWins }   // 默认 FirstWins【待验证：§35.6】
pub struct Reassembler { /* streams: HashMap<SessionKey, TcpStreamBuf>, limits, policy */ }
impl Reassembler {
    pub fn new(limits: ReassemblyLimits, policy: OverlapPolicy) -> Self;
    pub fn feed(&mut self, key: &SessionKey, dir: Direction, seg: Segment<'_>) -> SegmentVerdict;
    /// 取出当前可连续交付的数据（按 seq 顺序）；调用即移出缓冲
    pub fn take_contiguous(&mut self, key: &SessionKey, dir: Direction) -> Vec<Bytes>;
    /// 缺口区间；非空 ⇒ 流状态必须为 Incomplete（对齐 §10.4"不能伪造完整 Payload"）
    pub fn missing_ranges(&self, key: &SessionKey, dir: Direction) -> Vec<ByteRange>;
    pub fn state(&self, key: &SessionKey) -> StreamState;
    pub fn close(&mut self, key: &SessionKey, dir: Option<Direction>, how: CloseHow); // FIN / RST
    pub fn prune_expired(&mut self, now_ns: i128);
    pub fn session_count(&self) -> usize;
}
```
**方向判定**【设计】：客户端 = 会话中**第一个裸 SYN（无 ACK）的发送方**；无 SYN 的会话（如 UDP 化导出的 TCP 片段）= 首包发送方。该端点即 `Direction::ClientToServer` 的源。
**超限策略**【设计，须有单测】：丢**最旧**缓冲段并将流标记为 `BufferOverflow`（对外呈现为 INCOMPLETE 类异常，`StreamStateEvent` 记录原因）；不静默清空、不终止任务。
### 4.6 `conversation`：会话聚合
```rust
pub struct ConversationAggregator { /* sessions: HashMap<SessionKey, SessionEntry> */ }
pub struct SessionEntry {
    pub session_id: String,          // S-000001 起，出现顺序分配
    pub key: SessionKey,
    pub stats: ConversationStats,
}
pub struct ConversationStats {     // 字段逐一对应 §11 列表
    pub first_ts_ns: i128, pub last_ts_ns: i128,
    pub packets: u64, pub bytes: u64,
    pub src_packets: u64, pub dst_packets: u64,
    pub src_bytes: u64,   pub dst_bytes: u64,
    pub syn_count: u32, pub syn_ack_count: u32,
    pub rst_count: u32,  pub fin_count: u32,
    pub retransmission_count: u32,       // 来源：Reassembler 的 Retransmission 判定
    pub out_of_order_count: u32,         // 来源：Buffered 判定
    pub state: StreamState,
    pub app_protocol: Option<AppProto>,  // 首个识别出的应用层协议
}
impl ConversationAggregator {
    pub fn observe(&mut self, rec: &PacketRecord<'_>);          // 由 pipeline 逐包调用
    pub fn sessions(&self) -> impl Iterator<Item = &SessionEntry>;
    pub fn get(&self, session_id: &str) -> Option<&SessionEntry>;
}
```
### 4.7 `query`：分析查询引擎
```rust
pub struct QueryEngine<'a> {
    summary:   &'a CaptureSummary,
    sessions:  &'a ConversationAggregator,
    packets:   &'a PacketIndex,        // 轻量索引：仅 header/meta，不存 payload（§34 风险控制）
}
impl QueryEngine<'_> {
    pub fn capture_summary(&self) -> CaptureSummary;                    // 对齐 §14.2 get_capture_summary
    pub fn protocol_stats(&self, layer: Layer, top: usize) -> Vec<ProtocolCount>;
    pub fn conversations(&self, q: &ConversationQuery) -> Vec<ConversationDto>;   // sort_by/limit/filter
    pub fn filter_packets(&self, f: &PacketFilter, limit: usize) -> Vec<u64>;     // 返回 packet_index 列表
    pub fn inspect_packets(&self, idx: &[u64], preview_bytes: usize) -> Vec<PacketInspectDto>;
    pub fn reconstruct_stream(&self, q: &StreamQuery) -> Result<StreamPreview, PacketSageError>;
    //           ↑ M2 提供引擎侧实现；有界预览，payload 不入库
}
```
`PacketFilter` 字段与 §14.2 `filter_packets` 一致：`src_ip, dst_ip, src_port, dst_port, tcp_flags, protocol, time_start, time_end, limit`。
### 4.8 `pipeline` 与事件汇
```rust
pub trait EventSink {
    fn emit(&mut self, ev: &EngineEvent) -> std::io::Result<()>;
}
pub struct JsonlSink<W: Write> { /* stdout 或文件 */ }       // M1 验收主体
pub struct CountingSink { /* 人读模式：只计数 */ }
pub struct AnalyzePipeline {
    pub config: EngineConfig,
    pub sink: Box<dyn EventSink>,
}
impl AnalyzePipeline {
    /// 完整执行：open_reader → 逐包 decode → reassembly → aggregate
    /// → 结束时 flush SessionSummary/Stats 事件 → TaskFinished
    pub fn run(&mut self, input: &Path, task_id: TaskId) -> Result<TaskSummary, PacketSageError>;
}
```
`EngineConfig`（对齐 §24，`serde` 从 yaml/env 加载，优先级 CLI > env > file > default）：
```rust
pub struct EngineConfig {
    pub engine: EngineCfg,          // max_sessions 等
    pub reassembly: ReassemblyLimits,
    pub emit: EmitCfg,              // packet_events: All | ErrorsOnly | None（默认 All）
    pub storage: StorageCfg,        // M2 占位：url: sqlite://…（M3 起启用）
}
```
### 4.9 `error`：统一错误模型（对齐 §25）
```rust
#[derive(Debug, thiserror::Error)]
pub enum PacketSageError {
    #[error("invalid argument: {0}")]                    InvalidArgument(String),
    #[error("unsupported capture: {path}: {reason}")]    UnsupportedCapture { path: PathBuf, reason: String },
    #[error("capture corrupted at byte {offset}: {reason}")] CaptureCorrupted { offset: u64, reason: String },
    #[error("decode error packet #{packet_index} layer {layer}: {code:?}")]
        DecodeError { packet_index: u64, layer: Layer, code: DecodeErrorCode },
    #[error("rule error: {0}")]   RuleError(String),        // M3 起
    #[error("database error: {0}")] DatabaseError(String),  // M3 起
    #[error("llm error: {0}")]    LlmError(String),         // M4 起
    #[error("tool error: {0}")]   ToolError(String),        // M4 起
    #[error("internal error: {0}")] Internal(String),
}
```
**终止 vs 继续**【设计】：`reader` 内部把包级问题折叠为 `DecodeError`/`NonFatal`，只有**无法继续定位下一条记录**时才向上抛 `CaptureCorrupted`；`pipeline` 收到后者才终止任务并在 `TaskFinished` 中写 `error_code`。
---
## 5. `packetsage-cli`（L3）规格
### 5.1 命令树
```text
packetsage
├── analyze <capture>            --jsonl [--limit-events N] [--full] [--report <path>]
├── serve                        # JSONL RPC worker（stdin/stdout）
├── query <sessions|stats|alerts> ...
├── rules <list|check>           # M3 前输出 NotImplemented（exit 5）
├── chat <capture>               # M4 前输出 NotImplemented（exit 5）
├── db query --readonly          # M6
├── doctor
└── version                      # 同 --version
```
### 5.2 M0～M2 各命令交付状态
| 命令 | M0 | M1 | M2 |
|---|---|---|---|
| `version` / `doctor` | ✅ | ✅ | ✅ |
| `analyze --jsonl` | — | ✅ 输出 TaskStarted/CaptureInfo/Packet/DecodeError/TaskFinished | ✅ 追加 SessionSummary/Stats |
| `analyze`（人读） | — | ✅ 打印 summary（格式/包数/字节数/错误数） | ✅ 打印 Top conversations |
| `analyze --full --report` | — | — | 报告生成留 M5；M2 仅聚合+查询，`--report` 返回 exit 5 |
| `serve` | ✅ 仅响应 `ping`（返回版本/schema，验证 JSONL 通路） | ✅ + `analyze_file` | ✅ 全部查询方法（见下表） |
| `query sessions/stats` | — | — | ✅（读取本次或已落库任务） |
| `doctor` | 检查项：binary 版本、配置文件可解析、rules 目录存在、样本目录可读、数据库连接 | 同左 | 同左 |
### 5.3 `serve` RPC 方法表（M2 冻结）
| method | 参数（要点） | M2 状态 |
|---|---|---|
| `ping` | — | ✅ `{version, schema_version}` |
| `analyze_file` | `{path, emit}` | ✅ 返回 `task_id` + summary |
| `get_capture_summary` | `{task_id}` | ✅（对齐 §14.2 返回结构） |
| `get_protocol_stats` | `{task_id, layer, top}` | ✅ |
| `get_conversations` | `{task_id, sort_by, limit, filter}` | ✅ |
| `filter_packets` | `{task_id, …filter}` | ✅ |
| `inspect_packets` | `{task_id, packet_indices, preview_bytes}` | ✅ |
| `reconstruct_stream` | `{task_id, session_id, direction, max_bytes}` | ✅（预览有界：单结果 ≤ 64 KiB，§2.3） |
| `check_alerts` / `query_history` | — | 返回 `NOT_IMPLEMENTED`（M3/M4） |
单进程内 `serve` 持有 `task_id → AnalysisStore` 的内存映射（上限 8 个任务，LRU 淘汰）【设计】。
### 5.4 Exit Codes（对齐 §23，`cli::exit` 单点定义）
```rust
pub enum ExitCode { Success = 0, Usage = 1, CaptureError = 2, ConfigError = 3, Internal = 4, Unsupported = 5 }
```
映射：`PacketSageError::InvalidArgument → 1`；`UnsupportedCapture / CaptureCorrupted → 2`；`DecodeError` 不断链不置非零（任务正常结束，错误在事件流内）。
---
## 6. `packetsage-rules` 占位契约（M3 前冻结入口）
```rust
//! M0～M2：本 crate 编译为空库，仅保证 workspace 完整与 CLI rules 子命令占位。
//! M3 契约预告（不在本规格实现）：
//!   pub struct RuleEngine { … }
//!   impl RuleEngine {
//!       pub fn load_dir(&mut self, path: &Path) -> Result<usize, PacketSageError>;
//!       pub fn evaluate(&mut self, ev: &EngineEvent) -> Vec<AlertCandidate>;
//!   }
//! 依赖方向：rules → protocol（M3 起追加 rules → core 的 SessionEntry 只读视图）。
```
---
## 7. 工程规范
### 7.1 代码风格
- 命名、目录、pub API 一律 `rustfmt` + `clippy -D warnings`；
- 每个公开 API 必须有 doc-comment 与 `# Errors` 段（`missing_errors_doc = warn`）；
- crate 内部类型禁止直接暴露三方类型（`pcap_parser::*` / `etherparse::*`）到 pub 签名——转换在模块边界完成。
### 7.2 panic-free 执行策略
1. workspace 级 `unsafe_code = forbid`、`clippy::unwrap_used = deny`（见 §2.2）；
2. `cli::main` 不安装 `catch_unwind`（panic 即 bug，修复而非吞掉）；**解析路径上禁止任何可达 panic**，由 fuzz 目标背书（§8.4）；
3. 算术一律 `checked_/saturating_`（`integer_division = warn` 辅助审计 seq 运算）。
### 7.3 日志与诊断
- `tracing`：`INFO` 任务级里程碑、`DEBUG` block 级、`WARN` NonFatal 计数；
- **日志走 stderr，JSONL 事件走 stdout**——两者物理隔离，这是 `serve` 模式正确性的硬约束【设计】。
### 7.4 CI（`.github/workflows/ci.yml` 最小集）
```yaml
jobs:
  check:   cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
  test:    cargo test --workspace
  msrv:    cargo +1.80 check --workspace
  build:   cargo build --release
  fuzz-smoke: cargo fuzz build && 每目标 60s 短跑   # M2 起
```
---
## 8. 测试标准（M0～M2 范围）
### 8.1 单元测试清单
| 模块 | 必测点 |
|---|---|
| reader::detect_format | 4 种 pcap 魔数（两序 × 两精度）、SHB 魔数、垃圾头 → `UnsupportedCapture` |
| reader::ts | Decimal/Binary 两种 resolution、默认值、溢出边界 |
| reader::pcapng | 多 Section（跨 section 字节序变化）、多接口、未知 block 计数 |
| model::SessionKey | 同会话双向 `canonicalize` 结果相等；V4/V6 混合排序 |
| decoder | 矩阵每协议正例 + `truncated_header`/`bad_length`/`unsupported_linktype` 负例 |
| reassembly | 见 8.3 用例表 |
| conversation | flags 计数、bytes 双向、app_protocol 首识别 |
### 8.2 Golden tests
- 样本入 `tests/fixtures/`，期望结果入 `tests/golden/<name>.golden.json`；
- 比较前做 normalize：`task_id` 注入固定值 `task_TEST00000000000000000000`；其余字段（含 ts、索引、计数）**逐字节比较**；
- 增量协议：先跑 `cargo insta`-风格 review，接受后固化为 golden，人工 diff。
### 8.3 M2 重组验收用例表（对齐 §10 与 §32-M2）
| 用例 | 输入 seq/data | 期望 |
|---|---|---|
| retrans-exact | 1000-1099, 1000-1099 | 第二段 = Retransmission；payload 只含一份 |
| reorder-fill | 1000-1099, 1200-1299, 1100-1199 | 最终 `take_contiguous` = 1000-1299 连续；`missing_ranges` 空 |
| reorder-missing | 1000-1099, 1200-1299（截止） | state = Incomplete；`missing_ranges` = [1100-1200) |
| overlap-firstwins | 1000-1099, 1050-1149（内容不同） | 1050-1099 保留首见内容；`PartialOverlap{dropped:50}` |
| buffer-overflow | 单流累计 > 8MiB 未消费 | `BufferOverflow` + `StreamStateEvent`；任务不崩溃 |
| session-cap | > max_sessions 新五元组 | 新流 Dropped(SessionLimit)，计数告警 |
| timeout | 流 last_ts 距 now > 60s | `prune_expired` 后释放，状态 Closed(Timeout) |
| dup-五元组 | 同一对端双向 1000 包 | 恰好 1 个 session，src/dst 计数互补 |
### 8.4 Fuzz 目标（M2 建目录，`cargo-fuzz`）
`fuzz_pcap`、`fuzz_pcapng`、`fuzz_decoder`、`fuzz_config_yaml`（规则/RPC fuzz M3 起）。**验收：任何输入零 panic**。
---
## 9. 里程碑 DoD（验收检查表）
### M0 — Skeleton
- [ ] workspace 四 crate 编译通过，lint 全绿（含 forbid unwrap）
- [ ] `cargo run -p packetsage-cli -- version` 与 `doctor` 可执行且退出码 0
- [ ] `serve` 能完成 `ping` RPC（JSONL 往返，stderr/stdout 隔离有效）
- [ ] CI 五 job 全绿
### M1 — Parser
- [ ] `analyze <pcap> --jsonl` / `analyze <pcapng> --jsonl` 输出符合 §3.2 schema（`cargo run -- schema 验证脚本` 全部通过）
- [ ] §8.1 reader/decoder 单测全绿；golden ≥ 3 个真实样例
- [ ] 构造坏包：`decode_error` 事件后任务继续，`TaskFinished.packets` 含坏包计数
- [ ] snaplen 截断包带 `truncated: true` 事件字段
### M2 — Analyzer
- [ ] §8.3 用例表全部通过（重传/乱序/缺口/超限）
- [ ] 同一五元组双向 1000 包 → 恰好 1 个 `SessionSummary`（**M2 核心验收**，对齐 §32）
- [ ] `analyze --jsonl` 事件流含 Stats（协议计数 + Top conversations）
- [ ] `serve` 查询方法表全部可往返调用
- [ ] RSS：1 GB 样例 parse + aggregate ≤ 1 GB【待验证，初测记录】
---
## 10. 实现顺序 Checklist（严格串行，每项可独立合入）
```text
01  workspace + lints + CI + doctor 骨架
02  protocol: events + rpc + ids（含 schema 导出测试）
03  core::error + config
04  core::model（SessionKey 规范化 + 单测）
05  core::reader::detect_format + LegacyPcapReader + ts 换算
06  core::reader::PcapNgReader（Section/Interface 状态机 + 未知 block）
07  core::decoder（etherparse 对接 + 矩阵负例）
08  pipeline + JsonlSink          ← M1 验收点
09  core::reassembly（含 8.3 用例 TDD）
10  core::conversation
11  core::query + Stats 事件      ← M2 验收点
12  cli: serve 查询方法 + query 子命令
13  golden/fuzz 目录固化，MSRV 实测回填
```
---
## 附：与开发文档的偏差记录
| #   | 偏差                                                                                           | 理由                                    | 状态       |
| --- | -------------------------------------------------------------------------------------------- | ------------------------------------- | -------- |
| D-1 | 新增 `packetsage-protocol` 为独立 L0 crate（开发文档 §5.3 规划在 protocol crate 内，本规格进一步细化为 workspace 成员） | 使 Python 侧可 `pip` 复用同一 schema 定义      | 【设计】建议采纳 |
| D-2 | 重组超限策略明确为"丢最旧 + BufferOverflow 状态"，开发文档 §10.5 留白                                             | 实现需唯一确定行为并测试                          | 【设计】     |
| D-3 | reader 支持 Linux SLL（linktype 113）                                                            | etherparse 0.19 `from_linux_sll` 免费获得 | 【事实】     |
| D-4 | `analyze --report` 在 M2 返回 exit 5 | report 属 M5 范围 | 【设计】 |
---
这份规格书可直接作为 M0 的第一个 PR（§2 workspace + §7.4 CI）与 M1/M2 的验收依据。如需下一步，我可以按此规格输出 `packetsage-protocol` 的完整可编译代码，或先生成 §8.3 的重组测试用例代码骨架。
