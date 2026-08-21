import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import {
  Button,
  Icon,
  Notice,
  Pill,
  ProgressBar,
  Select,
  Spinner,
  StatusDot,
} from "./design";
import type {
  AsrDownloadProgress,
  AsrModelStatus,
  AudioReadiness,
  OnboardingState,
  Permissions,
  PlatformInfo,
} from "./types";

function stepsFor(platform: PlatformInfo | null) {
  const windows = platform?.os === "windows";
  return [
    {
      title: "欢迎使用 Lumen Navi",
      body: windows
        ? "本地持续记录屏幕与声音，转成可搜索的上下文。数据默认留在本机 %LOCALAPPDATA%\\LumenNavi。"
        : "本地持续记录屏幕与声音，转成可搜索的上下文。数据默认留在本机 Application Support。",
      icon: "layers" as const,
    },
    {
      title: "屏幕截取",
      body: windows
        ? "Windows 桌面程序截屏无需单独授权，直接进入下一步即可。若截图为空，请确认没有全屏独占的应用挡住采集。"
        : "Observe 需要 Screen Recording 权限才能截取屏幕。点击下方按钮打开系统设置并授权本应用。",
      icon: "folder" as const,
      kind: "screen" as const,
    },
    {
      title: "麦克风与划词",
      body: windows
        ? "音频 chunk 需要麦克风权限：在「设置 → 隐私和安全性 → 麦克风」允许桌面应用访问。持续转写走本机 SenseVoice。"
        : "音频 chunk 需要麦克风权限；划词助手需要辅助功能（读取选中文字）。持续转写默认走本机 SenseVoice。",
      icon: "microphone" as const,
      kind: "microphone" as const,
    },
    {
      title: "本地 ASR 模型",
      body: "默认 SenseVoice。模型装在 Lumen 共享目录（navi / asr 等共用，只下一次）。",
      icon: "transcript" as const,
    },
    {
      title: "准备就绪",
      body: "可以随时在概览页分别开关屏幕、麦克风与浏览器通道。配置会自动生效。",
      icon: "check" as const,
    },
  ];
}

