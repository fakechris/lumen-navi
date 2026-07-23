import React from "react";

export interface TextareaProps extends React.TextareaHTMLAttributes<HTMLTextAreaElement> {
  /** Render the value in the serif reading face (long-form editing). */
  reading?: boolean;
}

/**
 * Multi-line text field.
 * @dsCard group="Components"
 */
export function Textarea(props: TextareaProps): JSX.Element;
