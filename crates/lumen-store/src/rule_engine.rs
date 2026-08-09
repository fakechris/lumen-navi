//! Fixed rule engine + loadable rule sets.
//!
//! **Engine** (this module): match semantics, priority, reload plumbing.
//! **Rules** (JSON under `rules/`): keywords, genres, catalog entries.
//!
//! Ship defaults via `include_str!`. At store open, copy into
//! `$data_dir/rules/*.json` if missing so operators can edit without
//! recompiling. Call [`reload_rules_from_dir`] after an external update.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::Deserialize;

use crate::categorization::{
    CategoryRule, Classification, MatchField, ProductivityLevel,
};
use crate::StoreError;

const EMBEDDED_MAPPING: &str = include_str!("../rules/category_mapping.v1.json");
const EMBEDDED_CATALOG: &str = include_str!("../rules/app_catalog.v1.json");

static ACTIVE_MAPPING: RwLock<Option<Arc<MappingRuleSet>>> = RwLock::new(None);
static ACTIVE_CATALOG: RwLock<Option<Arc<CatalogRuleSet>>> = RwLock::new(None);

// --- JSON schema -----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct MappingFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    text_rules: Vec<TextRuleJson>,
    #[serde(default)]
    itunes_genre_rules: Vec<GenreRuleJson>,
    #[serde(default)]
    ls_uti_rules: Vec<UtiRuleJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct TextRuleJson {
    #[serde(default)]
    id: String,
    /// Match if **any** of these substrings appear (OR).
    #[serde(default)]
    any: Vec<String>,
    /// Match only if **all** appear (AND).
    #[serde(default)]
    all: Vec<String>,
    /// Fail if **any** of these appear.
    #[serde(default)]
    none: Vec<String>,
    /// Each inner list is OR; groups are AND together.
    /// Example: `[["remote"], ["desktop","control"]]` ⇒ remote AND (desktop|control).
    #[serde(default)]
    all_groups: Vec<Vec<String>>,
    category: String,
    #[serde(default)]
    level: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GenreRuleJson {
    genre: String,
    category: String,
    #[serde(default)]
    level: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct UtiRuleJson {
    uti: String,
    category: String,
    #[serde(default)]
    level: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    rules: Vec<CatalogRuleJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogRuleJson {
    field: String,
    value: String,
    category: String,
    #[serde(default)]
    level: Option<String>,
}

// --- Compiled rule sets ----------------------------------------------------

#[derive(Debug, Clone)]
pub struct TextRule {
    pub id: String,
    pub any: Vec<String>,
    pub all: Vec<String>,
    pub none: Vec<String>,
    pub all_groups: Vec<Vec<String>>,
    pub category: String,
    pub level: Option<ProductivityLevel>,
}

#[derive(Debug, Clone)]
pub struct MappingRuleSet {
    pub version: u32,
    pub text_rules: Vec<TextRule>,
    /// Lowercased genre → classification
    pub itunes_genres: Vec<(String, Classification)>,
    /// Lowercased UTI suffix (no `public.app-category.`) → classification
    pub ls_utis: Vec<(String, Classification)>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct CatalogRuleSet {
    pub version: u32,
    pub rules: Vec<CategoryRule>,
    pub source: String,
}

fn parse_level(s: Option<&str>) -> Option<ProductivityLevel> {
    match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
        Some("productive") => Some(ProductivityLevel::Productive),
        Some("neutral") => Some(ProductivityLevel::Neutral),
        Some("distracting") => Some(ProductivityLevel::Distracting),
        _ => None,
    }
}

fn classification(category: &str, level: Option<&str>) -> Classification {
    Classification {
        category: Some(category.to_string()),
        level: parse_level(level),
    }
}

fn parse_field(s: &str) -> Result<MatchField, StoreError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "bundle_id" | "bundleid" => Ok(MatchField::BundleId),
        "app_name" | "appname" => Ok(MatchField::AppName),
        "domain" => Ok(MatchField::Domain),
        "url" => Ok(MatchField::Url),
        "title" => Ok(MatchField::Title),
        other => Err(StoreError::Other(format!("unknown match field: {other}"))),
    }
}

impl MappingRuleSet {
    pub fn from_json(raw: &str, source: impl Into<String>) -> Result<Self, StoreError> {
        let file: MappingFile = serde_json::from_str(raw)
            .map_err(|e| StoreError::Other(format!("parse mapping rules: {e}")))?;
        let text_rules = file
            .text_rules
            .into_iter()
            .map(|r| TextRule {
                id: r.id,
                any: r.any.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
                all: r.all.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
                none: r.none.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
                all_groups: r
                    .all_groups
                    .into_iter()
                    .map(|g| g.into_iter().map(|s| s.to_ascii_lowercase()).collect())
                    .collect(),
                category: r.category,
                level: parse_level(r.level.as_deref()),
            })
            .collect();
        let itunes_genres = file
            .itunes_genre_rules
            .into_iter()
            .map(|r| {
                (
                    r.genre.to_ascii_lowercase(),
                    classification(&r.category, r.level.as_deref()),
                )
            })
            .collect();
        let ls_utis = file
            .ls_uti_rules
            .into_iter()
            .map(|r| {
                (
                    r.uti
                        .trim()
                        .trim_start_matches("public.app-category.")
                        .to_ascii_lowercase(),
                    classification(&r.category, r.level.as_deref()),
                )
            })
            .collect();
        Ok(Self {
            version: file.version,
            text_rules,
            itunes_genres,
            ls_utis,
            source: source.into(),
        })
    }

    pub fn embedded() -> Self {
        Self::from_json(EMBEDDED_MAPPING, "embedded:category_mapping.v1.json")
            .expect("embedded category_mapping.v1.json must parse")
    }

    /// First matching text rule wins (file order = priority).
    pub fn classify_text(&self, text: &str) -> Option<Classification> {
        let t = text.to_ascii_lowercase();
        for rule in &self.text_rules {
            if text_rule_matches(&t, rule) {
                return Some(Classification {
                    category: Some(rule.category.clone()),
                    level: rule.level,
                });
            }
        }
        None
    }

    pub fn classify_itunes_genre(&self, genre: &str) -> Option<Classification> {
        let g = genre.trim().to_ascii_lowercase();
        self.itunes_genres
            .iter()
            .find(|(key, _)| key == &g)
            .map(|(_, c)| c.clone())
    }

    pub fn classify_ls_uti(&self, uti: &str) -> Option<Classification> {
        let key = uti
            .trim()
            .trim_start_matches("public.app-category.")
            .to_ascii_lowercase();
        self.ls_utis
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, c)| c.clone())
    }
}

