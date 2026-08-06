import type { RuntimeMessage } from "../../lib/messages";
import type { LocalStoreStats, Settings } from "../../lib/types";

interface Status {
  settings: Settings;
  queueDepth: number;
  droppedEvents: number;
  captureEnabled: boolean;
  localStore?: LocalStoreStats;
  health?: { browser?: { configured: boolean; paused: boolean; accepted_events: number } };
  currentHost?: string;
  feedback?: "flag" | "dismiss";
}

const statusEl = required("status");
const pauseButton = requiredButton("pause");
const dashboardButton = requiredButton("dashboard");
const excludeButton = requiredButton("exclude");
const flagButton = requiredButton("flag");
const dismissButton = requiredButton("dismiss");
const messageEl = required("message");
const tokenInput = requiredInput("token");
const daemonUrlInput = requiredInput("daemon-url");
const allowInput = requiredTextArea("allow");
const saveButton = requiredButton("save");

let status: Status;
let activeTabId: number | undefined;
void refresh();

dashboardButton.addEventListener("click", async () => {
  await browser.tabs.create({ url: browser.runtime.getURL("/dashboard.html") });
  window.close();
});

pauseButton.addEventListener("click", async () => {
  await send({ type: "set-paused", paused: !status.settings.paused });
  await refresh();
});
excludeButton.addEventListener("click", async () => {
  const result = (await send({ type: "exclude-current-host", tabId: activeTabId })) as { host?: string; error?: string };
  messageEl.textContent = result.host ? `已排除 ${result.host}` : result.error ?? "无法排除当前页面";
  await refresh();
});
flagButton.addEventListener("click", () => toggleFeedback("flag"));
dismissButton.addEventListener("click", () => toggleFeedback("dismiss"));
saveButton.addEventListener("click", async () => {
  await send({
    type: "update-settings",
    patch: {
      token: tokenInput.value.trim(),
      daemonUrl: daemonUrlInput.value.trim() || "http://127.0.0.1:7420",
      contentAllowHosts: parseHosts(allowInput.value),
    },
  });
  messageEl.textContent = "设置已保存在本机";
  await send({ type: "flush" });
  await refresh();
});

async function toggleFeedback(action: "flag" | "dismiss") {
  const result = (await send({ type: "set-feedback", tabId: activeTabId, action })) as { error?: string };
  if (result.error) messageEl.textContent = result.error;
  await refresh();
}

async function refresh() {
  const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
  activeTabId = tab?.id;
  status = (await send({ type: "get-status", tabId: activeTabId })) as Status;
  const daemon = status.health?.browser?.configured ? "Navi 已连接" : "Navi 未连接";
  const gate = status.captureEnabled ? "本地采集中" : "采集已暂停";
  const stored = status.localStore?.observations ?? 0;
  statusEl.textContent = `${gate} · 本地 ${stored} 条 · ${daemon} · 待同步 ${status.queueDepth}`;
  pauseButton.textContent = status.settings.paused ? "恢复采集" : "暂停采集";
  pauseButton.classList.toggle("active", status.settings.paused);
  excludeButton.disabled = !status.currentHost;
  flagButton.classList.toggle("active", status.feedback === "flag");
  dismissButton.classList.toggle("active", status.feedback === "dismiss");
  tokenInput.value = status.settings.token;
  daemonUrlInput.value = status.settings.daemonUrl;
  allowInput.value = status.settings.contentAllowHosts.join("\n");
}

function parseHosts(value: string): string[] {
  return [...new Set(value
    .split(/[\n,]/u)
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean)
    .map((item) => {
      try {
        return new URL(item.includes("://") ? item : `https://${item}`).hostname;
      } catch {
        return "";
      }
    })
    .filter(Boolean))];
}

function send(message: RuntimeMessage): Promise<unknown> {
  return browser.runtime.sendMessage(message);
}

function required(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing #${id}`);
  return element;
}
function requiredButton(id: string): HTMLButtonElement { return required(id) as HTMLButtonElement; }
function requiredInput(id: string): HTMLInputElement { return required(id) as HTMLInputElement; }
function requiredTextArea(id: string): HTMLTextAreaElement { return required(id) as HTMLTextAreaElement; }
