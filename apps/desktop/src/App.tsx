import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import { Onboarding } from "./Onboarding";
import type {
  AsrModelStatus,
  AssistantConfig,
  AssistantUpdate,
  ConfigSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  Permissions,
  SearchHit,
  TabId,
  TimelineItem,
} from "./types";

const NAV: { id: TabId; label: string; title: string; blurb: string }[] = [
  {
    id: "overview",
    label: "概览",
    title: "概览",
    blurb: "权限 · 摄入状态 · 一键开始 Observe",
  },
  {
    id: "search",
    label: "搜索",
    title: "全文搜索",
    blurb: "OCR + 语音转写（同一 FTS 索引）",
  },
  {
    id: "activity",
    label: "活动",
    title: "时间线",
    blurb: "缩略图 · OCR/转写预览 · 按类型/应用过滤",
  },
  {
    id: "settings",
    label: "设置",
    title: "设置",
    blurb: "源开关 · 隐私 · 日摘要 · 数据目录",
  },
];

function fmtTime(iso?: string | null): string {
  if (!iso) return "—";
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function permPill(v: string): "ok" | "warn" | "err" {
  const s = v.toLowerCase();
  if (s.includes("granted")) return "ok";
  if (s.includes("denied") || s.includes("restricted")) return "err";
  return "warn";
}

export default function App() {
  const [tab, setTab] = useState<TabId>("overview");
  const [health, setHealth] = useState<Health | null>(null);
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [cfg, setCfg] = useState<ConfigSummary | null>(null);
  const [observe, setObserve] = useState<ObserveStatus | null>(null);
  const [timeline, setTimeline] = useState<TimelineItem[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [kindFilter, setKindFilter] = useState("");
  const [appFilter, setAppFilter] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingState | null>(null);
  const [summaryText, setSummaryText] = useState<string | null>(null);
  const [asrModels, setAsrModels] = useState<AsrModelStatus | null>(null);
  const [assistant, setAssistant] = useState<AssistantConfig | null>(null);
  const [assistantKey, setAssistantKey] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [h, p, c, o, ob, models, asst] = await Promise.all([
        api.getHealth(),
        api.getPermissions(),
        api.getConfigSummary(),
        api.observeStatus(),
        api.getOnboarding(),
        api.checkAsrModelStatus(),
        api.assistantGetConfig(),
      ]);
      setHealth(h);
      setPerms(p);
      setCfg(c);
      setObserve(o);
      setOnboarding(ob);
      setAsrModels(models);
      setAssistant(asst);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), 4000);
    return () => clearInterval(t);
  }, [refresh]);

  const loadTimeline = useCallback(async () => {
    try {
      const items = await api.listTimeline({
        limit: 60,
        kindContains: kindFilter || undefined,
        appContains: appFilter || undefined,
      });
      setTimeline(items);
      setError(null);
      // Lazy-load a few image thumbs
      const need = items.filter((i) => i.has_image).slice(0, 12);
      const next: Record<string, string> = {};
      await Promise.all(
        need.map(async (i) => {
          try {
            const url = await api.getEventImageDataUrl(i.id);
            if (url) next[i.id] = url;
          } catch {
            /* ignore */
          }
        }),
      );
      setThumbs((prev) => ({ ...prev, ...next }));
    } catch (e) {
      setError(String(e));
    }
  }, [kindFilter, appFilter]);

  useEffect(() => {
    if (tab === "activity") {
      void loadTimeline();
    }
  }, [tab, loadTimeline]);

  const nav = NAV.find((n) => n.id === tab)!;

  const startObserve = useCallback(async () => {
    setBusy(true);
    setStatusNote(null);
    try {
      const o = await api.observeStart();
      setObserve(o);
      setStatusNote(
        o.running
          ? `Observe 已启动${o.pid ? ` (pid ${o.pid})` : ""}`
          : "未能启动",
      );
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const stopObserve = useCallback(async () => {
    setBusy(true);
    try {
      const o = await api.observeStop();
      setObserve(o);
      setStatusNote("Observe 已停止");
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const togglePause = useCallback(async () => {
    if (!cfg) return;
    setBusy(true);
    try {
      await api.setPrivacyPaused(!cfg.paused);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [cfg, refresh]);

  const updateAssistant = useCallback(async (update: AssistantUpdate) => {
    setBusy(true);
    try {
      const a = await api.assistantUpdateConfig(update);
      setAssistant(a);
      setStatusNote("划词助手配置已保存。");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  // Tray menu → UI actions
  useEffect(() => {
    const unsubs: Array<() => void> = [];
    void listen("tray://observe-start", () => {
      void startObserve();
    }).then((u) => unsubs.push(u));
    void listen("tray://observe-stop", () => {
      void stopObserve();
    }).then((u) => unsubs.push(u));
    void listen("tray://toggle-pause", () => {
      void togglePause();
    }).then((u) => unsubs.push(u));
    return () => {
      unsubs.forEach((u) => u());
    };
  }, [startObserve, stopObserve, togglePause]);

  async function onSearch() {
    setBusy(true);
    try {
      const r = await api.searchText(query.trim(), 40);
      setHits(r);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function reindex() {
    setBusy(true);
    try {
      const n = await api.reindexSearch();
      setStatusNote(`已重建搜索索引：${n} 篇`);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="app">
      {onboarding?.needs_onboarding && (
        <Onboarding initial={onboarding} onDone={() => void refresh()} />
      )}
      <aside className="sidebar">
        <div className="brand">
          Lumen <span>Navi</span>
        </div>
        {NAV.map((n) => (
          <button
            key={n.id}
            className={`nav-btn ${tab === n.id ? "active" : ""}`}
            onClick={() => setTab(n.id)}
          >
            {n.label}
          </button>
        ))}
      </aside>

      <main className="main">
        <header className="header">
          <h1>{nav.title}</h1>
          <p>{nav.blurb}</p>
        </header>

        {error && <div className="error">{error}</div>}
        {statusNote && !error && (
          <div className="error" style={{ color: "var(--ok)", background: "color-mix(in srgb, var(--ok) 12%, var(--card))", borderColor: "color-mix(in srgb, var(--ok) 25%, var(--border))" }}>
            {statusNote}
          </div>
        )}

        <div className="content">
          {tab === "overview" && (
            <div className="stack">
              <div className="row">
                {observe?.running ? (
                  <button className="btn danger" disabled={busy} onClick={() => void stopObserve()}>
                    停止 Observe
                  </button>
                ) : (
                  <button className="btn primary" disabled={busy} onClick={() => void startObserve()}>
                    开始 Observe
                  </button>
                )}
                <button className="btn" disabled={busy} onClick={() => void refresh()}>
                  刷新
                </button>
                <button className="btn" disabled={busy} onClick={() => void togglePause()}>
                  {cfg?.paused ? "恢复采集" : "隐私暂停"}
                </button>
                <span className={`pill ${observe?.running ? "ok" : "warn"}`}>
                  {observe?.running ? "Running" : "Stopped"}
                </span>
                {cfg?.paused && <span className="pill warn">Paused</span>}
              </div>

              <div className="grid mt">
                <div className="card">
                  <h3>Events</h3>
                  <div className="value">{health?.stored_events ?? "—"}</div>
                  <div className="meta">schema v{health?.schema_version ?? "—"}</div>
                </div>
                <div className="card">
                  <h3>Search docs</h3>
                  <div className="value">{health?.ocr_docs ?? "—"}</div>
                  <div className="meta">OCR + transcripts</div>
                </div>
                <div className="card">
                  <h3>Screen</h3>
                  <div className="value" style={{ fontSize: 16 }}>
                    {health?.sources.find((s) => s.id === "screen")?.enabled
                      ? health.sources.find((s) => s.id === "screen")?.running
                        ? "运行中"
                        : "已启用"
                      : "关闭"}
                  </div>
                </div>
                <div className="card">
                  <h3>Audio / ASR</h3>
                  <div className="value" style={{ fontSize: 16 }}>
                    {cfg?.audio ? (cfg.asr ? "摄入+转写" : "仅摄入") : "关闭"}
                  </div>
                  <div className="meta">
                    {cfg?.asr_engine ?? "sensevoice"} · {cfg?.asr_locale ?? ""} ·{" "}
                    {cfg?.audio_chunk_ms ?? "—"}ms
                  </div>
                </div>
              </div>

              <div className="card mt">
                <h3>Permissions</h3>
                <div className="row mt">
                  <span className={`pill ${permPill(perms?.screen_recording ?? "")}`}>
                    Screen {perms?.screen_recording ?? "—"}
                  </span>
                  <span className={`pill ${permPill(perms?.microphone ?? "")}`}>
                    Mic {perms?.microphone ?? "—"}
                  </span>
                  <span className={`pill ${permPill(perms?.accessibility ?? "")}`}>
                    AX {perms?.accessibility ?? "—"}
                  </span>
                </div>
                <p className="meta mt">
                  首次截屏/录音时系统会弹权限。Speech Recognition 用于本地转写（非听写注入）。
                  听写产品见 Lumen ASR。
                </p>
              </div>
            </div>
          )}

          {tab === "search" && (
            <div className="stack">
              <div className="row">
                <input
                  type="search"
                  placeholder="搜索屏幕文字或转写…"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void onSearch();
                  }}
                />
                <button className="btn primary" disabled={busy || !query.trim()} onClick={() => void onSearch()}>
                  搜索
                </button>
                <button className="btn" disabled={busy} onClick={() => void reindex()}>
                  重建索引
                </button>
              </div>
              <div className="list">
                {hits.length === 0 && (
                  <div className="meta">输入关键词搜索 OCR / transcript 全文索引。</div>
                )}
                {hits.map((h) => (
                  <div className="list-item" key={h.event_id}>
                    <div className="snippet" dangerouslySetInnerHTML={{ __html: escapeHtml(h.snippet) }} />
                    <div className="meta">
                      <span>{fmtTime(h.event_ts)}</span>
                      <span className="mono">{h.event_id.slice(0, 8)}</span>
                      <span>conf {h.confidence.toFixed(2)}</span>
                    </div>
                    <div className="meta">{h.text_preview}</div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {tab === "activity" && (
            <div className="stack">
              <div className="row">
                <select
                  value={kindFilter}
                  onChange={(e) => setKindFilter(e.target.value)}
                  style={{ height: 34, borderRadius: 9, border: "1px solid var(--border)", background: "var(--card)", color: "var(--text)", padding: "0 8px" }}
                >
                  <option value="">全部类型</option>
                  <option value="screenshot">screenshot</option>
                  <option value="audio_chunk">audio_chunk</option>
                  <option value="summary">summary</option>
                  <option value="daemon">daemon</option>
                </select>
                <input
                  type="text"
                  placeholder="过滤应用 / 标题 / 文本…"
                  value={appFilter}
                  onChange={(e) => setAppFilter(e.target.value)}
                />
                <button className="btn" disabled={busy} onClick={() => void loadTimeline()}>
                  刷新
                </button>
                <button
                  className="btn primary"
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    void api
                      .generateDaySummary()
                      .then((body) => {
                        try {
                          const v = JSON.parse(body) as { text?: string };
                          setSummaryText(v.text ?? body);
                        } catch {
                          setSummaryText(body);
                        }
                        return loadTimeline();
                      })
                      .catch((e) => setError(String(e)))
                      .finally(() => setBusy(false));
                  }}
                >
                  生成今日摘要
                </button>
              </div>
              {summaryText && (
                <div className="card">
                  <h3>Day summary</h3>
                  <pre className="meta mt" style={{ whiteSpace: "pre-wrap", margin: 0 }}>
                    {summaryText}
                  </pre>
                </div>
              )}
              <div className="list">
                {timeline.length === 0 && (
                  <div className="meta">暂无事件。启动 Observe 后会持续写入。</div>
                )}
                {timeline.map((e) => (
                  <div className="list-item timeline-row" key={e.id}>
                    {e.has_image && thumbs[e.id] ? (
                      <img className="thumb" src={thumbs[e.id]} alt="" />
                    ) : e.has_image ? (
                      <div className="thumb placeholder">img</div>
                    ) : e.kind.includes("audio") ? (
                      <div className="thumb placeholder">♪</div>
                    ) : (
                      <div className="thumb placeholder">·</div>
                    )}
                    <div className="timeline-body">
                      <div className="title">
                        {e.app_name || e.kind}
                        <span className="meta">
                          {" "}
                          · {e.kind}
                          {e.window_title ? ` · ${e.window_title}` : ""}
                        </span>
                      </div>
                      {e.text_preview && (
                        <div className="snippet">{e.text_preview}</div>
                      )}
                      <div className="meta">
                        <span>{fmtTime(e.ts)}</span>
                        <span className="mono">{e.id.slice(0, 8)}</span>
                        {e.text_kind && <span>{e.text_kind}</span>}
                        {e.artifact_bytes != null && <span>{Math.round(e.artifact_bytes / 1024)} KB</span>}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {tab === "settings" && (
            <div className="stack">
              <div className="card">
                <h3>Data</h3>
                <p className="mono mt">{cfg?.data_dir ?? "—"}</p>
                <p className="meta">config: {cfg?.config_path ?? "—"}</p>
                <div className="row mt">
                  <button className="btn" onClick={() => void api.openDataDir()}>
                    在 Finder 中打开
                  </button>
                </div>
              </div>
              <div className="card">
                <h3>Sources / engines</h3>
                <div className="stack mt">
                  {(
                    [
                      ["screen", "屏幕截图", cfg?.screen],
                      ["audio", "麦克风", cfg?.audio],
                      ["ocr", "OCR", cfg?.ocr],
                      ["asr", "ASR 转写", cfg?.asr],
                    ] as const
                  ).map(([key, label, val]) => (
                    <label className="check" key={key}>
                      <input
                        type="checkbox"
                        checked={!!val}
                        onChange={(e) => {
                          const checked = e.target.checked;
                          setBusy(true);
                          void api
                            .updateSourcesConfig({ [key]: checked })
                            .then((c) => {
                              setCfg(c);
                              setStatusNote(
                                "配置已写入。若 Observe 正在运行，请 Stop 再 Start 以生效。",
                              );
                            })
                            .catch((err) => setError(String(err)))
                            .finally(() => setBusy(false));
                        }}
                      />
                      {label}
                    </label>
                  ))}
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!cfg?.system_audio}
                      onChange={(e) => {
                        const checked = e.target.checked;
                        setBusy(true);
                        void api
                          .updateSourcesConfig({ system_audio: checked })
                          .then((c) => {
                            setCfg(c);
                            setStatusNote(
                              checked
                                ? "system_audio 已标记（ScreenCaptureKit 捕获尚未实现，仅配置位）。"
                                : "system_audio 已关闭。",
                            );
                          })
                          .catch((err) => setError(String(err)))
                          .finally(() => setBusy(false));
                      }}
                    />
                    系统音频（预留，未实现）
                  </label>
                </div>
                <div className="stack mt">
                  <label className="field">
                    <span className="meta">持续 ASR 引擎</span>
                    <select
                      className="input"
                      value={cfg?.asr_engine ?? "sensevoice"}
                      onChange={(e) => {
                        const asr_engine = e.target.value;
                        setBusy(true);
                        void api
                          .updateSourcesConfig({ asr_engine })
                          .then((c) => {
                            setCfg(c);
                            setStatusNote(
                              `ASR 引擎 → ${asr_engine}。Stop/Start Observe 后生效。`,
                            );
                          })
                          .catch((err) => setError(String(err)))
                          .finally(() => setBusy(false));
                      }}
                    >
                      <option value="sensevoice">SenseVoice（本地 sherpa，默认）</option>
                      <option value="whisper">Whisper（本地 sherpa）</option>
                      <option value="speech">macOS Speech</option>
                      <option value="openai_audio">OpenAI 兼容 HTTP</option>
                      <option value="qwen">Qwen ASR（HTTP，如 0.8B）</option>
                    </select>
                  </label>
                  <label className="field">
                    <span className="meta">ASR locale</span>
                    <input
                      className="input"
                      value={cfg?.asr_locale ?? "zh-CN"}
                      onChange={(e) => {
                        const asr_locale = e.target.value;
                        setCfg((prev) =>
                          prev ? { ...prev, asr_locale } : prev,
                        );
                      }}
                      onBlur={() => {
                        if (!cfg?.asr_locale) return;
                        setBusy(true);
                        void api
                          .updateSourcesConfig({ asr_locale: cfg.asr_locale })
                          .then((c) => setCfg(c))
                          .catch((err) => setError(String(err)))
                          .finally(() => setBusy(false));
                      }}
                    />
                  </label>
                  {(cfg?.asr_engine === "openai_audio" ||
                    cfg?.asr_engine === "qwen") && (
                    <>
                      <label className="field">
                        <span className="meta">HTTP base URL（…/v1）</span>
                        <input
                          className="input"
                          placeholder="https://dashscope.aliyuncs.com/compatible-mode/v1"
                          value={cfg?.asr_http_base_url ?? ""}
                          onChange={(e) => {
                            const asr_http_base_url = e.target.value;
                            setCfg((prev) =>
                              prev ? { ...prev, asr_http_base_url } : prev,
                            );
                          }}
                          onBlur={() => {
                            setBusy(true);
                            void api
                              .updateSourcesConfig({
                                asr_http_base_url: cfg?.asr_http_base_url ?? "",
                              })
                              .then((c) => setCfg(c))
                              .catch((err) => setError(String(err)))
                              .finally(() => setBusy(false));
                          }}
                        />
                      </label>
                      <label className="field">
                        <span className="meta">HTTP model</span>
                        <input
                          className="input"
                          placeholder="qwen3-asr-0.8b"
                          value={cfg?.asr_http_model ?? ""}
                          onChange={(e) => {
                            const asr_http_model = e.target.value;
                            setCfg((prev) =>
                              prev ? { ...prev, asr_http_model } : prev,
                            );
                          }}
                          onBlur={() => {
                            setBusy(true);
                            void api
                              .updateSourcesConfig({
                                asr_http_model: cfg?.asr_http_model ?? "",
                              })
                              .then((c) => setCfg(c))
                              .catch((err) => setError(String(err)))
                              .finally(() => setBusy(false));
                          }}
                        />
                      </label>
                      <p className="meta">
                        API key 写入 <span className="mono">navi.toml</span> 的{" "}
                        <span className="mono">asr.http_api_key</span>，或环境变量{" "}
                        <span className="mono">LUMEN_NAVI_ASR_API_KEY</span>。
                      </p>
                    </>
                  )}
                  {(cfg?.asr_engine === "sensevoice" ||
                    cfg?.asr_engine === "whisper") && (
                    <>
                      {asrModels && (
                        <div className="onboard-status">
                          <div className="meta">Lumen 共享模型目录</div>
                          <p
                            className="meta mono"
                            style={{ wordBreak: "break-all", marginTop: 4 }}
                          >
                            {asrModels.models_root}
                          </p>
                          {asrModels.candidates
                            .filter(
                              (candidate) =>
                                candidate.ready && candidate.engine === cfg?.asr_engine,
                            )
                            .map((candidate) => (
                              <div
                                key={`${candidate.engine}:${candidate.path}`}
                                className="onboard-candidate"
                              >
                                <span className="meta" style={{ wordBreak: "break-all" }}>
                                  {candidate.label}
                                </span>
                                <button
                                  type="button"
                                  className="btn"
                                  disabled={busy}
                                  onClick={() => {
                                    setBusy(true);
                                    void api
                                      .useExistingAsrModel(candidate.path, candidate.engine)
                                      .then((status) => {
                                        setAsrModels(status);
                                        return api.getConfigSummary();
                                      })
                                      .then((config) => setCfg(config))
                                      .catch((err) => setError(String(err)))
                                      .finally(() => setBusy(false));
                                  }}
                                >
                                  使用
                                </button>
                              </div>
                            ))}
                        </div>
                      )}
                      <label className="field">
                        <span className="meta">本地模型目录（可空=自动）</span>
                        <input
                          className="input"
                          placeholder="~/Library/Application Support/Lumen/models/sensevoice"
                          value={cfg?.asr_model_dir ?? ""}
                          onChange={(e) => {
                            const asr_model_dir = e.target.value;
                            setCfg((prev) =>
                              prev ? { ...prev, asr_model_dir } : prev,
                            );
                          }}
                        />
                      </label>
                      <button
                        type="button"
                        className="btn"
                        disabled={busy || !(cfg?.asr_model_dir ?? "").trim()}
                        onClick={() => {
                          setBusy(true);
                          void api
                            .useExistingAsrModel(
                              (cfg?.asr_model_dir ?? "").trim(),
                              cfg?.asr_engine,
                            )
                            .then((status) => {
                              setAsrModels(status);
                              return api.getConfigSummary();
                            })
                            .then((config) => setCfg(config))
                            .catch((err) => setError(String(err)))
                            .finally(() => setBusy(false));
                        }}
                      >
                        验证并使用此目录
                      </button>
                      {!!cfg?.asr_model_dir && (
                        <button
                          type="button"
                          className="btn"
                          disabled={busy}
                          onClick={() => {
                            setBusy(true);
                            void api
                              .updateSourcesConfig({ asr_model_dir: "" })
                              .then((config) => {
                                setCfg(config);
                                return api.checkAsrModelStatus();
                              })
                              .then((status) => setAsrModels(status))
                              .catch((err) => setError(String(err)))
                              .finally(() => setBusy(false));
                          }}
                        >
                          恢复自动发现
                        </button>
                      )}
                      {cfg?.asr_engine === "sensevoice" && (
                        <div className="row">
                          <button
                            className="btn"
                            disabled={busy}
                            onClick={() => {
                              setBusy(true);
                              void api
                                .checkAsrModelStatus()
                                .then((s) => {
                                  setAsrModels(s);
                                  setStatusNote(
                                    s.sensevoice_ready
                                      ? `SenseVoice 就绪 · ${s.sensevoice_dir}`
                                      : `SenseVoice 未就绪 · 可下载到 ${s.sensevoice_dir}`,
                                  );
                                  if (s.active_model_dir) {
                                    setCfg((prev) =>
                                      prev
                                        ? {
                                            ...prev,
                                            asr_model_dir: s.active_model_dir,
                                            asr_engine: s.active_engine,
                                          }
                                        : prev,
                                    );
                                  }
                                })
                                .catch((err) => setError(String(err)))
                                .finally(() => setBusy(false));
                            }}
                          >
                            检查模型
                          </button>
                          <button
                            className="btn primary"
                            disabled={busy}
                            onClick={() => {
                              setBusy(true);
                              setStatusNote("正在下载 SenseVoice…");
                              void api
                                .startAsrModelDownload()
                                .then((s) => {
                                  setAsrModels(s);
                                  setStatusNote(
                                    s.sensevoice_ready
                                      ? `SenseVoice 已安装 · ${s.sensevoice_dir}`
                                      : "下载完成但未检测到模型",
                                  );
                                  return api.getConfigSummary();
                                })
                                .then((c) => setCfg(c))
                                .catch((err) => setError(String(err)))
                                .finally(() => setBusy(false));
                            }}
                          >
                            下载 SenseVoice
                          </button>
                        </div>
                      )}
                    </>
                  )}
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={cfg?.asr_fallback_speech ?? true}
                      onChange={(e) => {
                        const checked = e.target.checked;
                        setBusy(true);
                        void api
                          .updateSourcesConfig({ asr_fallback_speech: checked })
                          .then((c) => {
                            setCfg(c);
                            setStatusNote(
                              checked
                                ? "本地模型不可用时回退 macOS Speech。"
                                : "已关闭 Speech 回退。",
                            );
                          })
                          .catch((err) => setError(String(err)))
                          .finally(() => setBusy(false));
                      }}
                    />
                    本地引擎不可用时回退 Speech
                  </label>
                </div>
                <div className="meta mt">
                  api={cfg?.api_bind} · chunk={cfg?.audio_chunk_ms}ms · engine=
                  {cfg?.asr_engine ?? "—"} · locale={cfg?.asr_locale}
                </div>
                <p className="meta mt">
                  开关写入 <span className="mono">navi.toml</span>
                  。采集进程需重启后读取新配置。
                </p>
              </div>
              <div className="card">
                <h3>划词助手（选中文字 → 翻译 / 问答）</h3>
                <div className="stack mt">
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!assistant?.popup_enabled}
                      onChange={(e) =>
                        void updateAssistant({ popup_enabled: e.target.checked })
                      }
                    />
                    鼠标划词后自动弹出面板
                  </label>
                  {assistant?.popup_enabled &&
                    !assistant?.accessibility_trusted && (
                      <div>
                        <p className="meta">
                          需要「辅助功能」权限来读取其他应用中的选中文字。
                          授权后几秒内自动生效，无需重启。
                        </p>
                        <div className="row mt">
                          <button
                            className="btn"
                            disabled={busy}
                            onClick={() => {
                              setBusy(true);
                              void api
                                .requestAccessibilityPermission()
                                .then(() => api.assistantGetConfig())
                                .then((a) => setAssistant(a))
                                .catch((err) => setError(String(err)))
                                .finally(() => setBusy(false));
                            }}
                          >
                            请求权限
                          </button>
                          <button
                            className="btn"
                            onClick={() =>
                              void api.openPrivacySettings("accessibility")
                            }
                          >
                            打开系统设置
                          </button>
                        </div>
                      </div>
                    )}
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!assistant?.enabled}
                      onChange={(e) =>
                        void updateAssistant({ enabled: e.target.checked })
                      }
                    />
                    启用助手（点击动作时把选中文字发给 LLM）
                  </label>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!assistant?.clipboard_fallback}
                      onChange={(e) =>
                        void updateAssistant({
                          clipboard_fallback: e.target.checked,
                        })
                      }
                    />
                    无 AX 应用（钉钉文档 / 终端）用 ⌘C 兜底取词（读取后立即恢复剪贴板）
                  </label>
                  <label className="field">
                    <span className="meta">LLM base URL（OpenAI 兼容 …/v1）</span>
                    <input
                      className="input"
                      placeholder="https://api.openai.com/v1"
                      value={assistant?.base_url ?? ""}
                      onChange={(e) => {
                        const base_url = e.target.value;
                        setAssistant((prev) =>
                          prev ? { ...prev, base_url } : prev,
                        );
                      }}
                      onBlur={() => void updateAssistant({ base_url: assistant?.base_url ?? "" })}
                    />
                  </label>
                  <label className="field">
                    <span className="meta">模型</span>
                    <input
                      className="input"
                      placeholder="gpt-4o-mini"
                      value={assistant?.model ?? ""}
                      onChange={(e) => {
                        const model = e.target.value;
                        setAssistant((prev) =>
                          prev ? { ...prev, model } : prev,
                        );
                      }}
                      onBlur={() => void updateAssistant({ model: assistant?.model ?? "" })}
                    />
                  </label>
                  <label className="field">
                    <span className="meta">翻译目标语言</span>
                    <input
                      className="input"
                      placeholder="中文"
                      value={assistant?.target_lang ?? ""}
                      onChange={(e) => {
                        const target_lang = e.target.value;
                        setAssistant((prev) =>
                          prev ? { ...prev, target_lang } : prev,
                        );
                      }}
                      onBlur={() =>
                        void updateAssistant({ target_lang: assistant?.target_lang ?? "" })
                      }
                    />
                  </label>
                  <label className="field">
                    <span className="meta">
                      API key（{assistant?.api_key_set ? "已配置，输入以更换" : "未配置"}）
                    </span>
                    <input
                      className="input"
                      type="password"
                      placeholder="sk-…"
                      value={assistantKey}
                      onChange={(e) => setAssistantKey(e.target.value)}
                      onBlur={() => {
                        const k = assistantKey.trim();
                        if (k) {
                          void updateAssistant({ api_key: k });
                          setAssistantKey("");
                        }
                      }}
                    />
                  </label>
                  {assistant?.api_key_set && (
                    <div className="row">
                      <button
                        className="btn"
                        disabled={busy}
                        onClick={() => void updateAssistant({ api_key: "" })}
                      >
                        清除 API key
                      </button>
                    </div>
                  )}
                  <p className="meta">
                    写入 <span className="mono">navi.toml</span> 的{" "}
                    <span className="mono">assistant</span> 段；也可用环境变量{" "}
                    <span className="mono">LUMEN_NAVI_LLM_API_KEY</span>。选中文字仅在
                    你点击「翻译 / 提问」时发送，不会被采集或存储。
                  </p>
                </div>
              </div>
              <div className="card">
                <h3>Shell</h3>
                <label className="check mt">
                  <input
                    type="checkbox"
                    checked={!!onboarding?.launch_observe}
                    onChange={(e) => {
                      void api
                        .setLaunchObserve(e.target.checked)
                        .then(() => refresh());
                    }}
                  />
                  启动应用时自动开始 Observe
                </label>
                <div className="row mt">
                  <button
                    className="btn"
                    onClick={() => void api.reopenOnboarding().then(() => refresh())}
                  >
                    重新打开首次引导
                  </button>
                </div>
              </div>
              <div className="card">
                <h3>Related</h3>
                <p className="meta mt">
                  听写/热键注入 →{" "}
                  <a href="https://github.com/fakechris/lumen-asr" target="_blank" rel="noreferrer">
                    Lumen ASR
                  </a>
                  （独立产品，不合并 monorepo）
                </p>
                <p className="meta">菜单栏托盘可 Start/Stop Observe、暂停与退出。</p>
              </div>
            </div>
          )}
        </div>
      </main>
    </div>
  );
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/「/g, "<mark>")
    .replace(/」/g, "</mark>");
}