/// Fixed match semantics for text rules.
fn text_rule_matches(hay_lower: &str, rule: &TextRule) -> bool {
    // none: fail if any forbidden substring present
    if rule.none.iter().any(|n| hay_lower.contains(n.as_str())) {
        return false;
    }
    // all: every substring required
    if !rule.all.is_empty() && !rule.all.iter().all(|a| hay_lower.contains(a.as_str())) {
        return false;
    }
    // all_groups: each group needs ≥1 hit
    for group in &rule.all_groups {
        if group.is_empty() {
            continue;
        }
        if !group.iter().any(|a| hay_lower.contains(a.as_str())) {
            return false;
        }
    }
    // any: if non-empty, need ≥1 hit
    let any_ok = rule.any.is_empty() || rule.any.iter().any(|a| hay_lower.contains(a.as_str()));
    if !any_ok {
        return false;
    }
    // Must have at least one positive clause so empty rules never match everything.
    let has_positive = !rule.any.is_empty() || !rule.all.is_empty() || !rule.all_groups.is_empty();
    has_positive
}

impl CatalogRuleSet {
    pub fn from_json(raw: &str, source: impl Into<String>) -> Result<Self, StoreError> {
        let file: CatalogFile = serde_json::from_str(raw)
            .map_err(|e| StoreError::Other(format!("parse catalog rules: {e}")))?;
        let mut rules = Vec::with_capacity(file.rules.len());
        for r in file.rules {
            rules.push(CategoryRule {
                field: parse_field(&r.field)?,
                value: r.value,
                category: r.category,
                level: parse_level(r.level.as_deref()),
            });
        }
        Ok(Self {
            version: file.version,
            rules,
            source: source.into(),
        })
    }

