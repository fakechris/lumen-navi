//! Browser observation wire contract and privacy validation.

use std::collections::HashMap;
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use lumen_types::{SourceEvent, SourceKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub const BROWSER_SCHEMA_VERSION: u32 = 1;
pub const CAPTURE_PROFILE_V1: &str = "browser-mvp-v1";

pub mod event_kind {
    pub const NAVIGATION_COMMITTED_V1: &str = "browser.navigation_committed.v1";
    pub const DOCUMENT_READY_V1: &str = "browser.document_ready.v1";
    pub const VISIBILITY_FOCUS_CHANGE_V1: &str = "browser.visibility_focus_change.v1";
    pub const FEEDBACK_V1: &str = "browser.feedback.v1";
    pub const VISIT_CLOSED_V1: &str = "browser.visit_closed.v1";
    pub const HEALTH_V1: &str = "browser.health.v1";
    pub const GAP_V1: &str = "browser.gap.v1";

    pub fn is_v1(kind: &str) -> bool {
        matches!(
            kind,
            NAVIGATION_COMMITTED_V1
                | DOCUMENT_READY_V1
                | VISIBILITY_FOCUS_CHANGE_V1
                | FEEDBACK_V1
                | VISIT_CLOSED_V1
                | HEALTH_V1
                | GAP_V1
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserBatch {
    pub installation_id: String,
    pub schema_version: u32,
    pub capture_profile_version: String,
    pub config_hash: String,
    pub observations: Vec<BrowserObservation>,
    #[serde(default)]
    pub artifacts: Vec<BrowserArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserObservation {
    pub id: Uuid,
    pub kind: String,
    pub ts: DateTime<Utc>,
    pub visit_id: Uuid,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserArtifact {
    pub event_id: Uuid,
    pub media_type: String,
    pub body: String,
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BrowserIngestPolicy {
    pub content_allow_hosts: Vec<String>,
    pub excluded_hosts: Vec<String>,
    pub max_batch_size: usize,
    pub max_artifact_bytes: usize,
}

impl Default for BrowserIngestPolicy {
    fn default() -> Self {
        Self {
            content_allow_hosts: Vec::new(),
            excluded_hosts: vec![
                "mail.google.com".into(),
                "outlook.office.com".into(),
                "slack.com".into(),
                "discord.com".into(),
                "web.whatsapp.com".into(),
                "web.telegram.org".into(),
            ],
            max_batch_size: 100,
            max_artifact_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedArtifact {
    pub event_id: Uuid,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub claimed_content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValidatedBrowserBatch {
    pub events: Vec<SourceEvent>,
    pub artifacts: Vec<ValidatedArtifact>,
    pub rejected_artifacts: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("unsupported browser schema version: {0}")]
    SchemaVersion(u32),
    #[error("invalid installation id")]
    InstallationId,
    #[error("capture profile version or config hash is missing")]
    CaptureProfile,
    #[error("browser batch contains {actual} observations; maximum is {maximum}")]
    BatchSize { actual: usize, maximum: usize },
    #[error("unsupported browser event kind: {0}")]
    EventKind(String),
    #[error("invalid or unsafe URL: {0}")]
    UnsafeUrl(String),
    #[error("host is excluded from browser observation: {0}")]
    ExcludedHost(String),
    #[error("artifact refers to an unknown event: {0}")]
    ArtifactEvent(Uuid),
    #[error("unsupported browser artifact media type: {0}")]
    ArtifactMediaType(String),
    #[error("browser artifact is too large")]
    ArtifactTooLarge,
    #[error("content host is not explicitly allowed: {0}")]
    ContentHostNotAllowed(String),
    #[error("page did not pass the content privacy gate")]
    ContentPrivacyGate,
    #[error("browser payload contains forbidden field: {0}")]
    ForbiddenPayloadField(String),
}

#[derive(Debug)]
struct SanitizedUrl {
    value: String,
    host: String,
    origin: String,
    removed_keys: Vec<String>,
    sensitive_path: bool,
}

pub fn validate_batch(
    batch: BrowserBatch,
    policy: &BrowserIngestPolicy,
) -> Result<ValidatedBrowserBatch, ValidationError> {
    if batch.schema_version != BROWSER_SCHEMA_VERSION {
        return Err(ValidationError::SchemaVersion(batch.schema_version));
    }
    if Uuid::parse_str(&batch.installation_id).is_err() {
        return Err(ValidationError::InstallationId);
    }
    if batch.capture_profile_version.trim().is_empty()
        || batch.capture_profile_version.len() > 64
        || batch.config_hash.trim().is_empty()
        || batch.config_hash.len() > 128
    {
        return Err(ValidationError::CaptureProfile);
    }
    if batch.observations.is_empty() || batch.observations.len() > policy.max_batch_size {
        return Err(ValidationError::BatchSize {
            actual: batch.observations.len(),
            maximum: policy.max_batch_size,
        });
    }

    let mut event_context = HashMap::new();
    let mut event_index = HashMap::new();
    let mut events = Vec::with_capacity(batch.observations.len());
    for observation in &batch.observations {
        if !event_kind::is_v1(&observation.kind) {
            return Err(ValidationError::EventKind(observation.kind.clone()));
        }
        let sanitized = match observation.url.as_deref() {
            Some(raw) => {
                let value = sanitize_url(raw)?;
                if host_matches_any(&value.host, &policy.excluded_hosts) {
                    return Err(ValidationError::ExcludedHost(value.host));
                }
                Some(value)
            }
            None => None,
        };

        let sanitized_payload = sanitize_payload(&observation.payload, policy)?;
        let payload = json!({
            "installation_id": batch.installation_id,
            "schema_version": batch.schema_version,
            "capture_profile_version": batch.capture_profile_version,
            "config_hash": batch.config_hash,
            "visit_id": observation.visit_id,
            "document_id": observation.document_id,
            "url": sanitized.as_ref().map(|u| u.value.as_str()),
            "origin": sanitized.as_ref().map(|u| u.origin.as_str()),
            "domain": sanitized.as_ref().map(|u| u.host.as_str()),
            "url_redactions": sanitized.as_ref().map(|u| u.removed_keys.as_slice()).unwrap_or(&[]),
            "data": sanitized_payload,
        });
        events.push(SourceEvent {
            id: observation.id,
            source: SourceKind::Browser,
            kind: observation.kind.clone(),
            ts: observation.ts,
            session_id: Some(observation.visit_id),
            payload,
            artifacts: Vec::new(),
        });
        event_index.insert(observation.id, events.len() - 1);
        event_context.insert(observation.id, (observation, sanitized));
    }

    let mut artifacts = Vec::with_capacity(batch.artifacts.len());
    let mut rejected_artifacts = 0;
    for artifact in batch.artifacts {
        if artifact.media_type != "text/markdown" {
            return Err(ValidationError::ArtifactMediaType(artifact.media_type));
        }
        let (observation, sanitized) = event_context
            .get(&artifact.event_id)
            .ok_or(ValidationError::ArtifactEvent(artifact.event_id))?;
        if observation.kind != event_kind::DOCUMENT_READY_V1 {
            return Err(ValidationError::ContentPrivacyGate);
        }
        let url = sanitized
            .as_ref()
            .ok_or(ValidationError::ContentPrivacyGate)?;
        if !host_matches_any(&url.host, &policy.content_allow_hosts) {
            return Err(ValidationError::ContentHostNotAllowed(url.host.clone()));
        }
        if url.sensitive_path || !payload_allows_content(&observation.payload) {
            return Err(ValidationError::ContentPrivacyGate);
        }
        if artifact.body.len() > policy.max_artifact_bytes {
            rejected_artifacts += 1;
            if let Some(data) = event_index
                .get(&artifact.event_id)
                .and_then(|index| events.get_mut(*index))
                .and_then(|event| event.payload.get_mut("data"))
                .and_then(Value::as_object_mut)
            {
                data.insert("extraction_status".into(), json!("artifact_too_large"));
                data.insert("privacy_gate".into(), json!("metadata_only"));
            }
            continue;
        }
        artifacts.push(ValidatedArtifact {
            event_id: artifact.event_id,
            media_type: artifact.media_type,
            bytes: artifact.body.into_bytes(),
            claimed_content_hash: artifact.content_hash,
        });
    }

    Ok(ValidatedBrowserBatch {
        events,
        artifacts,
        rejected_artifacts,
    })
}

fn payload_allows_content(payload: &Value) -> bool {
    payload.get("privacy_gate").and_then(Value::as_str) == Some("allowed")
        && payload.get("extraction_status").and_then(Value::as_str) == Some("success")
        && !payload
            .get("has_password_input")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        && !payload
            .get("has_email_input")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        && !payload
            .get("has_contenteditable")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        && !payload
            .get("noindex")
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

fn sanitize_payload(value: &Value, policy: &BrowserIngestPolicy) -> Result<Value, ValidationError> {
    match value {
        Value::Object(object) => {
            let mut sanitized = serde_json::Map::with_capacity(object.len());
            for (key, value) in object {
                let normalized = key.to_ascii_lowercase();
                if [
                    "html",
                    "raw_html",
                    "markdown",
                    "body",
                    "text",
                    "input_value",
                    "form_value",
                    "selection",
                    "selected_text",
                    "clipboard",
                    "links",
                    "outlinks",
                    "candidates",
                    "candidate_impressions",
                ]
                .contains(&normalized.as_str())
                {
                    return Err(ValidationError::ForbiddenPayloadField(key.clone()));
                }
                if matches!(normalized.as_str(), "canonical" | "referrer") {
                    let safe_url = value
                        .as_str()
                        .and_then(|raw| sanitize_url(raw).ok())
                        .filter(|url| !host_matches_any(&url.host, &policy.excluded_hosts));
                    sanitized.insert(
                        key.clone(),
                        safe_url
                            .map(|url| Value::String(url.value))
                            .unwrap_or(Value::Null),
                    );
                } else {
                    sanitized.insert(key.clone(), sanitize_payload(value, policy)?);
                }
            }
            Ok(Value::Object(sanitized))
        }
        Value::Array(values) => values
            .iter()
            .map(|value| sanitize_payload(value, policy))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Ok(value.clone()),
    }
}

fn host_matches_any(host: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim().trim_start_matches('.').to_ascii_lowercase();
        !pattern.is_empty() && (host == pattern || host.ends_with(&format!(".{pattern}")))
    })
}

fn sanitize_url(raw: &str) -> Result<SanitizedUrl, ValidationError> {
    let mut url = Url::parse(raw).map_err(|_| ValidationError::UnsafeUrl("parse".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ValidationError::UnsafeUrl(url.scheme().into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ValidationError::UnsafeUrl("missing host".into()))?
        .to_ascii_lowercase();
    if host == "localhost"
        || !host.contains('.')
        || [".local", ".lan", ".home", ".internal", ".corp"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
        || host.parse::<IpAddr>().map(is_private_ip).unwrap_or(false)
    {
        return Err(ValidationError::UnsafeUrl(host));
    }

    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for (key, value) in url.query_pairs() {
        if is_sensitive_query_key(&key) {
            if !removed.iter().any(|existing| existing == key.as_ref()) {
                removed.push(key.into_owned());
            }
        } else {
            kept.push((key.into_owned(), value.into_owned()));
        }
    }
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    url.set_fragment(None);

    let path = url.path().to_ascii_lowercase();
    let sensitive_path = ["inbox", "messages", "message", "dm", "settings", "admin"]
        .iter()
        .any(|segment| path.split('/').any(|part| part == *segment));

    let origin = url.origin().ascii_serialization();
    Ok(SanitizedUrl {
        value: url.into(),
        host,
        origin,
        removed_keys: removed,
        sensitive_path,
    })
}

fn is_sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "access_token",
        "refresh_token",
        "session",
        "session_id",
        "auth",
        "authorization",
        "code",
        "email",
        "api_key",
        "apikey",
        "key",
        "signature",
        "sig",
    ]
    .contains(&key.as_str())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.to_ipv4_mapped().map(is_private_ipv4).unwrap_or(false)
        }
    }
}

fn is_private_ipv4(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        validate_batch, BrowserArtifact, BrowserBatch, BrowserIngestPolicy, BrowserObservation,
        ValidationError, BROWSER_SCHEMA_VERSION,
    };

    fn observation(url: &str) -> BrowserObservation {
        BrowserObservation {
            id: Uuid::new_v4(),
            kind: "browser.document_ready.v1".into(),
            ts: Utc::now(),
            visit_id: Uuid::new_v4(),
            document_id: Some("document-1".into()),
            url: Some(url.into()),
            payload: json!({
                "privacy_gate": "allowed",
                "has_password_input": false,
                "has_email_input": false,
                "has_contenteditable": false,
                "noindex": false,
                "extraction_status": "success"
            }),
        }
    }

    fn batch(observation: BrowserObservation) -> BrowserBatch {
        BrowserBatch {
            installation_id: "00000000-0000-4000-8000-000000000001".into(),
            schema_version: BROWSER_SCHEMA_VERSION,
            capture_profile_version: "browser-mvp-v1".into(),
            config_hash: "fixture-config-hash".into(),
            observations: vec![observation],
            artifacts: vec![],
        }
    }

    #[test]
    fn sanitizes_sensitive_url_values_before_persistence() {
        let input = observation(
            "https://example.test/article?topic=rust&token=secret&email=a%40example.test#magic",
        );
        let output = validate_batch(batch(input), &BrowserIngestPolicy::default()).unwrap();
        let url = output.events[0].payload["url"].as_str().unwrap();

        assert_eq!(url, "https://example.test/article?topic=rust");
        assert_eq!(
            output.events[0].payload["url_redactions"],
            json!(["token", "email"])
        );
    }

    #[test]
    fn markdown_requires_an_explicit_content_allow_host() {
        let input = observation("https://example.test/article");
        let event_id = input.id;
        let mut request = batch(input);
        request.artifacts.push(BrowserArtifact {
            event_id,
            media_type: "text/markdown".into(),
            body: "Synthetic article body.".into(),
            content_hash: None,
        });

        let error = validate_batch(request, &BrowserIngestPolicy::default()).unwrap_err();
        assert!(matches!(error, ValidationError::ContentHostNotAllowed(_)));
    }

    #[test]
    fn rejects_single_label_private_network_hosts() {
        let error = validate_batch(
            batch(observation("http://nas/article")),
            &BrowserIngestPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(error, ValidationError::UnsafeUrl(_)));
        let mapped = validate_batch(
            batch(observation("http://[::ffff:127.0.0.1]/article")),
            &BrowserIngestPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(mapped, ValidationError::UnsafeUrl(_)));
    }

    #[test]
    fn html_artifacts_are_rejected_even_on_allowed_hosts() {
        let input = observation("https://example.test/article");
        let event_id = input.id;
        let mut request = batch(input);
        request.artifacts.push(BrowserArtifact {
            event_id,
            media_type: "text/html".into(),
            body: "<p>Synthetic body</p>".into(),
            content_hash: None,
        });
        let policy = BrowserIngestPolicy {
            content_allow_hosts: vec!["example.test".into()],
            ..BrowserIngestPolicy::default()
        };

        let error = validate_batch(request, &policy).unwrap_err();
        assert!(matches!(error, ValidationError::ArtifactMediaType(_)));
    }

    #[test]
    fn artifact_limit_is_per_item_and_degrades_oversize_content_to_metadata() {
        let first = observation("https://example.test/first");
        let second = observation("https://example.test/second");
        let mut request = batch(first.clone());
        request.observations.push(second.clone());
        request.artifacts = vec![
            BrowserArtifact {
                event_id: first.id,
                media_type: "text/markdown".into(),
                body: "123456".into(),
                content_hash: None,
            },
            BrowserArtifact {
                event_id: second.id,
                media_type: "text/markdown".into(),
                body: "abcdef".into(),
                content_hash: None,
            },
        ];
        let policy = BrowserIngestPolicy {
            content_allow_hosts: vec!["example.test".into()],
            max_artifact_bytes: 6,
            ..BrowserIngestPolicy::default()
        };
        let accepted = validate_batch(request, &policy).unwrap();
        assert_eq!(accepted.artifacts.len(), 2);
        assert_eq!(accepted.rejected_artifacts, 0);

        let oversized = observation("https://example.test/oversized");
        let event_id = oversized.id;
        let mut request = batch(oversized);
        request.artifacts.push(BrowserArtifact {
            event_id,
            media_type: "text/markdown".into(),
            body: "1234567".into(),
            content_hash: None,
        });
        let degraded = validate_batch(request, &policy).unwrap();
        assert!(degraded.artifacts.is_empty());
        assert_eq!(degraded.rejected_artifacts, 1);
        assert_eq!(
            degraded.events[0].payload["data"]["extraction_status"],
            "artifact_too_large"
        );
    }

    #[test]
    fn nested_metadata_urls_are_sanitized_and_payload_content_is_rejected() {
        let mut input = observation("https://example.test/article");
        input.payload["referrer"] = json!("https://source.example.test/list?token=secret#row");
        let output = validate_batch(batch(input.clone()), &BrowserIngestPolicy::default()).unwrap();
        assert_eq!(
            output.events[0].payload["data"]["referrer"],
            "https://source.example.test/list"
        );

        input.payload["html"] = json!("<p>must not bypass artifact validation</p>");
        let error = validate_batch(batch(input), &BrowserIngestPolicy::default()).unwrap_err();
        assert!(matches!(error, ValidationError::ForbiddenPayloadField(_)));
    }
}
