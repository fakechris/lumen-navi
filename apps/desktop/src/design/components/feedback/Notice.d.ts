import React from "react";
import type { IconName } from "../core/Icon";

export interface NoticeProps extends React.HTMLAttributes<HTMLDivElement> {
  /** Semantic tone. Default "info". */
  tone?: "info" | "success" | "warn" | "danger";
  /** Bold lead line above the body. */
  title?: string;
  /** Override the default tone icon. */
  icon?: IconName;
}

/**
 * Inline status/notification banner.
 * @dsCard group="Components"
 */
export function Notice(props: NoticeProps): JSX.Element;
