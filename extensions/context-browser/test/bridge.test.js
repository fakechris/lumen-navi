import assert from "node:assert/strict";
import test from "node:test";

import { normalizeFrames, selectFocusedFrame } from "../src/bridge.js";

test("empty frame discovery falls back to the active tab main frame", () => {
  assert.deepEqual(normalizeFrames([], "https://example.test/editor"), [
    { frameId: 0, url: "https://example.test/editor" }
  ]);
});

test("discovered frames are preserved", () => {
  const frames = [{ frameId: 0, url: "https://example.test/" }, { frameId: 4, url: "https://child.example.test/" }];
  assert.equal(normalizeFrames(frames, "https://fallback.test/"), frames);
});

test("focused child frame wins when parent document also reports focus", () => {
  const main = {
    frame: { frameId: 0 },
    result: { result: { has_focus: true, focused_element: { tag: "iframe" } } }
  };
  const child = {
    frame: { frameId: 7 },
    result: { result: { has_focus: true, focused_element: { tag: "textarea" } } }
  };

  assert.equal(selectFocusedFrame([main, child], main), child);
});
