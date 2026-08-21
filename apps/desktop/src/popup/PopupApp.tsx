import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";

/**
 * Selection popup (划词弹窗). Shows the text captured from the frontmost app;
 * every action is explicit — nothing is sent to the LLM until the user clicks.
 */
export default function PopupApp() {
  const [text, setText] = useState("");
  const [result, setResult] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [question, setQuestion] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [targetApp, setTargetApp] = useState<string | null>(null);
  const [injecting, setInjecting] = useState(false);
  const [agents, setAgents] = useState<Array<{ id: string; label: string }>>([]);
  const [agentId, setAgentId] = useState("http");
  const reqIdRef = useRef<string | null>(null);

  useEffect(() => {
    // First load: pull the text that triggered this window (event may have
    // fired before the webview subscribed).
    api
      .selectionPopupCurrent()
      .then((t) => {
        if (t) setText(t);
      })
      .catch(() => {});
    api
      .assistantAgents()
      .then((list) => setAgents(list))
      .catch(() => {});

    const unlisteners: Array<() => void> = [];
    const sub = <T,>(event: string, cb: (payload: T) => void) => {
      listen<T>(event, (e) => cb(e.payload)).then((u) => unlisteners.push(u));
    };

    sub<{ text: string; target?: string | null }>("selection-changed", ({ text, target }) => {
      cancelInFlight();
      setText(text);
      setTargetApp(target ?? null);
      setResult("");
      setError(null);
      setStreaming(false);
      setQuestion("");
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
    api.selectionPopupHide().catch(() => {});
  }

  async function run(action: "translate" | "ask", q?: string) {
    if (!text.trim() || streaming) return;
    setResult("");
    setError(null);
    setStreaming(true);
    try {
      const useAgent = action === "ask" && agentId !== "http" ? agentId : undefined;
      reqIdRef.current = await api.assistantRun(action, text, q, useAgent);
    } catch (e) {
      setStreaming(false);
      setError(String(e));
    }
  }

  async function stop() {
    cancelInFlight();
    setStreaming(false);
  }

  async function copy() {
    const content = result || text;
    if (!content) return;
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // clipboard requires focus; ignore failures silently
    }
  }

  /** Write the assistant result back into the app the text came from. */
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
          Lumen <span>划词</span>
        </div>
        <button className="popup-close" onClick={close} title="关闭 (Esc)">
          ✕
        </button>
      </div>

      <div className="popup-selection">{text || "未获取到选中文字"}</div>

      <div className="popup-actions">
        <button
          className="popup-btn primary"
          disabled={!text.trim() || streaming}
          onClick={() => run("translate")}
        >
          翻译
        </button>
        <button className="popup-btn" onClick={copy}>
          {copied ? "已复制" : "复制"}
        </button>
        {streaming && (
          <button className="popup-btn" onClick={stop}>
            停止
          </button>
        )}
      </div>

      <div className="popup-ask">
        <input
          value={question}
          placeholder="针对这段文字提问…"
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && question.trim() && !streaming) {
              run("ask", question.trim());
            }
          }}
        />
        <button
          className="popup-btn"
          disabled={!text.trim() || !question.trim() || streaming}
          onClick={() => run("ask", question.trim())}
        >
          提问
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
            title="本地 agent 在 navi.toml [agents] 中启用后出现"
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
                ? `把结果写回 ${targetApp} 的原文本框（替换选中内容）`
                : "来源应用未知（⌘C 兜底路径无目标），无法写回"
            }
            onClick={() => inject("replace")}
          >
            {injecting ? "写入中…" : targetApp ? `写入 ${targetApp}` : "写入原文"}
          </button>
          <button
            className="popup-btn"
            disabled={!targetApp || injecting}
            title={`在 ${targetApp ?? "原文本框"}现有内容后追加`}
            onClick={() => inject("append")}
          >
            追加
          </button>
        </div>
      )}

      <div className="popup-hint">Esc 关闭 · 点击其他区域自动隐藏</div>
    </div>
  );
}
