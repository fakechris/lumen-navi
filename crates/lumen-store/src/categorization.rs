//! Deterministic activity → (category, productivity_level) classifier.
//!
//! Priority (first match wins):
//! 1. User rules (`kv` table, Timing-style overrides)
//! 2. Built-in default catalog (bundle / name / domain)
//! 3. System `LSApplicationCategoryType` hint (from Info.plist, when present)
//! 4. Resolved identity cache (Homebrew / iTunes enrichment)
//! 5. Known product-family heuristics (e.g. `com.lumenopen.*`)
//!
//! No ML. Unclassified activities get `None` and are **excluded from the
//! pulse-score denominator**. Async enrichment fills the cache later and
//! re-applies to historical segments.

use serde::{Deserialize, Serialize};

/// Productivity tier (simplified 3-level). `None` = unclassified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductivityLevel {
    Productive,
    Neutral,
    Distracting,
}

/// Aggregation dimension for top-apps / top-sites rollups.
/// `App` groups by bundle identity (the default, pre-existing behavior).
/// `Site` breaks browser time down by registrable domain (github.com) extracted
/// from the segment's `url`; non-browser segments (no url) are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    App,
    Site,
}

impl ProductivityLevel {
    /// Weight used by the pulse-score weighted average. Unclassified
    /// (`None`) is intentionally absent so it stays out of the denominator.
    pub fn weight(self) -> f64 {
        match self {
            Self::Productive => 100.0,
            Self::Neutral => 50.0,
            Self::Distracting => 0.0,
        }
    }
}

/// A single classification rule. Match is case-insensitive; first matching
/// rule wins (user rules before defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryRule {
    pub field: MatchField,
    /// Lowercased substring or exact value (see `MatchField`).
    pub value: String,
    pub category: String,
    #[serde(default)]
    pub level: Option<ProductivityLevel>,
}

/// Which activity attribute a rule matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchField {
    /// Exact bundle id, e.g. `com.apple.dt.Xcode`.
    BundleId,
    /// Exact (case-insensitive) app display name, e.g. `Slack`.
    AppName,
    /// Exact (case-insensitive) registrable domain, e.g. `github.com`.
    Domain,
    /// Substring of the full URL.
    Url,
    /// Substring of the window title.
    Title,
}

/// Fields available to the classifier for one activity sample.
#[derive(Debug, Clone, Default)]
pub struct ActivityFields<'a> {
    pub bundle_id: Option<&'a str>,
    pub app_name: Option<&'a str>,
    pub window_title: Option<&'a str>,
    pub url: Option<&'a str>,
    /// Raw `LSApplicationCategoryType` from the app's Info.plist when known
    /// (e.g. `public.app-category.developer-tools`). Used only as a fallback
    /// after user + default rules miss.
    pub ls_category_type: Option<&'a str>,
}

/// Extract the registrable domain from a full URL — e.g. `https://github.com/foo`
/// → `github.com`, `https://mail.google.com/x` → `google.com`. Shared by the
/// classifier (`MatchField::Domain`) and the dashboard's "group by site"
/// aggregation so both use identical logic. Deliberately simple — not a full
/// Public Suffix List, good enough for the common cases.
pub(crate) fn registrable_domain(url: &str) -> Option<String> {
    // Strip scheme, take the host, drop the first subdomain label when
    // there are 3+ parts (www.github.com → github.com). Deliberately
    // simple — not a full PSL, good enough for the common cases.
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    let host = no_scheme.split('/').next()?;
    let host = host.split('?').next()?;
    let host = host.to_ascii_lowercase();
    let parts: Vec<&str> = host.split('.').collect();
    match parts.len() {
        0 => None,
        1 => Some(host),
        _ => {
            // Drop leading "www" / "m" / "mail" etc. when 3+ labels.
            if parts.len() >= 3 {
                Some(parts[parts.len() - 2..].join("."))
            } else {
                Some(host)
            }
        }
    }
}

