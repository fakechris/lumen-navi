import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import { Onboarding } from "./Onboarding";
import type {
  ConfigSummary,
  EventSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  Permissions,
  SearchHit,
  TabId,
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
    title: "最近事件",
    blurb: "截图 / 音频 chunk / daemon 事件时间线",
  },
  {
    id: "settings",
    label: "设置",
    title: "设置",
    blurb: "隐私暂停 · 数据目录 · 引擎开关（只读摘要）",
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
  const [events, setEvents] = useState<EventSummary[]>([]);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingState | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [h, p, c, o, ob] = await Promise.all([
        api.getHealth(),
        api.getPermissions(),
        api.getConfigSummary(),
        api.observeStatus(),
        api.getOnboarding(),
      ]);
      setHealth(h);
      setPerms(p);
      setCfg(c);
      setObserve(o);
      setOnboarding(ob);
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

  useEffect(() => {
    if (tab === "activity") {
      void api
        .listEvents(80)
        .then(setEvents)
        .catch((e) => setError(String(e)));
    }
  }, [tab]);

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
                  <div className="meta">{cfg?.asr_locale ?? ""} · {cfg?.audio_chunk_ms ?? "—"}ms</div>
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
            <div className="list">
              {events.length === 0 && <div className="meta">暂无事件。启动 Observe 后会持续写入。</div>}
              {events.map((e) => (
                <div className="list-item" key={e.id}>
                  <div className="title">
                    {e.kind} <span className="meta">· {e.source}</span>
                  </div>
                  <div className="meta">
                    <span>{fmtTime(e.ts)}</span>
                    <span className="mono">{e.id.slice(0, 8)}</span>
                  </div>
                </div>
              ))}
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
                <div className="meta mt">
                  screen={String(cfg?.screen)} · audio={String(cfg?.audio)} · ocr=
                  {String(cfg?.ocr)} · asr={String(cfg?.asr)}
                </div>
                <div className="meta">
                  api={cfg?.api_bind} · chunk={cfg?.audio_chunk_ms}ms · locale=
                  {cfg?.asr_locale}
                </div>
                <p className="meta mt">
                  详细开关请编辑 <span className="mono">navi.toml</span>
                  。桌面壳负责控制与检索；采集逻辑由 <span className="mono">lumen-daemon</span> 执行。
                </p>
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
