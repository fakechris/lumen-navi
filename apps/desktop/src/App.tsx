import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import { Onboarding } from "./Onboarding";
import { DashboardView } from "./views/Dashboard";
import {
  Button,
  EmptyState,
  Icon,
  Input,
  NavItem,
  Notice,
  Pill,
  Select,
  StatCard,
  StatusDot,
  ThemeToggle,
} from "./design";
import type { IconName } from "./design";
import type {
  AsrModelStatus,
  AssistantConfig,
  AssistantUpdate,
  BrowserPairing,
  ConfigSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  Permissions,
  SearchHit,
  SourcesUpdate,
  TabId,
  TimelineItem,
} from "./types";

const NAV: {
  id: TabId;
  label: string;
  icon: IconName;
  eyebrow: string;
  title: string;
  blurb: string;
}[] = [
  {
    id: "overview",
    label: "概览",
    icon: "layers",
    eyebrow: "Overview",
    title: "概览",
    blurb: "权限 · 数据通道 · 本地服务状态",
  },
  {
    id: "dashboard",
    label: "时间",
    icon: "clock",
    eyebrow: "Time",
    title: "时间追踪",
    blurb: "今天你在哪些 App、哪类事情上花了时间",
  },
  {
    id: "search",
    label: "搜索",
    icon: "search",
    eyebrow: "Search",
    title: "全文搜索",
    blurb: "OCR 与语音转写共用一套 FTS 索引",
  },
  {
    id: "activity",
    label: "活动",
    icon: "transcript",
    eyebrow: "Activity",
    title: "时间线",
    blurb: "缩略图 · OCR/转写预览 · 按类型或应用过滤",
  },
  {
    id: "settings",
    label: "设置",
    icon: "settings",
    eyebrow: "Settings",
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

function permStatus(v: string): "done" | "failed" | "idle" {
  const s = v.toLowerCase();
  if (s.includes("granted")) return "done";
  if (s.includes("denied") || s.includes("restricted")) return "failed";
  return "idle";
}

function AudioPreview({ item }: { item: TimelineItem }) {
  const [url, setUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    if (!url) return;
    void audioRef.current?.play().catch(() => {
      // WebKit may require a second explicit click after an async load. The
      // native controls remain visible in that case.
    });
  }, [url]);

  async function loadAudio() {
    if (url) {
      void audioRef.current?.play();
      return;
    }
    setLoading(true);
    setLoadError(null);
    try {
      const next = await api.getEventMediaDataUrl(item.id);
      if (!next) throw new Error("音频文件不可用");
      setUrl(next);
    } catch (error) {
      setLoadError(String(error));
    } finally {
      setLoading(false);
    }
  }

  if (url) {
    return (
      <audio
        ref={audioRef}
        className="timeline-audio"
        controls
        preload="metadata"
        src={url}
      >
        当前系统无法播放这段音频。
      </audio>
    );
  }

  return (
    <div className="audio-load-row">
      <Button
        variant="secondary"
        icon="play"
        disabled={loading}
        onClick={() => void loadAudio()}
      >
        {loading ? "正在载入…" : "播放录音"}
      </Button>
      {loadError && <span className="meta audio-error">{loadError}</span>}
    </div>
  );
}

export default function App() {
  const [tab, setTab] = useState<TabId>("overview");
  const [health, setHealth] = useState<Health | null>(null);
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [cfg, setCfg] = useState<ConfigSummary | null>(null);
  const [observe, setObserve] = useState<ObserveStatus | null>(null);
  const [timeline, setTimeline] = useState<TimelineItem[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [activeImage, setActiveImage] = useState<{ src: string; label: string } | null>(null);
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
  const [browserPairing, setBrowserPairing] = useState<BrowserPairing | null>(null);
  const screenPermissionPending = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const [h, p, c, o, ob, models, asst, browser] = await Promise.all([
        api.getHealth(),
        api.getPermissions(),
        api.getConfigSummary(),
        api.observeStatus(),
        api.getOnboarding(),
        api.checkAsrModelStatus(),
        api.assistantGetConfig(),
        api.getBrowserPairing(),
      ]);
      setHealth(h);
      setPerms(p);
      setCfg(c);
      setObserve(o);
      setOnboarding(ob);
      setAsrModels(models);
      setAssistant(asst);
      setBrowserPairing(browser);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  async function openPrivacySettings(kind: string) {
    try {
      await api.openPrivacySettings(kind);
      setStatusNote("已打开 macOS 隐私与安全设置。授权后 Navi 会自动刷新状态。");
      setError(null);
    } catch (e) {
      setError(`无法打开系统设置：${String(e)}`);
    }
  }

  async function requestAccessibility() {
    setBusy(true);
    try {
      const granted = await api.requestAccessibilityPermission();
      if (!granted) await api.openPrivacySettings("accessibility");
      setAssistant(await api.assistantGetConfig());
      setStatusNote(
        granted
          ? "辅助功能权限已生效。"
          : "请在系统设置中允许 Lumen Navi；返回后状态会自动刷新。",
      );
      setError(null);
    } catch (e) {
      setError(`请求辅助功能权限失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  async function requestScreenRecording() {
    setBusy(true);
    setError(null);
    // Open Settings immediately so a hung/slow host never looks like a no-op.
    setStatusNote(
      "正在打开系统设置并请求 Lumen Cua 屏幕录制…若列表没有 Lumen Cua，请点 + 选择 Finder 中高亮的应用。",
    );
    try {
      await api.openPrivacySettings("screen");
    } catch {
      // request_screen_permission also opens Settings; continue.
    }
    try {
      const granted = await api.requestScreenPermission();
      if (granted) {
        screenPermissionPending.current = false;
        await api.updateSourcesConfig({ screen: cfg?.screen ?? true });
        setStatusNote("屏幕录制权限已生效；实际捕获已验证，采集服务已重载。");
        setError(null);
      } else {
        screenPermissionPending.current = true;
        setError(
          "macOS 未授予 Lumen Cua 屏幕录制。tccutil reset 之后系统经常不再自动弹框——请在设置中手动开启（没有条目时用 + 选 /Applications/Lumen Cua.app），然后回到 Navi 再点一次。",
        );
        setStatusNote(
          "系统设置与 Finder 中的 Lumen Cua 应已打开。开启开关后返回 Navi，再点“请求屏幕录制”完成 Ready 验证。",
        );
      }
      setPerms(await api.getPermissions());
    } catch (e) {
      screenPermissionPending.current = true;
      try {
        await api.openPrivacySettings("screen");
      } catch {
        // still surface the original error below
      }
      setError(`请求屏幕录制权限失败：${String(e)}`);
      setStatusNote(
        "已打开系统设置。请手动开启 Lumen Cua（必要时 + 选择 /Applications/Lumen Cua.app），返回后再点一次。",
      );
    } finally {
      setBusy(false);
    }
  }

  async function requestMicrophone() {
    setBusy(true);
    try {
      const granted = await api.requestMicrophonePermission();
      await api.openPrivacySettings("microphone");
      setStatusNote(
        granted
          ? "麦克风权限已生效；已打开系统设置供你核对。"
          : "麦克风权限尚未允许；Lumen Navi 现在应已出现在系统列表中。",
      );
      setError(null);
    } catch (e) {
      setError(`请求麦克风权限失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  async function configureBrowserPairing(rotate = false) {
    setBusy(true);
    try {
      const pairing = await api.enableBrowserPairing(rotate);
      setBrowserPairing(pairing);
      setStatusNote(
        "浏览通道已启用，本地服务已自动重载。把下方地址和 token 填入扩展即可联动。",
      );
      await refresh();
      setError(null);
    } catch (e) {
      setError(`配置浏览器联动失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), 4000);
    return () => clearInterval(t);
  }, [refresh]);

  useEffect(() => {
    const refreshPendingScreenPermission = async () => {
      if (!screenPermissionPending.current) return;
      try {
        const granted = await api.refreshScreenPermission();
        setPerms(await api.getPermissions());
        if (!granted) return;
        screenPermissionPending.current = false;
        setStatusNote("Lumen Cua 基础屏幕权限已生效；请再次点击“请求屏幕录制”完成实际捕获验证。");
        setError(null);
        await refresh();
      } catch (e) {
        setError(`刷新屏幕录制权限失败：${String(e)}`);
      }
    };
    window.addEventListener("focus", refreshPendingScreenPermission);
    return () => window.removeEventListener("focus", refreshPendingScreenPermission);
  }, [cfg?.screen, refresh]);

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

  useEffect(() => {
    if (!activeImage) return;
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setActiveImage(null);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [activeImage]);

  const nav = NAV.find((n) => n.id === tab)!;

  const updateRuntimeConfig = useCallback(async (
    update: SourcesUpdate,
    note: string,
  ) => {
    setBusy(true);
    try {
      const next = await api.updateSourcesConfig(update);
      setCfg(next);
      setStatusNote(note);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const updateChannel = useCallback(async (
    channel: "screen" | "audio" | "browser",
    enabled: boolean,
    label: string,
  ) => {
    if (channel === "browser" && enabled && !browserPairing?.configured) {
      await configureBrowserPairing(false);
      return;
    }
    await updateRuntimeConfig(
      { [channel]: enabled },
      `${label}通道已${enabled ? "开启" : "关闭"}，本地服务已自动重载。`,
    );
  }, [browserPairing?.configured, updateRuntimeConfig]);

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

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen("tray://toggle-pause", () => {
      void togglePause();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [togglePause]);

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
    <div className="shell">
      {onboarding?.needs_onboarding && (
        <Onboarding initial={onboarding} onDone={() => void refresh()} />
      )}
      <aside className="sidebar">
        <div className="brand">
          <img className="brand-mark" src="/marks/lumen-navi.svg" alt="Lumen Navi" />
          <div>
            <strong>lumen-navi</strong>
            <span>持续上下文</span>
          </div>
        </div>
        <nav className="nav">
          {NAV.map((n) => (
            <NavItem
              key={n.id}
              icon={n.icon}
              label={n.label}
              active={tab === n.id}
              onClick={() => setTab(n.id)}
            />
          ))}
        </nav>
        <div className="side-foot">
          <ThemeToggle
            storageKey="lumen-navi.theme"
            onChange={(t) => {
              try {
                localStorage.setItem("lumen-navi.theme", t);
              } catch {
                /* ignore */
              }
            }}
          />
          <span className="ver">v0.1.0</span>
        </div>
      </aside>

      <main className="workspace">
        <div className="workspace-head">
          <p className="eyebrow">{nav.eyebrow}</p>
          <h1>{nav.title}</h1>
          <p className="sub">{nav.blurb}</p>
        </div>

        {error && (
          <div className="banner">
            <Notice tone="danger">{error}</Notice>
          </div>
        )}
        {statusNote && !error && (
          <div className="banner">
            <Notice tone="success">{statusNote}</Notice>
          </div>
        )}

        <div className="content">
          {tab === "overview" && (
            <div className="stack">
              <div className="row">
                <Button variant="secondary" disabled={busy} onClick={() => void refresh()}>
                  刷新
                </Button>
                <Button variant="secondary" disabled={busy} onClick={() => void togglePause()}>
                  {cfg?.paused ? "恢复采集" : "隐私暂停"}
                </Button>
                <StatusDot
                  status={observe?.running ? "running" : "idle"}
                  label={
                    observe?.running
                      ? "本地服务运行中"
                      : cfg?.screen || cfg?.audio || cfg?.browser
                        ? "本地服务未运行"
                        : "所有通道已关闭"
                  }
                />
                {cfg?.paused && <Pill tone="warn">已暂停</Pill>}
              </div>

              <div className="card mt">
                <h3>数据通道</h3>
                <p className="meta mt">
                  各通道独立控制；修改后会自动重载本地服务，不需要手动开始或停止。
                </p>
                <div className="row mt">
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!cfg?.screen}
                      disabled={busy}
                      onChange={(e) => void updateChannel("screen", e.target.checked, "屏幕")}
                    />
                    屏幕截图
                  </label>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!cfg?.audio}
                      disabled={busy}
                      onChange={(e) => void updateChannel("audio", e.target.checked, "麦克风")}
                    />
                    麦克风音频
                  </label>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!cfg?.browser}
                      disabled={busy}
                      onChange={(e) => void updateChannel("browser", e.target.checked, "浏览器")}
                    />
                    浏览器行为
                  </label>
                </div>
              </div>

              <div className="grid mt">
                <StatCard
                  label="Events"
                  value={health?.stored_events ?? "—"}
                  hint={`schema v${health?.schema_version ?? "—"}`}
                />
                <StatCard
                  label="Search docs"
                  value={health?.ocr_docs ?? "—"}
                  hint="OCR 与转写"
                />
                <StatCard
                  label="Screen"
                  tone={
                    health?.sources.find((s) => s.id === "screen")?.running
                      ? "accent"
                      : "default"
                  }
                  value={
                    health?.sources.find((s) => s.id === "screen")?.enabled
                      ? health.sources.find((s) => s.id === "screen")?.running
                        ? "运行中"
                        : "已启用"
                      : "关闭"
                  }
                />
                <StatCard
                  label="Audio / ASR"
                  tone={cfg?.asr ? "accent" : "default"}
                  value={cfg?.audio ? (cfg.asr ? "转写" : "仅摄入") : "关闭"}
                  hint={`${cfg?.asr_engine ?? "sensevoice"} · ${cfg?.asr_locale ?? ""} · ${cfg?.audio_chunk_ms ?? "—"}ms`}
                />
                <StatCard
                  label="Browser"
                  tone={health?.browser?.last_ingest_at ? "accent" : "default"}
                  value={
                    health?.browser?.last_ingest_at
                      ? "已联动"
                      : health?.browser?.configured
                        ? "等待扩展"
                        : browserPairing?.configured
                          ? "等待本地服务"
                          : "未配对"
                  }
                  hint={
                    health?.browser
                      ? `${health.browser.accepted_events} events · ${fmtTime(health.browser.last_ingest_at)}`
                      : "扩展仍可独立采集"
                  }
                />
              </div>

              <div className="card mt">
                <h3>权限</h3>
                <div className="stack mt">
                  <StatusDot
                    status={permStatus(perms?.screen_recording ?? "")}
                    label={`屏幕录制 · ${perms?.screen_recording ?? "—"}`}
                  />
                  <StatusDot
                    status={
                      perms?.screen_capture_ready === true
                        ? "done"
                        : perms?.direct_capture_status === "unavailable" ||
                            perms?.direct_capture_status === "probe_failed" ||
                            perms?.direct_capture_status === "timed_out"
                          ? "failed"
                          : "idle"
                    }
                    label={`实际捕获 · ${perms?.direct_capture_status ?? "—"}`}
                  />
                  <StatusDot
                    status={permStatus(perms?.microphone ?? "")}
                    label={`麦克风 · ${perms?.microphone ?? "—"}`}
                  />
                  <StatusDot
                    status={permStatus(perms?.accessibility ?? "")}
                    label={`辅助功能 · ${perms?.accessibility ?? "—"}`}
                  />
                </div>
                <p className="meta mt">
                  屏幕录制由共享的 Lumen Cua 请求授权；麦克风与辅助功能仍属于 Lumen Navi。
                  语音识别权限用于本机转写，不做听写注入。
                </p>
                {perms?.direct_capture_error && (
                  <p className="meta mt">{perms.direct_capture_error}</p>
                )}
                <div className="row mt">
                  <Button variant="secondary" disabled={busy} onClick={() => void requestScreenRecording()}>
                    请求屏幕录制
                  </Button>
                  <Button variant="secondary" disabled={busy} onClick={() => void requestMicrophone()}>
                    请求麦克风
                  </Button>
                  <Button variant="secondary" disabled={busy} onClick={() => void requestAccessibility()}>
                    请求辅助功能
                  </Button>
                  <Button variant="secondary" disabled={busy} onClick={() => void openPrivacySettings("accessibility")}>
                    辅助功能设置
                  </Button>
                </div>
              </div>
            </div>
          )}

          {tab === "dashboard" && (
            <DashboardView />
          )}

          {tab === "search" && (
            <div className="stack">
              <div className="row">
                <div style={{ flex: 1, minWidth: 220 }}>
                  <Input
                    icon="search"
                    type="search"
                    placeholder="搜索屏幕文字或转写…"
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void onSearch();
                    }}
                  />
                </div>
                <Button variant="primary" disabled={busy || !query.trim()} onClick={() => void onSearch()}>
                  搜索
                </Button>
                <Button variant="secondary" disabled={busy} onClick={() => void reindex()}>
                  重建索引
                </Button>
              </div>
              <div className="list">
                {hits.length === 0 && (
                  <EmptyState icon="search" title="搜索屏幕文字与转写">
                    输入关键词，检索 OCR 与转写共用的全文索引。
                  </EmptyState>
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
                <Select
                  value={kindFilter}
                  onChange={(e) => setKindFilter(e.target.value)}
                  style={{ width: 150 }}
                >
                  <option value="">全部类型</option>
                  <option value="screenshot">screenshot</option>
                  <option value="audio_chunk">audio_chunk</option>
                  <option value="summary">summary</option>
                  <option value="daemon">daemon</option>
                </Select>
                <div style={{ flex: 1, minWidth: 220 }}>
                  <Input
                    type="text"
                    placeholder="过滤应用 / 标题 / 文本…"
                    value={appFilter}
                    onChange={(e) => setAppFilter(e.target.value)}
                  />
                </div>
                <Button variant="secondary" disabled={busy} onClick={() => void loadTimeline()}>
                  刷新
                </Button>
                <Button
                  variant="primary"
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
                </Button>
              </div>
              {summaryText && (
                <div className="card">
                  <h3>今日摘要</h3>
                  <pre className="meta mt" style={{ whiteSpace: "pre-wrap", margin: 0 }}>
                    {summaryText}
                  </pre>
                </div>
              )}
              <div className="list">
                {timeline.length === 0 && (
                  <EmptyState icon="transcript" title="暂无事件">
                    开启屏幕、麦克风或浏览器通道后，数据会持续写入这里。
                  </EmptyState>
                )}
                {timeline.map((e) => (
                  <div className="list-item timeline-row" key={e.id}>
                    {e.has_image && thumbs[e.id] ? (
                      <button
                        className="thumb-button"
                        type="button"
                        aria-label="放大查看截图"
                        onClick={() =>
                          setActiveImage({
                            src: thumbs[e.id],
                            label: `${e.app_name || "屏幕截图"} · ${fmtTime(e.ts)}`,
                          })
                        }
                      >
                        <img
                          className="thumb"
                          src={thumbs[e.id]}
                          alt={`${e.app_name || "应用"} 的屏幕截图`}
                        />
                        <span className="thumb-zoom">放大</span>
                      </button>
                    ) : e.has_image ? (
                      <div className="thumb placeholder">img</div>
                    ) : e.kind.includes("audio") ? (
                      <div className="thumb placeholder">
                        <Icon name="microphone" size={18} />
                      </div>
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
                      {e.kind.includes("audio_chunk") && e.artifact_bytes != null && (
                        <AudioPreview item={e} />
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
              {activeImage && (
                <div
                  className="image-viewer"
                  role="dialog"
                  aria-modal="true"
                  aria-label={activeImage.label}
                  onClick={(event) => {
                    if (event.target === event.currentTarget) setActiveImage(null);
                  }}
                >
                  <div className="image-viewer-bar">
                    <span>{activeImage.label}</span>
                    <Button variant="secondary" onClick={() => setActiveImage(null)}>
                      关闭
                    </Button>
                  </div>
                  <img src={activeImage.src} alt={activeImage.label} />
                </div>
              )}
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
                <h3>Browser extension</h3>
                <div className="stack mt">
                  <StatusDot
                    status={health?.browser?.last_ingest_at ? "done" : "idle"}
                    label={
                      health?.browser?.last_ingest_at
                        ? `已联动 · 最后同步 ${fmtTime(health.browser.last_ingest_at)}`
                        : health?.browser?.configured
                          ? "Navi 已就绪，等待扩展发送数据"
                          : browserPairing?.configured
                            ? "配对已配置；开启浏览器通道后提供同步 API"
                            : "扩展尚未与 Navi 配对"
                    }
                  />
                  <p className="meta">
                    扩展始终先写自己的 IndexedDB。只有填入下方地址与 token 后，Navi
                    才能看到同步事件；未配对时 App 无法读取扩展内部数据。
                  </p>
                  {browserPairing?.configured && (
                    <>
                      <label className="field">
                        <span className="meta">本地地址</span>
                        <input className="input mono" readOnly value={browserPairing.endpoint} />
                      </label>
                      <label className="field">
                        <span className="meta">配对 token</span>
                        <input className="input mono" type="password" readOnly value={browserPairing.token} />
                      </label>
                      <div className="meta">
                        已接收 {health?.browser?.accepted_events ?? 0} · 重复 {health?.browser?.duplicate_events ?? 0} · 拒绝批次 {health?.browser?.rejected_batches ?? 0}
                      </div>
                    </>
                  )}
                  <div className="row">
                    <Button variant="primary" disabled={busy} onClick={() => void configureBrowserPairing(false)}>
                      {browserPairing?.configured ? "重新应用配置" : "启用并生成 token"}
                    </Button>
                    {browserPairing?.configured && (
                      <>
                        <Button
                          variant="secondary"
                          disabled={busy}
                          onClick={() => {
                            void navigator.clipboard
                              .writeText(`${browserPairing.endpoint}\n${browserPairing.token}`)
                              .then(() => setStatusNote("地址和 token 已复制。"))
                              .catch((err) => setError(`复制失败：${String(err)}`));
                          }}
                        >
                          复制连接信息
                        </Button>
                        <Button variant="secondary" disabled={busy} onClick={() => void configureBrowserPairing(true)}>
                          轮换 token
                        </Button>
                      </>
                    )}
                  </div>
                  <p className="meta">
                    在扩展弹窗展开“连接 Lumen Navi（可选）”，分别粘贴地址和 token，保存后会立即尝试同步。
                  </p>
                </div>
              </div>
              <div className="card">
                <h3>Sources / engines</h3>
                <div className="stack mt">
                  {(
                    [
                      ["screen", "屏幕截图", cfg?.screen],
                      ["audio", "麦克风", cfg?.audio],
                      ["browser", "浏览器行为", cfg?.browser],
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
                          if (key === "screen" || key === "audio" || key === "browser") {
                            void updateChannel(key, checked, label);
                          } else {
                            void updateRuntimeConfig(
                              { [key]: checked },
                              `${label}已${checked ? "开启" : "关闭"}，本地服务已自动重载。`,
                            );
                          }
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
                          void updateRuntimeConfig(
                            { asr_engine },
                            `ASR 引擎 → ${asr_engine}，本地服务已自动重载。`,
                          );
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
                            onClick={() => void requestAccessibility()}
                          >
                            请求权限
                          </button>
                          <button
                            className="btn"
                            onClick={() => void openPrivacySettings("accessibility")}
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
                  启动应用时运行本地服务（仅采集已开启的通道）
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
                <p className="meta">菜单栏托盘可打开 Navi、切换隐私暂停与退出。</p>
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
