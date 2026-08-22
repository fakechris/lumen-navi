import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";

/**
 * Quick composer (⌥Space) — free-form prompt with the same context / agent /
 * inject machinery as the selection popup. No selection required.
 */
export default function ComposerApp() {
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [targetApp, setTargetApp] = useState<string | null>(null);
  const [injecting, setInjecting] = useState(false);
  const [agents, setAgents] = useState<Array<{ id: string; label: string }>>([]);
  const [agentId, setAgentId] = useState("http");
  const reqIdRef = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    api
      .assistantAgents()
      .then((list) => setAgents(list))
      .catch(() => {});

    const unlisteners: Array<() => void> = [];
    const sub = <T,>(event: string, cb: (payload: T) => void) => {
      listen<T>(event, (e) => cb(e.payload)).then((u) => unlisteners.push(u));
    };

    sub<{ target?: string | null }>("composer-shown", ({ target }) => {
      setTargetApp(target ?? null);
      setPrompt("");
      setResult("");
      setError(null);
      setStreaming(false);
      inputRef.current?.focus();
    });
    sub<{ id: string; delta: string }>("assistant-stream", ({ id, delta }) => {
      if (id !== reqIdRef.current) return;
      setResult((r) => r + delta);
    });
    sub<{ id: string }>("assistant-done", ({ id }) => {
      if (id !== reqIdRef.current) return;
      reqIdRef.current = null;
      setStreaming(false);
    });
    sub<{ id: string; message: string }>("assistant-error", ({ id, message }) => {
      if (id !== reqIdRef.current) return;
      reqIdRef.current = null;
      setStreaming(false);
      setError(message);
    });

    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unlisteners.forEach((u) => u());
      window.removeEventListener("keydown", onKey);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function cancelInFlight() {
    if (reqIdRef.current) {
      api.assistantCancel(reqIdRef.current).catch(() => {});
      reqIdRef.current = null;
    }
  }

  function close() {
    cancelInFlight();
    api.composerHide().catch(() => {});
  }

  async function run() {
    if (!prompt.trim() || streaming) return;
    setResult("");
    setError(null);
    setStreaming(true);
    try {
      const useAgent = agentId !== "http" ? agentId : undefined;
      reqIdRef.current = await api.assistantRun("compose", "", prompt.trim(), useAgent);
    } catch (e) {
      setStreaming(false);
      setError(String(e));
    }
  }

  async function inject(mode: "replace" | "append") {
    if (!result.trim() || injecting) return;
    setInjecting(true);
    setError(null);
    try {
      await api.assistantInject(mode, result);
    } catch (e) {
      setError(String(e));
    }
    setInjecting(false);
  }

  return (
    <div className="popup">
      <div className="popup-head">
        <div className="popup-title">
          Lumen <span>Composer</span>
        </div>
        {targetApp && (
          <span className="meta" style={{ marginRight: 8 }}>
            写回 {targetApp}
          </span>
        )}
        <button className="popup-close" onClick={close} title="关闭 (Esc)">
          ✕
        </button>
      </div>

      <div className="popup-ask">
        <input
          ref={inputRef}
          value={prompt}
          placeholder="问点什么，或让 AI 处理当前屏幕内容…（⌥Space 呼出）"
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && prompt.trim() && !streaming) {
              run();
            }
          }}
        />
        <button
          className="popup-btn primary"
          disabled={!prompt.trim() || streaming}
          onClick={run}
        >
          {streaming ? "生成中" : "发送"}
        </button>
      </div>

      {agents.length > 1 && (
        <div className="popup-agent-row">
          <span className="popup-agent-label">引擎</span>
          <select
            className="popup-agent-select"
            value={agentId}
            disabled={streaming}
            onChange={(e) => setAgentId(e.target.value)}
          >
            {agents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
          </select>
        </div>
      )}

      {error && <div className="popup-error">{error}</div>}
      {(result || streaming) && (
        <div className="popup-result">
          {result}
          {streaming && <span className="cursor" />}
        </div>
      )}

      {!streaming && result.trim() && (
        <div className="popup-actions">
          <button
            className="popup-btn primary"
            disabled={!targetApp || injecting}
            title={
              targetApp
                ? `把结果写回 ${targetApp}（替换选中/光标处内容）`
                : "呼出时没有检测到可写回的输入框"
            }
            onClick={() => inject("replace")}
          >
            {injecting ? "写入中…" : targetApp ? `写入 ${targetApp}` : "写入原文"}
          </button>
          <button
            className="popup-btn"
            disabled={!targetApp || injecting}
            onClick={() => inject("append")}
          >
            追加
          </button>
          <button
            className="popup-btn"
            onClick={() => void navigator.clipboard.writeText(result).catch(() => {})}
          >
            复制
          </button>
        </div>
      )}

      <div className="popup-hint">
        Esc 关闭 · 自动附带屏幕 OCR 与近期记录 · Enter 发送
      </div>
    </div>
  );
}
