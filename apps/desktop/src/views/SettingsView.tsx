import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { api } from "../api";
import { CHAT_PROVIDERS, getProvider } from "../llm/catalog";
import { Button, NavItem, Notice, StatusDot } from "../design";
import type { IconName } from "../design";
import type {
  ActDriverInfo,
  AsrModelStatus,
  SkillDto,
  AssistantConfig,
  AssistantUpdate,
  AudioDevices,
  AudioRecordingTest,
  BrowserPairing,
  ConfigSummary,
  Health,
  OnboardingState,
  PlatformInfo,
  SourcesUpdate,
} from "../types";

// ── sections ─────────────────────────────────────────────────────────────

type SettingsSectionId =
  | "general"
  | "capture"
  | "voice"
  | "ai"
  | "shortcuts"
  | "skills";

const SETTINGS_SECTIONS: {
  id: SettingsSectionId;
  label: string;
  icon: IconName;
}[] = [
  { id: "general", label: "通用", icon: "settings" },
  { id: "capture", label: "采集", icon: "layers" },
  { id: "voice", label: "语音与转写", icon: "microphone" },
  { id: "ai", label: "AI 与划词", icon: "translate" },
  { id: "shortcuts", label: "快捷键", icon: "star" },
  { id: "skills", label: "技能库", icon: "play" },
];

const SECTION_STORAGE_KEY = "lumen-navi.settings.section";

// ── helpers ──────────────────────────────────────────────────────────────

function fmtTime(iso?: string | null): string {
  if (!iso) return "—";
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

/** KeyboardEvent.code → accelerator key token (null = not hotkey-able). */
const CODE_KEY_MAP: Record<string, string> = {
  Space: "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Tab: "Tab",
  Enter: "Enter",
  Delete: "Delete",
  Insert: "Insert",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Semicolon: ";",
  Quote: "'",
  Backslash: "\\",
  Comma: ",",
  Period: ".",
  Slash: "/",
};

function keyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^F\d{1,2}$/.test(code)) return code;
  return CODE_KEY_MAP[code] ?? null;
}

/** Build a tauri accelerator ("Alt+Space") from a capture-time keydown. */
function acceleratorFromEvent(e: KeyboardEvent): string | null {
  const key = keyFromCode(e.code);
  if (!key) return null;
  const mods: string[] = [];
  if (e.metaKey) mods.push("Command");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.ctrlKey) mods.push("Control");
  return [...mods, key].join("+");
}

/** "Alt+Space" → "⌥空格" (macOS) / "Alt+空格" (Windows), for display only. */
function prettyShortcut(accelerator: string, os?: string | null): string {
  const mac = os !== "windows";
  const parts = accelerator.split("+").filter(Boolean);
  if (parts.length === 0) return "未设置";
  const key = parts[parts.length - 1];
  const modOrder = mac
    ? ["super", "cmd", "command", "meta", "win", "cmdorctrl", "commandorcontrol", "alt", "option", "shift", "ctrl", "control"]
    : ["ctrl", "control", "cmdorctrl", "commandorcontrol", "alt", "option", "shift", "super", "cmd", "command", "meta", "win"];
  const sym = (m: string): string => {
    switch (m) {
      case "alt":
      case "option":
        return mac ? "⌥" : "Alt";
      case "ctrl":
      case "control":
        return mac ? "⌃" : "Ctrl";
      case "shift":
        return mac ? "⇧" : "Shift";
      case "super":
      case "cmd":
      case "command":
      case "meta":
      case "win":
        return mac ? "⌘" : "Win";
      case "cmdorctrl":
      case "commandorcontrol":
        return mac ? "⌘" : "Ctrl";
      default:
        return m;
    }
  };
  const sortedMods = parts
    .slice(0, -1)
    .map((m) => m.toLowerCase())
    .sort((a, b) => modOrder.indexOf(a) - modOrder.indexOf(b))
    .map(sym);
  const keyLabel = key === "Space" ? "空格" : key;
  if (mac) return [...sortedMods, keyLabel].join("");
  return [...sortedMods, keyLabel].join("+");
}

