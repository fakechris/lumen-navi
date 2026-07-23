import React from "react";
import type { IconName } from "../core/Icon";

export interface NavItemProps {
  /** Leading icon from the Lumen set. */
  icon?: IconName;
  /** Label text. */
  label: string;
  /** Highlighted current entry. */
  active?: boolean;
  /** Renders as a link when set. */
  href?: string;
  onClick?: () => void;
  disabled?: boolean;
  style?: React.CSSProperties;
}

/**
 * Sidebar / top-bar navigation entry.
 * @dsCard group="Components"
 */
export function NavItem(props: NavItemProps): JSX.Element;
