//! Observe MCP over stdio. Talks to the running daemon on `daemon.sock`.
//!
//! Tools: status, settings, pause, resume, recent_context, search.
//! No wipe, no Act / computer-use, no screenshot bytes.

use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use lumen_api::{ControlRequest, ControlResponse};
use lumen_config::Config;
use serde_json::{json, Value};
use tracing::{error, info};

const PROTOCOL: &str = "2024-11-05";

pub async fn run() -> Result<()> {
    let socket = resolve_socket_path();
    info!(path = %socket.display(), "lumen-navi MCP (stdio) talking to daemon");
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line.context("read stdin")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "invalid MCP json");
                continue;
            }
        };
        if let Some(resp) = handle_message(&msg, &socket) {
            serde_json::to_writer(&mut stdout, &resp).context("write MCP response")?;
            stdout.write_all(b"\n").ok();
            stdout.flush().ok();
        }
    }
    Ok(())
}

fn resolve_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("LUMEN_NAVI_SOCKET") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let config_path = std::env::var("LUMEN_NAVI_CONFIG").unwrap_or_else(|_| "navi.toml".into());
    let mut config = Config::load_or_default(&config_path).unwrap_or_default();
    if std::env::var_os("LUMEN_NAVI_CONFIG").is_none()
        && !std::path::Path::new(&config_path).exists()
        && config.data_dir == std::path::Path::new("data")
    {
        config.data_dir = default_data_dir();
    }
    config.data_dir.join("daemon.sock")
}

fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join("Library/Application Support/LumenNavi")
    }
    #[cfg(target_os = "windows")]
    {
        match std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            Some(local) => PathBuf::from(local).join("LumenNavi"),
            None => std::env::temp_dir().join("LumenNavi"),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".lumen-navi")
    }
}

fn handle_message(msg: &Value, socket: &Path) -> Option<Value> {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();
    if id.is_none() {
        return None;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "lumen-navi",
                "version": env!("CARGO_PKG_VERSION"),
            },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_defs() })),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or(json!({}));
            call_tool(&params, socket)
        }
        other => Err(rpc_error(-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": err }),
    })
}

fn tool_defs() -> Vec<Value> {
    vec![
        tool(
            "navi_status",
            "Observe status: recording, pause, closed_eyes, persist counters, stored events.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "navi_get_settings",
            "Full Observe settings snapshot (pause, closed_eyes, app_blocklist, sources).",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "navi_pause",
            "Pause Observe. Does not quit the daemon or uninstall capture.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "navi_resume",
            "Resume Observe after navi_pause.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "navi_recent_context",
            "Last N 15-minute History cards (apps by duration, title, narrative).",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 48 }
                }
            }),
        ),
        tool(
            "navi_search",
            "Full-text search over OCR / transcript / AX derived text.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                },
                "required": ["query"]
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn call_tool(params: &Value, socket: &Path) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing tool name"))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let req = match name {
        "navi_status" => ControlRequest::Health,
        "navi_get_settings" => ControlRequest::GetSettings,
        "navi_pause" => ControlRequest::Pause { source: None },
        "navi_resume" => ControlRequest::Resume { source: None },
        "navi_recent_context" => ControlRequest::RecentContext {
            limit: args
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
        },
        "navi_search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| rpc_error(-32602, "navi_search requires query"))?
                .to_string();
            if query.trim().is_empty() {
                return Err(rpc_error(-32602, "navi_search query is empty"));
            }
            ControlRequest::SearchOcr {
                query,
                limit: args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize),
            }
        }
        other => return Err(rpc_error(-32601, format!("unknown tool: {other}"))),
    };
    match control(socket, &req) {
        Ok(resp) => Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&resp).unwrap_or_else(|_| "{}".into()),
            }],
            "isError": matches!(resp, ControlResponse::Error { .. }),
        })),
        Err(e) => Ok(json!({
            "content": [{ "type": "text", "text": format!("daemon unavailable: {e}") }],
            "isError": true,
        })),
    }
}

fn control(socket: &Path, req: &ControlRequest) -> Result<ControlResponse, String> {
    let body = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let mut stream = UnixStream::connect(socket).map_err(|e| {
        format!(
            "connect {}: {e} (is lumen-daemon running?)",
            socket.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let header = format!(
        "POST /v1/control HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|e| format!("write control request: {e}"))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| format!("read control response: {e}"))?;
    let body_start = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .ok_or_else(|| "invalid HTTP response from daemon".to_string())?;
    serde_json::from_slice::<ControlResponse>(&buf[body_start..]).map_err(|e| {
        format!(
            "parse control response: {e}: {}",
            String::from_utf8_lossy(&buf[body_start..])
                .chars()
                .take(200)
                .collect::<String>()
        )
    })
}

fn rpc_error(code: i64, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list_tools() {
        let init = handle_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            Path::new("/tmp/missing.sock"),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL);
        let listed = handle_message(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            Path::new("/tmp/missing.sock"),
        )
        .unwrap();
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "navi_status",
                "navi_get_settings",
                "navi_pause",
                "navi_resume",
                "navi_recent_context",
                "navi_search"
            ]
        );
    }

    #[test]
    fn search_without_query_is_invalid() {
        let resp = handle_message(
            &json!({
                "jsonrpc":"2.0",
                "id":3,
                "method":"tools/call",
                "params":{"name":"navi_search","arguments":{}}
            }),
            Path::new("/tmp/missing.sock"),
        )
        .unwrap();
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn missing_daemon_is_tool_error_not_rpc_crash() {
        let resp = handle_message(
            &json!({
                "jsonrpc":"2.0",
                "id":4,
                "method":"tools/call",
                "params":{"name":"navi_status","arguments":{}}
            }),
            Path::new("/tmp/lumen-navi-mcp-missing.sock"),
        )
        .unwrap();
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("daemon unavailable"));
    }

    #[test]
    fn notification_has_no_response() {
        assert!(handle_message(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            Path::new("/tmp/x"),
        )
        .is_none());
    }
}
