import React from "react";

export interface TabItem { id: string; label: string; }

export interface TabsProps {
  /** Tabs as strings or {id,label}. */
  tabs: (string | TabItem)[];
  /** Active tab id. */
  active: string;
  /** Fired with the new tab id. */
  onChange?: (id: string) => void;
  style?: React.CSSProperties;
}

/**
 * Underline tab bar for section switching.
 * @dsCard group="Components"
 */
export function Tabs(props: TabsProps): JSX.Element;
