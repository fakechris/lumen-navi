use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use thiserror::Error;
use tokio::sync::Notify;

use crate::browser::{BrowserCaptureRequest, BrowserSnapshotProvider};
use crate::types::{
    CaptureRequest, ContextConfig, ContextManifest, ContextSnapshot, PrivacyContext, SourceCapture,
    SourceKind, SourceState, SourceStatus, CONTEXT_SCHEMA_VERSION,
};

#[derive(Debug, Error)]
pub enum ContextInitError {
    #[error("invalid context config: {0}")]
    InvalidConfig(String),
    #[error("duplicate context source: {0:?}")]
    DuplicateSource(SourceKind),
}

#[derive(Debug, Error)]
pub enum CaptureStartError {
    #[error("context capture requires an active Tokio runtime")]
    RuntimeUnavailable,
}

#[derive(Debug)]
pub(crate) struct SourceError {
    state: SourceState,
    code: String,
    message: String,
    retryable: bool,
}

impl SourceError {
    pub(crate) fn failed(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            state: SourceState::Failed,
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }

    pub(crate) fn from_platform(error: crate::PlatformError) -> Self {
        match error {
            crate::PlatformError::PermissionDenied(message) => Self {
                state: SourceState::Denied,
                code: "accessibility_permission_denied".to_owned(),
                message,
                retryable: false,
            },
            crate::PlatformError::Unsupported(message) => Self {
                state: SourceState::Unsupported,
                code: "platform_unsupported".to_owned(),
                message,
                retryable: false,
            },
            crate::PlatformError::Message(message) => {
                Self::failed("platform_capture_failed", message, true)
            }
        }
    }

    pub(crate) fn stale(message: impl Into<String>) -> Self {
        Self {
            state: SourceState::Stale,
            code: "target_generation_stale".to_owned(),
            message: message.into(),
            retryable: false,
        }
    }

    pub(crate) fn skipped_policy(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            state: SourceState::SkippedPolicy,
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }
}

#[async_trait]
pub(crate) trait ContextSource: Send + Sync {
    fn kind(&self) -> SourceKind;

    fn dependencies(&self) -> &'static [SourceKind] {
        &[]
    }

    fn optional_dependencies(&self) -> &'static [SourceKind] {
        &[]
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        input: SourceInput,
    ) -> Result<SourceCapture, SourceError>;
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SourceInput {
    pub(crate) target: Option<crate::TargetContext>,
    pub(crate) editor: Option<crate::EditorContext>,
    pub(crate) ax_visible: Option<crate::AxVisibleContext>,
    pub(crate) browser: Option<crate::BrowserContext>,
    pub(crate) screenshots: Vec<crate::ScreenshotDocument>,
    pub(crate) ocr_documents: Vec<crate::OcrDocument>,
    pub(crate) artifacts: Vec<crate::CapturedArtifact>,
}

struct BrowserSource {
    provider: Arc<dyn BrowserSnapshotProvider>,
    timeout: std::time::Duration,
    max_chars: usize,
    max_nodes: usize,
}

#[async_trait]
impl ContextSource for BrowserSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Browser
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        _input: SourceInput,
    ) -> Result<SourceCapture, SourceError> {
        if !request.privacy_policy.capture_raw_text {
            return Err(SourceError::skipped_policy(
                "browser_raw_text_disabled",
                "browser DOM capture is disabled by raw-text policy",
            ));
        }
        let requested_at = Utc::now();
        let browser_request = BrowserCaptureRequest {
            request_id: uuid::Uuid::new_v4(),
            capture_id: request.capture_id,
            target_generation: request.target_generation,
            target_hint: request.target_hint.clone(),
            requested_at,
            deadline: requested_at
                + chrono::Duration::milliseconds(
                    self.timeout.as_millis().min(i64::MAX as u128) as i64
                ),
            max_chars: self.max_chars,
            max_nodes: self.max_nodes,
            allow_private_browsing: request.privacy_policy.allow_private_browsing,
            denied_bundle_ids: request.privacy_policy.denied_bundle_ids.clone(),
            denied_domains: request.privacy_policy.denied_domains.clone(),
        };
        let snapshot =
            tokio::time::timeout(self.timeout, self.provider.capture(browser_request.clone()))
                .await
                .map_err(|_| {
                    browser_source_error(crate::BrowserCaptureError::Timeout(
                        "provider deadline elapsed".to_owned(),
                    ))
                })?
                .map_err(browser_source_error)?;
        let browser = snapshot
            .validate_for(&browser_request)
            .map_err(browser_source_error)?;

        let empty = browser.title.is_none()
            && browser.url.is_none()
            && browser.selection_text.is_none()
            && browser.focused_element.is_none()
            && browser.viewport_text_blocks.is_empty();
        let mut captured = SourceCapture::browser(browser);
        captured.empty = empty;
        Ok(captured)
    }
}

