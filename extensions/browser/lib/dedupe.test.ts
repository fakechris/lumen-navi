import { describe, expect, it } from "vitest";

import { isUnchangedReload } from "./dedupe";

describe("reload deduplication", () => {
  it("suppresses only reloads with an established unchanged fingerprint", () => {
    expect(isUnchangedReload("same", "same")).toBe(true);
    expect(isUnchangedReload("old", "new")).toBe(false);
    expect(isUnchangedReload(undefined, "new")).toBe(false);
  });
});
