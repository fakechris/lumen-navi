import React from "react";

/**
 * Underline tab bar (the family's editor/section switcher). Bottom
 * hairline with a 2px accent underline under the active tab. `tabs` is
 * an array of {id,label} or strings; controlled via `active`/`onChange`.
 */
export function Tabs({ tabs, active, onChange, style }) {
  const norm = tabs.map((t) => (typeof t === "string" ? { id: t, label: t } : t));
  return (
    <div style={{ display: "flex", gap: 4, borderBottom: "1px solid var(--border)", overflowX: "auto", ...style }}>
      {norm.map((t) => {
        const on = t.id === active;
        return (
          <button
            key={t.id}
            onClick={() => onChange && onChange(t.id)}
            style={{
              appearance: "none",
              background: "transparent",
              border: 0,
              borderBottom: `2px solid ${on ? "var(--accent)" : "transparent"}`,
              marginBottom: -1,
              minHeight: 42,
              padding: "0 14px",
              fontFamily: "var(--font-sans)",
              fontSize: "var(--text-sm)",
              fontWeight: on ? "var(--weight-semibold)" : "var(--weight-medium)",
              color: on ? "var(--text)" : "var(--text-secondary)",
              cursor: "pointer",
              whiteSpace: "nowrap",
              transition: "color var(--dur-fast) var(--ease)",
            }}
          >
            {t.label}
          </button>
        );
      })}
    </div>
  );
}
