import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as d3 from "d3";
import { api } from "../api";
import { Button, Card, EmptyState, IconButton, Input, Select, StatCard } from "../design";
import type { CategoryRule, MatchField, ProductivityLevel, ActivitySegment, DayStats, RangeStats, SceneDay, HistorySlot } from "../types";
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

/** Format a YYYY-MM-DD for display, e.g. "8月9日 周六". */
function prettyDay(dayStr: string): string {
  const d = new Date(dayStr + "T00:00:00");
  const weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
  return `${d.getMonth() + 1}月${d.getDate()}日 ${weekdays[d.getDay()]}`;
}

/** Shift a YYYY-MM-DD by `delta` days (local). Returns YYYY-MM-DD. */
function shiftDay(dayStr: string, delta: number): string {
  const d = new Date(dayStr + "T00:00:00");
  d.setDate(d.getDate() + delta);
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

/**
 * Extract the registrable domain (e.g. "github.com") from a full URL for
 * compact display. Drops scheme, path, query, and leading www./m./mail.
 * Returns null for non-URL strings, about:blank, chrome://, etc.
 */
export function registrableDomain(url: string | null | undefined): string | null {
  if (!url) return null;
  let host: string | null = null;
  try {
    host = new URL(url).hostname;
  } catch {
    return null;
  }
  if (!host) return null;
  // Drop common leading prefixes (www, m, mail, mobile) when 3+ labels.
  const labels = host.split(".");
  if (labels.length >= 3 && /^(www|m|mail|mobile)$/i.test(labels[0])) {
    return labels.slice(1).join(".");
  }
  return host;
}

// --- main view ------------------------------------------------------------

export function DashboardView() {
  const [day, setDay] = useState(todayStr());
  const [view, setView] = useState<"today" | "week" | "last7" | "month" | "total">("today");
  const [segments, setSegments] = useState<ActivitySegment[] | null>(null);
  const [stats, setStats] = useState<DayStats | null>(null);
  const [rangeStats, setRangeStats] = useState<RangeStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Top-apps grouping: "app" (bundle identity, default) or "site" (domain).
  // Only affects the top-apps ranking, not the timeline/categories.
  const [groupBy, setGroupBy] = useState<"app" | "site" | "scene">("app");
  const [scenes, setScenes] = useState<SceneDay | null>(null);
  const [slots, setSlots] = useState<HistorySlot[] | null>(null);
  // Calendar popover open state for the day picker.
  const [calendarOpen, setCalendarOpen] = useState(false);

  const load = useCallback(async () => {
    try {
      if (view === "last7" || view === "month" || view === "total") {
        const today = todayStr();
        const from =
          view === "last7"
            ? shiftDay(today, -6)
            : view === "month"
              ? `${today.slice(0, 8)}01`
              : "1970-01-01";
        setRangeStats(null);
        setSegments([]);
        setStats(null);
        setRangeStats(await api.activityRange(from, today, groupBy === "scene" ? "app" : groupBy));
        setError(null);
        return;
      }
      setRangeStats(null);
      const [segs, st, sc, sl] = await Promise.all([
        api.activitySegments(day),
        api.activityStats(day, groupBy === "scene" ? "app" : groupBy),
        api.activityScenes(day),
        api.activityHistorySlots(day),
      ]);
      setSegments(segs);
      setStats(st);
      setScenes(sc);
      setSlots(sl);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [day, groupBy, view]);

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

  const rangeView = view === "last7" || view === "month" || view === "total";
  const loading = rangeView ? rangeStats === null : segments === null || stats === null;
  const hasData = !loading && (segments!.length > 0);

  return (
    <div className="stack">
      {/* Today / This week toggle */}
      <div className="row" style={{ gap: 0, alignSelf: "flex-start", borderRadius: "var(--radius-input)", overflow: "hidden" }}>
        <ViewTab active={view === "today"} onClick={() => setView("today")} first>今日</ViewTab>
        <ViewTab active={view === "week"} onClick={() => setView("week")}>本周</ViewTab>
        <ViewTab active={view === "last7"} onClick={() => setView("last7")}>最近 7 天</ViewTab>
        <ViewTab active={view === "month"} onClick={() => setView("month")}>本月</ViewTab>
        <ViewTab active={view === "total"} onClick={() => setView("total")}>全部</ViewTab>
      </div>

      {view === "week" && <WeeklyView />}

      {rangeView && (
        <RangeSummary
          label={view === "last7" ? "最近 7 天" : view === "month" ? "本月" : "全部累计"}
          stats={rangeStats}
          loading={loading}
        />
      )}

      {view === "today" && (
        <>
      {/* Day navigation: prev / label (click for calendar) / next / today */}
      <div className="row" style={{ alignItems: "center", gap: 8, position: "relative" }}>
        <button
          className="btn"
          onClick={() => setDay((d) => shiftDay(d, -1))}
          aria-label="前一天"
          style={{ padding: "4px 10px", minWidth: 0 }}
        >‹</button>
        <button
          className="btn"
          onClick={() => setCalendarOpen((v) => !v)}
          style={{ fontSize: "var(--text-sm)", fontWeight: "var(--weight-semibold)", minWidth: 110, textAlign: "center", padding: "4px 8px" }}
          aria-label="选择日期"
        >
          {prettyDay(day)} 📅
        </button>
        <button
          className="btn"
          onClick={() => setDay((d) => shiftDay(d, 1))}
          disabled={day >= todayStr()}
          aria-label="后一天"
          style={{ padding: "4px 10px", minWidth: 0 }}
        >›</button>
        {day !== todayStr() && (
          <button className="btn" onClick={() => setDay(todayStr())} style={{ padding: "4px 10px", fontSize: "var(--text-xs)" }}>
            今天
          </button>
        )}
        {calendarOpen && (
          <CalendarPicker
            selected={day}
            onPick={(d) => { setDay(d); setCalendarOpen(false); }}
            onClose={() => setCalendarOpen(false)}
          />
        )}
      </div>

      {!rangeView && loading && (
        <Card pad={16}>
          <div style={{ color: "var(--text-tertiary)" }}>加载活动…（{prettyDay(day)}）</div>
        </Card>
      )}

      {!rangeView && hasData && (
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
              value={
                stats!.by_category.find((c) => c.category && c.category !== "Uncategorized")?.category
                ?? "—"
              }
              hint={
                (stats!.by_category.find((c) => c.category && c.category !== "Uncategorized"))
                  ? fmtDuration(stats!.by_category.find((c) => c.category && c.category !== "Uncategorized")!.ms)
                  : undefined
              }
            />
          </div>

          {slots && slots.length > 0 && (
            <Card pad={16}>
              <SectionHeader
                title="今日回顾"
                subtitle="每 15 分钟一张 · 图标是这段出现过的 app · 按时长不是按次数"
              />
              <HistorySlotList slots={slots} />
            </Card>
          )}

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

          {/* Top apps / top sites */}
          <Card pad={16}>
            <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 12, marginBottom: 12 }}>
              <SectionHeader
                title={groupBy === "site" ? "网站排行" : groupBy === "scene" ? "场景排行" : "应用排行"}
                subtitle={
                  groupBy === "site"
                    ? "浏览器时长按域名聚合（Safari/Chrome/Comet）"
                    : groupBy === "scene"
                      ? "同一场景栈合并：Ghostty → herdr → writing，Safari → kimi.com"
                      : "按活跃时长排序"
                }
              />
              <div style={{ display: "flex", gap: 0, borderRadius: "var(--radius-md)", overflow: "hidden", border: "1px solid var(--border)", flex: "0 0 auto" }}>
                {(["app", "site", "scene"] as const).map((g) => (
                  <button
                    key={g}
                    onClick={() => setGroupBy(g)}
                    style={{
                      padding: "4px 10px",
                      fontSize: "var(--text-xs)",
                      cursor: "pointer",
                      background: groupBy === g ? "var(--accent)" : "transparent",
                      color: groupBy === g ? "#fff" : "var(--text-secondary)",
                      fontWeight: groupBy === g ? "var(--weight-semibold)" : "normal",
                      border: "none",
                      borderBottom: "none",
                    }}
                  >
                    {g === "app" ? "应用" : g === "site" ? "网站" : "场景"}
                  </button>
                ))}
              </div>
            </div>
            {groupBy === "scene" ? (
              <SceneRanking scenes={scenes} />
            ) : (
              <TopApps stats={stats!} groupBy={groupBy} />
            )}
          </Card>

          {groupBy !== "scene" && scenes && scenes.rollups.length > 0 && (
            <Card pad={16}>
              <SectionHeader
                title="场景"
                subtitle="同一栈合并时长：Ghostty → herdr → writing，Safari → kimi.com"
              />
              <SceneRanking scenes={scenes} />
            </Card>
          )}
        </>
      )}

      {!rangeView && !loading && !hasData && (
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

/**
 * Lightweight month calendar popover. Renders one month at a time with
 * prev/next month nav; clicking a day (not in the future) calls onPick.
 * Closes on outside click (a backdrop) or Escape.
 */
function CalendarPicker({
  selected,
  onPick,
  onClose,
}: {
  selected: string; // YYYY-MM-DD
  onPick: (day: string) => void;
  onClose: () => void;
}) {
  // The month being viewed, anchored on the selected day's month initially.
  const [viewYear, setViewYear] = useState(() => new Date(selected + "T00:00:00").getFullYear());
  const [viewMonth, setViewMonth] = useState(() => new Date(selected + "T00:00:00").getMonth());
  const today = todayStr();

  const firstWeekday = new Date(viewYear, viewMonth, 1).getDay(); // 0=Sun
  const daysInMonth = new Date(viewYear, viewMonth + 1, 0).getDate();
  const weekdayLabels = ["日", "一", "二", "三", "四", "五", "六"];
  const monthLabel = `${viewYear}年${viewMonth + 1}月`;

  const shiftMonth = (delta: number) => {
    const d = new Date(viewYear, viewMonth + delta, 1);
    setViewYear(d.getFullYear());
    setViewMonth(d.getMonth());
  };

  const cells: (number | null)[] = [];
  for (let i = 0; i < firstWeekday; i++) cells.push(null);
  for (let d = 1; d <= daysInMonth; d++) cells.push(d);

  const fmt = (d: number) =>
    `${viewYear}-${String(viewMonth + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`;

  return (
    <>
      {/* backdrop: click anywhere closes */}
      <div onClick={onClose} style={{ position: "fixed", inset: 0, zIndex: 50 }} />
      <div
        onKeyDown={(e) => { if (e.key === "Escape") onClose(); }}
        style={{
          position: "absolute", top: "100%", left: 0, marginTop: 4, zIndex: 51,
          background: "var(--surface-elevated, var(--surface))",
          border: "1px solid var(--border)",
          borderRadius: "var(--radius-md)",
          boxShadow: "0 8px 24px rgba(0,0,0,0.18)",
          padding: 12, width: 240,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 8 }}>
          <button className="btn" onClick={() => shiftMonth(-1)} style={{ padding: "2px 8px", minWidth: 0 }}>‹</button>
          <span style={{ fontSize: "var(--text-sm)", fontWeight: "var(--weight-semibold)" }}>{monthLabel}</span>
          <button className="btn" onClick={() => shiftMonth(1)} style={{ padding: "2px 8px", minWidth: 0 }}>›</button>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(7, 1fr)", gap: 2 }}>
          {weekdayLabels.map((w) => (
            <div key={w} style={{ textAlign: "center", fontSize: 10, color: "var(--text-tertiary)", padding: "2px 0" }}>{w}</div>
          ))}
          {cells.map((d, i) => {
            if (d === null) return <div key={`b${i}`} />;
            const ds = fmt(d);
            const isFuture = ds > today;
            const isSelected = ds === selected;
            return (
              <button
                key={ds}
                disabled={isFuture}
                onClick={() => onPick(ds)}
                style={{
                  padding: "5px 0", fontSize: "var(--text-xs)",
                  cursor: isFuture ? "not-allowed" : "pointer",
                  borderRadius: "var(--radius-sm)", border: "none",
                  background: isSelected ? "var(--accent)" : "transparent",
                  color: isSelected ? "#fff" : isFuture ? "var(--text-tertiary)" : "var(--text)",
                  fontWeight: isSelected ? "var(--weight-semibold)" : "normal",
                }}
              >{d}</button>
            );
          })}
        </div>
      </div>
    </>
  );
}

function RangeSummary({
  label,
  stats,
  loading,
}: {
  label: string;
  stats: RangeStats | null;
  loading: boolean;
}) {
  if (loading || !stats) {
    return (
      <Card pad={16}>
        <div style={{ color: "var(--text-tertiary)" }}>加载{label}统计…</div>
      </Card>
    );
  }

  const topCategory = stats.by_category.find(
    (category) => category.category && category.category !== "Uncategorized",
  );

  return (
    <div className="stack">
      <div className="meta">统计范围：{label} · 活跃时长按日期范围汇总，事件总数见概览页</div>
      <div className="grid">
        <StatCard
          label="活跃时长"
          value={fmtDuration(stats.total_active_ms)}
          hint={`空闲 ${fmtDuration(stats.total_idle_ms)}`}
        />
        <StatCard
          label="生产力分"
          value={stats.pulse_score !== null ? Math.round(stats.pulse_score).toString() : "—"}
          hint="0–100，未分类不计入"
          tone={stats.pulse_score !== null && stats.pulse_score >= 70 ? "success" : "default"}
        />
        <StatCard
          label="切换次数"
          value={String(stats.days.reduce((sum, day) => sum + day.context_switches, 0))}
          hint={`${stats.days.length} 个有活动的日期`}
        />
        <StatCard
          label="Top 类别"
          value={topCategory?.category ?? "—"}
          hint={topCategory ? fmtDuration(topCategory.ms) : undefined}
        />
      </div>

      {stats.top_apps.length > 0 && (
        <Card pad={16}>
          <SectionHeader title="应用排行" subtitle={`${label} · 按活跃时长排序`} />
          <TopApps stats={{
            day: label,
            total_active_ms: stats.total_active_ms,
            total_idle_ms: stats.total_idle_ms,
            pulse_score: stats.pulse_score,
            context_switches: 0,
            by_category: stats.by_category,
            top_apps: stats.top_apps,
            by_hour: [],
          }} groupBy="app" />
        </Card>
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

  // Split active vs idle: active segments get category colors and full
  // interactivity; idle segments render as muted gray blocks so the user can
  // see when they were away (e.g. walked off without sleeping the Mac).
  const active = useMemo(
    () => segments.filter((s) => !s.is_idle && s.duration_ms > 0),
    [segments],
  );
  const idleSegs = useMemo(
    () => segments.filter((s) => s.is_idle && s.duration_ms > 0),
    [segments],
  );

  useEffect(() => {
    const el = ref.current;
    if (!el || (active.length === 0 && idleSegs.length === 0)) return;

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

    // Idle segments: drawn first (under active) as muted gray so they read as
    // "away" without competing with category colors. Visible on the band so the
    // user can tell when they stepped away; hover shows a minimal "空闲" tooltip.
    const barH = 20;
    const barY = height / 2 - barH / 2;
    const tooltipContainer = el;
    const idleColor = readCssVar("--border-strong") || "rgba(127,127,127,0.35)";
    svg.selectAll("rect.idle")
      .data(idleSegs)
      .enter()
      .append("rect")
      .attr("class", "idle")
      .attr("x", (d) => x(new Date(d.started_at).getTime()))
      .attr("y", barY)
      .attr("width", (d) =>
        Math.max(1, x(new Date(d.ended_at ?? d.started_at).getTime()) - x(new Date(d.started_at).getTime()))
      )
      .attr("height", barH)
      .attr("rx", 3)
      .attr("fill", idleColor)
      .attr("opacity", 0.7)
      .style("cursor", "help")
      .on("mouseenter", function (event, d) {
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
        setTooltip(null);
      });

    // Hour gridlines (every 6h)
    for (let h = 0; h <= 24; h += 6) {
      const px = x(dayStartMs + h * 3600 * 1000);
      svg.append("line")
        .attr("x1", px).attr("x2", px)
        .attr("y1", height / 2 - 14).attr("y2", height / 2 + 14)
        .attr("stroke", readCssVar("--border") || "rgba(127,127,127,0.2)")
        .attr("stroke-width", 1);
    }

    // Segments (active)
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
  }, [active, idleSegs]);

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
            {tooltip.seg.is_idle ? "空闲 / 离开" : (tooltip.seg.app_name ?? "未知")}
            {tooltip.seg.source === "manual" && (
              <span style={{
                fontSize: 9, padding: "1px 5px", borderRadius: "var(--radius-pill)",
                background: "var(--graph-grid)", color: "var(--text-tertiary)",
              }}>手动</span>
            )}
          </div>
          {(() => {
            const domain = registrableDomain(tooltip.seg.url);
            return domain && !tooltip.seg.is_idle ? (
              <div style={{
                color: "var(--text-secondary)", marginBottom: 2, maxWidth: 280,
                overflow: "hidden", textOverflow: "ellipsis",
                fontFamily: "var(--font-mono)", fontSize: "var(--text-xs)",
              }}>
                {domain}
              </div>
            ) : null;
          })()}
          {tooltip.seg.scene_label && !tooltip.seg.is_idle && (
            <div style={{
              color: "var(--text-secondary)", marginBottom: 2, maxWidth: 280,
              overflow: "hidden", textOverflow: "ellipsis",
              fontFamily: "var(--font-mono)", fontSize: "var(--text-xs)",
            }}>
              {tooltip.seg.scene_label}
            </div>
          )}
          {tooltip.seg.window_title && !tooltip.seg.is_idle && (
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
  const [tooltip, setTooltip] = useState<{
    x: number; y: number; hour: number; cats: { category: string; ms: number }[]; total: number;
  } | null>(null);

  // Pre-bucket by_hour_category by hour for hover lookup.
  const byHourMap = useMemo(() => {
    const m = new Map<number, { category: string; ms: number }[]>();
    for (const h of stats.by_hour_category ?? []) {
      const arr = m.get(h.hour) ?? [];
      arr.push({ category: h.category, ms: h.ms });
      m.set(h.hour, arr);
    }
    // sort each hour's categories desc
    for (const arr of m.values()) arr.sort((a, b) => b.ms - a.ms);
    return m;
  }, [stats.by_hour_category]);

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
      .attr("opacity", 0.9)
      .style("cursor", "pointer")
      .on("mouseenter", function (event, d) {
        showTip(event, d);
      })
      .on("mousemove", function (event, d) {
        showTip(event, d);
      })
      .on("mouseleave", function () {
        setTooltip(null);
      });

    function showTip(event: { offsetX: number; clientX: number; clientY: number }, ms: number) {
      const rect = el!.getBoundingClientRect();
      // Which hour bar is under the cursor? barW = innerW / 24.
      const hour = Math.min(23, Math.max(0, Math.floor(event.offsetX / barW)));
      setTooltip({
        x: event.clientX - rect.left,
        y: event.clientY - rect.top,
        hour,
        cats: byHourMap.get(hour) ?? [],
        total: ms,
      });
    }

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
  }, [data, byHourMap]);

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
          pointerEvents: "none",
          whiteSpace: "nowrap",
          boxShadow: "0 4px 16px rgba(0,0,0,0.15)",
          zIndex: 10,
        }}>
          <div style={{ fontWeight: 600, marginBottom: 4 }}>
            {String(tooltip.hour).padStart(2, "0")}:00 · {fmtDuration(tooltip.total)}
          </div>
          {tooltip.cats.length === 0 ? (
            <div style={{ color: "var(--text-tertiary)" }}>无分类明细</div>
          ) : (
            tooltip.cats.slice(0, 6).map((c) => (
              <div key={c.category} style={{ display: "flex", alignItems: "center", gap: 6, color: "var(--text-secondary)" }}>
                <span style={{ width: 7, height: 7, borderRadius: "50%", background: categoryColor(c.category), flexShrink: 0 }} />
                <span style={{ flex: 1 }}>{c.category}</span>
                <span style={{ fontFamily: "var(--font-mono)" }}>{fmtDuration(c.ms)}</span>
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}

function HistorySlotList({ slots }: { slots: HistorySlot[] }) {
  const [icons, setIcons] = useState<Record<string, string>>({});
  useEffect(() => {
    const ids = [
      ...new Set(
        slots.flatMap((s) =>
          s.apps.map((a) => a.bundle_id).filter((id): id is string => !!id),
        ),
      ),
    ];
    if (ids.length === 0) return;
    void api.appIcons(ids).then(setIcons).catch(() => {});
  }, [slots]);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
      {slots.map((slot) => (
        <div
          key={slot.slot_start}
          style={{
            paddingBottom: 14,
            borderBottom: "1px solid var(--border)",
          }}
        >
          <div
            style={{
              fontSize: "var(--text-xs)",
              color: "var(--text-tertiary)",
              fontFamily: "var(--font-mono)",
              marginBottom: 4,
            }}
          >
            {fmtClock(slot.slot_start)}
          </div>
          <div
            style={{
              fontSize: "var(--text-sm)",
              fontWeight: "var(--weight-semibold)",
              marginBottom: 4,
            }}
          >
            {slot.title}
          </div>
          {slot.body && (
            <div
              style={{
                fontSize: "var(--text-sm)",
                color: "var(--text-secondary)",
                lineHeight: 1.45,
                marginBottom: 8,
              }}
            >
              {slot.body}
            </div>
          )}
          <div style={{ display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center" }}>
            {slot.apps.map((app) => {
              const src = app.bundle_id ? icons[app.bundle_id] : undefined;
              const letter = (app.app_name || "?").slice(0, 1);
              return (
                <span
                  key={`${app.bundle_id ?? app.app_name}`}
                  style={{
                    display: "inline-flex",
                    alignItems: "center",
                    gap: 5,
                    fontSize: "var(--text-xs)",
                    padding: "2px 8px 2px 4px",
                    borderRadius: 999,
                    border: "1px solid var(--border)",
                    color: "var(--text-secondary)",
                    background: "var(--surface-elevated, var(--surface))",
                  }}
                  title={`${app.app_name} · ${fmtDuration(app.ms)}`}
                >
                  {src ? (
                    <img
                      src={src}
                      width={16}
                      height={16}
                      alt=""
                      style={{ borderRadius: 4, display: "block" }}
                    />
                  ) : (
                    <span
                      style={{
                        width: 16,
                        height: 16,
                        borderRadius: 4,
                        display: "inline-flex",
                        alignItems: "center",
                        justifyContent: "center",
                        fontSize: 10,
                        fontWeight: 600,
                        background: "var(--border)",
                        color: "var(--text)",
                      }}
                    >
                      {letter}
                    </span>
                  )}
                  {app.app_name}
                </span>
              );
            })}
            {(slot.suggested_skills ?? []).map((sk) => (
              <button
                key={sk.name}
                type="button"
                className="skill-chip"
                title="点击回放一次（先聚焦窗口再键鼠）。Shift+点击只复制草稿。"
                onClick={(e) => {
                  if (e.shiftKey) {
                    void navigator.clipboard.writeText(formatCuaSkill(sk));
                    return;
                  }
                  if (
                    !window.confirm(
                      `按 ${sk.steps?.length ?? 0} 步回放「${sk.name}」？\n会激活对应窗口并发送键鼠。`,
                    )
                  ) {
                    return;
                  }
                  void api
                    .replayHistorySkill(slot.slot_start)
                    .then((msg) => window.alert(msg))
                    .catch((err: unknown) =>
                      window.alert(String(err ?? "回放失败")),
                    );
                }}
              >
                CUA 回放 · {sk.name}
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function formatCuaSkill(sk: {
  name: string;
  trigger: string;
  prompt: string;
  verify?: string;
  steps?: Array<{
    action: string;
    app: string;
    window?: string | null;
    target?: string | null;
    keys?: string | null;
    rel_x?: number | null;
    rel_y?: number | null;
    note?: string | null;
  }>;
}): string {
  const lines = [`# ${sk.name}`, "", `何时再用：${sk.trigger}`, ""];
  if (sk.verify) {
    lines.push(`验收：${sk.verify}`, "");
  }
  lines.push("CUA 步骤：");
  const steps = sk.steps ?? [];
  if (steps.length === 0) {
    lines.push("- （无步骤）");
  } else {
    steps.forEach((st, i) => {
      const win = st.window ? ` 「${st.window}」` : "";
      const tgt = st.target ? ` → ${st.target}` : "";
      const keys = st.keys ? ` [${st.keys}]` : "";
      const rel =
        st.rel_x != null && st.rel_y != null
          ? ` @${st.rel_x.toFixed(2)},${st.rel_y.toFixed(2)}`
          : "";
      const note = st.note ? ` (${st.note})` : "";
      lines.push(`${i + 1}. ${st.action} ${st.app}${win}${tgt}${rel}${keys}${note}`);
    });
  }
  lines.push("", sk.prompt, "");
  return lines.join("\n");
}

function SceneRanking({ scenes }: { scenes: SceneDay | null }) {
  const rollups = scenes?.rollups ?? [];
  const maxMs = Math.max(1, ...rollups.map((r) => r.ms));
  if (rollups.length === 0) {
    return <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-sm)" }}>暂无场景数据</div>;
  }
  const kindLabel: Record<string, string> = {
    browser: "浏览",
    development: "开发",
    communication: "通讯",
    other: "其他",
  };
  return (
    <div className="stack">
      {rollups.map((r) => (
        <div key={r.label} style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{
            width: 8, height: 8, borderRadius: "50%",
            background: categoryColor(r.kind),
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
                fontFamily: "var(--font-mono)",
              }}>{r.label}</span>
              <span style={{
                fontSize: "var(--text-xs)",
                fontFamily: "var(--font-mono)",
                color: "var(--text-secondary)",
                flexShrink: 0,
              }}>{fmtDuration(r.ms)}</span>
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
                width: `${(r.ms / maxMs) * 100}%`,
                background: categoryColor(r.kind),
                borderRadius: 2,
              }} />
            </div>
            <div style={{
              fontSize: "var(--text-xs)",
              color: "var(--text-tertiary)",
              marginTop: 2,
            }}>{kindLabel[r.kind] ?? r.kind} · {r.episode_count} 段</div>
          </div>
        </div>
      ))}
    </div>
  );
}

// --- Top apps ranking (list with duration bars) --------------------------

function TopApps({ stats, groupBy = "app" }: { stats: DayStats; groupBy?: "app" | "site" }) {
  const maxMs = Math.max(1, ...stats.top_apps.map((a) => a.ms));
  if (stats.top_apps.length === 0) {
    return <div style={{ color: "var(--text-tertiary)", fontSize: "var(--text-sm)" }}>暂无数据</div>;
  }
  return (
    <div className="stack">
      {stats.top_apps.map((app, i) => {
        // Site mode: prefer the representative page title as the primary label,
        // show the domain as a secondary line (mirrors ActivityWatch's
        // title-first grouping). App mode: just the app name.
        const useTitle = groupBy === "site" && app.title && app.title.trim();
        const primary = useTitle ? app.title! : app.app_name;
        const secondary = useTitle ? app.app_name : null;
        return (
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
              }}>{primary}</span>
              <span style={{
                fontSize: "var(--text-xs)",
                fontFamily: "var(--font-mono)",
                color: "var(--text-secondary)",
                flexShrink: 0,
              }}>{fmtDuration(app.ms)}</span>
            </div>
            {secondary && (
              <div style={{
                fontSize: "var(--text-xs)",
                color: "var(--text-tertiary)",
                marginTop: 1,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
                fontFamily: "var(--font-mono)",
              }}>{secondary}</div>
            )}
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
        );
      })}
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
