import React from "react";
import { Icon } from "./Icon.jsx";

const sizes = {
  sm: { minHeight: 32, padding: "0 10px", fontSize: "var(--text-sm)" },
  md: { minHeight: 40, padding: "0 15px", fontSize: "var(--text-sm)" },
  lg: { minHeight: 46, padding: "0 18px", fontSize: "var(--text-base)" },
};

function variantStyle(variant, selected) {
  if (selected) {
    return { background: "var(--accent-soft)", border: "1px solid color-mix(in srgb, var(--accent) 35%, var(--border))", color: "var(--accent)" };
  }
  switch (variant) {
    case "secondary":
      return { background: "var(--surface)", border: "1px solid var(--border-strong)", color: "var(--text)" };
    case "ghost":
      return { background: "transparent", border: "1px solid transparent", color: "var(--text-secondary)" };
    case "danger":
      return { background: "var(--danger)", border: "1px solid var(--danger)", color: "#fff7f5" };
    case "primary":
    default:
      return { background: "var(--accent)", border: "1px solid var(--accent)", color: "var(--on-accent)", fontWeight: "var(--weight-semibold)" };
  }
}

/**
 * The Lumen button. Terracotta-filled primary, bordered secondary,
 * transparent ghost, and a destructive danger. Quiet-utility house
 * rules: hover shifts background/opacity only — never position.
 */
export function Button({
  variant = "primary",
  size = "md",
  icon,
  selected = false,
  disabled = false,
  fullWidth = false,
  style,
  children,
  ...props
}) {
  const base = {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: "var(--space-2)",
    borderRadius: "var(--radius-input)",
    fontFamily: "var(--font-sans)",
    fontWeight: variant === "primary" ? "var(--weight-semibold)" : "var(--weight-medium)",
    lineHeight: 1,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.55 : 1,
    width: fullWidth ? "100%" : undefined,
    transition: "background var(--dur-fast) var(--ease), border-color var(--dur-fast) var(--ease), opacity var(--dur-fast) var(--ease)",
    whiteSpace: "nowrap",
  };
  return (
    <button
      disabled={disabled}
      style={{ ...base, ...sizes[size], ...variantStyle(variant, selected), ...style }}
      data-variant={variant}
      {...props}
    >
      {icon && <Icon name={icon} size={size === "lg" ? 18 : 16} />}
      {children}
    </button>
  );
}
