use std::path::Path;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};
use lumen_platform::{DisplayEnumerator, DisplayId, ScreenCapturer};
#[cfg(target_os = "macos")]
use lumen_platform_macos::{ax_tree::walk_window, MacDisplays, MacScreenCapturer};
#[cfg(target_os = "macos")]
use lumen_platform::AxTreeWalkConfig;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::protocol::{
    AxSnapshotMeta, Command, EncodedFrameMeta, RawFrameMeta, RequestEnvelope, ResponseEnvelope,
    ResponseResult, MAX_HEADER_BYTES, MAX_PAYLOAD_BYTES, PROTOCOL_VERSION,
};
use crate::CuaStatus;

static LIVE_CAPTURE_READY: AtomicBool = AtomicBool::new(false);

pub async fn serve(socket_path: &Path, token_file: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        let token = std::fs::read_to_string(token_file)
            .with_context(|| format!("read token {}", token_file.display()))?;
        let token = token.trim().to_owned();
        if token.len() < 32 {
            bail!("invalid Lumen Cua token");
        }
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if socket_path.exists() {
            if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
                bail!("Lumen Cua is already serving {}", socket_path.display());
            }
            std::fs::remove_file(socket_path)
                .with_context(|| format!("remove stale socket {}", socket_path.display()))?;
        }
        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("bind {}", socket_path.display()))?;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
        tracing::info!(socket = %socket_path.display(), "Lumen Cua ready");

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let token = token.clone();
                    let shutdown_tx = shutdown_tx.clone();
                    tokio::spawn(async move {
                        match handle_connection(stream, &token).await {
                            Ok(true) => { let _ = shutdown_tx.send(()).await; }
                            Ok(false) => {}
                            Err(error) => tracing::warn!(%error, "Lumen Cua request failed"),
                        }
                    });
                }
                _ = shutdown_rx.recv() => break,
            }
        }
        let _ = std::fs::remove_file(socket_path);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (socket_path, token_file);
        bail!("Lumen Cua requires a Unix socket")
    }
}

#[cfg(unix)]
async fn handle_connection(stream: tokio::net::UnixStream, token: &str) -> Result<bool> {
    #[cfg(target_os = "macos")]
    crate::peer_auth::authorize_peer(&stream)?;
    let (read_half, mut write_half) = stream.into_split();
    let reader = BufReader::new(read_half);
    let mut header = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reader
            .take((MAX_HEADER_BYTES + 1) as u64)
            .read_until(b'\n', &mut header),
    )
    .await
    .context("request header timed out")??;
    if header.last() != Some(&b'\n') || header.len() > MAX_HEADER_BYTES {
        bail!("invalid request header");
    }
    let request: RequestEnvelope = serde_json::from_slice(&header)?;
    let request_id = request.request_id.clone();
    let is_shutdown = matches!(request.command, Command::Shutdown);

    let response = if request.protocol_version != PROTOCOL_VERSION {
        (
            ResponseEnvelope::failure(request_id, "unsupported protocol version"),
            Vec::new(),
        )
    } else if request.token.as_bytes() != token.as_bytes() {
        (
            ResponseEnvelope::failure(request_id, "unauthorized"),
            Vec::new(),
        )
    } else {
        tracing::info!(command = ?request.command, "cua executing command");
        let exec_result = execute(request.command).await;
        match &exec_result {
            Ok((result, payload)) => tracing::info!(result = ?result, payload_len = payload.len(), "cua execute ok"),
            Err(e) => tracing::warn!(error = %e, "cua execute failed"),
        }
        match exec_result {
            Ok((result, payload)) if payload.len() <= MAX_PAYLOAD_BYTES => (
                ResponseEnvelope::success(request_id, result, payload.len()),
                payload,
            ),
            Ok(_) => (
                ResponseEnvelope::failure(request_id, "frame is too large"),
                Vec::new(),
            ),
            Err(error) => (
                ResponseEnvelope::failure(request_id, error.to_string()),
                Vec::new(),
            ),
        }
    };

    let mut response_header = serde_json::to_vec(&response.0)?;
    response_header.push(b'\n');
    write_half.write_all(&response_header).await?;
    write_half.write_all(&response.1).await?;
    write_half.shutdown().await?;
    Ok(is_shutdown && response.0.ok)
}

