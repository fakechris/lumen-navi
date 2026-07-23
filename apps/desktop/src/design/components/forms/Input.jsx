import React from "react";
import { Icon } from "../core/Icon.jsx";

const fieldBase = {
  width: "100%",
  minHeight: 40,
  padding: "8px 11px",
  fontFamily: "var(--font-sans)",
  fontSize: "var(--text-sm)",
  color: "var(--text)",
  background: "var(--surface-subtle)",
  border: "1px solid var(--border-strong)",
  borderRadius: "var(--radius-input)",
  outline: "none",
  transition: "border-color var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease)",
};

/**
 * Single-line text field. Optional leading icon (renders inside a
 * search-style wrapper). Set `invalid` to flag a validation error.
 */
export function Input({ icon, invalid = false, style, ...props }) {
  const border = invalid ? "var(--danger)" : "var(--border-strong)";
  if (icon) {
    return (
      <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "0 11px", background: "var(--surface-subtle)", border: `1px solid ${border}`, borderRadius: "var(--radius-input)" }}>
        <Icon name={icon} size={16} style={{ color: "var(--text-tertiary)", flex: "0 0 auto" }} />
        <input style={{ ...fieldBase, border: 0, background: "transparent", padding: "8px 0", minHeight: 38 }} {...props} />
      </div>
    );
  }
  return <input style={{ ...fieldBase, borderColor: border, ...style }} {...props} />;
}
