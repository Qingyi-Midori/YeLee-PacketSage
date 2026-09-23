# ADR-025：`DirectionBasis::FirstPacket` 改名 `FirstSeen`

**状态：已采纳**（关闭 `docs/architecture.md` 中的偏差 D-7）

**背景：** M2v0.2 §4.3 的权威枚举是
`DirectionBasis { SynFirst, PortHeuristic, FirstSeen, ConfigOverride }`
（`serde(rename_all = "snake_case")`）。实现里自拟的 `first_packet` 语义有歧义
（读起来像 packet index），且 `sessions.direction_basis` 尚未落库，改名成本为零。

**决议：** 枚举与线上值改为
`syn_first | port_heuristic | first_seen | config_override`；当前聚合器只产出
`syn_first` 与 `first_seen`，另两个保留给端口启发式与配置覆盖。

**实现落点：** `packetsage_protocol::events::DirectionBasis`（含
`direction_basis_wire_values_match_the_frozen_enum` 单测）、
`packetsage_core::conversation`、`packetsage-cli::persist`、
`agent/packetsage_agent/report.py` 第 4 节。
