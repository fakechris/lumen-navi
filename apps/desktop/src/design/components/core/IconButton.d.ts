import React from "react";
import type { IconName } from "./Icon";

export interface IconButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  /** Glyph from the Lumen set. */
  icon: IconName;
  /** Accessible label (also the tooltip). */
  label: string;
  /** Button dimensions. Default "md" (34px). */
  size?: "sm" | "md" | "lg";
  /** Active/toggled — accent-tinted. */
  active?: boolean;
}

/**
 * Icon-only button for toolbars and row affordances.
 * @dsCard group="Components"
 */
export function IconButton(props: IconButtonProps): JSX.Element;
