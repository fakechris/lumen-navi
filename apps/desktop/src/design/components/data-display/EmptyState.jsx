import React from "react";
import { Icon } from "../core/Icon.jsx";

/**
 * Empty-state block. Every list in the family shows one instead of a
 * blank area — text-first, with a quiet icon, a line of guidance, and an
 * optional action passed as `action`.
 */
export function EmptyState({ icon = "layers", title, children, action, style }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", textAlign: "center", gap: 10, padding: "36px 24px", color: "var(--text-tertiary)", ...style }}>
      <Icon name={icon} size={26} style={{ opacity: 0.6 }} />
      {title && <strong style={{ fontSize: "var(--text-base)", color: "var(--text-soft)", fontWeight: "var(--weight-semibold)" }}>{title}</strong>}
      {children && <p style={{ margin: 0, fontSize: "var(--text-sm)", maxWidth: 340, lineHeight: "var(--leading-snug)" }}>{children}</p>}
      {action && <div style={{ marginTop: 4 }}>{action}</div>}
    </div>
  );
}
