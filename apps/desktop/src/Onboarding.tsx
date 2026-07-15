import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type {
  AsrDownloadProgress,
  AsrModelStatus,
  OnboardingState,
  Permissions,
} from "./types";

const STEPS = [
  {
    title: "欢迎使用 Lumen Navi",
    body: "本地持续记录屏幕与声音，转成可搜索的上下文。数据默认留在本机 Application Support。",
  },
  {
    title: "屏幕录制",
    body: "Observe 需要 Screen Recording 权限才能截取屏幕。点击下方按钮打开系统设置并授权本应用。",
    kind: "screen" as const,
  },
  {
    title: "麦克风",
    body: "音频 chunk 需要麦克风权限。持续转写默认走本机 SenseVoice；也可改用 macOS Speech。",
    kind: "microphone" as const,
  },
  {
    title: "本地 ASR 模型",
    body: "默认 SenseVoice（sherpa-onnx）。可选用本机已有模型，或下载官方 int8 包。也可暂时用 Speech。",
  },
  {
    title: "准备就绪",
    body: "可以随时在概览页开始/停止 Observe。也可在设置里重新打开本引导。",
  },
];

export function Onboarding({
  initial,
  onDone,
}: {
  initial: OnboardingState;
  onDone: () => void;
}) {
  const [step, setStep] = useState(Math.min(initial.step, STEPS.length - 1));
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [launch, setLaunch] = useState(initial.launch_observe);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [asr, setAsr] = useState<AsrModelStatus | null>(null);
  const [engineChoice, setEngineChoice] = useState("sensevoice");
  const [customPath, setCustomPath] = useState("");
  const [dlMsg, setDlMsg] = useState("");
  const [dlPct, setDlPct] = useState<number | null>(null);

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
    void api.getPermissions().then(setPerms).catch(() => {});
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

  async function go(next: number) {
    setBusy(true);
    try {
      await api.setOnboardingStep(next);
      setStep(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finish(start: boolean) {
    setBusy(true);
    try {
      await api.completeOnboarding(start || launch);
      if (start || launch) {
        try {
          await api.observeStart();
        } catch (e) {
          setError(String(e));
        }
      }
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
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
    setError(null);
    try {
      setEngineChoice(eng);
      if (eng === "speech") {
        setAsr(await api.setAsrEnginePreference("speech"));
      } else if (eng === "sensevoice" || eng === "whisper") {
        // Prefer existing ready path if known
        const status = await api.setAsrEnginePreference(eng);
        setAsr(status);
      } else {
        setAsr(await api.setAsrEnginePreference(eng));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="onboard-backdrop">
      <div className="onboard-card" style={{ width: "min(560px, 100%)" }}>
        <div className="onboard-kicker">
          首次设置 · {step + 1}/{STEPS.length}
        </div>
        <h2>{s.title}</h2>
        <p>{s.body}</p>

        {step === 1 && (
          <div className="row mt">
            <span className={`pill ${permClass(perms?.screen_recording)}`}>
              Screen {perms?.screen_recording ?? "…"}
            </span>
            <button
              className="btn primary"
              disabled={busy}
              onClick={() => {
                void api.requestScreenPermission();
                void api.openPrivacySettings("screen");
              }}
            >
              请求 / 打开屏幕权限
            </button>
          </div>
        )}

        {step === 2 && (
          <div className="stack mt">
            <div className="row">
              <span className={`pill ${permClass(perms?.microphone)}`}>
                Mic {perms?.microphone ?? "…"}
              </span>
              <button
                className="btn"
                disabled={busy}
                onClick={() => void api.openPrivacySettings("microphone")}
              >
                麦克风设置
              </button>
              <button
                className="btn"
                disabled={busy}
                onClick={() => void api.openPrivacySettings("speech")}
              >
                语音识别设置（Speech 回退）
              </button>
            </div>
          </div>
        )}

        {step === 3 && (
          <div className="stack mt">
            <div className="field">
              <span className="meta">引擎</span>
              <select
                className="input"
                value={engineChoice}
                disabled={busy}
                onChange={(e) => void applyEngine(e.target.value)}
              >
                <option value="sensevoice">SenseVoice（本地，推荐）</option>
                <option value="whisper">Whisper（本地）</option>
                <option value="speech">macOS Speech（无需下载）</option>
              </select>
            </div>

            {engineChoice === "sensevoice" && (
              <div className={`onboard-status ${asrReady ? "ok" : ""}`}>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <strong>SenseVoice</strong>
                  <span className={`pill ${asrReady ? "ok" : "warn"}`}>
                    {asrReady ? "就绪" : "未就绪"}
                  </span>
                </div>
                <p className="meta mono" style={{ wordBreak: "break-all", marginTop: 6 }}>
                  {asr?.sensevoice_dir ?? "…"}
                </p>
              </div>
            )}

            {engineChoice === "whisper" && (
              <div className={`onboard-status ${whisperReady ? "ok" : ""}`}>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <strong>Whisper</strong>
                  <span className={`pill ${whisperReady ? "ok" : "warn"}`}>
                    {whisperReady ? "就绪" : "未就绪"}
                  </span>
                </div>
                <p className="meta mono" style={{ wordBreak: "break-all", marginTop: 6 }}>
                  {asr?.whisper_dir ?? "…"}
                </p>
                <p className="meta mt">
                  Whisper 暂无内置下载；请选择已有 sherpa Whisper 目录，或改用 SenseVoice。
                </p>
              </div>
            )}

            {engineChoice === "speech" && (
              <div className="onboard-status ok">
                <p className="meta" style={{ margin: 0 }}>
                  将使用系统 Speech Recognition。无需下载模型；需在系统设置中授权语音识别。
                </p>
              </div>
            )}

            {(engineChoice === "sensevoice" || engineChoice === "whisper") &&
              asr &&
              asr.candidates.filter(
                (c) => c.ready && c.engine === engineChoice,
              ).length > 0 && (
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
                        <button
                          type="button"
                          className="btn"
                          disabled={busy}
                          onClick={() =>
                            void (async () => {
                              setBusy(true);
                              setError(null);
                              try {
                                setAsr(
                                  await api.useExistingAsrModel(c.path, engineChoice),
                                );
                                setEngineChoice(engineChoice);
                              } catch (e) {
                                setError(String(e));
                              } finally {
                                setBusy(false);
                              }
                            })()
                          }
                        >
                          使用
                        </button>
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
                <button
                  type="button"
                  className="btn"
                  disabled={busy || !customPath.trim()}
                  onClick={() =>
                    void (async () => {
                      setBusy(true);
                      setError(null);
                      try {
                        setAsr(
                          await api.useExistingAsrModel(
                            customPath.trim(),
                            engineChoice,
                          ),
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
                </button>
              </div>
            )}

            {engineChoice === "sensevoice" && (
              <div className="row">
                <button
                  type="button"
                  className="btn primary"
                  disabled={busy || asrReady}
                  onClick={() =>
                    void (async () => {
                      setBusy(true);
                      setError(null);
                      setDlMsg("开始下载…");
                      setDlPct(null);
                      try {
                        setAsr(await api.startAsrModelDownload());
                        setDlMsg("完成");
                        setEngineChoice("sensevoice");
                      } catch (e) {
                        setError(String(e));
                      } finally {
                        setBusy(false);
                      }
                    })()
                  }
                >
                  {asrReady ? "已就绪" : "下载 SenseVoice"}
                </button>
                <button
                  type="button"
                  className="btn"
                  disabled={!busy}
                  onClick={() => void api.cancelAsrModelDownload()}
                >
                  取消下载
                </button>
                <button
                  type="button"
                  className="btn"
                  disabled={busy}
                  onClick={() => void refreshAsr()}
                >
                  刷新
                </button>
              </div>
            )}

            {(dlMsg || dlPct != null) && (
              <div className="stack">
                <p className="meta" style={{ margin: 0 }}>
                  {dlMsg}
                  {dlPct != null ? ` · ${dlPct.toFixed(0)}%` : ""}
                </p>
                {dlPct != null && (
                  <div className="progress-track">
                    <div
                      className="progress-fill"
                      style={{ width: `${Math.min(100, Math.max(0, dlPct))}%` }}
                    />
                  </div>
                )}
              </div>
            )}

            {asr && (
              <p className="meta mono" style={{ margin: 0 }}>
                当前配置: engine={asr.active_engine}
                {asr.active_model_dir ? ` · ${asr.active_model_dir}` : ""}
              </p>
            )}
          </div>
        )}

        {step === 4 && (
          <label className="check mt">
            <input
              type="checkbox"
              checked={launch}
              onChange={(e) => setLaunch(e.target.checked)}
            />
            以后启动应用时自动开始 Observe
          </label>
        )}

        {error && (
          <div className="error mt" style={{ margin: "12px 0 0" }}>
            {error}
          </div>
        )}

        <div className="row mt" style={{ justifyContent: "space-between" }}>
          <button className="btn" disabled={busy} onClick={() => void skip()}>
            跳过全部
          </button>
          <div className="row">
            {step > 0 && (
              <button className="btn" disabled={busy} onClick={() => void go(step - 1)}>
                上一步
              </button>
            )}
            {step === 3 && (
              <button className="btn" disabled={busy} onClick={() => void go(4)}>
                跳过（稍后配置）
              </button>
            )}
            {step < STEPS.length - 1 ? (
              <button
                className="btn primary"
                disabled={busy || (step === 3 && !localReady)}
                onClick={() => void go(step + 1)}
              >
                下一步
              </button>
            ) : (
              <>
                <button className="btn" disabled={busy} onClick={() => void finish(false)}>
                  完成
                </button>
                <button className="btn primary" disabled={busy} onClick={() => void finish(true)}>
                  完成并开始 Observe
                </button>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function permClass(v?: string): "ok" | "warn" | "err" {
  const s = (v ?? "").toLowerCase();
  if (s.includes("granted")) return "ok";
  if (s.includes("denied") || s.includes("restricted")) return "err";
  return "warn";
}
