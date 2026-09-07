import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";
import { Onboarding } from "./Onboarding";
import { DashboardView } from "./views/Dashboard";
import { AIView } from "./views/AIView";
import { SettingsView } from "./views/SettingsView";
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
  BrowserPairing,
  ConfigSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  OverviewRange,
  Permissions,
  PlatformInfo,
  SearchHit,
  SourcesUpdate,
  TabId,
  TimelineItem,
} from "./types";

type OverviewRangeKey = "today" | "week" | "last7" | "month" | "total";

const OVERVIEW_RANGE_LABELS: Record<OverviewRangeKey, string> = {
  today: "今日",
  week: "本周",
  last7: "最近 7 天",
  month: "本月",
  total: "全部累计",
};

function localDayString(date = new Date()): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function shiftLocalDay(day: string, delta: number): string {
  const date = new Date(`${day}T00:00:00`);
  date.setDate(date.getDate() + delta);
  return localDayString(date);
}

function overviewRangeBounds(range: OverviewRangeKey): { from: string; to: string } {
  const to = localDayString();
  if (range === "today") return { from: to, to };
  if (range === "week") {
    const date = new Date(`${to}T00:00:00`);
    const day = date.getDay();
    return { from: shiftLocalDay(to, day === 0 ? -6 : 1 - day), to };
  }
  if (range === "last7") return { from: shiftLocalDay(to, -6), to };
  if (range === "month") return { from: `${to.slice(0, 8)}01`, to };
  return { from: "1970-01-01", to };
}

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
    id: "ai",
    label: "AI",
    icon: "star",
    eyebrow: "AI",
    title: "AI 助手",
    blurb: "Roast 我的一天 · AI Chat · LLM 状态",
  },
  {
    id: "settings",
    label: "设置",
    icon: "settings",
    eyebrow: "Settings",
    title: "设置",
    blurb: "通用 · 采集 · 语音 · AI · 快捷键 · 技能库",
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

/** Thousands separator for counts: 1234567 → "1,234,567". */
function fmtNum(n: number | null | undefined): string {
  if (n === null || n === undefined) return "0";
  return n.toLocaleString("en-US");
}

function permStatus(v: string): "done" | "failed" | "idle" {
  const s = v.toLowerCase();
  if (s.includes("granted")) return "done";
  if (s.includes("denied") || s.includes("restricted")) return "failed";
  return "idle";
}

function permissionLabel(v?: string | null): string {
  switch ((v ?? "").toLowerCase()) {
    case "granted":
      return "已允许";
    case "denied":
      return "已拒绝";
    case "restricted":
      return "受限";
    case "notdetermined":
      return "未确认";
    default:
      return v || "—";
  }
}

function captureStatusLabel(v?: string | null): string {
  switch ((v ?? "").toLowerCase()) {
    case "ready":
      return "已验证";
    case "not_checked":
      return "待验证";
    case "blocked_by_screen_recording":
      return "等待屏幕权限";
    case "unavailable":
      return "不可用";
    case "probe_failed":
      return "验证失败";
    case "timed_out":
      return "验证超时";
    case "native":
      return "系统默认";
    default:
      return v || "—";
  }
}

function titleMissingLabel(reason: string): string {
  switch (reason) {
    case "no_frontmost":
      return "无前台应用";
    case "no_window":
      return "无窗口";
    case "empty_title":
      return "空标题";
    default:
      return reason;
  }
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
  const [overviewRange, setOverviewRange] = useState<OverviewRangeKey>("today");
  const [overviewStats, setOverviewStats] = useState<OverviewRange | null>(null);
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  const [cfg, setCfg] = useState<ConfigSummary | null>(null);
  const [observe, setObserve] = useState<ObserveStatus | null>(null);
  const [timeline, setTimeline] = useState<TimelineItem[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [activeImage, setActiveImage] = useState<{ src: string; label: string } | null>(null);
  const [kindFilter, setKindFilter] = useState("screenshot");
  const [appFilter, setAppFilter] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [healthAlert, setHealthAlert] = useState<{ reason: string } | null>(null);
  const [buildInfo, setBuildInfo] = useState<{ version: string; sha: string } | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingState | null>(null);
  const [summaryText, setSummaryText] = useState<string | null>(null);
  const [asrModels, setAsrModels] = useState<AsrModelStatus | null>(null);
  const [assistant, setAssistant] = useState<AssistantConfig | null>(null);
  const [browserPairing, setBrowserPairing] = useState<BrowserPairing | null>(null);
  const screenVerificationStarted = useRef(false);

  useEffect(() => {
    void api.getBuildInfo().then(setBuildInfo).catch(() => {});
  }, []);
  const screenPermissionPending = useRef(false);
  // Thumbnail loading strategy: load every thumb in the current timeline page
  // eagerly (they're ~200-400KB JPEG data URLs, 60 items ≈ 15-24MB — fine),
  // and keep an IntersectionObserver around so that when the list later grows
  // (pagination / infinite scroll), off-screen thumbs still lazy-load.
  // Background: blobs are always on disk; the old slice(0,12) eager load made
  // it look like images were missing because newest-first + 30s auto-refresh
  // pushes items 13-60 below the fold before the user ever scrolls.
  const thumbsRef = useRef<Record<string, string>>({});
  // In-flight promise per id, so concurrent callers (eager load + click +
  // observer) share one fetch instead of the later ones seeing null.
  const thumbLoadingRef = useRef<Map<string, Promise<string | null>>>(new Map());
  const thumbObserverRef = useRef<IntersectionObserver | null>(null);

  const ensureThumb = useCallback(async (id: string): Promise<string | null> => {
    const cached = thumbsRef.current[id];
    if (cached) return cached;
    const inflight = thumbLoadingRef.current.get(id);
    if (inflight) return inflight;
    const p = (async () => {
      try {
        const url = await api.getEventImageDataUrl(id);
        if (url) {
          thumbsRef.current = { ...thumbsRef.current, [id]: url };
          setThumbs((prev) => ({ ...prev, [id]: url }));
          return url;
        }
        return null;
      } catch (e) {
        console.warn("thumb load failed", id.slice(0, 8), e);
        return null;
      } finally {
        thumbLoadingRef.current.delete(id);
      }
    })();
    thumbLoadingRef.current.set(id, p);
    return p;
  }, []);

  /** Attach lazy-load observer to a row that still needs a thumb. */
  const bindLazyThumb = useCallback(
    (el: HTMLElement | null, id: string) => {
      if (!el) return;
      if (thumbsRef.current[id]) return;
      if (!thumbObserverRef.current) {
        thumbObserverRef.current = new IntersectionObserver(
          (entries) => {
            for (const entry of entries) {
              if (!entry.isIntersecting) continue;
              const eid = (entry.target as HTMLElement).dataset.eventId;
              if (!eid) continue;
              thumbObserverRef.current?.unobserve(entry.target);
              void ensureThumb(eid);
            }
          },
          // Prefetch slightly before the row enters the viewport.
          { root: null, rootMargin: "240px 0px", threshold: 0.01 },
        );
      }
      el.dataset.eventId = id;
      thumbObserverRef.current.observe(el);
    },
    [ensureThumb],
  );

  const refresh = useCallback(async () => {
    try {
      const [h, p, plat, c, o, ob, models, asst, browser] = await Promise.all([
        api.getHealth(),
        api.getPermissions(),
        api.getPlatformInfo(),
        api.getConfigSummary(),
        api.observeStatus(),
        api.getOnboarding(),
        api.checkAsrModelStatus(),
        api.assistantGetConfig(),
        api.getBrowserPairing(),
      ]);
      setHealth(h);
      setPerms(p);
      setPlatform(plat);
      setCfg(c);
      setObserve(o);
      setOnboarding(ob);
      setAsrModels(models);
      setAssistant(asst);
      setBrowserPairing(browser);
      setError(null);
      if (
        plat.os === "macos" &&
        c.screen &&
        p.screen_recording.toLowerCase() === "granted" &&
        p.direct_capture_status === "not_checked" &&
        !screenVerificationStarted.current
      ) {
        screenVerificationStarted.current = true;
        void verifyScreenCapture();
      }
    } catch (e) {
      setError(String(e));
    }
  }, []);

  async function openPrivacySettings(kind: string) {
    try {
      await api.openPrivacySettings(kind);
      setStatusNote(
        platform?.os === "windows"
          ? "已打开 Windows 隐私设置。授权后 Navi 会自动刷新状态。"
          : "已打开 macOS 隐私与安全设置。授权后 Navi 会自动刷新状态。",
      );
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
    setError(null);
    try {
      await api.openPrivacySettings("microphone");
      const nextPerms = await api.getPermissions();
      setPerms(nextPerms);
      if (nextPerms.microphone.toLowerCase() !== "granted") {
        setStatusNote(
          "已打开系统设置。请在隐私与安全性 → 麦克风中打开 Lumen Navi，返回后点击“检查采集”。应用不会替你修改权限。",
        );
        return;
      }
      const probe = await api.checkAudioReadiness();
      if (!probe.ready) {
        setError(`麦克风权限已允许，但采集无法启动：${probe.error ?? "未知错误"}`);
        setStatusNote("请检查系统输入设备，修复后点击“检查采集”重试。");
        return;
      }
      setStatusNote("麦克风权限和采集设备均已验证。点击“启动采集”开启持续录音。");
    } catch (e) {
      setError(`检查麦克风失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  async function verifyScreenCapture() {
    setBusy(true);
    setError(null);
    setStatusNote("正在执行一次实际屏幕捕获验证…");
    try {
      const ready = await api.refreshScreenPermission();
      const nextPerms = await api.getPermissions();
      setPerms(nextPerms);
      if (!ready) {
        setError(
          `屏幕录制权限已允许，但实际捕获验证未通过：${nextPerms.direct_capture_error ?? "请检查 Lumen Cua 和系统设置。"}`,
        );
        return;
      }
      setStatusNote("实际屏幕捕获验证成功。");
    } catch (e) {
      setError(`实际屏幕捕获验证失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  const restartAudio = useCallback(async () => {
    setBusy(true);
    setError(null);
    setStatusNote("正在检查麦克风设备…");
    try {
      const probe = await api.checkAudioReadiness();
      if (!probe.ready) {
        setError(
          `麦克风还不能启动：${probe.error ?? "未知错误"}。请先完成系统权限和输入设备设置。`,
        );
        return;
      }
      const next = await api.updateSourcesConfig({ audio: true });
      setCfg(next);
      await refresh();
      setStatusNote("麦克风采集已启动，正在等待实时状态确认。");
    } catch (e) {
      setError(`启动麦克风采集失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }, [refresh]);

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

  const loadOverviewStats = useCallback(async () => {
    try {
      const { from, to } = overviewRangeBounds(overviewRange);
      setOverviewStats(await api.overviewRange(from, to));
    } catch (e) {
      setError(String(e));
    }
  }, [overviewRange]);

  useEffect(() => {
    void loadOverviewStats();
    const t = setInterval(() => void loadOverviewStats(), 30_000);
    return () => clearInterval(t);
  }, [loadOverviewStats]);

  useEffect(() => {
    const refreshPendingScreenPermission = async () => {
      if (!screenPermissionPending.current) return;
      try {
        const granted = await api.refreshScreenPermission();
        setPerms(await api.getPermissions());
        if (!granted) return;
        screenPermissionPending.current = false;
        setStatusNote("Lumen Cua 屏幕权限已刷新，实际捕获验证成功。");
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
        limit: 200,
        kindContains: kindFilter || undefined,
        appContains: appFilter || undefined,
      });
      setTimeline(items);
      setError(null);
      // First screen = the whole current page. Eager-load every thumb in this
      // batch (concurrency-limited); each ensureThumb dedupes via thumbsRef so
      // the 30s auto-refresh re-running this is cheap. Below-page items (when
      // pagination lands) still lazy-load via bindLazyThumb's observer.
      const need = items.filter((i) => i.has_image);
      const concurrency = 6;
      for (let i = 0; i < need.length; i += concurrency) {
        const batch = need.slice(i, i + concurrency);
        await Promise.all(batch.map((item) => ensureThumb(item.id)));
      }
    } catch (e) {
      setError(String(e));
    }
  }, [kindFilter, appFilter, ensureThumb]);

  useEffect(() => {
    if (tab === "activity") {
      void loadTimeline();
      // Auto-refresh every 30s while on the activity tab.
      const t = setInterval(() => void loadTimeline(), 30_000);
      return () => clearInterval(t);
    }
    return () => {
      thumbObserverRef.current?.disconnect();
      thumbObserverRef.current = null;
    };
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
  const audioSource = health?.sources.find((source) => source.id === "audio");
  const audioSignalFiltered = Boolean(
    audioSource?.last_error?.includes("静音检测过滤"),
  );
  const audioPersistedEvents = health?.stored_audio_events ?? 0;
  const audioStatusMessage = audioSignalFiltered
    ? `当前未检测到有效语音 · 历史累计 ${audioPersistedEvents} 条`
    : audioSource?.last_error;
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

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen("tray://toggle-pause", () => {
      void togglePause();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [togglePause]);

  // Daemon crash alert: the supervisor (Rust) emits `daemon://exited` when the
  // daemon process dies unexpectedly. Before this, a SIGSEGV was invisible —
  // the UI just silently showed "本地服务未运行" with no explanation. The
  // supervisor also auto-restarts; this banner just tells the user what
  // happened. Cleared on next successful refresh.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<number>("daemon://exited", (event) => {
      const crashes = event.payload;
      setError(
        crashes > 5
          ? `本地服务反复崩溃（已尝试 ${crashes} 次），已停止自动重启。请检查日志或重启 App。`
          : `本地服务意外退出（第 ${crashes} 次），正在自动重启…`
      );
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  // Capture health alert: emitted by the health monitor when the capture
  // pipeline has been stagnant for >60s and self-healing failed.
  useEffect(() => {
    let unlistenAlert: (() => void) | undefined;
    let unlistenRecover: (() => void) | undefined;
    // Clear any stale badge on mount.
    getCurrentWindow()
      .setBadgeCount(undefined)
      .catch(() => {});
    void listen<{ reason: string }>("health://alert", (event) => {
      setHealthAlert(event.payload);
      // Set dock badge so the user notices even if the window is hidden.
      getCurrentWindow()
        .setBadgeCount(1)
        .catch(() => {});
    }).then((fn) => {
      unlistenAlert = fn;
    });
    void listen("health://recovered", () => {
      setHealthAlert(null);
      getCurrentWindow()
        .setBadgeCount(undefined)
        .catch(() => {});
    }).then((fn) => {
      unlistenRecover = fn;
    });
    return () => {
      unlistenAlert?.();
      unlistenRecover?.();
    };
  }, []);

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
          <span className="ver" title="Build version">v{buildInfo?.version ?? "0.1.0"} ({buildInfo?.sha ?? "dev"})</span>
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
        {healthAlert && (
          <div className="banner">
            <Notice tone="warn">
              ⚠️ 采集可能已停滞：{healthAlert.reason}。系统已尝试自动恢复。如果持续出现，请检查系统设置中的权限或重启 App。
            </Notice>
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

              <div
                className="row mt"
                style={{
                  gap: 0,
                  alignSelf: "flex-start",
                  borderRadius: "var(--radius-input)",
                  overflow: "hidden",
                }}
                aria-label="概览统计范围"
              >
                {(Object.keys(OVERVIEW_RANGE_LABELS) as OverviewRangeKey[]).map((range, index) => (
                  <button
                    key={range}
                    onClick={() => setOverviewRange(range)}
                    style={{
                      background:
                        overviewRange === range ? "var(--surface)" : "transparent",
                      border: "1px solid var(--border)",
                      borderLeft: index === 0 ? "1px solid var(--border)" : "none",
                      fontSize: "var(--text-xs)",
                      fontWeight: overviewRange === range ? 600 : 400,
                      color:
                        overviewRange === range
                          ? "var(--text)"
                          : "var(--text-tertiary)",
                      padding: "5px 14px",
                      cursor: "pointer",
                    }}
                  >
                    {OVERVIEW_RANGE_LABELS[range]}
                  </button>
                ))}
              </div>

              <div className="grid mt">
                <StatCard
                  label="Events"
                  value={overviewStats ? fmtNum(overviewStats.stored_events) : "—"}
                  hint={`${OVERVIEW_RANGE_LABELS[overviewRange]} · schema v${health?.schema_version ?? "—"}`}
                />
                <StatCard
                  label="Search docs"
                  value={overviewStats ? fmtNum(overviewStats.ocr_docs) : "—"}
                  hint={`${OVERVIEW_RANGE_LABELS[overviewRange]} · OCR 与转写`}
                />
                <StatCard
                  label="Audio events"
                  value={overviewStats?.audio_events ?? "—"}
                  hint={`${OVERVIEW_RANGE_LABELS[overviewRange]} · audio_chunk.v1`}
                />
                <StatCard
                  label="Screen"
                  tone={
                    health?.sources.find((s) => s.id === "screen")?.running
                      ? "accent"
                      : health?.sources.find((s) => s.id === "screen")?.enabled
                        ? "success"
                        : "danger"
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
                  tone={
                    !audioSource?.enabled
                      ? "danger"
                      : audioSignalFiltered
                        ? "warn"
                        : audioSource?.last_error
                          ? "danger"
                          : audioSource?.running
                            ? "accent"
                            : "default"
                  }
                  value={
                    !audioSource?.enabled
                      ? "关闭"
                      : audioSource.running
                        ? cfg?.asr
                          ? "运行中 · 转写"
                          : "运行中 · 仅摄入"
                        : "已启用但未运行"
                  }
                  hint={
                    audioStatusMessage ??
                    `${cfg?.asr_engine ?? "sensevoice"} · ${cfg?.asr_locale ?? ""} · ${cfg?.audio_chunk_ms ?? "—"}ms`
                  }
                />
                {health?.observe && (
                  <>
                    <StatCard
                      label="已写入"
                      value={fmtNum(health.observe.persisted)}
                      hint="本进程成功落库"
                    />
                    <StatCard
                      label="写入失败"
                      tone={health.observe.persist_failed > 0 ? "danger" : "default"}
                      value={fmtNum(health.observe.persist_failed)}
                      hint="SQLite / 磁盘"
                    />
                    <StatCard
                      label="门挡下"
                      tone={health.observe.skipped_gate > 0 ? "warn" : "default"}
                      value={fmtNum(health.observe.skipped_gate)}
                      hint="暂停 / 闭眼 / 锁屏 / 名单"
                    />
                    <StatCard
                      label="队列丢弃"
                      tone={health.observe.dropped_backpressure > 0 ? "danger" : "default"}
                      value={fmtNum(health.observe.dropped_backpressure)}
                      hint="截图背压"
                    />
                  </>
                )}
                {(health?.browser?.last_ingest_at ||
                  health?.browser?.configured ||
                  browserPairing?.configured) && (
                  <StatCard
                    label="Browser"
                    tone={health?.browser?.last_ingest_at ? "accent" : "default"}
                    value={
                      health?.browser?.last_ingest_at
                        ? "已联动"
                        : health?.browser?.configured
                          ? "等待扩展"
                          : "等待本地服务"
                    }
                    hint={
                      health?.browser?.last_ingest_at
                        ? `${health.browser.accepted_events} events · ${fmtTime(health.browser.last_ingest_at)}`
                        : undefined
                    }
                  />
                )}
              </div>

              <div className="card mt">
                <h3>权限</h3>
                <div className="stack mt">
                  <StatusDot
                    status={permStatus(perms?.screen_recording ?? "")}
                    label={
                      platform && !platform.screen_permission_gate
                        ? "屏幕截取 · 无需授权"
                        : `屏幕录制 · ${permissionLabel(perms?.screen_recording)}`
                    }
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
                    label={`实际捕获 · ${captureStatusLabel(perms?.direct_capture_status)}`}
                  />
                  <StatusDot
                    status={permStatus(perms?.microphone ?? "")}
                    label={`麦克风 · ${permissionLabel(perms?.microphone)}`}
                  />
                  {platform?.accessibility_gate !== false && (
                    <StatusDot
                      status={permStatus(perms?.accessibility ?? "")}
                      label={`辅助功能 · ${permissionLabel(perms?.accessibility)}`}
                    />
                  )}
                </div>
                <p className="meta mt">
                  {platform?.os === "windows"
                    ? "桌面程序截屏无需授权；首次录音需在「设置 → 隐私和安全性 → 麦克风」允许桌面应用。听写产品见 Lumen ASR。"
                    : "屏幕录制由共享的 Lumen Cua 请求授权；麦克风与辅助功能仍属于 Lumen Navi。语音识别权限用于本机转写，不做听写注入。"}
                </p>
                {perms?.direct_capture_error && (
                  <p className="meta mt">{perms.direct_capture_error}</p>
                )}
                <div
                  className={`onboard-status mt ${audioSource?.running ? "ok" : ""}`}
                  role={audioSource?.last_error && !audioSignalFiltered ? "alert" : "status"}
                >
                  <div className="row" style={{ justifyContent: "space-between" }}>
                    <strong>麦克风采集</strong>
                    <span
                      className={`pill ${
                        audioSource?.last_error && !audioSignalFiltered
                          ? "err"
                          : audioSource?.running
                            ? "ok"
                            : "warn"
                      }`}
                    >
                      {!audioSource?.enabled
                        ? "已关闭"
                        : audioSource?.running
                          ? "运行中"
                          : "未运行"}
                    </span>
                  </div>
                  <p className="meta mt">
                    {audioSignalFiltered
                      ? audioStatusMessage
                      : audioSource?.last_error ??
                        (audioSource?.running
                          ? "已成功打开输入设备，音频会按配置写入本地事件。"
                          : "权限可能已允许，但采集设备还没有成功启动。")}
                  </p>
                  <div className="row mt">
                    <Button
                      variant="secondary"
                      disabled={busy}
                      onClick={() => void requestMicrophone()}
                    >
                      检查采集
                    </Button>
                    {audioSource?.running ? (
                      <span className="meta">采集已运行，无需启动</span>
                    ) : (
                      <Button
                        variant="primary"
                        disabled={busy}
                        onClick={() => void restartAudio()}
                      >
                        启动采集
                      </Button>
                    )}
                  </div>
                </div>
                <div className="row mt">
                  <Button variant="secondary" disabled={busy} onClick={() => void requestScreenRecording()}>
                    请求屏幕录制
                  </Button>
                  {platform?.os === "macos" && (
                    <Button variant="secondary" disabled={busy} onClick={() => void verifyScreenCapture()}>
                      验证实际捕获
                    </Button>
                  )}
                  <Button variant="secondary" disabled={busy} onClick={() => void requestMicrophone()}>
                    打开麦克风设置
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
                      {h.confidence > 0 && (
                        <span>conf {h.confidence.toFixed(2)}</span>
                      )}
                    </div>
                    <div className="meta">{h.text_preview}</div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {tab === "ai" && (
            <AIView assistant={assistant} onOpenSettings={() => setTab("settings")} />
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
                {timeline.filter((e) => e.kind !== "activity.focus.v1").map((e) => (
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
                      <button
                        ref={(el) => bindLazyThumb(el, e.id)}
                        className="thumb placeholder"
                        type="button"
                        aria-label="加载并查看截图"
                        title="点击加载截图"
                        onClick={() => {
                          void (async () => {
                            const url = await ensureThumb(e.id);
                            if (!url) {
                              setError(`截图文件无法读取（event ${e.id.slice(0, 8)}）`);
                              return;
                            }
                            setActiveImage({
                              src: url,
                              label: `${e.app_name || "屏幕截图"} · ${fmtTime(e.ts)}`,
                            });
                          })();
                        }}
                      >
                        img
                      </button>
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
                          {e.window_title
                            ? ` · ${e.window_title}`
                            : e.window_title_missing_reason
                              ? ` · ${titleMissingLabel(e.window_title_missing_reason)}`
                              : ""}
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
            <SettingsView
              cfg={cfg}
              setCfg={setCfg}
              assistant={assistant}
              setAssistant={setAssistant}
              browserPairing={browserPairing}
              platform={platform}
              onboarding={onboarding}
              health={health}
              asrModels={asrModels}
              setAsrModels={setAsrModels}
              busy={busy}
              setBusy={setBusy}
              setError={setError}
              setStatusNote={setStatusNote}
              refresh={refresh}
              updateRuntimeConfig={updateRuntimeConfig}
              updateChannel={updateChannel}
              configureBrowserPairing={configureBrowserPairing}
              requestAccessibility={requestAccessibility}
              openPrivacySettings={openPrivacySettings}
            />
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
