import React from "react";

export interface SelectOption { value: string; label: string; }

export interface SelectProps extends React.SelectHTMLAttributes<HTMLSelectElement> {
  /** Options as strings or {value,label}. Alternatively pass <option> children. */
  options?: (string | SelectOption)[];
}

/**
 * Styled native dropdown with the Lumen chevron.
 * @dsCard group="Components"
 */
export function Select(props: SelectProps): JSX.Element;
