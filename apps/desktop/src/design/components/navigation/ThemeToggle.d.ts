import React from "react";

export interface ThemeToggleProps {
  /** Controlled value. Omit for uncontrolled (reads data-theme). */
  theme?: "light" | "dark";
  /** Fired with the new theme. */
  onChange?: (theme: "light" | "dark") => void;
  /** localStorage key. Default "lumen-theme" (OVP2 uses "ovp-theme"). */
  storageKey?: string;
  /** Button size. Default "md". */
  size?: "sm" | "md" | "lg";
}

/**
 * Atelier ⇄ Vault theme switch (sets data-theme on <html>).
 * @dsCard group="Components"
 */
export function ThemeToggle(props: ThemeToggleProps): JSX.Element;
