/**
 * 渲染件：会话流里的一条条（《GUI 工程规格书 v0.2》§5 的设计基线）。
 *
 * 三条界面纪律（对应这次重设计）：
 *
 * 1. 结论卡是全屏唯一的视觉主角，其余信息一律降级成可展开的一条 `.block`；
 * 2. 状态只有三种颜色语义——无异常（绿）/ 调查中（蓝）/ 未开始（灰），
 *    失败与降级另有红/琥珀，但不参与"正常扫读"的层级；
 * 3. 结论、告警、空状态都用人的语言，`tc_*` 这类锚点默认截断、藏进展开项。
 */

import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useRef, useState } from "react";

import type {
  Alert,
  CaptureSummary,
  Finding,
  FindingSummary,
  ReportResult,
  Severity,
  ToolCall,
  TraceEntry,
} from "./types";
import { deltaLane as laneOfDelta } from "./session";

const SEVERITY_LABEL: Record<Severity, string> = {
  high: "严重",
  medium: "中等",
  low: "轻微",
  info: "提示",
};

const SEVERITY_CLASS: Record<Severity, string> = {
  high: "sev-high",
  medium: "sev-medium",
  low: "sev-low",
  info: "sev-info",
};

const BASIS_LABEL: Record<string, string> = {
  rule_match: "规则命中",
  direct_observation: "直接观测",
  correlated_observation: "交叉观测",
  hypothesis: "待人工确认",
};

export type Tone = "ok" | "busy" | "idle" | "warn" | "bad";

export function Mono({ children }: { children: React.ReactNode }) {
  return <code className="mono">{children}</code>;
}

