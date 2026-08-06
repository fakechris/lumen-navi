import { Readability } from "@mozilla/readability";
import TurndownService from "turndown";

import type { RuntimeMessage } from "../lib/messages";
import { evaluatePage, sanitizeUrl } from "../lib/privacy";
import {
  captureAllowed,
  effectiveContentAllowHosts,
  effectiveExcludedHosts,
  readSettings,
  sha256,
} from "../lib/settings";

const MAX_MARKDOWN_BYTES = 2 * 1024 * 1024;

export default defineContentScript({
  matches: ["http://*/*", "https://*/*"],
  runAt: "document_idle",
  allFrames: false,
  async main() {
    const gate = (await browser.runtime.sendMessage({
      type: "get-capture-gate",
    } satisfies RuntimeMessage)) as { allowed?: boolean } | undefined;
    if (!gate?.allowed) return;
    const settings = await readSettings();
    if (!captureAllowed(settings)) return;
    const signals = pageSignals();
    const decision = evaluatePage(
      location.href,
      effectiveContentAllowHosts(settings),
      effectiveExcludedHosts(settings),
      signals,
    );
    if (!decision.observe) return;

    const response = (await browser.runtime.sendMessage({
      type: "document-ready",
      url: location.href,
      metadata: metadata(decision.contentAllowed),
      signals,
    } satisfies RuntimeMessage)) as { visitId?: string; documentId?: string } | undefined;
    if (!response?.visitId || !response.documentId) return;

    const metrics = new VisitMetrics();
    let checkpointTimer: number | undefined;
    const sendCheckpoint = () => {
      void browser.runtime.sendMessage({
        type: "metrics-checkpoint",
        visitId: response.visitId!,
        documentId: response.documentId!,
        activeMs: metrics.activeMs,
        visibleMs: metrics.visibleMs,
        backgroundMs: metrics.backgroundMs,
        maxScrollRatio: metrics.maxScrollRatio,
      } satisfies RuntimeMessage);
    };
    const scheduleCheckpoint = () => {
      if (checkpointTimer !== undefined) window.clearTimeout(checkpointTimer);
      checkpointTimer = window.setTimeout(() => {
        metrics.sample();
        sendCheckpoint();
      }, 1_000);
    };
    const emitState = () => {
      metrics.sample();
      void browser.runtime.sendMessage({
        type: "visibility-focus",
        visitId: response.visitId!,
        documentId: response.documentId!,
        visible: document.visibilityState === "visible",
        focused: document.hasFocus(),
        maxScrollRatio: metrics.maxScrollRatio,
      } satisfies RuntimeMessage);
      sendCheckpoint();
    };
    document.addEventListener("visibilitychange", emitState, { passive: true });
    window.addEventListener("focus", emitState, { passive: true });
    window.addEventListener("blur", emitState, { passive: true });
    window.addEventListener("scroll", () => {
      metrics.sampleScroll();
      scheduleCheckpoint();
    }, { passive: true });
    const checkpointInterval = window.setInterval(() => {
      metrics.sample();
      sendCheckpoint();
    }, 10_000);

    let closed = false;
    const close = (reason: string) => {
      if (closed) return;
      closed = true;
      if (checkpointTimer !== undefined) window.clearTimeout(checkpointTimer);
      window.clearInterval(checkpointInterval);
      metrics.sample();
      void browser.runtime.sendMessage({
        type: "visit-close",
        visitId: response.visitId!,
        documentId: response.documentId!,
        activeMs: metrics.activeMs,
        visibleMs: metrics.visibleMs,
        backgroundMs: metrics.backgroundMs,
        visibleAtClose: document.visibilityState === "visible",
        maxScrollRatio: metrics.maxScrollRatio,
        closeReason: reason,
      } satisfies RuntimeMessage);
    };
    window.addEventListener("pagehide", () => close("pagehide"), { once: true });
    emitState();
    if (decision.contentAllowed) {
      window.setTimeout(
        () => void extractContent(response.visitId!, response.documentId!),
        0,
      );
    }
  },
});

