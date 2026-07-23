import React from "react";

export interface ProgressBarProps {
  /** Percent 0–100. Omit for an indeterminate track. */
  value?: number;
  /** Label shown above-left. */
  label?: string;
  /** Fill color. Default "accent". */
  tone?: "accent" | "success" | "warn" | "danger";
  style?: React.CSSProperties;
}

/**
 * Determinate/indeterminate progress bar.
 * @dsCard group="Components"
 */
export function ProgressBar(props: ProgressBarProps): JSX.Element;