/** 状态药丸：绿=无异常、蓝=进行中、灰=未开始、琥珀=降级、红=失败。 */
export function StatusPill({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return <span className={`pill pill-${tone}`}>{children}</span>;
}

/** 证据锚点：默认截断，hover 给全量（`tc_*` 是给人核对用的，不是给人读的）。 */
export function AnchorId({
  id,
  onSelect,
}: {
  id: string;
  onSelect?: (id: string) => void;
}) {
  return (
    <button className="anchor-id" title={id} onClick={() => onSelect?.(id)}>
      {id.length > 14 ? `${id.slice(0, 14)}…` : id}
    </button>
  );
}

export function SeverityBadge({ severity }: { severity: Severity }) {
  return (
    <span className={`badge ${SEVERITY_CLASS[severity] ?? "sev-info"}`}>
      {SEVERITY_LABEL[severity] ?? severity}
    </span>
  );
}

export function Banner({
  kind,
  title,
  detail,
  action,
  onAction,
  secondary,
  onSecondary,
}: {
  kind: "error" | "warn" | "info";
  title: string;
  detail?: string;
  /** 次要（但更要紧）的那个动作，比如"前往配置"——只给"知道了"等于把人堵在死路上。 */
  secondary?: string;
  onSecondary?: () => void;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className={`banner banner-${kind}`}>
      <strong>{title}</strong>
      {detail ? <pre className="banner-detail">{detail}</pre> : null}
      {secondary || action ? (
        <div className="banner-actions">
          {secondary && onSecondary ? (
            <button onClick={onSecondary}>{secondary}</button>
          ) : null}
          {action && onAction ? (
            <button className="ghost" onClick={onAction}>
              {action}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 报告预览：九节 Markdown 渲染成文档，而不是把 `##` 原文糊在等宽盒子里。
 *
 * 报告模板本身是英文的（`docs/report-spec.md` 冻结了九节标题），所以这里只做
 * **显示层**的对照：中文在前、英文术语留在括号里，术语不丢、人也读得懂。
 */
export function MarkdownView({ text }: { text: string }) {
  const localized = text.replace(/^#{1,3} .+$/gm, (line) => {
    const level = line.match(/^#+/)?.[0].length ?? 1;
    const title = line.replace(/^#+ /, "").trim();
    const chinese = SECTION_ZH[title];
    return chinese ? `${"#".repeat(level)} ${chinese} · ${title}` : line;
  });
  return (
    <div className="md">
      <Markdown remarkPlugins={[remarkGfm]}>{localized}</Markdown>
    </div>
  );
}

/** 九节标题的中英对照（只影响显示；报告文件本身不动）。 */
const SECTION_ZH: Record<string, string> = {
  "PacketSage Analysis Report": "PacketSage 分析报告",
  "1. Executive Summary": "结论摘要",
  "2. Capture Overview": "抓包概览",
  "3. Protocol Statistics": "协议统计",
  "4. Top Conversations": "主要会话",
  "5. Rule Alerts": "规则告警",
  "6. Agent Findings": "调查结论",
  "7. Evidence": "证据",
  "8. Limitations": "局限",
  "9. Analysis Trace": "分析台账",
};

/**
 * 结论 = **Hero**（v0.4 重做）：没有边框、没有底色，一句话就是主角。
 *
 * 原来那版是"带边框的文本块 + 右上角药丸 + 彩色按钮"，三样都在抢注意力：
 * 现在状态走图标化徽章、动作只留一个次级描边按钮、统计上提到结论正下方。
 */
export function ConclusionCard({
  tone,
  title,
  detail,
  badge,
  stats,
  actions,
  children,
}: {
  tone: Tone;
  title: string;
  detail?: string;
  badge?: React.ReactNode;
  /** 结论正下方的关键数字（包 / 会话 / 步…），等宽数字、一行读完。 */
  stats?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <section className={`hero hero-${tone}`}>
      <div className="hero-head">
        <div className="hero-main">
          {badge ? <div className="hero-badge-row">{badge}</div> : null}
          <h1 className="hero-title">{title}</h1>
          {detail ? <p className="hero-detail">{detail}</p> : null}
          {stats ? <div className="hero-stats">{stats}</div> : null}
        </div>
        {actions ? <div className="hero-actions">{actions}</div> : null}
      </div>
      {children ? <div className="hero-body">{children}</div> : null}
    </section>
  );
}

/**
 * 图标化状态徽章：**彩色只表达状态**，所以它就是"图标 + 一句话"，没有边框与底色。
 *
 * 绿=无异常、红=失败、琥珀=需要注意/降级、灰=未开始、蓝=调查中。
 */
export function HeroBadge({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  const icon = tone === "ok" ? "🛡" : tone === "bad" ? "⚠" : tone === "warn" ? "▲" : tone === "busy" ? "◐" : "○";
  return (
    <span className={`hero-badge hero-badge-${tone}`}>
      <span className="hero-badge-icon" aria-hidden="true">
        {icon}
      </span>
      {children}
    </span>
  );
}

/**
 * 等宽数字的 stat 块（v0.4）：把"2 步、2 次工具调用（另有 4 次引擎预取）· 9.1k tokens ·
 * 0.00 元 · 4.0 s 模型 / 0 ms 工具"这种一串间隔号的混合句，拆成能扫读的块。
 */
export function StatRow({
  items,
  compact,
}: {
  items: { label: string; value: string; hint?: string }[];
  compact?: boolean;
}) {
  return (
    <div className={`statrow${compact ? " statrow-compact" : ""}`}>
      {items.map((item) => (
        <div className="statrow-item" key={item.label} title={item.hint}>
          <span className="statrow-value">{item.value}</span>
          <span className="statrow-label">{item.label}</span>
        </div>
      ))}
    </div>
  );
}

/** 结论里的一行：严重度 + 一句话 + 证据入口。 */
export function FindingRow({
  finding,
  evidenceCount,
  onEvidence,
  detail,
}: {
  finding: FindingSummary;
  evidenceCount: number;
  onEvidence?: () => void;
  /** 模型为这条结论写的那句话（来自引擎库里的 finding.summary）。 */
  detail?: string;
}) {
  const basis = BASIS_LABEL[finding.basis] ?? finding.basis;
  // 标题本身就写着"规则命中 XXX"时不再重复一遍依据（截图里最刺眼的一处）。
  const showBasis = !finding.title.includes(basis);
  return (
    <div className={`finding-row finding-${finding.severity}`}>
      <span className="dot" />
      <div className="finding-col">
        <div className="finding-row-main">
          <span className="finding-row-title">{finding.title}</span>
          {showBasis ? <span className="muted">{basis}</span> : null}
        </div>
        {detail ? <p className="finding-detail muted">{detail}</p> : null}
      </div>
      {evidenceCount ? (
        <button className="chip" onClick={onEvidence} title="看这条结论的证据">
          证据 {evidenceCount}
        </button>
      ) : null}
      <SeverityBadge severity={finding.severity} />
    </div>
  );
}

/** 规则告警的一行（引擎视角，和结论分开放）。 */
export function AlertRow({ alert }: { alert: Alert }) {
  const metric = alert.evidence?.metric;
  const value = alert.evidence?.value;
  const threshold = alert.evidence?.threshold;
  const window = alert.evidence?.window_ns;
  return (
    <div className={`finding-row finding-${alert.severity}`}>
      <span className="dot" />
      <div className="finding-row-main">
        <span className="finding-row-title">
          {metric ? `${metric} 达到 ${fmt(value)}` : alert.rule_id}
          {threshold !== undefined ? `（阈值 ${fmt(threshold)}）` : ""}
        </span>
        <span className="muted">
          规则 <Mono>{alert.rule_id}</Mono> · 包 {alert.first_packet}–{alert.last_packet}
          {window ? ` · 窗口 ${fmtWindow(window)}` : ""}
          {alert.evidence?.degraded ? " · 证据降级" : ""}
        </span>
      </div>
      <SeverityBadge severity={alert.severity} />
    </div>
  );
}

/** 机器说的状态不是给人读的：`ok` / `preload` / `invalid_args` 一律翻成人话。 */
const TOOL_STATUS_LABEL: Record<string, string> = {
  running: "进行中",
  ok: "完成",
  // 预取是引擎替模型先读一步，对用户就是"这一步已经读过了"。
  preload: "完成",
  invalid_args: "参数不对",
  error: "失败",
};

/**
 * 一次工具调用 —— 时间线上**可展开的一步**（v0.4：工具是本产品的差异点，
 * 点开就能看见入参与结果，而不是只给一行摘要）。
 */
export function ToolCard({ call }: { call: ToolCall }) {
  const args = typeof call.args === "string" ? call.args : JSON.stringify(call.args ?? {});
  const done = call.status === "ok" || call.status === "preload";
  return (
    <details className={`tool ${call.pending ? "tool-pending" : ""}`}>
      <summary className="tool-head">
        <span className="caret">▸</span>
        <Mono>{call.tool_name}</Mono>
        <span className="spacer" />
        <span className={done ? "ok" : "warn"}>
          {call.pending ? "进行中" : (TOOL_STATUS_LABEL[call.status] ?? call.status)}
        </span>
      </summary>
      <div className="tool-body">
        <div className="muted">{args}</div>
        {call.result_summary ? <div>{call.result_summary}</div> : null}
        {call.tc_id ? (
          <div className="muted">
            证据锚点 <AnchorId id={call.tc_id} />
          </div>
        ) : null}
      </div>
    </details>
  );
}

/**
 * 空态里的建议问题（§5.1：空态给建议问题，而不是让人对着空白想措辞）。
 *
 * 点了就是一次 `mode=chat` 的追问，和底部输入框走同一条路。
 */
export function Suggestions({
  items,
  onPick,
  disabled,
}: {
  items: string[];
  onPick: (text: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="suggestions">
      {items.map((text) => (
        <button
          key={text}
          className="suggestion"
          onClick={() => onPick(text)}
          disabled={disabled}
        >
          {text}
        </button>
      ))}
    </div>
  );
}

/** 一次调查里给用户的建议问题：都是引擎真的能回答的那种。 */
export const SUGGESTED_QUESTIONS = [
  "这份抓包里有扫描行为吗",
  "哪些会话不完整",
  "有没有可疑的 DNS 行为",
];

/**
 * 会话流里的一条（§4.5 的事件粒度）。
 *
 * 规格明确**不做 token 级流式**（provider 单次请求、无 SSE），所以"流式"是
 * 这一层的事：一次 run 的事件按发生顺序追加，每条占一行——模型每一轮往返、
 * 每个工具的开始与完成、每条结论落地，都能眼看着往下长。
 */
export type FeedItem =
  | {
      kind: "step";
      step: number;
      /** 这一轮模型还没回话（`llm_round_started` 之后、`llm_round` 之前）。 */
      pending?: boolean;
      llmMs: number;
      toolMs: number;
      tokens: number;
      costCents: number;
      /** 累计的输入侧缓存命中/未命中 tokens（DeepSeek 上下文硬盘缓存）。 */
      cacheHit: number;
      cacheMiss: number;
    }
  | {
      kind: "delta";
      step: number;
      channel: string;
      toolName: string | null;
      text: string;
      /** 这一轮已经结束：原文收进折叠项，不再当"正在打字"渲染。 */
      done: boolean;
    }
  | { kind: "tool"; call: ToolCall }
  | { kind: "finding"; finding: FindingSummary }
  | { kind: "rejected"; title: string; reason: string; code: string };

/**
 * 三条泳道：**思考 / 调用 / 输出**（用户要的"看得见 LLM 在干什么"）。
 *
 * * 思考 = 模型自己的推理内容（`reasoning_content`，推理模型才有）；
 * * 调用 = 选工具 + 拼参数 + 工具结果；
 * * 输出 = 模型写正文（收尾那一轮的结论 JSON）。
 */
export type Lane = "思考" | "调用" | "输出" | "结论" | "拒绝";

/**
 * 推理强度档位（DeepSeek `reasoning_effort`，见 thinking_mode 指南）。
 *
 * 界面参考 Codex 风格的"模型 + 推理强度"控件（`HanaAyane/dsh-reasoning-effort`）：
 * 档位是**一条连续轴**，不是开关；`自动` = 不显式传，跟随厂商默认。
 */
export const EFFORT_LEVELS = [
  { value: "off", label: "关闭", hint: "关掉思考：最快最省，界面上也不会有「思考」" },
  { value: "low", label: "低", hint: "最快；适合「先看看大概」的粗筛" },
  { value: "high", label: "高", hint: "默认档；结论更稳，来回稍慢" },
  { value: "max", label: "最高", hint: "最慢；适合大抓包或需要反复推理的场合" },
] as const;

/** 档位 → 中文标签（没存过 = 厂商默认，按「高」显示）。 */
export function effortLabel(value: string): string {
  return value ? (EFFORT_LEVELS.find((level) => level.value === value)?.label ?? "高") : "高";
}

/** 没存过强度时的显示档位：DeepSeek 默认思考 + 强度 high，所以落在"高"。 */
const DEFAULT_EFFORT_INDEX = 2;

/**
 * 推理强度滑块：点轨道或点档位都行，当前档位下面给一句人话说明。
 *
 * 刻意做成"一条轴 + 四档"：开关（开/关）表达不了"想让它更用力一点"这件事，
 * 而档位是连续可比较的。
 */
export function EffortSlider({
  value,
  onChange,
}: {
  value: string;
  onChange: (next: string) => void;
}) {
  const found = EFFORT_LEVELS.findIndex((level) => level.value === value);
  const index = found >= 0 ? found : DEFAULT_EFFORT_INDEX;
  const current = EFFORT_LEVELS[index];
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const railRef = useRef<HTMLDivElement | null>(null);
  const shown = dragIndex ?? index;
  const percent = (shown / (EFFORT_LEVELS.length - 1)) * 100;

  const indexFromX = (clientX: number): number => {
    const rail = railRef.current;
    if (!rail) return index;
    const rect = rail.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / Math.max(1, rect.width)));
    return Math.round(ratio * (EFFORT_LEVELS.length - 1));
  };

  const move = (clientX: number) => setDragIndex(indexFromX(clientX));
  const commit = (clientX: number) => {
    const next = EFFORT_LEVELS[indexFromX(clientX)];
    setDragIndex(null);
    if (next && next.value !== current.value) onChange(next.value);
  };

  return (
    <div className="effort">
      <div
        className={`effort-rail${dragIndex === null ? "" : " dragging"}`}
        ref={railRef}
        role="slider"
        tabIndex={0}
        aria-valuemin={0}
        aria-valuemax={EFFORT_LEVELS.length - 1}
        aria-valuenow={index}
        aria-valuetext={current.label}
        data-level={current.value}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          move(event.clientX);
        }}
        onPointerMove={(event) => {
          if (dragIndex !== null) move(event.clientX);
        }}
        onPointerUp={(event) => commit(event.clientX)}
        onPointerCancel={(event) => commit(event.clientX)}
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft" || event.key === "ArrowDown") {
            event.preventDefault();
            onChange(EFFORT_LEVELS[Math.max(0, index - 1)].value);
          }
          if (event.key === "ArrowRight" || event.key === "ArrowUp") {
            event.preventDefault();
            onChange(EFFORT_LEVELS[Math.min(EFFORT_LEVELS.length - 1, index + 1)].value);
          }
        }}
      >
        <div className="effort-fill" data-level={current.value} style={{ width: `${percent}%` }} />
        <div className="effort-dot" style={{ left: `${percent}%` }} />
        <div className="effort-marks">
          {EFFORT_LEVELS.map((level) => (
            <span key={level.value} className={level.value === current.value ? "on" : ""}>
              {level.label}
            </span>
          ))}
        </div>
      </div>
      <p className="effort-hint">{current.hint}</p>
    </div>
  );
}

function LaneTag({ name }: { name: Lane }) {
  return <span className={`lane lane-${LANE_CLASS[name]}`}>{name}</span>;
}

const LANE_CLASS: Record<Lane, string> = {
  思考: "think",
  调用: "call",
  输出: "out",
  结论: "finding",
  拒绝: "rejected",
};

/**
 * `llm_delta` 的一句话说明（channel → 泳道 + 人话）。
 *
 * 泳道本身由 `session.ts` 的 `deltaLane()` 定（时间线的排序用的是同一份映射），
 * 这里只给它配人话。
 */
function deltaLane(channel: string): { lane: Lane; label: string } {
  const lane = laneOfDelta(channel);
  if (lane === "调用") return { lane, label: "正在准备调用" };
  if (lane === "思考") return { lane, label: "模型在推理" };
  return { lane, label: "模型在写" };
}

/** 时间轴上的一条：左边的线由 CSS 画，这里只给内容。 */
/**
 * 调查过程 = 一条可折叠的时间线（Codex 的 "Worked for …" 那种）。
 *
 * **跑的时候展开**（用户要看得见模型在干什么），**跑完自己收成一行**
 * （`open` 由 true 变 false 时 React 会把 `open` 属性抹掉），之后点标题行
 * 还能再展开——收起来只是默认，不是藏起来。
 */
export function Timeline({
  open,
  head,
  innerRef,
  children,
}: {
  open: boolean;
  head: React.ReactNode;
  innerRef?: React.Ref<HTMLDetailsElement>;
  children: React.ReactNode;
}) {
  return (
    <details className="timeline" open={open} ref={innerRef}>
      <summary className="timeline-head">
        <span className="caret">▸</span>
        {head}
      </summary>
      <div className="timeline-body">{children}</div>
    </details>
  );
}

export function FeedLine({ item, evidence }: { item: FeedItem; evidence?: React.ReactNode }) {
  if (item.kind === "step") {
    return (
      <div className="feed-step">
        <span className="muted">
          第 {item.step} 步{item.pending ? " · 正在问模型…" : ""}
        </span>
      </div>
    );
  }
  if (item.kind === "delta") {
    const { lane, label } = deltaLane(item.channel);
    const title =
      lane === "调用" && item.toolName ? `${label} ${item.toolName}` : label;
    if (item.done) {
      return (
        <details className="feed-raw">
          <summary>
            <span className="caret">▸</span> <LaneTag name={lane} /> {title} · 原文{" "}
            {item.text.length} 字符
          </summary>
          <pre className="feed-raw-text">{item.text}</pre>
        </details>
      );
    }
    // 只画尾巴：几千字符的 JSON 里，人要看的是"现在写到哪了"。
    const tail = item.text.length > 900 ? item.text.slice(-900) : item.text;
    return (
      <div className={`feed-delta lane-${LANE_CLASS[lane]}`}>
        <div className="feed-delta-head">
          <LaneTag name={lane} />
          <span className="pulse" /> {title}…
        </div>
        <pre className="feed-delta-text">
          {tail}
          <span className="caret-live" />
        </pre>
      </div>
    );
  }
  if (item.kind === "tool") {
    return (
      <div className={`feed-tool${item.call.pending ? " pending" : ""}`}>
        <LaneTag name="调用" />
        <ToolCard call={item.call} />
      </div>
    );
  }
  if (item.kind === "finding") {
    const basis = BASIS_LABEL[item.finding.basis] ?? item.finding.basis;
    // 标题里已经写着"规则命中 X"时不再重复一遍依据（和结论卡同一条规矩）。
    const bits = [
      item.finding.title.includes(basis) ? "" : basis,
      item.finding.evidence_ids?.length ? `证据 ${item.finding.evidence_ids.length}` : "",
    ].filter(Boolean);
    return (
      <div className="feed-finding-wrap">
        <div className={`feed-finding finding-${item.finding.severity}`}>
          <LaneTag name="结论" />
          <span className="dot" />
          <span className="finding-row-title">{item.finding.title}</span>
          {bits.length ? <span className="muted">{bits.join(" · ")}</span> : null}
          <span className="spacer" />
          <SeverityBadge severity={item.finding.severity} />
        </div>
        {evidence}
      </div>
    );
  }
  return (
    <div className="feed-rejected">
      <LaneTag name="拒绝" />
      <span className="warn">✕ {item.code}</span>
      <span className="muted">
        {item.title || "(标题缺失)"} —— {item.reason}
      </span>
    </div>
  );
}

/**
 * 图例卡片：四层输出 + 严重度色阶。
 *
 * 空态里是"第一次进来就懂这套界面怎么读"；会话流里折起来，是"这份输出怎么看"。
 */
export function LegendCards() {
  return (
    <div className="legend">
      <div className="legend-cards">
        {LEGEND_LAYERS.map((item) => (
          <div className="legend-card" key={item.title}>
            <div className="legend-mark">{item.mark}</div>
            <div className="legend-title">{item.title}</div>
            <div className="legend-text">{item.text}</div>
          </div>
        ))}
      </div>
      <div className="legend-scale">
        <span className="muted">严重度</span>
        <span className="badge sev-high">严重</span>
        <span className="badge sev-medium">中等</span>
        <span className="badge sev-low">轻微</span>
        <span className="badge sev-info">提示</span>
        <span className="muted">只表示"多值得先看"，不表示已经确认是攻击。</span>
      </div>
    </div>
  );
}

const LEGEND_LAYERS = [
  {
    mark: "◆",
    title: "结论",
    text: "一句话说清这份抓包怎么回事；每条带严重度徽章与证据入口。",
  },
  {
    mark: "▤",
    title: "工具",
    text: "Agent 每一步查了什么、拿到了什么。",
  },
  {
    mark: "⌗",
    title: "证据",
    text: "每条结论都能点回它用到的证据：工具调用 → 会话 / 告警 / 包号。",
  },
  {
    mark: "¶",
    title: "报告",
    text: "引擎渲染的九节 Markdown，可导出、可打开所在文件夹。",
  },
];

/** 原始数据：这份抓包里有什么（"规则未命中"这类信息在这里当普通一行）。 */
export function CaptureOverview({
  summary,
  protocolCounts,
}: {
  summary: CaptureSummary;
  protocolCounts?: Record<string, number>;
}) {
  return (
    <div className="metrics">
      <Metric label="包" value={fmt(summary.packets)} />
      <Metric label="会话" value={fmt(summary.sessions)} />
      <Metric label="字节" value={fmtBytes(summary.bytes)} />
      <Metric label="规则告警" value={fmt(summary.alerts ?? 0)} />
      <Metric
        label="不完整会话"
        value={fmt((summary.incomplete_sessions ?? 0) + (summary.dropped_sessions ?? 0))}
      />
      <Metric label="解码错误" value={fmt(summary.decode_errors ?? 0)} />
      {summary.source_path ? (
        <div className="metrics-wide muted" title={summary.source_path}>
          文件 <Mono>{basename(summary.source_path)}</Mono>
          {summary.format ? ` · ${summary.format}` : ""}
          {summary.duration_s !== undefined ? ` · 时长 ${summary.duration_s}s` : ""}
          {protocolCounts && Object.keys(protocolCounts).length
            ? ` · 协议 ${Object.entries(protocolCounts)
                .sort((a, b) => b[1] - a[1])
                .slice(0, 4)
                .map(([name, count]) => `${name} ${count}`)
                .join(" / ")}`
            : ""}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 一条结论的证据：tc 锚点 → 台账行（v0.4 直接挂在时间线的结论节点下面，
 * 不再单开一块"证据链"——**信任是点开验证出来的**，所以它就长在结论旁边）。
 */
export function FindingEvidence({
  finding,
  trace,
  selected,
  onSelect,
}: {
  finding: Finding;
  trace: TraceEntry[];
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  const anchors = finding.evidence ?? [];
  if (!anchors.length) return null;
  return (
    <div className="anchor-list">
      {anchors.map((anchor) => (
        <button
          key={anchor._id}
          className={`anchor ${selected === anchor._id ? "anchor-active" : ""}`}
          title={`${anchor.method} · ${anchor._id}`}
          onClick={() => onSelect(selected === anchor._id ? null : anchor._id)}
        >
          <Mono>{anchor.method}</Mono>
          <AnchorId id={anchor._id} />
        </button>
      ))}
      {selected && anchors.some((anchor) => anchor._id === selected) ? (
        <TraceTable rows={trace.filter((row) => row._id === selected)} />
      ) : null}
    </div>
  );
}

function TraceTable({ rows }: { rows: TraceEntry[] }) {
  if (!rows.length) {
    return <p className="muted">这条锚点没有对应的工具调用记录。</p>;
  }
  return (
    <table className="table">
      <thead>
        <tr>
          <th>工具</th>
          <th>状态</th>
          <th>耗时</th>
          <th>参数</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row._id}>
            <td>
              <Mono>{row.method}</Mono>
            </td>
            <td>{row.status}</td>
            <td>{row.duration_ms} ms</td>
            <td className="truncate" title={row.args}>
              {row.args}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** 报告：这里只给状态、指纹与动作——**正文在右侧的报告坞里**（§5.1 v0.3）。 */
export function ReportPanel({
  report,
  busy,
  disabled,
  open,
  onToggle,
  onGenerate,
  onOpenFolder,
}: {
  report: ReportResult | null;
  /** 正在生成（按钮显示"生成中…"）。 */
  busy: boolean;
  /** 现在不能生成（没有任务 / sidecar 不健康）。 */
  disabled: boolean;
  open: boolean;
  onToggle: () => void;
  onGenerate: () => void;
  onOpenFolder: () => void;
}) {
  return (
    <div className="block report">
      <div className="block-head report-bar">
        <span>报告</span>
        {report ? (
          <>
            <StatusPill tone={report.degraded ? "warn" : "ok"}>
              {report.degraded ? "已生成（有未核实项）" : "已生成"}
            </StatusPill>
            <span className="muted">
              {report.unverified_items
                ? `${report.unverified_items} 处未经引擎验证`
                : "全部有引擎证据"}
            </span>
            <span className="span" />
            <button onClick={onToggle}>{open ? "收起报告栏" : "在侧栏查看"}</button>
            <button onClick={onOpenFolder}>打开所在文件夹</button>
            <button onClick={onGenerate} disabled={busy || disabled}>
              {busy ? "生成中…" : "重新生成"}
            </button>
          </>
        ) : (
          <>
            <span className="muted">引擎渲染的九节报告，不需要模型。</span>
            <span
              className="info-dot"
              title={
                "报告由引擎数据渲染（不是模型写的）：抓包概览、协议统计、主要会话、规则告警、\n" +
                "调查结论、证据、局限与台账。跑过一次调查后信息最全；生成的文件在报告栏里可打开所在文件夹。"
              }
            >
              ⓘ
            </span>
            <span className="span" />
            <button className="primary" onClick={onGenerate} disabled={busy || disabled}>
              {busy ? "生成中…" : "生成报告"}
            </button>
          </>
        )}
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="metric">
      <div className="metric-value">{value}</div>
      <div className="metric-label">{label}</div>
    </div>
  );
}

function fmt(value: number | undefined): string {
  if (value === undefined) return "-";
  return value.toLocaleString("zh-CN");
}

function fmtBytes(value: number | undefined): string {
  if (value === undefined) return "-";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

/** 窗口是纳秒字符串（引擎原样给的）；人读的话说到 s/ms 就够了。 */
function fmtWindow(value: string | number | undefined): string {
  if (value === undefined) return "-";
  const ns = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(ns)) return String(value);
  if (ns >= 1e9) return `${(ns / 1e9).toFixed(2)} s`;
  if (ns >= 1e6) return `${(ns / 1e6).toFixed(1)} ms`;
  return `${ns} ns`;
}

export function fmtCost(cents: number): string {
  return `${(cents / 100).toFixed(2)} 元`;
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
