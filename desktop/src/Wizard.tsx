/**
 * 首次运行向导（《GUI 工程规格书 v0.2》§7.4 / U7）。
 *
 * 三步，顺序按"先让应用能用起来"排：
 *
 * 1. **模型 API Key**——选 provider、把 key 填进来、当场校验；没有 key 的人
 *    可以选"演示模式"（mock，文案写明是脚本回放，不是真实分析）；
 * 2. **模型**——`/models` 通过之后确认模型名（默认值就是文档里的那个）；
 * 3. **保存与自检**——key 进 Windows 凭据管理器、其余进 `provider.json`，
 *    重启 Agent sidecar，跑 doctor 十项，失败项带 `repair_hint` 与"重跑自检"。
 *
 * 它**不是**必须走完的关卡：任何时候都能"先不接模型"直接进主界面，
 * 那时分析、告警、证据链、报告都照常用，只有 run/chat 灰着。
 */

import { useCallback, useEffect, useState } from "react";

import {
  api,
  type AppInfo,
  type DoctorItem,
  type ProviderStatus,
  type VerifyResult,
} from "./api";
import { Banner, Mono, effortLabel } from "./components";
import type { Welcome } from "./types";

/** 与 agent 侧 `PROVIDER_DEFAULTS` 对齐；改一处必须改两处。 */
const PROVIDERS = [
  {
    id: "deepseek",
    name: "DeepSeek",
    hint: "api.deepseek.com · 需要 API key",
    model: "deepseek-flash",
    baseUrl: "https://api.deepseek.com/v1",
    needsKey: true,
  },
  {
    id: "openai",
    name: "OpenAI",
    hint: "api.openai.com · 需要 API key",
    model: "gpt-4o-mini",
    baseUrl: "https://api.openai.com/v1",
    needsKey: true,
  },
  {
    id: "local",
    name: "本地/自建端点",
    hint: "OpenAI 兼容的本地服务，不需要 key",
    model: "local-model",
    baseUrl: "http://localhost:8000/v1",
    needsKey: false,
  },
  {
    id: "mock",
    name: "演示模式（mock）",
    hint: "脚本回放，不是真实分析：不联网、不需要 key",
    model: "",
    baseUrl: "",
    needsKey: false,
  },
] as const;

type Step = 1 | 2 | 3;

