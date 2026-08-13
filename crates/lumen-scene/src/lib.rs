//! Scene stack: nested identity for a captured frame.
//!
//! Capture-time title/url define the leaf. AX may add a shell layer (herdr).
//! Title strings that disagree are layers or navigate — never a desync
//! detector. Bind (same window or not) is `window_id` only, and lives in
//! the AX worker, not here.

use url::Url;

const SHELL_TITLES: &[&str] = &["herdr"];

const BROWSER_BUNDLES: &[&str] = &[
    "com.apple.Safari",
    "ai.perplexity.comet",
    "com.google.Chrome",
    "org.mozilla.firefox",
    "company.thebrowser.dia",
    "at.studio.AsideBrowser",
    "com.microsoft.edgemac",
    "com.brave.Browser",
    "company.thebrowser.Browser",
];

const DEV_TERMINAL: &[&str] = &[
    "com.mitchellh.ghostty",
    "com.googlecode.iterm2",
    "com.apple.Terminal",
    "dev.warp.Warp-Stable",
];

const DEV_EDITOR: &[&str] = &[
    "dev.zcode.app",
    "com.anysphere.sand",
    "com.todesktop.230313mzl4w4u92",
    "com.microsoft.VSCode",
];

const COMM_BUNDLES: &[&str] = &[
    "com.tencent.xinWeChat",
    "com.electron.lark",
    "com.alibaba.DingTalkMac",
];

const LOOPBACK: &[&str] = &["127.0.0.1", "localhost", "0.0.0.0"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneKind {
    Browser,
    Development,
    Communication,
    Other,
}

impl SceneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Development => "development",
            Self::Communication => "communication",
            Self::Other => "other",
        }
    }
}

/// Nested identity. `leaf` is capture-time content; `shell` is mux/chrome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneStack {
    pub app: String,
    pub bundle: String,
    pub kind: SceneKind,
    pub shell: Option<String>,
    pub leaf: String,
}

impl SceneStack {
    pub fn label(&self) -> String {
        let mut parts = vec![self.app.as_str()];
        if let Some(s) = self.shell.as_deref() {
            parts.push(s);
        }
        if !self.leaf.is_empty()
            && self.leaf != self.app
            && self.shell.as_deref() != Some(self.leaf.as_str())
        {
            parts.push(self.leaf.as_str());
        }
        parts.join(" → ")
    }
}

pub fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_shell_title(title: &str) -> bool {
    SHELL_TITLES
        .iter()
        .any(|s| norm(title).eq_ignore_ascii_case(s))
}

/// herdr tab titles: `cwd · prompt · sid`
pub fn looks_like_herdr_tab(title: &str) -> bool {
    let t = norm(title);
    let parts: Vec<&str> = t.split(" · ").collect();
    parts.len() == 3 && !parts[0].is_empty() && !parts[2].is_empty()
}

pub fn registrable_domain(url: Option<&str>) -> Option<String> {
    let raw = url?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = if raw.contains("://") {
        Url::parse(raw).ok()?
    } else {
        Url::parse(&format!("https://{raw}")).ok()?
    };
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    if LOOPBACK.contains(&host.as_str()) || host.ends_with(".local") {
        return Some(host);
    }
    if host.bytes().all(|b| b == b'.' || b.is_ascii_digit()) {
        return Some(host);
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 3 && matches!(parts[0], "www" | "m" | "mail") {
        return Some(parts[parts.len() - 2..].join("."));
    }
    if parts.len() >= 2 {
        Some(parts[parts.len() - 2..].join("."))
    } else {
        Some(host)
    }
}

pub fn ghostty_leaf(title: &str) -> String {
    let t = norm(title);
    if t.is_empty() || t == "👻" || t == "Ghostty" {
        return if t.is_empty() {
            "Ghostty".into()
        } else {
            t
        };
    }
    if looks_like_herdr_tab(&t) {
        return t.split(" · ").next().unwrap_or(&t).to_string();
    }
    if let Some((cwd, _)) = t.split_once(" · ") {
        return cwd.to_string();
    }
    t
}

pub fn page_leaf(title: &str, url: Option<&str>) -> String {
    if let Some(d) = registrable_domain(url) {
        return d;
    }
    let mut t = norm(title);
    for suffix in [" - Comet", " - Safari", " - Chrome", " - Firefox", " - Dia", " - Aside"] {
        if let Some(stripped) = t.strip_suffix(suffix) {
            t = stripped.to_string();
        }
    }
    if t.starts_with("http") {
        return registrable_domain(Some(&t)).unwrap_or(t);
    }
    if t.is_empty() {
        "browser".into()
    } else {
        t
    }
}

/// Capture-time title wins the leaf. A known shell name on either side
/// becomes `shell`. Never replace a specific capture leaf with a later AX title.
pub fn fuse_titles(capture_title: &str, ax_title: &str) -> (Option<String>, String) {
    let cap = norm(capture_title);
    let ax = norm(ax_title);
    let cap_shell = is_shell_title(&cap);
    let ax_shell = is_shell_title(&ax);

    if cap_shell && !ax.is_empty() && !ax_shell {
        return (Some(cap.to_ascii_lowercase()), ghostty_leaf(&ax));
    }
    if ax_shell && !cap.is_empty() && !cap_shell {
        return (Some(ax.to_ascii_lowercase()), ghostty_leaf(&cap));
    }
    if cap_shell {
        return (Some(cap.to_ascii_lowercase()), cap.to_ascii_lowercase());
    }
    if !cap.is_empty() {
        return (None, ghostty_leaf(&cap));
    }
    if ax_shell {
        return (Some(ax.to_ascii_lowercase()), ax.to_ascii_lowercase());
    }
    (None, if ax.is_empty() { String::new() } else { ghostty_leaf(&ax) })
}

fn bundle_matches(bundle: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| bundle.starts_with(p))
}

