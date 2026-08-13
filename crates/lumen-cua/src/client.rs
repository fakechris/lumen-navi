use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lumen_platform::{DisplayId, DisplayInfo, RawFrame, ScreenshotFrame};
use thiserror::Error;
use uuid::Uuid;

use crate::protocol::{
    Command, RequestEnvelope, ResponseEnvelope, ResponseResult, MAX_HEADER_BYTES,
    MAX_PAYLOAD_BYTES, PROTOCOL_VERSION,
};
use crate::CuaStatus;

#[derive(Debug, Error)]
pub enum CuaError {
    #[error("Lumen Cua is unavailable: {0}")]
    Unavailable(String),
    #[error("Lumen Cua protocol error: {0}")]
    Protocol(String),
    #[error("Lumen Cua request failed: {0}")]
    Request(String),
    #[error("Lumen Cua is unsupported on this platform")]
    Unsupported,
    #[error("ax window {0} is gone")]
    WindowGone(u64),
}

#[derive(Debug, Clone)]
pub struct CuaClient {
    socket: PathBuf,
    token_file: PathBuf,
    timeout: Duration,
}

impl CuaClient {
    pub fn new(socket: impl Into<PathBuf>, token_file: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            token_file: token_file.into(),
            timeout: Duration::from_secs(15),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub fn status(&self) -> Result<CuaStatus, CuaError> {
        let current = self.call_with_protocol(Command::Status, PROTOCOL_VERSION);
        match current {
            Ok((ResponseResult::Status { status }, _)) => Ok(status),
            Ok((other, _)) => Err(unexpected(other)),
            Err(CuaError::Protocol(_)) | Err(CuaError::Request(_)) if PROTOCOL_VERSION > 1 => {
                match self
                    .call_with_protocol(Command::Status, PROTOCOL_VERSION - 1)?
                    .0
                {
                    ResponseResult::Status { status } => Ok(status),
                    other => Err(unexpected(other)),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn list_displays(&self) -> Result<Vec<DisplayInfo>, CuaError> {
        match self.call(Command::ListDisplays)?.0 {
            ResponseResult::Displays { displays } => Ok(displays),
            other => Err(unexpected(other)),
        }
    }

    pub fn capture_encoded(
        &self,
        display_id: DisplayId,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    ) -> Result<ScreenshotFrame, CuaError> {
        let (result, payload) = self.call(Command::CaptureEncoded {
            display_id: display_id.0,
            max_edge,
            jpeg,
            jpeg_quality,
        })?;
        match result {
            ResponseResult::EncodedFrame { frame }
                if frame.width > 0
                    && frame.height > 0
                    && frame.display_id == display_id.0
                    && !payload.is_empty()
                    && matches!(frame.media_type.as_str(), "image/jpeg" | "image/png") =>
            {
                Ok(ScreenshotFrame {
                    png_or_jpeg_bytes: payload,
                    media_type: frame.media_type,
                    width: frame.width,
                    height: frame.height,
                    display_id: DisplayId(frame.display_id),
                })
            }
            ResponseResult::EncodedFrame { .. } => {
                Err(CuaError::Protocol("invalid encoded frame metadata".into()))
            }
            other => Err(unexpected(other)),
        }
    }

    pub fn capture_raw(&self, display_id: DisplayId, scale_div: u32) -> Result<RawFrame, CuaError> {
        let (result, payload) = self.call(Command::CaptureRaw {
            display_id: display_id.0,
            scale_div,
        })?;
        match result {
            ResponseResult::RawFrame { frame } => {
                let min_row = (frame.width as usize).checked_mul(4);
                let expected = frame.bytes_per_row.checked_mul(frame.height as usize);
                if frame.width == 0
                    || frame.height == 0
                    || frame.display_id != display_id.0
                    || min_row.is_none_or(|min| frame.bytes_per_row < min)
                    || expected != Some(payload.len())
                {
                    return Err(CuaError::Protocol("invalid raw frame metadata".into()));
                }
                Ok(RawFrame {
                    bgra: payload,
                    width: frame.width,
                    height: frame.height,
                    bytes_per_row: frame.bytes_per_row,
                    display_id: DisplayId(frame.display_id),
                })
            }
            other => Err(unexpected(other)),
        }
    }

    /// Walk the AX tree of the focused window of `pid`'s app. Returns the
    /// flattened text + metadata. Runs inside cua (Accessibility TCC holder).
    pub fn walk_ax_tree(
        &self,
        pid: i32,
        window_id: Option<u64>,
        config: &lumen_platform::AxTreeWalkConfig,
    ) -> Result<lumen_platform::AxTreeSnapshot, CuaError> {
        let (result, payload) = self.call(Command::AxWalk {
            pid,
            window_id,
            max_depth: config.max_depth,
            max_nodes: config.max_nodes,
            walk_timeout_ms: config.walk_timeout_ms,
            element_timeout_ms: config.element_timeout_ms,
            max_text_length: config.max_text_length,
        })?;
        match result {
            ResponseResult::AxWindowGone { window_id } => Err(CuaError::WindowGone(window_id)),
            ResponseResult::AxSnapshot { meta } => {
                let text_content = String::from_utf8_lossy(&payload).into_owned();
                Ok(lumen_platform::AxTreeSnapshot {
                    text_content,
                    node_count: meta.node_count,
                    content_hash: meta.content_hash,
                    walk_duration_ms: meta.walk_duration_ms,
                    truncated: meta.truncated,
                    app_name: meta.app_name,
                    window_title: meta.window_title,
                    document_path: meta.document_path,
                    browser_url: meta.browser_url,
                })
            }
            other => Err(unexpected(other)),
        }
    }

    pub fn shutdown(&self) -> Result<(), CuaError> {
        let current = self.call_with_protocol(Command::Shutdown, PROTOCOL_VERSION);
        match current {
            Ok((ResponseResult::Ack, _)) => Ok(()),
            Ok((other, _)) => Err(unexpected(other)),
            Err(CuaError::Protocol(_)) | Err(CuaError::Request(_)) if PROTOCOL_VERSION > 1 => {
                match self
                    .call_with_protocol(Command::Shutdown, PROTOCOL_VERSION - 1)?
                    .0
                {
                    ResponseResult::Ack => Ok(()),
                    other => Err(unexpected(other)),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn call(&self, command: Command) -> Result<(ResponseResult, Vec<u8>), CuaError> {
        self.call_with_protocol(command, PROTOCOL_VERSION)
    }

    fn call_with_protocol(
        &self,
        command: Command,
        protocol_version: u16,
    ) -> Result<(ResponseResult, Vec<u8>), CuaError> {
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;

            let token = std::fs::read_to_string(&self.token_file)
                .map_err(|e| CuaError::Unavailable(format!("read token: {e}")))?;
            let request_id = Uuid::new_v4().to_string();
            let request = RequestEnvelope {
                protocol_version,
                request_id: request_id.clone(),
                token: token.trim().to_owned(),
                command,
            };
            let mut encoded =
                serde_json::to_vec(&request).map_err(|e| CuaError::Protocol(e.to_string()))?;
            if encoded.len() > MAX_HEADER_BYTES {
                return Err(CuaError::Protocol("request header is too large".into()));
            }
            encoded.push(b'\n');

            let mut stream = UnixStream::connect(&self.socket)
                .map_err(|e| CuaError::Unavailable(e.to_string()))?;
            stream.set_read_timeout(Some(self.timeout)).ok();
            stream.set_write_timeout(Some(self.timeout)).ok();
            stream
                .write_all(&encoded)
                .map_err(|e| CuaError::Unavailable(e.to_string()))?;

            let mut reader = BufReader::new(stream);
            let mut header = Vec::new();
            reader
                .by_ref()
                .take((MAX_HEADER_BYTES + 1) as u64)
                .read_until(b'\n', &mut header)
                .map_err(|e| CuaError::Protocol(e.to_string()))?;
            if header.last() != Some(&b'\n') || header.len() > MAX_HEADER_BYTES {
                return Err(CuaError::Protocol("invalid response header".into()));
            }
            let response: ResponseEnvelope =
                serde_json::from_slice(&header).map_err(|e| CuaError::Protocol(e.to_string()))?;
            if response.protocol_version != protocol_version || response.request_id != request_id {
                return Err(CuaError::Protocol("response identity mismatch".into()));
            }
            if !response.ok {
                return Err(CuaError::Request(
                    response.error.unwrap_or_else(|| "unknown error".into()),
                ));
            }
            if response.payload_len > MAX_PAYLOAD_BYTES {
                return Err(CuaError::Protocol("response payload is too large".into()));
            }
            let mut payload = vec![0; response.payload_len];
            reader
                .read_exact(&mut payload)
                .map_err(|e| CuaError::Protocol(format!("read payload: {e}")))?;
            let result = response
                .result
                .ok_or_else(|| CuaError::Protocol("missing response result".into()))?;
            Ok((result, payload))
        }
        #[cfg(not(unix))]
        {
            let _ = (command, protocol_version);
            Err(CuaError::Unsupported)
        }
    }
}

fn unexpected(result: ResponseResult) -> CuaError {
    CuaError::Protocol(format!("unexpected response: {result:?}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::protocol::{EncodedFrameMeta, ResponseEnvelope};
    use crate::{ensure_token_file, CuaPaths};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;

    #[test]
    fn encoded_frame_payload_round_trips_without_base64() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        ensure_token_file(&paths.token_file).unwrap();
        std::fs::create_dir_all(paths.socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        let expected = vec![0xff, 0xd8, 0x00, 0x11, 0xff, 0xd9];
        let server_payload = expected.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: RequestEnvelope = serde_json::from_str(&request_line).unwrap();
            let response = ResponseEnvelope::success(
                request.request_id,
                ResponseResult::EncodedFrame {
                    frame: EncodedFrameMeta {
                        media_type: "image/jpeg".into(),
                        width: 2,
                        height: 1,
                        display_id: 7,
                    },
                },
                server_payload.len(),
            );
            let mut header = serde_json::to_vec(&response).unwrap();
            header.push(b'\n');
            stream.write_all(&header).unwrap();
            stream.write_all(&server_payload).unwrap();
        });

        let frame = CuaClient::new(&paths.socket, &paths.token_file)
            .capture_encoded(DisplayId(7), 1600, true, 80)
            .unwrap();
        assert_eq!(frame.png_or_jpeg_bytes, expected);
        assert_eq!(frame.media_type, "image/jpeg");
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.display_id, DisplayId(7));
        server.join().unwrap();
    }

    #[test]
    fn shutdown_retries_the_previous_protocol_for_upgrade_migration() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        ensure_token_file(&paths.token_file).unwrap();
        std::fs::create_dir_all(paths.socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut current_stream, _) = listener.accept().unwrap();
            let mut current_line = String::new();
            BufReader::new(current_stream.try_clone().unwrap())
                .read_line(&mut current_line)
                .unwrap();
            let current: RequestEnvelope = serde_json::from_str(&current_line).unwrap();
            assert_eq!(current.protocol_version, PROTOCOL_VERSION);
            let current_response = ResponseEnvelope {
                protocol_version: 1,
                request_id: current.request_id,
                ok: false,
                error: Some("unsupported protocol version".into()),
                result: None,
                payload_len: 0,
            };
            let mut header = serde_json::to_vec(&current_response).unwrap();
            header.push(b'\n');
            current_stream.write_all(&header).unwrap();

            let (mut legacy_stream, _) = listener.accept().unwrap();
            let mut legacy_line = String::new();
            BufReader::new(legacy_stream.try_clone().unwrap())
                .read_line(&mut legacy_line)
                .unwrap();
            let legacy: RequestEnvelope = serde_json::from_str(&legacy_line).unwrap();
            assert_eq!(legacy.protocol_version, PROTOCOL_VERSION - 1);
            let legacy_response = ResponseEnvelope {
                protocol_version: PROTOCOL_VERSION - 1,
                request_id: legacy.request_id,
                ok: true,
                error: None,
                result: Some(ResponseResult::Ack),
                payload_len: 0,
            };
            let mut header = serde_json::to_vec(&legacy_response).unwrap();
            header.push(b'\n');
            legacy_stream.write_all(&header).unwrap();
        });

        CuaClient::new(&paths.socket, &paths.token_file)
            .shutdown()
            .unwrap();
        server.join().unwrap();
    }
}