async function extractContent(
  visitId: string,
  documentId: string,
) {
  const gate = (await browser.runtime.sendMessage({
    type: "get-capture-gate",
  } satisfies RuntimeMessage)) as { allowed?: boolean } | undefined;
  if (!gate?.allowed) return;
  const signals = pageSignals();
  const settings = await readSettings();
  const decision = evaluatePage(
    location.href,
    effectiveContentAllowHosts(settings),
    effectiveExcludedHosts(settings),
    signals,
  );
  let markdown: string | undefined;
  let contentHash: string | undefined;
  let extractionStatus = decision.contentAllowed ? "empty" : "privacy_blocked";
  if (decision.contentAllowed) {
    try {
      const article = new Readability(document.cloneNode(true) as Document).parse();
      if (article?.content) {
        const candidate = new TurndownService({ headingStyle: "atx" }).turndown(article.content);
        if (
          new TextEncoder().encode(candidate).byteLength <=
          Math.min(MAX_MARKDOWN_BYTES, settings.maxArtifactBytes)
        ) {
          markdown = candidate;
          contentHash = await sha256(candidate);
          extractionStatus = "success";
        } else {
          extractionStatus = "too_large";
        }
      }
    } catch {
      extractionStatus = "failed";
    }
  }
  await browser.runtime.sendMessage({
    type: "content-ready",
    url: location.href,
    visitId,
    documentId,
    signals,
    extractionStatus,
    markdown,
    contentHash,
  } satisfies RuntimeMessage);
}

function pageSignals() {
  return {
    hasPasswordInput: Boolean(document.querySelector('input[type="password"]')),
    hasEmailInput: Boolean(document.querySelector('input[type="email"]')),
    hasContenteditable: Boolean(document.querySelector('[contenteditable=""], [contenteditable="true"]')),
    noindex: Boolean(document.querySelector('meta[name="robots" i][content*="noindex" i]')),
  };
}

function metadata(includeRichMetadata: boolean) {
  const text = document.body?.innerText ?? "";
  return {
    title: document.title,
    canonical: includeRichMetadata
      ? sanitizeOptionalUrl(document.querySelector<HTMLLinkElement>('link[rel="canonical"]')?.href)
      : undefined,
    language: includeRichMetadata ? document.documentElement.lang || undefined : undefined,
    author: includeRichMetadata
      ? document.querySelector<HTMLMetaElement>('meta[name="author"]')?.content
      : undefined,
    publishedAt: includeRichMetadata
      ? document.querySelector<HTMLMetaElement>('meta[property="article:published_time"]')?.content
      : undefined,
    referrer: document.referrer
      ? evaluatePage(document.referrer, [], [], {}).sanitized?.url
      : undefined,
    wordCount: text.trim() ? text.trim().split(/\s+/u).length : 0,
  };
}

function sanitizeOptionalUrl(value?: string) {
  return value ? sanitizeUrl(value)?.url : undefined;
}

class VisitMetrics {
  activeMs = 0;
  visibleMs = 0;
  maxScrollRatio = 0;
  private startedAt = performance.now();
  private lastSample = performance.now();
  private wasVisible = document.visibilityState === "visible";
  private wasActive = this.wasVisible && document.hasFocus();

  sample() {
    const now = performance.now();
    const elapsed = Math.max(0, now - this.lastSample);
    if (this.wasVisible) this.visibleMs += elapsed;
    if (this.wasActive) this.activeMs += elapsed;
    this.lastSample = now;
    this.wasVisible = document.visibilityState === "visible";
    this.wasActive = this.wasVisible && document.hasFocus();
    this.sampleScroll();
  }

  sampleScroll() {
    const scrollable = Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
    const ratio = scrollable === 0 ? 1 : Math.min(1, Math.max(0, window.scrollY / scrollable));
    this.maxScrollRatio = Math.max(this.maxScrollRatio, ratio);
  }


  get backgroundMs() {
    return Math.max(0, performance.now() - this.startedAt - this.visibleMs);
  }
}
