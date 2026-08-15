//! Coalesce raw HID samples into Observe interaction events.
//!
//! Text is buffered until idle, submit, shortcut, or app switch. Mouse down/up
//! becomes a click, context menu, or drag depending on button and distance.

use std::time::{Duration, Instant};

use lumen_platform::{ObserveHidEvent, ObserveHidKind};
use lumen_types::{event_kind, SourceEvent, SourceKind};
use serde_json::json;
use uuid::Uuid;

const TEXT_IDLE: Duration = Duration::from_millis(450);
const DRAG_THRESHOLD_PX: f64 = 8.0;

#[derive(Debug, Clone)]
pub struct InteractionContext {
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
    pub session_id: Option<Uuid>,
}

#[derive(Debug, Default)]
pub struct InteractionCoalescer {
    text: String,
    text_ctx: Option<InteractionContext>,
    last_text_at: Option<Instant>,
    drag: Option<DragInProgress>,
}

#[derive(Debug, Clone)]
struct DragInProgress {
    button: u8,
    origin_x: f64,
    origin_y: f64,
    click_count: u32,
}

impl InteractionCoalescer {
    pub fn push(
        &mut self,
        raw: ObserveHidEvent,
        ctx: InteractionContext,
        now: Instant,
    ) -> Vec<SourceEvent> {
        let mut out = Vec::new();
        match raw.kind {
            ObserveHidKind::KeyDown => {
                let unicode = raw.unicode;
                let keycode = raw.keycode;
                let command = raw.command;
                let control = raw.control;
                let shift = raw.shift;
                let option = raw.option;
                let mods = modifiers(command, control, shift, option);
                if is_submit(keycode) && !command && !control {
                    out.extend(self.flush_text(now));
                    out.push(attach(
                        event_kind::KEYBOARD_SUBMIT_V1,
                        &ctx,
                        json!({
                            "payload_version": 1,
                            "keyboard": {
                                "key_equivalent": "return",
                                "modifiers": mods,
                            }
                        }),
                    ));
                } else if command || control {
                    out.extend(self.flush_text(now));
                    if let Some(name) = shortcut_name(keycode) {
                        out.push(attach(
                            event_kind::KEYBOARD_SHORTCUT_V1,
                            &ctx,
                            json!({
                                "payload_version": 1,
                                "keyboard": {
                                    "key_equivalent": name,
                                    "modifiers": mods,
                                }
                            }),
                        ));
                    }
                } else if let Some(ch) = unicode.filter(|s| !s.is_empty() && s.chars().any(|c| !c.is_control()))
                {
                    if self.text_ctx.as_ref().map(|c| c.bundle_id.as_deref())
                        != Some(ctx.bundle_id.as_deref())
                    {
                        out.extend(self.flush_text(now));
                    }
                    self.text.push_str(&ch);
                    self.text_ctx = Some(ctx);
                    self.last_text_at = Some(now);
                }
            }
            ObserveHidKind::MouseDown => {
                self.drag = Some(DragInProgress {
                    button: raw.button,
                    origin_x: raw.x,
                    origin_y: raw.y,
                    click_count: raw.click_count,
                });
            }
            ObserveHidKind::MouseUp => {
                let button = raw.button;
                let x = raw.x;
                let y = raw.y;
                out.extend(self.flush_text(now));
                if let Some(drag) = self.drag.take() {
                    let dx = x - drag.origin_x;
                    let dy = y - drag.origin_y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= DRAG_THRESHOLD_PX {
                        out.push(attach(
                            event_kind::MOUSE_DRAG_V1,
                            &ctx,
                            json!({
                                "payload_version": 1,
                                "mouse": {
                                    "button": button_name(drag.button),
                                    "click_count": drag.click_count,
                                    "origin": {"x": drag.origin_x, "y": drag.origin_y},
                                    "destination": {"x": x, "y": y},
                                }
                            }),
                        ));
                    } else if button == 1 {
                        out.push(attach(
                            event_kind::MOUSE_CONTEXT_MENU_V1,
                            &ctx,
                            json!({
                                "payload_version": 1,
                                "mouse": { "button": "right", "click_count": drag.click_count }
                            }),
                        ));
                    } else {
                        out.push(attach(
                            event_kind::MOUSE_CLICK_V1,
                            &ctx,
                            json!({
                                "payload_version": 1,
                                "mouse": {
                                    "button": button_name(button),
                                    "click_count": drag.click_count,
                                    "x": x,
                                    "y": y,
                                }
                            }),
                        ));
                    }
                }
            }
        }
        out
    }

    pub fn flush_due(&mut self, now: Instant) -> Vec<SourceEvent> {
        match self.last_text_at {
            Some(t) if now.duration_since(t) >= TEXT_IDLE => self.flush_text(now),
            _ => Vec::new(),
        }
    }

    fn flush_text(&mut self, _now: Instant) -> Vec<SourceEvent> {
        if self.text.is_empty() {
            self.last_text_at = None;
            return Vec::new();
        }
        let ctx = self.text_ctx.take().unwrap_or(InteractionContext {
            app_name: None,
            bundle_id: None,
            window_title: None,
            url: None,
            session_id: None,
        });
        let text = std::mem::take(&mut self.text);
        self.last_text_at = None;
        vec![attach(
            event_kind::KEYBOARD_TEXT_INPUT_V1,
            &ctx,
            json!({
                "payload_version": 1,
                "keyboard": { "text": text }
            }),
        )]
    }
}

