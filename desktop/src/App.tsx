/**
 * PacketSage 桌面界面。
 *
 * 布局按主流桌面 Agent 来（《GUI 工程规格书 v0.2》§5.1）：
 *
 * * **左栏**——去哪儿：打开抓包、历史抓包、设置与自检；
 * * **顶部**——当前抓包 + 状态 + 用量（模型与用量常显）；
 * * **中央**——会话流：用户那一句，然后 Agent 这一段
 *   **结论 → 工具 → 证据 → 原始数据 → 报告**（后四样都是可展开的一条）；
 * * **底部**——常驻输入框：`Enter` 发送、`Shift+Enter` 换行。
 *
 * 状态只有三种颜色语义：无异常（绿）/ 调查中（蓝）/ 未开始（灰）；
 * 失败（红）与降级（琥珀）只在真出问题时出现。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";

import {
  api,
  onSidecarExited,
  startRun,
  type AppInfo,
  type DoctorItem,
  type ProviderStatus,
} from "./api";
import {
  AlertRow,
  AnchorId,
  Banner,
  CaptureOverview,
  ConclusionCard,
  FeedLine,
  FindingEvidence,
  FindingRow,
  HeroBadge,
  LegendCards,
  MarkdownView,
  Mono,
  ReportPanel,
  StatRow,
  SUGGESTED_QUESTIONS,
  Suggestions,
  fmtCost,
  type FeedItem,
  type Tone,
} from "./components";
import { Wizard } from "./Wizard";
import type {
  AgentEvent,
  BudgetState,
  FindingSummary,
  LlmDelta,
  LlmRound,
  ReportResult,
  RunFinished,
  RunStatus,
  TaskOverview,
  TaskRow,
  ToolCall,
  Welcome,
} from "./types";

const DEFAULT_GOAL = "分析该捕获并给出可追溯结论";

/**
 * 右侧栏能装什么（v0.10）。次要信息一律进这里，不占会话流版面：
 * 抓包里有什么 / 图例 / 报告；以后的「证据台账」也是往这张表里加一行。
 */
type DockView = "capture" | "legend" | "report";

const DOCK_VIEWS: [DockView, string][] = [
  ["capture", "抓包里有什么"],
  ["legend", "图例"],
  ["report", "报告"],
];

/**
 * 一轮对话（用户一句 + Agent 这一段）。
 *
 * 引擎库里没有对话文本（`agent_runs` 只存模型/状态/耗时/费用，没有 goal），
 * 所以历史由桌面端自己存：**当前这一轮**在内存里逐事件生长，切到下一轮时
 * 归档进 `history` 并落到 localStorage（按 task 分桶）。
 */
interface Turn {
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

/** 归档后的结论卡文案：和"跑完"那条分支同一套口径。 */
function heroFromTurn(turn: Turn): {
  tone: Tone;
  eyebrow: string;
  title: string;
  detail?: string;
  pill: string;
} {
  const rows = turn.findings.length ? turn.findings : (turn.finished?.findings ?? []);
  const finished = turn.finished;
  if (finished?.status === "failed") {
    return {
      tone: "bad",
      eyebrow: "调查失败",
      title: "调查没能完成",
      detail: finished.error?.message ?? undefined,
      pill: "失败",
    };
  }
  if (!finished) {
    return { tone: "idle", eyebrow: "已就绪", title: "这份抓包还没有结论", pill: "未开始" };
  }
  if (rows.length) {
    const high = rows.some((row) => row.severity === "high");
    return {
      tone: high ? "bad" : "warn",
      eyebrow: "调查结论",
      title: `发现 ${rows.length} 条需要你确认的行为`,
      detail: "每条结论都能点开它用到的证据。",
      pill: finished.status === "degraded" ? "可能不完整" : "需要注意",
    };
  }
  return {
    tone: finished.status === "degraded" ? "warn" : "ok",
    eyebrow: "调查结论",
    title: "没有发现可疑行为",
    detail: "这次调查没有提交任何结论。",
    pill: finished.status === "degraded" ? "可能不完整" : "无异常",
  };
}

const EMPTY_STATE: BudgetState = {
  steps: 0,
  llm_calls: 0,
  tool_calls: 0,
  tokens_in: 0,
  tokens_out: 0,
  cost_cents: 0,
};

/**
 * 进度条的满格。规格里的单次 run 步数上限，**只用来画这根条**——上限本身是
 * 工程参数，不给用户看（用户要知道的是"还在跑"，不是"12 步里的第 4 步"）。
 */
const STEP_BAR_CAP = 12;

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [welcome, setWelcome] = useState<Welcome | null>(null);
  /** U7：向导的状态与开关。 */
  const [providerStatus, setProviderStatus] = useState<ProviderStatus | null>(null);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [doctor, setDoctor] = useState<DoctorItem[]>([]);
  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [taskId, setTaskId] = useState<string>("");
  const [overview, setOverview] = useState<TaskOverview | null>(null);
  const [status, setStatus] = useState<RunStatus>("idle");
  /** 输入框里现在打的那句。空着也能发：那时用 `DEFAULT_GOAL`。 */
  const [goal, setGoal] = useState("");
  /** 会话流里"用户那一句"：最近一次真正交出去的目标。 */
  const [asked, setAsked] = useState("");
  /** 会话流里的过程：按事件到达顺序追加（流式的"输出"就是它）。 */
  const [feed, setFeed] = useState<FeedItem[]>([]);
  /** 已经翻篇的对话（上一轮及更早）；当前这一轮在 feed/findings/finished 里活着。 */
  const [history, setHistory] = useState<Turn[]>([]);
  const [turnStartedAt, setTurnStartedAt] = useState(0);
  const [liveTurnId, setLiveTurnId] = useState("");
  const [liveMode, setLiveMode] = useState<"run" | "chat">("run");
  const [findings, setFindings] = useState<RunFinished["findings"]>([]);
  const [rejected, setRejected] = useState<{ title: string; reason: string; code: string }[]>([]);
  const [budgetState, setBudgetState] = useState<BudgetState>(EMPTY_STATE);
  const [finished, setFinished] = useState<RunFinished | null>(null);
  const [error, setError] = useState<{ code: string; message: string; retryable: boolean } | null>(
    null,
  );
  const [notice, setNotice] = useState<string>("");
  const [report, setReport] = useState<ReportResult | null>(null);
  const [reportText, setReportText] = useState<string>("");
  /**
   * 右栏（v0.10）：报告坞推广成**通用侧栏**——内容可切（抓包里有什么 / 图例 /
   * 报告），开关在顶栏。`null` = 收起。
   */
  const [dock, setDock] = useState<DockView | null>(null);
  const [dockMenu, setDockMenu] = useState(false);
  const [selectedAnchor, setSelectedAnchor] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** P2：`sidecar-exited` 到达后不再假装一切正常。 */
  const [sidecarDown, setSidecarDown] = useState<{
    label: string;
    code: number | null;
    tail: string[];
  } | null>(null);
  /** P7：心跳连续失败的次数（≥2 判为不健康）。 */
  const [heartbeatMisses, setHeartbeatMisses] = useState(0);
  /** P8：事件队列溢出告警。 */
  const [dropped, setDropped] = useState<{ dropped: number; capacity: number } | null>(null);
  /** 左栏折叠（§5.3：折叠状态记住；这里落在 `localStorage`）。 */
  const [railOpen, setRailOpen] = useState(() => localStorage.getItem("ps.rail") !== "0");
  /** M16：有文件正被拖到窗口上方。 */
  const [dragging, setDragging] = useState(false);
  /** 会话流是否贴着底部（跟着新内容走）；用户往上翻就停下，不抢滚动。 */
  const [stick, setStick] = useState(true);
  /** 输入框右下角的"模型 / 思考模式"面板（v0.6）。 */
  const [modelPanel, setModelPanel] = useState(false);
  const [models, setModels] = useState<string[] | null>(null);
  const [pendingModel, setPendingModel] = useState("");
  const [pendingThinking, setPendingThinking] = useState("");
  const [savingModel, setSavingModel] = useState(false);
  const runRef = useRef<string>("");
  const reportRef = useRef<HTMLDivElement | null>(null);
  /** 上次开的是哪一栏：顶栏那个开关再点一次要回到原来的内容。 */
  const lastDockRef = useRef<DockView>("capture");
  /**
   * 主动重启 Agent 的**免罪窗口**（2026-09-23 实测补的第 4 条）：向导里保存模型、
   * 输入框里换模型、左栏「重建 Agent」走的都是"退出旧的再起新的"，而外壳给**真崩溃**
   * 和**主动重启**发的是同一条 `sidecar-exited`——不这么做，配完模型界面就挂着
   * 「Agent 已退出 / 调查没能完成」，把一次正常的配置说成事故。
   */
  const agentRestartUntilRef = useRef(0);
  const timelineRef = useRef<HTMLElement | null>(null);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const streamRef = useRef<HTMLDivElement | null>(null);
  const stickRef = useRef(true);
  const userScrolledRef = useRef(false);
  const turnRefs = useRef(new Map<string, HTMLElement>());

