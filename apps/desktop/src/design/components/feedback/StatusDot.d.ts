import React from "react";

export interface StatusDotProps {
  /** Lifecycle status. "running" pulses. */
  status?: "idle" | "running" | "done" | "failed";
  /** Optional trailing label. */
  label?: string;
  style?: React.CSSProperties;
}

/**
 * Inline status indicator dot.
 * @dsCard group="Components"
 */
export function StatusDot(props: StatusDotProps): JSX.Element;