fn attach(kind: &str, ctx: &InteractionContext, mut payload: serde_json::Value) -> SourceEvent {
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("app_name".into(), json!(ctx.app_name));
        obj.insert("bundle_id".into(), json!(ctx.bundle_id));
        obj.insert("window_title".into(), json!(ctx.window_title));
        obj.insert("url".into(), json!(ctx.url));
    }
    let mut ev = SourceEvent::new(SourceKind::Screen, kind, payload);
    ev.session_id = ctx.session_id;
    ev
}

fn is_submit(keycode: u32) -> bool {
    keycode == 0x24 || keycode == 0x4C
}

fn shortcut_name(keycode: u32) -> Option<&'static str> {
    Some(match keycode {
        0x08 => "c",
        0x09 => "v",
        0x07 => "x",
        0x06 => "z",
        0x00 => "a",
        0x01 => "s",
        0x03 => "f",
        0x0D => "w",
        0x2D => "n",
        0x11 => "t",
        _ => return None,
    })
}

fn modifiers(command: bool, control: bool, shift: bool, option: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if command {
        out.push("command");
    }
    if control {
        out.push("control");
    }
    if shift {
        out.push("shift");
    }
    if option {
        out.push("option");
    }
    out
}

fn button_name(button: u8) -> &'static str {
    match button {
        0 => "left",
        1 => "right",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> InteractionContext {
        InteractionContext {
            app_name: Some("Safari".into()),
            bundle_id: Some("com.apple.Safari".into()),
            window_title: Some("Home".into()),
            url: Some("https://example.com".into()),
            session_id: None,
        }
    }

    fn key(unicode: &str, keycode: u32) -> ObserveHidEvent {
        ObserveHidEvent {
            kind: ObserveHidKind::KeyDown,
            keycode,
            unicode: Some(unicode.into()),
            command: false,
            control: false,
            shift: false,
            option: false,
            button: 0,
            x: 0.0,
            y: 0.0,
            click_count: 1,
        }
    }

    #[test]
    fn coalesces_text_until_idle() {
        let mut c = InteractionCoalescer::default();
        let t0 = Instant::now();
        let evs = c.push(key("h", 4), ctx(), t0);
        assert!(evs.is_empty());
        c.push(key("i", 34), ctx(), t0);
        let flushed = c.flush_due(t0 + Duration::from_millis(500));
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].kind, event_kind::KEYBOARD_TEXT_INPUT_V1);
        assert_eq!(flushed[0].payload["keyboard"]["text"], "hi");
    }

    #[test]
    fn enter_emits_submit_after_text() {
        let mut c = InteractionCoalescer::default();
        let t0 = Instant::now();
        c.push(key("ok", 31), ctx(), t0);
        let evs = c.push(
            ObserveHidEvent {
                kind: ObserveHidKind::KeyDown,
                keycode: 0x24,
                unicode: None,
                command: false,
                control: false,
                shift: false,
                option: false,
                button: 0,
                x: 0.0,
                y: 0.0,
                click_count: 1,
            },
            ctx(),
            t0,
        );
        assert_eq!(evs[0].kind, event_kind::KEYBOARD_TEXT_INPUT_V1);
        assert_eq!(evs[1].kind, event_kind::KEYBOARD_SUBMIT_V1);
    }

    #[test]
    fn cmd_c_is_shortcut() {
        let mut c = InteractionCoalescer::default();
        let evs = c.push(
            ObserveHidEvent {
                kind: ObserveHidKind::KeyDown,
                keycode: 0x08,
                unicode: Some("c".into()),
                command: true,
                control: false,
                shift: false,
                option: false,
                button: 0,
                x: 0.0,
                y: 0.0,
                click_count: 1,
            },
            ctx(),
            Instant::now(),
        );
        assert_eq!(evs[0].kind, event_kind::KEYBOARD_SHORTCUT_V1);
        assert_eq!(evs[0].payload["keyboard"]["key_equivalent"], "c");
    }

    #[test]
    fn left_up_is_click_right_is_context_menu_move_is_drag() {
        let mut c = InteractionCoalescer::default();
        let t0 = Instant::now();
        fn mouse(kind: ObserveHidKind, button: u8, x: f64, y: f64) -> ObserveHidEvent {
            ObserveHidEvent {
                kind,
                keycode: 0,
                unicode: None,
                command: false,
                control: false,
                shift: false,
                option: false,
                button,
                x,
                y,
                click_count: 1,
            }
        }
        c.push(mouse(ObserveHidKind::MouseDown, 0, 10.0, 10.0), ctx(), t0);
        let click = c.push(mouse(ObserveHidKind::MouseUp, 0, 11.0, 10.0), ctx(), t0);
        assert_eq!(click[0].kind, event_kind::MOUSE_CLICK_V1);

        c.push(mouse(ObserveHidKind::MouseDown, 1, 10.0, 10.0), ctx(), t0);
        let menu = c.push(mouse(ObserveHidKind::MouseUp, 1, 10.0, 10.0), ctx(), t0);
        assert_eq!(menu[0].kind, event_kind::MOUSE_CONTEXT_MENU_V1);

        c.push(mouse(ObserveHidKind::MouseDown, 0, 0.0, 0.0), ctx(), t0);
        let drag = c.push(mouse(ObserveHidKind::MouseUp, 0, 40.0, 0.0), ctx(), t0);
        assert_eq!(drag[0].kind, event_kind::MOUSE_DRAG_V1);
    }
}
