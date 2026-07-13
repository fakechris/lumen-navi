import { extractFrame } from "./extractor.js";

const api = globalThis.browser ?? globalThis.chrome;
const SCHEMA_VERSION = 1;

function errorResult(command, code, message, retryable = false) {
  return {
    kind: "capture_result",
    request_id: command?.request_id ?? null,
    ok: false,
    error: { code, message, retryable }
  };
}

function navigationId(tab) {
  return `${tab.id}:${tab.url || tab.pendingUrl || ""}`;
}

function originPattern(url) {
  const parsed = new URL(url);
  return `${parsed.protocol}//${parsed.host}/*`;
}

export function domainMatches(domain, denied) {
  const normalizedDomain = domain.toLowerCase().replace(/\.$/, "");
  const normalizedDenied = String(denied).trim().toLowerCase().replace(/^\*\./, "").replace(/\.$/, "");
  return normalizedDenied && (normalizedDomain === normalizedDenied || normalizedDomain.endsWith(`.${normalizedDenied}`));
}

export function normalizeFrames(frames, fallbackUrl) {
  if (Array.isArray(frames) && frames.length > 0) return frames;
  return [{ frameId: 0, url: fallbackUrl }];
}

export function selectFocusedFrame(extracted, main) {
  return extracted.find(({ frame, result }) =>
    frame.frameId !== 0 && result.result?.has_focus && result.result?.focused_element
  ) || extracted.find(({ result }) => result.result?.has_focus) || main;
}

async function executeFrame(tabId, frameId, maxChars, maxNodes) {
  const results = await api.scripting.executeScript({
    target: { tabId, frameIds: [frameId] },
    func: extractFrame,
    args: [{ max_chars: maxChars, max_nodes: maxNodes }]
  });
  return results[0];
}

async function capture(command, browserName) {
  if (command.schema_version !== SCHEMA_VERSION) {
    return errorResult(command, "schema_unsupported", "browser command schema is unsupported");
  }
  if (Date.now() >= Date.parse(command.deadline)) {
    return errorResult(command, "deadline_elapsed", "browser command deadline elapsed", true);
  }
  const [started] = await api.tabs.query({ active: true, lastFocusedWindow: true });
  if (!started?.id || !started.url) {
    return errorResult(command, "active_tab_unavailable", "no active browser tab", true);
  }
  if (started.incognito && !command.allow_private_browsing) {
    return errorResult(command, "private_browsing_denied", "private browsing capture is disabled");
  }
  let pattern;
  try {
    pattern = originPattern(started.url);
  } catch {
    return errorResult(command, "restricted_page", "the active page does not expose a capturable origin");
  }
  if (!/^https?:/.test(started.url)) {
    return errorResult(command, "restricted_page", "browser-internal pages are not captured");
  }
  const startedUrl = new URL(started.url);
  if ((command.denied_domains || []).some((denied) => domainMatches(startedUrl.hostname, denied))) {
    return errorResult(command, "domain_denied", "the active domain is excluded by local capture policy");
  }
  if (command.target_hint?.bundle_id && (command.denied_bundle_ids || []).includes(command.target_hint.bundle_id)) {
    return errorResult(command, "bundle_denied", "the active browser is excluded by local capture policy");
  }
  const hasPermission = await api.permissions.contains({ origins: [pattern] });
  if (!hasPermission) {
    return errorResult(command, "origin_permission_required", `site permission is required for ${pattern}`);
  }

  const discoveredFrames = api.webNavigation?.getAllFrames
    ? await api.webNavigation.getAllFrames({ tabId: started.id })
    : null;
  const frames = normalizeFrames(discoveredFrames, started.url);
  const frameStatus = [];
  const extracted = [];
  let remainingChars = Math.max(1, Number(command.max_chars) || 200000);
  let remainingNodes = Math.max(1, Number(command.max_nodes) || 5000);
  const orderedFrames = [...(frames || [])].sort((left, right) => left.frameId - right.frameId);
  for (const frame of orderedFrames) {
    if (remainingChars === 0 || remainingNodes === 0) {
      frameStatus.push({
        frame_id: frame.frameId,
        document_id: frame.documentId || null,
        origin: (() => { try { return new URL(frame.url).origin; } catch { return null; } })(),
        captured: false,
        reason: "global_capture_budget_exhausted"
      });
      continue;
    }
    try {
      const result = await executeFrame(started.id, frame.frameId, remainingChars, remainingNodes);
      extracted.push({ frame, result });
      const data = result.result || {};
      const usedChars = JSON.stringify({
        selection_text: data.selection_text,
        focused_element: data.focused_element,
        nearby_before: data.nearby_before,
        nearby_after: data.nearby_after,
        viewport_text_blocks: data.viewport_text_blocks
      }).length;
      remainingChars = Math.max(0, remainingChars - usedChars);
      remainingNodes = Math.max(0, remainingNodes - Number(data.visited_nodes || 0));
      frameStatus.push({
        frame_id: frame.frameId,
        document_id: result.documentId || frame.documentId || null,
        origin: (() => { try { return new URL(frame.url).origin; } catch { return null; } })(),
        captured: true,
        reason: null
      });
    } catch (error) {
      frameStatus.push({
        frame_id: frame.frameId,
        document_id: frame.documentId || null,
        origin: (() => { try { return new URL(frame.url).origin; } catch { return null; } })(),
        captured: false,
        reason: String(error?.message || error)
      });
    }
  }
  const main = extracted.find(({ frame }) => frame.frameId === 0);
  if (!main) {
    return errorResult(command, "main_frame_unavailable", "the main frame could not be captured", true);
  }
  const focused = selectFocusedFrame(extracted, main);
  const identity = await api.scripting.executeScript({
    target: { tabId: started.id, frameIds: [0] },
    func: () => location.href
  });
  const [completed] = await api.tabs.query({ active: true, lastFocusedWindow: true });
  if (!completed?.id || !completed.url) {
    return errorResult(command, "target_stale", "the active browser tab disappeared during capture", true);
  }
  const startedDocumentId = main.result.documentId || null;
  const completedDocumentId = identity[0]?.documentId || null;
  const contextData = main.result.result;
  const focusedData = focused.result.result;
  const parsedUrl = new URL(completed.url);
  const allBlocks = extracted.flatMap(({ result }) => result.result?.viewport_text_blocks || []);
  const truncated = extracted.some(({ result }) => result.result?.truncated);
  const capturedAt = new Date().toISOString();
  return {
    kind: "capture_result",
    request_id: command.request_id,
    ok: true,
    snapshot: {
      schema_version: SCHEMA_VERSION,
      request_id: command.request_id,
      capture_id: command.capture_id,
      target_generation: command.target_generation,
      started_tab_id: started.id,
      completed_tab_id: completed?.id ?? null,
      started_navigation_id: navigationId(started),
      completed_navigation_id: completed ? navigationId(completed) : null,
      started_document_id: startedDocumentId,
      completed_document_id: completedDocumentId,
      context: {
        browser: browserName,
        browser_bundle_id: command.target_hint?.bundle_id || null,
        browser_pid: command.target_hint?.pid || null,
        profile: null,
        incognito: Boolean(completed?.incognito),
        window_id: completed?.windowId ?? null,
        tab_id: completed?.id ?? null,
        frame_id: focused.frame.frameId,
        title: completed?.title || null,
        url: completed?.url || null,
        origin: parsedUrl.origin,
        domain: parsedUrl.hostname,
        navigation_id: completed ? navigationId(completed) : null,
        document_id: completedDocumentId,
        page_language: contextData.page_language,
        selection_text: focusedData.selection_text,
        focused_element: focusedData.focused_element,
        nearby_before: focusedData.nearby_before,
        nearby_after: focusedData.nearby_after,
        viewport: contextData.viewport,
        viewport_text_blocks: allBlocks,
        captured_at: capturedAt,
        permission_scope: pattern,
        truncated
      },
      frame_status: frameStatus,
      captured_at: capturedAt,
      extension_version: api.runtime.getManifest().version
    }
  };
}

