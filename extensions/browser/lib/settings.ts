import type { QueueState, Settings } from "./types";

const SETTINGS_KEY = "browserSettingsV1";
const QUEUE_KEY = "browserQueueV1";
export const DEFAULT_BATCH_SIZE = 10;

export const DEFAULT_EXCLUDED_HOSTS = [
  "mail.google.com",
  "outlook.office.com",
  "slack.com",
  "discord.com",
  "web.whatsapp.com",
  "web.telegram.org",
];

export function defaultSettings(): Settings {
  return {
    installationId: crypto.randomUUID(),
    daemonUrl: "http://127.0.0.1:7420",
    token: "",
    paused: false,
    daemonCaptureAllowed: false,
    daemonCaptureKnown: false,
    contentAllowHosts: [],
    daemonContentAllowHosts: [],
    excludedHosts: [...DEFAULT_EXCLUDED_HOSTS],
    daemonExcludedHosts: [],
    batchSize: DEFAULT_BATCH_SIZE,
    flushIntervalMs: 30_000,
    maxQueueSize: 500,
    maxQueueBytes: 4 * 1024 * 1024,
    maxArtifactBytes: 2 * 1024 * 1024,
    captureProfileVersion: "browser-mvp-v1",
  };
}

export async function readSettings(): Promise<Settings> {
  const stored = (await browser.storage.local.get(SETTINGS_KEY))[SETTINGS_KEY] as
    | Partial<Settings>
    | undefined;
  const defaults = defaultSettings();
  const settings = { ...defaults, ...stored };
  if (!stored?.installationId) await writeSettings(settings);
  return settings;
}

export async function writeSettings(settings: Settings): Promise<void> {
  await browser.storage.local.set({ [SETTINGS_KEY]: settings });
}

export async function updateSettings(patch: Partial<Settings>): Promise<Settings> {
  const settings = { ...(await readSettings()), ...patch };
  await writeSettings(settings);
  return settings;
}

export async function readQueue(): Promise<QueueState> {
  const stored = (await browser.storage.local.get(QUEUE_KEY))[QUEUE_KEY] as QueueState | undefined;
  return stored ?? { observations: [], artifacts: [], droppedEvents: 0 };
}

export async function writeQueue(queue: QueueState): Promise<void> {
  await browser.storage.local.set({ [QUEUE_KEY]: queue });
}

export async function settingsHash(settings: Settings): Promise<string> {
  const stable = JSON.stringify({
    contentAllowHosts: effectiveContentAllowHosts(settings).sort(),
    excludedHosts: effectiveExcludedHosts(settings).sort(),
    batchSize: settings.batchSize,
    flushIntervalMs: settings.flushIntervalMs,
    maxQueueSize: settings.maxQueueSize,
    maxQueueBytes: settings.maxQueueBytes,
    maxArtifactBytes: settings.maxArtifactBytes,
    captureProfileVersion: settings.captureProfileVersion,
  });
  return sha256(stable);
}

export function captureAllowed(settings: Settings): boolean {
  if (settings.paused) return false;
  // A reachable Navi policy may add system-level gates such as screen lock.
  // When Navi is absent, capture remains available in standalone mode.
  return !(settings.token && settings.daemonCaptureKnown && !settings.daemonCaptureAllowed);
}

export function effectiveContentAllowHosts(settings: Settings): string[] {
  return [...new Set([...settings.contentAllowHosts, ...settings.daemonContentAllowHosts])];
}

export function effectiveExcludedHosts(settings: Settings): string[] {
  return [...new Set([...settings.excludedHosts, ...settings.daemonExcludedHosts])];
}

export async function sha256(value: string): Promise<string> {
  const bytes = new TextEncoder().encode(value);
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
