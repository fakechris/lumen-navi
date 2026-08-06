import {
  buildVisitRecords,
  dailyActivity,
  summarizeDomains,
  type DomainSummary,
  type ReadingTier,
  type VisitRecord,
} from "../../lib/dashboard";
import { localStoreStats, readLocalArchive } from "../../lib/local-store";

const archiveStatus = required("archive-status");
const timelineEl = required("timeline");
const domainsEl = required("domains");
const detailEl = required("detail");
const heatmapEl = required("heatmap");
const searchInput = requiredInput("search");
const tierFilter = requiredSelect("tier-filter");
const rangeFilter = requiredSelect("range-filter");
const clearDomainButton = requiredButton("clear-domain");

let visits: VisitRecord[] = [];
let selectedId: string | undefined;
let selectedDomain = "";

requiredButton("refresh").addEventListener("click", () => void load());
clearDomainButton.addEventListener("click", () => {
  selectedDomain = "";
  render();
});
searchInput.addEventListener("input", render);
tierFilter.addEventListener("change", render);
rangeFilter.addEventListener("change", render);

void load();

async function load() {
  archiveStatus.textContent = "正在读取本机档案…";
  try {
    const [archive, stats] = await Promise.all([readLocalArchive(10_000), localStoreStats()]);
    visits = buildVisitRecords(archive.observations, archive.artifacts);
    archiveStatus.textContent = `本机档案 ${formatNumber(stats.observations)} 条 · ${formatNumber(stats.pendingSync)} 条待同步`;
    renderSummary();
    renderHeatmap();
    render();
  } catch (error) {
    archiveStatus.textContent = "无法读取本机档案";
    timelineEl.replaceChildren(emptyState("读取记录失败", String(error)));
  }
}

function render() {
  renderDomains();
  const filtered = filteredVisits();
  if (!filtered.some((visit) => visit.id === selectedId)) selectedId = filtered[0]?.id;
  renderTimeline(filtered);
  renderDetail(filtered.find((visit) => visit.id === selectedId));
}

function renderSummary() {
  const todayKey = localDateKey(new Date());
  const today = visits.filter((visit) => localDateKey(new Date(visit.lastSeenAt)) === todayKey);
  setText("metric-today", String(today.length));
  setText("metric-deep", String(today.filter((visit) => visit.readingTier === "deep").length));
  setText("metric-content", String(today.filter((visit) => Boolean(visit.content)).length));
  setText("metric-time", formatDuration(today.reduce((total, visit) => total + visit.activeMs, 0)));
}

function renderHeatmap() {
  const activity = dailyActivity(visits, 28);
  const max = Math.max(1, ...activity.map((day) => day.visits));
  heatmapEl.replaceChildren(...activity.map((day) => {
    const cell = document.createElement("div");
    cell.className = "heat-day";
    cell.dataset.level = String(heatLevel(day.visits, max));
    cell.title = `${formatDate(day.date)} · ${day.visits} 个页面 · ${formatDuration(day.activeMs)}`;
    cell.setAttribute("aria-label", cell.title);
    return cell;
  }));
  setText("active-days", `${activity.filter((day) => day.visits > 0).length} 活跃日`);
}

function renderDomains() {
  const domains = summarizeDomains(visits).slice(0, 12);
  const maxAttention = Math.max(1, ...domains.map((domain) => domain.activeMs || domain.visits));
  domainsEl.replaceChildren(...domains.map((domain) => domainButton(domain, maxAttention)));
  clearDomainButton.hidden = !selectedDomain;
}

function domainButton(domain: DomainSummary, maxAttention: number): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "domain-row";
  button.setAttribute("aria-pressed", String(selectedDomain === domain.domain));
  button.style.setProperty("--share", `${Math.max(8, Math.round(((domain.activeMs || domain.visits) / maxAttention) * 100))}%`);
  const name = document.createElement("strong");
  name.textContent = domain.domain;
  const count = document.createElement("span");
  count.textContent = `${domain.visits} · ${formatDuration(domain.activeMs)}`;
  button.append(name, count);
  button.addEventListener("click", () => {
    selectedDomain = selectedDomain === domain.domain ? "" : domain.domain;
    render();
  });
  return button;
}

