import { describe, expect, it } from "vitest";

import { buildVisitRecords, dailyActivity, summarizeDomains } from "./dashboard";
import type { StoredArtifact, StoredObservation } from "./local-store";

function observation(
  id: string,
  visitId: string,
  kind: StoredObservation["kind"],
  ts: string,
  payload: Record<string, unknown>,
): StoredObservation {
  return {
    id,
    visit_id: visitId,
    document_id: `doc-${visitId}`,
    kind,
    ts,
    url: "https://www.example.com/article",
    payload,
    local_stored_at: ts,
    sync_status: "pending",
  };
}

describe("browser dashboard model", () => {
  it("assembles lifecycle events and markdown into one visit", () => {
    const observations = [
      observation("nav", "visit-1", "browser.navigation_committed.v1", "2026-08-05T10:00:00Z", {}),
      observation("doc", "visit-1", "browser.document_ready.v1", "2026-08-05T10:00:02Z", {
        title: "A useful article",
        wordCount: 1_200,
        privacy_gate: "allowed",
        extraction_status: "success",
      }),
      observation("close", "visit-1", "browser.visit_closed.v1", "2026-08-05T10:03:00Z", {
        active_ms: 150_000,
        visible_ms: 170_000,
        max_scroll_ratio: 0.8,
      }),
    ];
    const artifacts: StoredArtifact[] = [{
      event_id: "doc",
      media_type: "text/markdown",
      body: "# Article\n\nUseful text.",
      local_stored_at: "2026-08-05T10:00:02Z",
    }];

    const [visit] = buildVisitRecords(observations, artifacts);
    expect(visit).toMatchObject({
      title: "A useful article",
      domain: "example.com",
      activeMs: 150_000,
      readingTier: "deep",
      privacyGate: "allowed",
      extractionStatus: "success",
      content: "# Article\n\nUseful text.",
    });
  });

  it("summarizes domains by attention rather than raw visit count", () => {
    const observations = [
      observation("a", "visit-1", "browser.visit_closed.v1", "2026-08-05T10:00:00Z", { active_ms: 10_000 }),
      {
        ...observation("b", "visit-2", "browser.visit_closed.v1", "2026-08-05T11:00:00Z", { active_ms: 180_000 }),
        url: "https://docs.example.org/guide",
      },
    ];
    const domains = summarizeDomains(buildVisitRecords(observations, []));
    expect(domains.map((item) => item.domain)).toEqual(["docs.example.org", "example.com"]);
    expect(domains[0]?.deepReads).toBe(1);
  });

  it("builds a continuous activity window", () => {
    const visits = buildVisitRecords([
      observation("a", "visit-1", "browser.visit_closed.v1", "2026-08-04T10:00:00Z", { active_ms: 30_000 }),
    ], []);
    const days = dailyActivity(visits, 3, new Date("2026-08-05T12:00:00"));
    expect(days).toHaveLength(3);
    expect(days.map((item) => item.visits)).toEqual([0, 1, 0]);
  });

  it("does not call a short page deep reading from scroll alone", () => {
    const visits = buildVisitRecords([
      observation("a", "visit-1", "browser.visit_closed.v1", "2026-08-05T10:00:00Z", {
        active_ms: 2_000,
        max_scroll_ratio: 1,
      }),
    ], []);
    expect(visits[0]?.readingTier).toBe("brief");
  });
});
