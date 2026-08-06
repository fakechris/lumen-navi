import { evaluatePage, sanitizeUrl } from "../lib/privacy";
import { isUnchangedReload } from "../lib/dedupe";
import {
  acknowledge,
  appendObservation,
  buildBatch,
  discardObservations,
} from "../lib/queue";
import {
  DEFAULT_BATCH_SIZE,
  captureAllowed,
  effectiveContentAllowHosts,
  effectiveExcludedHosts,
  readQueue,
  readSettings,
  sha256,
  settingsHash,
  updateSettings,
  writeQueue,
} from "../lib/settings";
import {
  backfillQueue,
  localStoreStats,
  markLocalSync,
  storeLocalObservation,
} from "../lib/local-store";
import type { RuntimeMessage } from "../lib/messages";
import type { BrowserArtifact, BrowserObservation, ObservationKind } from "../lib/types";

interface VisitContext {
  visitId: string;
  documentId: string;
  url: string;
  feedback?: "flag" | "dismiss";
  maxScrollRatio?: number;
  pendingNavigation?: Record<string, unknown>;
  pendingReload?: boolean;
  pendingDocument?: Record<string, unknown>;
  activeMs?: number;
  visibleMs?: number;
  backgroundMs?: number;
  metricBaseline?: { activeMs: number; visibleMs: number; backgroundMs: number };
}

const visits = new Map<number, VisitContext>();
const fingerprints = new Map<string, string>();
const VISITS_KEY = "activeBrowserVisitsV1";
const FINGERPRINTS_KEY = "browserDocumentFingerprintsV1";
const PAGE_FACT_MESSAGES = new Set<RuntimeMessage["type"]>([
  "document-ready",
  "content-ready",
  "metrics-checkpoint",
  "visibility-focus",
  "visit-close",
]);
let queueMutation: Promise<void> = Promise.resolve();
let policySync: Promise<void> = Promise.resolve();
let flushTimer: ReturnType<typeof setTimeout> | undefined;
let visitsReady: Promise<void> = Promise.resolve();

export default defineBackground(() => {
  visitsReady = restoreVisits();
  browser.webNavigation.onCommitted.addListener((details) => {
    if (details.frameId !== 0) return;
    void beginVisit(details);
  });

  browser.tabs.onRemoved.addListener((tabId) => {
    void closeRemovedTab(tabId);
  });

  browser.runtime.onMessage.addListener((message: RuntimeMessage, sender) =>
    handleMessage(message, sender.tab),
  );

  browser.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === "browser-flush") void syncPolicy().then(() => flushQueue());
    if (alarm.name === "browser-health") void emitHealth("periodic");
  });
  void browser.alarms.create("browser-flush", { periodInMinutes: 0.5 });
  void browser.alarms.create("browser-health", { periodInMinutes: 15 });
  void syncPolicy().then(() => emitHealth("startup"));
  void readQueue().then(backfillQueue).catch(() => undefined);
  void flushQueue();
});