function filteredVisits(): VisitRecord[] {
  const query = searchInput.value.trim().toLocaleLowerCase();
  const tier = tierFilter.value as ReadingTier | "";
  const range = rangeFilter.value;
  const cutoff = range === "all" ? undefined : Date.now() - Number(range) * 86_400_000;
  return visits.filter((visit) => {
    if (selectedDomain && visit.domain !== selectedDomain) return false;
    if (tier && visit.readingTier !== tier) return false;
    if (cutoff !== undefined && new Date(visit.lastSeenAt).getTime() < cutoff) return false;
    if (query && !`${visit.title}\n${visit.domain}\n${visit.url}`.toLocaleLowerCase().includes(query)) return false;
    return true;
  });
}

function renderTimeline(filtered: VisitRecord[]) {
  if (filtered.length === 0) {
    timelineEl.replaceChildren(emptyState("还没有符合条件的记录", "继续浏览网页，或调整上方筛选条件。"));
    return;
  }
  const groups = new Map<string, VisitRecord[]>();
  for (const visit of filtered) {
    const key = localDateKey(new Date(visit.lastSeenAt));
    groups.set(key, [...(groups.get(key) ?? []), visit]);
  }
  const fragments = [...groups.entries()].map(([date, items], index) => {
    const section = document.createElement("section");
    section.className = "day-group";
    section.style.animationDelay = `${Math.min(index, 5) * 45}ms`;
    const heading = document.createElement("div");
    heading.className = "day-heading";
    const title = document.createElement("h3");
    title.textContent = relativeDate(date);
    const summary = document.createElement("span");
    summary.textContent = `${items.length} 个页面 · ${formatDuration(items.reduce((total, item) => total + item.activeMs, 0))}`;
    heading.append(title, summary);
    section.append(heading, ...items.map(visitButton));
    return section;
  });
  timelineEl.replaceChildren(...fragments);
}

function visitButton(visit: VisitRecord): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "visit-row";
  button.setAttribute("aria-current", String(selectedId === visit.id));
  button.setAttribute("aria-label", `${visit.title}，${tierLabel(visit.readingTier)}，${formatDuration(visit.activeMs)}`);

  const time = document.createElement("span");
  time.className = "visit-time";
  time.textContent = formatTime(visit.lastSeenAt);
  const mark = document.createElement("span");
  mark.className = `tier-mark ${visit.readingTier}`;
  mark.title = tierLabel(visit.readingTier);
  const copy = document.createElement("span");
  copy.className = "visit-copy";
  const title = document.createElement("span");
  title.className = "visit-title";
  title.textContent = visit.title;
  const meta = document.createElement("span");
  meta.className = "visit-meta";
  const domain = document.createElement("span");
  domain.className = "visit-domain";
  domain.textContent = visit.domain;
  const attention = document.createElement("span");
  attention.textContent = `${formatDuration(visit.activeMs)} · 滚动 ${formatPercent(visit.maxScrollRatio)}`;
  meta.append(domain, attention);
  copy.append(title, meta);
  const flags = document.createElement("span");
  flags.className = "visit-flags";
  if (visit.content) flags.append(tag("正文", "content"));
  if (visit.feedback === "flag") flags.append(tag("标记", "flag"));
  button.append(time, mark, copy, flags);
  button.addEventListener("click", () => {
    selectedId = visit.id;
    renderTimeline(filteredVisits());
    renderDetail(visit);
  });
  return button;
}

