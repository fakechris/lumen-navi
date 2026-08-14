//! Scene stack: nested identity for a captured frame.
//!
//! Capture-time title/url define the leaf. AX may add a shell layer (herdr).
//! Title strings that disagree are layers or navigate — never a desync
//! detector. Bind (same window or not) is `window_id` only, and lives in
//! the AX worker, not here.
//!
//! Rules are externalized to `$data_dir/rules/scene_rules.v1.json`, mirroring
//! the category rule engine pattern. Edit the file → next dashboard refresh
//! (30s) picks up changes. No rebuild needed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use url::Url;

const EMBEDDED_RULES: &str = include_str!("../rules/scene_rules.v1.json");

static ACTIVE_RULES: RwLock<Option<Arc<SceneRuleSet>>> = RwLock::new(None);

// ── Rule data model ────────────────────────────────────────────────────

/// JSON file schema for scene rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneRuleFile {
    pub version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub shell_titles: Vec<String>,
    #[serde(default)]
    pub browser_bundles: Vec<String>,
    #[serde(default)]
    pub dev_terminal_bundles: Vec<String>,
    #[serde(default)]
    pub dev_editor_bundles: Vec<String>,
    #[serde(default)]
    pub comm_bundles: Vec<String>,
    #[serde(default)]
    pub loopback_hosts: Vec<String>,
    #[serde(default)]
    pub browser_title_suffixes: Vec<String>,
    #[serde(default)]
    pub known_hosts: HashMap<String, String>,
}

/// Compiled rule set (all strings lowercased at parse time).
#[derive(Debug, Clone)]
pub struct SceneRuleSet {
    pub shell_titles: Vec<String>,
    pub browser_bundles: Vec<String>,
    pub dev_terminal_bundles: Vec<String>,
    pub dev_editor_bundles: Vec<String>,
    pub comm_bundles: Vec<String>,
    pub loopback_hosts: Vec<String>,
    pub browser_title_suffixes: Vec<String>,
    pub known_hosts: HashMap<String, String>,
}

impl SceneRuleSet {
    fn from_file(f: &SceneRuleFile) -> Self {
        Self {
            shell_titles: f.shell_titles.iter().map(|s| s.to_ascii_lowercase()).collect(),
            browser_bundles: f.browser_bundles.clone(),
            dev_terminal_bundles: f.dev_terminal_bundles.clone(),
            dev_editor_bundles: f.dev_editor_bundles.clone(),
            comm_bundles: f.comm_bundles.clone(),
            loopback_hosts: f.loopback_hosts.iter().map(|s| s.to_ascii_lowercase()).collect(),
            browser_title_suffixes: f.browser_title_suffixes.clone(),
            known_hosts: f.known_hosts.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect(),
        }
    }

    fn default_set() -> Self {
        let f: SceneRuleFile = serde_json::from_str(EMBEDDED_RULES).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to parse embedded scene rules; using empty");
            SceneRuleFile {
                version: 1,
                description: String::new(),
                shell_titles: vec!["herdr".into()],
                browser_bundles: vec!["com.apple.Safari".into()],
                dev_terminal_bundles: vec![],
                dev_editor_bundles: vec![],
                comm_bundles: vec![],
                loopback_hosts: vec!["127.0.0.1".into(), "localhost".into()],
                browser_title_suffixes: vec![],
                known_hosts: HashMap::new(),
            }
        });
        Self::from_file(&f)
    }

    fn is_shell_title(&self, title: &str) -> bool {
        let t = norm(title).to_ascii_lowercase();
        self.shell_titles.iter().any(|s| *s == t)
    }

    fn bundle_in(&self, bundle: &str, list: &[String]) -> bool {
        list.iter().any(|p| bundle.starts_with(p))
    }

    fn is_browser(&self, bundle: &str) -> bool {
        self.bundle_in(bundle, &self.browser_bundles)
    }
    fn is_dev_terminal(&self, bundle: &str) -> bool {
        self.bundle_in(bundle, &self.dev_terminal_bundles)
    }
    fn is_dev_editor(&self, bundle: &str) -> bool {
        self.bundle_in(bundle, &self.dev_editor_bundles)
    }
    fn is_comm(&self, bundle: &str) -> bool {
        self.bundle_in(bundle, &self.comm_bundles)
    }
    fn is_loopback(&self, host: &str) -> bool {
        self.loopback_hosts.iter().any(|h| h == host)
    }
}

