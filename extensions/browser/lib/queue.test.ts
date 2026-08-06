import { describe, expect, it } from "vitest";

import { acknowledge, appendObservation, buildBatch, discardObservations } from "./queue";
import type { BrowserObservation, QueueState, Settings } from "./types";

const settings: Settings = {
  installationId: "00000000-0000-4000-8000-000000000001",
  daemonUrl: "http://127.0.0.1:7420",
  token: "fixture-token",
  paused: false,
  daemonCaptureAllowed: true,
  daemonCaptureKnown: true,
  contentAllowHosts: [],
  daemonContentAllowHosts: [],
  excludedHosts: [],
  daemonExcludedHosts: [],
  batchSize: 2,
  flushIntervalMs: 30_000,
  maxQueueSize: 2,
  maxQueueBytes: 64,
  maxArtifactBytes: 64,
  captureProfileVersion: "browser-mvp-v1",
};

function event(id: string): BrowserObservation {
  return {
    id,
    kind: "browser.health.v1",
    ts: "2026-08-05T00:00:00.000Z",
    visit_id: "00000000-0000-4000-8000-000000000001",
    payload: {},
  };
}

describe("persistent browser queue", () => {
  it("caps observations and exposes a durable gap record", async () => {
    let state: QueueState = { observations: [], artifacts: [], droppedEvents: 0 };
    state = appendObservation(state, event("00000000-0000-4000-8000-000000000101"), undefined, 2, 64);
    state = appendObservation(state, event("00000000-0000-4000-8000-000000000102"), undefined, 2, 64);
    state = appendObservation(state, event("00000000-0000-4000-8000-000000000103"), undefined, 2, 64);

    expect(state.observations.map((item) => item.id)).toEqual([
      "00000000-0000-4000-8000-000000000102",
      "00000000-0000-4000-8000-000000000103",
    ]);
    expect(state.droppedEvents).toBe(1);
    expect(state.pendingGap?.kind).toBe("browser.gap.v1");

    const batch = await buildBatch(state, settings, "fixture-config-hash");
    expect(batch.observations[0]?.kind).toBe("browser.gap.v1");

    state = acknowledge(state, batch.observations.map((item) => item.id));
    expect(state.pendingGap).toBeUndefined();
  });

  it("caps queued markdown by byte size", () => {
    let state: QueueState = { observations: [], artifacts: [], droppedEvents: 0 };
    state = appendObservation(
      state,
      event("00000000-0000-4000-8000-000000000111"),
      {
        event_id: "00000000-0000-4000-8000-000000000111",
        media_type: "text/markdown",
        body: "a".repeat(40),
      },
      10,
      64,
    );
    state = appendObservation(
      state,
      event("00000000-0000-4000-8000-000000000112"),
      {
        event_id: "00000000-0000-4000-8000-000000000112",
        media_type: "text/markdown",
        body: "b".repeat(40),
      },
      10,
      64,
    );

    expect(state.observations.map((item) => item.id)).toEqual([
      "00000000-0000-4000-8000-000000000112",
    ]);
    expect(state.droppedEvents).toBe(1);
  });

  it("drops a rejected batch without losing its gap explanation", () => {
    const first = event("00000000-0000-4000-8000-000000000121");
    const second = event("00000000-0000-4000-8000-000000000122");
    const state: QueueState = {
      observations: [first, second],
      artifacts: [],
      droppedEvents: 0,
    };
    const next = discardObservations(state, [first.id], "capture_gate");
    expect(next.observations).toEqual([second]);
    expect(next.droppedEvents).toBe(1);
    expect(next.pendingGap?.payload).toEqual({
      dropped_events: 1,
      reason: "capture_gate",
    });
  });
});