export function Wizard({
  status,
  info,
  welcome,
  onConfigured,
  onClose,
}: {
  status: ProviderStatus | null;
  /** 外壳自报的参数（版本、两个 sidecar 的路径与数据目录）。 */
  info: AppInfo | null;
  /** 握手结果：Agent 版本、协议版本、引擎 schema 版本、能力。 */
  welcome: Welcome | null;
  /** 保存成功：把新的握手结果交回 App。 */
  onConfigured: (welcome: Welcome) => void;
  /** 关掉向导进主界面（"先不接模型"与第 3 步的"进入 PacketSage"）。 */
  onClose: () => void;
}) {
  const [step, setStep] = useState<Step>(1);
  const [kind, setKind] = useState("deepseek");
  const [model, setModel] = useState("deepseek-flash");
  const [baseUrl, setBaseUrl] = useState("https://api.deepseek.com/v1");
  const [apiKey, setApiKey] = useState("");
  /** 思考模式："" = 自动（DeepSeek 默认开）/ enabled / disabled。 */
  const [thinking, setThinking] = useState("");
  /** 推理强度："" = 自动（不显式传）/ low / high / max。 */
  const [effort, setEffort] = useState("");
  const [verify, setVerify] = useState<VerifyResult | null>(null);
  const [busy, setBusy] = useState<"verify" | "save" | "doctor" | "clear" | null>(null);
  const [error, setError] = useState<string>("");
  const [saved, setSaved] = useState<{ provider: string; model: string; key: boolean } | null>(null);
  const [doctor, setDoctor] = useState<DoctorItem[]>([]);

  // 已经配过的人重开向导：把当前值填回来，只用改想改的那一项。
  useEffect(() => {
    if (!status?.configured) return;
    const known = PROVIDERS.find((item) => item.id === status.provider);
    setKind(status.provider);
    setModel(status.model || known?.model || "");
    setBaseUrl(status.base_url || known?.baseUrl || "");
    setThinking(status.thinking || "");
    setEffort(status.effort || "");
  }, [status]);

  const selected = PROVIDERS.find((item) => item.id === kind) ?? PROVIDERS[0];
  const needsKey = selected.needsKey;

  const pick = useCallback((id: string) => {
    const next = PROVIDERS.find((item) => item.id === id);
    setKind(id);
    setVerify(null);
    setError("");
    if (next) {
      setModel(next.model);
      setBaseUrl(next.baseUrl);
    }
  }, []);

  const runVerify = useCallback(async () => {
    setBusy("verify");
    setError("");
    try {
      const result = await api.providerVerify({
        provider: kind,
        model,
        baseUrl,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        thinking,
        effort,
      });
      setVerify(result);
      // 校验通过（或本来就不需要 key）就直接进第二步；失败留在原地给人看原因。
      if (result.ok) setStep(2);
    } catch (exc) {
      setError(String(exc));
    } finally {
      setBusy(null);
    }
  }, [apiKey, baseUrl, effort, kind, model, thinking]);

  const runDoctor = useCallback(async () => {
    setBusy("doctor");
    try {
      const payload = await api.doctor();
      setDoctor(payload.items ?? []);
    } catch (exc) {
      setError(String(exc));
      setDoctor([]);
    } finally {
      setBusy(null);
    }
  }, []);

  /** 清除已保存的 key（凭据管理器里的那一条）；清了之后 run/chat 会被拒。 */
  const clearKey = useCallback(async () => {
    setBusy("clear");
    setError("");
    try {
      const result = await api.providerClear();
      setVerify(null);
      setApiKey("");
      onConfigured(result.welcome);
    } catch (exc) {
      setError(String(exc));
    } finally {
      setBusy(null);
    }
  }, [onConfigured]);

  const save = useCallback(async () => {
    setBusy("save");
    setError("");
    try {
      const result = await api.providerSave({
        provider: kind,
        model,
        baseUrl,
        apiKey: apiKey.trim() ? apiKey.trim() : null,
        thinking,
        effort,
      });
      setSaved({ provider: result.provider, model: result.model, key: result.key_stored });
      setApiKey("");
      setStep(3);
      await runDoctor();
      onConfigured(result.welcome);
    } catch (exc) {
      setError(String(exc));
    } finally {
      setBusy(null);
    }
  }, [apiKey, baseUrl, effort, kind, model, onConfigured, runDoctor, thinking]);

  const failed = doctor.filter((item) => item.status === "fail");
  const warned = doctor.filter((item) => item.status === "warn");

  /**
   * 本机参数（用户 2026-09-23：设置里看不到版本号、协议这些）。
   *
   * 只读、不猜：拿不到的写「—」。版本号全来自各自唯一的那份来源——外壳读
   * `Cargo.toml`、Agent 与协议读握手、引擎 schema 也读握手。
   */
  const params: { label: string; value: string }[] = [
    { label: "应用版本（外壳）", value: info?.version || "—" },
    { label: "Agent 版本", value: welcome?.agent_version || "—" },
    { label: "协议版本", value: welcome ? `v${welcome.protocol_version}` : "—" },
    { label: "引擎 schema 版本", value: welcome ? `v${welcome.schema_version}` : "—" },
    { label: "Agent 能力", value: welcome?.capabilities?.join(" · ") || "—" },
    { label: "provider", value: status?.provider || "—" },
    { label: "模型", value: status?.model || "—" },
    { label: "端点", value: status?.base_url || "—" },
    {
      label: "推理强度",
      value: status?.effort ? `${effortLabel(status.effort)}（${status.effort}）` : "自动",
    },
    {
      label: "思考模式",
      value:
        status?.thinking === "enabled"
          ? "开"
          : status?.thinking === "disabled"
            ? "关"
            : "自动（跟随模型默认）",
    },
    { label: "API key", value: status?.has_key ? "在 Windows 凭据管理器里" : "没存" },
    { label: "数据目录", value: info?.data_dir || "—" },
    { label: "引擎库", value: info?.db_url || "—" },
    {
      label: "引擎程序",
      value: info?.engine_program ? `${info.engine_program}（${info.engine}）` : "—",
    },
    {
      label: "Agent 程序",
      value: info?.agent_program ? `${info.agent_program}（${info.agent}）` : "—",
    },
    { label: "进程状态", value: info ? `引擎 ${info.engine_alive ? "在" : "不在"} · Agent ${info.agent_alive ? "在" : "不在"}` : "—" },
  ];

  return (
    <div className="wizard-backdrop">
      <div className="wizard">
        <header className="wizard-head">
          <div>
            <div className="brand">🛡️ YeLee’ PacketSage</div>
            <div className="muted">
              {status?.configured ? "设置 · 模型与 API key" : "第一次运行：把模型接上"}（第 {step} / 3
              步）
            </div>
          </div>
          <button className="wizard-skip" onClick={onClose}>
            {status?.configured ? "关闭" : "先不接模型，直接进界面"}
          </button>
        </header>

        <div className="wizard-steps">
          {(["模型 API Key", "模型", "保存与自检"] as const).map((label, index) => (
            <div
              key={label}
              className={`wizard-step ${step === index + 1 ? "wizard-step-active" : ""} ${
                step > index + 1 ? "wizard-step-done" : ""
              }`}
            >
              <span className="wizard-dot">{step > index + 1 ? "✓" : index + 1}</span>
              {label}
            </div>
          ))}
        </div>

        {error ? <Banner kind="error" title="出错了" detail={error} /> : null}

        {step === 1 ? (
          <section className="wizard-body">
            <h2>第 1 步 · 模型 API Key</h2>
            <p className="muted">
              key 只存进 <strong>Windows 凭据管理器</strong>（不写明文文件）；启动时以环境变量
              交给 Agent sidecar。填完会当场做一次 `GET /models` 校验。
            </p>

            <div className="wizard-grid">
              {PROVIDERS.map((item) => (
                <button
                  key={item.id}
                  className={`wizard-card ${kind === item.id ? "wizard-card-active" : ""}`}
                  onClick={() => pick(item.id)}
                >
                  <strong>{item.name}</strong>
                  <span className="muted">{item.hint}</span>
                </button>
              ))}
            </div>

            {needsKey ? (
              <>
                <label className="wizard-field">
                  <span className="label">{selected.name} API key（不回显）</span>
                  <input
                    type="password"
                    value={apiKey}
                    autoComplete="off"
                    spellCheck={false}
                    placeholder={status?.has_key ? "已保存一个 key；留空=沿用旧的" : "sk-…"}
                    onChange={(event) => setApiKey(event.target.value)}
                  />
                </label>
                <label className="wizard-field">
                  <span className="label">模型</span>
                  <input value={model} onChange={(event) => setModel(event.target.value)} />
                </label>
                {status?.has_key ? (
                  <div className="row">
                    <button
                      className="ghost"
                      onClick={() => void clearKey()}
                      disabled={busy !== null}
                    >
                      {busy === "clear" ? "清除中…" : "清除已保存的 key"}
                    </button>
                    <span className="muted">
                      清掉之后 run / chat 会被拒，直到重新填一个（引擎那半边照常可用）。
                    </span>
                  </div>
                ) : null}
                <label className="wizard-field">
                  <span className="label">推理强度</span>
                  <select value={effort} onChange={(event) => setEffort(event.target.value)}>
                    <option value="">自动（跟随模型默认，DeepSeek 为 high）</option>
                    <option value="low">低：最快，适合先看个大概</option>
                    <option value="high">高：默认档，结论更稳</option>
                    <option value="max">最高：最慢，复杂抓包用</option>
                  </select>
                  <span className="muted">
                    「思考」始终打开（DeepSeek 默认）：这里调的是它使多大劲。
                    要彻底关掉用 `PACKETSAGE_LLM_THINKING=disabled`。
                  </span>
                </label>
                <details className="wizard-advanced">
                  <summary>端点（默认就用 {selected.name} 的官方地址）</summary>
                  <label className="wizard-field">
                    <span className="label">base_url</span>
                    <input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} />
                  </label>
                </details>
              </>
            ) : (
              <>
                <p className="muted">
                  {kind === "mock"
                    ? "演示模式用确定性的脚本回放，不联网、不需要 key —— 报告里会写明这是回放，"
                    : "本地端点不需要 key，只要端点可达就好。"}
                </p>
                {kind === "local" ? (
                  <label className="wizard-field">
                    <span className="label">base_url</span>
                    <input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} />
                  </label>
                ) : null}
              </>
            )}

            {verify && !verify.ok ? (
              <Banner
                kind="warn"
                title="校验没过"
                detail={`${verify.detail}\n可以先修 key/端点再重试；也可以继续保存（run 会照常失败并报 PROVIDER_UNREACHABLE）。`}
                action="仍然继续"
                onAction={() => setStep(2)}
              />
            ) : null}

            <div className="wizard-actions">
              <button className="primary" onClick={() => void runVerify()} disabled={busy !== null}>
                {busy === "verify" ? "校验中…" : "校验并继续"}
              </button>
              {needsKey && !apiKey.trim() ? (
                <span className="muted">
                  没有 key？选「演示模式」先看完整流程，或点右上角直接进界面。
                </span>
              ) : null}
            </div>
          </section>
        ) : null}

        {step === 2 ? (
          <section className="wizard-body">
            <h2>第 2 步 · 模型</h2>
            <p className="muted">
              {verify?.ok ? `校验通过：${verify.detail}` : "没有做校验（或校验未通过）。"}
            </p>
            <label className="wizard-field">
              <span className="label">模型名</span>
              <input value={model} onChange={(event) => setModel(event.target.value)} />
            </label>
            <p className="muted">
              provider <Mono>{kind}</Mono>
              {baseUrl ? (
                <>
                  {" · "}
                  <Mono>{baseUrl}</Mono>
                </>
              ) : null}
              {needsKey ? ` · key ${apiKey.trim() ? "来自本次输入" : status?.has_key ? "沿用已保存的" : "还没给"}` : ""}
            </p>
            <div className="wizard-actions">
              <button onClick={() => setStep(1)}>返回</button>
              <button className="primary" onClick={() => void save()} disabled={busy !== null}>
                {busy === "save" ? "保存中…" : "保存并重启 Agent"}
              </button>
            </div>
          </section>
        ) : null}

        {step === 3 ? (
          <section className="wizard-body">
            <h2>第 3 步 · 保存与自检</h2>
            {saved ? (
              <div className="notice">
                ✅ 已保存：provider <Mono>{saved.provider}</Mono> · model{" "}
                <Mono>{saved.model || "-"}</Mono>
                {saved.key ? " · key 已写入 Windows 凭据管理器" : " · 不需要 key"}
                。Agent sidecar 已用新配置重启。
              </div>
            ) : null}

            <div className="row">
              <button onClick={() => void runDoctor()} disabled={busy !== null}>
                {busy === "doctor" ? "自检中…" : "重跑自检"}
              </button>
              <span className="muted">
                十项自检：{doctor.length ? `${doctor.length} 项，${failed.length} 失败` : "还没跑"}
              </span>
            </div>

            {failed.concat(warned).slice(0, 6).map((item) => (
              <div key={item.id} className={`check ${item.status === "fail" ? "check-fail" : ""}`}>
                <strong>
                  {item.status === "fail" ? "❌" : "⚠"} {item.name}
                </strong>
                <div className="muted">{item.detail}</div>
                {item.repair_hint ? <pre className="code">{item.repair_hint}</pre> : null}
              </div>
            ))}
            {!failed.length && doctor.length ? (
              <div className="notice">没有失败项，可以开始了。</div>
            ) : null}

            <div className="wizard-actions">
              <button onClick={() => setStep(2)}>返回</button>
              <button className="primary" onClick={onClose}>
                进入 YeLee’ PacketSage
              </button>
            </div>
          </section>
        ) : null}

        {/*
          本机参数（用户 2026-09-23：设置里看不到版本号、协议这些）。
          只读、随时可看：版本 / 协议 / 能力 / provider / 两个 sidecar 的路径与数据目录。
        */}
        <details className="wizard-advanced wizard-params">
          <summary>本机参数：版本 · 协议 · 路径</summary>
          <div className="params">
            {params.map((item) => (
              <div className="param-row" key={item.label}>
                <span className="param-label">{item.label}</span>
                <span className="param-value">
                  <Mono>{item.value}</Mono>
                </span>
              </div>
            ))}
          </div>
        </details>
      </div>
    </div>
  );
}