async function beginVisit(details: Browser.webNavigation.WebNavigationTransitionCallbackDetails) {
  await visitsReady;
  if ((details as { documentLifecycle?: string }).documentLifecycle === "prerender") return;
  void syncPolicy();
  const settings = await readSettings();
  if (!captureAllowed(settings)) return;
  const tab = await browser.tabs.get(details.tabId).catch(() => undefined);
  if (tab?.incognito) {
    visits.delete(details.tabId);
    await persistVisits();
    return;
  }
  const decision = evaluatePage(
    details.url,
    effectiveContentAllowHosts(settings),
    effectiveExcludedHosts(settings),
    {},
  );
  if (!decision.observe || !decision.sanitized) {
    visits.delete(details.tabId);
    await persistVisits();
    return;
  }
  const previous = visits.get(details.tabId);
  const continuesReload = details.transitionType === "reload" && previous !== undefined;
  if (previous && !continuesReload) {
    if (!previous.pendingNavigation) {
      await enqueueEvent(event("browser.visit_closed.v1", previous, {
        active_ms: previous.activeMs,
        visible_ms: previous.visibleMs,
        background_ms: previous.backgroundMs,
        max_scroll_ratio: previous.maxScrollRatio,
        close_reason: "navigation_replaced",
      }));
    }
  }
  const navigation = {
    tab_id: details.tabId,
    window_id: tab?.windowId,
    transition: details.transitionType,
    transition_qualifiers: details.transitionQualifiers,
    opener_tab_id: tab?.openerTabId,
    restored: details.transitionQualifiers?.includes("forward_back") ?? false,
  };
  const context: VisitContext = {
    visitId: continuesReload ? previous.visitId : crypto.randomUUID(),
    documentId: details.documentId ?? crypto.randomUUID(),
    url: decision.sanitized.url,
    pendingNavigation: navigation,
    pendingReload: continuesReload,
    maxScrollRatio: continuesReload ? previous.maxScrollRatio : undefined,
    activeMs: continuesReload ? previous.activeMs : undefined,
    visibleMs: continuesReload ? previous.visibleMs : undefined,
    backgroundMs: continuesReload ? previous.backgroundMs : undefined,
    metricBaseline: continuesReload
      ? {
          activeMs: previous.activeMs ?? 0,
          visibleMs: previous.visibleMs ?? 0,
          backgroundMs: previous.backgroundMs ?? 0,
        }
      : undefined,
  };
  visits.set(details.tabId, context);
  await persistVisits();
  if (!context.pendingReload) {
    await enqueueEvent(event("browser.navigation_committed.v1", context, navigation));
    context.pendingNavigation = undefined;
    await persistVisits();
  }
}

