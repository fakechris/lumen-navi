import React from "react";

/**
 * Multi-line text field. Vertically resizable. Pass `reading` to render
 * the value in the serif reading face (transcript / translation editing).
 */
export function Textarea({ reading = false, style, ...props }) {
  return (
    <textarea
      style={{
        width: "100%",
        minHeight: 88,
        padding: "9px 11px",
        fontFamily: reading ? "var(--font-serif)" : "var(--font-sans)",
        fontSize: reading ? "var(--text-lg)" : "var(--text-sm)",
        lineHeight: reading ? "var(--leading-relaxed)" : "var(--leading-normal)",
        color: "var(--text)",
        background: "var(--surface-subtle)",
        border: "1px solid var(--border-strong)",
        borderRadius: "var(--radius-input)",
        outline: "none",
        resize: "vertical",
        transition: "border-color var(--dur-fast) var(--ease)",
        ...style,
      }}
      {...props}
    />
  );
}
