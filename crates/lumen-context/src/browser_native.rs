use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    BrowserCaptureError, BrowserCaptureRequest, BrowserSnapshot, BrowserSnapshotProvider,
    BROWSER_CONTEXT_SCHEMA_VERSION,
};

const NATIVE_BRIDGE_PROTOCOL_VERSION: u32 = 1;
const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeBrowserBridgeConfig {
    pub owner: String,
    pub socket_path: PathBuf,
    pub token_path: PathBuf,
    pub allowed_origins: BTreeSet<String>,
    pub max_message_bytes: usize,
}

impl NativeBrowserBridgeConfig {
    pub fn new(
        owner: impl Into<String>,
        socket_path: PathBuf,
        token_path: PathBuf,
        allowed_origins: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            socket_path,
            token_path,
            allowed_origins: allowed_origins.into_iter().collect(),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }

    pub fn validate(&self) -> Result<(), BrowserCaptureError> {
        if self.owner.trim().is_empty() {
            return Err(BrowserCaptureError::Failed(
                "native browser bridge owner must not be empty".to_owned(),
            ));
        }
        if self.allowed_origins.is_empty() {
            return Err(BrowserCaptureError::Failed(
                "native browser bridge requires an extension origin allowlist".to_owned(),
            ));
        }
        if self.max_message_bytes == 0 || self.max_message_bytes > 64 * 1024 * 1024 {
            return Err(BrowserCaptureError::Failed(
                "native browser bridge message limit is invalid".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn write_host_config(&self, path: &Path) -> Result<(), BrowserCaptureError> {
        self.validate()?;
        ensure_parent(path)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| {
            BrowserCaptureError::Failed(format!("encode native browser host config: {error}"))
        })?;
        let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| {
                BrowserCaptureError::Failed(format!("create native browser host config: {error}"))
            })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                BrowserCaptureError::Failed(format!("write native browser host config: {error}"))
            })?;
        std::fs::rename(&temporary, path).map_err(|error| {
            BrowserCaptureError::Failed(format!("install native browser host config: {error}"))
        })?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
            BrowserCaptureError::Failed(format!("secure native browser host config: {error}"))
        })
    }
}

#[derive(Clone)]
pub struct NativeBrowserProvider {
    shared: Arc<NativeBrowserShared>,
}

struct NativeBrowserShared {
    config: NativeBrowserBridgeConfig,
    token: String,
    connection: tokio::sync::Mutex<Option<ActiveConnection>>,
    pending: Mutex<HashMap<Uuid, oneshot::Sender<Result<BrowserSnapshot, BrowserCaptureError>>>>,
}

#[derive(Clone)]
struct ActiveConnection {
    id: Uuid,
    writer: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HostHello {
    kind: String,
    protocol_version: u32,
    owner: String,
    origin: String,
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HostAck {
    kind: String,
    ok: bool,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct NativeCaptureCommand<'a> {
    kind: &'static str,
    schema_version: u32,
    #[serde(flatten)]
    request: &'a BrowserCaptureRequest,
}

#[derive(Debug, Deserialize)]
struct NativeCaptureResult {
    kind: String,
    request_id: Option<Uuid>,
    ok: bool,
    snapshot: Option<BrowserSnapshot>,
    error: Option<NativeCaptureFailure>,
}

#[derive(Debug, Deserialize)]
struct NativeCaptureFailure {
    code: String,
    message: String,
    retryable: bool,
}

impl NativeBrowserProvider {
    pub async fn bind(config: NativeBrowserBridgeConfig) -> Result<Self, BrowserCaptureError> {
        config.validate()?;
        ensure_parent(&config.socket_path)?;
        ensure_parent(&config.token_path)?;
        let token = load_or_create_token(&config.token_path)?;
        if config.socket_path.exists() {
            let metadata = std::fs::symlink_metadata(&config.socket_path).map_err(|error| {
                BrowserCaptureError::Failed(format!("inspect browser socket: {error}"))
            })?;
            if !std::os::unix::fs::FileTypeExt::is_socket(&metadata.file_type()) {
                return Err(BrowserCaptureError::Failed(
                    "browser socket path exists and is not a socket".to_owned(),
                ));
            }
            std::fs::remove_file(&config.socket_path).map_err(|error| {
                BrowserCaptureError::Failed(format!("remove stale browser socket: {error}"))
            })?;
        }
        let listener = UnixListener::bind(&config.socket_path).map_err(|error| {
            BrowserCaptureError::Failed(format!("bind native browser socket: {error}"))
        })?;
        std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                BrowserCaptureError::Failed(format!("secure native browser socket: {error}"))
            })?;
        let shared = Arc::new(NativeBrowserShared {
            config,
            token,
            connection: tokio::sync::Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
        });
        let accept_shared = shared.clone();
        tokio::spawn(async move {
            accept_connections(listener, accept_shared).await;
        });
        Ok(Self { shared })
    }

