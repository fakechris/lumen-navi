// LLM provider catalog — vendored from lumen-translation (canonical source:
// lumen-suite contracts). Byte-for-byte copy; do not hand-edit.
import raw from "./provider-catalog.v1.json";

export interface CatalogEndpoint {
  base_url: string;
  notes?: string;
}

export interface CatalogProvider {
  id: string;
  aliases?: string[];
  display_name: { en: string; zh?: string };
  api_style: "openai_compat" | "anthropic" | string;
  region: "cn" | "global" | "both" | "local";
  capabilities: string[];
  endpoints?: { cn?: CatalogEndpoint; global?: CatalogEndpoint; local?: CatalogEndpoint };
  chat_path?: string;
  default_model?: string;
  models?: string[];
  needs_key: boolean;
  auth?: { header: string; value_template: string };
  extra_headers?: Record<string, string>;
  docs_url?: string;
  notes?: string;
}

export interface ProviderPreset {
  id: string;
  label: string;
  /** e.g. "https://api.deepseek.com/v1" (no chat path) */
  baseUrl: string;
  /** Overseas endpoint, only when the provider has both cn + global. */
  overseasBaseUrl?: string;
  apiStyle: "openai_compat" | "anthropic";
  chatPath: string;
  defaultModel: string;
  models: string[];
  needsKey: boolean;
  /** Auth header name (default "Authorization" with Bearer template). */
  authHeader?: string;
  authTemplate?: string;
  extraHeaders?: Record<string, string>;
  docs?: string;
}

function toLabel(p: CatalogProvider): string {
  const en = p.display_name.en;
  const zh = p.display_name.zh;
  if (zh && zh !== en && !zh.includes(en)) return `${en} ${zh}`;
  return en;
}

const CATALOG = (raw as { providers: CatalogProvider[] }).providers;

/** All chat-capable providers (including anthropic native), for UI dropdowns. */
export const CHAT_PROVIDERS: ProviderPreset[] = CATALOG.filter(
  (p) => p.capabilities.includes("chat") && p.region !== "local",
).map((p) => {
  const cn = p.endpoints?.cn?.base_url;
  const global = p.endpoints?.global?.base_url;
  const primary = cn ?? global ?? "";
  return {
    id: p.id,
    label: toLabel(p),
    baseUrl: primary,
    overseasBaseUrl: cn && global && cn !== global ? global : undefined,
    apiStyle: p.api_style === "anthropic" ? "anthropic" : "openai_compat",
    chatPath: p.chat_path ?? "/chat/completions",
    defaultModel: p.default_model ?? p.models?.[0] ?? "",
    models: p.models ?? [],
    needsKey: p.needs_key,
    authHeader: p.auth?.header,
    authTemplate: p.auth?.value_template,
    extraHeaders: p.extra_headers,
    docs: p.docs_url,
  };
});

export function getProvider(id: string): ProviderPreset | undefined {
  return CHAT_PROVIDERS.find((p) => p.id === id);
}

/** Resolve the base URL for a provider + region ("cn" | "global"). */
export function resolveBaseUrl(preset: ProviderPreset, region: string): string {
  if (region === "global" && preset.overseasBaseUrl) return preset.overseasBaseUrl;
  return preset.baseUrl;
}
