//! Async category enrichment sources: Homebrew Cask + iTunes Lookup.
//!
//! These run **off** the frontmost sampling path. Results are written to
//! `app_category_cache` / `brew_cask_by_bundle` and then re-applied to
//! historical segments. Network failures are soft: the app stays
//! Uncategorized until a later pass succeeds.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::categorization::{
    classify_from_itunes_genre, classify_from_metadata_texts, Classification, ProductivityLevel,
};
use crate::StoreError;

const BREW_CASK_JSON: &str = "https://formulae.brew.sh/api/cask.json";
const BREW_CASK_ONE: &str = "https://formulae.brew.sh/api/cask";
const BREW_ANALYTICS_30D: &str =
    "https://formulae.brew.sh/api/analytics/cask-install/homebrew-cask/30d.json";
const ITUNES_LOOKUP: &str = "https://itunes.apple.com/lookup";

/// One row suitable for `brew_cask_by_bundle`.
#[derive(Debug, Clone)]
pub struct BrewCaskRow {
    pub bundle_id: String,
    pub cask_token: String,
    pub name: Option<String>,
    pub desc: Option<String>,
    pub homepage: Option<String>,
    pub installs_30d: Option<i64>,
}

/// Outcome of enriching a single unknown bundle.
#[derive(Debug, Clone)]
pub struct EnrichmentHit {
    pub classification: Classification,
    pub source: &'static str,
    pub confidence: f64,
    pub brew_token: Option<String>,
    pub brew_desc: Option<String>,
    pub itunes_genre: Option<String>,
}

