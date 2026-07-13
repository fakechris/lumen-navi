export function extractFrame(options = {}) {
  const maxChars = Math.max(1, Number(options.max_chars) || 200000);
  const maxNodes = Math.max(1, Number(options.max_nodes) || 5000);
  let remainingChars = maxChars;
  let visitedNodes = 0;
  let truncated = false;

  function take(value) {
    if (value == null) return null;
    const text = String(value);
    if (text.length <= remainingChars) {
      remainingChars -= text.length;
      return text;
    }
    truncated = true;
    const result = text.slice(0, remainingChars);
    remainingChars = 0;
    return result;
  }

  function rectOf(element) {
    const rect = element?.getBoundingClientRect?.();
    if (!rect || (!rect.width && !rect.height)) return null;
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  }

  function isVisible(element) {
    if (!element || !element.isConnected) return false;
    const style = window.getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden" || (style.opacity !== "" && Number(style.opacity) === 0)) {
      return false;
    }
    const rect = element.getBoundingClientRect();
    return rect.bottom >= 0 && rect.right >= 0 && rect.top <= window.innerHeight && rect.left <= window.innerWidth;
  }

  function labelTexts(element) {
    const labels = [];
    for (const label of element?.labels || []) {
      const text = label.textContent?.trim();
      if (text) labels.push(take(text));
    }
    const aria = element?.getAttribute?.("aria-labelledby");
    if (aria) {
      for (const id of aria.split(/\s+/)) {
        const text = document.getElementById(id)?.textContent?.trim();
        if (text) labels.push(take(text));
      }
    }
    return labels.filter(Boolean);
  }

  function ancestorPath(element) {
    const path = [];
    let current = element?.parentElement;
    while (current && path.length < 8) {
      let descriptor = current.tagName.toLowerCase();
      if (current.id) descriptor += `#${current.id}`;
      const role = current.getAttribute("role");
      if (role) descriptor += `[role=${role}]`;
      path.unshift(descriptor);
      current = current.parentElement;
    }
    return path;
  }

  function siblingText(element, direction) {
    let sibling = direction < 0 ? element?.previousElementSibling : element?.nextElementSibling;
    while (sibling) {
      const text = sibling.textContent?.trim();
      if (text) return take(text.slice(0, 500));
      sibling = direction < 0 ? sibling.previousElementSibling : sibling.nextElementSibling;
    }
    return null;
  }

  function focusedContext() {
    const element = document.activeElement;
    if (!element || element === document.body || element === document.documentElement) return null;
    const tag = element.tagName?.toLowerCase() || null;
    const inputType = tag === "input" ? (element.type || "text").toLowerCase() : null;
    const secure = inputType === "password";
    const editable = tag === "input" || tag === "textarea" || element.isContentEditable;
    const rawValue = secure ? null : tag === "input" || tag === "textarea"
      ? element.value
      : element.isContentEditable ? element.innerText || element.textContent || "" : null;
    const selectionStart = secure ? null : Number.isInteger(element.selectionStart) ? element.selectionStart : null;
    const selectionEnd = secure ? null : Number.isInteger(element.selectionEnd) ? element.selectionEnd : null;
    const value = editable ? take(rawValue) : null;
    return {
      tag,
      input_type: inputType,
      role: element.getAttribute?.("role"),
      aria_label: take(element.getAttribute?.("aria-label")),
      name: take(element.getAttribute?.("name")),
      id: take(element.id || null),
      classes: Array.from(element.classList || []).slice(0, 16),
      placeholder: secure ? null : take(element.getAttribute?.("placeholder")),
      value,
      selection_start: selectionStart,
      selection_end: selectionEnd,
      contenteditable: Boolean(element.isContentEditable),
      disabled: "disabled" in element ? Boolean(element.disabled) : null,
      readonly: "readOnly" in element ? Boolean(element.readOnly) : null,
      bounding_rect: rectOf(element),
      labels: labelTexts(element),
      ancestor_path: ancestorPath(element),
      sibling_before: siblingText(element, -1),
      sibling_after: siblingText(element, 1),
      secure,
      coordinate_space: "viewport_css",
      nearby_before: selectionStart == null || rawValue == null ? null : take(rawValue.slice(Math.max(0, selectionStart - 1000), selectionStart)),
      nearby_after: selectionEnd == null || rawValue == null ? null : take(rawValue.slice(selectionEnd, selectionEnd + 1000))
    };
  }

  function roots() {
    const result = [document];
    const walker = document.createTreeWalker(document, NodeFilter.SHOW_ELEMENT);
    while (walker.nextNode() && result.length < maxNodes) {
      if (walker.currentNode.shadowRoot) result.push(walker.currentNode.shadowRoot);
    }
    return result;
  }

  const blocks = [];
  for (const root of roots()) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      visitedNodes += 1;
      if (visitedNodes > maxNodes || remainingChars === 0) {
        truncated = true;
        break;
      }
      const textNode = walker.currentNode;
      const parent = textNode.parentElement;
      const raw = textNode.nodeValue?.replace(/\s+/g, " ").trim();
      if (!raw || !isVisible(parent)) continue;
      const text = take(raw);
      if (!text) continue;
      blocks.push({
        text,
        source_refs: [`dom:${location.href}:${visitedNodes}`],
        global_bounds: rectOf(parent),
        coordinate_space: "viewport_css",
        semantic_role: parent.getAttribute("role") || parent.tagName.toLowerCase(),
        order: blocks.length,
        confidence: 1,
        duplicate_group_id: null,
        conflict_group_id: null
      });
    }
  }

  const focused = focusedContext();
  const selection = window.getSelection?.()?.toString() || null;
  return {
    page_language: document.documentElement.lang || null,
    selection_text: take(selection),
    focused_element: focused,
    nearby_before: focused?.nearby_before || null,
    nearby_after: focused?.nearby_after || null,
    viewport: {
      width: window.innerWidth,
      height: window.innerHeight,
      scroll_x: window.scrollX,
      scroll_y: window.scrollY,
      device_pixel_ratio: window.devicePixelRatio || 1
    },
    viewport_text_blocks: blocks,
    has_focus: document.hasFocus(),
    visited_nodes: visitedNodes,
    truncated
  };
}
