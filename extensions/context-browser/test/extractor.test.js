import assert from "node:assert/strict";
import test from "node:test";

import { JSDOM } from "jsdom";

import { extractFrame } from "../src/extractor.js";
import { domainMatches } from "../src/bridge.js";

function installDom(html) {
  const dom = new JSDOM(html, { url: "https://docs.example.test/editor", pretendToBeVisual: true });
  const previous = {};
  for (const name of ["window", "document", "location", "NodeFilter"]) {
    previous[name] = globalThis[name];
    globalThis[name] = dom.window[name];
  }
  Object.defineProperty(dom.window.HTMLElement.prototype, "isContentEditable", {
    configurable: true,
    get() { return this.getAttribute("contenteditable") === "true"; }
  });
  dom.window.HTMLElement.prototype.getBoundingClientRect = function () {
    return { x: 10, y: 20, left: 10, top: 20, right: 210, bottom: 60, width: 200, height: 40 };
  };
  Object.defineProperty(dom.window.document, "hasFocus", { value: () => true });
  return () => {
    for (const [name, value] of Object.entries(previous)) {
      if (value === undefined) delete globalThis[name];
      else globalThis[name] = value;
    }
    dom.window.close();
  };
}

test("captures textarea value, selection, labels, nearby text and viewport blocks", () => {
  const restore = installDom(`
    <main><p>Visible instructions</p><form>
      <label for="message">Message</label>
      <textarea id="message" name="message">before selected after</textarea>
      <button>Send</button>
    </form></main>
  `);
  try {
    const textarea = document.getElementById("message");
    textarea.focus();
    textarea.setSelectionRange(7, 15);
    const result = extractFrame({ max_chars: 10000, max_nodes: 100 });
    assert.equal(result.focused_element.tag, "textarea");
    assert.equal(result.focused_element.value, "before selected after");
    assert.equal(result.focused_element.selection_start, 7);
    assert.equal(result.focused_element.selection_end, 15);
    assert.deepEqual(result.focused_element.labels, ["Message"]);
    assert.equal(result.nearby_before, "before ");
    assert.equal(result.nearby_after, " after");
    assert.ok(result.viewport_text_blocks.some((block) => block.text === "Visible instructions"));
  } finally {
    restore();
  }
});

test("never returns a password value and marks bounded truncation", () => {
  const restore = installDom(`<input id="secret" type="password" value="do-not-capture"><p>${"x".repeat(200)}</p>`);
  try {
    const input = document.getElementById("secret");
    input.focus();
    const result = extractFrame({ max_chars: 20, max_nodes: 100 });
    assert.equal(result.focused_element.value, null);
    assert.equal(result.focused_element.placeholder, null);
    assert.equal(result.focused_element.secure, true);
    assert.equal(result.truncated, true);
  } finally {
    restore();
  }
});

test("walks open shadow roots", () => {
  const restore = installDom(`<div id="host"></div>`);
  try {
    const root = document.getElementById("host").attachShadow({ mode: "open" });
    root.innerHTML = `<p>Text inside shadow DOM</p>`;
    const result = extractFrame({ max_chars: 1000, max_nodes: 100 });
    assert.ok(result.viewport_text_blocks.some((block) => block.text === "Text inside shadow DOM"));
  } finally {
    restore();
  }
});

test("denylist matching covers exact domains and subdomains only", () => {
  assert.equal(domainMatches("docs.example.test", "example.test"), true);
  assert.equal(domainMatches("example.test", "*.example.test"), true);
  assert.equal(domainMatches("notexample.test", "example.test"), false);
});