fn browser_source_error(error: crate::BrowserCaptureError) -> SourceError {
    use crate::BrowserCaptureError;

    let state = match &error {
        BrowserCaptureError::Unavailable(_) => SourceState::Unavailable,
        BrowserCaptureError::Denied(_) => SourceState::Denied,
        BrowserCaptureError::Stale(_) => SourceState::Stale,
        BrowserCaptureError::Timeout(_) => SourceState::Timeout,
        BrowserCaptureError::Failed(_) => SourceState::Failed,
    };
    SourceError {
        state,
        code: error.code().to_owned(),
        message: error.to_string(),
        retryable: matches!(
            error,
            BrowserCaptureError::Unavailable(_)
                | BrowserCaptureError::Timeout(_)
                | BrowserCaptureError::Failed(_)
        ),
    }
}

pub struct ContextCollector {
    config: ContextConfig,
    sources: Arc<BTreeMap<SourceKind, Arc<dyn ContextSource>>>,
}

impl ContextCollector {
    pub fn new(
        config: ContextConfig,
        browser: Option<Arc<dyn BrowserSnapshotProvider>>,
    ) -> Result<Self, ContextInitError> {
        validate_config(&config)?;

        let mut sources: Vec<Arc<dyn ContextSource>> = Vec::new();
        sources.extend(crate::macos::default_ax_sources(&config));
        sources.extend(crate::macos::default_screen_sources(&config));
        sources.extend(crate::macos::default_vision_sources(&config));
        sources.push(Arc::new(crate::fusion::VisibleTextFusionSource));
        if let Some(provider) = browser {
            sources.push(Arc::new(BrowserSource {
                provider,
                timeout: std::time::Duration::from_millis(config.browser_timeout_ms),
                max_chars: config.browser_max_chars,
                max_nodes: config.browser_max_nodes,
            }));
        }
        Self::build(config, sources)
    }

    fn build(
        config: ContextConfig,
        sources: Vec<Arc<dyn ContextSource>>,
    ) -> Result<Self, ContextInitError> {
        let mut by_kind = BTreeMap::new();
        for source in sources {
            let kind = source.kind();
            if by_kind.insert(kind, source).is_some() {
                return Err(ContextInitError::DuplicateSource(kind));
            }
        }
        Ok(Self {
            config,
            sources: Arc::new(by_kind),
        })
    }

