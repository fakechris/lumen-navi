import React from "react";
import type { IconName } from "./Icon";

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  /** Visual weight. Default "primary". */
  variant?: "primary" | "secondary" | "ghost" | "danger";
  /** Control height. Default "md" (40px); "sm" 32px; "lg" 46px. */
  size?: "sm" | "md" | "lg";
  /** Optional leading icon from the Lumen set. */
  icon?: IconName;
  /** Toggled/active state — accent-tinted fill. */
  selected?: boolean;
  /** Stretch to fill the container width. */
  fullWidth?: boolean;
}

/**
 * Primary action control for the Lumen family.
 * @dsCard group="Components"
 * @startingPoint section="Core" subtitle="Button variants, sizes & states" viewport="700x150"
 */
export function Button(props: ButtonProps): JSX.Element;
