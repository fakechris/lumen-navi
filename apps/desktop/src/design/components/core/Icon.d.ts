import React from "react";

export type IconName =
  | "folder" | "transcript" | "settings" | "upload" | "link" | "microphone"
  | "play" | "chevronRight" | "chevronDown" | "search" | "star" | "sun"
  | "moon" | "check" | "alert" | "translate" | "layers" | "close";

export interface IconProps extends React.SVGProps<SVGSVGElement> {
  /** Glyph name from the Lumen set. */
  name: IconName;
  /** Square px size. Default 20. */
  size?: number;
}

/**
 * Outline icon from the Lumen set (24px grid, 1.8px stroke, currentColor).
 * @dsCard group="Components"
 */
export function Icon(props: IconProps): JSX.Element;
export const iconNames: IconName[];