// ── props ────────────────────────────────────────────────────────────────

export interface SettingsViewProps {
  cfg: ConfigSummary | null;
  setCfg: Dispatch<SetStateAction<ConfigSummary | null>>;
  assistant: AssistantConfig | null;
  setAssistant: Dispatch<SetStateAction<AssistantConfig | null>>;
  browserPairing: BrowserPairing | null;
  platform: PlatformInfo | null;
  onboarding: OnboardingState | null;
  health: Health | null;
  asrModels: AsrModelStatus | null;
  setAsrModels: Dispatch<SetStateAction<AsrModelStatus | null>>;
  busy: boolean;
  setBusy: Dispatch<SetStateAction<boolean>>;
  setError: Dispatch<SetStateAction<string | null>>;
  setStatusNote: Dispatch<SetStateAction<string | null>>;
  refresh: () => Promise<void>;
  updateRuntimeConfig: (update: SourcesUpdate, note: string) => Promise<void>;
  updateChannel: (
    channel: "screen" | "audio" | "browser",
    enabled: boolean,
    label: string,
  ) => Promise<void>;
  configureBrowserPairing: (rotate?: boolean) => Promise<void>;
  requestAccessibility: () => Promise<void>;
  openPrivacySettings: (kind: string) => Promise<void>;
}

export function SettingsView(props: SettingsViewProps) {
  const {
    cfg,
    setCfg,
    assistant,
    setAssistant,
    browserPairing,
    platform,
    onboarding,
    health,
    asrModels,
    setAsrModels,
    busy,
    setBusy,
    setError,
    setStatusNote,
    refresh,
    updateRuntimeConfig,
    updateChannel,
    configureBrowserPairing,
    requestAccessibility,
    openPrivacySettings,
  } = props;

  const [section, setSection] = useState<SettingsSectionId>(() => {
    try {
      const saved = localStorage.getItem(SECTION_STORAGE_KEY);
      if (saved && SETTINGS_SECTIONS.some((s) => s.id === saved)) {
        return saved as SettingsSectionId;
      }
    } catch {
      /* ignore */
    }
    return "general";
  });

  const onSectionChange = useCallback((id: string) => {
    setSection(id as SettingsSectionId);
    try {
      localStorage.setItem(SECTION_STORAGE_KEY, id);
    } catch {
      /* ignore */
    }
  }, []);

  // ── settings-local state (moved out of App) ────────────────────────────

  const [audioDevices, setAudioDevices] = useState<AudioDevices | null>(null);
  const [audioDevicesBusy, setAudioDevicesBusy] = useState(false);
  const [audioDevicesError, setAudioDevicesError] = useState<string | null>(null);
  const [audioTestState, setAudioTestState] = useState<
    "idle" | "testing" | "success" | "error"
  >("idle");
  const [audioTestResult, setAudioTestResult] = useState<AudioRecordingTest | null>(null);
  const [writeAudioTestEvent, setWriteAudioTestEvent] = useState(false);
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
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const assistantSaveRef = useRef<Promise<void>>(Promise.resolve());
  const assistantLastSaveRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    void api
      .getAutostart()
      .then(setAutostart)
      .catch(() => {});
  }, []);

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

  useEffect(() => {
    if (!audioDevices && !audioDevicesBusy) {
      void refreshAudioDevices();
    }
  }, [audioDevices, audioDevicesBusy, refreshAudioDevices]);

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

  const selectedAudioDeviceMissing = Boolean(
    cfg?.audio_device &&
      audioDevices &&
      !audioDevices.devices.some((device) => device.name === cfg.audio_device),
  );

  const updateAssistant = useCallback(
    async (update: AssistantUpdate) => {
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
    },
    [setAssistant],
  );

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

  // ── sections ───────────────────────────────────────────────────────────

  return (
    <div className="settings-layout">
      <nav className="settings-nav">
        {SETTINGS_SECTIONS.map((s) => (
          <NavItem
            key={s.id}
            icon={s.icon}
            label={s.label}
            active={section === s.id}
            onClick={() => onSectionChange(s.id)}
          />
        ))}
      </nav>

      <div className="stack settings-pane">
        {section === "general" && (
          <>
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
              <h3>Shell 与启动</h3>
              <label className="check mt">
                <input
                  type="checkbox"
                  checked={autostart}
                  disabled={autostartBusy}
                  onChange={(e) => {
                    const checked = e.target.checked;
                    setAutostartBusy(true);
                    void api
                      .setAutostart(checked)
                      .then(() => {
                        setAutostart(checked);
                        setStatusNote(
                          checked
                            ? "已开启开机自启动（系统登录时自动运行 Lumen Navi）。"
                            : "已关闭开机自启动。",
                        );
                      })
                      .catch((err) => setError(String(err)))
                      .finally(() => setAutostartBusy(false));
                  }}
                />
                开机自动启动（系统登录时自动运行 Lumen Navi）
              </label>
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
          </>
        )}

        {section === "capture" && (
          <>
            <div className="card">
              <h3>采集通道</h3>
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
                <p className="meta">
                  开关写入 <span className="mono">navi.toml</span>
                  。采集进程需重启后读取新配置。
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
              <h3>浏览器扩展联动</h3>
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
          </>
        )}

        {section === "voice" && (
          <>
            <div className="card">
              <h3>语音识别（ASR）</h3>
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
                <div className="meta mt">
                  api={cfg?.api_bind} · chunk={cfg?.audio_chunk_ms}ms · engine=
                  {cfg?.asr_engine ?? "—"} · locale={cfg?.asr_locale}
                </div>
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
          </>
        )}

        {section === "ai" && (
          <>
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
                  LLM 提供商 / 模型 / 密钥在上方「LLM 配置」卡片统一设置。选中文字仅在你点击「翻译 / 提问」时发送，不会被采集或存储。
                </p>
              </div>
            </div>
          </>
        )}

        {section === "shortcuts" && (
          <ShortcutCard
            cfg={cfg}
            setCfg={setCfg}
            platform={platform}
            busy={busy}
            setStatusNote={setStatusNote}
            setError={setError}
          />
        )}

        {section === "skills" && <SkillLibraryCard />}
      </div>
    </div>
  );
}