    pub async fn is_connected(&self) -> bool {
        self.shared.connection.lock().await.is_some()
    }
}

#[async_trait]
impl BrowserSnapshotProvider for NativeBrowserProvider {
    async fn capture(
        &self,
        request: BrowserCaptureRequest,
    ) -> Result<BrowserSnapshot, BrowserCaptureError> {
        let connection = self.shared.connection.lock().await.clone().ok_or_else(|| {
            BrowserCaptureError::Unavailable(
                "browser extension native host is not connected".to_owned(),
            )
        })?;
        let command = serde_json::to_vec(&NativeCaptureCommand {
            kind: "capture",
            schema_version: BROWSER_CONTEXT_SCHEMA_VERSION,
            request: &request,
        })
        .map_err(|error| BrowserCaptureError::Failed(format!("encode browser command: {error}")))?;
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(request.request_id, sender);
        if let Err(error) = write_frame(
            &mut *connection.writer.lock().await,
            &command,
            self.shared.config.max_message_bytes,
        )
        .await
        {
            self.shared
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&request.request_id);
            return Err(error);
        }
        let remaining = (request.deadline - chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        if remaining.is_zero() {
            self.shared
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&request.request_id);
            return Err(BrowserCaptureError::Timeout(
                "browser request deadline elapsed".to_owned(),
            ));
        }
        match tokio::time::timeout(remaining, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(BrowserCaptureError::Unavailable(
                "browser extension connection closed".to_owned(),
            )),
            Err(_) => {
                self.shared
                    .pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&request.request_id);
                Err(BrowserCaptureError::Timeout(
                    "browser extension response deadline elapsed".to_owned(),
                ))
            }
        }
    }
}

async fn accept_connections(listener: UnixListener, shared: Arc<NativeBrowserShared>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let connection_shared = shared.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, connection_shared).await;
                });
            }
            Err(error) => {
                tracing::warn!(error = %error, "native browser listener failed");
                break;
            }
        }
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    shared: Arc<NativeBrowserShared>,
) -> Result<(), BrowserCaptureError> {
    let hello: HostHello =
        serde_json::from_slice(&read_frame(&mut stream, shared.config.max_message_bytes).await?)
            .map_err(|error| {
                BrowserCaptureError::Denied(format!("invalid browser host hello: {error}"))
            })?;
    let authenticated = hello.kind == "hello"
        && hello.protocol_version == NATIVE_BRIDGE_PROTOCOL_VERSION
        && hello.owner == shared.config.owner
        && shared.config.allowed_origins.contains(&hello.origin)
        && constant_time_eq(hello.token.as_bytes(), shared.token.as_bytes());
    let ack = HostAck {
        kind: "hello_ack".to_owned(),
        ok: authenticated,
        message: (!authenticated).then(|| "native browser host authentication failed".to_owned()),
    };
    write_frame(
        &mut stream,
        &serde_json::to_vec(&ack).map_err(|error| {
            BrowserCaptureError::Failed(format!("encode browser host ack: {error}"))
        })?,
        shared.config.max_message_bytes,
    )
    .await?;
    if !authenticated {
        return Err(BrowserCaptureError::Denied(
            "native browser host authentication failed".to_owned(),
        ));
    }

    let connection_id = Uuid::new_v4();
    let (reader, writer) = stream.into_split();
    *shared.connection.lock().await = Some(ActiveConnection {
        id: connection_id,
        writer: Arc::new(tokio::sync::Mutex::new(writer)),
    });
    read_results(reader, shared.clone()).await;
    let mut active = shared.connection.lock().await;
    if active
        .as_ref()
        .is_some_and(|connection| connection.id == connection_id)
    {
        *active = None;
        drop(active);
        fail_all_pending(&shared, "browser extension disconnected");
    }
    Ok(())
}

