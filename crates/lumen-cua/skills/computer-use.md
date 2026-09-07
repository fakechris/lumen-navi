---
name: computer-use
description: Drive native macOS apps through Lumen Cua's embedded MIT cua-driver. Snapshot AX+PNG, act with delivery_mode background, never steal focus.
---

# computer-use (Lumen Cua)

Call the local MCP server named `computer-use`. Do not shell out to `open`,
`osascript`, `cliclick`, or HID `CGEventPost`.

Every input tool (`click`, `right_click`, `type_text`, `press_key`, `hotkey`,
`scroll`) MUST pass `delivery_mode: "background"` and a `session` label.

Loop:

1. `launch_app({bundle_id})` — idempotent, does not foreground.
2. Pick `window_id` from the returned `windows` array (or `list_windows`).
3. `get_window_state({pid, window_id, session})` — AX tree + screenshot.
4. Act with `element_token` first. If the target has no AX handle, window-local
   screenshot pixels: `click({pid, window_id, x, y, delivery_mode:"background", session})`.
5. Read `effect` (`confirmed` / `unverifiable` / `suspected_noop`). Re-snapshot
   unless `confirmed`.
6. Web content: `page({pid, window_id, action:"get_text"|"query_dom"|"execute_javascript"})`.

Never `bring_to_front`. Never desktop-scope coordinates. Never `cmd+l` in a
browser — open URLs with `launch_app({bundle_id, urls:[...]})`.

Cursor overlay is on when `session` is set. Toggle with
`set_agent_cursor_enabled({session, enabled})`.
