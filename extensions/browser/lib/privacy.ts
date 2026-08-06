const SENSITIVE_QUERY_KEYS = new Set([
  "token",
  "access_token",
  "refresh_token",
  "session",
  "session_id",
  "auth",
  "authorization",
  "code",
  "email",
  "api_key",
  "apikey",
  "key",
  "signature",
  "sig",
]);

const SENSITIVE_PATH_PARTS = new Set([
  "inbox",
  "messages",
  "message",
  "dm",
  "settings",
  "admin",
]);

export interface SanitizedUrl {
  url: string;
  host: string;
  removedQueryKeys: string[];
  sensitivePath: boolean;
}

export interface PageSignals {
  hasPasswordInput?: boolean;
  hasEmailInput?: boolean;
  hasContenteditable?: boolean;
  noindex?: boolean;
}

export interface PageDecision {
  observe: boolean;
  contentAllowed: boolean;
  reason: string;
  sanitized?: SanitizedUrl;
}

export function sanitizeUrl(raw: string): SanitizedUrl | undefined {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return undefined;
  }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return undefined;
  const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (isLocalHost(host)) return undefined;

  const removedQueryKeys: string[] = [];
  for (const key of [...url.searchParams.keys()]) {
    if (!SENSITIVE_QUERY_KEYS.has(key.toLowerCase())) continue;
    if (!removedQueryKeys.includes(key)) removedQueryKeys.push(key);
    url.searchParams.delete(key);
  }
  url.hash = "";
  const sensitivePath = url.pathname
    .toLowerCase()
    .split("/")
    .some((part) => SENSITIVE_PATH_PARTS.has(part));

  return { url: url.toString(), host, removedQueryKeys, sensitivePath };
}

export function evaluatePage(
  rawUrl: string,
  contentAllowHosts: string[],
  excludedHosts: string[],
  signals: PageSignals,
): PageDecision {
  const sanitized = sanitizeUrl(rawUrl);
  if (!sanitized) return { observe: false, contentAllowed: false, reason: "unsafe_url" };
  if (hostMatchesAny(sanitized.host, excludedHosts)) {
    return { observe: false, contentAllowed: false, reason: "excluded_host", sanitized };
  }

  const contentAllowed =
    hostMatchesAny(sanitized.host, contentAllowHosts) &&
    !sanitized.sensitivePath &&
    !signals.hasPasswordInput &&
    !signals.hasContenteditable &&
    !signals.noindex;
  return {
    observe: true,
    contentAllowed,
    reason: contentAllowed ? "allowed" : "metadata_only",
    sanitized,
  };
}

export function hostMatchesAny(host: string, patterns: string[]): boolean {
  const normalizedHost = host.toLowerCase();
  return patterns.some((value) => {
    const pattern = value.trim().replace(/^\./, "").toLowerCase();
    return pattern.length > 0 &&
      (normalizedHost === pattern || normalizedHost.endsWith(`.${pattern}`));
  });
}

function isLocalHost(host: string): boolean {
  if (
    host === "localhost" ||
    !host.includes(".") ||
    [".local", ".lan", ".home", ".internal", ".corp"].some((suffix) => host.endsWith(suffix))
  ) return true;
  if (/^127\./.test(host) || /^10\./.test(host) || /^192\.168\./.test(host)) return true;
  if (/^::ffff:/i.test(host)) return true;
  const private172 = host.match(/^172\.(\d{1,3})\./);
  if (private172 && Number(private172[1]) >= 16 && Number(private172[1]) <= 31) return true;
  return host === "::1" || host.startsWith("fc") || host.startsWith("fd") || host.startsWith("fe80:");
}
