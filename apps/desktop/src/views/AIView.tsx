import { useCallback, useState } from "react";
import { api } from "../api";
import { Button, Card, StatusDot } from "../design";
import type { AssistantConfig } from "../types";

// ── LLM Status Card（不重复配置表单 — 设置页的“划词助手”就是同一份 config）──

function LlmStatusCard({ cfg }: { cfg: AssistantConfig | null }) {
  const configured =
    cfg != null && cfg.base_url.trim() !== "" && cfg.model.trim() !== "";
  const hasKey = cfg?.api_key_set ?? false;

  return (
    <Card pad={16}>
      <h3 style={{ margin: 0, marginBottom: 8 }}>LLM</h3>
      {cfg == null ? (
        <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-sm)" }}>
          加载中…
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, fontSize: "var(--text-sm)" }}>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <StatusDot status={configured ? "done" : "idle"} />
            <span>
              {configured ? "已配置" : "未配置"} — {cfg.base_url || "（空）"} · {cfg.model || "（空）"}
            </span>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <StatusDot status={hasKey ? "done" : "idle"} />
            <span>API Key {hasKey ? "已设置" : "未设置"}</span>
          </div>
          {!configured || !hasKey ? (
            <div style={{ color: "var(--warn, var(--text-secondary))", fontSize: "var(--text-xs)" }}>
              Roast 和 Chat 需要 LLM。请在 设置 → 划词助手 中配置（同一个 LLM 配置全局共用）。
            </div>
          ) : null}
        </div>
      )}
    </Card>
  );
}

// ── Roast Card ──────────────────────────────────────────────────────────

function RoastCard() {
  const now = new Date();
  const day = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  const [roast, setRoast] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async () => {
    setLoading(true);
    setError(null);
    setRoast(null);
    try {
      const text = await api.roastDay(day);
      setRoast(text);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [day]);

  return (
    <Card pad={16}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, marginBottom: 8 }}>
        <div>
          <h3 style={{ margin: 0 }}>Roast 我的一天 🔥</h3>
          <p style={{ color: "var(--text-tertiary)", fontSize: "var(--text-xs)", margin: "4px 0 0" }}>
            {day} · 把真实行为数据喂给 LLM 毒舌点评
          </p>
        </div>
        <Button variant="primary" disabled={loading} onClick={() => void run()}>
          {loading ? "生成中…" : "Roast 我"}
        </Button>
      </div>
      {error && (
        <div style={{ color: "var(--danger)", fontSize: "var(--text-sm)", whiteSpace: "pre-wrap" }}>{error}</div>
      )}
      {roast && (
        <div
          style={{
            whiteSpace: "pre-wrap",
            fontSize: "var(--text-sm)",
            lineHeight: 1.8,
            background: "var(--surface-2, var(--surface))",
            borderRadius: "var(--radius-md)",
            padding: "14px 16px",
          }}
        >
          {roast}
        </div>
      )}
    </Card>
  );
}

// ── Chat Card ───────────────────────────────────────────────────────────

interface ChatMsg {
  role: "user" | "assistant";
  content: string;
}

function ChatCard() {
  const [messages, setMessages] = useState<ChatMsg[]>([]);
  const [input, setInput] = useState("");
  const [loading, setLoading] = useState(false);

  const send = async () => {
    const text = input.trim();
    if (!text || loading) return;
    setInput("");
    const next: ChatMsg[] = [...messages, { role: "user", content: text }];
    setMessages(next);
    setLoading(true);
    try {
      const reply = await api.aiChat(
        next.map((m) => ({ role: m.role, content: m.content })),
      );
      setMessages((prev) => [...prev, { role: "assistant", content: reply }]);
    } catch (e) {
      setMessages((prev) => [...prev, { role: "assistant", content: `⚠️ ${String(e)}` }]);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Card pad={16}>
      <h3 style={{ margin: 0, marginBottom: 8 }}>AI Chat</h3>
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 8,
          minHeight: 120,
          maxHeight: 360,
          overflowY: "auto",
          padding: "4px 0",
        }}
      >
        {messages.length === 0 && (
          <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-sm)" }}>
            使用上方配置的 LLM 对话。你的行为数据不会被发送，除非你主动粘贴。
          </div>
        )}
        {messages.map((m, i) => (
          <div
            key={i}
            style={{
              alignSelf: m.role === "user" ? "flex-end" : "flex-start",
              maxWidth: "85%",
              padding: "8px 12px",
              borderRadius: "var(--radius-md)",
              fontSize: "var(--text-sm)",
              lineHeight: 1.6,
              whiteSpace: "pre-wrap",
              background:
                m.role === "user"
                  ? "var(--accent)"
                  : "var(--surface-2, var(--surface))",
              color: m.role === "user" ? "#fff" : "var(--text)",
            }}
          >
            {m.content}
          </div>
        ))}
        {loading && (
          <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-xs)", alignSelf: "flex-start" }}>
            思考中…
          </div>
        )}
      </div>
      <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
        <input
          className="input"
          style={{ flex: 1 }}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder="输入消息，Enter 发送"
        />
        <Button variant="primary" disabled={loading || !input.trim()} onClick={() => void send()}>
          发送
        </Button>
      </div>
    </Card>
  );
}

// ── Main ────────────────────────────────────────────────────────────────

export function AIView({ assistant }: { assistant: AssistantConfig | null }) {
  return (
    <div className="stack">
      <LlmStatusCard cfg={assistant} />
      <RoastCard />
      <ChatCard />
    </div>
  );
}