async function handleMessage(message: RuntimeMessage, senderTab?: Browser.tabs.Tab): Promise<unknown> {
  await visitsReady;
  if (senderTab?.incognito) return { ignored: true, reason: "incognito" };
  const senderTabId = senderTab?.id;
  if (message.type === "get-capture-gate") {
    void syncPolicy();
    const settings = await readSettings();
    return { allowed: captureAllowed(settings) };
  }
  if (senderTab && PAGE_FACT_MESSAGES.has(message.type)) {
    void syncPolicy();
    const settings = await readSettings();
    if (!captureAllowed(settings)) {
      return { ignored: true, reason: "capture_gate" };
    }
  }
  switch (message.type) {
    case "document-ready": {
      if (senderTabId === undefined) return { ignored: true };
      const settings = await readSettings();
      if (!captureAllowed(settings)) return { ignored: true };
      const decision = evaluatePage(
        message.url,
        effectiveContentAllowHosts(settings),
        effectiveExcludedHosts(settings),
        message.signals,
      );
      if (!decision.observe || !decision.sanitized) return { ignored: true };
      const context = visits.get(senderTabId) ?? {
        visitId: crypto.randomUUID(),
        documentId: crypto.randomUUID(),
        url: decision.sanitized.url,
      };
      visits.set(senderTabId, context);
      await persistVisits();
      const observation = event("browser.document_ready.v1", context, {
        ...message.metadata,
        privacy_gate: decision.contentAllowed ? "allowed" : "metadata_only",
        has_password_input: message.signals.hasPasswordInput,
        has_email_input: message.signals.hasEmailInput,
        has_contenteditable: message.signals.hasContenteditable,
        noindex: message.signals.noindex,
        extraction_status: decision.contentAllowed ? "pending" : "metadata_only",
      });
      context.pendingDocument = observation.payload;
      const fingerprint = await documentFingerprint(observation.payload);
      if (context.pendingReload) {
        if (!decision.contentAllowed) {
          const duplicate = await finalizeReload(senderTabId, context, observation, undefined, fingerprint);
          if (duplicate) {
            return {
              visitId: context.visitId,
              documentId: context.documentId,
              duplicate: true,
            };
          }
        } else {
          await persistVisits();
        }
      } else {
        await enqueueEvent(observation);
        if (!decision.contentAllowed) {
          fingerprints.set(context.url, fingerprint);
          await persistVisits();
        }
      }
      return { visitId: context.visitId, documentId: context.documentId };
    }
    case "content-ready": {
      if (senderTabId === undefined) return { ignored: true };
      const settings = await readSettings();
      if (!captureAllowed(settings)) return { ignored: true };
      const context = matchVisit(senderTabId, message.visitId, message.documentId);
      if (!context) return { ignored: true };
      const decision = evaluatePage(
        message.url,
        effectiveContentAllowHosts(settings),
        effectiveExcludedHosts(settings),
        message.signals,
      );
      const observation = event("browser.document_ready.v1", context, {
        ...(context.pendingDocument ?? {}),
        phase: "content",
        privacy_gate: decision.contentAllowed ? "allowed" : "metadata_only",
        has_password_input: message.signals.hasPasswordInput,
        has_email_input: message.signals.hasEmailInput,
        has_contenteditable: message.signals.hasContenteditable,
        noindex: message.signals.noindex,
        extraction_status: message.extractionStatus,
      });
      const artifact: BrowserArtifact | undefined =
        decision.contentAllowed && message.markdown && message.extractionStatus === "success"
          ? {
              event_id: observation.id,
              media_type: "text/markdown",
              body: message.markdown,
              content_hash: message.contentHash,
            }
          : undefined;
      const fingerprint = message.contentHash ??
        await documentFingerprint(observation.payload);
      if (context.pendingReload) {
        const duplicate = await finalizeReload(
          senderTabId,
          context,
          observation,
          artifact,
          fingerprint,
        );
        return duplicate ? { ignored: true, duplicate: true } : { ok: true };
      }
      await enqueueEvent(observation, artifact);
      context.pendingDocument = undefined;
      fingerprints.set(context.url, fingerprint);
      await persistVisits();
      return { ok: true };
    }
    case "metrics-checkpoint": {
      const context = matchVisit(senderTabId, message.visitId, message.documentId);
      if (!context) return { ignored: true };
      updateMetrics(context, message);
      await persistVisits();
      return { ok: true };
    }
    case "visibility-focus": {
      const context = matchVisit(senderTabId, message.visitId, message.documentId);
      if (!context) return { ignored: true };
      context.maxScrollRatio = Math.max(context.maxScrollRatio ?? 0, message.maxScrollRatio);
      if (context.pendingReload) {
        await persistVisits();
        return { ok: true, pending: true };
      }
      await enqueueEvent(event("browser.visibility_focus_change.v1", context, {
        visible: message.visible,
        focused: message.focused,
        max_scroll_ratio: message.maxScrollRatio,
      }));
      await persistVisits();
      return { ok: true };
    }
    case "visit-close": {
      const context = matchVisit(senderTabId, message.visitId, message.documentId);
      if (!context) return { ignored: true };
      updateMetrics(context, message);
      if (context.pendingReload) {
        return { ignored: true, pending: true };
      }
      await enqueueEvent(event("browser.visit_closed.v1", context, {
        active_ms: context.activeMs,
        visible_ms: context.visibleMs,
        background_ms: context.backgroundMs,
        visible_at_close: message.visibleAtClose,
        max_scroll_ratio: message.maxScrollRatio,
        close_reason: message.closeReason,
      }));
      if (senderTabId !== undefined) {
        visits.delete(senderTabId);
        await persistVisits();
      }
      return { ok: true };
    }
    case "get-status":
      void syncPolicy();
      return getStatus(message.tabId ?? senderTabId);
    case "set-paused":
      await setPaused(message.paused);
      return getStatus(message.type === "set-paused" ? senderTabId : undefined);
    case "exclude-current-host":
      return excludeCurrentHost(message.tabId ?? senderTabId);
    case "set-feedback":
      return setFeedback(message.tabId ?? senderTabId, message.action);
    case "update-settings":
      await updateSettings(message.patch);
      await syncPolicy();
      return { ok: true };
    case "flush":
      await flushQueue();
      return { ok: true };
  }
}

