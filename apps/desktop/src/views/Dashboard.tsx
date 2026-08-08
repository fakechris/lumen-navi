import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as d3 from "d3";
import { api } from "../api";
import { Button, Card, EmptyState, IconButton, Input, Select, StatCard } from "../design";
import type { CategoryRule, MatchField, ProductivityLevel, ActivitySegment, DayStats } from "../types";
import { WeeklyView } from "./WeeklyView";

// --- helpers --------------------------------------------------------------

/** Format milliseconds as compact human duration: "6h 42m" / "12m 30s" / "45s". */
export function fmtDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const remS = s % 60;
  if (m < 60) return remS ? `${m}m ${remS}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const remM = m % 60;
  return remM ? `${h}h ${remM}m` : `${h}h`;
}

/** Format an RFC3339 timestamp to a local HH:MM label. */
function fmtClock(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "--:--";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

/** Today's local YYYY-MM-DD. */
function todayStr(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** Map a category name to one of the 8 design-system category palette tokens.
 *  Deterministic so the same category always gets the same color. */
const PALETTE_SIZE = 8;
const categoryColorIndex = (cat: string): number => {
  let h = 0;
  for (let i = 0; i < cat.length; i++) h = (h * 31 + cat.charCodeAt(i)) >>> 0;
  return h % PALETTE_SIZE + 1;
};
export const categoryColor = (cat: string): string => `var(--c-${categoryColorIndex(cat)})`;

/** Read a CSS variable's resolved value from the document root. */
export function readCssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

// --- main view ------------------------------------------------------------

export function DashboardView() {
  const [day] = useState(todayStr());
  const [view, setView] = useState<"today" | "week">("today");
  const [segments, setSegments] = useState<ActivitySegment[] | null>(null);
  const [stats, setStats] = useState<DayStats | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [segs, st] = await Promise.all([
        api.activitySegments(day),
        api.activityStats(day),
      ]);
      setSegments(segs);
      setStats(st);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [day]);

  useEffect(() => {
    void load();
    const t = setInterval(() => void load(), 30_000);
    return () => clearInterval(t);
  }, [load]);

  if (error) {
    return (
      <div className="stack">
        <Card pad={16}>
          <div style={{ color: "var(--danger, var(--text-secondary))" }}>{error}</div>
          <button onClick={() => void load()} style={{ marginTop: 8 }}>重试</button>
        </Card>
      </div>
    );
  }

  const loading = segments === null || stats === null;
  const hasData = !loading && (segments!.length > 0);

  return (
    <div className="stack">
      {/* Today / This week toggle */}
      <div className="row" style={{ gap: 0, alignSelf: "flex-start", borderRadius: "var(--radius-input)", overflow: "hidden" }}>
        <ViewTab active={view === "today"} onClick={() => setView("today")} first>今日</ViewTab>
        <ViewTab active={view === "week"} onClick={() => setView("week")}>本周</ViewTab>
      </div>

      {view === "week" && <WeeklyView />}

      {view === "today" && (
        <>
      {loading && (
        <Card pad={16}>
          <div style={{ color: "var(--text-tertiary)" }}>加载今日活动…</div>
        </Card>
      )}

      {hasData && (
        <>
          {/* Stat cards row */}
          <div className="grid">
            <StatCard
              label="活跃时长"
              value={fmtDuration(stats!.total_active_ms)}
              hint={`空闲 ${fmtDuration(stats!.total_idle_ms)}`}
            />
            <StatCard
              label="生产力分"
              value={stats!.pulse_score !== null ? Math.round(stats!.pulse_score).toString() : "—"}
              hint="0–100，未分类不计入"
              tone={stats!.pulse_score !== null && stats!.pulse_score >= 70 ? "success" : "default"}
            />
            <StatCard
              label="切换次数"
              value={String(stats!.context_switches)}
              hint="活跃片段数"
            />
            <StatCard
              label="Top 类别"
              value={stats!.by_category[0]?.category ?? "—"}
              hint={stats!.by_category[0] ? fmtDuration(stats!.by_category[0].ms) : undefined}
            />
          </div>

          {/* Category rules manager */}
          <CategoryRulesManager onRulesChanged={load} />

          {/* Retro-entry (manual segment) form */}
          <ManualSegmentForm day={day} onAdded={load} />

          {/* Timeline */}
          <Card pad={16}>
            <SectionHeader title="今日时间线" subtitle="按类别着色 · hover 查看详情 · 手动条目可删除" />
            <TimelineChart segments={segments!} onDeleted={load} />
          </Card>

          {/* Hour distribution */}
          <Card pad={16}>
            <SectionHeader title="小时分布" subtitle="一天中每个小时的活跃时长" />
            <HourDistribution stats={stats!} />
          </Card>

          {/* Top apps */}
          <Card pad={16}>
            <SectionHeader title="应用排行" subtitle="按活跃时长排序" />
            <TopApps stats={stats!} />
          </Card>
        </>
      )}

      {!loading && !hasData && (
        <EmptyState
          icon="clock"
          title="今天还没有活动数据"
        >
          启动观察后，这里会显示你一天的时间花在哪。
        </EmptyState>
      )}
        </>
      )}
    </div>
  );
}

function ViewTab({ active, onClick, children, first }: {
  active: boolean; onClick: () => void; children: React.ReactNode; first?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      style={{
        background: active ? "var(--surface)" : "transparent",
        border: "1px solid var(--border)",
        borderLeft: first ? "1px solid var(--border)" : "none",
        fontSize: "var(--text-xs)",
        fontWeight: active ? 600 : 400,
        color: active ? "var(--text)" : "var(--text-tertiary)",
        padding: "5px 14px",
        cursor: "pointer",
      }}
    >{children}</button>
  );
}

function SectionHeader({ title, subtitle }: { title: string; subtitle?: string }) {
  return (
    <div style={{ marginBottom: 12 }}>
      <div style={{
        fontSize: "var(--text-sm)",
        fontWeight: "var(--weight-semibold)",
        color: "var(--text)",
      }}>{title}</div>
      {subtitle && (
        <div style={{
          fontSize: "var(--text-xs)",
          color: "var(--text-tertiary)",
          marginTop: 2,
        }}>{subtitle}</div>
      )}
    </div>
  );
}

// --- Timeline chart (24h horizontal band, colored by category) ------------

function TimelineChart({ segments, onDeleted }: { segments: ActivitySegment[]; onDeleted?: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [tooltip, setTooltip] = useState<{
    x: number; y: number; seg: ActivitySegment;
  } | null>(null);
  const [deleting, setDeleting] = useState(false);

  // Active segments only (idle renders as faint gray).
  const active = useMemo(
    () => segments.filter((s) => !s.is_idle && s.duration_ms > 0),
    [segments],
  );

  useEffect(() => {
    const el = ref.current;
    if (!el || active.length === 0) return;

    const width = el.clientWidth;
    const height = 56;
    const svg = d3.select(el).append("svg").attr("width", width).attr("height", height);

    // 24-hour scale (local day). Segments store UTC; convert to local seconds-of-day.
    const dayStart = new Date();
    dayStart.setHours(0, 0, 0, 0);
    const dayStartMs = dayStart.getTime();
    const dayEndMs = dayStartMs + 24 * 3600 * 1000;
    const x = d3.scaleLinear().domain([dayStartMs, dayEndMs]).range([0, width]);

    // Background track
    const gridColor = readCssVar("--graph-grid") || "rgba(127,127,127,0.1)";
    svg.append("rect")
      .attr("x", 0).attr("y", height / 2 - 10)
      .attr("width", width).attr("height", 20)
      .attr("rx", 4)
      .attr("fill", gridColor);

    // Hour gridlines (every 6h)
    for (let h = 0; h <= 24; h += 6) {
      const px = x(dayStartMs + h * 3600 * 1000);
      svg.append("line")
        .attr("x1", px).attr("x2", px)
        .attr("y1", height / 2 - 14).attr("y2", height / 2 + 14)
        .attr("stroke", readCssVar("--border") || "rgba(127,127,127,0.2)")
        .attr("stroke-width", 1);
    }

    // Segments
    const barH = 20;
    const barY = height / 2 - barH / 2;
    const tooltipContainer = el;

    svg.selectAll("rect.seg")
      .data(active)
      .enter()
      .append("rect")
      .attr("class", "seg")
      .attr("x", (d) => x(new Date(d.started_at).getTime()))
      .attr("y", barY)
      .attr("width", (d) =>
        Math.max(1, x(new Date(d.ended_at ?? d.started_at).getTime()) - x(new Date(d.started_at).getTime()))
      )
      .attr("height", barH)
      .attr("rx", 3)
      .attr("fill", (d) => categoryColor(d.category ?? "Uncategorized"))
      .attr("opacity", 0.92)
      .style("cursor", "pointer")
      .on("mouseenter", function (event, d) {
        d3.select(this).attr("opacity", 1);
        const rect = tooltipContainer.getBoundingClientRect();
        setTooltip({
          x: event.clientX - rect.left,
          y: event.clientY - rect.top,
          seg: d as unknown as ActivitySegment,
        });
      })
      .on("mousemove", function (event, d) {
        const rect = tooltipContainer.getBoundingClientRect();
        setTooltip({
          x: event.clientX - rect.left,
          y: event.clientY - rect.top,
          seg: d as unknown as ActivitySegment,
        });
      })
      .on("mouseleave", function () {
        d3.select(this).attr("opacity", 0.92);
        setTooltip(null);
      });

    // Hour labels
    const labelColor = readCssVar("--text-tertiary") || "rgba(127,127,127,0.6)";
    svg.append("text")
      .attr("x", 2).attr("y", 12)
      .attr("fill", labelColor)
      .attr("font-size", 10)
      .attr("font-family", "var(--font-mono)")
      .text("00:00");
    svg.append("text")
      .attr("x", width / 2 - 16).attr("y", 12)
      .attr("fill", labelColor)
      .attr("font-size", 10)
      .attr("font-family", "var(--font-mono)")
      .text("12:00");
    svg.append("text")
      .attr("x", width - 32).attr("y", 12)
      .attr("fill", labelColor)
      .attr("font-size", 10)
      .attr("font-family", "var(--font-mono)")
      .text("24:00");

    return () => {
      svg.remove();
    };
  }, [active]);

  return (
    <div style={{ position: "relative" }}>
      <div ref={ref} />
      {tooltip && (
        <div style={{
          position: "absolute",
          left: tooltip.x + 12,
          top: tooltip.y - 8,
          transform: "translateY(-100%)",
          background: "var(--surface-elevated, var(--surface))",
          border: "1px solid var(--border)",
          borderRadius: "var(--radius-md)",
          padding: "8px 10px",
          fontSize: "var(--text-xs)",
          pointerEvents: tooltip.seg.source === "manual" ? "auto" : "none",
          whiteSpace: "nowrap",
          boxShadow: "0 4px 16px rgba(0,0,0,0.15)",
          zIndex: 10,
        }}>
          <div style={{ fontWeight: 600, marginBottom: 2, display: "flex", alignItems: "center", gap: 6 }}>
            {tooltip.seg.app_name ?? "未知"}
            {tooltip.seg.source === "manual" && (
              <span style={{
                fontSize: 9, padding: "1px 5px", borderRadius: "var(--radius-pill)",
                background: "var(--graph-grid)", color: "var(--text-tertiary)",
              }}>手动</span>
            )}
          </div>
          {tooltip.seg.window_title && (
            <div style={{ color: "var(--text-secondary)", marginBottom: 2, maxWidth: 280, overflow: "hidden", textOverflow: "ellipsis" }}>
              {tooltip.seg.window_title}
            </div>
          )}
          <div style={{ color: "var(--text-tertiary)", fontFamily: "var(--font-mono)" }}>
            {fmtClock(tooltip.seg.started_at)}–{tooltip.seg.ended_at ? fmtClock(tooltip.seg.ended_at) : "现在"} · {fmtDuration(tooltip.seg.duration_ms)}
          </div>
          {tooltip.seg.source === "manual" && onDeleted && (
            <button
              onClick={async () => {
                setDeleting(true);
                try {
                  await api.activityDeleteSegment(tooltip.seg.seg_id);
                  onDeleted();
                  setTooltip(null);
                } catch { /* ignore */ }
                setDeleting(false);
              }}
              disabled={deleting}
              style={{
                marginTop: 6, fontSize: "var(--text-xs)", color: "var(--danger, #e5484d)",
                background: "none", border: "none", cursor: "pointer", padding: 0,
              }}
            >{deleting ? "删除中…" : "删除此条目"}</button>
          )}
        </div>
      )}
    </div>
  );
}

// --- Hour distribution (24-bar stacked-by-productivity) ------------------

function HourDistribution({ stats }: { stats: DayStats }) {
  const ref = useRef<HTMLDivElement>(null);
  const data = stats.by_hour;

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const width = el.clientWidth;
    const height = 120;
    const margin = { top: 8, right: 4, bottom: 18, left: 4 };

    d3.select(el).selectAll("*").remove();
    const svg = d3.select(el).append("svg").attr("width", width).attr("height", height);

    const innerW = width - margin.left - margin.right;
    const innerH = height - margin.top - margin.bottom;
    const g = svg.append("g").attr("transform", `translate(${margin.left},${margin.top})`);

    const maxMs = Math.max(1, d3.max(data) ?? 0);
    const y = d3.scaleLinear().domain([0, maxMs]).range([innerH, 0]).nice();
    const barW = innerW / 24;
    const gap = 2;

    const gridColor = readCssVar("--graph-grid") || "rgba(127,127,127,0.1)";
    const accent = readCssVar("--c-1") || "#3b82f6";
    const labelColor = readCssVar("--text-tertiary") || "rgba(127,127,127,0.6)";

    g.selectAll("rect.bar")
      .data(data)
      .enter()
      .append("rect")
      .attr("class", "bar")
      .attr("x", (_d, i) => i * barW + gap / 2)
      .attr("y", (d) => y(d))
      .attr("width", barW - gap)
      .attr("height", (d) => innerH - y(d))
      .attr("rx", 2)
      .attr("fill", accent)
      .attr("opacity", 0.9);

    // Hour labels every 3h
    g.selectAll("text.h")
      .data(d3.range(0, 24, 3))
      .enter()
      .append("text")
      .attr("class", "h")
      .attr("x", (d) => d * barW + barW / 2)
      .attr("y", innerH + 14)
      .attr("text-anchor", "middle")
      .attr("fill", labelColor)
      .attr("font-size", 9)
      .attr("font-family", "var(--font-mono)")
      .text((d) => String(d).padStart(2, "0"));

    // Baseline
    g.append("line")
      .attr("x1", 0).attr("x2", innerW)
      .attr("y1", innerH).attr("y2", innerH)
      .attr("stroke", gridColor);

    return () => { svg.remove(); };
  }, [data]);

  return <div ref={ref} />;
}

// --- Top apps ranking (list with duration bars) --------------------------

function TopApps({ stats }: { stats: DayStats }) {
  const maxMs = Math.max(1, ...stats.top_apps.map((a) => a.ms));
  if (stats.top_apps.length === 0) {
    return <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-sm)" }}>暂无数据</div>;
  }
  return (
    <div className="stack">
      {stats.top_apps.map((app, i) => (
        <div key={`${app.app_name}-${i}`} style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{
            width: 8, height: 8, borderRadius: "50%",
            background: categoryColor(app.category ?? "Uncategorized"),
            flexShrink: 0,
          }} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: 8 }}>
              <span style={{
                fontSize: "var(--text-sm)",
                fontWeight: 500,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}>{app.app_name}</span>
              <span style={{
                fontSize: "var(--text-xs)",
                fontFamily: "var(--font-mono)",
                color: "var(--text-secondary)",
                flexShrink: 0,
              }}>{fmtDuration(app.ms)}</span>
            </div>
            <div style={{
              height: 4,
              borderRadius: 2,
              background: "var(--graph-grid)",
              marginTop: 4,
              overflow: "hidden",
            }}>
              <div style={{
                height: "100%",
                width: `${(app.ms / maxMs) * 100}%`,
                background: categoryColor(app.category ?? "Uncategorized"),
                borderRadius: 2,
              }} />
            </div>
            {app.category && (
              <div style={{
                fontSize: "var(--text-xs)",
                color: "var(--text-tertiary)",
                marginTop: 2,
              }}>{app.category}</div>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

// --- Category rules manager (collapsible) ---------------------------------

function CategoryRulesManager({ onRulesChanged }: { onRulesChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const [rules, setRules] = useState<CategoryRule[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  // new-rule form
  const [field, setField] = useState<MatchField>("app_name");
  const [value, setValue] = useState("");
  const [category, setCategory] = useState("");
  const [level, setLevel] = useState<ProductivityLevel | "">("productive");

  const loadRules = useCallback(async () => {
    setLoading(true);
    try {
      const r = await api.activityListCategoryRules();
      setRules(r);
    } catch { /* ignore */ }
    setLoading(false);
  }, []);

  useEffect(() => {
    if (open && rules.length === 0 && !loading) {
      void loadRules();
    }
  }, [open, rules.length, loading, loadRules]);

  const addRule = useCallback(async () => {
    if (!value.trim() || !category.trim()) return;
    const newRule: CategoryRule = {
      field,
      value: value.trim(),
      category: category.trim(),
      level: level || null,
    };
    setSaving(true);
    try {
      await api.activitySaveCategoryRules([...rules, newRule]);
      setValue("");
      setCategory("");
      await loadRules();
      onRulesChanged();
    } catch { /* ignore */ }
    setSaving(false);
  }, [field, value, category, level, rules, loadRules, onRulesChanged]);

  const removeRule = useCallback(async (idx: number) => {
    const next = rules.filter((_, i) => i !== idx);
    setSaving(true);
    try {
      await api.activitySaveCategoryRules(next);
      await loadRules();
      onRulesChanged();
    } catch { /* ignore */ }
    setSaving(false);
  }, [rules, loadRules, onRulesChanged]);

  return (
    <Card pad={16}>
      <button
        onClick={() => setOpen((v) => !v)}
        style={{
          background: "none",
          border: "none",
          cursor: "pointer",
          padding: 0,
          display: "flex",
          alignItems: "center",
          gap: 6,
          width: "100%",
        }}
      >
        <span style={{
          fontSize: "var(--text-sm)",
          fontWeight: "var(--weight-semibold)",
          color: "var(--text)",
        }}>分类规则</span>
        <span style={{ fontSize: "var(--text-xs)", color: "var(--text-tertiary)" }}>
          （{rules.length} 条自定义 · 覆盖内置默认表 · 保存后重算历史）
        </span>
        <span style={{ marginLeft: "auto", color: "var(--text-tertiary)", fontSize: 12 }}>
          {open ? "▾" : "▸"}
        </span>
      </button>

      {open && (
        <div className="stack" style={{ marginTop: 12 }}>
          {/* existing rules */}
          {loading && <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-xs)" }}>加载…</div>}
          {!loading && rules.length === 0 && (
            <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-xs)" }}>
              暂无自定义规则。常用 app 已内置分类（如 VSCode=Development）。
            </div>
          )}
          {rules.map((r, i) => (
            <div key={i} style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              fontSize: "var(--text-xs)",
              fontFamily: "var(--font-mono)",
            }}>
              <span style={{ color: "var(--text-tertiary)" }}>{r.field}:</span>
              <span style={{ color: "var(--text-secondary)" }}>{r.value}</span>
              <span style={{ color: "var(--text-tertiary)" }}>→</span>
              <span style={{ color: "var(--text)" }}>{r.category}</span>
              {r.level && (
                <span style={{
                  padding: "1px 6px",
                  borderRadius: "var(--radius-pill)",
                  background: r.level === "productive" ? "var(--c-3)" : r.level === "distracting" ? "var(--c-6)" : "var(--graph-grid)",
                  color: "var(--text)",
                  fontSize: 10,
                }}>{r.level}</span>
              )}
              <IconButton
                icon="close"
                size="sm"
                label="删除"
                onClick={() => void removeRule(i)}
                disabled={saving}
                style={{ marginLeft: "auto" }}
              />
            </div>
          ))}

          {/* add-rule form */}
          <div style={{
            display: "flex",
            gap: 6,
            flexWrap: "wrap",
            alignItems: "center",
            paddingTop: 6,
            borderTop: rules.length > 0 ? "1px solid var(--border)" : "none",
            marginTop: rules.length > 0 ? 4 : 0,
          }}>
            <Select
              value={field}
              onChange={(e) => setField(e.target.value as MatchField)}
              style={{ width: "auto", fontSize: "var(--text-xs)" }}
            >
              <option value="app_name">应用名</option>
              <option value="bundle_id">Bundle ID</option>
              <option value="domain">域名</option>
              <option value="title">标题包含</option>
              <option value="url">URL 包含</option>
            </Select>
            <Input
              type="text"
              placeholder="匹配值（如 Slack）"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              style={{ fontSize: "var(--text-xs)", width: 120 }}
            />
            <Input
              type="text"
              placeholder="类别（如 沟通）"
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              style={{ fontSize: "var(--text-xs)", width: 120 }}
            />
            <Select
              value={level}
              onChange={(e) => setLevel(e.target.value as ProductivityLevel | "")}
              style={{ width: "auto", fontSize: "var(--text-xs)" }}
            >
              <option value="productive">高效</option>
              <option value="neutral">中性</option>
              <option value="distracting">分心</option>
            </Select>
            <Button
              variant="primary"
              disabled={saving || !value.trim() || !category.trim()}
              onClick={() => void addRule()}
            >
              添加
            </Button>
          </div>
        </div>
      )}
    </Card>
  );
}

// --- Manual segment (retro-entry) form ------------------------------------

function ManualSegmentForm({ day, onAdded }: { day: string; onAdded: () => void }) {
  const [open, setOpen] = useState(false);
  const [appName, setAppName] = useState("");
  const [start, setStart] = useState("10:00");
  const [end, setEnd] = useState("10:30");
  const [category, setCategory] = useState("");
  const [level, setLevel] = useState<ProductivityLevel | "">("neutral");
  const [saving, setSaving] = useState(false);

  const submit = useCallback(async () => {
    if (!appName.trim() || !start || !end) return;
    // Build RFC3339 timestamps in local time for the given day.
    const toIso = (t: string) => {
      // local → ISO: construct as local, let Date serialize.
      const d = new Date(`${day}T${t}:00`);
      return d.toISOString();
    };
    const startedAt = toIso(start);
    const endedAt = toIso(end);
    if (new Date(endedAt) <= new Date(startedAt)) return;
    setSaving(true);
    try {
      await api.activityAddManualSegment({
        startedAt,
        endedAt,
        appName: appName.trim(),
        category: category.trim() || null,
        productivityLevel: level || null,
      });
      setAppName("");
      setCategory("");
      setOpen(false);
      onAdded();
    } catch { /* ignore */ }
    setSaving(false);
  }, [day, appName, start, end, category, level, onAdded]);

  return (
    <Card pad={16}>
      <button
        onClick={() => setOpen((v) => !v)}
        style={{
          background: "none", border: "none", cursor: "pointer", padding: 0,
          display: "flex", alignItems: "center", gap: 6, width: "100%",
        }}
      >
        <span style={{
          fontSize: "var(--text-sm)", fontWeight: "var(--weight-semibold)", color: "var(--text)",
        }}>＋ 补录活动</span>
        <span style={{ fontSize: "var(--text-xs)", color: "var(--text-tertiary)" }}>
          （手动添加一段时间，如"开会""阅读"）
        </span>
        <span style={{ marginLeft: "auto", color: "var(--text-tertiary)", fontSize: 12 }}>
          {open ? "▾" : "▸"}
        </span>
      </button>

      {open && (
        <div style={{ display: "flex", gap: 6, flexWrap: "wrap", alignItems: "center", marginTop: 12 }}>
          <Input
            type="text"
            placeholder="活动 / 应用（如 团队会议）"
            value={appName}
            onChange={(e) => setAppName(e.target.value)}
            style={{ fontSize: "var(--text-xs)", width: 160 }}
          />
          <label style={{ fontSize: "var(--text-xs)", color: "var(--text-tertiary)" }}>从
            <input
              type="time"
              value={start}
              onChange={(e) => setStart(e.target.value)}
              style={{
                marginLeft: 4, fontSize: "var(--text-xs)",
                background: "var(--surface)", border: "1px solid var(--border)",
                borderRadius: "var(--radius-input)", padding: "4px 6px", color: "var(--text)",
              }}
            />
          </label>
          <label style={{ fontSize: "var(--text-xs)", color: "var(--text-tertiary)" }}>到
            <input
              type="time"
              value={end}
              onChange={(e) => setEnd(e.target.value)}
              style={{
                marginLeft: 4, fontSize: "var(--text-xs)",
                background: "var(--surface)", border: "1px solid var(--border)",
                borderRadius: "var(--radius-input)", padding: "4px 6px", color: "var(--text)",
              }}
            />
          </label>
          <Input
            type="text"
            placeholder="类别（可选，如 沟通）"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            style={{ fontSize: "var(--text-xs)", width: 110 }}
          />
          <Select
            value={level}
            onChange={(e) => setLevel(e.target.value as ProductivityLevel | "")}
            style={{ width: "auto", fontSize: "var(--text-xs)" }}
          >
            <option value="productive">高效</option>
            <option value="neutral">中性</option>
            <option value="distracting">分心</option>
          </Select>
          <Button
            variant="primary"
            disabled={saving || !appName.trim()}
            onClick={() => void submit()}
          >添加</Button>
        </div>
      )}
    </Card>
  );
}