    pub fn begin(&self, mut request: CaptureRequest) -> Result<CaptureSession, CaptureStartError> {
        expand_dependencies(&mut request.sources.requested, &self.sources);
        let runnable: Vec<_> = request
            .sources
            .requested
            .iter()
            .filter_map(|kind| self.sources.get(kind).cloned())
            .collect();
        let runtime = if runnable.is_empty() {
            None
        } else {
            Some(
                tokio::runtime::Handle::try_current()
                    .map_err(|_| CaptureStartError::RuntimeUnavailable)?,
            )
        };

        let now = Utc::now();
        let runnable_kinds: BTreeSet<_> = runnable.iter().map(|source| source.kind()).collect();
        let mut statuses = BTreeMap::new();
        for source in &request.sources.requested {
            let mut status = if runnable_kinds.contains(source) {
                SourceStatus::new(*source, SourceState::Queued, request.target_generation)
            } else {
                SourceStatus::new(*source, SourceState::Unavailable, request.target_generation)
            };
            if !runnable_kinds.contains(source) {
                status.completed_at = Some(now);
                status.reason_code = Some("source_not_configured".to_owned());
                status.message = Some("requested source is not configured".to_owned());
            }
            statuses.insert(*source, status);
        }

        let manifest = ContextManifest {
            schema_version: CONTEXT_SCHEMA_VERSION,
            capture_id: request.capture_id,
            consumer_session_id: request.consumer_session_id,
            revision: 1,
            profile: request.profile,
            trigger: request.trigger.clone(),
            requested_at: request.requested_at,
            frozen_at: now,
            target_generation: request.target_generation,
            target: None,
            system: None,
            editor: None,
            ax_visible: None,
            browser: None,
            screenshots: Vec::new(),
            ocr_documents: Vec::new(),
            visible_text_fused: None,
            artifacts: Vec::new(),
            source_status: statuses,
            privacy: PrivacyContext {
                raw_text_allowed: request.privacy_policy.capture_raw_text,
                screenshots_allowed: request.privacy_policy.capture_screenshots,
                applied_gates: Vec::new(),
                policy_reason: None,
            },
            diagnostics: Default::default(),
        };

        let shared = Arc::new(SharedCapture {
            state: Mutex::new(CaptureState {
                manifest,
                payloads: Vec::new(),
            }),
            notify: Notify::new(),
            max_payload_bytes: self.config.max_payload_bytes_per_capture,
        });

        if let Some(runtime) = runtime {
            for source in runnable {
                let source_request = request.clone();
                let source_shared = shared.clone();
                runtime.spawn(async move {
                    run_source(source, source_request, source_shared).await;
                });
            }
        }

        Ok(CaptureSession {
            capture_id: request.capture_id,
            shared,
        })
    }

    #[cfg(test)]
    fn with_sources(
        config: ContextConfig,
        sources: Vec<Arc<dyn ContextSource>>,
    ) -> Result<Self, ContextInitError> {
        validate_config(&config)?;
        Self::build(config, sources)
    }
}

