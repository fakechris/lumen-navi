//! Act stop-lines. Daemon sanitizer and Cua replay both consult these.

pub fn is_terminal_or_ide(app: &str, bundle_id: Option<&str>) -> bool {
    let app_l = app.to_ascii_lowercase();
    // Substrings — never a bare "code" (matches "QR Code", "Barcode", …).
    const APP_NAMES: &[&str] = &[
        "terminal",
        "iterm",
        "iterm2",
        "warp",
        "kitty",
        "ghostty",
        "alacritty",
        "cursor",
        "visual studio code",
        "vscode",
        "zed",
        "intellij idea",
        "goland",
        "webstorm",
        "pycharm",
        "xcode",
    ];
    if APP_NAMES.iter().any(|n| app_l.contains(n)) {
        return true;
    }
    // VS Code's display name is often just "Code".
    if app_l == "code" {
        return true;
    }
    let Some(bundle) = bundle_id else {
        return false;
    };
    const BUNDLES: &[&str] = &[
        "com.apple.Terminal",
        "com.googlecode.iterm2",
        "dev.warp.Warp-Stable",
        "net.kovidgoyal.kitty",
        "com.mitchellh.ghostty",
        "org.alacritty",
        "com.todesktop.230313mzl4w4u92",
        "com.microsoft.VSCode",
        "dev.zed.Zed",
        "com.apple.dt.Xcode",
    ];
    BUNDLES.iter().any(|b| bundle.eq_ignore_ascii_case(b))
}

pub fn irreversible_text(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "付款",
        "支付",
        "下单",
        "提交订单",
        "删除",
        "清空回收站",
        "覆盖保存",
        "同意条款",
        "发布到",
        "pay now",
        "place order",
        "delete permanently",
        "empty trash",
        "overwrite",
        "i agree",
    ];
    NEEDLES.iter().any(|n| t.contains(&n.to_ascii_lowercase()))
}

pub fn dangerous_shortcut(keys: Option<&str>) -> bool {
    let Some(keys) = keys else {
        return false;
    };
    let k = keys.to_ascii_lowercase().replace(' ', "");
    matches!(
        k.as_str(),
        "cmd+q"
            | "command+q"
            | "cmd+shift+q"
            | "command+shift+q"
            | "cmd+alt+esc"
            | "command+option+escape"
    )
}

pub fn refuse_replay_step(
    action: &str,
    app: &str,
    bundle_id: Option<&str>,
    keys: Option<&str>,
    extra: &[&str],
) -> Option<&'static str> {
    if extra.iter().any(|s| irreversible_text(s)) {
        return Some("stop_line");
    }
    if dangerous_shortcut(keys) {
        return Some("dangerous_shortcut");
    }
    if action == "submit" && is_terminal_or_ide(app, bundle_id) {
        return Some("terminal_submit");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_submit_is_refused() {
        assert_eq!(
            refuse_replay_step(
                "submit",
                "Terminal",
                Some("com.apple.Terminal"),
                Some("return"),
                &[]
            ),
            Some("terminal_submit")
        );
        assert!(refuse_replay_step(
            "submit",
            "Safari",
            Some("com.apple.Safari"),
            Some("return"),
            &[]
        )
        .is_none());
    }

    #[test]
    fn payment_copy_is_a_stop_line() {
        assert_eq!(
            refuse_replay_step("click", "Safari", None, None, &["确认付款"]),
            Some("stop_line")
        );
    }

    #[test]
    fn quit_shortcut_is_refused() {
        assert_eq!(
            refuse_replay_step("shortcut", "TextEdit", None, Some("command+q"), &[]),
            Some("dangerous_shortcut")
        );
    }

    #[test]
    fn vscode_is_ide_but_qr_code_is_not() {
        assert!(is_terminal_or_ide("Visual Studio Code", None));
        assert!(is_terminal_or_ide("Code", Some("com.microsoft.VSCode")));
        assert!(is_terminal_or_ide("Code", None));
        assert!(is_terminal_or_ide("VSCode", None));
        assert!(!is_terminal_or_ide("QR Code", None));
        assert!(!is_terminal_or_ide("Barcode Scanner", None));
        assert!(!is_terminal_or_ide("Safari", None));
    }
}
