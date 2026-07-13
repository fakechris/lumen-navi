use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{BrowserContext, CaptureId, TargetHint};

pub const BROWSER_CONTEXT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserCaptureRequest {
    pub request_id: Uuid,
    pub capture_id: CaptureId,
    pub target_generation: u64,
    pub target_hint: Option<TargetHint>,
    pub requested_at: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
    pub max_chars: usize,
    pub max_nodes: usize,
    pub allow_private_browsing: bool,
    pub denied_bundle_ids: BTreeSet<String>,
    pub denied_domains: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserFrameStatus {
    pub frame_id: i64,
    pub document_id: Option<String>,
    pub origin: Option<String>,
    pub captured: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSnapshot {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub capture_id: CaptureId,
    pub target_generation: u64,
    pub started_tab_id: Option<i64>,
    pub completed_tab_id: Option<i64>,
    pub started_navigation_id: Option<String>,
    pub completed_navigation_id: Option<String>,
    pub started_document_id: Option<String>,
    pub completed_document_id: Option<String>,
    pub context: BrowserContext,
    pub frame_status: Vec<BrowserFrameStatus>,
    pub captured_at: DateTime<Utc>,
    pub extension_version: Option<String>,
}

#[derive(Debug, Error)]
pub enum BrowserCaptureError {
    #[error("browser context is unavailable: {0}")]
    Unavailable(String),
    #[error("browser context permission denied: {0}")]
    Denied(String),
    #[error("browser target became stale: {0}")]
    Stale(String),
    #[error("browser capture timed out: {0}")]
    Timeout(String),
    #[error("browser capture failed: {0}")]
    Failed(String),
}

impl BrowserCaptureError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "browser_unavailable",
            Self::Denied(_) => "browser_permission_denied",
            Self::Stale(_) => "browser_target_stale",
            Self::Timeout(_) => "browser_timeout",
            Self::Failed(_) => "browser_capture_failed",
        }
    }
}

impl BrowserSnapshot {
    pub fn validate_for(
        self,
        request: &BrowserCaptureRequest,
    ) -> Result<BrowserContext, BrowserCaptureError> {
        if self.schema_version != BROWSER_CONTEXT_SCHEMA_VERSION {
            return Err(BrowserCaptureError::Failed(format!(
                "unsupported browser schema version {}",
                self.schema_version
            )));
        }
        if self.request_id != request.request_id
            || self.capture_id != request.capture_id
            || self.target_generation != request.target_generation
        {
            return Err(BrowserCaptureError::Stale(
                "response correlation does not match the capture request".to_owned(),
            ));
        }
        if self.captured_at > request.deadline {
            return Err(BrowserCaptureError::Timeout(
                "snapshot completed after its deadline".to_owned(),
            ));
        }
        ensure_stable(
            "tab",
            self.started_tab_id.as_ref(),
            self.completed_tab_id.as_ref(),
        )?;
        ensure_stable(
            "navigation",
            self.started_navigation_id.as_ref(),
            self.completed_navigation_id.as_ref(),
        )?;
        ensure_stable(
            "document",
            self.started_document_id.as_ref(),
            self.completed_document_id.as_ref(),
        )?;
        if self.context.tab_id != self.completed_tab_id
            || self.context.navigation_id != self.completed_navigation_id
            || self.context.document_id != self.completed_document_id
        {
            return Err(BrowserCaptureError::Stale(
                "context identity does not match the completed browser target".to_owned(),
            ));
        }
        if self.context.incognito && !request.allow_private_browsing {
            return Err(BrowserCaptureError::Denied(
                "private browsing capture is disabled".to_owned(),
            ));
        }
        if let Some(domain) = self.context.domain.as_deref() {
            if request
                .denied_domains
                .iter()
                .any(|denied| domain_matches(domain, denied))
            {
                return Err(BrowserCaptureError::Denied(
                    "domain is excluded by local capture policy".to_owned(),
                ));
            }
        }
        if let Some(hint) = request.target_hint.as_ref() {
            if hint
                .bundle_id
                .as_ref()
                .is_some_and(|bundle_id| request.denied_bundle_ids.contains(bundle_id))
            {
                return Err(BrowserCaptureError::Denied(
                    "browser application is excluded by local capture policy".to_owned(),
                ));
            }
            if hint.pid.is_some()
                && self.context.browser_pid.is_some()
                && hint.pid != self.context.browser_pid
            {
                return Err(BrowserCaptureError::Stale(
                    "browser process changed during capture".to_owned(),
                ));
            }
            if hint.bundle_id.is_some()
                && self.context.browser_bundle_id.is_some()
                && hint.bundle_id != self.context.browser_bundle_id
            {
                return Err(BrowserCaptureError::Stale(
                    "browser application changed during capture".to_owned(),
                ));
            }
        }
        if self.context.viewport_text_blocks.len() > request.max_nodes {
            return Err(BrowserCaptureError::Failed(
                "browser snapshot exceeded the node limit".to_owned(),
            ));
        }
        if browser_char_count(&self.context) > request.max_chars {
            return Err(BrowserCaptureError::Failed(
                "browser snapshot exceeded the character limit".to_owned(),
            ));
        }
        Ok(self.context)
    }
}