async fn execute(command: Command) -> Result<(ResponseResult, Vec<u8>)> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = command;
        bail!("Lumen Cua capture service requires macOS")
    }

    #[cfg(target_os = "macos")]
    match command {
        Command::Status => Ok((ResponseResult::Status { status: status() }, Vec::new())),
        Command::ListDisplays => {
            let displays = MacDisplays.list_displays().await?;
            Ok((ResponseResult::Displays { displays }, Vec::new()))
        }
        Command::CaptureEncoded {
            display_id,
            max_edge,
            jpeg,
            jpeg_quality,
        } => {
            let frame = MacScreenCapturer
                .capture_display(DisplayId(display_id), max_edge, jpeg, jpeg_quality)
                .await?;
            LIVE_CAPTURE_READY.store(true, Ordering::Relaxed);
            let meta = EncodedFrameMeta {
                media_type: frame.media_type,
                width: frame.width,
                height: frame.height,
                display_id: frame.display_id.0,
            };
            Ok((
                ResponseResult::EncodedFrame { frame: meta },
                frame.png_or_jpeg_bytes,
            ))
        }
        Command::CaptureRaw {
            display_id,
            scale_div,
        } => {
            let frame = MacScreenCapturer
                .capture_display_raw(DisplayId(display_id), scale_div.max(1))
                .await?;
            LIVE_CAPTURE_READY.store(true, Ordering::Relaxed);
            let meta = RawFrameMeta {
                width: frame.width,
                height: frame.height,
                bytes_per_row: frame.bytes_per_row,
                display_id: frame.display_id.0,
            };
            Ok((ResponseResult::RawFrame { frame: meta }, frame.bgra))
        }
        Command::Shutdown => Ok((ResponseResult::Ack, Vec::new())),
        Command::AxWalk {
            pid,
            window_id,
            max_depth,
            max_nodes,
            walk_timeout_ms,
            element_timeout_ms,
            max_text_length,
        } => {
            let config = AxTreeWalkConfig {
                max_depth,
                max_nodes,
                walk_timeout_ms,
                element_timeout_ms,
                max_text_length,
            };
            tracing::info!(pid, window_id, max_depth, max_nodes, "AxWalk starting");

            // AX API calls can block for seconds on some apps. Using
            // tokio::spawn_blocking would consume a tokio blocking-pool thread
            // that can't be cancelled on timeout — starving the pool and
            // freezing cua's other work (screen capture, status).
            //
            // Instead, dispatch to a dedicated OS thread via a channel. The
            // timeout cancels the *receiver*, not the thread — the thread
            // continues to completion in the background and self-terminates.
            // This is safe because:
            //   - AX walks are read-only (no mutation, no lock held)
            //   - At most one walk per screenshot; overlap is fine
            //   - The thread exits when walk_focused_window returns
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::Builder::new()
                .name("ax-walk".into())
                .spawn(move || {
                    let result = walk_window(pid, window_id, &config);
                    let _ = tx.send(result);
                })
                .map_err(|e| anyhow::anyhow!("spawn ax-walk thread: {e}"))?;

            let walk_timeout = Duration::from_millis(walk_timeout_ms.max(500) as u64 * 3);
            let snapshot = match tokio::time::timeout(walk_timeout, rx).await {
                Ok(Ok(Ok(snap))) => {
                    tracing::info!(
                        pid,
                        node_count = snap.node_count,
                        walk_ms = snap.walk_duration_ms,
                        text_len = snap.text_content.len(),
                        window = ?snap.window_title,
                        "AxWalk done"
                    );
                    snap
                }
                Ok(Ok(Err(lumen_platform::PlatformError::WindowGone(id)))) => {
                    tracing::info!(pid, window_id = id, "AxWalk window gone");
                    return Ok((ResponseResult::AxWindowGone { window_id: id }, Vec::new()));
                }
                Ok(Ok(Err(e))) => {
                    tracing::warn!(pid, error = %e, "AxWalk returned error");
                    bail!("AX walk error: {e}");
                }
                Ok(Err(_)) => {
                    tracing::warn!(pid, "AxWalk channel dropped");
                    bail!("AX walk channel closed unexpectedly");
                }
                Err(_) => {
                    tracing::warn!(pid, timeout_ms = walk_timeout.as_millis() as u64, "AxWalk TIMED OUT (thread continues in background)");
                    bail!("AX walk timed out after {}ms", walk_timeout.as_millis());
                }
            };
            let text_bytes = snapshot.text_content.into_bytes();
            let meta = AxSnapshotMeta {
                node_count: snapshot.node_count,
                content_hash: snapshot.content_hash,
                walk_duration_ms: snapshot.walk_duration_ms,
                truncated: snapshot.truncated,
                app_name: snapshot.app_name,
                window_title: snapshot.window_title,
                document_path: snapshot.document_path,
                browser_url: snapshot.browser_url,
            };
            Ok((ResponseResult::AxSnapshot { meta }, text_bytes))
        }
    }
}

