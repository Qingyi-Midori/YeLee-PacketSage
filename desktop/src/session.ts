/**
 * 会话流的两条**纯逻辑**（不碰 React、不碰 Tauri，改完能单独验算）：
 *
 * * **轮次装配**——历史按 `task_id` 分桶存在本机 `localStorage`，读回来之前先过一遍
 *   `sanitizeTurns()`：只认"真跑过的轮次"（有过程、有结论，或者已收尾）。老版本
 *   每次打开抓包都留下的「打开 XXX」空壳就在这里丢掉（2026-09-23）。
 * * **时间线次序**——同一轮里 `调用` 在最上、`思考` 夹中间、`输出` 在最下。
 */

import type { FeedItem, Lane } from "./components";
import type { FindingSummary, RunFinished, RunStatus } from "./types";

/**
 * 一轮对话（用户一句 + Agent 这一段）。
 *
 * 引擎库里没有对话文本（`agent_runs` 只存模型/状态/耗时/费用，没有 goal），
 * 所以历史由桌面端自己存：**当前这一轮**在内存里逐事件生长，切到下一轮时
 * 归档进 `history` 并落到 localStorage（按 task 分桶）。
 */
export interface Turn {
  id: string;
  goal: string;
  mode: "run" | "chat";
  startedAt: number;
  status: RunStatus;
  feed: FeedItem[];
  findings: FindingSummary[];
  finished: RunFinished | null;
  rejected: { title: string; reason: string; code: string }[];
}

function readFeed(turn: Partial<Turn>): FeedItem[] {
  return Array.isArray(turn.feed) ? turn.feed : [];
}

function readFindings(turn: Partial<Turn>): FindingSummary[] {
  return Array.isArray(turn.findings) ? turn.findings : [];
}

/**
 * 这一轮**真的跑过**吗？
 *
 * 判据是"留下过东西"：过程（工具卡片 / 步骤 / 原文）、结论，或者已经收尾。
 * 三者都没有的，就是"打开抓包"那种空壳——它既没有 run 也没有 chat，
 * Hero 只能落在"还没有结论"的空态上，留在会话流里纯属噪音。
 */
export function isRealTurn(turn: unknown): turn is Turn {
  if (!turn || typeof turn !== "object") return false;
  const candidate = turn as Partial<Turn>;
  if (typeof candidate.id !== "string" || !candidate.id) return false;
  return Boolean(
    readFeed(candidate).length || readFindings(candidate).length || candidate.finished,
  );
}

/** 老数据可能缺字段：补齐数组与 mode，渲染时就不用到处判空。 */
function normalizeTurn(turn: Turn): Turn {
  return {
    ...turn,
    goal: typeof turn.goal === "string" ? turn.goal : "",
    mode: turn.mode === "chat" ? "chat" : "run",
    startedAt: Number(turn.startedAt) || 0,
    feed: readFeed(turn),
    findings: readFindings(turn),
    rejected: Array.isArray(turn.rejected) ? turn.rejected : [],
  };
}

/** 从本机读回来的一桶历史：丢掉空壳轮次，并把老数据补齐。 */
export function sanitizeTurns(raw: unknown): Turn[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter(isRealTurn).map(normalizeTurn);
}

/** 时间线上的最小形状：只需要判得出"这是哪条泳道、是不是一轮的开头"。 */
type TimelineEntry = {
  kind: string;
  channel?: string;
  call?: { tool_name?: string };
  step?: number;
};

/** 泳道自上而下的次序（用户 2026-09-23 定）：调用 → 思考 → 输出 → 结论 → 拒绝。 */
const LANE_RANK: Record<string, number> = {
  调用: 0,
  思考: 1,
  输出: 2,
  结论: 3,
  拒绝: 4,
};

/**
 * `llm_delta` 的 `channel` → 泳道。**这是全项目唯一一份映射**：
 * 时间线的排序（本文件）与渲染标签（`components.tsx` 的 `deltaLane`）都从这里取，
 * 免得"排一种、画另一种"。
 */
export function deltaLane(channel: string): Lane {
  if (channel === "tool_args") return "调用";
  if (channel === "reasoning") return "思考";
  return "输出";
}

/** 一条过程属于哪条泳道（`步骤` 不参与泳道排序，它是每一块的块头）。 */
export function entryLane(item: TimelineEntry): Lane | "步骤" {
  if (item.kind === "tool") return "调用";
  if (item.kind === "delta") return deltaLane(item.channel ?? "content");
  if (item.kind === "finding") return "结论";
  if (item.kind === "rejected") return "拒绝";
  return "步骤";
}

/**
 * 时间线的次序：按**轮**分块（`第 N 步` 那一行是块头），块内按泳道自上而下排。
 *
 * 块与块之间保持先来后到——引擎预取那一块仍然在最前面，模型第 1、2、3 轮依次往下，
 * 不会因为排序把整条时间轴的先后关系搅乱。块内用稳定排序，同泳道仍按到达顺序。
 */
export function orderTimeline<T extends TimelineEntry>(items: T[]): T[] {
  const blocks: T[][] = [];
  /** 当前这一块里有没有"过程"（只有块头的那种块，不跟上一块分开）。 */
  let hasBody = true;
  for (const item of items) {
    const startsRound = item.kind === "step";
    const block = blocks[blocks.length - 1];
    if (!block || (startsRound && hasBody)) {
      blocks.push([item]);
      hasBody = !startsRound;
      continue;
    }
    block.push(item);
    if (!startsRound) hasBody = true;
  }
  const ordered: T[] = [];
  for (const block of blocks) {
    const ranked = block.map((item, index) => ({
      item,
      index,
      // 块头（"第 N 步"）压在所有泳道上面。
      rank: item.kind === "step" ? -1 : (LANE_RANK[entryLane(item)] ?? 99),
    }));
    ranked.sort((left, right) => left.rank - right.rank || left.index - right.index);
    for (const row of ranked) ordered.push(row.item);
  }
  return ordered;
}
