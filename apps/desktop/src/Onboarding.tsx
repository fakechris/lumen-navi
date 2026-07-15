import { useEffect, useState } from "react";
import { api } from "./api";
import type { OnboardingState, Permissions } from "./types";

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
    title: "麦克风与语音识别",
    body: "音频 chunk 需要麦克风；本地转写需要 Speech Recognition。听写注入仍由独立产品 Lumen ASR 负责。",
    kind: "microphone" as const,
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
  const [step, setStep] = useState(initial.step);
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [launch, setLaunch] = useState(initial.launch_observe);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void api.getPermissions().then(setPerms).catch(() => {});
  }, [step]);

  const s = STEPS[Math.min(step, STEPS.length - 1)];

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
          // Still complete onboarding; user can start later.
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

  return (
    <div className="onboard-backdrop">
      <div className="onboard-card">
        <div className="onboard-kicker">首次设置 · {step + 1}/{STEPS.length}</div>
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
                语音识别设置
              </button>
            </div>
          </div>
        )}

        {step === 3 && (
          <label className="check mt">
            <input
              type="checkbox"
              checked={launch}
              onChange={(e) => setLaunch(e.target.checked)}
            />
            以后启动应用时自动开始 Observe
          </label>
        )}

        {error && <div className="error mt" style={{ margin: "12px 0 0" }}>{error}</div>}

        <div className="row mt" style={{ justifyContent: "space-between" }}>
          <button className="btn" disabled={busy} onClick={() => void skip()}>
            跳过
          </button>
          <div className="row">
            {step > 0 && (
              <button className="btn" disabled={busy} onClick={() => void go(step - 1)}>
                上一步
              </button>
            )}
            {step < STEPS.length - 1 ? (
              <button className="btn primary" disabled={busy} onClick={() => void go(step + 1)}>
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
