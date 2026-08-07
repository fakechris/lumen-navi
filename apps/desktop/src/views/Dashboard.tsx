import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as d3 from "d3";
import { api } from "../api";
import { Card, EmptyState, StatCard } from "../design";
import type { ActivitySegment, DayStats } from "../types";

// --- helpers --------------------------------------------------------------

/** Format milliseconds as compact human duration: "6h 42m" / "12m 30s" / "45s". */
function fmtDuration(ms: number): string {
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
const categoryColor = (cat: string): string => `var(--c-${categoryColorIndex(cat)})`;

/** Read a CSS variable's resolved value from the document root. */
function readCssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

// --- main view ------------------------------------------------------------

export function DashboardView() {
  const [day] = useState(todayStr());
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

          {/* Timeline */}
          <Card pad={16}>
            <SectionHeader title="今日时间线" subtitle="按类别着色 · hover 查看详情" />
            <TimelineChart segments={segments!} />
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
    </div>
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

function TimelineChart({ segments }: { segments: ActivitySegment[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const [tooltip, setTooltip] = useState<{
    x: number; y: number; seg: ActivitySegment;
  } | null>(null);

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
          pointerEvents: "none",
          whiteSpace: "nowrap",
          boxShadow: "0 4px 16px rgba(0,0,0,0.15)",
          zIndex: 10,
        }}>
          <div style={{ fontWeight: 600, marginBottom: 2 }}>
            {tooltip.seg.app_name ?? "未知"}
          </div>
          {tooltip.seg.window_title && (
            <div style={{ color: "var(--text-secondary)", marginBottom: 2, maxWidth: 280, overflow: "hidden", textOverflow: "ellipsis" }}>
              {tooltip.seg.window_title}
            </div>
          )}
          <div style={{ color: "var(--text-tertiary)", fontFamily: "var(--font-mono)" }}>
            {fmtClock(tooltip.seg.started_at)}–{tooltip.seg.ended_at ? fmtClock(tooltip.seg.ended_at) : "现在"} · {fmtDuration(tooltip.seg.duration_ms)}
          </div>
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