    pub fn embedded() -> Self {
        Self::from_json(EMBEDDED_CATALOG, "embedded:app_catalog.v1.json")
            .expect("embedded app_catalog.v1.json must parse")
    }
}

// --- Global active sets (engine wiring) ------------------------------------

pub fn active_mapping() -> Arc<MappingRuleSet> {
    if let Ok(guard) = ACTIVE_MAPPING.read() {
        if let Some(ref s) = *guard {
            return Arc::clone(s);
        }
    }
    let embedded = Arc::new(MappingRuleSet::embedded());
    if let Ok(mut w) = ACTIVE_MAPPING.write() {
        *w = Some(Arc::clone(&embedded));
    }
    embedded
}

pub fn active_catalog() -> Arc<CatalogRuleSet> {
    if let Ok(guard) = ACTIVE_CATALOG.read() {
        if let Some(ref s) = *guard {
            return Arc::clone(s);
        }
    }
    let embedded = Arc::new(CatalogRuleSet::embedded());
    if let Ok(mut w) = ACTIVE_CATALOG.write() {
        *w = Some(Arc::clone(&embedded));
    }
    embedded
}

pub fn set_active_mapping(set: Arc<MappingRuleSet>) {
    if let Ok(mut w) = ACTIVE_MAPPING.write() {
        *w = Some(set);
    }
}

pub fn set_active_catalog(set: Arc<CatalogRuleSet>) {
    if let Ok(mut w) = ACTIVE_CATALOG.write() {
        *w = Some(set);
    }
}

/// Ensure `$data_dir/rules/` has editable copies of embedded defaults, then
/// load overrides from disk (file wins over embedded when present & valid).
pub fn install_and_load_rules(data_dir: &Path) -> Result<(Arc<MappingRuleSet>, Arc<CatalogRuleSet>), StoreError> {
    let rules_dir = data_dir.join("rules");
    std::fs::create_dir_all(&rules_dir).map_err(StoreError::io)?;

    let mapping_path = rules_dir.join("category_mapping.v1.json");
    let catalog_path = rules_dir.join("app_catalog.v1.json");

    seed_if_missing(&mapping_path, EMBEDDED_MAPPING)?;
    seed_if_missing(&catalog_path, EMBEDDED_CATALOG)?;

    let mapping = load_mapping_prefer_file(&mapping_path)?;
    let catalog = load_catalog_prefer_file(&catalog_path)?;
    set_active_mapping(Arc::clone(&mapping));
    set_active_catalog(Arc::clone(&catalog));
    tracing::info!(
        mapping = %mapping.source,
        catalog = %catalog.source,
        text_rules = mapping.text_rules.len(),
        catalog_rules = catalog.rules.len(),
        "category rule sets loaded"
    );
    Ok((mapping, catalog))
}

/// Reload from `$data_dir/rules/` without reseeding (for external file edits).
pub fn reload_rules_from_dir(data_dir: &Path) -> Result<(), StoreError> {
    let rules_dir = data_dir.join("rules");
    let mapping = load_mapping_prefer_file(&rules_dir.join("category_mapping.v1.json"))?;
    let catalog = load_catalog_prefer_file(&rules_dir.join("app_catalog.v1.json"))?;
    set_active_mapping(mapping);
    set_active_catalog(catalog);
    Ok(())
}

fn seed_if_missing(path: &Path, embedded: &str) -> Result<(), StoreError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(StoreError::io)?;
    }
    std::fs::write(path, embedded).map_err(StoreError::io)?;
    Ok(())
}

