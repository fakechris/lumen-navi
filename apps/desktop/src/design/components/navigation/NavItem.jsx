import React from "react";
import { Icon } from "../core/Icon.jsx";

/**
 * Navigation entry for a sidebar or top bar — icon + label, with an
 * active state that raises a surface fill. Renders as <a> when `href`
 * is given, else <button>.
 */
export function NavItem({ icon, label, active = false, href, onClick, disabled = false, style }) {
  const Tag = href ? "a" : "button";
  return (
    <Tag
      href={href}
      onClick={onClick}
      disabled={Tag === "button" ? disabled : undefined}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        width: "100%",
        minHeight: 40,
        padding: "0 10px",
        borderRadius: "var(--radius-input)",
        border: 0,
        background: active ? "var(--surface)" : "transparent",
        boxShadow: active ? "0 1px 2px rgba(31,26,23,0.06)" : "none",
        color: active ? "var(--text)" : "var(--text-secondary)",
        fontFamily: "var(--font-sans)",
        fontSize: "var(--text-sm)",
        fontWeight: active ? "var(--weight-semibold)" : "var(--weight-regular)",
        textAlign: "left",
        textDecoration: "none",
        cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.48 : 1,
        transition: "background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease)",
      }}
    >
      {icon && <Icon name={icon} size={18} style={{ flex: "0 0 auto" }} />}
      <span>{label}</span>
    </Tag>
  );
}
