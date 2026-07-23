import React from "react";
import { Icon } from "../core/Icon.jsx";

const tones = {
  info:    { bg: "var(--info-soft)",    fg: "var(--info)",    icon: "layers" },
  success: { bg: "var(--success-soft)", fg: "var(--success)", icon: "check" },
  warn:    { bg: "var(--warn-soft)",    fg: "var(--warn)",    icon: "alert" },
  danger:  { bg: "var(--danger-soft)",  fg: "var(--danger)",  icon: "alert" },
};

/**
 * Inline status banner. Four tones map to the semantic palette. Quiet
 * fill + matching text/icon color, no border unless it's a warning.
 */
export function Notice({ tone = "info", title, icon, children, style, ...props }) {
  const t = tones[tone] || tones.info;
  return (
    <div
      role="status"
      style={{
        display: "flex",
        alignItems: children && title ? "flex-start" : "center",
        gap: "var(--space-3)",
        padding: "11px 13px",
        borderRadius: "var(--radius-input)",
        background: t.bg,
        color: t.fg,
        border: tone === "warn" ? "1px solid var(--warn-border)" : "1px solid transparent",
        fontSize: "var(--text-sm)",
        lineHeight: "var(--leading-snug)",
        ...style,
      }}
      {...props}
    >
      <Icon name={icon || t.icon} size={17} style={{ flex: "0 0 auto", marginTop: children && title ? 1 : 0 }} />
      <div>
        {title && <strong style={{ display: "block", fontWeight: "var(--weight-semibold)" }}>{title}</strong>}
        {children && <span style={{ color: title ? "var(--text-soft)" : "inherit" }}>{children}</span>}
      </div>
    </div>
  );
}
