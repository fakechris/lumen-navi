import React from "react";
import { Card } from "./Card.jsx";

/**
 * Compact metric tile — a big tabular-nums figure with a label above and
 * optional delta/hint below. Used across the "Today" dashboards
 * (came in / read / crystallized / attention).
 */
export function StatCard({ label, value, hint, tone = "default", onClick }) {
  const valueColor = tone === "default" ? "var(--text)" : `var(--${tone})`;
  const valueLength = typeof value === "string" ? value.length : 0;
  const valueFontSize =
    valueLength > 18
      ? "var(--text-lg)"
      : valueLength > 10
        ? "var(--text-xl)"
        : "var(--text-3xl)";
  return (
    <Card interactive={!!onClick} pad={16} onClick={onClick} style={{ minWidth: 120 }}>
      <div style={{ fontSize: "var(--text-xs)", fontWeight: "var(--weight-semibold)", letterSpacing: "var(--tracking-wide)", textTransform: "uppercase", color: "var(--text-tertiary)" }}>{label}</div>
      <div style={{ fontSize: valueFontSize, fontWeight: "var(--weight-semibold)", lineHeight: 1.15, marginTop: 6, color: valueColor, fontVariantNumeric: "tabular-nums", letterSpacing: "var(--tracking-tight)", overflowWrap: "anywhere", wordBreak: "break-word" }}>{value}</div>
      {hint && <div style={{ fontSize: "var(--text-xs)", color: "var(--text-tertiary)", marginTop: 6 }}>{hint}</div>}
    </Card>
  );
}