function matchVisit(tabId: number | undefined, visitId: string, documentId: string) {
  if (tabId === undefined) return undefined;
  const context = visits.get(tabId);
  return context?.visitId === visitId && context.documentId === documentId ? context : undefined;
}

function event(
  kind: ObservationKind,
  context: VisitContext,
  payload: Record<string, unknown>,
): BrowserObservation {
  return {
    id: crypto.randomUUID(),
    kind,
    ts: new Date().toISOString(),
    visit_id: context.visitId,
    document_id: context.documentId,
    url: context.url,
    payload,
  };
}

async function enqueueEvent(observation: BrowserObservation, artifact?: BrowserArtifact) {
  const settings = await readSettings();
  if (!captureAllowed(settings)) return;
  await storeLocalObservation(observation, artifact);
  queueMutation = queueMutation.catch(() => undefined).then(async () => {
    const queue = await readQueue();
    await writeQueue(
      appendObservation(
        queue,
        observation,
        artifact,
        settings.maxQueueSize,
        settings.maxQueueBytes,
      ),
    );
  });
  await queueMutation;
  const queue = await readQueue();
  if (queue.observations.length >= settings.batchSize) await flushQueue();
  else scheduleFlush(settings.flushIntervalMs);
}

function scheduleFlush(delay: number) {
  if (flushTimer) clearTimeout(flushTimer);
  flushTimer = setTimeout(() => void flushQueue(), Math.max(1_000, delay));
}

async function flushQueue() {
  await queueMutation;
  const settings = await readSettings();
  if (settings.paused || !settings.token) return;
  const queue = await readQueue();
  if (queue.observations.length === 0 && !queue.pendingGap) return;
  const batch = await buildBatch(queue, settings, await settingsHash(settings));
  const response = await fetch(`${settings.daemonUrl.replace(/\/$/, "")}/v1/browser/batches`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${settings.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(batch),
  }).catch(() => undefined);
  if (!response) return;
  if (!response.ok) {
    if ([400, 413, 423, 507].includes(response.status)) {
      const reason = response.status === 423
        ? "capture_gate"
        : response.status === 507
          ? "retention_limit"
          : "rejected_batch";
      queueMutation = queueMutation.catch(() => undefined).then(async () => {
        const latest = await readQueue();
        await writeQueue(
          discardObservations(latest, batch.observations.map((item) => item.id), reason),
        );
      });
      await queueMutation;
      await markLocalSync(
        batch.observations.map((item) => item.id),
        "rejected",
        reason,
      );
      if (response.status === 423) {
        await updateSettings({ daemonCaptureAllowed: false });
        visits.clear();
        await persistVisits();
      }
    }
    return;
  }
  queueMutation = queueMutation.catch(() => undefined).then(async () => {
    const latest = await readQueue();
    await writeQueue(acknowledge(latest, batch.observations.map((item) => item.id)));
  });
  await queueMutation;
  await markLocalSync(batch.observations.map((item) => item.id), "synced");
}

async function getStatus(tabId?: number) {
  const [settings, queue, localStore] = await Promise.all([
    readSettings(),
    readQueue(),
    localStoreStats().catch(() => undefined),
  ]);
  const health = settings.token
    ? await fetch(`${settings.daemonUrl.replace(/\/$/, "")}/v1/health`)
        .then((response) => (response.ok ? response.json() : undefined))
        .catch(() => undefined)
    : undefined;
  const context = tabId === undefined ? undefined : visits.get(tabId);
  return {
    settings,
    queueDepth: queue.observations.length + (queue.pendingGap ? 1 : 0),
    droppedEvents: queue.droppedEvents,
    captureEnabled: captureAllowed(settings),
    localStore,
    health,
    currentHost: context ? sanitizeUrl(context.url)?.host : undefined,
    feedback: context?.feedback,
  };
}

