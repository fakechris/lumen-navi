import React from "react";
import { Icon } from "../core/Icon.jsx";

/**
 * Native select styled to match Lumen fields, with the family's
 * chevron glyph. Pass `options` (array of {value,label} or strings) or
 * <option> children.
 */
export function Select({ options, children, style, ...props }) {
  return (
    <div style={{ position: "relative", display: "inline-flex", width: style?.width || "auto" }}>
      <select
        style={{
          width: "100%",
          minHeight: 40,
          padding: "8px 34px 8px 11px",
          fontFamily: "var(--font-sans)",
          fontSize: "var(--text-sm)",
          color: "var(--text)",
          background: "var(--surface)",
          border: "1px solid var(--border-strong)",
          borderRadius: "var(--radius-input)",
          appearance: "none",
          WebkitAppearance: "none",
          outline: "none",
          cursor: "pointer",
          ...style,
        }}
        {...props}
      >
        {options
          ? options.map((o) => {
              const v = typeof o === "string" ? o : o.value;
              const l = typeof o === "string" ? o : o.label;
              return <option key={v} value={v}>{l}</option>;
            })
          : children}
      </select>
      <Icon name="chevronDown" size={15} style={{ position: "absolute", right: 10, top: "50%", transform: "translateY(-50%)", color: "var(--text-tertiary)", pointerEvents: "none" }} />
    </div>
  );
}