  /**
   * 把"当前这一轮"冻成一条历史。
   *
   * 切轮（追问 / 再调查）与换任务时调用——不这么做，用户问第二句时上一轮的
   * 过程与结论就被清空了（这正是"页面要有对话的历史"那条）。
   */
  const archiveLiveTurn = useCallback((): Turn | null => {
    if (!liveTurnId) return null;
    if (!feed.length && !findings.length && !finished) return null;
    return {
      id: liveTurnId,
      goal: asked || DEFAULT_GOAL,
      mode: liveMode,
      startedAt: turnStartedAt || Date.now(),
      status,
      feed,
      findings,
      finished,
      rejected,
    };
  }, [asked, feed, findings, finished, liveMode, liveTurnId, rejected, status, turnStartedAt]);

  const toggleRail = useCallback((open: boolean) => {
    setRailOpen(open);
    localStorage.setItem("ps.rail", open ? "1" : "0");
  }, []);

  /** 开右栏并切到某一栏内容（顶栏的 ＋ 菜单与会话流里的入口都走这一条）。 */
  const openDock = useCallback((view: DockView) => {
    lastDockRef.current = view;
    setDock(view);
    setDockMenu(false);
  }, []);

  /** 顶栏那个方框图标：收起 / 回到上次看的那一栏。 */
  const toggleDock = useCallback(() => {
    setDock((current) => (current ? null : lastDockRef.current));
    setDockMenu(false);
  }, []);

  /** ＋ 菜单点开之后，点空白处就收起来（不然它会一直挂在那儿）。 */
  useEffect(() => {
    if (!dockMenu) return undefined;
    const close = () => setDockMenu(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [dockMenu]);

  /**
   * 告诉界面"接下来这次 Agent 退出是我们自己让它退的"：清掉上一条坏消息，并在
   * 窗口期内忽略 `sidecar-exited`（新的 Agent 起来之后照常握手）。
   */
  const pardonAgentRestart = useCallback(() => {
    agentRestartUntilRef.current = Date.now() + 20_000;
    setSidecarDown(null);
    setHeartbeatMisses(0);
    setError(null);
    setStatus((current) => (current === "failed" ? "idle" : current));
  }, []);

  /**
   * 用户**自己动手**了（滚轮 / 拖滚动条 / 点进流里）——只有这种情况才停止跟随。
   *
   * 不按"离底部多远"判断，是因为内容一边长一边跟随：自己刚滚到底，下一条又把它
   * 顶下去，位置探针会把这种"不到底"误判成"用户往上翻"。
   */
  const markUserScroll = useCallback(() => {
    userScrolledRef.current = true;
  }, []);

  const onStreamScroll = useCallback(() => {
    const element = streamRef.current;
    if (!element) return;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    if (distance < 24) {
      // 回到底部 = 恢复跟随。
      userScrolledRef.current = false;
      if (!stickRef.current) {
        stickRef.current = true;
        setStick(true);
      }
      return;
    }
    if (userScrolledRef.current && stickRef.current) {
      stickRef.current = false;
      setStick(false);
    }
  }, []);

  const jumpToLatest = useCallback(() => {
    const element = streamRef.current;
    if (!element) return;
    userScrolledRef.current = false;
    stickRef.current = true;
    setStick(true);
    element.scrollTop = element.scrollHeight;
  }, []);

  /** 从结论一行跳到时间线上这条结论的证据（选中第一枚锚点并滚过去）。 */
  const scrollToEvidence = useCallback(
    (findingId: string) => {
      const anchors =
        (overview?.findings ?? []).find((finding) => finding.finding_id === findingId)?.evidence ??
        [];
      setSelectedAnchor(anchors[0]?._id ?? null);
      window.requestAnimationFrame(() =>
        timelineRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }),
      );
    },
    [overview?.findings],
  );

  const refreshOverview = useCallback(async (id: string) => {
    if (!id) {
      setOverview(null);
      return;
    }
    try {
      setOverview(await api.overview(id));
    } catch (exc) {
      setError({ code: "ENGINE", message: String(exc), retryable: true });
    }
  }, []);

