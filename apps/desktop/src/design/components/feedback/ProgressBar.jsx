import React from "react";

/**
 * Slim determinate progress bar for transcription / translation / model
 * download. Pass `value` 0–100; omit for an indeterminate accent track.
 * Optional `label` + percentage row above.
 */
export function ProgressBar({ value, label, tone = "accent", style }) {
  const color = tone === "accent" ? "var(--accent)" : `var(--${tone})`;
  const pct = typeof value === "number" ? Math.max(0, Math.min(100, value)) : null;
  return (
    <div style={{ width: "100%", ...style }}>
      {(label || pct !== null) && (
        <div style={{ display: "flex", justifyContent: "space-between", fontSize: "var(--text-xs)", marginBottom: 7 }}>
          {label && <strong style={{ fontWeight: "var(--weight-semibold)" }}>{label}</strong>}
          {pct !== null && <span style={{ color: "var(--text-tertiary)", fontVariantNumeric: "tabular-nums" }}>{Math.round(pct)}%</span>}
        </div>
      )}
      <div style={{ height: 6, borderRadius: "var(--radius-pill)", background: "var(--surface-subtle)", border: "1px solid var(--border)", overflow: "hidden" }}>
        <div style={{ height: "100%", width: pct !== null ? `${pct}%` : "40%", background: color, borderRadius: "var(--radius-pill)", transition: "width var(--dur-normal) var(--ease)" }} />
      </div>
    </div>
  );
}
