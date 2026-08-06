import { describe, expect, it } from "vitest";

import {
  captureAllowed,
  defaultSettings,
  effectiveContentAllowHosts,
  effectiveExcludedHosts,
} from "./settings";

describe("standalone capture gate", () => {
  it("captures without a daemon token", () => {
    expect(captureAllowed(defaultSettings())).toBe(true);
  });

  it("continues locally while a configured daemon is unreachable", () => {
    const settings = {
      ...defaultSettings(),
      token: "paired",
      daemonCaptureKnown: false,
      daemonCaptureAllowed: false,
    };
    expect(captureAllowed(settings)).toBe(true);
  });

  it("honors a reachable daemon privacy gate", () => {
    const settings = {
      ...defaultSettings(),
      token: "paired",
      daemonCaptureKnown: true,
      daemonCaptureAllowed: false,
    };
    expect(captureAllowed(settings)).toBe(false);
  });

  it("always honors the extension pause", () => {
    expect(captureAllowed({ ...defaultSettings(), paused: true })).toBe(false);
  });
});

describe("effective host policy", () => {
  it("merges local and Navi policies without duplicates", () => {
    const settings = {
      ...defaultSettings(),
      contentAllowHosts: ["docs.example.com"],
      daemonContentAllowHosts: ["docs.example.com", "wiki.example.com"],
      excludedHosts: ["mail.example.com"],
      daemonExcludedHosts: ["mail.example.com", "chat.example.com"],
    };
    expect(effectiveContentAllowHosts(settings)).toEqual([
      "docs.example.com",
      "wiki.example.com",
    ]);
    expect(effectiveExcludedHosts(settings)).toEqual([
      "mail.example.com",
      "chat.example.com",
    ]);
  });
});