impl<'a> ActivityFields<'a> {
    fn matches(&self, rule: &CategoryRule) -> bool {
        let v = rule.value.to_ascii_lowercase();
        let candidate = match rule.field {
            MatchField::BundleId => self.bundle_id.map(|s| s.to_ascii_lowercase()),
            MatchField::AppName => self.app_name.map(|s| s.to_ascii_lowercase()),
            MatchField::Domain => self
                .url
                .and_then(registrable_domain)
                .map(|d| d.to_ascii_lowercase()),
            MatchField::Url => self.url.map(|s| s.to_ascii_lowercase()),
            MatchField::Title => self.window_title.map(|s| s.to_ascii_lowercase()),
        };
        match (candidate.as_deref(), rule.field) {
            (Some(c), MatchField::BundleId | MatchField::AppName | MatchField::Domain) => c == v,
            (Some(c), MatchField::Url | MatchField::Title) => c.contains(&v),
            (None, _) => false,
        }
    }
}

/// Outcome of classifying one activity.
#[derive(Debug, Clone, Default)]
pub struct Classification {
    pub category: Option<String>,
    pub level: Option<ProductivityLevel>,
}

/// Classify an activity against user rules, defaults, local LS UTI, then
/// resolved enrichment cache / product-family heuristics.
///
/// `cached` is a previously resolved (bundle → category) entry from
/// Homebrew / iTunes enrichment. It sits **below** built-in defaults and
/// **below** on-device `LSApplicationCategoryType` so local truth wins;
/// enrichment only fills the cold-start hole when those miss.
pub fn classify(
    fields: &ActivityFields<'_>,
    user_rules: &[CategoryRule],
    cached: Option<&Classification>,
) -> Classification {
    for rule in user_rules {
        if fields.matches(rule) {
            return Classification {
                category: Some(rule.category.clone()),
                level: rule.level,
            };
        }
    }
    let catalog = crate::rule_engine::active_catalog();
    for rule in &catalog.rules {
        if fields.matches(rule) {
            return Classification {
                category: Some(rule.category.clone()),
                level: rule.level,
            };
        }
    }
    if let Some(uti) = fields.ls_category_type {
        if let Some(c) = classify_ls_application_category(uti) {
            return c;
        }
    }
    if let Some(c) = cached {
        if c.category.is_some() {
            return c.clone();
        }
    }
    // Known product family without Info.plist category (self-signed builds).
    if let Some(c) = classify_lumen_family(fields.bundle_id, fields.app_name) {
        return c;
    }
    Classification::default()
}

/// Map free-text metadata (Homebrew `desc` / name / subtitle, store blurbs)
/// → product category via the **loadable** mapping rule set.
///
/// Engine is fixed ([`crate::rule_engine`]); keywords live in
/// `rules/category_mapping.v1.json` (overridable under `$data_dir/rules/`).
pub fn classify_from_text_hint(text: &str) -> Option<Classification> {
    crate::rule_engine::active_mapping().classify_text(text)
}

/// Classify from one or more free-text metadata fields (desc, name, homepage).
/// First non-empty field that maps wins; then a joined fallback.
pub fn classify_from_metadata_texts(fields: &[&str]) -> Option<Classification> {
    let mut joined = String::new();
    for f in fields {
        let t = f.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(c) = classify_from_text_hint(t) {
            return Some(c);
        }
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(t);
    }
    if joined.is_empty() {
        None
    } else {
        classify_from_text_hint(&joined)
    }
}

/// Map iTunes / App Store `primaryGenreName` → product category (rule file).
pub fn classify_from_itunes_genre(genre: &str) -> Option<Classification> {
    crate::rule_engine::active_mapping().classify_itunes_genre(genre)
}

/// Map Apple `LSApplicationCategoryType` UTI → product category (rule file).
pub fn classify_ls_application_category(uti: &str) -> Option<Classification> {
    crate::rule_engine::active_mapping().classify_ls_uti(uti)
}