// ── shortcuts card ───────────────────────────────────────────────────────

function ShortcutCard({
  cfg,
  setCfg,
  platform,
  busy,
  setStatusNote,
  setError,
}: {
  cfg: ConfigSummary | null;
  setCfg: Dispatch<SetStateAction<ConfigSummary | null>>;
  platform: PlatformInfo | null;
  busy: boolean;
  setStatusNote: Dispatch<SetStateAction<string | null>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
  const [listening, setListening] = useState(false);
  const [saving, setSaving] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);

  const configured = cfg?.composer_shortcut ?? "";
  const active = cfg?.composer_shortcut_active ?? "";
  const registrationError = cfg?.composer_shortcut_error ?? null;

  const save = useCallback(
    async (accelerator: string) => {
      setSaving(true);
      setCaptureError(null);
      try {
        const next = await api.setComposerShortcut(accelerator);
        setCfg(next);
        setStatusNote(
          accelerator
            ? `快捷对话热键已保存为 ${prettyShortcut(accelerator, platform?.os)}。`
            : "快捷对话热键已停用。",
        );
      } catch (e) {
        // Backend keeps the previous shortcut live; surface the reason.
        setCaptureError(String(e));
        setError(null);
      } finally {
        setSaving(false);
      }
    },
    [platform?.os, setCfg, setStatusNote],
  );

  useEffect(() => {
    if (!listening) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setListening(false);
        setHint(null);
        return;
      }
      if (["Meta", "Alt", "Control", "Shift"].includes(event.key)) {
        const held: string[] = [];
        if (event.metaKey) held.push("⌘");
        if (event.altKey) held.push("⌥");
        if (event.shiftKey) held.push("⇧");
        if (event.ctrlKey) held.push("⌃");
        setHint(`已按住 ${held.join(" ")}，再按一个普通键完成组合…`);
        return;
      }
      setHint(null);
      const accelerator = acceleratorFromEvent(event);
      if (!accelerator) {
        setCaptureError("这个按键不能作为全局热键，请换一个（字母 / 数字 / 空格 / F1-F12 / 方向键…）。");
        return;
      }
      if (!event.metaKey && !event.altKey && !event.ctrlKey) {
        setCaptureError("请至少搭配一个修饰键（⌘/⌥/Ctrl），否则会拦截正常打字。");
        return;
      }
      setListening(false);
      void save(accelerator);
    };
    const onBlur = () => setListening(false);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("blur", onBlur);
    };
  }, [listening, save]);

  const disabled = !configured;
  const activeOk = !disabled && active === configured && !registrationError;

  return (
    <div className="card">
      <h3>快捷对话（Quick Chat）</h3>
      <p className="meta mt">
        随时呼出的迷你对话窗：输入问题或指令，回答可以直接注入当前应用。热键在此修改。
      </p>
      <div className="stack mt">
        <div className="row" style={{ alignItems: "center" }}>
          <span className="meta">呼出热键</span>
          <kbd className="shortcut-kbd">
            {listening
              ? "按下新组合键…"
              : disabled
                ? "已停用"
                : prettyShortcut(configured, platform?.os)}
          </kbd>
          <Button
            variant={listening ? "secondary" : "primary"}
            disabled={saving || busy}
            onClick={() => {
              setCaptureError(null);
              setHint(null);
              setListening(!listening);
            }}
          >
            {listening ? "取消（Esc）" : saving ? "保存中…" : "录制新热键"}
          </Button>
          {!disabled && (
            <Button
              variant="ghost"
              disabled={saving || busy}
              onClick={() => void save("")}
            >
              停用热键
            </Button>
          )}
        </div>
        {hint && listening && <p className="meta">{hint}</p>}
        <StatusDot
          status={activeOk ? "done" : disabled ? "idle" : "failed"}
          label={
            activeOk
              ? "热键已生效"
              : disabled
                ? "热键已停用，可用菜单栏托盘 →「打开快捷对话」呼出"
                : "热键未生效"
          }
        />
        {captureError && (
          <Notice tone="danger" title="热键未能修改">
            {captureError}
          </Notice>
        )}
        {!captureError && registrationError && (
          <Notice tone="warn" title="当前热键未生效">
            {registrationError} 可在此换一个组合，或暂时用菜单栏托盘 →「打开快捷对话」呼出。
          </Notice>
        )}
        <p className="meta">
          提示：避开系统和常用工具占用的组合——⌘Space 是 Spotlight，⌥Space
          常被 Raycast/Alfred 或输入法切换占用。修改立即生效，无需重启。
        </p>
      </div>
    </div>
  );
}