async fn read_results(mut reader: OwnedReadHalf, shared: Arc<NativeBrowserShared>) {
    loop {
        let bytes = match read_frame(&mut reader, shared.config.max_message_bytes).await {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let result: NativeCaptureResult = match serde_json::from_slice(&bytes) {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(error = %error, "invalid native browser result");
                continue;
            }
        };
        if result.kind != "capture_result" {
            continue;
        }
        let Some(request_id) = result.request_id else {
            continue;
        };
        let sender = shared
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request_id);
        if let Some(sender) = sender {
            let response = if result.ok {
                result.snapshot.ok_or_else(|| {
                    BrowserCaptureError::Failed("browser extension returned no snapshot".to_owned())
                })
            } else {
                Err(map_native_failure(result.error))
            };
            let _ = sender.send(response);
        }
    }
}

fn map_native_failure(error: Option<NativeCaptureFailure>) -> BrowserCaptureError {
    let Some(error) = error else {
        return BrowserCaptureError::Failed("browser extension returned no error".to_owned());
    };
    match error.code.as_str() {
        "origin_permission_required"
        | "private_browsing_denied"
        | "domain_denied"
        | "bundle_denied" => BrowserCaptureError::Denied(error.message),
        "deadline_elapsed" => BrowserCaptureError::Timeout(error.message),
        "target_stale" => BrowserCaptureError::Stale(error.message),
        "active_tab_unavailable" | "restricted_page" | "main_frame_unavailable" => {
            BrowserCaptureError::Unavailable(error.message)
        }
        _ if error.retryable => BrowserCaptureError::Unavailable(error.message),
        _ => BrowserCaptureError::Failed(error.message),
    }
}

