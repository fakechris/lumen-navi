import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { Markdown } from "../components/Markdown";
import { Button, Card, StatusDot } from "../design";
import { getProvider } from "../llm/catalog";
import type {
  AiMessage,
  AiThread,
  AssistantConfig,
  RoastIndexEntry,
  RoastRecord,
} from "../types";

// ── helpers ─────────────────────────────────────────────────────────────

function localDay(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate(),
  ).padStart(2, "0")}`;
}

function fmtTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// Some providers leave the CoT inline as <think>…</think>. The backend splits
// new completions; this also fixes records saved before that existed.
const THINK_RE = /<(think|thinking)>([\s\S]*?)<\/\1>/gi;

function splitInlineThink(text: string): { content: string; reasoning: string | null } {
  const thoughts: string[] = [];
  const content = text.replace(THINK_RE, (_m, _tag: string, inner: string) => {
    const t = inner.trim();
    if (t) thoughts.push(t);
    return "";
  });
  return {
    content: content.trim(),
    reasoning: thoughts.length ? thoughts.join("\n\n") : null,
  };
}

/** A reply body: collapsed CoT block + markdown answer. */
function ReplyBody({ content, reasoning }: { content: string; reasoning?: string | null }) {
  const split = splitInlineThink(content);
  const cot =
    [reasoning?.trim() || null, split.reasoning]
      .filter((x): x is string => x != null && x !== "")
      .join("\n\n") || null;
  return (
    <>
      {cot && (
        <details className="cot">
          <summary>思考过程</summary>
          <Markdown text={cot} />
        </details>
      )}
      <Markdown text={split.content} />
    </>
  );
}

// ── LLM status chip（正常时一行小标签；有问题时提示并跳转设置）─────────

function LlmChip({
  cfg,
  onOpenSettings,
}: {
  cfg: AssistantConfig | null;
  onOpenSettings: () => void;
}) {
  if (cfg == null) return null;
  const pid = cfg.provider_id || "custom";
  const providerLabel = pid === "custom" ? cfg.base_url || "自定义（未填 URL）" : getProvider(pid)?.label ?? pid;
  const configured = cfg.model.trim() !== "" && providerLabel !== "";
  const problem = !configured || !cfg.api_key_set;

  return (
    <button
      onClick={onOpenSettings}
      title={problem ? "LLM 配置不完整 — 点击前往设置" : "LLM 配置 — 点击前往设置"}
      style={{
        alignSelf: "flex-start",
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        border: "1px solid var(--border)",
        borderRadius: 999,
        background: "transparent",
        padding: "3px 10px",
        fontSize: "var(--text-xs)",
        color: problem ? "var(--warn, var(--text-secondary))" : "var(--text-tertiary)",
        cursor: "pointer",
      }}
    >
      <StatusDot status={problem ? "idle" : "done"} />
      {problem ? "LLM 未配置完整 · 点击前往设置" : `${providerLabel} · ${cfg.model}`}
    </button>
  );
}

// ── Mini month calendar ─────────────────────────────────────────────────

/** Month grid with a dot on days that have an archived roast. */
function MiniCalendar({
  month,
  selectedDay,
  roastDays,
  onPick,
  onMonthDelta,
}: {
  /** "YYYY-MM" being displayed. */
  month: string;
  selectedDay: string;
  /** day ("YYYY-MM-DD") → roast count. */
  roastDays: Map<string, number>;
  onPick: (day: string) => void;
  onMonthDelta: (delta: number) => void;
}) {
  const grid = useMemo(() => {
    const [y, m] = month.split("-").map(Number);
    const startDow = new Date(y, m - 1, 1).getDay();
    const daysInMonth = new Date(y, m, 0).getDate();
    const cells: (string | null)[] = Array.from({ length: startDow }, () => null);
    for (let d = 1; d <= daysInMonth; d++) {
      cells.push(`${month}-${String(d).padStart(2, "0")}`);
    }
    return cells;
  }, [month]);

  const today = localDay(new Date());
  const [y, m] = month.split("-").map(Number);
  const monthLabel = `${y} 年 ${m} 月`;

  return (
    <div style={{ userSelect: "none" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 6 }}>
        <button className="btn" style={{ padding: "2px 10px" }} onClick={() => onMonthDelta(-1)}>
          ‹
        </button>
        <span style={{ fontSize: "var(--text-sm)", fontWeight: 600 }}>{monthLabel}</span>
        <button className="btn" style={{ padding: "2px 10px" }} onClick={() => onMonthDelta(1)}>
          ›
        </button>
      </div>
      <div className="cal">
        {["日", "一", "二", "三", "四", "五", "六"].map((d) => (
          <div key={d} className="cal-dow">{d}</div>
        ))}
        {grid.map((day, i) =>
          day == null ? (
            <div key={`b${i}`} />
          ) : (
            <button
              key={day}
              className={`cal-day${day === selectedDay ? " sel" : ""}${day === today ? " today" : ""}`}
              onClick={() => onPick(day)}
              title={roastDays.has(day) ? `${roastDays.get(day)} 条 roast 存档` : undefined}
            >
              {Number(day.slice(8))}
              {roastDays.has(day) && <span className="cal-dot" />}
            </button>
          ),
        )}
      </div>
    </div>
  );
}

// ── Roast Card ──────────────────────────────────────────────────────────

function RoastCard() {
  const today = localDay(new Date());
  const [day, setDay] = useState(today);
  const [viewMonth, setViewMonth] = useState(today.slice(0, 7));
  const [tone, setTone] = useState<"roast" | "advisor">(() =>
    localStorage.getItem("roast.tone") === "advisor" ? "advisor" : "roast",
  );
  const [roastDays, setRoastDays] = useState<Map<string, number>>(new Map());
  const [records, setRecords] = useState<RoastRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pickTone = (t: "roast" | "advisor") => {
    setTone(t);
    localStorage.setItem("roast.tone", t);
  };

  const refreshIndex = useCallback(() => {
    void api
      .roastIndex()
      .then((ix: RoastIndexEntry[]) =>
        setRoastDays(new Map(ix.map((e) => [e.day, e.count]))),
      )
      .catch(() => {});
  }, []);

  const loadDay = useCallback(async (d: string) => {
    try {
      const list = await api.roastList(d);
      setRecords(list);
      setSelectedId(list[0]?.id ?? null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void loadDay(day);
  }, [day, loadDay]);

  useEffect(() => {
    refreshIndex();
  }, [refreshIndex]);

  const run = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      await api.roastDay(day, tone);
      await loadDay(day);
      refreshIndex();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [day, tone, loadDay, refreshIndex]);

  const selected = records.find((r) => r.id === selectedId) ?? records[0] ?? null;

  const shiftMonth = (delta: number) => {
    const [y, m] = viewMonth.split("-").map(Number);
    const d = new Date(y, m - 1 + delta, 1);
    setViewMonth(localDay(d).slice(0, 7));
  };

  return (
    <Card pad={16}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, marginBottom: 8 }}>
        <div>
          <h3 style={{ margin: 0 }}>Roast 我的一天 🔥</h3>
          <p style={{ color: "var(--text-tertiary)", fontSize: "var(--text-xs)", margin: "4px 0 0" }}>
            {day} · 把真实行为数据喂给 LLM 点评 · 存档自动保留
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <div className="seg" style={{ display: "inline-flex", border: "1px solid var(--border)", borderRadius: 999, overflow: "hidden" }}>
            {(
              [
                ["roast", "🔥 吐槽"],
                ["advisor", "🌱 真诚建议"],
              ] as const
            ).map(([v, label]) => (
              <button
                key={v}
                onClick={() => pickTone(v)}
                style={{
                  border: "none",
                  padding: "5px 12px",
                  fontSize: "var(--text-xs)",
                  cursor: "pointer",
                  background: tone === v ? "var(--accent)" : "transparent",
                  color: tone === v ? "#fff" : "var(--text-secondary)",
                }}
              >
                {label}
              </button>
            ))}
          </div>
          <Button variant="primary" disabled={loading} onClick={() => void run()}>
            {loading ? "生成中…" : day === today ? "开始" : `Roast ${day}`}
          </Button>
        </div>
      </div>

      <div style={{ display: "flex", gap: 16, alignItems: "flex-start", flexWrap: "wrap" }}>
        <MiniCalendar
          month={viewMonth}
          selectedDay={day}
          roastDays={roastDays}
          onPick={setDay}
          onMonthDelta={shiftMonth}
        />
        <div style={{ flex: 1, minWidth: 260 }}>
          {error && (
            <div style={{ color: "var(--danger)", fontSize: "var(--text-sm)", whiteSpace: "pre-wrap", marginBottom: 8 }}>
              {error}
            </div>
          )}
          {records.length > 1 && (
            <div style={{ marginBottom: 8 }}>
              <select
                className="input"
                value={selected?.id ?? ""}
                onChange={(e) => setSelectedId(e.target.value)}
              >
                {records.map((r, i) => (
                  <option key={r.id} value={r.id}>
                    第 {records.length - i} 次 · {fmtTime(r.created_at)} · {r.model}
                  </option>
                ))}
              </select>
            </div>
          )}
          {selected ? (
            <>
              <p style={{ color: "var(--text-tertiary)", fontSize: "var(--text-xs)", margin: "0 0 6px" }}>
                {fmtTime(selected.created_at)} · {selected.model}
              </p>
              <div
                style={{
                  fontSize: "var(--text-sm)",
                  lineHeight: 1.8,
                  background: "var(--surface-2, var(--surface))",
                  borderRadius: "var(--radius-md)",
                  padding: "14px 16px",
                }}
              >
                <ReplyBody content={selected.content} reasoning={selected.reasoning} />
              </div>
            </>
          ) : (
            !loading && (
              <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-sm)" }}>
                {day} 还没有 roast 存档。点击右上角生成一次，之后可在日历上随时回顾（有圆点标记的日期）。
              </div>
            )
          )}
        </div>
      </div>
    </Card>
  );
}

// ── Chat Card ───────────────────────────────────────────────────────────

interface ChatMsg {
  role: "user" | "assistant";
  content: string;
  reasoning?: string | null;
}

function fromStored(m: AiMessage): ChatMsg {
  return { role: m.role === "user" ? "user" : "assistant", content: m.content, reasoning: m.reasoning };
}

function ChatCard() {
  const [threads, setThreads] = useState<AiThread[]>([]);
  const [threadId, setThreadId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMsg[]>([]);
  const [input, setInput] = useState("");
  const [loading, setLoading] = useState(false);

  // Resume the most recent conversation on mount.
  useEffect(() => {
    void (async () => {
      try {
        const list = await api.aiThreadList();
        setThreads(list);
        if (list.length > 0) {
          setThreadId(list[0].id);
          const msgs = await api.aiThreadMessages(list[0].id);
          setMessages(msgs.map(fromStored));
        }
      } catch {
        // history load failure shouldn't block a fresh chat
      }
    })();
  }, []);

  const openThread = useCallback(async (id: string) => {
    setThreadId(id);
    try {
      const msgs = await api.aiThreadMessages(id);
      setMessages(msgs.map(fromStored));
    } catch {
      setMessages([]);
    }
  }, []);

  const newChat = useCallback(() => {
    setThreadId(null);
    setMessages([]);
    setInput("");
  }, []);

  const deleteThread = useCallback(async () => {
    if (!threadId) return;
    const target = threadId;
    try {
      await api.aiThreadDelete(target);
    } catch {
      // already gone — refresh anyway
    }
    const list = await api.aiThreadList().catch(() => []);
    setThreads(list);
    if (list.length > 0) {
      await openThread(list[0].id);
    } else {
      newChat();
    }
  }, [threadId, openThread, newChat]);

  const send = async () => {
    const text = input.trim();
    if (!text || loading) return;
    setInput("");
    setMessages((prev) => [...prev, { role: "user", content: text }]);
    setLoading(true);
    try {
      const r = await api.aiSend(threadId, text);
      setThreadId(r.thread_id);
      setMessages((prev) => [
        ...prev,
        { role: "assistant", content: r.content, reasoning: r.reasoning },
      ]);
      // Refresh thread list (title/order) without disturbing the open thread.
      void api
        .aiThreadList()
        .then(setThreads)
        .catch(() => {});
    } catch (e) {
      setMessages((prev) => [...prev, { role: "assistant", content: `⚠️ ${String(e)}` }]);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Card pad={16}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8, flexWrap: "wrap" }}>
        <h3 style={{ margin: 0, marginRight: "auto" }}>AI Chat</h3>
        <select
          className="input"
          style={{ maxWidth: 220 }}
          value={threadId ?? ""}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "") newChat();
            else void openThread(v);
          }}
        >
          <option value="">＋ 新对话</option>
          {threads.map((t) => (
            <option key={t.id} value={t.id}>
              {t.title || "（无标题）"}（{t.message_count} 条）
            </option>
          ))}
        </select>
        <Button variant="secondary" onClick={newChat}>
          新对话
        </Button>
        <Button variant="secondary" disabled={!threadId} onClick={() => void deleteThread()}>
          删除
        </Button>
      </div>
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
            使用上方配置的 LLM 对话，历史自动保存。你的行为数据不会被发送，除非你主动粘贴。
          </div>
        )}
        {messages.map((m, i) =>
          m.role === "user" ? (
            <div
              key={i}
              style={{
                alignSelf: "flex-end",
                maxWidth: "85%",
                padding: "8px 12px",
                borderRadius: "var(--radius-md)",
                fontSize: "var(--text-sm)",
                lineHeight: 1.6,
                whiteSpace: "pre-wrap",
                background: "var(--accent)",
                color: "#fff",
              }}
            >
              {m.content}
            </div>
          ) : (
            <div
              key={i}
              style={{
                alignSelf: "flex-start",
                maxWidth: "85%",
                padding: "8px 12px",
                borderRadius: "var(--radius-md)",
                fontSize: "var(--text-sm)",
                lineHeight: 1.6,
                background: "var(--surface-2, var(--surface))",
                color: "var(--text)",
              }}
            >
              <ReplyBody content={m.content} reasoning={m.reasoning} />
            </div>
          ),
        )}
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

export function AIView({
  assistant,
  onOpenSettings,
}: {
  assistant: AssistantConfig | null;
  onOpenSettings: () => void;
}) {
  return (
    <div className="stack">
      <LlmChip cfg={assistant} onOpenSettings={onOpenSettings} />
      <RoastCard />
      <ChatCard />
    </div>
  );
}