pub fn classify_kind(bundle: &str, url: Option<&str>) -> SceneKind {
    if bundle_matches(bundle, BROWSER_BUNDLES) {
        if let Some(host) = registrable_domain(url) {
            if LOOPBACK.contains(&host.as_str()) {
                return SceneKind::Development;
            }
        }
        return SceneKind::Browser;
    }
    if bundle_matches(bundle, DEV_TERMINAL) || bundle_matches(bundle, DEV_EDITOR) {
        return SceneKind::Development;
    }
    if bundle_matches(bundle, COMM_BUNDLES) {
        return SceneKind::Communication;
    }
    SceneKind::Other
}

/// Build the stack. Capture-time title/url define the leaf.
pub fn stack_for(
    app: &str,
    bundle: &str,
    capture_title: &str,
    ax_title: &str,
    url: Option<&str>,
) -> SceneStack {
    if bundle_matches(bundle, BROWSER_BUNDLES) {
        let leaf = page_leaf(capture_title, url);
        let kind = classify_kind(bundle, url);
        return SceneStack {
            app: app.to_string(),
            bundle: bundle.to_string(),
            kind,
            shell: None,
            leaf,
        };
    }

    if bundle_matches(bundle, DEV_TERMINAL) {
        let (mut shell, leaf) = fuse_titles(capture_title, ax_title);
        if shell.is_none() && looks_like_herdr_tab(capture_title) {
            shell = Some("herdr".into());
        }
        return SceneStack {
            app: app.to_string(),
            bundle: bundle.to_string(),
            kind: SceneKind::Development,
            shell,
            leaf: if leaf.is_empty() {
                app.to_string()
            } else {
                leaf
            },
        };
    }

    let leaf = {
        let t = norm(capture_title);
        if t.is_empty() {
            app.to_string()
        } else {
            t
        }
    };
    SceneStack {
        app: app.to_string(),
        bundle: bundle.to_string(),
        kind: classify_kind(bundle, url),
        shell: None,
        leaf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_shell_plus_pane_osc() {
        let (shell, leaf) = fuse_titles(
            "source · mac 下怎么解压缩这个自安装exe · dfa2e527-3211-49",
            "herdr",
        );
        assert_eq!(shell.as_deref(), Some("herdr"));
        assert_eq!(leaf, "source");
    }

    #[test]
    fn both_herdr_means_mux_chrome() {
        let (shell, leaf) = fuse_titles("herdr", "herdr");
        assert_eq!(shell.as_deref(), Some("herdr"));
        assert_eq!(leaf, "herdr");
    }

    #[test]
    fn capture_wins_when_both_specific() {
        let (shell, leaf) = fuse_titles(
            "…/source/research/grokbot",
            "staffg_installer · 看一下 claude session jsonl，以及 20 · kimicode-session",
        );
        assert!(shell.is_none());
        assert_eq!(leaf, "…/source/research/grokbot");
    }

    #[test]
    fn ghostty_herdr_nested() {
        let s = stack_for(
            "Ghostty",
            "com.mitchellh.ghostty",
            "source · mac 下怎么解压缩这个自安装exe · dfa2e527-3211-49",
            "herdr",
            None,
        );
        assert_eq!(s.shell.as_deref(), Some("herdr"));
        assert_eq!(s.leaf, "source");
        assert_eq!(s.label(), "Ghostty → herdr → source");
        assert_eq!(s.kind, SceneKind::Development);
    }

    #[test]
    fn ghostty_osc_tab_implies_herdr_shell() {
        let s = stack_for(
            "Ghostty",
            "com.mitchellh.ghostty",
            "writing · 看一下这篇的调研 · a0b45a82-37e5-43",
            "writing · 看一下这篇的调研 · a0b45a82-37e5-43",
            None,
        );
        assert_eq!(s.shell.as_deref(), Some("herdr"));
        assert_eq!(s.leaf, "writing");
        assert_eq!(s.label(), "Ghostty → herdr → writing");
    }

    #[test]
    fn safari_kimi_is_page_not_desync() {
        let s = stack_for(
            "Safari",
            "com.apple.Safari",
            "Kimi AI with K3 | Built for Agentic Coding & Knowledge Work",
            "每日分析OpenAI官方快照变更 — DeepSeek Harness",
            Some("https://www.kimi.com/membership/subscription?tab=quota"),
        );
        assert!(s.shell.is_none());
        assert_eq!(s.leaf, "kimi.com");
        assert_eq!(s.label(), "Safari → kimi.com");
        assert_eq!(s.kind, SceneKind::Browser);
        assert!(!s.leaf.contains("DeepSeek"));
    }

    #[test]
    fn safari_kimi_without_url_keeps_capture_title() {
        let s = stack_for(
            "Safari",
            "com.apple.Safari",
            "Kimi AI with K3 | Built for Agentic Coding & Knowledge Work",
            "每日分析OpenAI官方快照变更 — DeepSeek Harness",
            None,
        );
        assert!(s.leaf.starts_with("Kimi AI"));
        assert!(!s.leaf.contains("DeepSeek"));
    }

    #[test]
    fn loopback_is_development() {
        let s = stack_for(
            "Safari",
            "com.apple.Safari",
            "DeepSeek Harness",
            "",
            Some("http://127.0.0.1:3080/"),
        );
        assert_eq!(s.kind, SceneKind::Development);
        assert_eq!(s.leaf, "127.0.0.1");
    }

    #[test]
    fn zcode_sidebar_does_not_steal_leaf() {
        let s = stack_for("ZCode", "dev.zcode.app", "ZCode", "ZCode", None);
        assert_eq!(s.leaf, "ZCode");
        assert!(s.shell.is_none());
    }
}
