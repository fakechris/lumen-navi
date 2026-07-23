import React from "react";

/**
 * Borderless spinner in `currentColor` — inherits the color of its
 * context (a button, a notice, a toolbar).
 */
export function Spinner({ size = 15, style }) {
  return (
    <span
      role="status"
      aria-label="Loading"
      style={{
        display: "inline-block",
        width: size,
        height: size,
        border: "2px solid currentColor",
        borderRightColor: "transparent",
        borderRadius: "50%",
        opacity: 0.72,
        animation: "lm-spin 0.8s linear infinite",
        ...style,
      }}
    >
      <style>{"@keyframes lm-spin{to{transform:rotate(360deg)}}"}</style>
    </span>
  );
}
