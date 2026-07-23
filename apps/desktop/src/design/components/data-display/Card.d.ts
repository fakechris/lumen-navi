import React from "react";

export interface CardProps extends React.HTMLAttributes<HTMLElement> {
  /** Hover affordance for clickable cards. */
  interactive?: boolean;
  /** Padding in px. Default 20. */
  pad?: number;
  /** Element tag. Default "div". */
  as?: keyof JSX.IntrinsicElements;
}

/**
 * Base bordered surface container.
 * @dsCard group="Components"
 * @startingPoint section="Data display" subtitle="Card, StatCard, Pill, EmptyState" viewport="700x260"
 */
export function Card(props: CardProps): JSX.Element;