function renderDetail(visit?: VisitRecord) {
  if (!visit) {
    const empty = document.createElement("div");
    empty.className = "detail-empty";
    empty.append(eyebrow("Source Detail"), heading("选择一条记录", 2), paragraph("查看来源、阅读深度、采集范围和本机保存的正文。"));
    detailEl.replaceChildren(empty);
    return;
  }

  const head = document.createElement("div");
  head.className = "detail-head";
  head.append(eyebrow(`${visit.domain} · ${formatDateTime(visit.lastSeenAt)}`));
  const title = heading(visit.title, 2);
  const link = document.createElement("a");
  link.className = "source-link";
  link.href = visit.url;
  link.target = "_blank";
  link.rel = "noreferrer";
  link.textContent = visit.url;
  head.append(title, link);

  const facts = document.createElement("div");
  facts.className = "detail-facts";
  facts.append(
    fact(formatDuration(visit.activeMs), "有效阅读"),
    fact(formatDuration(visit.visibleMs), "页面可见"),
    fact(formatPercent(visit.maxScrollRatio), "最大滚动"),
    fact(formatNumber(visit.wordCount), "正文词数"),
  );

  const tags = document.createElement("div");
  tags.className = "detail-tags";
  tags.append(
    tag(tierLabel(visit.readingTier), visit.readingTier === "deep" ? "content" : ""),
    tag(visit.content ? "已保存正文" : "仅元数据", visit.content ? "content" : ""),
    tag(syncLabel(visit.syncStatus), visit.syncStatus === "rejected" ? "flag" : ""),
  );
  if (visit.feedback === "flag") tags.append(tag("用户标记", "flag"));
  if (visit.feedback === "dismiss") tags.append(tag("不相关", ""));

  const contentSection = document.createElement("section");
  contentSection.className = "detail-section";
  contentSection.append(heading("采集正文", 3));
  if (visit.content) {
    const content = document.createElement("pre");
    content.className = "captured-content";
    content.textContent = visit.content;
    contentSection.append(content);
  } else {
    const empty = paragraph(
      visit.privacyGate === "allowed"
        ? "这个页面没有提取到可读正文。"
        : "正文采集被隐私策略关闭。本次仅记录访问与关闭时间、前台和焦点时长、最大滚动及导航状态；不会记录点击、输入、复制或选择内容。",
    );
    empty.className = "content-empty";
    contentSection.append(empty);
  }

  detailEl.replaceChildren(head, facts, tags, contentSection);
}

function fact(value: string, label: string): HTMLDivElement {
  const wrapper = document.createElement("div");
  const strong = document.createElement("strong");
  strong.textContent = value;
  const span = document.createElement("span");
  span.textContent = label;
  wrapper.append(strong, span);
  return wrapper;
}

function tag(label: string, tone: string): HTMLSpanElement {
  const element = document.createElement("span");
  element.className = `mini-tag ${tone}`.trim();
  element.textContent = label;
  return element;
}

function eyebrow(value: string): HTMLParagraphElement {
  const element = paragraph(value);
  element.className = "eyebrow";
  return element;
}

function heading(value: string, level: 2 | 3): HTMLHeadingElement {
  const element = document.createElement(`h${level}`);
  element.textContent = value;
  return element;
}

function paragraph(value: string): HTMLParagraphElement {
  const element = document.createElement("p");
  element.textContent = value;
  return element;
}

function emptyState(title: string, body: string): HTMLDivElement {
  const element = document.createElement("div");
  element.className = "empty-state";
  element.append(eyebrow("Local Archive"), heading(title, 3), paragraph(body));
  return element;
}

function heatLevel(count: number, max: number): number {
  if (count === 0) return 0;
  const ratio = count / max;
  if (ratio <= 0.25) return 1;
  if (ratio <= 0.5) return 2;
  if (ratio <= 0.75) return 3;
  return 4;
}

function tierLabel(tier: ReadingTier): string {
  return tier === "deep" ? "深读" : tier === "scan" ? "浏览" : "短访";
}

function syncLabel(status: VisitRecord["syncStatus"]): string {
  return status === "synced" ? "已同步 Navi" : status === "rejected" ? "同步被拒绝" : "等待同步";
}

function formatTime(value: string): string {
  return new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false }).format(new Date(value));
}

function formatDateTime(value: string): string {
  return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }).format(new Date(value));
}

function formatDate(value: string): string {
  return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric" }).format(new Date(`${value}T12:00:00`));
}

function relativeDate(value: string): string {
  const today = localDateKey(new Date());
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  if (value === today) return "今天";
  if (value === localDateKey(yesterday)) return "昨天";
  return formatDate(value);
}

function formatDuration(ms: number): string {
  if (ms < 1_000) return "0 分钟";
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return remainder ? `${hours}时 ${remainder}分` : `${hours} 小时`;
}

function formatPercent(value: number): string {
  return `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%`;
}

function formatNumber(value: number): string {
  return new Intl.NumberFormat("zh-CN").format(value);
}

function localDateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function setText(id: string, value: string) {
  required(id).textContent = value;
}

function required(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing #${id}`);
  return element;
}

function requiredButton(id: string): HTMLButtonElement { return required(id) as HTMLButtonElement; }
function requiredInput(id: string): HTMLInputElement { return required(id) as HTMLInputElement; }
function requiredSelect(id: string): HTMLSelectElement { return required(id) as HTMLSelectElement; }
