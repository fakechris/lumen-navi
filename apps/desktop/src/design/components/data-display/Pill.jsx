import React from "react";

/**
 * Pill / badge — a rounded status label. `tone` maps to the semantic
 * palette, including the OVP2 knowledge states durable/caveated/blocked.
 * `solid` fills with the tone color; default is a soft tint.
 */
const map = {
  neutral:  ["var(--surface-subtle)", "var(--text-secondary)"],
  accent:   ["var(--accent-soft)",    "var(--accent)"],
  success:  ["var(--success-soft)",   "var(--success)"],
  warn:     ["var(--warn-soft)",      "var(--warn)"],
  danger:   ["var(--danger-soft)",    "var(--danger)"],
  durable:  ["var(--success-soft)",   "var(--durable)"],
  caveated: ["var(--warn-soft)",      "var(--caveated)"],
  blocked:  ["var(--danger-soft)",    "var(--blocked)"],
};

export function Pill({ tone = "neutral", solid = false, children, style, ...props }) {
  const [bg, fg] = map[tone] || map.neutral;
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "4px 9px",
        borderRadius: "var(--radius-pill)",
        fontSize: "var(--text-xs)",
        fontWeight: "var(--weight-semibold)",
        lineHeight: 1.3,
        background: solid ? fg : bg,
        color: solid ? "var(--on-accent)" : fg,
        ...style,
      }}
      {...props}
    >
      {children}
    </span>
  );
}