fn status() -> CuaStatus {
    status_with_capture_observation(
        crate::permissions::read_only_status(),
        LIVE_CAPTURE_READY.load(Ordering::Relaxed),
    )
}

fn status_with_capture_observation(mut status: CuaStatus, capture_ready: bool) -> CuaStatus {
    if capture_ready && status.screen_recording == lumen_platform::PermissionState::Granted {
        status.screen_recording_capturable = Some(true);
        status.direct_capture_status = crate::DirectCaptureStatus::Ready;
        status.direct_capture_error = None;
    }
    status
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{ensure_token_file, CuaClient, CuaPaths};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_and_server_round_trip_status_and_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        ensure_token_file(&paths.token_file).unwrap();
        let socket = paths.socket.clone();
        let token_file = paths.token_file.clone();
        let server = tokio::spawn(async move { serve(&socket, &token_file).await });

        for _ in 0..50 {
            if paths.socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(paths.socket.exists());

        let client = CuaClient::new(&paths.socket, &paths.token_file);
        let status_client = client.clone();
        tokio::task::spawn_blocking(move || status_client.status())
            .await
            .unwrap()
            .unwrap();
        tokio::task::spawn_blocking(move || client.shutdown())
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap().unwrap();
        assert!(!paths.socket.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn second_server_does_not_replace_a_live_socket() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        ensure_token_file(&paths.token_file).unwrap();
        let socket = paths.socket.clone();
        let token_file = paths.token_file.clone();
        let first = tokio::spawn(async move { serve(&socket, &token_file).await });
        for _ in 0..50 {
            if paths.socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let second = serve(&paths.socket, &paths.token_file).await.unwrap_err();
        assert!(second.to_string().contains("already serving"));

        let client = CuaClient::new(&paths.socket, &paths.token_file);
        tokio::task::spawn_blocking(move || client.shutdown())
            .await
            .unwrap()
            .unwrap();
        first.await.unwrap().unwrap();
    }

    #[test]
    fn successful_frame_capture_promotes_read_only_status_to_ready() {
        let status = CuaStatus {
            screen_recording: lumen_platform::PermissionState::Granted,
            screen_recording_capturable: None,
            direct_capture_status: crate::DirectCaptureStatus::NotChecked,
            direct_capture_error: None,
        };

        let status = status_with_capture_observation(status, true);

        assert_eq!(status.screen_recording_capturable, Some(true));
        assert_eq!(
            status.direct_capture_status,
            crate::DirectCaptureStatus::Ready
        );
    }
}
