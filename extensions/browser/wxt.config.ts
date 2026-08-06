import { defineConfig } from "wxt";

export default defineConfig({
  manifest: {
    name: "Lumen Navi Browser",
    version: "0.1.0",
    description: "Local, privacy-gated browser observation for Lumen Navi.",
    permissions: ["storage", "unlimitedStorage", "tabs", "webNavigation", "alarms", "scripting"],
    host_permissions: ["http://*/*", "https://*/*"],
    incognito: "not_allowed",
    action: {
      default_title: "Lumen Navi Browser"
    }
  }
});