// ── skills ───────────────────────────────────────────────────────────────

/** Settings → 技能库: library of CUA-replayable workflows (D2). */
function SkillLibraryCard() {
  const [skills, setSkills] = useState<SkillDto[] | null>(null);
  const [driver, setDriver] = useState<ActDriverInfo | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setSkills(await api.skillsList());
    } catch {
      setSkills([]);
    }
    try {
      setDriver(await api.actDriverStatus());
    } catch {
      setDriver(null);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function replay(sk: SkillDto) {
    if (
      !window.confirm(
        `按 ${sk.steps.length} 步回放「${sk.name}」？\n默认不抢焦点；失败会告诉你原因。`,
      )
    )
      return;
    const texts: Array<string | null> = sk.steps.map(() => null);
    for (let i = 0; i < sk.steps.length; i++) {
      if (sk.steps[i].action !== "type") continue;
      const input = window.prompt(
        `第 ${i + 1} 步需要键入文本（不会记录）：\n${sk.steps[i].note ?? sk.steps[i].target ?? ""}`,
        "",
      );
      if (input === null) return;
      texts[i] = input;
    }
    setBusy(true);
    try {
      window.alert(await api.skillReplay(sk.name, texts));
    } catch (e) {
      window.alert(String(e));
    }
    setBusy(false);
    void load();
  }

  return (
    <div className="card">
      <h3>技能库</h3>
      <p className="meta mt">
        从 15 分钟卡提取的可回放工作流。启用后：触发场景命中时菜单栏会出现「试试」建议；也可手动回放。回放走 Lumen Cua，默认不抢焦点。
      </p>
      <p className="meta mt">
        Act 引擎（MIT cua-driver，嵌在 Lumen Cua 里）:{" "}
        {driver == null
          ? "未探测"
          : driver.running
            ? `运行中${driver.version ? ` · ${driver.version}` : ""}`
            : driver.present
              ? "已捆绑，未启动"
              : "未捆绑（HID 回放仍可用）"}
        {driver?.error ? ` · ${driver.error}` : ""}
      </p>
      {driver?.present && !driver.running && (
        <div className="row mt">
          <Button
            variant="secondary"
            size="sm"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void api
                .actDriverEnsure()
                .then((info) => setDriver(info))
                .catch((e) => window.alert(String(e)))
                .finally(() => setBusy(false));
            }}
          >
            启动 Act 引擎
          </Button>
        </div>
      )}
      {driver?.present && (
        <div className="stack mt">
          <p className="meta">
            编码 agent 用 MCP 名 <code>computer-use</code>
            。默认后台点击，不抢焦点、不挪真光标。先启动引擎，再写入 Codex / Claude 配置。
          </p>
          {driver.mcp_snippet && (
            <pre className="meta" style={{ whiteSpace: "pre-wrap", fontSize: "11px" }}>
              {driver.mcp_snippet}
            </pre>
          )}
          <div className="row" style={{ gap: 6, flexWrap: "wrap" }}>
            <Button
              variant="secondary"
              size="sm"
              disabled={busy || !driver.mcp_snippet}
              onClick={() => {
                if (!driver.mcp_snippet) return;
                void navigator.clipboard.writeText(driver.mcp_snippet);
              }}
            >
              复制 MCP 片段
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void api
                  .actInstallMcp("codex")
                  .then((msg) => window.alert(msg))
                  .catch((e) => window.alert(String(e)))
                  .finally(() => setBusy(false));
              }}
            >
              写入 Codex
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void api
                  .actInstallMcp("claude")
                  .then((msg) => window.alert(msg))
                  .catch((e) => window.alert(String(e)))
                  .finally(() => setBusy(false));
              }}
            >
              写入 Claude Code
            </Button>
          </div>
        </div>
      )}
      <div className="stack mt skill-list-scroll">
        {skills === null && <p className="meta">加载中…</p>}
        {skills !== null && skills.length === 0 && (
          <p className="meta">还没有技能——持续使用后，AI 会从你的键鼠轨迹里提取可复用的工作流。</p>
        )}
        {skills?.map((sk) => (
          <div key={sk.name} className="list-item" style={{ padding: "10px 12px" }}>
            <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
              <strong style={{ fontSize: "var(--text-sm)" }}>{sk.name}</strong>
              <span className="meta">
                {sk.apps.slice(0, 3).join(" / ")}
                {sk.use_count > 0 ? ` · 用过 ${sk.use_count} 次` : ""}
              </span>
            </div>
            {sk.trigger && <p className="meta" style={{ margin: "4px 0 0" }}>{sk.trigger}</p>}
            <div className="row mt" style={{ gap: 6 }}>
              <Button
                variant="primary"
                size="sm"
                disabled={busy || sk.steps.length < 2}
                onClick={() => void replay(sk)}
              >
                回放 {sk.steps.length} 步
              </Button>
              <Button
                variant="secondary"
                size="sm"
                disabled={busy}
                onClick={() => void api.skillsSetEnabled(sk.name, !sk.enabled).then(load)}
              >
                {sk.enabled ? "禁用" : "启用"}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => {
                  if (window.confirm(`删除技能「${sk.name}」？`)) {
                    void api.skillsDelete(sk.name).then(load);
                  }
                }}
              >
                删除
              </Button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
