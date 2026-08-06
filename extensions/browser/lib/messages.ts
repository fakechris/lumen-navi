import type { PageSignals } from "./privacy";
import type { Settings } from "./types";

export interface PageMetadata {
  title: string;
  canonical?: string;
  language?: string;
  author?: string;
  publishedAt?: string;
  referrer?: string;
  wordCount: number;
}

export type RuntimeMessage =
  | { type: "get-capture-gate" }
  | {
      type: "document-ready";
      url: string;
      metadata: PageMetadata;
      signals: Required<PageSignals>;
    }
  | {
      type: "content-ready";
      url: string;
      visitId: string;
      documentId: string;
      signals: Required<PageSignals>;
      extractionStatus: string;
      markdown?: string;
      contentHash?: string;
    }
  | {
      type: "visibility-focus";
      visitId: string;
      documentId: string;
      visible: boolean;
      focused: boolean;
      maxScrollRatio: number;
    }
  | {
      type: "metrics-checkpoint";
      visitId: string;
      documentId: string;
      activeMs: number;
      visibleMs: number;
      backgroundMs: number;
      maxScrollRatio: number;
    }
  | {
      type: "visit-close";
      visitId: string;
      documentId: string;
      activeMs: number;
      visibleMs: number;
      backgroundMs: number;
      visibleAtClose: boolean;
      maxScrollRatio: number;
      closeReason: string;
    }
  | { type: "get-status"; tabId?: number }
  | { type: "set-paused"; paused: boolean }
  | { type: "exclude-current-host"; tabId?: number }
  | { type: "set-feedback"; tabId?: number; action: "flag" | "dismiss" }
  | { type: "update-settings"; patch: Partial<Settings> }
  | { type: "flush" };
