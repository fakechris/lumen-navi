import React from "react";

export interface SpinnerProps {
  /** Diameter in px. Default 15. */
  size?: number;
  style?: React.CSSProperties;
}

/**
 * Indeterminate loading spinner in currentColor.
 * @dsCard group="Components"
 */
export function Spinner(props: SpinnerProps): JSX.Element;