export function startNativeBridge(nativeHost, browserName) {
  let reconnectDelay = 250;

  function connect() {
    let port;
    try {
      port = api.runtime.connectNative(nativeHost);
    } catch {
      const delay = reconnectDelay;
      reconnectDelay = Math.min(reconnectDelay * 2, 30000);
      setTimeout(connect, delay);
      return;
    }
    port.onMessage.addListener(async (command) => {
      reconnectDelay = 250;
      try {
        port.postMessage(await capture(command, browserName));
      } catch (error) {
        port.postMessage(errorResult(command, "extractor_failed", String(error?.message || error), true));
      }
    });
    port.onDisconnect.addListener(() => {
      const delay = reconnectDelay;
      reconnectDelay = Math.min(reconnectDelay * 2, 30000);
      setTimeout(connect, delay);
    });
  }

  connect();
}

export function startSafariNativeBridge(applicationId, browserName) {
  let reconnectDelay = 250;

  function connect() {
    let port;
    try {
      port = api.runtime.connectNative(applicationId);
    } catch {
      const delay = reconnectDelay;
      reconnectDelay = Math.min(reconnectDelay * 2, 30000);
      setTimeout(connect, delay);
      return;
    }
    port.onMessage.addListener(async (message) => {
      reconnectDelay = 250;
      const command = message?.userInfo?.command ?? message?.command ?? message;
      let result;
      try {
        result = await capture(command, browserName);
      } catch (error) {
        result = errorResult(command, "extractor_failed", String(error?.message || error), true);
      }
      try {
        await api.runtime.sendNativeMessage(applicationId, result);
      } catch {
        // The containing app owns retry and request deadlines. Reconnect only when Safari
        // explicitly closes the command port.
      }
    });
    port.onDisconnect.addListener(() => {
      const delay = reconnectDelay;
      reconnectDelay = Math.min(reconnectDelay * 2, 30000);
      setTimeout(connect, delay);
    });
  }

  connect();
}
