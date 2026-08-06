import type { StoredArtifact, StoredObservation, SyncStatus } from "./local-store";

export type ReadingTier = "deep" | "scan" | "brief";

export interface VisitRecord {
  id: string;
  url: string;
  domain: string;
  title: string;
  firstSeenAt: string;
  lastSeenAt: string;
  activeMs: number;
  visibleMs: number;
  maxScrollRatio: number;
  wordCount: number;
  eventCount: number;
  readingTier: ReadingTier;
  privacyGate: string;
  extractionStatus: string;
  content?: string;
  contentHash?: string;
  feedback?: "flag" | "dismiss";
  syncStatus: SyncStatus;
}

export interface DomainSummary {
  domain: string;
  visits: number;
  activeMs: number;
  deepReads: number;
  contentItems: number;
}

export interface DailyActivity {
  date: string;
  visits: number;
  deepReads: number;
  activeMs: number;
}

interface VisitDraft extends Omit<VisitRecord, "readingTier"> {}

export function buildVisitRecords(
  observations: StoredObservation[],
  artifacts: StoredArtifact[],
): VisitRecord[] {
  const artifactByEvent = new Map(artifacts.map((item) => [item.event_id, item]));
  const drafts = new Map<string, VisitDraft>();
  const sorted = [...observations].sort((a, b) => a.ts.localeCompare(b.ts));

  for (const observation of sorted) {
    if (!observation.url || observation.visit_id.startsWith("00000000-0000-4000-8000")) continue;
    const url = observation.url;
    const current = drafts.get(observation.visit_id) ?? {
      id: observation.visit_id,
      url,
      domain: domainOf(url),
      title: domainOf(url) || "未命名页面",
      firstSeenAt: observation.ts,
      lastSeenAt: observation.ts,
      activeMs: 0,
      visibleMs: 0,
      maxScrollRatio: 0,
      wordCount: 0,
      eventCount: 0,
      privacyGate: "metadata_only",
      extractionStatus: "metadata_only",
      syncStatus: observation.sync_status,
    };
    current.url = url;
    current.domain = domainOf(url);
    current.firstSeenAt = earlier(current.firstSeenAt, observation.ts);
    current.lastSeenAt = later(current.lastSeenAt, observation.ts);
    current.eventCount += 1;
    current.syncStatus = mergeSyncStatus(current.syncStatus, observation.sync_status);

    if (observation.kind === "browser.document_ready.v1") {
      current.title = stringValue(observation.payload.title) || current.title;
      current.wordCount = Math.max(current.wordCount, numberValue(observation.payload.wordCount));
      current.privacyGate = stringValue(observation.payload.privacy_gate) || current.privacyGate;
      current.extractionStatus = stringValue(observation.payload.extraction_status) || current.extractionStatus;
      const artifact = artifactByEvent.get(observation.id);
      if (artifact) {
        current.content = artifact.body;
        current.contentHash = artifact.content_hash;
      }
    }
    if (observation.kind === "browser.visit_closed.v1") {
      current.activeMs = Math.max(current.activeMs, numberValue(observation.payload.active_ms));
      current.visibleMs = Math.max(current.visibleMs, numberValue(observation.payload.visible_ms));
      current.maxScrollRatio = Math.max(
        current.maxScrollRatio,
        numberValue(observation.payload.max_scroll_ratio),
      );
    }
    if (observation.kind === "browser.visibility_focus_change.v1") {
      current.maxScrollRatio = Math.max(
        current.maxScrollRatio,
        numberValue(observation.payload.max_scroll_ratio),
      );
    }
    if (observation.kind === "browser.feedback.v1") {
      const action = stringValue(observation.payload.action);
      const active = observation.payload.active !== false;
      if (active && (action === "flag" || action === "dismiss")) current.feedback = action;
      if (!active && current.feedback === action) current.feedback = undefined;
    }
    drafts.set(observation.visit_id, current);
  }

  return [...drafts.values()]
    .map((draft) => ({ ...draft, readingTier: readingTier(draft) }))
    .sort((a, b) => b.lastSeenAt.localeCompare(a.lastSeenAt));
}

export function summarizeDomains(visits: VisitRecord[]): DomainSummary[] {
  const summaries = new Map<string, DomainSummary>();
  for (const visit of visits) {
    const current = summaries.get(visit.domain) ?? {
      domain: visit.domain,
      visits: 0,
      activeMs: 0,
      deepReads: 0,
      contentItems: 0,
    };
    current.visits += 1;
    current.activeMs += visit.activeMs;
    if (visit.readingTier === "deep") current.deepReads += 1;
    if (visit.content) current.contentItems += 1;
    summaries.set(visit.domain, current);
  }
  return [...summaries.values()].sort(
    (a, b) => b.activeMs - a.activeMs || b.visits - a.visits || a.domain.localeCompare(b.domain),
  );
}

export function dailyActivity(visits: VisitRecord[], days = 28, now = new Date()): DailyActivity[] {
  const output: DailyActivity[] = [];
  const byDate = new Map<string, DailyActivity>();
  for (const visit of visits) {
    const date = localDateKey(new Date(visit.lastSeenAt));
    const current = byDate.get(date) ?? { date, visits: 0, deepReads: 0, activeMs: 0 };
    current.visits += 1;
    current.activeMs += visit.activeMs;
    if (visit.readingTier === "deep") current.deepReads += 1;
    byDate.set(date, current);
  }
  const cursor = new Date(now);
  cursor.setHours(12, 0, 0, 0);
  for (let offset = days - 1; offset >= 0; offset -= 1) {
    const date = new Date(cursor);
    date.setDate(cursor.getDate() - offset);
    const key = localDateKey(date);
    output.push(byDate.get(key) ?? { date: key, visits: 0, deepReads: 0, activeMs: 0 });
  }
  return output;
}

function readingTier(visit: VisitDraft): ReadingTier {
  if (visit.activeMs >= 120_000 || (visit.activeMs >= 45_000 && visit.maxScrollRatio >= 0.72)) return "deep";
  if (visit.activeMs >= 20_000 || (visit.activeMs >= 8_000 && visit.maxScrollRatio >= 0.28)) return "scan";
  return "brief";
}

function domainOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./u, "");
  } catch {
    return "unknown";
  }
}

function stringValue(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function numberValue(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function earlier(left: string, right: string): string {
  return left.localeCompare(right) <= 0 ? left : right;
}

function later(left: string, right: string): string {
  return left.localeCompare(right) >= 0 ? left : right;
}

function mergeSyncStatus(left: SyncStatus, right: SyncStatus): SyncStatus {
  if (left === "rejected" || right === "rejected") return "rejected";
  if (left === "pending" || right === "pending") return "pending";
  return "synced";
}

function localDateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}
