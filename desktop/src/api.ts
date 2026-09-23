import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AgentEvent,
  Budget,
  ReportResult,
  TaskOverview,
  TaskRow,
  Welcome,
} from "./types";

export interface AppInfo {
  engine: string;
  agent: string;
  engine_program: string;
  agent_program: string;
  db_url: string;
  data_dir: string;
  engine_alive: boolean;
  agent_alive: boolean;
  /** `非空` = sidecar 没起来（P5）：界面显示"安装损坏"并禁用调查。 */
  start_error: string | null;
}

export interface DoctorItem {
  id: string;
  name: string;
  status: "ok" | "warn" | "fail" | "skipped";
  detail: string;
  repair_hint: string;
}

export interface Diagnostics {
  start_error: string | null;
  engine: { alive: boolean; pid: number | null; stderr_tail: string[] };
  agent: { alive: boolean; pid: number | null; stderr_tail: string[] };
  /** P8：事件队列深度与累计丢弃数。 */
  events: { queued: number; dropped: number; capacity: number };
}

/** `status` 命令（§4.9 规则 3 的心跳）。 */
export interface AgentStatus {
  engine_alive: boolean;
  run: {
    run_id: string;
    task_id: string;
    mode: string;
    steps: number;
    llm_calls: number;
    tool_calls: number;
    tokens_in: number;
    tokens_out: number;
    cost_cents: number;
    cancel_requested: boolean;
  } | null;
  budget: Budget | null;
}

/** `sidecar-exited` 的载荷：`[label, exit_code]`（通道 C 的崩溃感知，P2）。 */
export type SidecarExited = [string, number | null];

/** U7：向导要的状态（配过什么、key 在不在）。 */
export interface ProviderStatus {
  configured: boolean;
  provider: string;
  model: string;
  base_url: string;
  has_key: boolean;
  /** 思考模式：`""` = 自动（跟随厂商默认）/ `enabled` / `disabled`。 */
  thinking: string;
  config_path: string;
}

/** `setup --verify` 的结果（§7.4 第 2 步的 `GET /models`）。 */
export interface VerifyResult {
  ok: boolean;
  detail: string;
  provider: string;
  model: string;
  base_url: string;
}

export interface ProviderSaveResult {
  provider: string;
  model: string;
  base_url: string;
  key_stored: boolean;
  welcome: Welcome;
}

/** 订阅 sidecar 退出；返回退订函数（`useEffect` 里直接用）。 */
export function onSidecarExited(
  handler: (event: SidecarExited) => void,
): Promise<UnlistenFn> {
  return listen<SidecarExited>("sidecar-exited", (message) => handler(message.payload));
}

/** 通道 C：对 Rust 命令的薄封装（错误一律是可读字符串）。 */

/** 一次 run：ack 立刻回来，事件走通道（U4：卡片逐条出现，不阻塞界面）。 */
export async function startRun(
  taskId: string,
  goal: string,
  mode: "run" | "chat",
  onEvent: (event: AgentEvent) => void,
): Promise<{ run_id: string }> {
  const channel = new Channel<AgentEvent>();
  channel.onmessage = (message) => {
    if (message && message.type === "event") {
      onEvent(message);
    }
  };
  return invoke<{ run_id: string }>("run_agent", {
    taskId,
    goal,
    mode,
    channel,
  });
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  handshake: () => invoke<Welcome>("handshake"),
  doctor: () => invoke<{ items: DoctorItem[] }>("doctor"),
  listTasks: () => invoke<TaskRow[]>("list_tasks"),
  analyze: (path: string) =>
    invoke<{ task_id: string; summary: TaskOverview["summary"] }>("analyze", { path }),
  overview: (taskId: string) => invoke<TaskOverview>("task_overview", { taskId }),
  cancel: () => invoke<Record<string, never>>("cancel"),
  report: (taskId: string) => invoke<ReportResult>("report", { taskId }),
  readText: (path: string) => invoke<string>("read_text_file", { path }),
  diagnostics: () => invoke<Diagnostics>("sidecar_diagnostics"),
  /** 心跳（§4.9 规则 3）：连续两次失败判为不健康。 */
  status: () => invoke<AgentStatus>("agent_status"),
  /** U7：配过什么、key 在不在凭据管理器里。 */
  providerStatus: () => invoke<ProviderStatus>("provider_status"),
  /** U7：`GET /models` 校验（走打包 Agent 自己的 `setup --verify`）。 */
  providerVerify: (input: {
    provider: string;
    model: string;
    baseUrl: string;
    apiKey: string | null;
    thinking?: string;
  }) =>
    invoke<VerifyResult>("provider_verify", {
      provider: input.provider,
      model: input.model,
      baseUrl: input.baseUrl,
      apiKey: input.apiKey,
      thinking: input.thinking ?? null,
    }),
  /** U7：存进凭据管理器 + `provider.json`，然后只重启 Agent。 */
  providerSave: (input: {
    provider: string;
    model: string;
    baseUrl: string;
    apiKey: string | null;
    thinking?: string;
  }) =>
    invoke<ProviderSaveResult>("provider_save", {
      provider: input.provider,
      model: input.model,
      baseUrl: input.baseUrl,
      apiKey: input.apiKey,
      thinking: input.thinking ?? null,
    }),
  providerClear: () => invoke<{ removed_key: boolean; welcome: Welcome }>("provider_clear"),
  /** 模型下拉的数据源：`GET /models` 的模型名列表（拉不到就是空数组）。 */
  providerModels: () => invoke<string[]>("provider_models"),
  /** U6：一键重建 Agent（引擎不动，已分析的抓包还在）。 */
  restartAgent: () => invoke<Welcome>("restart_agent"),
};