fn ensure_stable<T: PartialEq>(
    name: &str,
    started: Option<&T>,
    completed: Option<&T>,
) -> Result<(), BrowserCaptureError> {
    if started != completed {
        Err(BrowserCaptureError::Stale(format!(
            "{name} changed during extraction"
        )))
    } else {
        Ok(())
    }
}

fn domain_matches(domain: &str, denied: &str) -> bool {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let denied = denied
        .trim()
        .trim_start_matches("*.")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    !denied.is_empty() && (domain == denied || domain.ends_with(&format!(".{denied}")))
}

fn browser_char_count(context: &BrowserContext) -> usize {
    let fields = [
        context.title.as_deref(),
        context.url.as_deref(),
        context.selection_text.as_deref(),
        context.nearby_before.as_deref(),
        context.nearby_after.as_deref(),
        context
            .focused_element
            .as_ref()
            .and_then(|element| element.value.as_deref()),
    ];
    fields
        .into_iter()
        .flatten()
        .map(|value| value.chars().count())
        .sum::<usize>()
        + context
            .viewport_text_blocks
            .iter()
            .map(|block| block.text.chars().count())
            .sum::<usize>()
}

#[async_trait]
pub trait BrowserSnapshotProvider: Send + Sync {
    async fn capture(
        &self,
        request: BrowserCaptureRequest,
    ) -> Result<BrowserSnapshot, BrowserCaptureError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserContext;

    fn request() -> BrowserCaptureRequest {
        let now = Utc::now();
        BrowserCaptureRequest {
            request_id: Uuid::new_v4(),
            capture_id: CaptureId::new(),
            target_generation: 7,
            target_hint: None,
            requested_at: now,
            deadline: now + chrono::Duration::seconds(1),
            max_chars: 1_000,
            max_nodes: 10,
            allow_private_browsing: false,
            denied_bundle_ids: BTreeSet::new(),
            denied_domains: BTreeSet::new(),
        }
    }

    fn snapshot(request: &BrowserCaptureRequest) -> BrowserSnapshot {
        BrowserSnapshot {
            schema_version: BROWSER_CONTEXT_SCHEMA_VERSION,
            request_id: request.request_id,
            capture_id: request.capture_id,
            target_generation: request.target_generation,
            started_tab_id: Some(3),
            completed_tab_id: Some(3),
            started_navigation_id: Some("nav-1".to_owned()),
            completed_navigation_id: Some("nav-1".to_owned()),
            started_document_id: Some("doc-1".to_owned()),
            completed_document_id: Some("doc-1".to_owned()),
            context: BrowserContext {
                tab_id: Some(3),
                navigation_id: Some("nav-1".to_owned()),
                document_id: Some("doc-1".to_owned()),
                domain: Some("docs.example.com".to_owned()),
                ..BrowserContext::default()
            },
            frame_status: Vec::new(),
            captured_at: request.requested_at,
            extension_version: Some("0.1.0".to_owned()),
        }
    }

    #[test]
    fn accepts_correlated_stable_snapshot() {
        let request = request();
        assert!(snapshot(&request).validate_for(&request).is_ok());
    }

    #[test]
    fn rejects_navigation_change_and_denied_subdomain() {
        let mut request = request();
        let mut changed = snapshot(&request);
        changed.completed_navigation_id = Some("nav-2".to_owned());
        assert!(matches!(
            changed.validate_for(&request),
            Err(BrowserCaptureError::Stale(_))
        ));

        request.denied_domains.insert("example.com".to_owned());
        assert!(matches!(
            snapshot(&request).validate_for(&request),
            Err(BrowserCaptureError::Denied(_))
        ));
    }
}
