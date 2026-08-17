import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";
import { CHAT_PROVIDERS, getProvider } from "./llm/catalog";
import { Onboarding } from "./Onboarding";
import { DashboardView } from "./views/Dashboard";
import { AIView } from "./views/AIView";
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
  AudioDevices,
  AudioRecordingTest,
  AssistantConfig,
  AssistantUpdate,
  BrowserPairing,
  ConfigSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  Permissions,
  PlatformInfo,
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
  const [assistantKey, setAssistantKey] = useState("");
  const [assistantSaveState, setAssistantSaveState] = useState<
    "idle" | "saving" | "saved" | "error"
  >("idle");
  const [assistantSaveMessage, setAssistantSaveMessage] = useState<string | null>(null);
  const [llmTestState, setLlmTestState] = useState<
    "idle" | "testing" | "success" | "error"
  >("idle");
  const [llmTestMessage, setLlmTestMessage] = useState<string | null>(null);
  const [modelListBusy, setModelListBusy] = useState(false);
  const [modelListMessage, setModelListMessage] = useState<string | null>(null);
  const [audioDevices, setAudioDevices] = useState<AudioDevices | null>(null);
  const [audioDevicesBusy, setAudioDevicesBusy] = useState(false);
  const [audioDevicesError, setAudioDevicesError] = useState<string | null>(null);
  const [audioTestState, setAudioTestState] = useState<
    "idle" | "testing" | "success" | "error"
  >("idle");
  const [audioTestResult, setAudioTestResult] = useState<AudioRecordingTest | null>(null);
  const [writeAudioTestEvent, setWriteAudioTestEvent] = useState(false);
  const [browserPairing, setBrowserPairing] = useState<BrowserPairing | null>(null);
  const assistantSaveRef = useRef<Promise<void>>(Promise.resolve());
  const assistantLastSaveRef = useRef<Promise<void>>(Promise.resolve());

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

  const refreshAudioDevices = useCallback(async () => {
    setAudioDevicesBusy(true);
    setAudioDevicesError(null);
    try {
      setAudioDevices(await api.listAudioDevices());
    } catch (e) {
      setAudioDevicesError(`读取录音设备失败：${String(e)}`);
    } finally {
      setAudioDevicesBusy(false);
    }
  }, []);

  async function runAudioTest() {
    setAudioTestState("testing");
    setAudioTestResult(null);
    try {
      const result = await api.recordAudioTest(3_000, writeAudioTestEvent);
      setAudioTestResult(result);
      setAudioTestState(result.success && !result.error ? "success" : "error");
      if (result.error) {
        setError(`录音自测失败：${result.error}`);
      } else {
        setError(null);
        setStatusNote(
          result.event_written
            ? "录音自测完成，测试音频已写入时间线。"
            : "录音自测完成，未写入时间线。",
        );
      }
    } catch (e) {
      setAudioTestState("error");
      setAudioTestResult(null);
      setError(`录音自测失败：${String(e)}`);
    }
  }

  useEffect(() => {
    if (tab === "settings" && !audioDevices && !audioDevicesBusy) {
      void refreshAudioDevices();
    }
  }, [audioDevices, audioDevicesBusy, refreshAudioDevices, tab]);

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
  const selectedAudioDeviceMissing = Boolean(
    cfg?.audio_device &&
      audioDevices &&
      !audioDevices.devices.some((device) => device.name === cfg.audio_device),
  );

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
    const operation = assistantSaveRef.current.then(async () => {
      setAssistantSaveState("saving");
      setAssistantSaveMessage("正在保存配置…");
      try {
        const a = await api.assistantUpdateConfig(update);
        setAssistant(a);
        setAssistantSaveState("saved");
        setAssistantSaveMessage(`已保存 · ${new Date().toLocaleTimeString()}`);
        return;
      } catch (e) {
        const message = String(e);
        setAssistantSaveState("error");
        setAssistantSaveMessage(`保存失败：${message}`);
        throw e;
      }
    });
    assistantLastSaveRef.current = operation;
    assistantSaveRef.current = operation.catch(() => {});
    return operation;
  }, []);

  async function testLlm() {
    setLlmTestState("testing");
    setLlmTestMessage("正在测试当前配置…");
    try {
      await assistantLastSaveRef.current;
      const result = await api.llmTest();
      setLlmTestState("success");
      setLlmTestMessage(result || "连接成功");
      setError(null);
    } catch (e) {
      setLlmTestState("error");
      setLlmTestMessage(`连接失败：${String(e)}`);
    }
  }

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
        .setBadgeCount(0)
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
                  tone={audioSource?.last_error ? "danger" : audioSource?.running ? "accent" : "default"}
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
                    audioSource?.last_error ??
                    `${cfg?.asr_engine ?? "sensevoice"} · ${cfg?.asr_locale ?? ""} · ${cfg?.audio_chunk_ms ?? "—"}ms`
                  }
                />
                {health?.observe && (
                  <>
                    <StatCard
                      label="已写入"
                      value={health.observe.persisted}
                      hint="本进程成功落库"
                    />
                    <StatCard
                      label="写入失败"
                      tone={health.observe.persist_failed > 0 ? "danger" : "default"}
                      value={health.observe.persist_failed}
                      hint="SQLite / 磁盘"
                    />
                    <StatCard
                      label="门挡下"
                      tone={health.observe.skipped_gate > 0 ? "warn" : "default"}
                      value={health.observe.skipped_gate}
                      hint="暂停 / 闭眼 / 锁屏 / 名单"
                    />
                    <StatCard
                      label="队列丢弃"
                      tone={health.observe.dropped_backpressure > 0 ? "danger" : "default"}
                      value={health.observe.dropped_backpressure}
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
                        : `屏幕录制 · ${perms?.screen_recording ?? "—"}`
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
                    label={`实际捕获 · ${perms?.direct_capture_status ?? "—"}`}
                  />
                  <StatusDot
                    status={permStatus(perms?.microphone ?? "")}
                    label={`麦克风 · ${perms?.microphone ?? "—"}`}
                  />
                  {platform?.accessibility_gate !== false && (
                    <StatusDot
                      status={permStatus(perms?.accessibility ?? "")}
                      label={`辅助功能 · ${perms?.accessibility ?? "—"}`}
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
                  role={audioSource?.last_error ? "alert" : "status"}
                >
                  <div className="row" style={{ justifyContent: "space-between" }}>
                    <strong>麦克风采集</strong>
                    <span
                      className={`pill ${
                        audioSource?.last_error
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
                    {audioSource?.last_error ??
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
                    <Button
                      variant="primary"
                      disabled={busy}
                      onClick={() => void restartAudio()}
                    >
                      启动采集
                    </Button>
                  </div>
                </div>
                <div className="row mt">
                  <Button variant="secondary" disabled={busy} onClick={() => void requestScreenRecording()}>
                    请求屏幕录制
                  </Button>
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
                <h3>麦克风录音验证</h3>
                <p className="meta mt">
                  选择实际录音设备并做一次限时自测。自测只在你点击按钮后打开麦克风，不会自动修改系统权限。
                </p>
                <div className="stack mt">
                  <label className="field">
                    <span className="meta">录音设备</span>
                    <select
                      className="input"
                      value={cfg?.audio_device ?? ""}
                      disabled={audioDevicesBusy || busy}
                      onChange={(event) => {
                        void updateRuntimeConfig(
                          { audio_device: event.target.value },
                          event.target.value
                            ? `录音设备已保存为“${event.target.value}”，本地服务已自动重载。`
                            : "录音设备已恢复为系统默认设备，本地服务已自动重载。",
                        );
                      }}
                    >
                      <option value="">系统默认设备</option>
                      {(audioDevices?.devices ?? []).map((device) => (
                        <option key={device.name} value={device.name}>
                          {device.name}{device.is_default ? "（系统默认）" : ""}
                        </option>
                      ))}
                    </select>
                  </label>
                  {selectedAudioDeviceMissing && (
                    <Notice tone="warn">
                      当前保存的设备“{cfg?.audio_device}”不可用。请选择列表中的设备，或恢复为系统默认设备。
                    </Notice>
                  )}
                  {audioDevicesError && (
                    <Notice tone="danger">
                      {audioDevicesError} 请确认麦克风权限后重试。
                    </Notice>
                  )}
                  <div className="row">
                    <Button
                      variant="secondary"
                      disabled={audioDevicesBusy}
                      onClick={() => void refreshAudioDevices()}
                    >
                      {audioDevicesBusy ? "正在读取设备…" : "刷新设备列表"}
                    </Button>
                    <StatusDot
                      status={audioDevices ? "done" : "idle"}
                      label={
                        audioDevices
                          ? `${audioDevices.devices.length} 个录音设备`
                          : "尚未读取设备列表"
                      }
                    />
                  </div>
                  <div className="row">
                    <Button
                      variant="primary"
                      disabled={audioTestState === "testing" || busy}
                      onClick={() => void runAudioTest()}
                    >
                      {audioTestState === "testing" ? "正在录音 3 秒…" : "开始录音自测"}
                    </Button>
                    <label className="check">
                      <input
                        type="checkbox"
                        checked={writeAudioTestEvent}
                        disabled={audioTestState === "testing" || busy}
                        onChange={(event) => setWriteAudioTestEvent(event.target.checked)}
                      />
                      写入一条测试事件
                    </label>
                  </div>
                  <p className="meta">
                    默认不写入时间线；勾选后会写入一个带 test 标记的 audio_chunk.v1 事件，便于验证落库和播放链路。
                  </p>
                  {audioTestResult && (
                    <Notice
                      tone={
                        audioTestResult.success && !audioTestResult.error
                          ? "success"
                          : "danger"
                      }
                      title={
                        audioTestResult.success && !audioTestResult.error
                          ? "录音自测完成"
                          : "录音自测未通过"
                      }
                    >
                      {audioTestResult.error ? (
                        audioTestResult.error
                      ) : (
                        <span>
                          设备：{audioTestResult.device ?? "—"} · 帧数：
                          {audioTestResult.frames.toLocaleString()} · 时长：
                          {audioTestResult.captured_duration_ms} ms · RMS：
                          {audioTestResult.rms.toFixed(4)} · 峰值：
                          {audioTestResult.peak.toFixed(4)} ·{" "}
                          {audioTestResult.signal_detected ? "检测到有效信号" : "信号偏低"}
                          <br />
                          audio_chunk.v1：
                          {audioTestResult.event_written ? "已写入" : "未写入（诊断模式）"}
                        </span>
                      )}
                    </Notice>
                  )}
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
                <h3>行为采集（键鼠，Roast 数据源）</h3>
                <div className="stack mt">
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={!!cfg?.input_enabled}
                      onChange={(e) => {
                        const checked = e.target.checked;
                        setBusy(true);
                        void api
                          .updateSourcesConfig({ input_enabled: checked })
                          .then((c) => {
                            setCfg(c);
                            setStatusNote(
                              checked
                                ? "键鼠计数已开启（需在 系统设置 → 隐私与安全性 → 输入监控 中允许 lumen-daemon），本地服务已重载。"
                                : "键鼠计数已关闭，本地服务已重载。",
                            );
                          })
                          .catch((err) => setError(String(err)))
                          .finally(() => setBusy(false));
                      }}
                    />
                    键盘鼠标计数（按行为类别统计：删除/Tab/回车/点击…，不记录按键内容）
                  </label>
                  <label className="check">
                    <input
                      type="checkbox"
                      disabled={!cfg?.input_enabled}
                      checked={!!cfg?.input_interactions}
                      onChange={(e) => {
                        const checked = e.target.checked;
                        setBusy(true);
                        void api
                          .updateSourcesConfig({ input_interactions: checked })
                          .then((c) => {
                            setCfg(c);
                            setStatusNote(
                              checked
                                ? "交互事件已开启：记录点击/快捷键/提交的时刻与应用（不记录文本内容）。"
                                : "交互事件已关闭。",
                            );
                          })
                          .catch((err) => setError(String(err)))
                          .finally(() => setBusy(false));
                      }}
                    />
                    交互事件（更精确：每次点击/回车提交/快捷键的时刻 + 所在应用；文本内容不记录）
                  </label>
                  <p className="meta">
                    Roast / 行为分析靠这个区分「用户主动操作」和「程序自动变化」（自动截屏、安装器切窗、挂机）。
                    没开的话分析只能基于前台停留时长，无法归因。首次开启需要在系统设置授予「输入监控」权限给 lumen-daemon。
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
                      {platform?.system_speech_asr !== false && (
                        <option value="speech">macOS Speech</option>
                      )}
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
                          placeholder={
                            platform?.os === "windows"
                              ? "%LOCALAPPDATA%\\Lumen\\models\\sensevoice"
                              : "~/Library/Application Support/Lumen/models/sensevoice"
                          }
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
                  {platform?.system_speech_asr === false ? (
                    <p className="meta">
                      本系统没有内置语音识别引擎，本地模型不可用时不会回退 Speech；
                      请配置云端 ASR 引擎作为兜底。
                    </p>
                  ) : (
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
                                  ? "本地模型不可用时回退系统 Speech。"
                                  : "已关闭 Speech 回退。",
                              );
                            })
                            .catch((err) => setError(String(err)))
                            .finally(() => setBusy(false));
                        }}
                      />
                      本地引擎不可用时回退 Speech
                    </label>
                  )}
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
                  {assistant?.selection_supported === false && (
                    <p className="meta">
                      本系统暂不支持读取其他应用中的选中文字（macOS 走辅助功能 API，
                      Windows 的 UI Automation 实现尚未完成），划词弹窗不会触发。
                    </p>
                  )}
                  {assistant?.selection_supported !== false &&
                    assistant?.popup_enabled &&
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
                    无 AX 应用（钉钉文档 / 终端）用复制键兜底取词（读取后立即恢复剪贴板）
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
                  <p className="meta">
                    LLM 提供商 / 模型 / 密钥在下方「LLM 配置」卡片统一设置。选中文字仅在你点击「翻译 / 提问」时发送，不会被采集或存储。
                  </p>
                </div>
              </div>
              <div className="card">
                <h3>LLM 配置（全局）</h3>
                <p className="meta">
                  全应用共用一份配置：划词助手、Roast 我的一天、AI Chat。
                </p>
                <div className="stack mt">
                  <div
                    className={`onboard-status ${
                      assistantSaveState === "saved" ? "ok" : ""
                    }`}
                    role={assistantSaveState === "error" ? "alert" : "status"}
                  >
                    <div className="row" style={{ justifyContent: "space-between" }}>
                      <strong>配置状态</strong>
                      <span
                        className={`pill ${
                          assistantSaveState === "error"
                            ? "err"
                            : assistantSaveState === "saved"
                              ? "ok"
                              : assistantSaveState === "saving"
                                ? "warn"
                                : "warn"
                        }`}
                      >
                        {assistantSaveState === "saving"
                          ? "保存中…"
                          : assistantSaveState === "saved"
                            ? "已保存"
                            : assistantSaveState === "error"
                              ? "保存失败"
                              : "待修改"}
                      </span>
                    </div>
                    <p className="meta mt" style={{ marginBottom: 0 }}>
                      {assistantSaveMessage ??
                        (assistant?.api_key_set
                          ? "当前配置已加载，API key 已保存（不会显示明文）。"
                          : "填写配置后会自动保存，保存成功后会在这里确认。")}
                    </p>
                  </div>
                  {(llmTestState !== "idle" || llmTestMessage) && (
                    <div
                      className={`onboard-status ${
                        llmTestState === "success" ? "ok" : ""
                      }`}
                      role={llmTestState === "error" ? "alert" : "status"}
                    >
                      <div className="row" style={{ justifyContent: "space-between" }}>
                        <strong>连接测试</strong>
                        <span
                          className={`pill ${
                            llmTestState === "success"
                              ? "ok"
                              : llmTestState === "error"
                                ? "err"
                                : "warn"
                          }`}
                        >
                          {llmTestState === "testing"
                            ? "测试中…"
                            : llmTestState === "success"
                              ? "成功"
                              : "失败"}
                        </span>
                      </div>
                      <p className="meta mt" style={{ marginBottom: 0 }}>
                        {llmTestMessage}
                      </p>
                      {llmTestState === "error" && (
                        <Button
                          variant="secondary"
                          className="mt"
                          disabled={busy}
                          onClick={() => void testLlm()}
                        >
                          重试测试
                        </Button>
                      )}
                    </div>
                  )}
                  <label className="field">
                    <span className="meta">LLM Provider</span>
                    <select
                      className="input"
                      value={assistant?.provider_id ?? "custom"}
                      onChange={(e) => {
                        const provider_id = e.target.value;
                        setAssistant((prev) =>
                          prev ? { ...prev, provider_id } : prev,
                        );
                        const preset = getProvider(provider_id);
                        if (preset) {
                          const patch: AssistantUpdate = { provider_id };
                          if (preset.defaultModel) {
                            patch.model = preset.defaultModel;
                            setAssistant((prev) =>
                              prev ? { ...prev, model: preset.defaultModel } : prev,
                            );
                          }
                          void updateAssistant(patch);
                        } else {
                          void updateAssistant({ provider_id });
                        }
                      }}
                    >
                      <option value="custom">自定义（手动填写 base URL）</option>
                      {CHAT_PROVIDERS.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.label}
                        </option>
                      ))}
                    </select>
                  </label>
                  {(assistant?.provider_id ?? "custom") !== "custom" &&
                    getProvider(assistant?.provider_id ?? "")?.overseasBaseUrl && (
                      <label className="field">
                        <span className="meta">Endpoint 区域</span>
                        <select
                          className="input"
                          value={assistant?.region ?? "cn"}
                          onChange={(e) => {
                            const region = e.target.value;
                            setAssistant((prev) =>
                              prev ? { ...prev, region } : prev,
                            );
                            void updateAssistant({ region });
                          }}
                        >
                          <option value="cn">国内端点</option>
                          <option value="global">海外端点</option>
                        </select>
                      </label>
                    )}
                  {(assistant?.provider_id ?? "custom") === "custom" && (
                    <label className="field">
                      <span className="meta">LLM base URL（OpenAI 兼容 …/v1）</span>
                      <input
                        className="input mono"
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
                  )}
                  <label className="field">
                    <span className="meta">模型</span>
                    <div style={{ display: "flex", gap: 6 }}>
                      {(assistant?.provider_id ?? "custom") !== "custom" &&
                      (getProvider(assistant?.provider_id ?? "")?.models.length ?? 0) > 0 ? (
                        <select
                          className="input"
                          style={{ flex: 1 }}
                          value={assistant?.model ?? ""}
                          onChange={(e) => {
                            const model = e.target.value;
                            setAssistant((prev) =>
                              prev ? { ...prev, model } : prev,
                            );
                            void updateAssistant({ model });
                          }}
                        >
                          {(assistant?.model &&
                            !getProvider(assistant?.provider_id ?? "")?.models.includes(assistant.model)) ? (
                            <option value={assistant.model}>{assistant.model}（自定义）</option>
                          ) : null}
                          {getProvider(assistant?.provider_id ?? "")?.models.map((m) => (
                            <option key={m} value={m}>{m}</option>
                          ))}
                        </select>
                      ) : (
                        <input
                          className="input"
                          style={{ flex: 1 }}
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
                      )}
                      <Button
                        variant="secondary"
                        disabled={busy || modelListBusy}
                        onClick={() => {
                          setModelListBusy(true);
                          setModelListMessage("正在获取模型列表…");
                          void api
                            .llmListModels()
                            .then((models) => {
                              const pid = assistant?.provider_id ?? "custom";
                              if (pid === "custom") {
                                setModelListMessage(
                                  "自定义提供商不会自动发现模型，请直接填写模型名。",
                                );
                                return;
                              }
                              if (models.length === 0) {
                                setModelListMessage(
                                  "提供商没有返回可用模型，请检查 API key、Endpoint，或直接填写模型名。",
                                );
                                return;
                              }
                              // Merge fetched models into the preset's list for this session.
                              const existing = getProvider(pid);
                              if (existing) {
                                existing.models = Array.from(
                                  new Set([...existing.models, ...models]),
                                );
                              }
                              setModelListMessage(`已获取 ${models.length} 个模型。`);
                            })
                            .catch((e) => {
                              setModelListMessage(`获取失败：${String(e)}`);
                            })
                            .finally(() => setModelListBusy(false));
                        }}
                      >
                        {modelListBusy ? "获取中…" : "刷新模型"}
                      </Button>
                    </div>
                  </label>
                  {modelListMessage && (
                    <p className="meta" role="status">
                      {modelListMessage}
                    </p>
                  )}
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
                          void updateAssistant({ api_key: k }).then(
                            () => setAssistantKey(""),
                            () => {},
                          );
                        }
                      }}
                    />
                  </label>
                  <div className="row">
                    {assistant?.api_key_set && (
                      <button
                        className="btn"
                        disabled={busy || assistantSaveState === "saving"}
                        onClick={() => {
                          void updateAssistant({ api_key: "" }).then(
                            () => setAssistantKey(""),
                            () => {},
                          );
                        }}
                      >
                        清除 API key
                      </button>
                    )}
                    <Button
                      variant="secondary"
                      disabled={busy || llmTestState === "testing"}
                      onClick={() => void testLlm()}
                    >
                      {llmTestState === "testing" ? "测试中…" : "测试连接"}
                    </Button>
                  </div>
                  <p className="meta">
                    写入 <span className="mono">navi.toml</span> 的{" "}
                    <span className="mono">assistant</span> 段；也可用环境变量{" "}
                    <span className="mono">LUMEN_NAVI_LLM_API_KEY</span>。
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
