import React from "react";

/**
 * Base surface container. 1px border, 16px radius, no shadow (quiet-
 * utility: shadow lives only on the outer shell). `interactive` adds a
 * hover border-strong tint for clickable cards. `pad` controls padding.
 */
export function Card({ interactive = false, pad = 20, as = "div", style, children, ...props }) {
  const Tag = as;
  return (
    <Tag
      style={{
        background: "var(--surface)",
        border: "1px solid var(--border)",
        borderRadius: "var(--radius-card)",
        padding: pad,
        transition: "border-color var(--dur-fast) var(--ease)",
        cursor: interactive ? "pointer" : undefined,
        ...style,
      }}
      onMouseEnter={interactive ? (e) => (e.currentTarget.style.borderColor = "var(--border-strong)") : undefined}
      onMouseLeave={interactive ? (e) => (e.currentTarget.style.borderColor = "var(--border)") : undefined}
      {...props}
    >
      {children}
    </Tag>
  );
}
