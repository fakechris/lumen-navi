import React from "react";
import { IconButton } from "../core/IconButton.jsx";

/**
 * Atelier ⇄ Vault theme toggle. Flips `data-theme` on <html> and
 * persists to localStorage (key defaults to "lumen-theme"; OVP2 uses
 * "ovp-theme"). Uncontrolled by default; pass `theme`/`onChange` to
 * control it. Shows the moon in light mode (tap → dark) and vice-versa.
 */
export function ThemeToggle({ theme, onChange, storageKey = "lumen-theme", size = "md" }) {
  const [internal, setInternal] = React.useState(() => {
    if (typeof document !== "undefined") {
      return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
    }
    return "light";
  });
  const value = theme ?? internal;

  const apply = (next) => {
    if (typeof document !== "undefined") document.documentElement.setAttribute("data-theme", next);
    try { localStorage.setItem(storageKey, next); } catch (e) { /* storage disabled */ }
    setInternal(next);
    onChange && onChange(next);
  };

  const next = value === "dark" ? "light" : "dark";
  return (
    <IconButton
      icon={value === "dark" ? "sun" : "moon"}
      label={value === "dark" ? "Switch to Atelier (light)" : "Switch to Vault (dark)"}
      size={size}
      onClick={() => apply(next)}
    />
  );
}
