import React from "react";
import type { IconName } from "../core/Icon";

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  /** Leading icon; renders the field in a search-style wrapper. */
  icon?: IconName;
  /** Error state — danger-colored border. */
  invalid?: boolean;
}

/**
 * Single-line text input.
 * @dsCard group="Components"
 */
export function Input(props: InputProps): JSX.Element;