fn classify_lumen_family(
    bundle_id: Option<&str>,
    app_name: Option<&str>,
) -> Option<Classification> {
    use ProductivityLevel::*;
    let bundle = bundle_id.unwrap_or("").to_ascii_lowercase();
    let name = app_name.unwrap_or("").to_ascii_lowercase();
    if bundle.starts_with("com.lumenopen.")
        || name.contains("lumen navi")
        || name.contains("lumen-navi")
        || name.contains("lumen asr")
        || name.contains("lumen cua")
        || name.contains("lumen-cua")
    {
        return Some(Classification {
            category: Some("Development".into()),
            level: Some(Productive),
        });
    }
    None
}

/// Prefer a human display name when ranking: avoid executable-style names when
/// a nicer label was seen for the same bundle (e.g. `Lumen Navi` over
/// `lumen-navi-desktop`).
pub fn preferred_display_name(candidates: &[&str]) -> String {
    if candidates.is_empty() {
        return "Unknown".into();
    }
    let scored = |s: &str| -> i32 {
        let mut score = 0i32;
        if s.chars().any(|c| c.is_whitespace()) {
            score += 3; // "Lumen Navi"
        }
        if s.contains('-') || s.contains('_') {
            score -= 2; // "lumen-navi-desktop"
        }
        if s.chars().any(|c| c.is_uppercase()) {
            score += 1;
        }
        if s.to_ascii_lowercase().ends_with("desktop")
            || s.to_ascii_lowercase().ends_with("helper")
        {
            score -= 3;
        }
        score + s.len() as i32 / 20
    };
    candidates
        .iter()
        .max_by_key(|s| scored(s))
        .map(|s| (*s).to_string())
        .unwrap_or_else(|| candidates[0].to_string())
}

