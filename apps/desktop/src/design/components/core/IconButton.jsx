import React from "react";
import { Icon } from "./Icon.jsx";

const sizes = { sm: 30, md: 34, lg: 40 };

/**
 * Square, icon-only button — toolbar actions, theme toggles, row affordances.
 * Transparent by default; hover reveals a surface fill + hairline border.
 */
export function IconButton({ icon, size = "md", label, active = false, disabled = false, style, ...props }) {
  const dim = sizes[size];
  return (
    <button
      aria-label={label}
      title={label}
      disabled={disabled}
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: dim,
        height: dim,
        padding: 0,
        borderRadius: "var(--radius-input)",
        background: active ? "var(--accent-soft)" : "transparent",
        border: `1px solid ${active ? "color-mix(in srgb, var(--accent) 30%, var(--border))" : "transparent"}`,
        color: active ? "var(--accent)" : "var(--text-secondary)",
        cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.5 : 1,
        transition: "background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease)",
        ...style,
      }}
      {...props}
    >
      <Icon name={icon} size={size === "lg" ? 20 : 17} />
    </button>
  );
}
