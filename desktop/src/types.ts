/** 通道 B（§4.5）与通道 A（§5.4/§5.10）的数据形状。 */

export type Severity = "high" | "medium" | "low" | "info";

export type Basis =
  | "rule_match"
  | "direct_observation"
  | "correlated_observation"
  | "hypothesis";

/** `hello` 的 ack 载荷（§4.3）。 */
export interface Welcome {
  protocol_version: number;
  agent_version: string;
  schema_version: number;
  capabilities: string[];
  providers: { kind: string; model: string; configured: boolean };
}

export interface Budget {
  max_steps: number;
  max_llm_calls: number;
  max_tool_calls: number;
  max_tokens: number;
  max_cost_cents: number;
}

export interface BudgetState {
  steps: number;
  llm_calls: number;
  tool_calls: number;
  tokens_in: number;
  tokens_out: number;
  cost_cents: number;
}

/**
 * `llm_round`（§4.5）：预算 + 耗时 + **输入侧缓存命中情况**。
 *
 * `step` 是协议里的字段名（本地状态叫 `steps`，两个都认）；`cache_*` 是 DeepSeek
 * 上下文硬盘缓存的 `usage.prompt_cache_hit_tokens / prompt_cache_miss_tokens`，
 * provider 不报这两个数时是 0——界面要能分辨"没数据"和"没命中"。
 */
export interface LlmRound {
  step?: number;
  steps?: number;
  llm_calls: number;
  tool_calls: number;
  tokens_in: number;
  tokens_out: number;
  cost_cents: number;
  llm_ms: number;
  tool_ms: number;
  preloaded: number;
  cache_hit_tokens?: number;
  cache_miss_tokens?: number;
}

/** `llm_round` 上的耗时列（性能改造 #1）：模型慢还是工具慢，一眼分得开。 */
export interface RunTimings {
  llm_ms: number;
  tool_ms: number;
  preloaded: number;
}

/** 工具卡片：`tool_call_finished.data`（字段 = `ToolCallRecord`）。 */
export interface ToolCall {
  step: number;
  tool_name: string;
  args: unknown;
  status: string;
  duration_ms: number;
  tc_id: string | null;
  result_summary: string;
  /** 只在前端存在：`tool_call_started` 先到、`finished` 还没到。 */
  pending?: boolean;
}

export interface FindingSummary {
  id: string;
  severity: Severity;
  basis: Basis;
  title: string;
  evidence_ids: string[];
}

export interface Finding {
  finding_id: string;
  task_id: string;
  title: string;
  severity: Severity;
  basis: Basis;
  summary: string;
  validator_status: string;
  evidence: { _id: string; method: string; ref_id?: string | null }[];
}

export interface Alert {
  alert_id: string;
  rule_id: string;
  rule_version: number;
  rule_content_hash: string;
  severity: Severity;
  first_packet: number;
  last_packet: number;
  session_id?: string | null;
  group_key?: string[][];
  evidence: {
    metric?: string;
    value?: number;
    threshold?: number;
    window_ns?: string;
    operator?: string;
    sample_packets?: number[];
    degraded?: boolean;
  };
}

export interface TraceEntry {
  _id: string;
  method: string;
  ts_unix_ns: string;
  args: string;
  duration_ms: number;
  status: string;
  numbers?: Record<string, number>;
  ref_ids?: string[];
}

export interface CaptureSummary {
  task_id: string;
  source_path?: string;
  source_sha256?: string;
  format?: string;
  packets?: number;
  bytes?: number;
  sessions?: number;
  alerts?: number;
  decode_errors?: number;
  truncated_packets?: number;
  incomplete_sessions?: number;
  dropped_sessions?: number;
  duration_s?: number;
  protocols?: Record<string, number>;
}

export interface TaskOverview {
  summary: CaptureSummary;
  alerts: Alert[];
  findings: Finding[];
  trace: TraceEntry[];
  artifacts: {
    report_path?: string | null;
    report_meta?: Record<string, unknown> | null;
    [key: string]: unknown;
  } | null;
}

export interface TaskRow {
  id: string;
  status?: string;
  source_path?: string;
  packet_count?: number;
  byte_count?: number;
  started_at?: string;
}

/** `error` 事件 / ack 的 error 对象（§4.8）。 */
export interface ProtocolError {
  code: string;
  message: string;
  retryable: boolean;
  where: string;
}

export interface RunFinished {
  summary_version: number;
  run_id: string;
  task_id: string;
  status: "completed" | "degraded" | "failed";
  stop_reason: string | null;
  accepted: number;
  submit_rejects: number;
  malformed_output: boolean;
  steps: number;
  calls: number;
  tool_calls: number;
  tokens: { in: number; out: number; total: number };
  cost_cents: number;
  prompt_version: string;
  model: string;
  provider: string;
  /** 模型自己写的一段总结（对话里"它说了什么"）；模型没给就是空串。 */
  summary?: string;
  findings: FindingSummary[];
  /** 输入侧缓存命中情况（DeepSeek 上下文硬盘缓存）；provider 不报就是 0/0。 */
  cache?: { hit: number; miss: number };
  error?: ProtocolError;
}

export interface AgentEvent {
  type: "event";
  event: string;
  run_id: string;
  seq: number;
  ts_unix_ms: number;
  data: Record<string, unknown>;
}

/**
 * `llm_delta`（§4.5）：token 级流式的一段。
 *
 * 合并后的碎块，不是单个 token（agent 侧按 120 ms / 256 字符先到者 flush）；
 * `channel` 是 `content`（模型正在写正文）/ `reasoning`（推理模型的自述）/
 * `tool_args`（正在拼工具参数，`tool_name` 一并给出）。
 */
export interface LlmDelta {
  step: number;
  channel: string;
  text: string;
  tool_name: string | null;
  tool_index: number | null;
}

export interface ReportResult {
  report_path: string;
  sha256: string;
  degraded: boolean;
  unverified_items: number;
  status?: string;
  template_version?: string;
  prompt_version?: string;
}

export type RunStatus =
  | "idle"
  | "analyzing"
  | "running"
  | "finalizing"
  | "completed"
  | "degraded"
  | "failed";