  const refreshTasks = useCallback(async () => {
    try {
      setTasks(await api.listTasks());
    } catch {
      setTasks([]);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      let welcomed: Welcome | null = null;
      try {
        setInfo(await api.appInfo());
        welcomed = await api.handshake();
        setWelcome(welcomed);
      } catch (exc) {
        setError({ code: "SIDECAR", message: String(exc), retryable: true });
        setStatus("failed");
      }
      try {
        const stored = await api.providerStatus();
        setProviderStatus(stored);
        // U7 / §6.1：没配 provider 就先给向导——但**不拦人**，随时能跳过。
        if (welcomed && !welcomed.providers.configured) setWizardOpen(true);
      } catch {
        setProviderStatus(null);
      }
      try {
        const payload = await api.doctor();
        setDoctor(payload.items ?? []);
      } catch {
        setDoctor([]);
      }
      await refreshTasks();
    })();
  }, [refreshTasks]);

  /** 向导保存成功后：换回新的握手结果，并把任务列表刷一遍。 */
  const onWizardConfigured = useCallback(
    (next: Welcome) => {
      setWelcome(next);
      void api.providerStatus().then(setProviderStatus).catch(() => undefined);
      void refreshTasks();
      // 向导保存一定重启过 Agent（U7）：那次退出不算事故，别把界面钉在"失败"上。
      pardonAgentRestart();
    },
    [pardonAgentRestart, refreshTasks],
  );

  /** P2：`sidecar-exited` 是外壳唯一主动推的坏消息。 */
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void onSidecarExited(([label, code]) => {
      // 主动重启时旧进程也要退出：窗口期里的 agent 退出按"计划内"处理。
      if (label === "agent" && Date.now() < agentRestartUntilRef.current) return;
      setStatus("failed");
      runRef.current = "";
      void api
        .diagnostics()
        .then((payload) => {
          const side = label === "engine" ? payload.engine : payload.agent;
          setSidecarDown({ label, code, tail: side.stderr_tail ?? [] });
        })
        .catch(() => setSidecarDown({ label, code, tail: [] }));
    })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => undefined);
    return () => unlisten?.();
  }, []);

  /** P7：每 30 s 一次 `status`；连续两次失败判为不健康。 */
  useEffect(() => {
    const timer = window.setInterval(() => {
      void api
        .status()
        .then(() => setHeartbeatMisses(0))
        .catch(() => setHeartbeatMisses((misses) => misses + 1));
    }, 30_000);
    return () => window.clearInterval(timer);
  }, []);

  /** 通道 B 的事件处理（U4 / S61）。 */
  const onEvent = useCallback(
    (event: AgentEvent) => {
      switch (event.event) {
        case "events_dropped": {
          const data = event.data as unknown as { dropped: number; capacity: number };
          setDropped(data);
          break;
        }
        case "llm_delta": {
          // token 级流式（§4.5 / U11）：同一步同一路的碎块拼在一起长。
          const data = event.data as unknown as LlmDelta;
          const text = String(data.text ?? "");
          setFeed((current) => {
            const next = [...current];
            const last = next[next.length - 1];
            if (
              last &&
              last.kind === "delta" &&
              !last.done &&
              last.step === Number(data.step ?? 0) &&
              last.channel === String(data.channel ?? "content") &&
              last.toolName === (data.tool_name ?? null)
            ) {
              next[next.length - 1] = { ...last, text: last.text + text };
              return next;
            }
            return [
              ...next,
              {
                kind: "delta",
                step: Number(data.step ?? 0),
                channel: String(data.channel ?? "content"),
                toolName: data.tool_name ?? null,
                text,
                done: false,
              },
            ];
          });
          break;
        }
        case "run_started": {
          setStatus("running");
          break;
        }
        case "llm_round": {
          // 协议里这一条叫 `step`（§4.5），本地状态叫 `steps`：两个都认，别让
          // "调查进行中（第 N 步）"在跑的时候一直停在 0。
          const raw = event.data as unknown as LlmRound;
          const steps = Number(raw.step ?? raw.steps ?? 0);
          const cacheHit = Number(raw.cache_hit_tokens ?? 0);
          const cacheMiss = Number(raw.cache_miss_tokens ?? 0);
          setBudgetState({
            steps,
            llm_calls: Number(raw.llm_calls ?? 0),
            tool_calls: Number(raw.tool_calls ?? 0),
            tokens_in: Number(raw.tokens_in ?? 0),
            tokens_out: Number(raw.tokens_out ?? 0),
            cost_cents: Number(raw.cost_cents ?? 0),
          });
          // 流式输出的一条：模型这一轮往返用了多久、花了多少。
          // 这一轮结束：把还在"打字"的原文收进折叠项（模型原文仍然可查）。
          setFeed((current) =>
            current.map((item) =>
              item.kind === "delta" && !item.done ? { ...item, done: true } : item,
            ),
          );
          setFeed((current) => [
            ...current,
            {
              kind: "step",
              step: steps,
              llmMs: raw.llm_ms ?? 0,
              toolMs: raw.tool_ms ?? 0,
              tokens: Number(raw.tokens_in ?? 0) + Number(raw.tokens_out ?? 0),
              costCents: Number(raw.cost_cents ?? 0),
              cacheHit,
              cacheMiss,
            },
          ]);
          break;
        }
        case "tool_call_started": {
          const data = event.data as unknown as ToolCall;
          const started = { ...data, status: "running", pending: true };
          setFeed((current) => [...current, { kind: "tool", call: started }]);
          break;
        }
        case "tool_call_finished": {
          const data = event.data as unknown as ToolCall;
          // 配对：同一工具名最近的那张"进行中"卡片就地定稿（U4 / S61）。
          setFeed((current) => {
            const next = [...current];
            for (let index = next.length - 1; index >= 0; index -= 1) {
              const item = next[index];
              if (
                item.kind === "tool" &&
                item.call.pending &&
                item.call.tool_name === data.tool_name
              ) {
                next[index] = { kind: "tool", call: { ...data, pending: false } };
                return next;
              }
            }
            return [...next, { kind: "tool", call: { ...data, pending: false } }];
          });
          break;
        }
        case "finding_accepted": {
          const finding = event.data as unknown as RunFinished["findings"][number];
          setFindings((current) => [...current, finding]);
          setFeed((current) => [...current, { kind: "finding", finding }]);
          break;
        }
        case "finding_rejected": {
          const data = event.data as unknown as {
            title: string;
            reason: string;
            reason_code: string;
          };
          const rejectedRow = { title: data.title, reason: data.reason, code: data.reason_code };
          setRejected((current) => [...current, rejectedRow]);
          setFeed((current) => [...current, { kind: "rejected", ...rejectedRow }]);
          break;
        }
        case "error": {
          const data = event.data as unknown as {
            code: string;
            message: string;
            retryable: boolean;
          };
          setError(data);
          setStatus("failed");
          break;
        }
        case "run_finished": {
          const payload = event.data as unknown as RunFinished;
          setFeed((current) =>
            current.map((item) =>
              item.kind === "delta" && !item.done ? { ...item, done: true } : item,
            ),
          );
          setFinished(payload);
          setStatus(payload.status);
          setBudgetState({
            steps: payload.steps,
            llm_calls: payload.calls,
            tool_calls: payload.tool_calls,
            tokens_in: payload.tokens.in,
            tokens_out: payload.tokens.out,
            cost_cents: payload.cost_cents,
          });
          if (payload.error) setError(payload.error);
          runRef.current = "";
          void refreshOverview(payload.task_id);
          void refreshTasks();
          break;
        }
        default:
          break;
      }
    },
    [refreshOverview, refreshTasks],
  );

  const startInvestigation = useCallback(
    async (mode: "run" | "chat", text?: string) => {
      if (!taskId) return;
      const targetGoal = text?.trim() || goal.trim() || DEFAULT_GOAL;
      // 先把上一轮归档：对话历史就是这么攒起来的（不许再整屏清空）。
      const archived = archiveLiveTurn();
      if (archived) setHistory((current) => [...current, archived]);
      setLiveTurnId(`turn_${Date.now().toString(36)}`);
      setLiveMode(mode);
      setTurnStartedAt(Date.now());
      setError(null);
      setRejected([]);
      setReport(null);
      setReportText("");
      setDock((view) => (view === "report" ? null : view));
      // P9：run 与 chat 都清空"这一轮的"渲染状态。
      setFeed([]);
      setFindings([]);
      setFinished(null);
      setBudgetState(EMPTY_STATE);
      setDropped(null);
      setStatus("running");
      // 会话流里留下"用户那一句"，跑完也还能看见这一轮问的是什么。
      setAsked(targetGoal);
      try {
        const ack = await startRun(taskId, targetGoal, mode, onEvent);
        runRef.current = ack.run_id;
      } catch (exc) {
        setError({ code: "RUN", message: String(exc), retryable: true });
        setStatus("failed");
      }
    },
    [archiveLiveTurn, goal, onEvent, taskId],
  );

  /** 打开并分析一份抓包（对话框、拖放、左栏按钮都走这一条）。 */
  const analyzePath = useCallback(
    async (path: string) => {
      // 换抓包 = 换一条对话线：旧任务那一轮先落盘，当前列表清空。
      const archived = archiveLiveTurn();
      if (archived && taskId) saveHistory(taskId, [...history, archived]);
      setHistory([]);
      /**
       * 打开抓包**不是一轮对话**（2026-09-23 修）：`liveTurnId` / `asked` 只在真正
       * 发起 run / chat 时才设——以前这里也建了一轮，于是打开十份抓包就在会话流里
       * 留十轮「打开 XXX」的空壳，把真正的对话顶下去。
       */
      setLiveTurnId("");
      setAsked("");
      // 形态复位成"调查"，否则上一句 chat 的形态会漏过来（实测：打开另一份抓包时
      // 冒出"模型这次没有给出文字回答"）。
      setLiveMode("run");
      setTurnStartedAt(0);
      setStatus("analyzing");
      setError(null);
      setBudgetState(EMPTY_STATE);
      setNotice(`正在分析 ${path}`);
      try {
        const result = await api.analyze(path);
        setTaskId(result.task_id);
        setNotice("");
        setFeed([]);
        setFindings([]);
        setRejected([]);
        setFinished(null);
        setReport(null);
        setReportText("");
        setDock((view) => (view === "report" ? null : view));
        setStatus("idle");
        await refreshTasks();
        await refreshOverview(result.task_id);
      } catch (exc) {
        setError({ code: "ANALYZE", message: String(exc), retryable: true });
        setStatus("failed");
        setNotice("");
      }
    },
    [archiveLiveTurn, history, refreshOverview, refreshTasks, taskId],
  );

  const pickCapture = useCallback(async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "抓包文件", extensions: ["pcap", "pcapng"] }],
    });
    if (typeof selected !== "string") return;
    await analyzePath(selected);
  }, [analyzePath]);

  /** M16：拖进窗口即分析（左栏那句提示得说话算话）。 */
  useEffect(() => {
    let stop: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") {
          setDragging(true);
          return;
        }
        setDragging(false);
        if (event.payload.type !== "drop") return;
        const capture = event.payload.paths.find((path) => /\.pcapng?$/i.test(path));
        if (capture) {
          void analyzePath(capture);
        } else {
          setError({
            code: "DROP",
            message: "只认 .pcap / .pcapng：这份文件看不出来是抓包。",
            retryable: false,
          });
        }
      })
      .then((unlisten) => {
        stop = unlisten;
      })
      .catch(() => undefined);
    return () => stop?.();
  }, [analyzePath]);

  /** 输入框跟着内容长高（最高 180px，再多就自己滚）。 */
  useEffect(() => {
    const element = composerRef.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${Math.min(180, element.scrollHeight)}px`;
  }, [goal]);

  const cancel = useCallback(async () => {
    setStatus("finalizing");
    try {
      await api.cancel();
      setNotice("已请求收尾：当前工具调用跑完就停，已提交的结论保留");
    } catch (exc) {
      setError({ code: "CANCEL", message: String(exc), retryable: false });
    }
  }, []);

  const generateReport = useCallback(async () => {
    if (!taskId) return;
    setBusy(true);
      try {
        const result = await api.report(taskId);
        setReport(result);
        setReportText(await api.readText(result.report_path));
      // 生成完直接开右侧报告栏：正文归它，对话那半屏不动。
      openDock("report");
    } catch (exc) {
      setError({ code: "REPORT", message: String(exc), retryable: false });
    } finally {
      setBusy(false);
    }
  }, [openDock, taskId]);

  /** 报告在磁盘上：交给系统去定位/打开。 */
  const openReportFolder = useCallback(() => {
    if (!report) return;
    void revealItemInDir(report.report_path).catch(() => openPath(report.report_path));
  }, [report]);

  /** 输入框右下角的面板：模型（`GET /models` 的候选）+ 思考模式。 */
  const toggleModelPanel = useCallback(async () => {
    if (modelPanel) {
      setModelPanel(false);
      return;
    }
    setPendingModel(welcome?.providers.model || "");
    setPendingThinking(providerStatus?.thinking || "");
    setModels(null);
    setModelPanel(true);
    try {
      setModels(await api.providerModels());
    } catch {
      setModels([]);
    }
  }, [modelPanel, providerStatus?.thinking, welcome?.providers.model]);

  /** 保存 = 换模型 / 开关思考：走 U7 的 `provider_save`（key 沿用已存的那一个）。 */
  const saveModelPanel = useCallback(async () => {
    if (!welcome) return;
    setSavingModel(true);
    try {
      const result = await api.providerSave({
        provider: welcome.providers.kind,
        model: pendingModel.trim(),
        baseUrl: providerStatus?.base_url ?? "",
        apiKey: null,
        thinking: pendingThinking,
      });
      setWelcome(result.welcome);
      void api.providerStatus().then(setProviderStatus).catch(() => undefined);
      setModelPanel(false);
      // 「保存并重启 Agent」：重启是计划内的，别让 `sidecar-exited` 把界面打红。
      pardonAgentRestart();
      setNotice(
        `已切到 ${result.model || result.provider}${
          pendingThinking === "disabled" ? "（思考已关）" : ""
        }，Agent 已重启。`,
      );
    } catch (exc) {
      setError({ code: "MODEL", message: String(exc), retryable: false });
    } finally {
      setSavingModel(false);
    }
  }, [pardonAgentRestart, pendingModel, pendingThinking, providerStatus?.base_url, welcome]);

  /** 从证据锚点跳到报告（结论 → 证据 → 报告正文，一条线走到底）。 */
  const revealReport = useCallback(() => {
    if (report) openDock("report");
    window.requestAnimationFrame(() =>
      reportRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }),
    );
  }, [openDock, report]);

  const running = status === "running" || status === "finalizing";
  const providerBlocked = welcome ? !welcome.providers.configured : false;
  const broken = Boolean(info?.start_error);
  const unhealthy = heartbeatMisses >= 2 || Boolean(sidecarDown);
  /** 硬阻断：sidecar 起不来或不健康，连证据都拿不到。 */
  const blocked = broken || unhealthy;
  /** 只有"Agent 调查"需要模型：分析/告警/证据链/报告都是引擎路径。 */
  const investigationBlocked = blocked || providerBlocked;
  /** 输入框现在能不能发出去（没有任务、跑着、sidecar 不健康都不行）。 */
  const canSend = Boolean(taskId) && !running && !investigationBlocked;
  const failedChecks = useMemo(
    () => doctor.filter((item) => item.status === "fail" || item.status === "warn"),
    [doctor],
  );
  const failedOnly = useMemo(() => doctor.filter((item) => item.status === "fail"), [doctor]);
  const warnedOnly = useMemo(() => doctor.filter((item) => item.status === "warn"), [doctor]);
  const summary = overview?.summary;

  /** 结论卡的内容：优先级 = 失败 > 进行中 > 完成 > 只有引擎告警 > 还没跑。 */
  const hero = useMemo((): {
    tone: Tone;
    eyebrow: string;
    title: string;
    detail?: string;
    rows: FindingSummary[];
    alerts: TaskOverview["alerts"];
    pill: string;
  } => {
    const alerts = overview?.alerts ?? [];
    if (!taskId) {
      return {
        tone: "idle",
        eyebrow: "未开始",
        title: "先打开一份抓包",
        detail: "支持 .pcap / .pcapng：拖进窗口，或点左边的按钮。分析不需要模型，打开就能看统计与规则告警。",
        rows: [],
        alerts: [],
        pill: "未开始",
      };
    }
    if (status === "failed") {
      return {
        tone: "bad",
        eyebrow: "调查失败",
        title: error ? `调查没能完成：${error.code}` : "调查没能完成",
        detail: error?.message,
        rows: [],
        alerts,
        pill: "失败",
      };
    }
    if (running) {
      return {
        tone: "busy",
        eyebrow: "调查中",
        title: findings.length
          ? `正在调查 · 已有 ${findings.length} 条结论`
          : "正在调查",
        detail: findings.length
          ? "结论会一条条出现，随时可以停止。"
          : "工具调用会一条条出现在这里，随时可以停止。",
        // 逐条出现的结论：不用等 run 结束才让人看见（U4 / S61）。
        rows: findings,
        alerts,
        pill: "调查中",
      };
    }
    if (finished) {
      const rows = finished.findings ?? [];
      if (finished.status === "failed") {
        return {
          tone: "bad",
          eyebrow: "调查失败",
          title: "调查没能完成",
          detail: finished.error?.message ?? undefined,
          rows: [],
          alerts,
          pill: "失败",
        };
      }
      if (rows.length) {
        const high = rows.some((row) => row.severity === "high");
        return {
          tone: high ? "bad" : "warn",
          eyebrow: "调查结论",
          title: `发现 ${rows.length} 条需要你确认的行为`,
          detail: "每条结论都能点开它用到的证据。",
          rows,
          alerts,
          pill: finished.status === "degraded" ? "可能不完整" : "需要注意",
        };
      }
      return {
        tone: finished.status === "degraded" ? "warn" : "ok",
        eyebrow: "调查结论",
        title: "没有发现可疑行为",
        detail: "这次调查没有提交任何结论。下面是这份抓包的原始统计。",
        rows: [],
        alerts,
        pill: finished.status === "degraded" ? "可能不完整" : "无异常",
      };
    }
    if (alerts.length) {
      return {
        tone: "warn",
        eyebrow: "规则命中",
        title: `规则命中 ${alerts.length} 条告警，还没做调查`,
        detail: "规则只说明「这个模式出现了」，不代表有问题。点「开始调查」让 Agent 把它们串成结论。",
        rows: [],
        alerts,
        pill: "待调查",
      };
    }
    return {
      tone: "idle",
      eyebrow: "已就绪",
      title: "这份抓包还没有结论",
      detail: summary
        ? `${(summary.packets ?? 0).toLocaleString("zh-CN")} 个包 · ${
            summary.sessions ?? 0
          } 个会话 · 规则没有命中。跑一次调查才能给出结论。`
        : "跑一次调查才能给出结论。",
      rows: [],
      alerts: [],
      pill: "未开始",
    };
  }, [error, findings, finished, overview?.alerts, running, summary, taskId]);

  /**
   * 结论卡里的条目：跑的时候结论落在会话流里（流式），卡上只留状态与进度；
   * 跑完再把最终结论摆回卡上——那时它才是主角。
   */
  const cardRows = running ? [] : hero.rows;
  /** 引擎库里的 finding 原文（含模型为每条写的 summary），按 id 取。 */
  const findingDetails = useMemo(() => {
    const map = new Map<string, string>();
    for (const row of overview?.findings ?? []) {
      if (row.finding_id && row.summary) map.set(row.finding_id, row.summary);
    }
    return map;
  }, [overview?.findings]);

  /**
   * Hero 的数字行：只留用户读得懂的三样——这份抓包多大、花了多少钱。
   * 步骤 / 工具调用 / tokens / 耗时 / 缓存命中都是工程指标，不是给人看的。
   */
  const heroStats = useMemo(() => {
    const items: { label: string; value: string; hint?: string }[] = [];
    if (summary) {
      items.push({ label: "包", value: (summary.packets ?? 0).toLocaleString("zh-CN") });
      items.push({ label: "会话", value: (summary.sessions ?? 0).toLocaleString("zh-CN") });
    }
    if (!taskId) return items;
    if (budgetState.cost_cents > 0) {
      items.push({ label: "费用", value: fmtCost(budgetState.cost_cents) });
    }
    return items;
  }, [budgetState.cost_cents, summary, taskId]);
  /**
   * 空态的数字行：只留"这份抓包有多大"。还没跑过调查时，"步骤 0 / 工具调用 0"
   * 是噪音，不是信息。
   */
  const idleStats = useMemo(
    () => heroStats.filter((item) => item.value !== "0"),
    [heroStats],
  );
  /** 最后一条还没收尾的 delta = 正在流的那一段（此时不画"等模型下一步"）。 */
  const streamingNow = (() => {
    const last = feed[feed.length - 1];
    return Boolean(last && last.kind === "delta" && !last.done);
  })();

  /** 当前这一轮（还没归档的那一轮）。 */
  const liveTurn: Turn | null = liveTurnId
    ? {
        id: liveTurnId,
        goal: asked || DEFAULT_GOAL,
        mode: liveMode,
        startedAt: turnStartedAt,
        status,
        feed,
        findings,
        finished,
        rejected,
      }
    : null;
  /**
   * 左栏「历史对话」：把每个抓包下的轮次聚合起来（历史按 `task_id` 分桶存在本机），
   * 按时间倒序——点一条就切到那个抓包并跳到那一轮。
   */
  const chatHistory = useMemo(() => {
    const rows: { taskId: string; source: string; turn: Turn }[] = [];
    for (const task of tasks) {
      const bucket = task.id === taskId ? history : loadHistory(task.id);
      for (const turn of bucket) {
        rows.push({ taskId: task.id, source: task.source_path ?? task.id, turn });
      }
    }
    if (liveTurn && taskId) {
      const source = summary?.source_path ?? taskId;
      rows.push({ taskId, source, turn: liveTurn });
    }
    return rows.sort((a, b) => (b.turn.startedAt || 0) - (a.turn.startedAt || 0));
  }, [history, liveTurn, summary?.source_path, taskId, tasks]);

  /** 换任务（左栏抓包与历史对话共用一条路）。 */
  const openTask = useCallback(
    (id: string) => {
      const archived = archiveLiveTurn();
      if (archived && taskId) saveHistory(taskId, [...history, archived]);
      setHistory(loadHistory(id));
      // 切抓包同样**不建轮**：会话流里只留真正跑过的轮次（顶栏写着文件名就够）。
      setLiveTurnId("");
      setAsked("");
      setLiveMode("run");
      setTurnStartedAt(0);
      setTaskId(id);
      setFinished(null);
      setFeed([]);
      setFindings([]);
      setError(null);
      setStatus("idle");
      setBudgetState(EMPTY_STATE);
      setReport(null);
      setReportText("");
      // 报告是上一份抓包的，切走就收起来；「抓包里有什么」跟着新抓包继续看。
      setDock((view) => (view === "report" ? null : view));
      void refreshOverview(id);
    },
    [archiveLiveTurn, history, refreshOverview, taskId],
  );

  /** 历史落盘（按 task 分桶）；写失败也不影响用。 */
  useEffect(() => {
    if (taskId) saveHistory(taskId, history);
  }, [taskId, history]);

  /**
   * 跟到底：调查中每来一条（token 碎块、工具卡片、结论）都跟着往下走——
   * 前提是用户没有往上翻（翻上去看证据时不许被拽回来）。
   */
  useEffect(() => {
    if (!running || !stickRef.current) return;
    const element = streamRef.current;
    if (!element) return;
    const frame = window.requestAnimationFrame(() => {
      element.scrollTop = element.scrollHeight;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [feed, running]);

  return (
    <div className={`app${railOpen ? "" : " collapsed"}`}>
      {/* 左栏只干两件事：去哪儿（抓包），和设置入口。 */}
      <aside className="rail">
        <div className="rail-head">
          <div className="brand">
            <span className="logo">🛡️</span> YeLee’ PacketSage
          </div>
          <span className="spacer" />
          <button className="icon" title="收起左栏" onClick={() => toggleRail(false)}>
            ‹
          </button>
        </div>

        <button className="rail-action" onClick={pickCapture} disabled={running || blocked}>
          <span className="plus">＋</span> 打开抓包并分析
        </button>
        <div className="rail-hint">也可以把 .pcap / .pcapng 拖进窗口</div>

        <div className="rail-group grow">
          <div className="rail-label">抓包</div>
          {tasks.length ? (
            tasks.map((row) => (
              <button
                key={row.id}
                className={`rail-item${row.id === taskId ? " active" : ""}`}
                onClick={() => openTask(row.id)}
                title={row.source_path ?? row.id}
              >
                <span className="glyph">▤</span>
                <span className="name">
                  {row.source_path ? basename(row.source_path) : `${row.id.slice(0, 12)}…`}
                </span>
                {/* 同名抓包靠时间区分；悬停给全路径 + 包数 + task id。 */}
                <span
                  className="meta"
                  title={`${row.source_path ?? row.id}\n${row.id}\n${row.packet_count ?? "-"} 包`}
                >
                  {formatStamp(row.started_at)}
                </span>
              </button>
            ))
          ) : (
            <div className="rail-empty muted">还没有分析过的抓包。</div>
          )}
        </div>

        {/* 历史对话：所有抓包下的轮次，倒序（点一下切到那个抓包并跳到那一轮）。 */}
        {chatHistory.length ? (
          <div className="rail-group">
            <div className="rail-label">历史对话 · {chatHistory.length}</div>
            {chatHistory.map(({ taskId: owner, source, turn }, index) => (
              <button
                key={turn.id || `turn-${index}`}
                className={`rail-item${owner === taskId && index === 0 ? " active" : ""}`}
                title={`${turn.goal}\n${basename(source)} · ${formatClock(turn.startedAt)}`}
                onClick={() => {
                  if (owner === taskId) {
                    turnRefs.current
                      .get(turn.id)
                      ?.scrollIntoView({ behavior: "smooth", block: "start" });
                    return;
                  }
                  // 别的抓包：先切过去，等它渲染出来再滚到那一轮。
                  openTask(owner);
                  window.setTimeout(() => {
                    turnRefs.current
                      .get(turn.id)
                      ?.scrollIntoView({ behavior: "smooth", block: "start" });
                  }, 700);
                }}
              >
                <span className="glyph">💬</span>
                <span className="name">{turn.goal}</span>
                <span className="meta">
                  {owner === taskId ? formatClock(turn.startedAt) : basename(source)}
                </span>
              </button>
            ))}
          </div>
        ) : null}

        {/* 开发态信息全部收进左栏底部，默认不展开。 */}
        <div className="rail-foot">
          <details className="rail-details">
            <summary>诊断与自检</summary>
            <div className="rail-details-body">
              {failedOnly.map((item) => (
                <div key={item.id} className="check check-fail">
                  <strong>❌ {item.name}</strong>
                  <div className="muted">{item.detail}</div>
                  {item.repair_hint ? <pre className="code">{item.repair_hint}</pre> : null}
                </div>
              ))}
              {!failedChecks.length ? <div className="muted">十项自检全过。</div> : null}
              {warnedOnly.map((item) => (
                <div key={item.id} className="check">
                  <strong>⚠ {item.name}</strong>
                  <div className="muted">{item.detail}</div>
                </div>
              ))}
              <div className="row">
                <button
                  onClick={() => {
                    // 这次退出是我们叫的：先把免罪窗口立起来，再重启。
                    pardonAgentRestart();
                    void api
                      .restartAgent()
                      .then((next) => {
                        setWelcome(next);
                        setNotice("Agent sidecar 已重建，可以继续调查。");
                      })
                      .catch((exc) =>
                        setError({ code: "RESTART", message: String(exc), retryable: true }),
                      );
                  }}
                  disabled={broken}
                >
                  重建 Agent
                </button>
              </div>
            </div>
          </details>

        </div>
      </aside>

      <main className={`main${dock ? " docked" : ""}`}>
        {/* 顶部状态条（§5.1）：当前抓包 + 状态 + 用量；预算明细在「调查过程」里。 */}
        <header className="topbar">
          {!railOpen ? (
            <button className="icon" title="展开左栏" onClick={() => toggleRail(true)}>
              ›
            </button>
          ) : null}
          <div className="topbar-title">
            {/* 顶栏只说"在看哪份抓包"：task id 收进 tooltip，用量归 Hero 的 stat 行。 */}
            <strong title={taskId ? `${summary?.source_path ?? ""}\n${taskId}` : undefined}>
              {summary?.source_path ? basename(summary.source_path) : "还没有打开抓包"}
            </strong>
            {!taskId ? (
              <span className="muted">拖一份 .pcap / .pcapng 进来</span>
            ) : null}
          </div>
          <span className="spacer" />
          {/*
            右栏开关（v0.10）：Codex 那样——顶栏一个方框图标开合，旁边的 ＋ 列可选内容。
            次要信息（抓包里有什么 / 图例 / 报告）全进右栏，输入框那一带不再挂胶囊。
          */}
          <div className="dock-toggle">
            <button
              className={`icon${dock ? " on" : ""}`}
              title={dock ? "收起右侧栏" : "打开右侧栏"}
              onClick={toggleDock}
            >
              ▥
            </button>
            <button
              className={`icon${dockMenu ? " on" : ""}`}
              title="右侧栏：选内容"
              onClick={(event) => {
                event.stopPropagation();
                setDockMenu((open) => !open);
              }}
            >
              ＋
            </button>
          </div>
          {/* 设置入口常驻顶栏：左栏收起来也进得去（U7 的 key 只能从这里改）。 */}
          <button
            className="icon"
            title={providerBlocked ? "设置：接上模型（API key）" : "设置：模型 / API key"}
            onClick={() => setWizardOpen(true)}
            disabled={broken}
          >
            ⚙
          </button>
        </header>

        {/*
          ＋ 菜单挂在 `<main>` 上、而不是顶栏里：顶栏为了让标题省略号必须
          `overflow: hidden`，菜单挂进去会被裁掉（2026-09-23 实测）。
        */}
        {dockMenu ? (
          <div className="dock-menu" role="menu">
            {DOCK_VIEWS.map(([key, label]) => (
              <button
                key={key}
                role="menuitem"
                className={dock === key ? "on" : undefined}
                onClick={() => openDock(key)}
              >
                {key === "report" && report ? `${label} · 已生成` : label}
              </button>
            ))}
          </div>
        ) : null}

        <div
          className="stream"
          ref={streamRef}
          onScroll={onStreamScroll}
          onWheel={markUserScroll}
          onMouseDown={markUserScroll}
          onTouchMove={markUserScroll}
          onKeyDown={markUserScroll}
        >
          <div className={`stream-inner${taskId ? "" : " empty"}`}>
            {broken ? (
              <Banner
                kind="error"
                title="YeLee’ PacketSage 没能启动"
                detail="后台组件没有启动成功。重新安装通常能解决；如果重装后还是这样，请把这条提示反馈给开发者。"
              />
            ) : null}
            {sidecarDown ? (
              <Banner
                kind="error"
                title={`${sidecarDown.label === "engine" ? "分析引擎" : "Agent"} 意外退出了`}
                detail={
                  sidecarDown.tail.length
                    ? `最后几行输出：\n${sidecarDown.tail.join("\n")}`
                    : "可以在左边「诊断与自检」里重新启动它。"
                }
                action="知道了"
                onAction={() => setSidecarDown(null)}
              />
            ) : null}
            {!sidecarDown && heartbeatMisses >= 2 ? (
              <Banner
                kind="error"
                title="Agent 没有响应"
                detail="可以在左边「诊断与自检」里重新启动它。"
              />
            ) : null}
            {!broken && providerBlocked ? (
              <Banner
                kind="warn"
                title="还没有接模型：证据照常看，Agent 调查要等接上"
                detail="key 只存进 Windows 凭据管理器，不写明文文件。"
                action="接上模型"
                onAction={() => setWizardOpen(true)}
              />
            ) : null}
            {dropped ? (
              <Banner
                kind="warn"
                title="有一部分过程信息没能显示出来"
                detail="结论与证据都存在引擎里，不受影响；重开这份抓包就能看到完整结果。"
                action="知道了"
                onAction={() => setDropped(null)}
              />
            ) : null}
            {error && status !== "failed" ? (
              <Banner
                kind={error.retryable ? "warn" : "error"}
                title={error.retryable ? "这一步没能完成" : "这一步没能完成（要先修好配置）"}
                detail={error.message}
                secondary={error.retryable ? undefined : "前往配置"}
                onSecondary={error.retryable ? undefined : () => setWizardOpen(true)}
                action="知道了"
                onAction={() => setError(null)}
              />
            ) : null}
            {notice ? <p className="notice">{notice}</p> : null}

            {!taskId ? (
              /* 空态（§5.1）：给建议动作，而不是让人对着空白想措辞。 */
              <div className="empty-hero">
                <div className="empty-mark">🛡️</div>
                <h1 className="empty-title">从哪份抓包开始？</h1>
                <p className="empty-text">
                  支持 .pcap / .pcapng：拖进窗口，或者点下面的按钮。打开就能看统计与规则告警——这一步不需要模型。
                </p>
                <div className="empty-actions">
                  <button className="primary big" onClick={pickCapture} disabled={blocked}>
                    打开抓包并分析
                  </button>
                </div>
                <LegendCards />
              </div>
            ) : null}

            {/* 翻过篇的那几轮 */}
            {history.map((turn) => {
              const card = heroFromTurn(turn);
              return (
                <section
                  className="turn turn-past"
                  key={turn.id}
                  ref={(node) => {
                    if (node) turnRefs.current.set(turn.id, node);
                  }}
                >
                  <div className="turn-user">
                    <div className="user-bubble">
                      <div className="who">你 · {formatClock(turn.startedAt)}</div>
                      <div>{turn.goal}</div>
                    </div>
                  </div>
                  <div className="agent-head">
                    <span className="mark">◆</span> YeLee’ PacketSage
                  </div>
                  {turn.finished?.summary ? (
                    <div className={turn.mode === "chat" ? "answer" : "assistant-note"}>
                      <MarkdownView text={turn.finished.summary} />
                    </div>
                  ) : turn.mode === "chat" ? (
                    <p className="muted">这一轮模型没有给出文字回答。</p>
                  ) : null}
                  {/* chat 那一轮不摆 Hero：回答才是主角；有结论就平铺列出。 */}
                  {turn.mode === "chat" ? (
                    turn.findings.length || turn.finished?.findings?.length ? (
                      <div className="hero-body">
                        {(turn.findings.length
                          ? turn.findings
                          : (turn.finished?.findings ?? [])
                        ).map((row) => (
                          <FindingRow
                            key={row.id}
                            finding={row}
                            evidenceCount={row.evidence_ids?.length ?? 0}
                            onEvidence={() => setSelectedAnchor(null)}
                          />
                        ))}
                      </div>
                    ) : null
                  ) : (
                    <ConclusionCard
                      tone={card.tone}
                      title={card.title}
                      detail={card.detail}
                      badge={<HeroBadge tone={card.tone}>{card.pill}</HeroBadge>}
                    >
                      {(turn.findings.length
                        ? turn.findings
                        : (turn.finished?.findings ?? [])
                      ).map((row) => (
                        <FindingRow
                          key={row.id}
                          finding={row}
                          evidenceCount={row.evidence_ids?.length ?? 0}
                          onEvidence={() => setSelectedAnchor(null)}
                        />
                      ))}
                    </ConclusionCard>
                  )}
                  {turn.feed.filter((item) => item.kind !== "delta").length ? (
                    <section className="timeline">
                      <div className="timeline-head">
                        调查过程 · {turn.finished?.steps ?? 0} 步 ·{" "}
                        {turn.finished?.tool_calls ?? 0} 次工具调用
                      </div>
                      <div className="timeline-body">
                        <div className="feed">
                        {turn.feed
                          .filter((item) => item.kind !== "delta")
                          .map((item, index) => (
                            <FeedLine key={`${index}-${item.kind}`} item={item} />
                          ))}
                        </div>
                      </div>
                    </section>
                  ) : null}
                </section>
              );
            })}

            {/*
              用户那一句：这一轮到底问的是什么，留在流里（放在历史之后）。
              **只有真跑过的轮次才有这一句**——打开 / 切回抓包不再留痕（2026-09-23）。
            */}
            {liveTurn ? (
              <div className="turn turn-user">
                <div className="user-bubble">
                  <div className="who">你</div>
                  <div>{asked}</div>
                </div>
              </div>
            ) : null}

            {/* Agent 这一段：结论 → 工具 → 证据 → 原始数据 → 报告。 */}
            {liveTurn ? (
              <section className="turn turn-agent">
                <div className="agent-head">
                  <span className="mark">◆</span> YeLee’ PacketSage
                </div>

                {/*
                  每轮的形态按模式分（v0.6）：
                  * `chat` 追问 = 一次**对话**：模型那段回答就是主角，结论/证据退到后面；
                  * `run` 调查 = 一次**汇报**：Hero 结论是主角，模型总结在它上面。
                */}
                {/* 这一轮还没开跑（刚打开抓包）：只可能是"调查"形态，不许套 chat 的兜底文案。 */}
                {liveMode === "chat" && (running || feed.length > 0 || finished || error) ? (
                  <>
                    {finished?.summary ? (
                      <div className="answer">
                        <MarkdownView text={finished.summary} />
                      </div>
                    ) : running ? (
                      <p className="answer-pending">
                        <span className="pulse" /> 正在追查这个问题…
                      </p>
                    ) : (
                      <p className="muted">
                        模型这次没有给出文字回答（只跑了工具）。可以再问一次，或看下面的过程。
                      </p>
                    )}
                    {!running && cardRows.length ? (
                      <div className="hero-body">
                        {cardRows.map((row) => (
                          <FindingRow
                            key={row.id}
                            finding={row}
                            evidenceCount={row.evidence_ids?.length ?? 0}
                            detail={findingDetails.get(row.id)}
                            onEvidence={() => scrollToEvidence(row.id)}
                          />
                        ))}
                      </div>
                    ) : null}
                    {running ? (
                      <div
                        className="progress"
                        title={`第 ${budgetState.steps} 步`}
                      >
                        <div
                          className="progress-fill"
                          style={{
                            width: `${Math.min(100, (budgetState.steps / STEP_BAR_CAP) * 100).toFixed(0)}%`,
                          }}
                        />
                      </div>
                    ) : null}
                  </>
                ) : (
                  <>
                    {/* 模型自己写的总结：对话里"它说了什么"，没有就不编。 */}
                    {finished?.summary ? (
                      <div className="assistant-note">
                        <MarkdownView text={finished.summary} />
                      </div>
                    ) : null}

                    {/* ── 第一层：结论 ─────────────────────────────── */}
                    <ConclusionCard
                      tone={hero.tone}
                      title={hero.title}
                      detail={hero.detail}
                      badge={<HeroBadge tone={hero.tone}>{hero.pill}</HeroBadge>}
                      stats={<StatRow items={heroStats} />}
                      actions={
                        running ? null : (
                          <button
                            className="outline"
                            onClick={() => void startInvestigation("run")}
                            disabled={investigationBlocked}
                            title={providerBlocked ? "先接上模型" : undefined}
                          >
                            {finished ? "重新调查" : "开始调查"}
                          </button>
                        )
                      }
                    >
                  {cardRows.map((row) => (
                    <FindingRow
                      key={row.id}
                      finding={row}
                      evidenceCount={row.evidence_ids?.length ?? 0}
                      detail={findingDetails.get(row.id)}
                          onEvidence={() => scrollToEvidence(row.id)}
                        />
                      ))}
                  {!running && !cardRows.length && hero.alerts.length
                    ? hero.alerts
                        .slice(0, 5)
                        .map((alert) => <AlertRow key={alert.alert_id} alert={alert} />)
                    : null}
                  {running ? (
                    <div
                      className="progress"
                      title={`第 ${budgetState.steps} 步`}
                    >
                      <div
                        className="progress-fill"
                        style={{
                          width: `${Math.min(100, (budgetState.steps / STEP_BAR_CAP) * 100).toFixed(0)}%`,
                        }}
                      />
                    </div>
                  ) : null}
                    </ConclusionCard>
                  </>
                )}

                {/* 建议问题：点一下就是一次 chat 追问，和底部输入框同一条路。 */}
                {!running && !investigationBlocked ? (
                  <Suggestions
                    items={SUGGESTED_QUESTIONS}
                    onPick={(text) => void startInvestigation("chat", text)}
                  />
                ) : null}

                {/*
                  ── 调查过程 = 证据时间线（v0.4 前置 + 默认展开）────────────
                  引擎预取 → 工具调用 → 结论，一条时间线走到底；每个工具节点都能
                  点开看入参与结果，每条结论就地挂着自己的 tc 锚点。
                  这是 YeLee’ PacketSage 与普通 chatbot 的差异点，所以它不藏在折叠里。
                */}
                {feed.length ? (
                  <section className="timeline" ref={timelineRef}>
                    {/* 调查本身**不是卡片**：一个标题行 + 一条时间轴，里面只有
                        工具调用是可展开的卡片（用户 2026-09-22 的明确要求）。 */}
                    <div className="timeline-head">
                      调查过程 · 引擎预取 → 工具调用 → 结论
                      {overview?.findings?.length ? ` · ${overview.findings.length} 条结论` : ""}
                    </div>
                    <div className="timeline-body">
                      <div className="feed" aria-live={running ? "polite" : "off"}>
                        {feed.map((item, index) => (
                          <FeedLine
                            key={`${index}-${item.kind}`}
                            item={item}
                            evidence={
                              item.kind === "finding"
                                ? (() => {
                                    const stored = (overview?.findings ?? []).find(
                                      (row) => row.finding_id === item.finding.id,
                                    );
                                    return stored ? (
                                      <FindingEvidence
                                        finding={stored}
                                        trace={overview?.trace ?? []}
                                        selected={selectedAnchor}
                                        onSelect={setSelectedAnchor}
                                      />
                                    ) : null;
                                  })()
                                : undefined
                            }
                          />
                        ))}
                        {running && !streamingNow ? (
                          <div className="feed-live">
                            <span className="pulse" /> 等模型下一步…
                          </div>
                        ) : null}
                      </div>
                    </div>
                  </section>
                ) : null}

              </section>
            ) : taskId ? (
              /*
                当前抓包的空态（v0.10）：**它不是一轮对话**——打开 / 切回抓包只在顶栏
                留一个文件名，这里只回答"现在能看什么、下一步做什么"。所以切十次抓包，
                会话流里的轮次数仍然等于真正跑过的 run / chat 次数。
              */
              <section className="turn turn-agent turn-idle">
                <div className="agent-head">
                  <span className="mark">◆</span> YeLee’ PacketSage
                </div>
                {history.length ? (
                  <ConclusionCard
                    tone="idle"
                    title={`这份抓包有 ${history.length} 轮调查记录`}
                    detail="上面是已经跑过的轮次，结论与证据都还在。要继续就追问一句，或者点「重新调查」再跑一遍。"
                    badge={<HeroBadge tone="idle">已就绪</HeroBadge>}
                    stats={<StatRow items={idleStats} />}
                    actions={
                      <button
                        className="outline"
                        onClick={() => void startInvestigation("run")}
                        disabled={investigationBlocked}
                        title={providerBlocked ? "先接上模型" : undefined}
                      >
                        重新调查
                      </button>
                    }
                  />
                ) : (
                  <ConclusionCard
                    tone={hero.tone}
                    title={hero.title}
                    detail={hero.detail}
                    badge={<HeroBadge tone={hero.tone}>{hero.pill}</HeroBadge>}
                    stats={<StatRow items={idleStats} />}
                    actions={
                      <button
                        className="outline"
                        onClick={() => void startInvestigation("run")}
                        disabled={investigationBlocked}
                        title={providerBlocked ? "先接上模型" : undefined}
                      >
                        开始调查
                      </button>
                    }
                  >
                    {hero.alerts
                      .slice(0, 5)
                      .map((alert) => <AlertRow key={alert.alert_id} alert={alert} />)}
                  </ConclusionCard>
                )}
                {!investigationBlocked ? (
                  <Suggestions
                    items={SUGGESTED_QUESTIONS}
                    onPick={(text) => void startInvestigation("chat", text)}
                  />
                ) : null}
              </section>
            ) : null}
          </div>
        </div>

        {running && !stick ? (
          <button className="jump-latest" onClick={jumpToLatest}>
            ↓ 回到最新
          </button>
        ) : null}

        {/* 常驻输入框：Enter 发送、Shift+Enter 换行（§5.1）。 */}
        <footer className="composer-wrap">
          <form
            className="composer"
            onSubmit={(event) => {
              event.preventDefault();
              if (!canSend) return;
              void startInvestigation("chat", goal);
              setGoal("");
            }}
          >
            <textarea
              ref={composerRef}
              name="goal"
              rows={1}
              value={goal}
              onChange={(event) => setGoal(event.target.value)}
              onKeyDown={(event) => {
                // Enter 发送、Shift+Enter 换行；中文输入法组词时不抢键。
                if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  if (canSend) {
                    void startInvestigation("chat", goal);
                    setGoal("");
                  }
                }
              }}
              placeholder={taskId ? "追问一句，比如「这份抓包里有扫描行为吗」" : "先打开一份抓包"}
              maxLength={4000}
              disabled={running || !taskId || investigationBlocked}
            />
            <div className="composer-row">
              <button
                type="button"
                className="icon"
                title="打开抓包并分析"
                onClick={pickCapture}
                disabled={running || blocked}
              >
                ＋
              </button>
              <span className="composer-context" title={summary?.source_path ?? ""}>
                {summary?.source_path
                  ? `📄 ${basename(summary.source_path)}`
                  : "还没有打开抓包"}
              </span>
              <span className="spacer" />
              {/* 模型 / 思考模式在输入框右下角（和主流 Agent 一致）。 */}
              <button
                type="button"
                className="composer-model"
                onClick={() => void toggleModelPanel()}
                disabled={broken}
                title="模型与思考模式"
              >
                {welcome?.providers.model || welcome?.providers.kind || "未配置"} ▾
              </button>
              {running ? (
                <button
                  type="button"
                  className="stop"
                  onClick={cancel}
                  disabled={status === "finalizing"}
                >
                  ■ {status === "finalizing" ? "正在收尾…" : "停止"}
                </button>
              ) : (
                <button className="send" type="submit" disabled={!canSend} title="发送（Enter）">
                  ↑
                </button>
              )}
            </div>

            {modelPanel ? (
              <div className="model-panel">
                <div className="model-panel-head">
                  <span>模型与思考模式</span>
                  <button className="icon" onClick={() => setModelPanel(false)} title="关闭">
                    ✕
                  </button>
                </div>
                <label className="model-panel-field">
                  <span className="label">模型</span>
                  <input
                    value={pendingModel}
                    onChange={(event) => setPendingModel(event.target.value)}
                    placeholder="deepseek-flash"
                    spellCheck={false}
                  />
                </label>
                {models === null ? (
                  <p className="muted">正在从端点拉模型列表…</p>
                ) : models.length ? (
                  <div className="model-chips">
                    {models.map((id) => (
                      <button
                        key={id}
                        className={`suggestion${id === pendingModel ? " seg-on" : ""}`}
                        onClick={() => setPendingModel(id)}
                      >
                        {id}
                      </button>
                    ))}
                  </div>
                ) : (
                  <p className="muted">端点没返回模型列表（可以直接手填模型名）。</p>
                )}
                <div className="model-panel-field">
                  <span className="label">思考模式</span>
                  <div className="seg">
                    {(
                      [
                        ["", "自动"],
                        ["enabled", "开"],
                        ["disabled", "关"],
                      ] as const
                    ).map(([value, label]) => (
                      <button
                        key={value || "auto"}
                        className={pendingThinking === value ? "seg-on" : ""}
                        onClick={() => setPendingThinking(value)}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="row">
                  <button
                    className="primary"
                    onClick={() => void saveModelPanel()}
                    disabled={savingModel || !welcome}
                  >
                    {savingModel ? "保存中…" : "保存并重启 Agent"}
                  </button>
                  <span className="muted">key 沿用已保存的那一个。</span>
                </div>
              </div>
            ) : null}
          </form>
        </footer>

        {/* 证据锚点的兜底入口：详情里点开的锚点在这里显示全量。 */}
        {selectedAnchor ? (
          <div className="anchor-bar">
            已选证据 <AnchorId id={selectedAnchor} />
            <button className="link" onClick={revealReport}>
              在报告中查看
            </button>
            <button className="link" onClick={() => setSelectedAnchor(null)}>
              取消
            </button>
          </div>
        ) : null}

        {/* M16：拖进窗口即分析（左栏那句提示得说话算话）。 */}
        {dragging ? <div className="drop-veil">松开就把这份抓包丢进分析</div> : null}

        {/*
          右栏（v0.10）：原来是"报告坞"，现在推广成**通用侧栏**——内容可切
          （抓包里有什么 / 图例 / 报告），开关在顶栏。对话留在左边，不抢屏、不覆盖。
        */}
        {dock ? (
          <aside className="dock" aria-label="右侧栏">
            <div className="dock-head">
              <div className="dock-tabs">
                {DOCK_VIEWS.map(([key, label]) => (
                  <button
                    key={key}
                    className={`dock-tab${dock === key ? " on" : ""}`}
                    onClick={() => openDock(key)}
                  >
                    {key === "report" && report ? `${label} · 已生成` : label}
                  </button>
                ))}
              </div>
              <span className="spacer" />
              {dock === "report" && report ? (
                <button className="icon" title="打开所在文件夹" onClick={openReportFolder}>
                  📁
                </button>
              ) : null}
              <button className="icon" title="收起右侧栏" onClick={() => setDock(null)}>
                ✕
              </button>
            </div>
            <div className="dock-body" ref={dock === "report" ? reportRef : undefined}>
              {dock === "capture" ? (
                <>
                  {summary ? (
                    <>
                      <CaptureOverview summary={summary} protocolCounts={summary.protocols} />
                      <p className="muted">
                        {overview?.alerts?.length
                          ? `规则引擎命中 ${overview.alerts.length} 条告警（下面按窗口列出）。`
                          : "规则引擎没有命中任何告警。"}
                      </p>
                    </>
                  ) : (
                    <p className="muted">还没有分析过抓包。</p>
                  )}
                  {overview?.alerts?.length
                    ? overview.alerts.map((alert) => (
                        <AlertRow key={alert.alert_id} alert={alert} />
                      ))
                    : null}
                </>
              ) : null}
              {dock === "legend" ? <LegendCards /> : null}
              {dock === "report" ? (
                <>
                  <ReportPanel
                    report={report}
                    busy={busy}
                    disabled={!taskId || blocked}
                    open
                    onToggle={() => setDock(null)}
                    onGenerate={() => void generateReport()}
                    onOpenFolder={openReportFolder}
                  />
                  {report ? (
                    <p className="muted report-path">
                      报告文件 <Mono>{report.report_path}</Mono>
                    </p>
                  ) : null}
                  {report ? (
                    <MarkdownView text={reportText} />
                  ) : (
                    <p className="muted">
                      还没有报告。点「生成报告」——它由引擎数据渲染，不需要模型。
                    </p>
                  )}
                </>
              ) : null}
            </div>
          </aside>
        ) : null}
      </main>

      {wizardOpen && !broken ? (
        <Wizard
          status={providerStatus}
          onConfigured={onWizardConfigured}
          onClose={() => setWizardOpen(false)}
        />
      ) : null}
    </div>
  );
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** 对话历史落在本机浏览器存储里，按 task 分桶（引擎库没有对话文本，见 `Turn`）。 */
const HISTORY_KEY = "ps.history.v1";

function loadHistory(taskId: string): Turn[] {
  if (!taskId) return [];
  try {
    const raw = localStorage.getItem(`${HISTORY_KEY}.${taskId}`);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as Turn[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function saveHistory(taskId: string, turns: Turn[]): void {
  if (!taskId) return;
  try {
    // token 碎块是装饰，不进历史（工具卡片、步骤、结论全留着）；只留最近 20 轮。
    const trimmed = turns.slice(-20).map((turn) => ({
      ...turn,
      feed: turn.feed.filter((item) => item.kind !== "delta"),
    }));
    localStorage.setItem(`${HISTORY_KEY}.${taskId}`, JSON.stringify(trimmed));
  } catch {
    // 存不下就算了：历史是便利，不是证据（证据在引擎库里）。
  }
}

/** 历史条目上的时间（只到分钟，够认了）。 */
function formatClock(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
}

/** 抓包列表上的"导入时间"：月-日 时:分（同名文件靠它区分）。 */
function formatStamp(iso: string | undefined): string {
  if (!iso) return "";
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return "";
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(
    date.getMinutes(),
  )}`;
}
