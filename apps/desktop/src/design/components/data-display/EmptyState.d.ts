import React from "react";
import type { IconName } from "../core/Icon";

export interface EmptyStateProps {
  /** Quiet icon at the top. Default "layers". */
  icon?: IconName;
  /** Bold headline. */
  title?: string;
  /** Guidance line (children). */
  children?: React.ReactNode;
  /** Optional action node (e.g. a Button). */
  action?: React.ReactNode;
  style?: React.CSSProperties;
}

/**
 * Guidance block shown in place of an empty list.
 * @dsCard group="Components"
 */
export function EmptyState(props: EmptyStateProps): JSX.Element;