/// Built-in / file-backed default classification table.
///
/// Rules live in `rules/app_catalog.v1.json` (copied to
/// `$data_dir/rules/app_catalog.v1.json` on first store open). The match
/// engine is fixed in this crate; updating the JSON does not require a rebuild.
pub fn default_rules() -> Vec<CategoryRule> {
    crate::rule_engine::active_catalog().rules.clone()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_vscode_as_development() {
        let f = ActivityFields {
            bundle_id: Some("com.microsoft.VSCode"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert_eq!(c.category.as_deref(), Some("Development"));
        assert_eq!(c.level, Some(ProductivityLevel::Productive));
    }

    #[test]
    fn classifies_ghostty_by_bundle() {
        let f = ActivityFields {
            bundle_id: Some("com.mitchellh.ghostty"),
            app_name: Some("Ghostty"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert_eq!(c.category.as_deref(), Some("Development"));
    }

    #[test]
    fn system_uti_fallback_for_unknown_bundle() {
        let f = ActivityFields {
            bundle_id: Some("com.example.UnknownIde"),
            app_name: Some("UnknownIde"),
            ls_category_type: Some("public.app-category.developer-tools"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert_eq!(c.category.as_deref(), Some("Development"));
        assert_eq!(c.level, Some(ProductivityLevel::Productive));
    }

    #[test]
    fn user_rule_overrides_default() {
        let f = ActivityFields {
            bundle_id: Some("com.microsoft.VSCode"),
            ..Default::default()
        };
        let user = CategoryRule {
            field: MatchField::BundleId,
            value: "com.microsoft.VSCode".into(),
            category: "Personal".into(),
            level: Some(ProductivityLevel::Neutral),
        };
        let c = classify(&f, std::slice::from_ref(&user), None);
        assert_eq!(c.category.as_deref(), Some("Personal"));
    }

    #[test]
    fn user_rule_overrides_system_uti() {
        let f = ActivityFields {
            bundle_id: Some("com.example.x"),
            ls_category_type: Some("public.app-category.developer-tools"),
            ..Default::default()
        };
        let user = CategoryRule {
            field: MatchField::BundleId,
            value: "com.example.x".into(),
            category: "Writing".into(),
            level: Some(ProductivityLevel::Productive),
        };
        let c = classify(&f, std::slice::from_ref(&user), None);
        assert_eq!(c.category.as_deref(), Some("Writing"));
    }

    #[test]
    fn domain_extraction_drops_subdomain() {
        assert_eq!(
            registrable_domain("https://docs.github.com/pulls"),
            Some("github.com".into())
        );
        assert_eq!(
            registrable_domain("mail.google.com"),
            Some("google.com".into())
        );
        assert_eq!(
            registrable_domain("https://stackoverflow.com/q/1"),
            Some("stackoverflow.com".into())
        );
    }

    #[test]
    fn unclassified_returns_none() {
        let f = ActivityFields {
            app_name: Some("SomeUnknownApp"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert!(c.category.is_none());
        assert!(c.level.is_none());
    }

    #[test]
    fn youtube_is_distracting() {
        let f = ActivityFields {
            url: Some("https://www.youtube.com/watch?v=abc"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert_eq!(c.level, Some(ProductivityLevel::Distracting));
    }

    #[test]
    fn preferred_display_name_prefers_pretty_label() {
        let name = preferred_display_name(&["lumen-navi-desktop", "Lumen Navi"]);
        assert_eq!(name, "Lumen Navi");
    }

    #[test]
    fn lumen_family_classified_without_plist() {
        let f = ActivityFields {
            bundle_id: Some("com.lumenopen.navi"),
            app_name: Some("lumen-navi-desktop"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert_eq!(c.category.as_deref(), Some("Development"));
    }

    #[test]
    fn cache_used_when_defaults_and_ls_miss() {
        let f = ActivityFields {
            bundle_id: Some("ai.example.rareapp"),
            app_name: Some("RareApp"),
            ..Default::default()
        };
        let cached = Classification {
            category: Some("Browsing".into()),
            level: Some(ProductivityLevel::Neutral),
        };
        let c = classify(&f, &[], Some(&cached));
        assert_eq!(c.category.as_deref(), Some("Browsing"));
    }

    #[test]
    fn defaults_beat_cache() {
        let f = ActivityFields {
            bundle_id: Some("com.microsoft.VSCode"),
            ..Default::default()
        };
        let cached = Classification {
            category: Some("Entertainment".into()),
            level: Some(ProductivityLevel::Distracting),
        };
        let c = classify(&f, &[], Some(&cached));
        assert_eq!(c.category.as_deref(), Some("Development"));
    }

    #[test]
    fn text_hint_browser_and_ai() {
        let c = classify_from_text_hint("Web browser with integrated AI assistant").unwrap();
        assert_eq!(c.category.as_deref(), Some("Browsing"));
        let c2 = classify_from_text_hint("Terminal-based AI coding assistant").unwrap();
        assert_eq!(c2.category.as_deref(), Some("Development"));
    }

    #[test]
    fn text_hint_maps_teamwork_and_remote_desktop_genres() {
        // Real Homebrew descs — genre language, not vendor special-cases.
        let c = classify_from_text_hint("Teamwork app by Alibaba Group").unwrap();
        assert_eq!(c.category.as_deref(), Some("Communication"));
        let c2 = classify_from_text_hint(
            "NetEase UU remote desktop access and control tool",
        )
        .unwrap();
        assert_eq!(c2.category.as_deref(), Some("Utilities"));
    }

    #[test]
    fn metadata_fields_fallback_across_desc_and_name() {
        let c = classify_from_metadata_texts(&["", "Teamwork app by Alibaba Group"]).unwrap();
        assert_eq!(c.category.as_deref(), Some("Communication"));
    }

    #[test]
    fn itunes_genre_developer_tools() {
        let c = classify_from_itunes_genre("Developer Tools").unwrap();
        assert_eq!(c.category.as_deref(), Some("Development"));
    }

    #[test]
    fn comet_in_default_catalog() {
        let f = ActivityFields {
            bundle_id: Some("ai.perplexity.comet"),
            app_name: Some("Comet"),
            ..Default::default()
        };
        let c = classify(&f, &[], None);
        assert_eq!(c.category.as_deref(), Some("Browsing"));
    }
}