/// Get the active rule set (lazy-init from embedded defaults).
pub fn active_rules() -> Arc<SceneRuleSet> {
    {
        let guard = ACTIVE_RULES.read().unwrap();
        if let Some(set) = guard.as_ref() {
            return Arc::clone(set);
        }
    }
    let mut guard = ACTIVE_RULES.write().unwrap();
    if guard.is_none() {
        let set = Arc::new(SceneRuleSet::default_set());
        *guard = Some(Arc::clone(&set));
        tracing::info!("scene rules loaded from embedded defaults");
    }
    Arc::clone(guard.as_ref().unwrap())
}

/// Set the active rule set (used by the loader after reading from disk).
pub fn set_active_rules(set: Arc<SceneRuleSet>) {
    let mut guard = ACTIVE_RULES.write().unwrap();
    *guard = Some(set);
}

/// Install + load scene rules from `$data_dir/rules/scene_rules.v1.json`.
/// Seeds the file from embedded defaults if missing. Falls back to embedded
/// on parse error (never bricks).
pub fn install_and_load_scene_rules(data_dir: &Path) {
    let rules_dir = data_dir.join("rules");
    let _ = std::fs::create_dir_all(&rules_dir);
    let rule_path = rules_dir.join("scene_rules.v1.json");

    // Seed if missing.
    if !rule_path.exists() {
        if let Err(e) = std::fs::write(&rule_path, EMBEDDED_RULES) {
            tracing::warn!(error = %e, "failed to seed scene rules file");
            set_active_rules(Arc::new(SceneRuleSet::default_set()));
            return;
        }
        tracing::info!("seeded scene rules to {}", rule_path.display());
    }

    // Load from file, fall back to embedded on error.
    match std::fs::read_to_string(&rule_path) {
        Ok(content) => match serde_json::from_str::<SceneRuleFile>(&content) {
            Ok(f) => {
                let set = Arc::new(SceneRuleSet::from_file(&f));
                tracing::info!(
                    path = %rule_path.display(),
                    browsers = set.browser_bundles.len(),
                    terminals = set.dev_terminal_bundles.len(),
                    known_hosts = set.known_hosts.len(),
                    "scene rules loaded from file"
                );
                set_active_rules(set);
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse scene rules file; using embedded");
                set_active_rules(Arc::new(SceneRuleSet::default_set()));
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "failed to read scene rules file; using embedded");
            set_active_rules(Arc::new(SceneRuleSet::default_set()));
        }
    }
}

/// Reload from disk without reseeding.
pub fn reload_scene_rules_from_dir(data_dir: &Path) {
    install_and_load_scene_rules(data_dir);
}

// ── Scene types (unchanged) ────────────────────────────────────────────

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

// ── Helpers ────────────────────────────────────────────────────────────

pub fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
    let rules = active_rules();
    if rules.is_loopback(&host) || host.ends_with(".local") {
        return Some(host);
    }
    if host.bytes().all(|b| b == b'.' || b.is_ascii_digit()) {
        return Some(host);
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 3 && matches!(parts[0], "www" | "m" | "mail") {
        Some(parts[parts.len() - 2..].join("."))
    } else if parts.len() >= 2 {
        Some(parts[parts.len() - 2..].join("."))
    } else {
        Some(host)
    }
}

pub fn ghostty_leaf(title: &str) -> String {
    let t = norm(title);
    if t.is_empty() || t == "👻" || t == "Ghostty" {
        return if t.is_empty() { "Ghostty".into() } else { t };
    }
    if looks_like_herdr_tab(&t) {
        return t.split(" · ").next().unwrap_or(&t).to_string();
    }
    if let Some((cwd, _)) = t.split_once(" · ") {
        return cwd.to_string();
    }
    t
}