function syncPolicy(): Promise<void> {
  policySync = policySync.catch(() => undefined).then(syncPolicyOnce);
  return policySync;
}

async function syncPolicyOnce() {
  const settings = await readSettings();
  if (!settings.token) {
    const shouldResumePages = settings.daemonCaptureKnown && !settings.daemonCaptureAllowed;
    await updateSettings({ daemonCaptureAllowed: false, daemonCaptureKnown: false });
    if (shouldResumePages) await resumeActivePages();
    return;
  }
  const policy = await fetch(`${settings.daemonUrl.replace(/\/$/, "")}/v1/browser/policy`, {
    headers: { authorization: `Bearer ${settings.token}` },
    signal: AbortSignal.timeout(3_000),
  })
    .then((response) => (response.ok ? response.json() : undefined))
    .catch(() => undefined) as
    | {
        capture_allowed?: boolean;
        content_allow_hosts?: string[];
        excluded_hosts?: string[];
        max_batch_size?: number;
        max_artifact_bytes?: number;
      }
    | undefined;
  if (!policy) {
    const shouldResumePages = settings.daemonCaptureKnown && !settings.daemonCaptureAllowed;
    await updateSettings({ daemonCaptureAllowed: false, daemonCaptureKnown: false });
    if (shouldResumePages) await resumeActivePages();
    return;
  }
  const remoteCaptureAllowed = policy.capture_allowed === true;
  const shouldResumePages = settings.daemonCaptureKnown
    && !settings.daemonCaptureAllowed
    && remoteCaptureAllowed;
  await updateSettings({
    daemonCaptureAllowed: remoteCaptureAllowed,
    daemonCaptureKnown: true,
    daemonContentAllowHosts: policy.content_allow_hosts ?? [],
    daemonExcludedHosts: policy.excluded_hosts ?? [],
    batchSize: Math.min(DEFAULT_BATCH_SIZE, policy.max_batch_size ?? DEFAULT_BATCH_SIZE),
    maxArtifactBytes: policy.max_artifact_bytes ?? settings.maxArtifactBytes,
  });
  if (!remoteCaptureAllowed && visits.size > 0) {
    visits.clear();
    await persistVisits();
  }
  if (shouldResumePages) await resumeActivePages();
}

async function resumeActivePages() {
  const tabs = await browser.tabs.query({ active: true });
  await Promise.all(
    tabs
      .filter((tab) => tab.id !== undefined && !tab.incognito && /^https?:\/\//.test(tab.url ?? ""))
      .map((tab) =>
        browser.scripting.executeScript({
          target: { tabId: tab.id! },
          files: ["/content-scripts/content.js"],
        }).catch(() => undefined),
      ),
  );
}

async function setPaused(paused: boolean) {
  const settings = await updateSettings({ paused });
  if (paused) {
    visits.clear();
    await persistVisits();
  }
  if (!settings.token) return;
  await fetch(`${settings.daemonUrl.replace(/\/$/, "")}/v1/control`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${settings.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ op: paused ? "pause" : "resume", source: "browser" }),
  }).catch(() => undefined);
  await syncPolicy();
  if (!paused) void flushQueue();
}

async function excludeCurrentHost(tabId?: number) {
  if (tabId === undefined) return { error: "no_active_tab" };
  const context = visits.get(tabId);
  const host = context ? sanitizeUrl(context.url)?.host : undefined;
  if (!host) return { error: "no_active_host" };
  const settings = await readSettings();
  if (!settings.excludedHosts.includes(host)) {
    await updateSettings({ excludedHosts: [...settings.excludedHosts, host] });
  }
  visits.delete(tabId);
  await persistVisits();
  return { ok: true, host };
}