fn validate_config(config: &ContextConfig) -> Result<(), ContextInitError> {
    if config.ax_max_chars == 0 {
        return Err(ContextInitError::InvalidConfig(
            "ax_max_chars must be greater than zero".to_owned(),
        ));
    }
    if config.ax_max_nodes == 0 {
        return Err(ContextInitError::InvalidConfig(
            "ax_max_nodes must be greater than zero".to_owned(),
        ));
    }
    if config.ax_max_depth == 0 || config.ax_max_depth > 64 {
        return Err(ContextInitError::InvalidConfig(
            "ax_max_depth must be between 1 and 64".to_owned(),
        ));
    }
    if config.ax_timeout_ms == 0 {
        return Err(ContextInitError::InvalidConfig(
            "ax_timeout_ms must be greater than zero".to_owned(),
        ));
    }
    if config.screenshot_jpeg_quality == 0 || config.screenshot_jpeg_quality > 100 {
        return Err(ContextInitError::InvalidConfig(
            "screenshot_jpeg_quality must be between 1 and 100".to_owned(),
        ));
    }
    if config.screenshot_timeout_ms == 0 {
        return Err(ContextInitError::InvalidConfig(
            "screenshot_timeout_ms must be greater than zero".to_owned(),
        ));
    }
    if config.ocr_languages.is_empty() {
        return Err(ContextInitError::InvalidConfig(
            "ocr_languages must not be empty".to_owned(),
        ));
    }
    if config.ocr_max_image_bytes == 0 {
        return Err(ContextInitError::InvalidConfig(
            "ocr_max_image_bytes must be greater than zero".to_owned(),
        ));
    }
    if config.ocr_helper_timeout_ms == 0 {
        return Err(ContextInitError::InvalidConfig(
            "ocr_helper_timeout_ms must be greater than zero".to_owned(),
        ));
    }
    if config.browser_timeout_ms == 0 {
        return Err(ContextInitError::InvalidConfig(
            "browser_timeout_ms must be greater than zero".to_owned(),
        ));
    }
    if config.browser_max_chars == 0 {
        return Err(ContextInitError::InvalidConfig(
            "browser_max_chars must be greater than zero".to_owned(),
        ));
    }
    if config.browser_max_nodes == 0 {
        return Err(ContextInitError::InvalidConfig(
            "browser_max_nodes must be greater than zero".to_owned(),
        ));
    }
    if config.max_payload_bytes_per_capture == 0 {
        return Err(ContextInitError::InvalidConfig(
            "max_payload_bytes_per_capture must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

pub struct CaptureSession {
    capture_id: crate::CaptureId,
    shared: Arc<SharedCapture>,
}

impl CaptureSession {
    pub fn capture_id(&self) -> crate::CaptureId {
        self.capture_id
    }

    pub async fn snapshot(&self, deadline: Instant) -> ContextSnapshot {
        loop {
            let notified = self.shared.notify.notified();
            if self.is_terminal() || Instant::now() >= deadline {
                return self.current_snapshot();
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return self.current_snapshot();
            }
        }
    }

    fn is_terminal(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .manifest
            .all_requested_sources_terminal()
    }

    fn current_snapshot(&self) -> ContextSnapshot {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let frozen_at = Utc::now();
        state.manifest.revision += 1;
        state.manifest.frozen_at = frozen_at;
        state.manifest.diagnostics.total_duration_ms =
            Some(nonnegative_millis(frozen_at - state.manifest.requested_at));
        ContextSnapshot {
            manifest: state.manifest.clone(),
            payloads: state.payloads.clone(),
        }
    }
}

struct SharedCapture {
    state: Mutex<CaptureState>,
    notify: Notify,
    max_payload_bytes: usize,
}

struct CaptureState {
    manifest: ContextManifest,
    payloads: Vec<crate::CapturedArtifact>,
}

async fn run_source(
    source: Arc<dyn ContextSource>,
    request: CaptureRequest,
    shared: Arc<SharedCapture>,
) {
    let kind = source.kind();
    let input = match wait_for_dependencies(
        source.dependencies(),
        source.optional_dependencies(),
        &shared,
    )
    .await
    {
        Ok(input) => input,
        Err(error) => {
            let completed_at = Utc::now();
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let policy_error = capture_policy_error(kind, &request, state.manifest.target.as_ref());
            finish_error(
                &mut state,
                kind,
                completed_at,
                0,
                policy_error.unwrap_or(error),
            );
            state.manifest.revision += 1;
            drop(state);
            shared.notify.notify_waiters();
            return;
        }
    };
    if let Some(error) = capture_policy_error(kind, &request, input.target.as_ref()) {
        let completed_at = Utc::now();
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.manifest.privacy.applied_gates.contains(&error.code) {
            state
                .manifest
                .privacy
                .applied_gates
                .push(error.code.clone());
        }
        finish_error(&mut state, kind, completed_at, 0, error);
        state.manifest.revision += 1;
        drop(state);
        shared.notify.notify_waiters();
        return;
    }
    let started_at = Utc::now();
    let queue_wait_ms = nonnegative_millis(started_at - request.requested_at);
    {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(status) = state.manifest.source_status.get_mut(&kind) {
            status.state = SourceState::Running;
            status.started_at = Some(started_at);
            status.queue_wait_ms = Some(queue_wait_ms);
            state.manifest.revision += 1;
        }
    }
    shared.notify.notify_waiters();

    let started = Instant::now();
    let result = source.capture(&request, input).await;
    let completed_at = Utc::now();
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match result {
        Ok(captured) => {
            let payload_bytes = captured
                .artifacts
                .iter()
                .map(|artifact| artifact.descriptor.bytes)
                .sum::<u64>();
            let current_bytes = state.manifest.diagnostics.payload_bytes;
            if current_bytes.saturating_add(payload_bytes) > shared.max_payload_bytes as u64 {
                finish_error(
                    &mut state,
                    kind,
                    completed_at,
                    duration_ms,
                    SourceError::failed(
                        "capture_payload_budget_exceeded",
                        "source payload exceeded the per-capture budget",
                        false,
                    ),
                );
            } else {
                apply_capture(&mut state, captured, kind, completed_at, duration_ms);
            }
        }
        Err(error) => finish_error(&mut state, kind, completed_at, duration_ms, error),
    }
    state.manifest.revision += 1;
    drop(state);
    shared.notify.notify_waiters();
}

fn capture_policy_error(
    kind: SourceKind,
    request: &CaptureRequest,
    target: Option<&crate::TargetContext>,
) -> Option<SourceError> {
    if kind != SourceKind::Target
        && target
            .and_then(|target| target.bundle_id.as_ref())
            .is_some_and(|bundle_id| request.privacy_policy.denied_bundle_ids.contains(bundle_id))
    {
        return Some(SourceError::skipped_policy(
            "bundle_denied",
            "target application is excluded by local capture policy",
        ));
    }
    if !request.privacy_policy.capture_raw_text
        && matches!(
            kind,
            SourceKind::EditorAx
                | SourceKind::AxVisible
                | SourceKind::Browser
                | SourceKind::OcrElement
                | SourceKind::OcrWindow
                | SourceKind::OcrDisplays
                | SourceKind::VisibleTextFusion
        )
    {
        let (code, message) = if kind == SourceKind::Browser {
            (
                "browser_raw_text_disabled",
                "browser DOM capture is disabled by raw-text policy",
            )
        } else {
            (
                "raw_text_disabled",
                "raw text capture is disabled by local capture policy",
            )
        };
        return Some(SourceError::skipped_policy(code, message));
    }
    None
}

fn expand_dependencies(
    requested: &mut BTreeSet<SourceKind>,
    sources: &BTreeMap<SourceKind, Arc<dyn ContextSource>>,
) {
    loop {
        let dependencies: Vec<_> = requested
            .iter()
            .filter_map(|kind| sources.get(kind))
            .flat_map(|source| source.dependencies().iter().copied())
            .collect();
        let before = requested.len();
        requested.extend(dependencies);
        if requested.len() == before {
            break;
        }
    }
}

async fn wait_for_dependencies(
    dependencies: &[SourceKind],
    optional_dependencies: &[SourceKind],
    shared: &SharedCapture,
) -> Result<SourceInput, SourceError> {
    loop {
        let notified = shared.notify.notified();
        let decision = {
            let state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            dependency_decision(dependencies, optional_dependencies, &state)
        };
        match decision {
            DependencyDecision::Ready(input) => return Ok(*input),
            DependencyDecision::Failed(error) => return Err(error),
            DependencyDecision::Waiting => notified.await,
        }
    }
}

enum DependencyDecision {
    Ready(Box<SourceInput>),
    Failed(SourceError),
    Waiting,
}

fn dependency_decision(
    dependencies: &[SourceKind],
    optional_dependencies: &[SourceKind],
    state: &CaptureState,
) -> DependencyDecision {
    for dependency in dependencies {
        let Some(status) = state.manifest.source_status.get(dependency) else {
            return DependencyDecision::Failed(SourceError::failed(
                "dependency_not_requested",
                format!("required source {dependency:?} was not requested"),
                false,
            ));
        };
        if !status.state.is_terminal() {
            return DependencyDecision::Waiting;
        }
        if !matches!(status.state, SourceState::Succeeded | SourceState::Empty) {
            let state = match status.state {
                SourceState::Denied => SourceState::Denied,
                SourceState::SkippedPolicy => SourceState::SkippedPolicy,
                SourceState::Stale => SourceState::Stale,
                SourceState::Unsupported => SourceState::Unsupported,
                SourceState::Timeout => SourceState::Timeout,
                SourceState::Cancelled => SourceState::Cancelled,
                _ => SourceState::Unavailable,
            };
            let policy_failure = matches!(state, SourceState::Denied | SourceState::SkippedPolicy);
            return DependencyDecision::Failed(SourceError {
                state,
                code: if policy_failure {
                    status
                        .reason_code
                        .clone()
                        .unwrap_or_else(|| "dependency_policy_blocked".to_owned())
                } else {
                    "dependency_unavailable".to_owned()
                },
                message: if policy_failure {
                    status.message.clone().unwrap_or_else(|| {
                        format!("required source {dependency:?} was blocked by policy")
                    })
                } else {
                    format!("required source {dependency:?} did not produce input")
                },
                retryable: status.retryable,
            });
        }
    }
    for dependency in optional_dependencies {
        if state
            .manifest
            .source_status
            .get(dependency)
            .is_some_and(|status| !status.state.is_terminal())
        {
            return DependencyDecision::Waiting;
        }
    }
    let dependency_set: BTreeSet<_> = dependencies
        .iter()
        .chain(optional_dependencies.iter())
        .filter(|dependency| state.manifest.source_status.contains_key(dependency))
        .copied()
        .collect();
    DependencyDecision::Ready(Box::new(SourceInput {
        target: state.manifest.target.clone(),
        editor: state.manifest.editor.clone(),
        ax_visible: state.manifest.ax_visible.clone(),
        browser: state.manifest.browser.clone(),
        screenshots: state
            .manifest
            .screenshots
            .iter()
            .filter(|screenshot| {
                state.manifest.artifacts.iter().any(|artifact| {
                    artifact.artifact_id == screenshot.artifact_id
                        && dependency_set.contains(&artifact.source)
                })
            })
            .cloned()
            .collect(),
        ocr_documents: state.manifest.ocr_documents.clone(),
        artifacts: state
            .payloads
            .iter()
            .filter(|artifact| dependency_set.contains(&artifact.descriptor.source))
            .cloned()
            .collect(),
    }))
}

fn apply_capture(
    state: &mut CaptureState,
    captured: SourceCapture,
    kind: SourceKind,
    completed_at: chrono::DateTime<Utc>,
    duration_ms: u64,
) {
    for screenshot in &captured.screenshots {
        if let Some(reason) = screenshot.capture_fallback_reason.as_ref() {
            state
                .manifest
                .diagnostics
                .warnings
                .push(format!("{:?} fallback: {reason}", screenshot.kind));
        }
    }
    if let Some(target) = captured.target {
        state.manifest.target = Some(target);
    }
    if let Some(system) = captured.system {
        state.manifest.system = Some(system);
    }
    if let Some(editor) = captured.editor {
        state.manifest.editor = Some(editor);
    }
    if let Some(ax_visible) = captured.ax_visible {
        state.manifest.ax_visible = Some(ax_visible);
    }
    if let Some(browser) = captured.browser {
        state.manifest.browser = Some(browser);
    }
    state.manifest.screenshots.extend(captured.screenshots);
    state.manifest.ocr_documents.extend(captured.ocr_documents);
    if let Some(visible_text) = captured.visible_text_fused {
        state.manifest.visible_text_fused = Some(visible_text);
    }
    let payload_bytes = captured
        .artifacts
        .iter()
        .map(|artifact| artifact.descriptor.bytes)
        .sum::<u64>();
    state.manifest.diagnostics.payload_bytes = state
        .manifest
        .diagnostics
        .payload_bytes
        .saturating_add(payload_bytes);
    state.manifest.artifacts.extend(
        captured
            .artifacts
            .iter()
            .map(|artifact| artifact.descriptor.clone()),
    );
    state.payloads.extend(captured.artifacts);

    if let Some(status) = state.manifest.source_status.get_mut(&kind) {
        let final_state = if captured.empty {
            SourceState::Empty
        } else {
            SourceState::Succeeded
        };
        status.state = final_state;
        status.completed_at = Some(completed_at);
        status.duration_ms = Some(duration_ms);
        status.truncated_nodes = captured.truncated_nodes;
        status.truncated_chars = captured.truncated_chars;
        increment_source_counter(&mut state.manifest.diagnostics, kind, final_state);
    }
}

fn finish_error(
    state: &mut CaptureState,
    kind: SourceKind,
    completed_at: chrono::DateTime<Utc>,
    duration_ms: u64,
    error: SourceError,
) {
    let gate = matches!(
        error.state,
        SourceState::SkippedPolicy | SourceState::Denied
    )
    .then(|| (error.code.clone(), error.message.clone()));
    let final_state = error.state;
    if let Some(status) = state.manifest.source_status.get_mut(&kind) {
        status.state = final_state;
        status.completed_at = Some(completed_at);
        status.duration_ms = Some(duration_ms);
        status.reason_code = Some(error.code);
        status.message = Some(error.message);
        status.retryable = error.retryable;
    }
    increment_source_counter(&mut state.manifest.diagnostics, kind, final_state);
    if let Some((code, message)) = gate {
        if !state.manifest.privacy.applied_gates.contains(&code) {
            state.manifest.privacy.applied_gates.push(code);
        }
        state.manifest.privacy.policy_reason.get_or_insert(message);
    }
}

fn increment_source_counter(
    diagnostics: &mut crate::CaptureDiagnostics,
    kind: SourceKind,
    state: SourceState,
) {
    let key = format!("source.{kind:?}.{state:?}").to_ascii_lowercase();
    *diagnostics.counters.entry(key).or_default() += 1;
}

fn nonnegative_millis(duration: chrono::Duration) -> u64 {
    duration.num_milliseconds().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CaptureId, CaptureProfile, CaptureTrigger, EditorContext, PrivacyPolicy, SourceSelection,
        TargetContext, TriggerKind,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use uuid::Uuid;

    struct FakeSource {
        kind: SourceKind,
        delay: Duration,
        calls: AtomicUsize,
    }

    struct PayloadSource;

    #[async_trait]
    impl ContextSource for PayloadSource {
        fn kind(&self) -> SourceKind {
            SourceKind::Target
        }

        async fn capture(
            &self,
            _request: &CaptureRequest,
            _input: SourceInput,
        ) -> Result<SourceCapture, SourceError> {
            let bytes = bytes::Bytes::from_static(b"12345");
            let artifact_id = uuid::Uuid::new_v4();
            Ok(SourceCapture {
                artifacts: vec![crate::CapturedArtifact {
                    descriptor: crate::ArtifactRef {
                        artifact_id,
                        source: SourceKind::Target,
                        kind: "payload_fixture".to_owned(),
                        content_hash: blake3::hash(&bytes).to_hex().to_string(),
                        media_type: "application/octet-stream".to_owned(),
                        bytes: bytes.len() as u64,
                        metadata: serde_json::Value::Null,
                    },
                    payload: crate::ArtifactPayload::Bytes {
                        media_type: "application/octet-stream".to_owned(),
                        bytes,
                    },
                }],
                ..SourceCapture::default()
            })
        }
    }

    #[async_trait]
    impl ContextSource for FakeSource {
        fn kind(&self) -> SourceKind {
            self.kind
        }

        fn dependencies(&self) -> &'static [SourceKind] {
            match self.kind {
                SourceKind::EditorAx => &[SourceKind::Target],
                _ => &[],
            }
        }

        async fn capture(
            &self,
            _request: &CaptureRequest,
            _input: SourceInput,
        ) -> Result<SourceCapture, SourceError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(self.delay).await;
            let mut result = SourceCapture::default();
            match self.kind {
                SourceKind::Target => {
                    result.target = Some(TargetContext {
                        app_name: Some("Fixture App".to_owned()),
                        bundle_id: Some("org.lumen.fixture".to_owned()),
                        ..TargetContext::default()
                    });
                }
                SourceKind::EditorAx => {
                    result.editor = Some(EditorContext {
                        role: Some("AXTextArea".to_owned()),
                        ..EditorContext::default()
                    });
                }
                _ => result.empty = true,
            }
            Ok(result)
        }
    }

    fn request(sources: SourceSelection) -> CaptureRequest {
        let now = Utc::now();
        CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 7,
            profile: CaptureProfile::FullLocal,
            sources,
            trigger: CaptureTrigger {
                kind: TriggerKind::Test,
                pressed_at: now,
                released_at: None,
            },
            requested_at: now,
            target_hint: None,
            privacy_policy: PrivacyPolicy::default(),
        }
    }

    #[tokio::test]
    async fn snapshot_is_partial_then_advances_revision() {
        let fast: Arc<dyn ContextSource> = Arc::new(FakeSource {
            kind: SourceKind::Target,
            delay: Duration::from_millis(1),
            calls: AtomicUsize::new(0),
        });
        let slow: Arc<dyn ContextSource> = Arc::new(FakeSource {
            kind: SourceKind::EditorAx,
            delay: Duration::from_millis(40),
            calls: AtomicUsize::new(0),
        });
        let collector =
            ContextCollector::with_sources(ContextConfig::default(), vec![fast, slow]).unwrap();
        let session = collector
            .begin(request(SourceSelection::from_sources([
                SourceKind::Target,
                SourceKind::EditorAx,
            ])))
            .unwrap();

        let partial = session
            .snapshot(Instant::now() + Duration::from_millis(10))
            .await;
        assert!(partial.manifest.target.is_some());
        assert!(partial.manifest.editor.is_none());
        assert!(!partial.manifest.all_requested_sources_terminal());
        let partial_revision = partial.manifest.revision;

        let complete = session
            .snapshot(Instant::now() + Duration::from_millis(100))
            .await;
        assert!(complete.manifest.target.is_some());
        assert!(complete.manifest.editor.is_some());
        assert!(complete.manifest.all_requested_sources_terminal());
        assert!(complete.manifest.revision > partial_revision);
    }

    #[tokio::test]
    async fn every_frozen_snapshot_gets_an_append_only_revision() {
        let source: Arc<dyn ContextSource> = Arc::new(FakeSource {
            kind: SourceKind::Target,
            delay: Duration::from_millis(1),
            calls: AtomicUsize::new(0),
        });
        let collector =
            ContextCollector::with_sources(ContextConfig::default(), vec![source]).unwrap();
        let session = collector
            .begin(request(SourceSelection::from_sources([SourceKind::Target])))
            .unwrap();

        let first = session
            .snapshot(Instant::now() + Duration::from_millis(100))
            .await;
        let second = session.snapshot(Instant::now()).await;

        assert!(first.manifest.all_requested_sources_terminal());
        assert_eq!(second.manifest.revision, first.manifest.revision + 1);
        assert!(second.manifest.frozen_at >= first.manifest.frozen_at);
    }

    #[tokio::test]
    async fn unconfigured_requested_source_is_explicit() {
        let collector = ContextCollector::new(ContextConfig::default(), None).unwrap();
        let session = collector
            .begin(request(SourceSelection::from_sources([
                SourceKind::Browser,
            ])))
            .unwrap();
        let snapshot = session.snapshot(Instant::now()).await;
        let status = snapshot
            .manifest
            .source_status
            .get(&SourceKind::Browser)
            .unwrap();
        assert_eq!(status.state, SourceState::Unavailable);
        assert_eq!(status.reason_code.as_deref(), Some("source_not_configured"));
        assert!(snapshot.manifest.all_requested_sources_terminal());
    }

    #[tokio::test]
    async fn denied_bundle_stops_content_sources_before_they_run() {
        let target = Arc::new(FakeSource {
            kind: SourceKind::Target,
            delay: Duration::from_millis(1),
            calls: AtomicUsize::new(0),
        });
        let editor = Arc::new(FakeSource {
            kind: SourceKind::EditorAx,
            delay: Duration::from_millis(1),
            calls: AtomicUsize::new(0),
        });
        let collector = ContextCollector::with_sources(
            ContextConfig::default(),
            vec![target.clone(), editor.clone()],
        )
        .unwrap();
        let mut capture_request = request(SourceSelection::from_sources([
            SourceKind::Target,
            SourceKind::EditorAx,
        ]));
        capture_request
            .privacy_policy
            .denied_bundle_ids
            .insert("org.lumen.fixture".to_owned());
        let session = collector.begin(capture_request).unwrap();

        let snapshot = session
            .snapshot(Instant::now() + Duration::from_millis(100))
            .await;

        assert!(snapshot.manifest.target.is_some());
        assert!(snapshot.manifest.editor.is_none());
        assert_eq!(editor.calls.load(Ordering::Relaxed), 0);
        let status = &snapshot.manifest.source_status[&SourceKind::EditorAx];
        assert_eq!(status.state, SourceState::SkippedPolicy);
        assert_eq!(status.reason_code.as_deref(), Some("bundle_denied"));
        assert!(snapshot.payloads.is_empty());
    }

    #[tokio::test]
    async fn payload_budget_failure_discards_the_source_atomically() {
        let config = ContextConfig {
            max_payload_bytes_per_capture: 4,
            ..ContextConfig::default()
        };
        let collector =
            ContextCollector::with_sources(config, vec![Arc::new(PayloadSource)]).unwrap();
        let session = collector
            .begin(request(SourceSelection::from_sources([SourceKind::Target])))
            .unwrap();

        let snapshot = session
            .snapshot(Instant::now() + Duration::from_millis(100))
            .await;
        let status = &snapshot.manifest.source_status[&SourceKind::Target];
        assert_eq!(status.state, SourceState::Failed);
        assert_eq!(
            status.reason_code.as_deref(),
            Some("capture_payload_budget_exceeded")
        );
        assert!(snapshot.manifest.artifacts.is_empty());
        assert!(snapshot.payloads.is_empty());
    }
}
