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

    const unlisteners: Array<() => void> = [];
    const sub = <T,>(event: string, cb: (payload: T) => void) => {
      listen<T>(event, (e) => cb(e.payload)).then((u) => unlisteners.push(u));
    };

    sub<{ text: string }>("selection-changed", ({ text }) => {
      cancelInFlight();
      setText(text);
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
      reqIdRef.current = await api.assistantRun(action, text, q);
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

      {error && <div className="popup-error">{error}</div>}
      {(result || streaming) && (
        <div className="popup-result">
          {result}
          {streaming && <span className="cursor" />}
        </div>
      )}

      <div className="popup-hint">Esc 关闭 · 点击其他区域自动隐藏</div>
    </div>
  );
}