async function setFeedback(tabId: number | undefined, action: "flag" | "dismiss") {
  void syncPolicy();
  const settings = await readSettings();
  if (!captureAllowed(settings)) {
    return { error: "capture_gate" };
  }
  if (tabId === undefined) return { error: "no_active_tab" };
  const context = visits.get(tabId);
  if (!context) return { error: "page_not_observed" };
  const previous = context.feedback;
  if (previous) {
    await enqueueEvent(event("browser.feedback.v1", context, { action: previous, active: false }));
  }
  context.feedback = previous === action ? undefined : action;
  if (context.feedback) {
    await enqueueEvent(event("browser.feedback.v1", context, { action, active: true }));
  }
  await persistVisits();
  return { ok: true, feedback: context.feedback };
}

async function closeRemovedTab(tabId: number) {
  await visitsReady;
  void syncPolicy();
  const settings = await readSettings();
  if (!captureAllowed(settings)) {
    visits.delete(tabId);
    await persistVisits();
    return;
  }
  const context = visits.get(tabId);
  if (!context) return;
  await enqueueEvent(event("browser.visit_closed.v1", context, {
    active_ms: context.activeMs,
    visible_ms: context.visibleMs,
    background_ms: context.backgroundMs,
    max_scroll_ratio: context.maxScrollRatio,
    close_reason: "tab_removed",
  }));
  visits.delete(tabId);
  await persistVisits();
}

async function restoreVisits() {
  const storedState = await browser.storage.session.get([VISITS_KEY, FINGERPRINTS_KEY]);
  const stored = storedState[VISITS_KEY] as
    | Array<[number, VisitContext]>
    | undefined;
  for (const [tabId, context] of stored ?? []) visits.set(tabId, context);
  const storedFingerprints = storedState[FINGERPRINTS_KEY] as Array<[string, string]> | undefined;
  for (const [url, fingerprint] of storedFingerprints ?? []) fingerprints.set(url, fingerprint);
}

async function persistVisits() {
  await browser.storage.session.set({
    [VISITS_KEY]: [...visits.entries()],
    [FINGERPRINTS_KEY]: [...fingerprints.entries()].slice(-1_000),
  });
}

async function documentFingerprint(data: Record<string, unknown>) {
  return sha256(JSON.stringify(data, Object.keys(data).sort()));
}

async function finalizeReload(
  tabId: number,
  context: VisitContext,
  observation: BrowserObservation,
  artifact: BrowserArtifact | undefined,
  fingerprint: string,
) {
  if (isUnchangedReload(fingerprints.get(context.url), fingerprint)) {
    context.pendingNavigation = undefined;
    context.pendingReload = false;
    context.pendingDocument = undefined;
    await persistVisits();
    return true;
  }
  if (context.pendingNavigation) {
    await enqueueEvent(event("browser.navigation_committed.v1", context, context.pendingNavigation));
  }
  context.pendingNavigation = undefined;
  context.pendingReload = false;
  context.pendingDocument = undefined;
  await enqueueEvent(observation, artifact);
  fingerprints.set(context.url, fingerprint);
  await persistVisits();
  return false;
}

function updateMetrics(
  context: VisitContext,
  metrics: {
    activeMs: number;
    visibleMs: number;
    backgroundMs: number;
    maxScrollRatio: number;
  },
) {
  const baseline = context.metricBaseline ?? { activeMs: 0, visibleMs: 0, backgroundMs: 0 };
  context.activeMs = baseline.activeMs + metrics.activeMs;
  context.visibleMs = baseline.visibleMs + metrics.visibleMs;
  context.backgroundMs = baseline.backgroundMs + metrics.backgroundMs;
  context.maxScrollRatio = Math.max(context.maxScrollRatio ?? 0, metrics.maxScrollRatio);
}

async function emitHealth(reason: string) {
  const [settings, queue] = await Promise.all([readSettings(), readQueue()]);
  await enqueueEvent({
    id: crypto.randomUUID(),
    kind: "browser.health.v1",
    ts: new Date().toISOString(),
    visit_id: "00000000-0000-4000-8000-000000000000",
    document_id: "browser-extension",
    payload: {
      reason,
      queue_depth: queue.observations.length,
      dropped_events: queue.droppedEvents,
      capture_profile_version: settings.captureProfileVersion,
    },
  });
}
