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
    for rule in default_rules() {
        if fields.matches(&rule) {
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

/// Map free-text (Homebrew `desc`, app subtitle) → product category.
/// Conservative keyword heuristics; returns `None` when ambiguous.
pub fn classify_from_text_hint(text: &str) -> Option<Classification> {
    use ProductivityLevel::*;
    let t = text.to_ascii_lowercase();
    let (category, level) = if contains_any(
        &t,
        &[
            "web browser",
            "internet browser",
            "browser with",
            "chromium-based browser",
        ],
    ) || (t.contains("browser") && !t.contains("file browser") && !t.contains("browse files"))
    {
        ("Browsing", Neutral)
    } else if contains_any(
        &t,
        &[
            "code editor",
            "ide ",
            " ide",
            "terminal emulator",
            "terminal emulator",
            "coding agent",
            "coding assistant",
            "software development",
            "version control",
            "docker",
            "kubernetes",
            "api development",
            "git client",
        ],
    ) || (t.contains("ai") && t.contains("code"))
    {
        ("Development", Productive)
    } else if contains_any(&t, &["password manager", "2fa", "authenticator"]) {
        ("Utilities", Neutral)
    } else if contains_any(
        &t,
        &["note-taking", "notes app", "markdown editor", "knowledge base", "wiki"],
    ) {
        ("Writing", Productive)
    } else if contains_any(&t, &["chat", "messaging", "team communication", "email client"]) {
        ("Communication", Productive)
    } else if contains_any(&t, &["video player", "music player", "media player", "streaming"]) {
        ("Entertainment", Distracting)
    } else if contains_any(&t, &["social network", "social media"]) {
        ("Social", Distracting)
    } else if contains_any(&t, &["vpn", "proxy client", "firewall", "system monitor"]) {
        ("Utilities", Neutral)
    } else if contains_any(&t, &["spreadsheet", "word processor", "office suite", "presentation"]) {
        ("Productivity", Productive)
    } else if contains_any(&t, &["ai assistant", "chatbot", "large language", "llm"]) {
        // Generic AI desktop apps — productive by default (Claude/ChatGPT class).
        ("Productivity", Productive)
    } else {
        return None;
    };
    Some(Classification {
        category: Some(category.into()),
        level: Some(level),
    })
}

/// Map iTunes / App Store `primaryGenreName` → product category.
pub fn classify_from_itunes_genre(genre: &str) -> Option<Classification> {
    use ProductivityLevel::*;
    let g = genre.trim().to_ascii_lowercase();
    let (category, level) = match g.as_str() {
        "developer tools" => ("Development", Productive),
        "productivity" | "business" => ("Productivity", Productive),
        "utilities" => ("Utilities", Neutral),
        "social networking" => ("Social", Distracting),
        "entertainment" | "music" => ("Entertainment", Distracting),
        "games" => ("Games", Distracting),
        "education" | "reference" | "books" => ("Reference", Neutral),
        "graphics & design" | "photo & video" => ("Creative", Productive),
        "finance" => ("Finance", Neutral),
        "health & fitness" | "medical" => ("Health", Neutral),
        "lifestyle" | "travel" | "food & drink" | "shopping" => ("Lifestyle", Neutral),
        "news" | "magazines & newspapers" => ("News", Neutral),
        "weather" => ("Utilities", Neutral),
        "sports" => ("Entertainment", Neutral),
        "navigation" => ("Utilities", Neutral),
        _ => return None,
    };
    Some(Classification {
        category: Some(category.into()),
        level: Some(level),
    })
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Map Apple `LSApplicationCategoryType` UTI → product category + productivity.
pub fn classify_ls_application_category(uti: &str) -> Option<Classification> {
    use ProductivityLevel::*;
    let uti = uti.trim();
    // Accept both full UTI and bare suffix.
    let key = uti
        .strip_prefix("public.app-category.")
        .unwrap_or(uti)
        .to_ascii_lowercase();
    let (category, level) = match key.as_str() {
        "developer-tools" => ("Development", Productive),
        "productivity" | "business" => ("Productivity", Productive),
        "utilities" => ("Utilities", Neutral),
        "social-networking" => ("Social", Distracting),
        "entertainment" | "music" => ("Entertainment", Distracting),
        "games" | "action-games" | "adventure-games" | "arcade-games" | "board-games"
        | "card-games" | "casino-games" | "dice-games" | "educational-games" | "family-games"
        | "kids-games" | "music-games" | "puzzle-games" | "racing-games" | "role-playing-games"
        | "simulation-games" | "sports-games" | "strategy-games" | "trivia-games" | "word-games" => {
            ("Games", Distracting)
        }
        "education" | "reference" | "books" => ("Reference", Neutral),
        "graphics-design" | "photography" | "video" => ("Creative", Productive),
        "finance" => ("Finance", Neutral),
        "healthcare-fitness" | "medical" => ("Health", Neutral),
        "lifestyle" | "travel" | "food-and-drink" | "shopping" => ("Lifestyle", Neutral),
        "news" | "magazines-and-newspapers" => ("News", Neutral),
        "weather" => ("Utilities", Neutral),
        "sports" => ("Entertainment", Neutral),
        _ => return None,
    };
    Some(Classification {
        category: Some(category.into()),
        level: Some(level),
    })
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

/// Built-in default classification table.
pub fn default_rules() -> Vec<CategoryRule> {
    use MatchField::*;
    use ProductivityLevel::*;
    vec![
        // --- Development (productive) ---
        CategoryRule {
            field: BundleId,
            value: "com.apple.dt.Xcode".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.microsoft.VSCode".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.todesktop.230313mzl4w4u92".into(),
            category: "Development".into(),
            level: Some(Productive),
        }, // Cursor
        CategoryRule {
            field: BundleId,
            value: "com.github.atom".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.googlecode.iterm2".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.Terminal".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "dev.warp.Warp-Stable".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.mitchellh.ghostty".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "ai.perplexity.comet".into(),
            category: "Browsing".into(),
            level: Some(Neutral),
        }, // Perplexity Comet
        CategoryRule {
            field: BundleId,
            value: "dev.zcode.app".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.docker.docker".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.jetbrains.intellij".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.jetbrains.CLion".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.jetbrains.WebStorm".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.sublimetext.4".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Postman".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Tower".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Fork".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Zed".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Sublime Text".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Ghostty".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "ZCode".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: Domain,
            value: "github.com".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: Domain,
            value: "gitlab.com".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: Domain,
            value: "stackoverflow.com".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: Domain,
            value: "developer.mozilla.org".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: Domain,
            value: "docs.rs".into(),
            category: "Development".into(),
            level: Some(Productive),
        },
        // --- Communication (neutral) ---
        CategoryRule {
            field: BundleId,
            value: "com.tinyspeck.slackmacgap".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.MobileSMS".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.mail".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.tencent.xinWeChat".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.microsoft.teams2".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.microsoft.Outlook".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.tdesktop.Telegram".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.hnc.Discord".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "ru.keepcoder.Telegram".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: Domain,
            value: "gmail.com".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: Domain,
            value: "mail.google.com".into(),
            category: "Communication".into(),
            level: Some(Neutral),
        },
        // --- Writing / docs (productive) ---
        CategoryRule {
            field: BundleId,
            value: "notion.id".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: AppName,
            value: "Obsidian".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "md.obsidian".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.Notes".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.iWork.Pages".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.TextEdit".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        CategoryRule {
            field: BundleId,
            value: "com.microsoft.Word".into(),
            category: "Writing".into(),
            level: Some(Productive),
        },
        // --- Browsers (neutral — content decides via domain rules) ---
        CategoryRule {
            field: BundleId,
            value: "com.apple.Safari".into(),
            category: "Browsing".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.google.Chrome".into(),
            category: "Browsing".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "company.thebrowser.Browser".into(),
            category: "Browsing".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "org.mozilla.firefox".into(),
            category: "Browsing".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "org.mozilla.firefox".into(),
            category: "Browsing".into(),
            level: Some(Neutral),
        },
        // --- Utilities / system ---
        CategoryRule {
            field: BundleId,
            value: "io.github.clash-verge-rev.clash-verge-rev".into(),
            category: "Utilities".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: AppName,
            value: "Clash Verge".into(),
            category: "Utilities".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.west2online.ClashX".into(),
            category: "Utilities".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "com.apple.finder".into(),
            category: "System".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: AppName,
            value: "Finder".into(),
            category: "System".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: BundleId,
            value: "cx.c3.theunarchiver".into(),
            category: "Utilities".into(),
            level: Some(Neutral),
        },
        // --- Reference / learning ---
        CategoryRule {
            field: Domain,
            value: "wikipedia.org".into(),
            category: "Reference".into(),
            level: Some(Neutral),
        },
        CategoryRule {
            field: AppName,
            value: "Books".into(),
            category: "Reference".into(),
            level: Some(Neutral),
        },
        // --- Entertainment (distracting) ---
        CategoryRule {
            field: Domain,
            value: "youtube.com".into(),
            category: "Entertainment".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: Domain,
            value: "bilibili.com".into(),
            category: "Entertainment".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: Domain,
            value: "netflix.com".into(),
            category: "Entertainment".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: AppName,
            value: "Spotify".into(),
            category: "Entertainment".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: AppName,
            value: "Music".into(),
            category: "Entertainment".into(),
            level: Some(Neutral),
        },
        // --- Social (distracting) ---
        CategoryRule {
            field: Domain,
            value: "twitter.com".into(),
            category: "Social".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: Domain,
            value: "x.com".into(),
            category: "Social".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: Domain,
            value: "reddit.com".into(),
            category: "Social".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: Domain,
            value: "instagram.com".into(),
            category: "Social".into(),
            level: Some(Distracting),
        },
        CategoryRule {
            field: Domain,
            value: "weibo.com".into(),
            category: "Social".into(),
            level: Some(Distracting),
        },
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