fn fail_all_pending(shared: &NativeBrowserShared, message: &str) {
    let pending = std::mem::take(
        &mut *shared
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    for (_, sender) in pending {
        let _ = sender.send(Err(BrowserCaptureError::Unavailable(message.to_owned())));
    }
}

fn ensure_parent(path: &Path) -> Result<(), BrowserCaptureError> {
    let parent = path.parent().ok_or_else(|| {
        BrowserCaptureError::Failed("native browser path has no parent".to_owned())
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        BrowserCaptureError::Failed(format!("create native browser directory: {error}"))
    })
}

fn load_or_create_token(path: &Path) -> Result<String, BrowserCaptureError> {
    if path.exists() {
        return std::fs::read_to_string(path)
            .map(|token| token.trim().to_owned())
            .map_err(|error| BrowserCaptureError::Failed(format!("read browser token: {error}")));
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| BrowserCaptureError::Failed(format!("create browser token: {error}")))?;
    file.write_all(token.as_bytes())
        .map_err(|error| BrowserCaptureError::Failed(format!("write browser token: {error}")))?;
    file.sync_all()
        .map_err(|error| BrowserCaptureError::Failed(format!("sync browser token: {error}")))?;
    Ok(token)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> Result<Vec<u8>, BrowserCaptureError> {
    let length =
        reader.read_u32().await.map_err(|error| {
            BrowserCaptureError::Unavailable(format!("read browser frame: {error}"))
        })? as usize;
    if length == 0 || length > limit {
        return Err(BrowserCaptureError::Failed(
            "native browser frame length is invalid".to_owned(),
        ));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes).await.map_err(|error| {
        BrowserCaptureError::Unavailable(format!("read browser frame body: {error}"))
    })?;
    Ok(bytes)
}

async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
    limit: usize,
) -> Result<(), BrowserCaptureError> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(BrowserCaptureError::Failed(
            "native browser output frame length is invalid".to_owned(),
        ));
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|error| {
            BrowserCaptureError::Unavailable(format!("write browser frame: {error}"))
        })?;
    writer.write_all(bytes).await.map_err(|error| {
        BrowserCaptureError::Unavailable(format!("write browser frame body: {error}"))
    })?;
    writer
        .flush()
        .await
        .map_err(|error| BrowserCaptureError::Unavailable(format!("flush browser frame: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserContext, CaptureId};
    use chrono::Utc;

    fn request() -> BrowserCaptureRequest {
        let now = Utc::now();
        BrowserCaptureRequest {
            request_id: Uuid::new_v4(),
            capture_id: CaptureId::new(),
            target_generation: 3,
            target_hint: None,
            requested_at: now,
            deadline: now + chrono::Duration::seconds(2),
            max_chars: 1_000,
            max_nodes: 10,
            allow_private_browsing: false,
            denied_bundle_ids: BTreeSet::new(),
            denied_domains: BTreeSet::new(),
        }
    }

    async fn authenticate(
        config: &NativeBrowserBridgeConfig,
        token: &str,
    ) -> (UnixStream, HostAck) {
        let mut stream = UnixStream::connect(&config.socket_path).await.unwrap();
        let hello = HostHello {
            kind: "hello".to_owned(),
            protocol_version: NATIVE_BRIDGE_PROTOCOL_VERSION,
            owner: config.owner.clone(),
            origin: config.allowed_origins.iter().next().unwrap().clone(),
            token: token.to_owned(),
        };
        write_frame(
            &mut stream,
            &serde_json::to_vec(&hello).unwrap(),
            config.max_message_bytes,
        )
        .await
        .unwrap();
        let ack = serde_json::from_slice(
            &read_frame(&mut stream, config.max_message_bytes)
                .await
                .unwrap(),
        )
        .unwrap();
        (stream, ack)
    }

    #[tokio::test]
    async fn authenticated_bridge_round_trips_a_correlated_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let config = NativeBrowserBridgeConfig::new(
            "fixture-owner",
            directory.path().join("bridge.sock"),
            directory.path().join("bridge.token"),
            ["chrome-extension://fixture-id/".to_owned()],
        );
        let provider = NativeBrowserProvider::bind(config.clone()).await.unwrap();
        let token = std::fs::read_to_string(&config.token_path).unwrap();
        let (mut host, ack) = authenticate(&config, token.trim()).await;
        assert!(ack.ok);

        let host_task = tokio::spawn(async move {
            let command: serde_json::Value = serde_json::from_slice(
                &read_frame(&mut host, DEFAULT_MAX_MESSAGE_BYTES)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(command["kind"], "capture");
            let request_id: Uuid = serde_json::from_value(command["request_id"].clone()).unwrap();
            let capture_id: crate::CaptureId =
                serde_json::from_value(command["capture_id"].clone()).unwrap();
            let generation = command["target_generation"].as_u64().unwrap();
            let now = Utc::now();
            let snapshot = BrowserSnapshot {
                schema_version: BROWSER_CONTEXT_SCHEMA_VERSION,
                request_id,
                capture_id,
                target_generation: generation,
                started_tab_id: Some(9),
                completed_tab_id: Some(9),
                started_navigation_id: Some("nav".to_owned()),
                completed_navigation_id: Some("nav".to_owned()),
                started_document_id: Some("doc".to_owned()),
                completed_document_id: Some("doc".to_owned()),
                context: BrowserContext {
                    tab_id: Some(9),
                    navigation_id: Some("nav".to_owned()),
                    document_id: Some("doc".to_owned()),
                    ..BrowserContext::default()
                },
                frame_status: Vec::new(),
                captured_at: now,
                extension_version: Some("test".to_owned()),
            };
            let result = serde_json::json!({
                "kind": "capture_result",
                "request_id": request_id,
                "ok": true,
                "snapshot": snapshot
            });
            write_frame(
                &mut host,
                &serde_json::to_vec(&result).unwrap(),
                DEFAULT_MAX_MESSAGE_BYTES,
            )
            .await
            .unwrap();
        });

        let request = request();
        let request_id = request.request_id;
        let snapshot = provider.capture(request).await.unwrap();
        assert_eq!(snapshot.request_id, request_id);
        host_task.await.unwrap();
        let mode = std::fs::metadata(&config.token_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected_without_becoming_active() {
        let directory = tempfile::tempdir().unwrap();
        let config = NativeBrowserBridgeConfig::new(
            "fixture-owner",
            directory.path().join("bridge.sock"),
            directory.path().join("bridge.token"),
            ["chrome-extension://fixture-id/".to_owned()],
        );
        let provider = NativeBrowserProvider::bind(config.clone()).await.unwrap();
        let (_host, ack) = authenticate(&config, "wrong-token").await;

        assert!(!ack.ok);
        assert!(!provider.is_connected().await);
    }
}
