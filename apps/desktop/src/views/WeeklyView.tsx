import { useCallback, useEffect, useRef, useState } from "react";
import * as d3 from "d3";
import { api } from "../api";
import { Card, EmptyState, StatCard } from "../design";
import type { RangeStats } from "../types";
import { fmtDuration, categoryColor, readCssVar } from "./Dashboard";

/** Shift a date by n days, return YYYY-MM-DD. */
function shiftDay(dayStr: string, n: number): string {
  const d = new Date(dayStr + "T00:00:00");
  d.setDate(d.getDate() + n);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${dd}`;
}

/** Short weekday label for a YYYY-MM-DD. */
function weekdayLabel(dayStr: string): string {
  const d = new Date(dayStr + "T00:00:00");
  if (Number.isNaN(d.getTime())) return "";
  const names = ["日", "一", "二", "三", "四", "五", "六"];
  return names[d.getDay()];
}

function monthDayLabel(dayStr: string): string {
  const d = new Date(dayStr + "T00:00:00");
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

export function WeeklyView() {
  // Anchor on the current calendar week, Monday through today.
  const [endDay, setEndDay] = useState(() => {
    const d = new Date();
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, "0");
    const dd = String(d.getDate()).padStart(2, "0");
    return `${y}-${m}-${dd}`;
  });
  const endDate = new Date(endDay + "T00:00:00");
  const weekday = endDate.getDay();
  const fromDay = shiftDay(endDay, weekday === 0 ? -6 : 1 - weekday);

  const [stats, setStats] = useState<RangeStats | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const r = await api.activityRange(fromDay, endDay);
      setStats(r);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [fromDay, endDay]);

  useEffect(() => {
    void load();
    const t = setInterval(() => void load(), 60_000);
    return () => clearInterval(t);
  }, [load]);

  const loading = stats === null;

  return (
    <div className="stack">
      {/* Week navigation */}
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div style={{ fontSize: "var(--text-sm)", color: "var(--text-secondary)" }}>
          {fromDay} ～ {endDay}
        </div>
        <div className="row" style={{ gap: 6 }}>
          <button
            onClick={() => setEndDay((d) => shiftDay(d, -7))}
            style={navBtnStyle}
          >‹ 上一周</button>
          <button
            onClick={() => setEndDay((d) => {
              const next = shiftDay(d, 7);
              const today = new Date();
              const ty = today.getFullYear();
              const tm = String(today.getMonth() + 1).padStart(2, "0");
              const td = String(today.getDate()).padStart(2, "0");
              const todayStr = `${ty}-${tm}-${td}`;
              return next > todayStr ? todayStr : next;
            })}
            style={navBtnStyle}
          >下一周 ›</button>
        </div>
      </div>

      {error && (
        <Card pad={16}><div style={{ color: "var(--text-secondary)" }}>{error}</div></Card>
      )}

      {loading && !error && (
        <Card pad={16}><div style={{ color: "var(--text-tertiary)" }}>加载…</div></Card>
      )}

      {stats && (
        <>
          {/* Range summary cards */}
          <div className="grid">
            <StatCard
              label="本周活跃"
              value={fmtDuration(stats.total_active_ms)}
              hint={`空闲 ${fmtDuration(stats.total_idle_ms)}`}
            />
            <StatCard
              label="周平均生产力"
              value={stats.pulse_score !== null ? Math.round(stats.pulse_score).toString() : "—"}
              hint="0–100"
              tone={stats.pulse_score !== null && stats.pulse_score >= 70 ? "success" : "default"}
            />
            <StatCard
              label="记录天数"
              value={String(stats.days.length)}
              hint="有活动的天数"
            />
            <StatCard
              label="Top 类别"
              value={stats.by_category[0]?.category ?? "—"}
              hint={stats.by_category[0] ? fmtDuration(stats.by_category[0].ms) : undefined}
            />
          </div>

          {/* Daily stacked-bar chart */}
          {stats.days.length > 0 ? (
            <Card pad={16}>
              <SectionHeader title="每日活动" subtitle="按类别堆叠 · 看哪天花在哪类事情上" />
              <DailyStackChart stats={stats} />
            </Card>
          ) : (
            <EmptyState icon="clock" title="这段时间没有活动数据">
              启动观察后，这里会显示每日的活动趋势。
            </EmptyState>
          )}

          {/* Pulse trend */}
          {stats.days.filter((d) => d.pulse_score !== null).length > 0 && (
            <Card pad={16}>
              <SectionHeader title="生产力分趋势" subtitle="每天的加权生产力分" />
              <PulseTrendChart stats={stats} />
            </Card>
          )}

          {/* Range top apps */}
          {stats.top_apps.length > 0 && (
            <Card pad={16}>
              <SectionHeader title="本周应用排行" subtitle="按活跃时长排序" />
              <TopAppsList stats={stats} />
            </Card>
          )}
        </>
      )}
    </div>
  );
}

const navBtnStyle: React.CSSProperties = {
  background: "var(--surface)",
  border: "1px solid var(--border)",
  borderRadius: "var(--radius-input)",
  padding: "4px 10px",
  fontSize: "var(--text-xs)",
  color: "var(--text-secondary)",
  cursor: "pointer",
};

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

// --- Daily stacked bar chart (category × day) ----------------------------

export function DailyStackChart({ stats }: { stats: RangeStats }) {
  const ref = useRef<HTMLDivElement>(null);
  const [tooltip, setTooltip] = useState<{
    x: number; y: number; day: string; cats: { category: string; ms: number }[]; total: number;
  } | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el || stats.days.length === 0) return;
    const width = el.clientWidth;
    const height = 200;
    const margin = { top: 12, right: 8, bottom: 28, left: 8 };

    d3.select(el).selectAll("*").remove();
    const svg = d3.select(el).append("svg").attr("width", width).attr("height", height);
    const innerW = width - margin.left - margin.right;
    const innerH = height - margin.top - margin.bottom;
    const g = svg.append("g").attr("transform", `translate(${margin.left},${margin.top})`);

    // Collect all categories across days (preserve first-seen order for legend).
    const catOrder: string[] = [];
    const catSet = new Set<string>();
    for (const d of stats.days) {
      for (const c of d.by_category) {
        if (!catSet.has(c.category)) {
          catSet.add(c.category);
          catOrder.push(c.category);
        }
      }
    }

    // Build stacked data per day.
    type DayStackRow = { day: string; byCat: Map<string, number> };
    const stack = d3.stack<string, DayStackRow, string>()
      .keys(catOrder)
      .value((d, key) => d.byCat.get(key) ?? 0)
      .order(d3.stackOrderNone)
      .offset(d3.stackOffsetNone);
    const rows: DayStackRow[] = stats.days.map((d) => ({
      day: d.day,
      byCat: new Map(d.by_category.map((c) => [c.category, c.ms])),
    }));
    const series = stack(rows);

    const x = d3.scaleBand()
      .domain(stats.days.map((d) => d.day))
      .range([0, innerW])
      .padding(0.3);
    const maxTotal = d3.max(stats.days, (d) => d.total_active_ms) ?? 1;
    const y = d3.scaleLinear().domain([0, Math.max(1, maxTotal)]).range([innerH, 0]).nice();

    const labelColor = readCssVar("--text-tertiary") || "rgba(127,127,127,0.6)";

    // Bars (stacked).
    const groups = g.selectAll("g.day")
      .data(series)
      .enter()
      .append("g")
      .attr("class", "day")
      .attr("fill", (s) => categoryColor(s.key));

    groups.selectAll("rect")
      .data((s) => s.map((seg) => ({ seg, key: s.key })))
      .enter()
      .append("rect")
      .attr("x", (d) => x(d_segDay(d.seg))!)
      .attr("y", (d) => y(d.seg[1]))
      .attr("width", x.bandwidth())
      .attr("height", (d) => Math.max(0, y(d.seg[0]) - y(d.seg[1])))
      .attr("rx", 2)
      .style("cursor", "pointer")
      .on("mouseenter", function (event, d) {
        const day = d_segDay(d.seg);
        const dayRow = stats.days.find((dr) => dr.day === day);
        if (!dayRow) return;
        const rect = el.getBoundingClientRect();
        setTooltip({
          x: event.clientX - rect.left,
          y: event.clientY - rect.top,
          day,
          cats: [...dayRow.by_category].sort((a, b) => b.ms - a.ms),
          total: dayRow.total_active_ms,
        });
      })
      .on("mousemove", function (event, d) {
        const day = d_segDay(d.seg);
        const dayRow = stats.days.find((dr) => dr.day === day);
        if (!dayRow) return;
        const rect = el.getBoundingClientRect();
        setTooltip({
          x: event.clientX - rect.left,
          y: event.clientY - rect.top,
          day,
          cats: [...dayRow.by_category].sort((a, b) => b.ms - a.ms),
          total: dayRow.total_active_ms,
        });
      })
      .on("mouseleave", function () {
        setTooltip(null);
      });

    // X-axis labels (weekday + month/day).
    g.selectAll("text.x")
      .data(stats.days)
      .enter()
      .append("text")
      .attr("class", "x")
      .attr("x", (d) => (x(d.day) ?? 0) + x.bandwidth() / 2)
      .attr("y", innerH + 14)
      .attr("text-anchor", "middle")
      .attr("fill", labelColor)
      .attr("font-size", 9)
      .attr("font-family", "var(--font-mono)")
      .text((d) => `${weekdayLabel(d.day)}`);

    g.selectAll("text.x2")
      .data(stats.days)
      .enter()
      .append("text")
      .attr("class", "x2")
      .attr("x", (d) => (x(d.day) ?? 0) + x.bandwidth() / 2)
      .attr("y", innerH + 24)
      .attr("text-anchor", "middle")
      .attr("fill", labelColor)
      .attr("font-size", 8)
      .attr("font-family", "var(--font-mono)")
      .text((d) => monthDayLabel(d.day));

    return () => { svg.remove(); };
  }, [stats]);

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
            {weekdayLabel(tooltip.day)} · 共 {fmtDuration(tooltip.total)}
          </div>
          {tooltip.cats.slice(0, 6).map((c) => (
            <div key={c.category} style={{ display: "flex", alignItems: "center", gap: 6, color: "var(--text-secondary)" }}>
              <span style={{ width: 7, height: 7, borderRadius: "50%", background: categoryColor(c.category), flexShrink: 0 }} />
              <span style={{ flex: 1 }}>{c.category}</span>
              <span style={{ fontFamily: "var(--font-mono)" }}>{fmtDuration(c.ms)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// d3 stack segment is [number, number] with a `.data` property carrying the row.
function d_segDay(seg: unknown): string {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (seg as any).data.day;
}

// --- Pulse trend line chart ----------------------------------------------

export function PulseTrendChart({ stats }: { stats: RangeStats }) {
  const ref = useRef<HTMLDivElement>(null);
  const pts = stats.days.filter((d) => d.pulse_score !== null) as {
    day: string; pulse_score: number;
  }[];

  useEffect(() => {
    const el = ref.current;
    if (!el || pts.length === 0) return;
    const width = el.clientWidth;
    const height = 120;
    const margin = { top: 10, right: 8, bottom: 22, left: 28 };

    d3.select(el).selectAll("*").remove();
    const svg = d3.select(el).append("svg").attr("width", width).attr("height", height);
    const innerW = width - margin.left - margin.right;
    const innerH = height - margin.top - margin.bottom;
    const g = svg.append("g").attr("transform", `translate(${margin.left},${margin.top})`);

    const x = d3.scalePoint()
      .domain(pts.map((p) => p.day))
      .range([0, innerW])
      .padding(0.5);
    const y = d3.scaleLinear().domain([0, 100]).range([innerH, 0]);

    const accent = readCssVar("--c-1") || "#3b82f6";
    const gridColor = readCssVar("--graph-grid") || "rgba(127,127,127,0.1)";
    const labelColor = readCssVar("--text-tertiary") || "rgba(127,127,127,0.6)";

    // Y gridlines at 0/50/100.
    for (const v of [0, 50, 100]) {
      g.append("line")
        .attr("x1", 0).attr("x2", innerW)
        .attr("y1", y(v)).attr("y2", y(v))
        .attr("stroke", gridColor);
      g.append("text")
        .attr("x", -6).attr("y", y(v) + 3)
        .attr("text-anchor", "end")
        .attr("fill", labelColor)
        .attr("font-size", 9)
        .attr("font-family", "var(--font-mono)")
        .text(String(v));
    }

    const line = d3.line<typeof pts[number]>()
      .x((p) => x(p.day)!)
      .y((p) => y(p.pulse_score))
      .curve(d3.curveMonotoneX);

    g.append("path")
      .datum(pts)
      .attr("fill", "none")
      .attr("stroke", accent)
      .attr("stroke-width", 2)
      .attr("d", line);

    g.selectAll("circle")
      .data(pts)
      .enter()
      .append("circle")
      .attr("cx", (p) => x(p.day)!)
      .attr("cy", (p) => y(p.pulse_score))
      .attr("r", 3)
      .attr("fill", accent);

    g.selectAll("text.x")
      .data(pts)
      .enter()
      .append("text")
      .attr("class", "x")
      .attr("x", (p) => x(p.day)!)
      .attr("y", innerH + 14)
      .attr("text-anchor", "middle")
      .attr("fill", labelColor)
      .attr("font-size", 8)
      .attr("font-family", "var(--font-mono)")
      .text((p) => monthDayLabel(p.day));

    return () => { svg.remove(); };
  }, [pts]);

  return <div ref={ref} />;
}

// --- Top apps list (range) -----------------------------------------------

function TopAppsList({ stats }: { stats: RangeStats }) {
  const maxMs = Math.max(1, ...stats.top_apps.map((a) => a.ms));
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
                fontSize: "var(--text-sm)", fontWeight: 500,
                overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
              }}>{app.app_name}</span>
              <span style={{
                fontSize: "var(--text-xs)", fontFamily: "var(--font-mono)",
                color: "var(--text-secondary)", flexShrink: 0,
              }}>{fmtDuration(app.ms)}</span>
            </div>
            <div style={{
              height: 4, borderRadius: 2, background: "var(--graph-grid)",
              marginTop: 4, overflow: "hidden",
            }}>
              <div style={{
                height: "100%", width: `${(app.ms / maxMs) * 100}%`,
                background: categoryColor(app.category ?? "Uncategorized"), borderRadius: 2,
              }} />
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}
