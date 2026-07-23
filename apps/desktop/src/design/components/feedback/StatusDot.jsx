import React from "react";

const toneColor = { running: "var(--accent)", done: "var(--success)", failed: "var(--danger)", idle: "var(--text-tertiary)" };

/**
 * Small colored dot for live/inline status. `running` pulses; the rest
 * are static. Optional trailing label text.
 */
export function StatusDot({ status = "idle", label, style }) {
  const color = toneColor[status] || toneColor.idle;
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 7, fontSize: "var(--text-xs)", color: "var(--text-secondary)", ...style }}>
      <span
        style={{ width: 7, height: 7, borderRadius: "50%", background: color, flex: "0 0 auto", animation: status === "running" ? "lm-pulse 1.4s ease-in-out infinite" : "none" }}
      />
      {label}
      <style>{"@keyframes lm-pulse{50%{opacity:.35}}"}</style>
    </span>
  );
}
