//! Deterministic activity → (category, productivity_level) classifier.
//!
//! No ML — just an ordered rule list (Timing / RescueTime philosophy). A small
//! built-in default table covers common dev/communication/browsing apps and
//! domains; user overrides are stored in the `kv` table and take priority.
//!
//! Productivity levels use a simplified 3-tier scale (Qbserve-style) rather
//! than RescueTime's 5 tiers: `productive` / `neutral` / `distracting`.
//! Unclassified activities get `None` and are **excluded from the pulse-score
//! denominator** (unlike RescueTime, which punishes unclassified as neutral).

use serde::{Deserialize, Serialize};

/// Productivity tier (simplified 3-level). `None` = unclassified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductivityLevel {
    Productive,
    Neutral,
    Distracting,
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

/// A single classification rule. Match is case-insensitive substring on the
/// chosen field; first matching rule wins (user rules before defaults).
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
}

impl<'a> ActivityFields<'a> {
    fn registrable_domain(url: &str) -> Option<String> {
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

    fn matches(&self, rule: &CategoryRule) -> bool {
        let v = rule.value.to_ascii_lowercase();
        let candidate = match rule.field {
            MatchField::BundleId => self.bundle_id.map(|s| s.to_ascii_lowercase()),
            MatchField::AppName => self.app_name.map(|s| s.to_ascii_lowercase()),
            MatchField::Domain => self
                .url
                .and_then(Self::registrable_domain)
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

/// Classify an activity against user rules first, then the built-in defaults.
/// First match wins within each list; user rules take priority overall.
pub fn classify(fields: &ActivityFields<'_>, user_rules: &[CategoryRule]) -> Classification {
    for rule in user_rules {
        if fields.matches(rule) {
            return Classification {
                category: Some(rule.category.clone()),
                level: rule.level,
            };
        }
    }
    for rule in default_rules() {
        if fields.matches(&rule) {
            return Classification {
                category: Some(rule.category.clone()),
                level: rule.level,
            };
        }
    }
    Classification::default()
}

/// Built-in default classification table. Covers common macOS dev tools,
/// communication apps, and well-known web domains. Users override via the
/// `kv`-stored rule list (which is tried first).
pub fn default_rules() -> Vec<CategoryRule> {
    use MatchField::*;
    use ProductivityLevel::*;
    vec![
        // --- Development (productive) ---
        CategoryRule { field: BundleId, value: "com.apple.dt.Xcode".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.microsoft.VSCode".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.todesktop.230313mzl4w4u92".into(), category: "Development".into(), level: Some(Productive) }, // Cursor
        CategoryRule { field: BundleId, value: "com.github.atom".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.googlecode.iterm2".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.apple.Terminal".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "dev.warp.Warp-Stable".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.docker.docker".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: AppName, value: "Postman".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: AppName, value: "Tower".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: AppName, value: "Fork".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: AppName, value: "Zed".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: AppName, value: "Sublime Text".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: Domain, value: "github.com".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: Domain, value: "gitlab.com".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: Domain, value: "stackoverflow.com".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: Domain, value: "developer.mozilla.org".into(), category: "Development".into(), level: Some(Productive) },
        CategoryRule { field: Domain, value: "docs.rs".into(), category: "Development".into(), level: Some(Productive) },
        // --- Communication (neutral) ---
        CategoryRule { field: BundleId, value: "com.tinyspeck.slackmacgap".into(), category: "Communication".into(), level: Some(Neutral) },
        CategoryRule { field: BundleId, value: "com.apple.MobileSMS".into(), category: "Communication".into(), level: Some(Neutral) },
        CategoryRule { field: BundleId, value: "com.apple.mail".into(), category: "Communication".into(), level: Some(Neutral) },
        CategoryRule { field: BundleId, value: "com.tencent.xinWeChat".into(), category: "Communication".into(), level: Some(Neutral) },
        CategoryRule { field: BundleId, value: "com.microsoft.teams2".into(), category: "Communication".into(), level: Some(Neutral) },
        CategoryRule { field: Domain, value: "gmail.com".into(), category: "Communication".into(), level: Some(Neutral) },
        CategoryRule { field: Domain, value: "mail.google.com".into(), category: "Communication".into(), level: Some(Neutral) },
        // --- Writing / docs (productive) ---
        CategoryRule { field: BundleId, value: "notion.id".into(), category: "Writing".into(), level: Some(Productive) },
        CategoryRule { field: AppName, value: "Obsidian".into(), category: "Writing".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "md.obsidian".into(), category: "Writing".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.apple.Notes".into(), category: "Writing".into(), level: Some(Productive) },
        CategoryRule { field: BundleId, value: "com.apple.iWork.Pages".into(), category: "Writing".into(), level: Some(Productive) },
        // --- Reference / learning (neutral-productive) ---
        CategoryRule { field: Domain, value: "wikipedia.org".into(), category: "Reference".into(), level: Some(Neutral) },
        CategoryRule { field: AppName, value: "Books".into(), category: "Reference".into(), level: Some(Neutral) },
        // --- Entertainment (distracting) ---
        CategoryRule { field: Domain, value: "youtube.com".into(), category: "Entertainment".into(), level: Some(Distracting) },
        CategoryRule { field: Domain, value: "bilibili.com".into(), category: "Entertainment".into(), level: Some(Distracting) },
        CategoryRule { field: Domain, value: "netflix.com".into(), category: "Entertainment".into(), level: Some(Distracting) },
        CategoryRule { field: AppName, value: "Spotify".into(), category: "Entertainment".into(), level: Some(Distracting) },
        CategoryRule { field: AppName, value: "Music".into(), category: "Entertainment".into(), level: Some(Neutral) },
        // --- Social (distracting) ---
        CategoryRule { field: Domain, value: "twitter.com".into(), category: "Social".into(), level: Some(Distracting) },
        CategoryRule { field: Domain, value: "x.com".into(), category: "Social".into(), level: Some(Distracting) },
        CategoryRule { field: Domain, value: "reddit.com".into(), category: "Social".into(), level: Some(Distracting) },
        CategoryRule { field: Domain, value: "instagram.com".into(), category: "Social".into(), level: Some(Distracting) },
        CategoryRule { field: Domain, value: "weibo.com".into(), category: "Social".into(), level: Some(Distracting) },
    ]
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
        let c = classify(&f, &[]);
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
        let c = classify(&f, std::slice::from_ref(&user));
        assert_eq!(c.category.as_deref(), Some("Personal"));
    }

    #[test]
    fn domain_extraction_drops_subdomain() {
        assert_eq!(
            ActivityFields::registrable_domain("https://docs.github.com/pulls"),
            Some("github.com".into())
        );
        assert_eq!(
            ActivityFields::registrable_domain("mail.google.com"),
            Some("google.com".into())
        );
        assert_eq!(
            ActivityFields::registrable_domain("https://stackoverflow.com/q/1"),
            Some("stackoverflow.com".into())
        );
    }

    #[test]
    fn unclassified_returns_none() {
        let f = ActivityFields {
            app_name: Some("SomeUnknownApp"),
            ..Default::default()
        };
        let c = classify(&f, &[]);
        assert!(c.category.is_none());
        assert!(c.level.is_none());
    }

    #[test]
    fn youtube_is_distracting() {
        let f = ActivityFields {
            url: Some("https://www.youtube.com/watch?v=abc"),
            ..Default::default()
        };
        let c = classify(&f, &[]);
        assert_eq!(c.level, Some(ProductivityLevel::Distracting));
    }
}