/// Guess a Homebrew cask token from a display name (`Visual Studio Code` →
/// `visual-studio-code`). Best-effort; CJK-only names collapse to empty.
pub fn guess_cask_token(app_name: &str) -> String {
    let mut s = app_name.trim().to_ascii_lowercase();
    // Common suffixes that don't appear in cask tokens.
    for suffix in [" .app", ".app"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Candidate Homebrew cask tokens from **bundle id + display name**.
///
/// Prefer reverse-DNS labels (works when the UI name is CJK, e.g. `UU远程`
/// with bundle `com.netease.uuremote` → token `uuremote`). Also expands
/// CamelCase product labels (`DingTalkMac` → `dingtalk`, `dingtalkmac`).
pub fn cask_token_candidates(bundle_id: &str, app_name: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let t = raw.trim().trim_matches('-').to_ascii_lowercase();
        if t.len() < 2 {
            return;
        }
        // Skip pure noise labels.
        if matches!(
            t.as_str(),
            "com" | "org" | "net" | "io" | "app" | "mac" | "osx" | "desktop" | "helper" | "server"
                | "client" | "agent" | "launcher"
        ) {
            return;
        }
        if !out.iter().any(|e| e == &t) {
            out.push(t);
        }
    };

    let labels: Vec<&str> = bundle_id.split('.').filter(|s| !s.is_empty()).collect();
    // Walk labels from the right (most product-specific first).
    for label in labels.iter().rev().take(3) {
        for variant in label_token_variants(label) {
            push(&variant);
        }
    }

    if let Some(name) = app_name {
        let g = guess_cask_token(name);
        if !g.is_empty() {
            push(&g);
        }
        // If the display name had almost no ASCII, still try stripping spaces
        // from any latin islands (e.g. "UU远程" → "uu").
        let ascii: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if ascii.len() >= 2 {
            push(&ascii);
        }
    }

    out
}

/// Expand one reverse-DNS label into likely cask tokens.
fn label_token_variants(label: &str) -> Vec<String> {
    let mut v = Vec::new();
    let raw = label.trim();
    if raw.is_empty() {
        return v;
    }
    v.push(raw.to_ascii_lowercase());

    // Strip common packaging suffixes, repeatedly.
    let mut core = raw.to_string();
    for _ in 0..3 {
        let lower = core.to_ascii_lowercase();
        let stripped = [
            "formac",
            "formacos",
            "desktop",
            "macos",
            "osx",
            "mac",
            "app",
            "client",
            "helper",
            "launcher",
            "installer",
            "server",
        ]
        .iter()
        .find_map(|suf| lower.strip_suffix(suf).map(|s| s.to_string()));
        if let Some(s) = stripped {
            let s = s.trim_end_matches(['-', '_']).to_string();
            if s.len() >= 2 {
                core = preserve_case_prefix(&core, &s);
                v.push(core.to_ascii_lowercase());
                continue;
            }
        }
        break;
    }

    // CamelCase → kebab and concatenated lowercase.
    let parts = split_camel_case(raw);
    if parts.len() > 1 {
        v.push(parts.join("-").to_ascii_lowercase());
        v.push(parts.join("").to_ascii_lowercase());
        // Drop trailing packaging words.
        let meaningful: Vec<&str> = parts
            .iter()
            .map(|s| s.as_str())
            .filter(|p| {
                !matches!(
                    p.to_ascii_lowercase().as_str(),
                    "mac" | "osx" | "desktop" | "app" | "client" | "helper" | "server"
                )
            })
            .collect();
        if !meaningful.is_empty() && meaningful.len() < parts.len() {
            v.push(meaningful.join("-").to_ascii_lowercase());
            v.push(meaningful.join("").to_ascii_lowercase());
        }
    }

    v
}

fn preserve_case_prefix(original: &str, lower_stripped: &str) -> String {
    // Keep original casing length when possible for further camel splits.
    if original.len() >= lower_stripped.len() {
        original[..lower_stripped.len()].to_string()
    } else {
        lower_stripped.to_string()
    }
}

fn split_camel_case(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '-' || ch == '_' {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = ch.is_ascii_uppercase()
            && cur
                .chars()
                .last()
                .map(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                .unwrap_or(false)
            || (ch.is_ascii_uppercase()
                && prev.map(|p| p.is_ascii_uppercase()).unwrap_or(false)
                && next.map(|n| n.is_ascii_lowercase()).unwrap_or(false));
        if boundary && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn classify_brew_fields(
    desc: Option<&str>,
    name: Option<&str>,
    homepage: Option<&str>,
) -> Option<Classification> {
    let fields: Vec<&str> = [desc, name, homepage]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect();
    if fields.is_empty() {
        None
    } else {
        classify_from_metadata_texts(&fields)
    }
}

/// Extract reverse-DNS-looking bundle ids from Homebrew zap trash paths.
/// Prefers Preferences `*.plist` and Saved Application State entries.
pub fn extract_bundle_ids_from_zap_paths(paths: &[String]) -> Vec<(String, i32)> {
    let mut scores: HashMap<String, i32> = HashMap::new();
    for p in paths {
        let rules: &[(&str, i32)] = &[
            ("~/Library/Preferences/", 100),
            ("~/Library/Saved Application State/", 90),
            ("~/Library/Containers/", 80),
            ("~/Library/WebKit/", 70),
            ("~/Library/HTTPStorages/", 65),
            ("~/Library/Caches/", 50),
            ("~/Library/Application Support/", 40),
            ("~/Library/Group Containers/", 60),
        ];
        for (prefix, score) in rules {
            if let Some(rest) = p.strip_prefix(prefix) {
                let mut bid = rest
                    .trim_end_matches(".plist")
                    .trim_end_matches(".savedState")
                    .trim_end_matches('/')
                    .to_string();
                if let Some((head, _)) = bid.split_once('/') {
                    bid = head.to_string();
                }
                if bid.starts_with("group.") {
                    bid = bid.trim_start_matches("group.").to_string();
                }
                // Skip ToDesktop installer noise and wildcards.
                if bid.contains('*')
                    || bid.eq_ignore_ascii_case("todesktop.com.ToDesktop-Installer")
                    || bid.contains("ShipIt")
                    || bid.ends_with(".helper")
                    || bid.ends_with(".helper.Renderer")
                {
                    continue;
                }
                if looks_like_bundle_id(&bid) {
                    *scores.entry(bid).or_insert(0) += score;
                }
            }
        }
        // sharedfilelist …/com.foo.bar.sfl*
        if let Some(idx) = p.find("ApplicationRecentDocuments/") {
            let rest = &p[idx + "ApplicationRecentDocuments/".len()..];
            let bid = rest
                .trim_end_matches('*')
                .trim_end_matches(".sfl")
                .trim_end_matches(".sfl2")
                .trim_end_matches(".sfl3");
            if looks_like_bundle_id(bid) {
                *scores.entry(bid.to_string()).or_insert(0) += 85;
            }
        }
    }
    let mut v: Vec<(String, i32)> = scores.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

fn looks_like_bundle_id(s: &str) -> bool {
    if s.len() < 3 || !s.contains('.') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && s.split('.').count() >= 2
        && !s.starts_with('.')
        && !s.ends_with('.')
}

fn http_get_json(url: &str) -> Result<Value, StoreError> {
    // cask.json is multi-MB. Do **not** use `into_string()` — ureq caps it and
    // returns "response too big", which silently prevented the full brew index
    // from ever loading (only single-cask fetches worked).
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(180))
        .user_agent("lumen-navi-category-enrichment/0.1")
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| StoreError::Other(format!("http get {url}: {e}")))?;
    let reader = resp.into_reader();
    serde_json::from_reader(reader).map_err(|e| StoreError::json(format!("parse {url}: {e}")))
}

/// Download Homebrew cask catalog + 30d install analytics and build a
/// bundle_id → cask row map. Large (~few MB); call sparingly (daily).
pub fn fetch_brew_cask_index() -> Result<Vec<BrewCaskRow>, StoreError> {
    let casks = http_get_json(BREW_CASK_JSON)?;
    let analytics = http_get_json(BREW_ANALYTICS_30D).ok();
    let installs = parse_analytics_counts(analytics.as_ref());

    let arr = casks
        .as_array()
        .ok_or_else(|| StoreError::Other("cask.json root is not an array".into()))?;

    let mut out: Vec<BrewCaskRow> = Vec::new();
    let mut seen: HashMap<String, i32> = HashMap::new();

    for cask in arr {
        let token = cask
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if token.is_empty() {
            continue;
        }
        // Only GUI apps with an .app artifact matter for frontmost tracking.
        if !cask_has_app_artifact(cask) {
            continue;
        }
        let name = cask
            .get("name")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let desc = cask
            .get("desc")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let homepage = cask
            .get("homepage")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let installs_30d = installs.get(&token).copied();

        let paths = collect_zap_paths(cask);
        let bids = extract_bundle_ids_from_zap_paths(&paths);
        if bids.is_empty() {
            continue;
        }
        // Primary = highest score; skip lower-scored helpers unless unique.
        let primary_score = bids[0].1;
        for (bid, score) in bids {
            // Keep primary and near-primary (e.g. score ≥ 80% of best).
            if score * 100 < primary_score * 80 {
                continue;
            }
            // Prefer higher score if we already saw this bundle.
            if let Some(prev) = seen.get(&bid) {
                if *prev >= score {
                    continue;
                }
            }
            seen.insert(bid.clone(), score);
            out.push(BrewCaskRow {
                bundle_id: bid,
                cask_token: token.clone(),
                name: name.clone(),
                desc: desc.clone(),
                homepage: homepage.clone(),
                installs_30d,
            });
        }
    }

    // Dedup: keep best score per bundle (rebuild from seen order).
    let mut best: HashMap<String, BrewCaskRow> = HashMap::new();
    for row in out {
        best.entry(row.bundle_id.clone())
            .and_modify(|e| {
                // Prefer row with installs / longer desc if same token family.
                if e.installs_30d.unwrap_or(0) < row.installs_30d.unwrap_or(0) {
                    *e = row.clone();
                }
            })
            .or_insert(row);
    }
    Ok(best.into_values().collect())
}

fn parse_analytics_counts(analytics: Option<&Value>) -> HashMap<String, i64> {
    let mut m = HashMap::new();
    let Some(root) = analytics else {
        return m;
    };
    let Some(formulae) = root.get("formulae").and_then(|v| v.as_object()) else {
        return m;
    };
    for (token, arr) in formulae {
        let Some(count_val) = arr
            .as_array()
            .and_then(|a| a.first())
            .and_then(|o| o.get("count"))
        else {
            continue;
        };
        if let Some(n) = count_val.as_i64() {
            m.insert(token.clone(), n);
        } else if let Some(s) = count_val.as_str() {
            if let Ok(v) = s.replace(',', "").parse::<i64>() {
                m.insert(token.clone(), v);
            }
        }
    }
    m
}

fn cask_has_app_artifact(cask: &Value) -> bool {
    let Some(arts) = cask.get("artifacts").and_then(|v| v.as_array()) else {
        return false;
    };
    for a in arts {
        if a.get("app").is_some() {
            return true;
        }
        if let Some(s) = a.as_str() {
            if s.ends_with(".app") {
                return true;
            }
        }
    }
    false
}

fn collect_zap_paths(cask: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    let Some(arts) = cask.get("artifacts").and_then(|v| v.as_array()) else {
        return paths;
    };
    for a in arts {
        let Some(zaps) = a.get("zap").and_then(|v| v.as_array()) else {
            continue;
        };
        for z in zaps {
            for key in ["trash", "rmdir"] {
                match z.get(key) {
                    Some(Value::String(s)) => paths.push(s.clone()),
                    Some(Value::Array(arr)) => {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                paths.push(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    paths
}

/// Fetch a single cask by token (`comet`, `visual-studio-code`).
pub fn fetch_brew_cask(token: &str) -> Result<Option<BrewCaskOne>, StoreError> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let url = format!("{BREW_CASK_ONE}/{token}.json");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent("lumen-navi-category-enrichment/0.1")
        .build();
    let resp = match agent.get(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(e) => return Err(StoreError::Other(format!("brew cask {token}: {e}"))),
    };
    let body = resp
        .into_string()
        .map_err(|e| StoreError::Other(format!("brew cask body: {e}")))?;
    let v: Value = serde_json::from_str(&body).map_err(StoreError::json)?;
    let desc = v
        .get("desc")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let name = v
        .get("name")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let homepage = v
        .get("homepage")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let installs_30d = v
        .pointer("/analytics/install/30d")
        .and_then(|obj| obj.as_object())
        .and_then(|m| m.values().next())
        .and_then(|c| c.as_i64().or_else(|| c.as_str()?.replace(',', "").parse().ok()));
    let paths = collect_zap_paths(&v);
    let bids = extract_bundle_ids_from_zap_paths(&paths);
    Ok(Some(BrewCaskOne {
        token: token.to_string(),
        name,
        desc,
        homepage,
        installs_30d,
        bundle_ids: bids.into_iter().map(|(b, _)| b).collect(),
    }))
}

#[derive(Debug, Clone)]
pub struct BrewCaskOne {
    pub token: String,
    #[allow(dead_code)]
    pub name: Option<String>,
    pub desc: Option<String>,
    #[allow(dead_code)]
    pub homepage: Option<String>,
    #[allow(dead_code)]
    pub installs_30d: Option<i64>,
    pub bundle_ids: Vec<String>,
}

/// iTunes Lookup by bundle id (MAS apps). Empty results for direct-download apps.
pub fn fetch_itunes_genre(bundle_id: &str) -> Result<Option<String>, StoreError> {
    let url = format!("{ITUNES_LOOKUP}?bundleId={bundle_id}&country=us");
    #[derive(Deserialize)]
    struct Lookup {
        result_count: i64,
        results: Vec<ItunesResult>,
    }
    #[derive(Deserialize)]
    struct ItunesResult {
        #[serde(rename = "primaryGenreName")]
        primary_genre_name: Option<String>,
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .user_agent("lumen-navi-category-enrichment/0.1")
        .build();
    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| StoreError::Other(format!("itunes lookup: {e}")))?;
    let body = resp
        .into_string()
        .map_err(|e| StoreError::Other(format!("itunes body: {e}")))?;
    let parsed: Lookup = serde_json::from_str(&body).map_err(StoreError::json)?;
    if parsed.result_count <= 0 {
        return Ok(None);
    }
    Ok(parsed
        .results
        .into_iter()
        .find_map(|r| r.primary_genre_name)
        .filter(|s| !s.is_empty()))
}

/// Resolve category for one bundle using local brew row, multi-token cask
/// fetch, then iTunes.
///
/// Failure modes are distinguished in spirit (even when the DB only stores
/// `failed`):
/// - **Parse/index miss**: no brew row, token guesses 404, iTunes empty
/// - **Metadata present, rules miss**: brew/iTunes returned text/genre but
///   mappers returned `None` — fix mappers, don't hard-code the bundle
pub fn resolve_bundle_category(
    bundle_id: &str,
    app_name: Option<&str>,
    brew_row: Option<&BrewCaskRow>,
    allow_network: bool,
) -> Result<Option<EnrichmentHit>, StoreError> {
    // 1) Local brew index row (identity already resolved via zap→bundle).
    if let Some(row) = brew_row {
        if let Some(c) = classify_brew_fields(
            row.desc.as_deref(),
            row.name.as_deref(),
            row.homepage.as_deref(),
        ) {
            return Ok(Some(EnrichmentHit {
                classification: c,
                source: "brew",
                confidence: 0.8,
                brew_token: Some(row.cask_token.clone()),
                brew_desc: row.desc.clone(),
                itunes_genre: None,
            }));
        }
        // Index hit but genre rules couldn't map — fall through to iTunes
        // before giving up (MAS dual-listed apps).
    }

    if !allow_network {
        return Ok(None);
    }

    // 2) On-demand cask fetch using token candidates (bundle labels + name).
    //    Cap network attempts so one bad bundle cannot fan out unbounded.
    let mut best_unmapped: Option<BrewCaskOne> = None;
    let tokens = cask_token_candidates(bundle_id, app_name);
    for token in tokens.into_iter().take(6) {
        let Some(one) = fetch_brew_cask(&token)? else {
            continue;
        };
        let matches_bundle = one.bundle_ids.iter().any(|b| b.eq_ignore_ascii_case(bundle_id));
        // Accept weak token-only match only when zap listed nothing.
        let accept = matches_bundle || one.bundle_ids.is_empty();
        if !accept {
            continue;
        }
        if let Some(c) = classify_brew_fields(
            one.desc.as_deref(),
            one.name.as_deref(),
            one.homepage.as_deref(),
        ) {
            let conf = if matches_bundle { 0.85 } else { 0.5 };
            return Ok(Some(EnrichmentHit {
                classification: c,
                source: "brew",
                confidence: conf,
                brew_token: Some(one.token),
                brew_desc: one.desc,
                itunes_genre: None,
            }));
        }
        // Keep first bundle-matched cask for diagnostics / later iTunes combo.
        if matches_bundle && best_unmapped.is_none() {
            best_unmapped = Some(one);
        }
    }

    // 3) iTunes Lookup (genre is structured metadata — map via genre rules).
    if let Some(genre) = fetch_itunes_genre(bundle_id)? {
        if let Some(c) = classify_from_itunes_genre(&genre) {
            return Ok(Some(EnrichmentHit {
                classification: c,
                source: "itunes",
                confidence: 0.7,
                brew_token: best_unmapped.as_ref().map(|b| b.token.clone()),
                brew_desc: best_unmapped.and_then(|b| b.desc),
                itunes_genre: Some(genre),
            }));
        }
        // Genre string present but unmapped in our table — extend
        // `classify_from_itunes_genre`, don't special-case the bundle.
        // Soft fallback so structured store metadata still yields a category.
        return Ok(Some(EnrichmentHit {
            classification: Classification {
                category: Some("Utilities".into()),
                level: Some(ProductivityLevel::Neutral),
            },
            source: "itunes",
            confidence: 0.35,
            brew_token: best_unmapped.as_ref().map(|b| b.token.clone()),
            brew_desc: best_unmapped.and_then(|b| b.desc),
            itunes_genre: Some(genre),
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_token_from_display_name() {
        assert_eq!(guess_cask_token("Visual Studio Code"), "visual-studio-code");
        assert_eq!(guess_cask_token("Comet"), "comet");
        assert_eq!(guess_cask_token("Google Chrome"), "google-chrome");
    }

    #[test]
    fn token_candidates_from_bundle_work_when_ui_name_is_cjk() {
        let c = cask_token_candidates("com.netease.uuremote", Some("UU远程"));
        assert!(
            c.iter().any(|t| t == "uuremote"),
            "expected uuremote in {c:?}"
        );
        let c2 = cask_token_candidates("com.alibaba.DingTalkMac", Some("DingTalk"));
        assert!(
            c2.iter().any(|t| t == "dingtalk" || t == "dingtalkmac"),
            "expected dingtalk* in {c2:?}"
        );
    }

    #[test]
    fn brew_desc_teamwork_and_remote_map() {
        let c = classify_brew_fields(
            Some("Teamwork app by Alibaba Group"),
            Some("DingTalk"),
            Some("https://www.dingtalk.com/"),
        )
        .unwrap();
        assert_eq!(c.category.as_deref(), Some("Communication"));
        let c2 = classify_brew_fields(
            Some("NetEase UU remote desktop access and control tool"),
            Some("UU Remote"),
            None,
        )
        .unwrap();
        assert_eq!(c2.category.as_deref(), Some("Utilities"));
    }

    #[test]
    fn extract_comet_bundle_from_zap() {
        let paths = vec![
            "~/Library/Application Support/ai.perplexity.comet".into(),
            "~/Library/Preferences/ai.perplexity.comet.plist".into(),
            "~/Library/Saved Application State/ai.perplexity.comet.savedState".into(),
            "~/Library/Application Support/Comet".into(),
        ];
        let bids = extract_bundle_ids_from_zap_paths(&paths);
        assert_eq!(bids[0].0, "ai.perplexity.comet");
    }

    #[test]
    fn skips_todesktop_installer() {
        let paths = vec![
            "~/Library/Saved Application State/todesktop.com.ToDesktop-Installer.savedState".into(),
            "~/Library/Preferences/com.todesktop.230313mzl4w4u92.plist".into(),
        ];
        let bids = extract_bundle_ids_from_zap_paths(&paths);
        assert!(bids.iter().all(|(b, _)| b != "todesktop.com.ToDesktop-Installer"));
        assert!(bids.iter().any(|(b, _)| b == "com.todesktop.230313mzl4w4u92"));
    }
}