/// Build the leaf label for a browser page.
/// Priority: known_hosts override → registrable domain → stripped title.
pub fn page_leaf(title: &str, url: Option<&str>) -> String {
    let rules = active_rules();
    if let Some(d) = registrable_domain(url) {
        // Check known_hosts override.
        if let Some(display) = rules.known_hosts.get(&d) {
            return display.clone();
        }
        return d;
    }
    let mut t = norm(title);
    for suffix in &rules.browser_title_suffixes {
        if let Some(stripped) = t.strip_suffix(suffix) {
            t = stripped.to_string();
        }
    }
    if t.starts_with("http") {
        return registrable_domain(Some(&t)).unwrap_or(t);
    }
    if t.is_empty() { "browser".into() } else { t }
}

/// Capture-time title wins the leaf. A known shell name on either side
/// becomes `shell`. Never replace a specific capture leaf with a later AX title.
pub fn fuse_titles(capture_title: &str, ax_title: &str) -> (Option<String>, String) {
    let rules = active_rules();
    let cap = norm(capture_title);
    let ax = norm(ax_title);
    let cap_shell = rules.is_shell_title(&cap);
    let ax_shell = rules.is_shell_title(&ax);

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

fn classify_kind(bundle: &str, url: Option<&str>) -> SceneKind {
    let rules = active_rules();
    if rules.is_browser(bundle) {
        if let Some(host) = registrable_domain(url) {
            if rules.is_loopback(&host) {
                return SceneKind::Development;
            }
        }
        return SceneKind::Browser;
    }
    if rules.is_dev_terminal(bundle) || rules.is_dev_editor(bundle) {
        return SceneKind::Development;
    }
    if rules.is_comm(bundle) {
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
    let rules = active_rules();

    if rules.is_browser(bundle) {
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

    if rules.is_dev_terminal(bundle) {
        let (mut shell, leaf) = fuse_titles(capture_title, ax_title);
        if shell.is_none() && looks_like_herdr_tab(capture_title) {
            shell = Some("herdr".into());
        }
        return SceneStack {
            app: app.to_string(),
            bundle: bundle.to_string(),
            kind: SceneKind::Development,
            shell,
            leaf: if leaf.is_empty() { app.to_string() } else { leaf },
        };
    }

    let leaf = {
        let t = norm(capture_title);
        if t.is_empty() { app.to_string() } else { t }
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
    fn safari_kimi_uses_known_host() {
        let s = stack_for(
            "Safari",
            "com.apple.Safari",
            "Kimi AI with K3",
            "",
            Some("https://www.kimi.com/membership/subscription?tab=quota"),
        );
        assert_eq!(s.leaf, "Kimi");
        assert_eq!(s.label(), "Safari → Kimi");
    }

    #[test]
    fn safari_x_uses_known_host() {
        let s = stack_for(
            "Comet",
            "ai.perplexity.comet",
            "Home / X - Comet",
            "",
            Some("https://x.com/home"),
        );
        assert_eq!(s.leaf, "X");
    }

    #[test]
    fn safari_github_uses_known_host() {
        let s = stack_for(
            "Safari",
            "com.apple.Safari",
            "fakechris/lumen-navi",
            "",
            Some("https://github.com/fakechris/lumen-navi"),
        );
        assert_eq!(s.leaf, "GitHub");
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
    }

    #[test]
    fn zcode_sidebar_does_not_steal_leaf() {
        let s = stack_for("ZCode", "dev.zcode.app", "ZCode", "ZCode", None);
        assert_eq!(s.leaf, "ZCode");
        assert!(s.shell.is_none());
    }

    #[test]
    fn external_rule_override_works() {
        // Add a custom known_host at runtime.
        let mut set = (*active_rules()).clone();
        set.known_hosts.insert("example.com".into(), "My Site".into());
        set_active_rules(Arc::new(set));

        let s = stack_for(
            "Safari",
            "com.apple.Safari",
            "Some Page",
            "",
            Some("https://example.com/page"),
        );
        assert_eq!(s.leaf, "My Site");

        // Reset to default.
        set_active_rules(Arc::new(SceneRuleSet::default_set()));
    }
}