export function Onboarding({
  initial,
  onDone,
}: {
  initial: OnboardingState;
  onDone: () => void;
}) {
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  const STEPS = stepsFor(platform);
  const [step, setStep] = useState(Math.min(initial.step, 4));
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [audioProbe, setAudioProbe] = useState<AudioReadiness | null>(null);
  const [launch, setLaunch] = useState(initial.launch_observe);
  const [busy, setBusy] = useState(false);
  const [guide, setGuide] = useState<string | null>(null); // warn-level guidance
  const [error, setError] = useState<string | null>(null); // danger-level real errors
  const [doneFlash, setDoneFlash] = useState(false);

  const [asr, setAsr] = useState<AsrModelStatus | null>(null);
  const [engineChoice, setEngineChoice] = useState("sensevoice");
  const [customPath, setCustomPath] = useState("");
  const [dlMsg, setDlMsg] = useState("");
  const [dlPct, setDlPct] = useState<number | null>(null);
  const screenPermissionPending = useRef(false);
  const microphonePermissionPending = useRef(false);

  const refreshAsr = useCallback(async () => {
    try {
      const s = await api.checkAsrModelStatus();
      setAsr(s);
      if (s.active_engine) setEngineChoice(s.active_engine);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void api.getPlatformInfo().then(setPlatform).catch(() => {});
  }, []);

  // Poll permissions while on a permission step (2s) — live feedback while
  // the user is inside System Settings, replacing the old focus-event trick.
  useEffect(() => {
    if (step !== 1 && step !== 2) return;
    let alive = true;
    const tick = async () => {
      try {
        if (screenPermissionPending.current && step === 1) {
          const granted = await api.refreshScreenPermission();
          if (granted) {
            screenPermissionPending.current = false;
            setGuide(null);
          }
        }
        const nextPerms = await api.getPermissions();
        if (!alive) return;
        setPerms(nextPerms);
        if (step === 2 && nextPerms.microphone.toLowerCase() === "granted") {
          const probe = await api.checkAudioReadiness();
          if (!alive) return;
          setAudioProbe(probe);
          if (probe.ready) {
            microphonePermissionPending.current = false;
            setGuide(null);
          }
        }
      } catch {
        /* transient poll failure — next tick retries */
      }
    };
    void tick();
    const t = setInterval(() => void tick(), 2_000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [step]);

  useEffect(() => {
    if (step === 3) void refreshAsr();
  }, [step, refreshAsr]);

  useEffect(() => {
    let un: (() => void) | undefined;
    void listen<AsrDownloadProgress>("asr-download-progress", (e) => {
      setDlMsg(e.payload.message);
      setDlPct(e.payload.percent ?? null);
    }).then((fn) => {
      un = fn;
    });
    return () => {
      un?.();
    };
  }, []);

  const s = STEPS[Math.min(step, STEPS.length - 1)];
  const asrReady = !!asr?.sensevoice_ready;
  const whisperReady = !!asr?.whisper_ready;
  const localReady =
    (engineChoice === "sensevoice" && asrReady) ||
    (engineChoice === "whisper" && whisperReady) ||
    engineChoice === "speech";
  const windows = platform?.os === "windows";

  async function go(next: number) {
    setBusy(true);
    try {
      await api.setOnboardingStep(next);
      setStep(next);
      setGuide(null);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finish(start: boolean) {
    setDoneFlash(true);
    setBusy(true);
    let observeError: string | null = null;
    try {
      await api.completeOnboarding(start || launch);
      if (start || launch) {
        try {
          await api.observeStart();
        } catch (e) {
          observeError = String(e);
        }
      }
      if (observeError) {
        // Surface the start failure but allow completing onboarding.
        setDoneFlash(false);
        setBusy(false);
        setError(`本地服务启动失败：${observeError}（可稍后在概览页手动启动）`);
        return;
      }
      setTimeout(() => onDone(), 420);
    } catch (e) {
      setDoneFlash(false);
      setBusy(false);
      setError(String(e));
    }
  }

  async function skip() {
    setBusy(true);
    try {
      await api.skipOnboarding();
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function applyEngine(eng: string) {
    setBusy(true);
    setGuide(null);
    setError(null);
    try {
      setEngineChoice(eng);
      setAsr(await api.setAsrEnginePreference(eng));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function requestScreen() {
    setBusy(true);
    setGuide("正在打开系统设置。请在列表中打开 Lumen Cua 的开关；没有条目就点 + 选择 /Applications/Lumen Cua.app。这里会自动检测到授权。");
    try {
      await api.openPrivacySettings("screen");
    } catch {
      // backend also opens Settings
    }
    try {
      const granted = await api.requestScreenPermission();
      screenPermissionPending.current = !granted;
      setPerms(await api.getPermissions());
      setGuide(
        granted
          ? null
          : "macOS 未授予 Lumen Cua。reset 后常需手动开启；授权后本页会自动检测到。",
      );
    } catch (e) {
      screenPermissionPending.current = true;
      try {
        await api.openPrivacySettings("screen");
      } catch {
        // still surface the original error below
      }
      setError(`请求屏幕录制权限失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  async function inspectMicrophone(openSettings: boolean) {
    setBusy(true);
    setGuide(null);
    setError(null);
    try {
      if (openSettings) {
        await api.openPrivacySettings("microphone");
      }
      const nextPerms = await api.getPermissions();
      setPerms(nextPerms);
      if (nextPerms.microphone.toLowerCase() !== "granted") {
        microphonePermissionPending.current = true;
        setAudioProbe({
          permission: nextPerms.microphone,
          ready: false,
          error: "麦克风权限尚未允许",
        });
        setGuide("在系统设置的麦克风列表打开 Lumen Navi；授权后本页会自动检测到。");
        return;
      }
      const probe = await api.checkAudioReadiness();
      setAudioProbe(probe);
      microphonePermissionPending.current = !probe.ready;
      setGuide(
        probe.ready
          ? null
          : `权限已允许，但采集无法启动：${probe.error ?? "未知错误"}。请检查输入设备后重试。`,
      );
    } catch (e) {
      setError(`请求麦克风权限失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  const micGranted = perms?.microphone.toLowerCase() === "granted";
  const axGranted = perms?.accessibility.toLowerCase().includes("granted");
  const screenGranted = perms?.screen_recording.toLowerCase().includes("granted");

  return (
    <div className="onboard-backdrop">
      <div className={`onboard-card ${step === 0 || step === 4 ? "wide" : ""}`}>
        <div className="onboard-kicker">
          首次设置 · {step + 1}/{STEPS.length}
        </div>

        {/* Stepper */}
        <div className="onboard-stepper" aria-label={`第 ${step + 1} 步，共 ${STEPS.length} 步`}>
          {STEPS.map((st, i) => (
            <span key={st.title} style={{ display: "flex", alignItems: "center", flex: i < STEPS.length - 1 ? 1 : undefined }}>
              <span
                className={`dot ${i < step ? "done" : i === step ? "current" : ""}`}
                title={st.title}
              />
              {i < STEPS.length - 1 && <span className="line" style={{ flex: 1 }} />}
            </span>
          ))}
        </div>

        <div className="onboard-step-body" key={step}>
          <div className="row" style={{ gap: 10, alignItems: "center", marginTop: 10 }}>
            <Icon name={s.icon} size={22} />
            <h2 style={{ margin: 0 }}>{s.title}</h2>
          </div>
          <p>{s.body}</p>

          {/* ---- Step 0: Welcome with product demo ---- */}
          {step === 0 && (
            <div className="onboard-cols">
              <div className="onboard-demo">
                <div className="demo-kicker">时间线 · 今天 14:30</div>
                <div className="demo-row">
                  <span className="mono" style={{ fontSize: 10, color: "var(--text-tertiary)" }}>14:30</span>
                  <span className="demo-bar fill" />
                </div>
                <div className="demo-row">
                  <span className="mono" style={{ fontSize: 10, color: "var(--text-tertiary)" }}>14:35</span>
                  <span className="demo-bar" />
                </div>
                <div className="demo-row">
                  <span className="mono" style={{ fontSize: 10, color: "var(--text-tertiary)" }}>14:40</span>
                  <span className="demo-bar fill" />
                </div>
                <div className="meta" style={{ lineHeight: 1.5 }}>
                  屏幕与声音持续记录，自动生成 15 分钟卡片：
                  应用、标题、叙述，全部可搜索。
                </div>
                <div className="row" style={{ gap: 6, flexWrap: "wrap" }}>
                  <Pill tone="accent">OCR 全文</Pill>
                  <Pill tone="durable">语音转写</Pill>
                  <Pill tone="neutral">时间统计</Pill>
                </div>
              </div>
              <div className="stack" style={{ gap: 10 }}>
                <Notice tone="info" title="本地优先">
                  所有数据只存本机；隐私暂停、闭眼模式、锁屏停录都是一等公民。
                </Notice>
                <Notice tone="info" title="三个平面">
                  Observe（记录）→ Memory（15 分钟卡片与搜索）→ Act（划词问答，后续支持写回）。本向导只配置 Observe。
                </Notice>
              </div>
            </div>
          )}

          {/* ---- Step 1: Screen Recording with mock settings panel ---- */}
          {step === 1 && !windows && (
            <div className="stack" style={{ marginTop: 12 }}>
              <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                <Pill tone={screenGranted ? "success" : "warn"}>
                  Screen {permissionLabel(perms?.screen_recording)}
                </Pill>
                {platform?.screen_permission_gate !== false && (
                  <Pill tone={perms?.screen_capture_ready ? "success" : "warn"}>
                    Capture {captureStatusLabel(perms?.direct_capture_status)}
                  </Pill>
                )}
                {(screenPermissionPending.current || !screenGranted) && (
                  <StatusDot status="running" label="等待授权…" />
                )}
              </div>
              <div className="mock-settings">
                <div className="ms-head">
                  <Icon name="settings" size={14} /> 系统设置 → 隐私与安全性 → 屏幕与系统音频录制
                </div>
                <div className="ms-row">
                  <span>屏幕共享助手</span>
                  <span className={`mock-toggle ${screenGranted ? "on" : ""}`} />
                </div>
                <div className="ms-row highlight">
                  <span>Lumen Cua</span>
                  <span className={`mock-toggle ${screenGranted ? "on" : ""}`} />
                </div>
                <div className="ms-row">
                  <span>其他应用…</span>
                  <span className="meta">在系统设置中管理</span>
                </div>
              </div>
              <div className="row">
                <Button
                  variant="primary"
                  icon="folder"
                  disabled={busy}
                  onClick={() => void requestScreen()}
                >
                  请求 / 打开屏幕权限
                </Button>
              </div>
            </div>
          )}
          {step === 1 && windows && (
            <div className="row" style={{ marginTop: 12 }}>
              <Pill tone="success">Windows 无需单独授权</Pill>
            </div>
          )}

          {/* ---- Step 2: Microphone + Accessibility ---- */}
          {step === 2 && (
            <div className="stack" style={{ marginTop: 12 }}>
              <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
                <Pill tone={micGranted ? "success" : "warn"}>
                  Mic {permissionLabel(perms?.microphone)}
                </Pill>
                {!windows && (
                  <Pill tone={axGranted ? "success" : "warn"}>
                    划词 {permissionLabel(perms?.accessibility)}
                  </Pill>
                )}
                {(microphonePermissionPending.current || !micGranted) && (
                  <StatusDot status="running" label="等待麦克风授权…" />
                )}
              </div>
              <div className="row" style={{ flexWrap: "wrap" }}>
                <Button
                  variant="primary"
                  icon="microphone"
                  disabled={busy}
                  onClick={() => void inspectMicrophone(!micGranted)}
                >
                  {micGranted ? "重新检查麦克风" : "打开系统设置配置麦克风"}
                </Button>
                {platform?.system_speech_asr !== false && (
                  <Button
                    variant="secondary"
                    disabled={busy}
                    onClick={() => void api.openPrivacySettings("speech")}
                  >
                    语音识别设置（Speech 回退）
                  </Button>
                )}
              </div>
              <div className={`onboard-status ${audioProbe?.ready ? "ok" : ""}`}>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <strong>采集设备检查</strong>
                  <Pill tone={audioProbe?.ready ? "success" : "warn"}>
                    {audioProbe ? (audioProbe.ready ? "可用" : "需要处理") : "检查中…"}
                  </Pill>
                </div>
                <p className="meta" style={{ marginTop: 6 }}>
                  {audioProbe?.ready
                    ? "设备和音频格式已验证。完成 onboarding 后，点击「开始采集」才会启动持续录音。"
                    : audioProbe?.error ?? "先在系统设置允许麦克风访问，再回来重新检查。"}
                </p>
              </div>
              {!windows && (
                <Notice tone="info" title="划词助手（可选）">
                  在任意应用选中文字即可弹窗翻译/提问。需要在「辅助功能」里允许 Lumen Navi。
                  <div style={{ marginTop: 6 }}>
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={busy || axGranted}
                      onClick={() =>
                        void api
                          .requestAccessibilityPermission()
                          .then(() => api.getPermissions().then(setPerms))
                          .catch((e: unknown) => setError(String(e)))
                      }
                    >
                      {axGranted ? "已授权" : "请求辅助功能权限"}
                    </Button>
                  </div>
                </Notice>
              )}
            </div>
          )}

          {/* ---- Step 3: ASR (decluttered) ---- */}
          {step === 3 && (
            <div className="stack" style={{ marginTop: 12, gap: 12 }}>
              <div className="field">
                <span className="meta">引擎</span>
                <Select
                  value={engineChoice}
                  disabled={busy}
                  onChange={(e) => void applyEngine(e.target.value)}
                  style={{ maxWidth: 340 }}
                >
                  <option value="sensevoice">SenseVoice（本地，推荐）</option>
                  <option value="whisper">Whisper（本地）</option>
                  {platform?.system_speech_asr !== false && (
                    <option value="speech">macOS Speech（无需下载）</option>
                  )}
                </Select>
              </div>

              {engineChoice === "sensevoice" && (
                <div className={`onboard-status ${asrReady ? "ok" : ""}`}>
                  <div className="row" style={{ justifyContent: "space-between" }}>
                    <div className="row" style={{ gap: 8 }}>
                      <strong>SenseVoice</strong>
                      <span className="meta">
                        {asrReady ? "共享目录已就绪，所有 Lumen 应用共用" : "下载一次即可，约 230MB"}
                      </span>
                    </div>
                    <Pill tone={asrReady ? "success" : "warn"}>
                      {asrReady ? "就绪" : "未就绪"}
                    </Pill>
                  </div>
                  {!asrReady && (
                    <div className="row" style={{ marginTop: 8, flexWrap: "wrap" }}>
                      <Button
                        variant="primary"
                        disabled={busy}
                        onClick={() =>
                          void (async () => {
                            setBusy(true);
                            setError(null);
                            setGuide(null);
                            setDlMsg("开始下载…");
                            setDlPct(null);
                            try {
                              setAsr(await api.startAsrModelDownload());
                              setDlMsg("完成");
                            } catch (e) {
                              setError(String(e));
                            } finally {
                              setBusy(false);
                            }
                          })()
                        }
                      >
                        下载 SenseVoice（共享目录）
                      </Button>
                      <Button
                        variant="secondary"
                        disabled={!busy}
                        onClick={() => void api.cancelAsrModelDownload()}
                      >
                        取消下载
                      </Button>
                    </div>
                  )}
                  {(dlMsg || dlPct != null) && (
                    <div style={{ marginTop: 8 }}>
                      <ProgressBar
                        value={dlPct ?? 0}
                        label={`${dlMsg}${dlPct != null ? ` · ${dlPct.toFixed(0)}%` : ""}`}
                        tone={dlPct != null && dlPct >= 100 ? "success" : "accent"}
                      />
                    </div>
                  )}
                  {busy && dlPct == null && dlMsg && (
                    <div style={{ marginTop: 8 }}>
                      <Spinner size={14} />
                    </div>
                  )}
                </div>
              )}

              {engineChoice === "whisper" && (
                <div className={`onboard-status ${whisperReady ? "ok" : ""}`}>
                  <div className="row" style={{ justifyContent: "space-between" }}>
                    <strong>Whisper</strong>
                    <Pill tone={whisperReady ? "success" : "warn"}>
                      {whisperReady ? "就绪" : "未就绪"}
                    </Pill>
                  </div>
                  <p className="meta" style={{ marginTop: 6 }}>
                    暂无内置下载；在下方「高级」里选择已有目录，或改用 SenseVoice。
                  </p>
                </div>
              )}

              {engineChoice === "speech" && (
                <Notice tone="info" title="macOS Speech">
                  将使用系统语音识别，无需下载模型；需在系统设置中授权语音识别。
                </Notice>
              )}

              <details className="onboard-advanced">
                <summary>高级：自定义模型目录 / 使用已有模型</summary>
                <div className="stack" style={{ paddingTop: 6, gap: 10 }}>
                  {asr?.models_root && (
                    <div>
                      <div className="meta">Lumen 共享模型目录（全应用）</div>
                      <p className="meta mono" style={{ wordBreak: "break-all", marginTop: 4 }}>
                        {asr.models_root}
                      </p>
                    </div>
                  )}
                  {(engineChoice === "sensevoice" || engineChoice === "whisper") &&
                    asr &&
                    asr.candidates.filter((c) => c.ready && c.engine === engineChoice).length >
                      0 && (
                      <div>
                        <div className="meta" style={{ marginBottom: 6 }}>
                          检测到的本地模型
                        </div>
                        {asr.candidates
                          .filter((c) => c.ready && c.engine === engineChoice)
                          .map((c) => (
                            <div key={c.path} className="onboard-candidate">
                              <span className="meta" style={{ wordBreak: "break-all" }}>
                                {c.label}
                              </span>
                              <Button
                                variant="secondary"
                                size="sm"
                                disabled={busy}
                                onClick={() =>
                                  void (async () => {
                                    setBusy(true);
                                    setError(null);
                                    try {
                                      setAsr(
                                        await api.useExistingAsrModel(c.path, engineChoice),
                                      );
                                    } catch (e) {
                                      setError(String(e));
                                    } finally {
                                      setBusy(false);
                                    }
                                  })()
                                }
                              >
                                使用
                              </Button>
                            </div>
                          ))}
                      </div>
                    )}
                  {(engineChoice === "sensevoice" || engineChoice === "whisper") && (
                    <div className="row">
                      <input
                        className="input"
                        style={{ flex: 1 }}
                        placeholder="或粘贴本地模型目录路径…"
                        value={customPath}
                        disabled={busy}
                        onChange={(e) => setCustomPath(e.target.value)}
                      />
                      <Button
                        variant="secondary"
                        disabled={busy || !customPath.trim()}
                        onClick={() =>
                          void (async () => {
                            setBusy(true);
                            setError(null);
                            try {
                              setAsr(
                                await api.useExistingAsrModel(customPath.trim(), engineChoice),
                              );
                            } catch (e) {
                              setError(String(e));
                            } finally {
                              setBusy(false);
                            }
                          })()
                        }
                      >
                        验证并使用
                      </Button>
                    </div>
                  )}
                  <div className="row">
                    <Button variant="ghost" size="sm" disabled={busy} onClick={() => void refreshAsr()}>
                      刷新状态
                    </Button>
                  </div>
                  {asr && (
                    <p className="meta mono" style={{ margin: 0 }}>
                      当前配置: engine={asr.active_engine}
                      {asr.active_model_dir ? ` · ${asr.active_model_dir}` : ""}
                    </p>
                  )}
                </div>
              </details>
            </div>
          )}

          {/* ---- Step 4: Ready with status summary ---- */}
          {step === 4 && (
            <div className="onboard-cols">
              <div className="onboard-demo">
                <div className="demo-kicker">状态总览</div>
                <div className="stack" style={{ gap: 8 }}>
                  <Pill tone={screenGranted ? "success" : "warn"}>
                    屏幕截取 {screenGranted ? "已授权" : "未授权（可在概览页配置）"}
                  </Pill>
                  <Pill tone={micGranted ? "success" : "warn"}>
                    麦克风 {micGranted ? "已授权" : "未授权"}
                  </Pill>
                  <Pill tone={localReady ? "success" : "warn"}>
                    转写引擎 {localReady ? engineChoice : "待配置"}
                  </Pill>
                  {!windows && (
                    <Pill tone={axGranted ? "success" : "neutral"}>
                      划词助手 {axGranted ? "可用" : "未授权（可选）"}
                    </Pill>
                  )}
                </div>
              </div>
              <div className="stack" style={{ gap: 12 }}>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={launch}
                    onChange={(e) => setLaunch(e.target.checked)}
                  />
                  以后启动应用时运行本地服务（仅采集已开启的通道）
                </label>
                <Notice tone="info" title="隐私随时可控">
                  概览页可以一键暂停全部采集；闭眼模式立即停录；数据目录完全透明。
                </Notice>
                {doneFlash && (
                  <div className="onboard-success">
                    <Icon name="check" size={18} /> 已完成，正在进入…
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        {guide && (
          <div style={{ marginTop: 12 }}>
            <Notice tone="warn" title="操作指引">
              {guide}
            </Notice>
          </div>
        )}
        {error && (
          <div style={{ marginTop: 12 }}>
            <Notice tone="danger" title="出错了">
              {error}
            </Notice>
          </div>
        )}

        <div className="row mt" style={{ justifyContent: "space-between", marginTop: 16 }}>
          <Button variant="ghost" disabled={busy} onClick={() => void skip()}>
            跳过全部
          </Button>
          <div className="row">
            {step > 0 && (
              <Button variant="secondary" disabled={busy} onClick={() => void go(step - 1)}>
                上一步
              </Button>
            )}
            {step === 3 && (
              <Button variant="ghost" disabled={busy} onClick={() => void go(4)}>
                跳过（稍后配置）
              </Button>
            )}
            {step < STEPS.length - 1 ? (
              <Button
                variant="primary"
                icon="chevronRight"
                disabled={busy || (step === 3 && !localReady)}
                onClick={() => void go(step + 1)}
              >
                下一步
              </Button>
            ) : (
              <Button
                variant="primary"
                icon="check"
                disabled={busy || doneFlash}
                onClick={() => void finish(false)}
              >
                完成{launch ? "并启动采集" : ""}
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
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
      return v || "待确认";
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
    default:
      return v || "待验证";
  }
}
