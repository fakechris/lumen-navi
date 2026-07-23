import React from "react";

export interface StatCardProps {
  /** Uppercase label above the figure. */
  label: string;
  /** The metric (string or number). */
  value: React.ReactNode;
  /** Small hint/delta line below. */
  hint?: string;
  /** Colors the figure with a semantic tone. */
  tone?: "default" | "success" | "warn" | "danger" | "accent";
  onClick?: () => void;
}

/**
 * Dashboard metric tile.
 * @dsCard group="Components"
 */
export function StatCard(props: StatCardProps): JSX.Element;