fn load_mapping_prefer_file(path: &Path) -> Result<Arc<MappingRuleSet>, StoreError> {
    if path.is_file() {
        match std::fs::read_to_string(path) {
            Ok(raw) => match MappingRuleSet::from_json(&raw, path.display().to_string()) {
                Ok(set) => return Ok(Arc::new(set)),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "invalid mapping rules file; falling back to embedded"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "read mapping rules failed");
            }
        }
    }
    Ok(Arc::new(MappingRuleSet::embedded()))
}

fn load_catalog_prefer_file(path: &Path) -> Result<Arc<CatalogRuleSet>, StoreError> {
    if path.is_file() {
        match std::fs::read_to_string(path) {
            Ok(raw) => match CatalogRuleSet::from_json(&raw, path.display().to_string()) {
                Ok(set) => return Ok(Arc::new(set)),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "invalid catalog rules file; falling back to embedded"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "read catalog rules failed");
            }
        }
    }
    Ok(Arc::new(CatalogRuleSet::embedded()))
}

pub fn rules_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("rules")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_mapping_parses_and_maps_teamwork() {
        let m = MappingRuleSet::embedded();
        assert!(!m.text_rules.is_empty());
        let c = m
            .classify_text("Teamwork app by Alibaba Group")
            .expect("teamwork");
        assert_eq!(c.category.as_deref(), Some("Communication"));
        let c2 = m
            .classify_text("NetEase UU remote desktop access and control tool")
            .expect("remote");
        assert_eq!(c2.category.as_deref(), Some("Utilities"));
    }

    #[test]
    fn embedded_catalog_has_ghostty() {
        let c = CatalogRuleSet::embedded();
        assert!(c.rules.iter().any(|r| r.value == "com.mitchellh.ghostty"));
    }

    #[test]
    fn file_override_without_recompile() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        install_and_load_rules(data).unwrap();
        let path = rules_dir(data).join("category_mapping.v1.json");
        // Inject a high-priority custom rule at the front of text_rules via full rewrite.
        let mut file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let custom = serde_json::json!({
            "id": "custom-foobar",
            "any": ["foobarbaz-unique-token"],
            "category": "Writing",
            "level": "productive"
        });
        file["text_rules"]
            .as_array_mut()
            .unwrap()
            .insert(0, custom);
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        reload_rules_from_dir(data).unwrap();
        let c = active_mapping()
            .classify_text("this mentions foobarbaz-unique-token here")
            .unwrap();
        assert_eq!(c.category.as_deref(), Some("Writing"));
    }

    #[test]
    fn text_rule_none_excludes() {
        let m = MappingRuleSet::embedded();
        // "file browser" should not become Browsing
        assert!(m.classify_text("A nice file browser utility").is_none()
            || m.classify_text("A nice file browser utility")
                .map(|c| c.category.as_deref() != Some("Browsing"))
                .unwrap_or(true));
    }

    #[test]
    fn remote_phrases_hit_but_loose_support_does_not() {
        let m = MappingRuleSet::embedded();
        assert_eq!(
            m.classify_text("NetEase UU remote desktop access and control tool")
                .and_then(|c| c.category),
            Some("Utilities".into())
        );
        // Must NOT treat marketing "remote work support" as remote-desktop tool.
        let loose = m.classify_text(
            "Notes and tasks for remote work support across your team",
        );
        assert!(
            loose
                .as_ref()
                .map(|c| c.category.as_deref() != Some("Utilities"))
                .unwrap_or(true),
            "unexpected Utilities for remote-work marketing blurb: {loose:?}"
        );
        // Lone "customer support" must not become Utilities via remote rules.
        let support = m.classify_text("Helpdesk customer support software");
        assert!(
            support
                .as_ref()
                .map(|c| c.category.as_deref() != Some("Utilities"))
                .unwrap_or(true),
            "unexpected Utilities for customer support: {support:?}"
        );
    }
}
