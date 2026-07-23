import React from "react";

export interface PillProps extends React.HTMLAttributes<HTMLSpanElement> {
  /** Semantic tone. durable/caveated/blocked are the OVP2 knowledge states. */
  tone?: "neutral" | "accent" | "success" | "warn" | "danger" | "durable" | "caveated" | "blocked";
  /** Filled rather than soft-tinted. */
  solid?: boolean;
}

/**
 * Rounded status badge / label.
 * @dsCard group="Components"
 */
export function Pill(props: PillProps): JSX.Element;
